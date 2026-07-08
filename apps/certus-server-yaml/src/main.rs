//! Certus gRPC Server — YAML-Composed
//!
//! Drop-in replacement for certus-server whose component graph is
//! declared in a YAML profile manifest and assembled at compile time
//! by build.rs code generation.

mod config;
mod hooks;
mod metrics;
mod service;
#[cfg(feature = "otel")]
mod telemetry;

// Include the generated composition code (build_stack + ComponentStack).
include!(concat!(env!("OUT_DIR"), "/composition.rs"));

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use clap::Parser;
use tonic::transport::{Identity, Server, ServerTlsConfig};

use config::StackConfig;
use service::DispatcherService;

/// Certus gRPC server (YAML-composed) exposing the IDispatcher interface.
#[derive(Parser)]
#[command(
    name = "certus-server-yaml",
    about = "Certus dispatcher gRPC server — compile-time composed via YAML profiles"
)]
struct Cli {
    /// PCI address(es) of NVMe device(s) — may be specified multiple times.
    /// Mutually exclusive with --drive-count.
    #[arg(long = "device-pci")]
    device_pci: Vec<String>,

    /// Linux block device path(s) — may be specified multiple times.
    /// Use with the kernel block device backend (e.g., /dev/nvme0n1, /dev/md127).
    #[arg(long = "device-path")]
    device_path: Vec<String>,

    /// Use the first N discovered NVMe drives (alternative to --device-pci).
    #[arg(long = "drive-count", conflicts_with = "device_pci")]
    drive_count: Option<usize>,

    /// gRPC listen address
    #[arg(long = "listen", default_value = "0.0.0.0:50051")]
    listen: String,

    /// Memory-tier pool size (e.g. 256M, 1G, 512K). Defaults to 2G.
    #[arg(long = "memory-tier-size", value_parser = parse_size)]
    memory_tier_size: Option<usize>,

    /// Format extent managers on startup (destroys existing data).
    #[arg(long = "format")]
    format: bool,

    /// Path to TLS certificate file (enables TLS when provided with --tls-key)
    #[arg(long = "tls-cert")]
    tls_cert: Option<String>,

    /// Path to TLS private key file (enables TLS when provided with --tls-cert)
    #[arg(long = "tls-key")]
    tls_key: Option<String>,

    /// Pin each NVMe poller thread to a dedicated CPU core.
    #[arg(long = "poller-base-cpu")]
    poller_base_cpu: Option<usize>,

    /// Maximum eviction attempts before failing with pool-full error.
    #[arg(long = "max-eviction-attempts", default_value_t = 2048)]
    max_eviction_attempts: usize,

    /// Prometheus metrics HTTP port. Disabled by default; set > 0 to enable.
    #[arg(long = "metrics-port", default_value_t = 0)]
    metrics_port: u16,

    /// OTLP gRPC endpoint for metrics export (e.g. http://localhost:4317).
    /// Requires --features otel. Omit to disable.
    #[arg(long = "otel-endpoint")]
    otel_endpoint: Option<String>,

    /// OTel service name for this instance.
    #[arg(long = "otel-service-name", default_value = "certus-server-yaml")]
    otel_service_name: String,

    /// RDMA listener port for remote request handler (full-remote profile).
    /// Requires --features rdma. Set to 0 to disable.
    #[cfg(feature = "rdma")]
    #[arg(long = "rdma-port", default_value_t = 18515)]
    rdma_port: u16,
}

fn parse_size(s: &str) -> Result<usize, String> {
    let s = s.trim();
    if s.is_empty() {
        return Err("empty size string".into());
    }
    let (num_str, multiplier) = match s.as_bytes().last() {
        Some(b'K' | b'k') => (&s[..s.len() - 1], 1024usize),
        Some(b'M' | b'm') => (&s[..s.len() - 1], 1024 * 1024),
        Some(b'G' | b'g') => (&s[..s.len() - 1], 1024 * 1024 * 1024),
        _ => (s, 1usize),
    };
    let num: usize = num_str
        .parse()
        .map_err(|_| format!("invalid size number: '{num_str}'"))?;
    num.checked_mul(multiplier)
        .ok_or_else(|| format!("size overflow: '{s}'"))
}

