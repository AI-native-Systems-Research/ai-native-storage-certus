//! Certus dispatcher server — YAML-Composed
//!
//! Same control-plane transport as `certus-server` (the lock-free `/dev/shm`
//! mailbox from the `shm-queue` crate, driven by the shared `shmq-dispatcher`
//! serve loop), but the component graph is declared in a YAML profile manifest
//! and assembled at compile time by build.rs code generation.
//!
//! Unlike the plain server this binary also runs a tokio runtime for the
//! optional Prometheus metrics endpoint and OpenTelemetry OTLP export; the
//! blocking shmq serve loop runs on a dedicated worker via `spawn_blocking`
//! while those async tasks stay live.

mod config;
mod hooks;
mod metrics;
#[cfg(feature = "otel")]
mod telemetry;

// Include the generated composition code (build_stack + ComponentStack).
include!(concat!(env!("OUT_DIR"), "/composition.rs"));

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;

use clap::Parser;

use shmq_dispatcher::{serve, ServeConfig, Translator};

use config::StackConfig;
use metrics::{CountersObserver, ServiceCounters};

/// Set once by the SIGINT/SIGTERM handler; polled by the shmq poller and reaper.
static SHUTDOWN: AtomicBool = AtomicBool::new(false);

extern "C" fn handle_signal(_sig: libc::c_int) {
    // async-signal-safe: a single atomic store.
    SHUTDOWN.store(true, Ordering::SeqCst);
}

