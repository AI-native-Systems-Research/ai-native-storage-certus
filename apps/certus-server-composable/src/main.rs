//! Certus Composable gRPC Server
//!
//! A runtime-configurable variant of certus-server that loads all components
//! as dynamic libraries based on a JSON configuration file. Exposes the
//! identical `certus.dispatcher.v1` gRPC API.

mod binder;
mod config;
mod loader;
mod resolver;
mod runtime;
mod service;
mod topology;

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use clap::Parser;
use tonic::transport::{Identity, Server, ServerTlsConfig};

use service::DispatcherService;

/// Certus composable gRPC server with dynamic component loading.
#[derive(Parser)]
#[command(
    name = "certus-server-composable",
    about = "Certus dispatcher gRPC server with runtime-configurable components"
)]
struct Cli {
    /// Path to JSON configuration file (mandatory).
    #[arg(long = "config")]
    config: String,

    /// gRPC listen address (overrides config).
    #[arg(long = "listen")]
    listen: Option<String>,

    /// PCI address(es) of NVMe device(s) (overrides config). Repeatable.
    #[arg(long = "device-pci")]
    device_pci: Vec<String>,

    /// Use the first N discovered NVMe drives (overrides config).
    #[arg(long = "drive-count", conflicts_with = "device_pci")]
    drive_count: Option<usize>,

    /// Memory-tier pool size (e.g., 256M, 1G). Overrides config.
    #[arg(long = "memory-tier-size")]
    memory_tier_size: Option<String>,

    /// Format extent managers on startup (overrides config).
    #[arg(long = "format")]
    format: bool,

    /// Path to TLS certificate file (overrides config).
    #[arg(long = "tls-cert")]
    tls_cert: Option<String>,

    /// Path to TLS private key file (overrides config).
    #[arg(long = "tls-key")]
    tls_key: Option<String>,

    /// Pin each NVMe poller thread to a dedicated CPU core (overrides config).
    #[arg(long = "poller-base-cpu")]
    poller_base_cpu: Option<usize>,
}

fn run() -> Result<(), Box<dyn std::error::Error>> {
    let cli = Cli::parse();

    // 1. Load and validate configuration.
    let config_path = PathBuf::from(&cli.config);
    eprintln!(
        "[certus-composable] loading config: {}",
        config_path.display()
    );
    let mut cfg = config::load_config(&config_path)?;
    config::validate_config(&cfg)?;

    // 2. Merge CLI overrides into server config.
    config::merge_cli_overrides(
        &mut cfg.server,
        cli.listen.as_deref(),
        &cli.device_pci,
        cli.drive_count,
        cli.memory_tier_size.as_deref(),
        cli.format,
        cli.tls_cert.as_deref(),
        cli.tls_key.as_deref(),
        cli.poller_base_cpu,
    );

    // 3. Resolve all dylib paths (verify accessibility before loading).
    let search_paths = resolver::build_search_paths(&cfg.search_paths);
    let resolved =
        resolver::resolve_all_dylibs(&cfg.components, &search_paths).map_err(|errs| {
            let msg = format!(
                "dylib resolution failed:\n{}",
                errs.iter()
                    .map(|e| format!("  - {e}"))
                    .collect::<Vec<_>>()
                    .join("\n")
            );
            Box::<dyn std::error::Error>::from(msg)
        })?;

    let resolved_map: HashMap<String, PathBuf> = resolved
        .into_iter()
        .map(|r| (r.component_name, r.path))
        .collect();

    eprintln!(
        "[certus-composable] all {} dylibs resolved successfully",
        resolved_map.len()
    );

    // 4. Initialize component stack (load, create, bind).
    let stack = runtime::initialize_stack(&cfg, &resolved_map)?;
    eprintln!(
        "[certus-composable] component stack initialized ({} instances)",
        stack.components.len()
    );

    // 5. Find the dispatcher component for gRPC service.
    let dispatcher_component = stack
        .components
        .iter()
        .find(|c| c.name == "dispatcher")
        .map(|c| c.component.attach())
        .ok_or("no component named 'dispatcher' found in stack")?;

    start_grpc_server(cfg.server, dispatcher_component, stack)
}

#[tokio::main]
async fn start_grpc_server(
    server_config: config::ServerConfig,
    dispatcher_component: component_core::component_ref::ComponentRef,
    stack: runtime::ComponentStack,
) -> Result<(), Box<dyn std::error::Error>> {
    let listen_addr = server_config.listen.as_deref().unwrap_or("0.0.0.0:50051");

    let svc = DispatcherService::new(dispatcher_component.clone());
    let addr = listen_addr.parse()?;

    let mut server = Server::builder();
    if let (Some(cert_path), Some(key_path)) = (&server_config.tls_cert, &server_config.tls_key) {
        let cert = tokio::fs::read(cert_path).await?;
        let key = tokio::fs::read(key_path).await?;
        let identity = Identity::from_pem(cert, key);
        server = server.tls_config(ServerTlsConfig::new().identity(identity))?;
        eprintln!("[certus-composable] TLS enabled");
    }

    eprintln!("[certus-composable] listening on {addr}");

    let shutdown_flag = Arc::new(AtomicBool::new(false));
    let flag_clone = Arc::clone(&shutdown_flag);

    server
        .add_service(service::dispatcher_server(svc))
        .serve_with_shutdown(addr, async move {
            use tokio::signal::unix::{signal, SignalKind};
            let mut sigterm = signal(SignalKind::terminate()).expect("failed to register SIGTERM");
            tokio::select! {
                _ = tokio::signal::ctrl_c() => {},
                _ = sigterm.recv() => {},
            }
            flag_clone.store(true, Ordering::Release);
            eprintln!("[certus-composable] shutting down...");
        })
        .await?;

    stack.shutdown();
    eprintln!("[certus-composable] shutdown complete");
    Ok(())
}

fn main() {
    if let Err(e) = run() {
        eprintln!("[certus-composable] FATAL: {e}");
        std::process::exit(1);
    }
}