fn validate_pci_address(addr: &str) -> Result<(), String> {
    let parts: Vec<&str> = addr.split(':').collect();
    if parts.len() != 3 {
        return Err(format!(
            "invalid PCI address format '{addr}': expected DDDD:BB:DD.F"
        ));
    }
    u32::from_str_radix(parts[0], 16).map_err(|_| format!("invalid PCI domain in '{addr}'"))?;
    u8::from_str_radix(parts[1], 16).map_err(|_| format!("invalid PCI bus in '{addr}'"))?;
    let dev_func: Vec<&str> = parts[2].split('.').collect();
    if dev_func.len() != 2 {
        return Err(format!("invalid PCI dev.func in '{addr}': expected DD.F"));
    }
    u8::from_str_radix(dev_func[0], 16).map_err(|_| format!("invalid PCI device in '{addr}'"))?;
    u8::from_str_radix(dev_func[1], 16).map_err(|_| format!("invalid PCI function in '{addr}'"))?;
    Ok(())
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let cli = Cli::parse();

    // Validate PCI addresses
    for addr in &cli.device_pci {
        validate_pci_address(addr).map_err(Box::<dyn std::error::Error>::from)?;
    }
    if cli.device_pci.is_empty() && cli.drive_count.is_none() && cli.device_path.is_empty() {
        return Err(
            "one of --device-pci, --drive-count, or --device-path must be specified".into(),
        );
    }

    const DEFAULT_MEMORY_TIER_SIZE: usize = 2 * 1024 * 1024 * 1024; // 2 GiB

    let stack_config = StackConfig {
        device_pci: cli.device_pci.clone(),
        device_paths: cli.device_path.clone(),
        drive_count: cli.drive_count,
        memory_tier_size: cli.memory_tier_size.unwrap_or(DEFAULT_MEMORY_TIER_SIZE),
        format: cli.format,
        poller_base_cpu: cli.poller_base_cpu,
        max_eviction_attempts: cli.max_eviction_attempts,
        resolved_pci_addrs: std::cell::RefCell::new(Vec::new()),
        resolved_numa_node: std::cell::RefCell::new(None),
    };

    // Build the component stack from the YAML-generated composition
    let stack = build_stack(&stack_config)?;

    let logger = &stack.logger;
    logger.info(&format!(
        "certus-server-yaml: composed from profile, devices={:?}",
        cli.device_pci
    ));
    logger.info(&format!(
        "certus-server-yaml: memory-tier-size={} MiB",
        stack_config.memory_tier_size / (1024 * 1024)
    ));

    // Start Prometheus metrics HTTP endpoint
    if cli.metrics_port > 0 {
        let mt = Arc::clone(&stack.memory_tier);
        let port = cli.metrics_port;
        tokio::spawn(metrics::serve_metrics(port, mt));
        logger.info(&format!(
            "certus-server-yaml: metrics endpoint on port {port}"
        ));
    }

    // Initialize OpenTelemetry OTLP metrics export
    #[cfg(feature = "otel")]
    let _otel_metrics = {
        if let Some(ref endpoint) = cli.otel_endpoint {
            let m = telemetry::OtelMetrics::init(
                endpoint,
                &cli.otel_service_name,
                Arc::clone(&stack.memory_tier),
            )
            .map_err(|e| format!("otel init failed: {e}"))?;
            logger.info(&format!(
                "certus-server-yaml: OTel metrics exporting to {endpoint}"
            ));
            Some(m)
        } else {
            None
        }
    };
    #[cfg(not(feature = "otel"))]
    if cli.otel_endpoint.is_some() {
        logger.warn(
            "certus-server-yaml: --otel-endpoint specified but binary not compiled with --features otel"
        );
    }

    // RDMA listener shutdown handle (used during graceful shutdown)
    #[cfg(feature = "rdma")]
    let mut rdma_shutdown_handle: Option<Arc<remote_request_handler::rdma::RdmaListener>> = None;

    // Start RDMA remote-request-handler listener in background (optional: port 0 = disabled)
    #[cfg(feature = "rdma")]
    {
        let rdma_port = cli.rdma_port;
        if rdma_port > 0 {
            logger.info("remote-request-handler: initializing");

            let dm_resolve = Arc::clone(&stack.dispatch_map);
            let dispatcher_resolve = Arc::clone(&stack.dispatcher);
            let resolver: Arc<remote_request_handler::serve::Resolver> = Arc::new(move |key| {
                #[allow(unused_imports)]
                use interfaces::{IDispatchMap, IDispatcher};
                match dm_resolve.lookup(key) {
                    Ok(interfaces::LookupResult::MemoryTier { pointer, size }) => {
                        Some(remote_request_handler::serve::ResolvedEntry {
                            ptr: pointer as *const u8,
                            size,
                        })
                    }
                    Ok(interfaces::LookupResult::BlockDevice { .. }) => {
                        // SSD-resident: release read ref, promote to memory-tier, re-lookup
                        let _ = dm_resolve.release_read(key);
                        dispatcher_resolve.promote_to_memory_tier(&[key]);
                        match dm_resolve.lookup(key) {
                            Ok(interfaces::LookupResult::MemoryTier { pointer, size }) => {
                                Some(remote_request_handler::serve::ResolvedEntry {
                                    ptr: pointer as *const u8,
                                    size,
                                })
                            }
                            _ => None,
                        }
                    }
                    _ => None,
                }
            });

            let dm_release = Arc::clone(&stack.dispatch_map);
            let release: Arc<remote_request_handler::serve::ReleaseCallback> =
                Arc::new(move |key| {
                    #[allow(unused_imports)]
                    use interfaces::IDispatchMap;
                    let _ = dm_release.release_read(key);
                });

            #[allow(unused_imports)]
            use interfaces::IMemoryTier;
            let pool = stack.memory_tier.pool_info().map(|(base, size)| {
                Arc::new(remote_request_handler::serve::PoolRegion { base, size })
            });

            let rdma_logger =
                Arc::clone(&stack.logger) as Arc<dyn interfaces::ILogger + Send + Sync>;

            let rdma_listener =
                remote_request_handler::serve::bind_listener("0.0.0.0", rdma_port, logger.as_ref())
                    .map_err(|e| format!("remote-request-handler: bind failed: {e}"))?;

            rdma_shutdown_handle = Some(Arc::clone(&rdma_listener));
            tokio::task::spawn_blocking(move || {
                remote_request_handler::serve::serve_loop(
                    &rdma_listener,
                    Some(resolver),
                    Some(release),
                    pool,
                    rdma_logger,
                );
            });
            logger.info(&format!(
                "certus-server-yaml: RDMA remote-request-handler on port {rdma_port}"
            ));
        } else {
            logger.info("certus-server-yaml: RDMA remote-request-handler disabled (port=0)");
        }
    }

    let svc = DispatcherService::new(Arc::clone(&stack.dispatcher));
    let addr = cli.listen.parse()?;

    // Build server with optional TLS
    let mut server = Server::builder();
    if let (Some(cert_path), Some(key_path)) = (&cli.tls_cert, &cli.tls_key) {
        let cert = tokio::fs::read(cert_path).await?;
        let key = tokio::fs::read(key_path).await?;
        let identity = Identity::from_pem(cert, key);
        server = server.tls_config(ServerTlsConfig::new().identity(identity))?;
        logger.info("certus-server-yaml: TLS enabled");
    }

    logger.info(&format!("certus-server-yaml: listening on {addr}"));

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
        })
        .await?;

    // Mask signals during shutdown to prevent a second Ctrl+C from killing the
    // process mid-teardown (which would segfault as SPDK memory is freed while
    // actor threads are still running).
    unsafe {
        libc::signal(libc::SIGINT, libc::SIG_IGN);
        libc::signal(libc::SIGTERM, libc::SIG_IGN);
    }

    // Shut down RDMA listener (unblocks the accept loop)
    #[cfg(feature = "rdma")]
    if let Some(ref handle) = rdma_shutdown_handle {
        logger.info("remote-request-handler: shutting down");
        handle.shutdown();
    }

    let _ = stack.dispatcher.shutdown();
    stack.spdk_env.fini();
    stack.logger.info("certus-server-yaml: shutdown complete");

    // Exit immediately to avoid blocking on tokio runtime drop waiting for
    // spawn_blocking tasks (RDMA serve_loop) that may still be tearing down.
    std::process::exit(0);
}