/// Certus dispatcher server (YAML-composed) over a /dev/shm mailbox.
#[derive(Parser)]
#[command(
    name = "certus-server-yaml",
    about = "Certus dispatcher server over a /dev/shm mailbox — compile-time composed via YAML profiles"
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

    /// Path to the shared-memory mailbox file (created/truncated on start).
    #[arg(long = "shm-path", default_value = "/dev/shm/certus-shmq")]
    shm_path: String,

    /// Number of mailbox channels (= max in-flight requests = worker threads).
    #[arg(long = "channels", default_value_t = 8)]
    channels: usize,

    /// Per-channel request capacity in bytes (K/M/G suffixes accepted).
    #[arg(long = "cap-req", value_parser = parse_size, default_value = "1M")]
    cap_req: usize,

    /// Per-channel response capacity in bytes (K/M/G suffixes accepted).
    #[arg(long = "cap-resp", value_parser = parse_size, default_value = "128K")]
    cap_resp: usize,

    /// Reclaim reservations left uncommitted/unaborted for this many seconds.
    #[arg(long = "reserve-timeout-secs", default_value_t = 30)]
    reserve_timeout_secs: u64,

    /// Pin the shm-queue poller thread to this CPU core (optional). Choose a
    /// core outside the NVMe poller range (see --poller-base-cpu).
    #[arg(long = "shmq-poller-cpu")]
    shmq_poller_cpu: Option<usize>,

    /// Memory-tier pool size (e.g. 256M, 1G, 512K). Defaults to 2G.
    #[arg(long = "memory-tier-size", value_parser = parse_size)]
    memory_tier_size: Option<usize>,

    /// Format extent managers on startup (destroys existing data).
    #[arg(long = "format")]
    format: bool,

    /// Pin each NVMe poller thread to a dedicated CPU core.
    #[arg(long = "poller-base-cpu")]
    poller_base_cpu: Option<usize>,

    /// Maximum eviction attempts before failing with pool-full error.
    #[arg(long = "max-eviction-attempts", default_value_t = 2048)]
    max_eviction_attempts: usize,

    /// Memory-tier utilization threshold (0.0–1.0) for background DRAM→SSD demotion.
    /// Disabled by default (0.0). Set to e.g. 0.8 to start demoting at 80% full.
    #[arg(long = "memory-tier-eviction-threshold", default_value_t = 0.0)]
    memory_tier_eviction_threshold: f64,

    /// Prometheus metrics HTTP port. Disabled by default; set > 0 to enable.
    #[arg(long = "metrics-port", default_value_t = 0)]
    metrics_port: u16,

    /// OTLP HTTP endpoint for metrics export (e.g. http://localhost:4318).
    /// Requires --features otel. Omit to disable.
    #[arg(long = "otel-endpoint")]
    otel_endpoint: Option<String>,

    /// OTel service name for this instance.
    #[arg(long = "otel-service-name", default_value = "certus-server-yaml")]
    otel_service_name: String,

    /// zyre group (cluster name) for remote-lookup clustering. Defaults to a
    /// unique random group per process, so an unconfigured node forms its own
    /// single-node cluster and does not interfere with other users' nodes. Set
    /// the same value on every node that should share a cluster. Env fallback:
    /// CERTUS_RL_GROUP (this flag takes precedence).
    #[arg(long = "rl-group", env = "CERTUS_RL_GROUP")]
    rl_group: Option<String>,
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
        memory_tier_eviction_threshold: cli.memory_tier_eviction_threshold,
        rl_group: cli.rl_group.clone(),
        resolved_pci_addrs: std::cell::RefCell::new(Vec::new()),
        resolved_numa_node: std::cell::RefCell::new(None),
    };

    // Build the component stack from the YAML-generated composition
    let stack = build_stack(&stack_config)?;

    let logger = Arc::clone(&stack.logger);
    logger.info(&format!(
        "certus-server-yaml: composed from profile, devices={:?}",
        cli.device_pci
    ));
    logger.info(&format!(
        "certus-server-yaml: memory-tier-size={} MiB",
        stack_config.memory_tier_size / (1024 * 1024)
    ));
    if cli.format {
        logger.info("certus-server-yaml: --format specified, extent managers will be reformatted");
    }

    let counters = ServiceCounters::new();

    // Start Prometheus metrics HTTP endpoint
    if cli.metrics_port > 0 {
        let mt = Arc::clone(&stack.memory_tier);
        let disp = Arc::clone(&stack.dispatcher);
        let port = cli.metrics_port;
        tokio::spawn(metrics::serve_metrics(port, mt, disp, counters.clone()));
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
                Arc::clone(&stack.dispatcher),
                counters.clone(),
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

    // The remote-lookup-rdma-initiator component (full-remote profile) is instantiated
    // and wired by the generated composition; it is driven by remote-lookup, not
    // directly by this binary. It maintains its own outbound RDMA connections and
    // needs no listener here.

    // Build the opcode→IDispatcher translator, wiring the metrics observer so
    // the Prometheus/OTel counters survive the transport switch.
    let translator = Translator::new(
        Arc::clone(&stack.dispatcher),
        stack.eviction_rx.clone(),
        Arc::clone(&stack.eviction_dropped),
    )
    .with_observer(Arc::new(CountersObserver::new(counters)));

    // Create the shared-memory mailbox.
    let server = Arc::new(shm_queue::Server::create(
        &cli.shm_path,
        cli.channels,
        cli.cap_req,
        cli.cap_resp,
    )?);
    logger.info(&format!(
        "certus-server-yaml: shared-memory IPC path {} channels={} cap_req={} cap_resp={}",
        cli.shm_path,
        server.channel_count(),
        server.cap_req(),
        server.cap_resp()
    ));

    // Install SIGINT/SIGTERM handlers → flip SHUTDOWN. This replaces tokio's
    // signal handling; the blocking serve loop polls SHUTDOWN directly.
    // SAFETY: handle_signal is async-signal-safe (single atomic store).
    unsafe {
        libc::signal(libc::SIGINT, handle_signal as libc::sighandler_t);
        libc::signal(libc::SIGTERM, handle_signal as libc::sighandler_t);
    }

    // Run the shared poller + worker-pool + reaper loop; blocks until SHUTDOWN.
    // Offloaded to a blocking thread so the tokio runtime keeps driving the
    // metrics HTTP endpoint and OTel export.
    let serve_logger = Arc::clone(&logger);
    let serve_config = ServeConfig {
        channels: cli.channels,
        reserve_timeout: Duration::from_secs(cli.reserve_timeout_secs),
        poller_cpu: cli.shmq_poller_cpu,
    };
    tokio::task::spawn_blocking(move || {
        serve(server, translator, serve_config, &SHUTDOWN, serve_logger)
    })
    .await
    .map_err(|e| format!("serve task join error: {e}"))??;

    // Mask signals during shutdown to prevent a second Ctrl+C from killing the
    // process mid-teardown (which would segfault as SPDK memory is freed while
    // actor threads are still running).
    unsafe {
        libc::signal(libc::SIGINT, libc::SIG_IGN);
        libc::signal(libc::SIGTERM, libc::SIG_IGN);
    }

    let _ = stack.dispatcher.shutdown();
    stack.spdk_env.fini();
    logger.info("certus-server-yaml: shutdown complete");

    // Exit immediately rather than waiting on the tokio runtime / SPDK teardown.
    std::process::exit(0);
}
