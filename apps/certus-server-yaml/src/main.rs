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

use shmq_dispatcher::{log_cpu_bindings, serve, ServeConfig, Translator};

use config::StackConfig;
use metrics::{CountersObserver, ServiceCounters};

/// Set once by the SIGINT/SIGTERM handler; polled by the shmq poller and reaper.
static SHUTDOWN: AtomicBool = AtomicBool::new(false);

extern "C" fn handle_signal(_sig: libc::c_int) {
    // async-signal-safe: a single atomic store.
    SHUTDOWN.store(true, Ordering::SeqCst);
}

/// Format the cumulative KV-cache counters for the server log.
///
/// Two sources, because the line reports both directions of remote traffic and
/// they come from different components: tier movement and requester-side remote
/// results from the dispatcher, responder-side serve counts from remote-lookup.
///
/// **`from-peers` versus `to-peers` is the distinction this line exists to keep
/// straight.** `from-peers` is what this node *obtained* — its own hit rate.
/// `to-peers` is what this node *did for others* — work peers caused here. The
/// groups were named after the previous `remote[hits, misses]` proved ambiguous
/// about direction the moment a second remote-ish group joined it; a dashboard
/// reading the two backwards would draw the opposite conclusion about whether
/// remote lookup pays for itself.
///
/// **Parser contract, and its exact boundary.** `render_kvprofile.py`'s `TIER_RE`
/// scans `tier-events promotions[...] evictions[...]` and stops there — it matches
/// a prefix, not the whole line, so groups after `evictions[...]` may be added,
/// renamed or reordered freely. Verified 2026-09-29: nothing anywhere parses
/// `from-peers`, `to-peers` or `store`. Changing `promotions[...]` or
/// `evictions[...]` is the breaking edit; keep those in sync with `TIER_RE`.
///
/// `to-peers` reads `0, 0` on a node no peer has asked anything of, which is
/// honest rather than absent: the line has a fixed shape so a parser can rely on
/// it, and an omitted group would be a different contract.
fn format_cache_stats(
    s: &interfaces::TierEventStats,
    serve: &interfaces::RemoteServeStats,
) -> String {
    format!(
        "promotions[->memory {pm}, ->gpu {pg}]  evictions[memory {em}, ssd {es}]  \
         from-peers[hits {rh}, misses {rm}]  to-peers[served {ps}, own-disk {pp}]  \
         store[backpressure {sb}, drops-on-full {sd}]",
        pm = s.promotions_to_memory,
        pg = s.promotions_to_gpu,
        em = s.evictions_from_memory,
        es = s.evictions_from_ssd,
        rh = s.remote_lookup_hits,
        rm = s.remote_lookup_misses,
        ps = serve.peer_served_keys,
        pp = serve.peer_triggered_promotions,
        sb = s.store_backpressure_events,
        sd = s.store_drops_on_full,
    )
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

    /// Log periodic shm-queue poller fairness/backlog stats (per-channel
    /// serviced counts, low/high channel split, worker-queue depth). Diagnostic
    /// only; off by default.
    #[arg(long = "shmq-poller-stats")]
    shmq_poller_stats: bool,

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

    /// Milliseconds a store allocation backpressures on a momentarily full
    /// memory tier (retrying eviction while the background evictor drains)
    /// before surfacing AllocationFailed. 0 disables (fail fast).
    #[arg(long = "store-backpressure-ms", default_value_t = 5000)]
    store_backpressure_ms: u64,

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
        store_backpressure_ms: cli.store_backpressure_ms,
        memory_tier_eviction_threshold: cli.memory_tier_eviction_threshold,
        rl_group: cli.rl_group.clone(),
        resolved_pci_addrs: std::cell::RefCell::new(Vec::new()),
        resolved_numa_node: std::cell::RefCell::new(None),
    };

    // Build the component stack from the YAML-generated composition
    let stack = build_stack(&stack_config)?;

    let logger = Arc::clone(&stack.logger);
    logger.info(&format!(
        "certus-server-yaml: composed from profile '{}', devices={:?}",
        PROFILE_NAME, cli.device_pci
    ));
    logger.info(&format!(
        "certus-server-yaml: eviction policy = {} (algorithm: {})",
        EVICTION_POLICY_CRATE, EVICTION_POLICY_ALGORITHM
    ));
    logger.info(&format!(
        "certus-server-yaml: memory-tier-size={} MiB",
        stack_config.memory_tier_size / (1024 * 1024)
    ));
    if cli.format {
        logger.info("certus-server-yaml: --format specified, extent managers will be reformatted");
    }

    // Report the CPU cores the component threads (NVMe pollers, remote-lookup
    // actor, ...) were bound to during stack initialization.
    log_cpu_bindings(logger.as_ref(), "certus-server-yaml");
    match cli.shmq_poller_cpu {
        Some(cpu) => logger.info(&format!(
            "certus-server-yaml: shmq poller will bind to CPU {cpu}"
        )),
        None => logger.info("certus-server-yaml: shmq poller not pinned (use --shmq-poller-cpu)"),
    }

    let counters = ServiceCounters::new();

    // Start Prometheus metrics HTTP endpoint
    if cli.metrics_port > 0 {
        let mt = Arc::clone(&stack.memory_tier);
        let disp = Arc::clone(&stack.dispatcher);
        let rl = Arc::clone(&stack.remote_lookup);
        let port = cli.metrics_port;
        tokio::spawn(metrics::serve_metrics(port, mt, disp, rl, counters.clone()));
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
                Arc::clone(&stack.remote_lookup),
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
        // Total OP_RESERVE-batch backpressure budget, shared across all keys in
        // the batch (see Translator::op_reserve). Sourced from the same knob the
        // dispatcher uses per-call so a saturated tier does not overrun the
        // client's ring deadline.
        std::time::Duration::from_millis(cli.store_backpressure_ms),
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

    // Always-on KV-cache telemetry: a periodic "tier-events" line (cumulative
    // counts, every ~2s) that the kvprofile renderer parses into the
    // promotions/evictions series. The dispatcher's tier counters are always
    // present. (SSD read/write bytes are not logged here — the client queries
    // them over the shmq ring via GetIoStats, mirroring the old gRPC path.)
    //
    // The line also carries `to-peers`, from remote-lookup rather than the
    // dispatcher, so the pressure peers place on this node's disk can be read
    // against `store[drops-on-full]` **in the same tick**. That correlation over
    // time is the reason it is logged periodically at all and not only in the FINAL
    // summary: the open question is whether serving peers is what makes local
    // stores fail, and a per-run total cannot answer it.
    // Exits when SHUTDOWN flips; the process exits before any join is needed.
    {
        let tier_disp = Arc::clone(&stack.dispatcher);
        let tier_rl = Arc::clone(&stack.remote_lookup);
        let tier_logger = Arc::clone(&logger);
        std::thread::Builder::new()
            .name("tier-events".into())
            .spawn(move || {
                while !SHUTDOWN.load(Ordering::Relaxed) {
                    // Sleep in small steps so shutdown is responsive.
                    for _ in 0..8 {
                        if SHUTDOWN.load(Ordering::Relaxed) {
                            return;
                        }
                        std::thread::sleep(Duration::from_millis(250));
                    }
                    tier_logger.info(&format!(
                        "certus-server-yaml: tier-events {}",
                        format_cache_stats(&tier_disp.tier_event_stats(), &tier_rl.serve_stats(),)
                    ));
                }
            })
            .expect("failed to spawn tier-events telemetry thread");
    }

    // Run the shared poller + worker-pool + reaper loop; blocks until SHUTDOWN.
    // Offloaded to a blocking thread so the tokio runtime keeps driving the
    // metrics HTTP endpoint and OTel export.
    let serve_logger = Arc::clone(&logger);
    let serve_config = ServeConfig {
        channels: cli.channels,
        reserve_timeout: Duration::from_secs(cli.reserve_timeout_secs),
        poller_cpu: cli.shmq_poller_cpu,
        poller_stats: cli.shmq_poller_stats,
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

    // Always-on: the cumulative KV-cache tier-movement counts for this run,
    // logged before teardown while the dispatcher's counters are still live.
    logger.info(&format!(
        "certus-server-yaml: FINAL tier-events {}",
        format_cache_stats(
            &stack.dispatcher.tier_event_stats(),
            &stack.remote_lookup.serve_stats(),
        )
    ));

    let _ = stack.dispatcher.shutdown();
    stack.spdk_env.fini();
    logger.info("certus-server-yaml: shutdown complete");

    // Exit immediately rather than waiting on the tokio runtime / SPDK teardown.
    std::process::exit(0);
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The kvprofile parser contract: `promotions[...] evictions[...]` must keep its
    /// exact shape, and everything after it must stay free to change.
    ///
    /// `render_kvprofile.py`'s `TIER_RE` matches that prefix and stops, so this test
    /// guards the half that is a contract without freezing the half that is not.
    /// Written as a literal prefix rather than a regex because the literal is what a
    /// reader must compare against `TIER_RE` by eye when editing either side.
    ///
    /// Verified 2026-09-29 against the real regex read out of `render_kvprofile.py`:
    /// the pre- and post-rename lines produce identical captures.
    #[test]
    fn the_parsed_prefix_of_the_log_line_is_unchanged() {
        let tier = interfaces::TierEventStats {
            promotions_to_memory: 812,
            promotions_to_gpu: 4401,
            evictions_from_memory: 77,
            evictions_from_ssd: 12,
            ..Default::default()
        };
        let line = format_cache_stats(&tier, &interfaces::RemoteServeStats::default());

        assert!(
            line.starts_with(
                "promotions[->memory 812, ->gpu 4401]  evictions[memory 77, ssd 12]  "
            ),
            "the kvprofile parser scans this prefix; it changed:\n  {line}"
        );
    }

    /// The two directions of remote traffic must stay distinguishable in the log.
    ///
    /// **Must be shown to fail** if the two groups are swapped, which is the whole
    /// reason they stopped being called `remote[...]`: a reader who takes
    /// "what peers served us" for "what we served peers" draws the opposite
    /// conclusion about whether remote lookup earns its cost.
    #[test]
    fn the_log_line_says_which_direction_each_count_is() {
        let tier = interfaces::TierEventStats {
            remote_lookup_hits: 312,
            remote_lookup_misses: 166_405,
            ..Default::default()
        };
        let serve = interfaces::RemoteServeStats {
            peer_served_keys: 118,
            peer_triggered_promotions: 41,
            peer_pins_held: 0,
            peer_pins_taken: 0,
            peer_pin_hold_us_total: 0,
            peer_pin_hold_us_max: 0,
        };
        let line = format_cache_stats(&tier, &serve);

        assert!(
            line.contains("from-peers[hits 312, misses 166405]"),
            "requester-side counts must be labelled from-peers:\n  {line}"
        );
        assert!(
            line.contains("to-peers[served 118, own-disk 41]"),
            "responder-side counts must be labelled to-peers:\n  {line}"
        );
        assert!(
            line.find("from-peers").unwrap() < line.find("to-peers").unwrap(),
            "obtained-then-provided ordering is what makes the pair readable:\n  {line}"
        );
    }
}
