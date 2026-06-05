//! Filesystem-backed Certus gRPC Server
//!
//! Drop-in replacement for certus-server that uses local filesystem + standard
//! cudaMemcpy instead of SPDK NVMe + zero-copy DMA. Enables side-by-side
//! performance comparison using the same certus-api-bench.py benchmark.

mod memory_tier;
mod service;
mod storage;

use std::path::PathBuf;
use std::sync::Arc;

use clap::Parser;
use tonic::transport::{Identity, Server, ServerTlsConfig};

use service::FsDispatcherService;

/// Filesystem-backed Certus gRPC server for benchmarking comparison.
#[derive(Parser)]
#[command(
    name = "certus-server-fs",
    about = "Filesystem-backed Certus dispatcher gRPC server (baseline for benchmarking)"
)]
struct Cli {
    /// Directory for storing data files (created if absent).
    #[arg(long = "data-dir", default_value = "/tmp/certus-fs-data")]
    data_dir: String,

    /// gRPC listen address
    #[arg(long = "listen", default_value = "0.0.0.0:50051")]
    listen: String,

    /// Memory-tier pool size (e.g. 256M, 1G, 512K). Defaults to 2G.
    #[arg(long = "memory-tier-size", value_parser = parse_size, default_value = "2G")]
    memory_tier_size: usize,

    /// Format (clear) data directory on startup.
    #[arg(long = "format")]
    format: bool,

    /// Path to TLS certificate file (enables TLS when provided with --tls-key)
    #[arg(long = "tls-cert")]
    tls_cert: Option<String>,

    /// Path to TLS private key file (enables TLS when provided with --tls-cert)
    #[arg(long = "tls-key")]
    tls_key: Option<String>,

    /// Maximum staging buffer size for GPU↔host transfers (defaults to 16M).
    #[arg(long = "staging-size", value_parser = parse_size, default_value = "16M")]
    staging_size: usize,
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

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let cli = Cli::parse();

    let data_dir = PathBuf::from(&cli.data_dir);
    eprintln!("certus-server-fs: data-dir={}", data_dir.display());
    eprintln!(
        "certus-server-fs: memory-tier-size={} MiB",
        cli.memory_tier_size / (1024 * 1024)
    );
    eprintln!(
        "certus-server-fs: staging-size={} MiB",
        cli.staging_size / (1024 * 1024)
    );

    let fs_storage =
        Arc::new(storage::FsStorage::new(&data_dir).map_err(|e| format!("storage init: {e}"))?);

    if cli.format {
        eprintln!("certus-server-fs: formatting data directory...");
        fs_storage
            .format()
            .map_err(|e| format!("format failed: {e}"))?;
    }

    let mem_tier = Arc::new(memory_tier::MemoryTier::new(cli.memory_tier_size));

    eprintln!("certus-server-fs: initializing CUDA context...");
    // Initialize CUDA by setting device 0 (needed for IPC handle operations).
    let err = unsafe { gpu_services::cuda_ffi::cudaSetDevice(0) };
    if err != gpu_services::cuda_ffi::CUDA_SUCCESS {
        return Err(format!(
            "cudaSetDevice(0) failed: {}",
            gpu_services::cuda_ffi::cuda_error_string(err)
        )
        .into());
    }

    let svc = FsDispatcherService::new(fs_storage, mem_tier, cli.staging_size);
    let addr = cli.listen.parse()?;

    let mut server = Server::builder();
    if let (Some(cert_path), Some(key_path)) = (&cli.tls_cert, &cli.tls_key) {
        let cert = tokio::fs::read(cert_path).await?;
        let key = tokio::fs::read(key_path).await?;
        let identity = Identity::from_pem(cert, key);
        server = server.tls_config(ServerTlsConfig::new().identity(identity))?;
        eprintln!("certus-server-fs: TLS enabled");
    }

    eprintln!("certus-server-fs: listening on {addr}");

    server
        .add_service(service::dispatcher_server(svc))
        .serve_with_shutdown(addr, async {
            use tokio::signal::unix::{signal, SignalKind};
            let mut sigterm = signal(SignalKind::terminate()).expect("failed to register SIGTERM");
            tokio::select! {
                _ = tokio::signal::ctrl_c() => {},
                _ = sigterm.recv() => {},
            }
            eprintln!("certus-server-fs: shutting down...");
        })
        .await?;

    eprintln!("certus-server-fs: shutdown complete");
    Ok(())
}
