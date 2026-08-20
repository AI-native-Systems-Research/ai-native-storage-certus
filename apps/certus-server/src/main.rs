//! Certus dispatcher server.
//!
//! The control-plane transport is the lock-free `/dev/shm` mailbox from the
//! `shm-queue` crate; the poller/worker/reaper serve loop and the
//! opcode→`IDispatcher` translation live in the shared `shmq-dispatcher` crate.
//! The KV bytes move GPU↔DRAM↔SSD out of band via CUDA/SPDK DMA — only the
//! small control messages travel the mailbox.
//!
//! This binary owns the process-level concerns: CLI parsing, building the
//! component stack, installing signal handlers, the optional SSD-telemetry
//! thread, and driving [`shmq_dispatcher::serve`] until shutdown.

use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Duration;
#[cfg(feature = "rw-telemetry")]
use std::thread;

use clap::Parser;

use component_core::query_interface;
use interfaces::{
    DispatcherConfig, IDispatchMap, IDispatcher, IEvictionPolicy, IGpuServices, ILogger,
    IMemoryTier, IRemoteLookup, PciAddress,
};

use shmq_dispatcher::{serve, ServeConfig, Translator};

/// Set once by the SIGINT/SIGTERM handler; polled by the poller and reaper.
static SHUTDOWN: AtomicBool = AtomicBool::new(false);

extern "C" fn handle_signal(_sig: libc::c_int) {
    // async-signal-safe: a single relaxed atomic store.
    SHUTDOWN.store(true, Ordering::SeqCst);
}

/// How often the telemetry thread logs cumulative SSD I/O stats (rw-telemetry
/// builds only).
#[cfg(feature = "rw-telemetry")]
const TELEMETRY_INTERVAL_SECS: u64 = 10;

/// Render a byte count as a compact human-readable size (`4KiB`, `128KiB`,
/// `2MiB`, ...). Used to label transfer-size histogram buckets.
#[cfg(feature = "rw-telemetry")]
fn human_size(bytes: u64) -> String {
    const UNITS: [(&str, u64); 4] = [
        ("GiB", 1 << 30),
        ("MiB", 1 << 20),
        ("KiB", 1 << 10),
        ("B", 1),
    ];
    for (name, scale) in UNITS {
        if bytes >= scale {
            return format!("{}{}", bytes / scale, name);
        }
    }
    "0B".to_string()
}

/// Render one direction's transfer-size histogram as `lo-hi:count` bins over the
/// non-empty buckets (e.g. `[4KiB,8KiB):12 [128KiB,256KiB):340 >=8MiB:1024`).
/// The final bucket is open-ended (sizes are clamped into it).
#[cfg(feature = "rw-telemetry")]
fn format_size_hist(buckets: &[u64; interfaces::IO_SIZE_BUCKETS]) -> String {
    use interfaces::{ReadWriteStats, IO_SIZE_BUCKETS};
    let mut parts = Vec::new();
    for (idx, &count) in buckets.iter().enumerate() {
        if count == 0 {
            continue;
        }
        let lo = ReadWriteStats::bucket_lower_bound(idx);
        let label = if idx == IO_SIZE_BUCKETS - 1 {
            format!(">={}", human_size(lo))
        } else {
            format!("[{},{})", human_size(lo), human_size(1u64 << idx))
        };
        parts.push(format!("{label}:{count}"));
    }
    if parts.is_empty() {
        "(none)".to_string()
    } else {
        parts.join(" ")
    }
}

/// Format a cumulative SSD read/write telemetry snapshot for the server log:
/// op counts, GiB moved, mean transfer size (bytes/op = the effective on-device
/// I/O block size), mean per-op latency, and the full per-direction transfer-size
/// distribution. Only compiled with `rw-telemetry`; without that feature
/// `read_write_stats()` returns all-zero counters.
#[cfg(feature = "rw-telemetry")]
fn format_io_stats(s: &interfaces::ReadWriteStats) -> String {
    let gib = |b: u64| b as f64 / (1u64 << 30) as f64;
    let per = |num: u64, den: u64| if den > 0 { num / den } else { 0 };
    format!(
        "reads[{ro} ops, {rg:.3} GiB, {rbs} B/op, {rl} us/op]  \
         writes[{wo} ops, {wg:.3} GiB, {wbs} B/op, {wl} us/op]\n    \
         read-sizes:  {rh}\n    write-sizes: {wh}",
        ro = s.read_ops,
        rg = gib(s.read_bytes),
        rbs = per(s.read_bytes, s.read_ops),
        rl = per(s.read_latency_ns_sum / 1000, s.read_ops),
        wo = s.write_ops,
        wg = gib(s.write_bytes),
        wbs = per(s.write_bytes, s.write_ops),
        wl = per(s.write_latency_ns_sum / 1000, s.write_ops),
        rh = format_size_hist(&s.read_size_buckets),
        wh = format_size_hist(&s.write_size_buckets),
    )
}

/// Certus shared-memory-queue server exposing the IDispatcher control plane.
#[derive(Parser)]
#[command(
    name = "certus-server",
    about = "Certus dispatcher server over a /dev/shm mailbox"
)]
struct Cli {
    /// PCI address(es) of NVMe device(s) — may be specified multiple times.
    /// Mutually exclusive with --drive-count.
    #[arg(long = "device-pci")]
    device_pci: Vec<String>,

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

    /// Pin each NVMe poller thread to a dedicated CPU core (drive N → base+N).
    #[arg(long = "poller-base-cpu")]
    poller_base_cpu: Option<usize>,

    /// Maximum eviction attempts before failing with pool-full error.
    #[arg(long = "max-eviction-attempts", default_value_t = 2048)]
    max_eviction_attempts: usize,

    /// Memory-tier utilization threshold (0.0–1.0) for background DRAM→SSD demotion.
    #[arg(long = "memory-tier-eviction-threshold", default_value_t = 0.0)]
    memory_tier_eviction_threshold: f64,
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
    parse_pci_address(addr)?;
    Ok(())
}

fn parse_pci_address(addr: &str) -> Result<PciAddress, String> {
    let parts: Vec<&str> = addr.split(':').collect();
    if parts.len() != 3 {
        return Err(format!(
            "invalid PCI address format '{addr}': expected DDDD:BB:DD.F"
        ));
    }
    let domain =
        u32::from_str_radix(parts[0], 16).map_err(|_| format!("invalid PCI domain in '{addr}'"))?;
    let bus =
        u8::from_str_radix(parts[1], 16).map_err(|_| format!("invalid PCI bus in '{addr}'"))?;
    let dev_func: Vec<&str> = parts[2].split('.').collect();
    if dev_func.len() != 2 {
        return Err(format!("invalid PCI dev.func in '{addr}': expected DD.F"));
    }
    let dev = u8::from_str_radix(dev_func[0], 16)
        .map_err(|_| format!("invalid PCI device in '{addr}'"))?;
    let func = u8::from_str_radix(dev_func[1], 16)
        .map_err(|_| format!("invalid PCI function in '{addr}'"))?;
    Ok(PciAddress {
        domain,
        bus,
        dev,
        func,
    })
}

fn resolve_device_addresses(cli: &Cli) -> Result<Vec<String>, String> {
    if !cli.device_pci.is_empty() {
        for addr in &cli.device_pci {
            validate_pci_address(addr)?;
        }
        Ok(cli.device_pci.clone())
    } else if cli.drive_count.is_some() {
        Ok(Vec::new()) // resolved after SPDK init
    } else {
        Err("either --device-pci or --drive-count must be specified".into())
    }
}

/// Builds the full component stack and returns the dispatcher facade plus the
/// dispatcher component (for the eviction channel).
#[allow(clippy::type_complexity)]
fn initialize_component_stack(
    device_pci_addrs: &[String],
    drive_count: Option<usize>,
    memory_tier_size: usize,
    format: bool,
    poller_base_cpu: Option<usize>,
    max_eviction_attempts: usize,
    memory_tier_eviction_threshold: f64,
) -> Result<
    (
        Arc<dyn IDispatcher + Send + Sync>,
        Arc<dyn ILogger + Send + Sync>,
        Vec<String>,
        Arc<dispatcher::DispatcherComponent>,
    ),
    String,
> {
    let logger: Arc<dyn ILogger + Send + Sync> = logger::LoggerComponent::new_default();

    logger.info("certus-server: initializing SPDK environment...");
    let spdk_comp = spdk_env::SPDKEnvComponent::new_default();
    let spdk_iface =
        query_interface!(spdk_comp, spdk_env::ISPDKEnv).ok_or("failed to query ISPDKEnv")?;
    spdk_iface
        .init()
        .map_err(|e| format!("SPDK init failed: {e}"))?;

    // NVMe PCI class code: 0x010802 (Mass Storage Controller, NVM Express).
    const NVME_CLASS_CODE: u32 = 0x010802;
    let device_pci_addrs = if device_pci_addrs.is_empty() {
        let count = drive_count.unwrap_or(1);
        let devices = spdk_iface.devices();
        let mut nvme_devices: Vec<_> = devices
            .iter()
            .filter(|d| d.id.class_id == NVME_CLASS_CODE)
            .collect();
        nvme_devices.sort_by_key(|d| if d.numa_node == 0 { 0 } else { 1 });
        if nvme_devices.len() < count {
            return Err(format!(
                "--drive-count={count} but only {} NVMe device(s) discovered",
                nvme_devices.len()
            ));
        }
        let addrs: Vec<String> = nvme_devices[..count]
            .iter()
            .map(|d| d.address.to_string())
            .collect();
        logger.info(&format!(
            "certus-server: auto-selected {count} drive(s): {addrs:?}"
        ));
        addrs
    } else {
        device_pci_addrs.to_vec()
    };

    logger.info("certus-server: initializing GPU services...");
    let gpu_comp = gpu_services::GpuServicesComponent::new_default();
    gpu_comp
        .logger
        .connect(Arc::clone(&logger))
        .map_err(|e| format!("gpu logger bind: {e}"))?;
    let gpu: Arc<dyn IGpuServices + Send + Sync> =
        query_interface!(gpu_comp, IGpuServices).ok_or("failed to query IGpuServices")?;
    gpu.initialize()
        .map_err(|e| format!("GPU init failed: {e}"))?;

    let ep_comp = eviction_policy_lru::EvictionPolicyLruComponent::new_default();
    ep_comp
        .logger
        .connect(Arc::clone(&logger))
        .map_err(|e| format!("eviction-policy logger bind: {e}"))?;
    let eviction_policy: Arc<dyn IEvictionPolicy + Send + Sync> =
        query_interface!(ep_comp, IEvictionPolicy).ok_or("failed to query IEvictionPolicy")?;

    logger.info("certus-server: initializing dispatch map...");
    let dm_comp =
        dispatch_map::DispatchMapComponent::new(dispatch_map::DispatchMapState::default());
    dm_comp
        .logger
        .connect(Arc::clone(&logger))
        .map_err(|e| format!("dispatch map logger bind: {e}"))?;
    dm_comp
        .eviction_policy
        .connect(Arc::clone(&eviction_policy))
        .map_err(|e| format!("dispatch map eviction_policy bind: {e}"))?;
    let dm: Arc<dyn IDispatchMap + Send + Sync> =
        query_interface!(dm_comp, IDispatchMap).ok_or("failed to query IDispatchMap")?;
    dm.initialize()
        .map_err(|e| format!("DispatchMap init failed: {e}"))?;

    logger.info("certus-server: initializing memory-tier...");
    let mt_comp = memory_tier::MemoryTierComponent::new_default();
    mt_comp
        .logger
        .connect(Arc::clone(&logger))
        .map_err(|e| format!("memory-tier logger bind: {e}"))?;
    mt_comp
        .eviction_policy
        .connect(Arc::clone(&eviction_policy))
        .map_err(|e| format!("memory-tier eviction_policy bind: {e}"))?;
    let mt: Arc<dyn IMemoryTier + Send + Sync> =
        query_interface!(mt_comp, IMemoryTier).ok_or("failed to query IMemoryTier")?;

    let mt_numa_node: Option<i32> = device_pci_addrs.first().and_then(|first_addr| {
        spdk_iface
            .devices()
            .iter()
            .find(|d| d.address.to_string() == *first_addr)
            .map(|d| d.numa_node)
    });
    mt.initialize(memory_tier_size, mt_numa_node)
        .map_err(|e| format!("MemoryTier init failed: {e}"))?;

    if let Some((pool_ptr, pool_size)) = mt.pool_info() {
        let err = unsafe {
            gpu_services::cuda_ffi::cudaHostRegister(
                pool_ptr as *mut std::ffi::c_void,
                pool_size,
                0,
            )
        };
        if err != gpu_services::cuda_ffi::CUDA_SUCCESS {
            logger.warn(&format!(
                "certus-server: cudaHostRegister failed (err={err}), \
                 memory-tier transfers will use staged path"
            ));
        } else {
            logger.info(&format!(
                "certus-server: memory-tier pool registered with CUDA ({} MiB pinned)",
                pool_size / (1024 * 1024)
            ));
        }
    }

    let rl_comp = remote_lookup::RemoteLookupComponent::new_default();
    rl_comp
        .logger
        .connect(Arc::clone(&logger))
        .map_err(|e| format!("failed to bind remote_lookup logger: {e}"))?;
    let remote_lookup: Arc<dyn IRemoteLookup + Send + Sync> =
        query_interface!(rl_comp, IRemoteLookup).ok_or("failed to query IRemoteLookup")?;

    logger.info("certus-server: initializing dispatcher...");
    let disp_comp = dispatcher::DispatcherComponent::new_default();
    disp_comp
        .dispatch_map
        .connect(Arc::clone(&dm))
        .map_err(|e| format!("failed to bind dispatch_map: {e}"))?;
    disp_comp
        .memory_tier
        .connect(Arc::clone(&mt))
        .map_err(|e| format!("failed to bind memory_tier: {e}"))?;
    disp_comp
        .gpu_services
        .connect(Arc::clone(&gpu))
        .map_err(|e| format!("failed to bind gpu_services: {e}"))?;
    disp_comp
        .spdk_env
        .connect(Arc::clone(&spdk_iface))
        .map_err(|e| format!("failed to bind spdk_env: {e}"))?;
    disp_comp
        .logger
        .connect(Arc::clone(&logger))
        .map_err(|e| format!("failed to bind logger: {e}"))?;
    disp_comp
        .remote_lookup
        .connect(Arc::clone(&remote_lookup))
        .map_err(|e| format!("failed to bind remote_lookup: {e}"))?;

    let dispatcher: Arc<dyn IDispatcher + Send + Sync> =
        query_interface!(disp_comp, IDispatcher).ok_or("failed to query IDispatcher")?;

    dispatcher
        .initialize(DispatcherConfig {
            data_pci_addrs: device_pci_addrs.clone(),
            format_on_init: format,
            poller_base_cpu,
            max_eviction_attempts,
            memory_tier_eviction_threshold,
            ..Default::default()
        })
        .map_err(|e| format!("Dispatcher init failed: {e}"))?;

    logger.info("certus-server: component stack initialized");
    Ok((dispatcher, logger, device_pci_addrs, disp_comp))
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let cli = Cli::parse();

    let device_pci = resolve_device_addresses(&cli).map_err(Box::<dyn std::error::Error>::from)?;

    const DEFAULT_MEMORY_TIER_SIZE: usize = 2 * 1024 * 1024 * 1024; // 2 GiB
    let pool_size = cli.memory_tier_size.unwrap_or(DEFAULT_MEMORY_TIER_SIZE);
    let (dispatcher, logger, device_pci, disp_comp) = initialize_component_stack(
        &device_pci,
        cli.drive_count,
        pool_size,
        cli.format,
        cli.poller_base_cpu,
        cli.max_eviction_attempts,
        cli.memory_tier_eviction_threshold,
    )?;

    logger.info(&format!("certus-server: devices={device_pci:?}"));
    logger.info(&format!(
        "certus-server: memory-tier-size={} MiB",
        pool_size / (1024 * 1024)
    ));
    if cli.format {
        logger.info("certus-server: --format specified, extent managers will be reformatted");
    } else {
        logger.info(
            "certus-server: recovering extents from disk (use --format for clean slate)",
        );
    }

    // Eviction event channel drained by the TakeEvents op.
    let eviction_rx = disp_comp.create_eviction_channel(16384);
    let eviction_dropped = Arc::new(AtomicU64::new(0));

    let translator = Translator::new(
        Arc::clone(&dispatcher),
        eviction_rx,
        Arc::clone(&eviction_dropped),
    );

    // Create the shared-memory mailbox.
    let server = Arc::new(shm_queue::Server::create(
        &cli.shm_path,
        cli.channels,
        cli.cap_req,
        cli.cap_resp,
    )?);
    logger.info(&format!(
        "certus-server: shared-memory IPC path {} channels={} cap_req={} cap_resp={}",
        cli.shm_path,
        server.channel_count(),
        server.cap_req(),
        server.cap_resp()
    ));

    // Install SIGINT/SIGTERM handlers → flip SHUTDOWN.
    // SAFETY: handle_signal is async-signal-safe (single atomic store).
    unsafe {
        libc::signal(libc::SIGINT, handle_signal as libc::sighandler_t);
        libc::signal(libc::SIGTERM, handle_signal as libc::sighandler_t);
    }

    // Telemetry logger (rw-telemetry builds only): periodically log cumulative
    // SSD read/write stats + derived mean I/O block size, so a run's disk
    // behaviour is visible directly in the server log without a client polling
    // GetIoStats. Spawned before the blocking serve() loop and joined after it
    // returns.
    #[cfg(feature = "rw-telemetry")]
    let telemetry = {
        let dispatcher = Arc::clone(&dispatcher);
        let logger = Arc::clone(&logger);
        thread::Builder::new()
            .name("shmq-telemetry".into())
            .spawn(move || {
                let tick = Duration::from_millis(500);
                let mut elapsed = Duration::ZERO;
                while !SHUTDOWN.load(Ordering::Relaxed) {
                    thread::sleep(tick);
                    elapsed += tick;
                    if elapsed >= Duration::from_secs(TELEMETRY_INTERVAL_SECS) {
                        elapsed = Duration::ZERO;
                        logger.info(&format!(
                            "certus-server: io-stats {}",
                            format_io_stats(&dispatcher.read_write_stats())
                        ));
                    }
                }
            })
            .expect("spawn telemetry")
    };

    // Run the shared poller + worker-pool + reaper loop; blocks until SHUTDOWN.
    serve(
        server,
        translator,
        ServeConfig {
            channels: cli.channels,
            reserve_timeout: Duration::from_secs(cli.reserve_timeout_secs),
            poller_cpu: cli.shmq_poller_cpu,
        },
        &SHUTDOWN,
        Arc::clone(&logger),
    )?;

    // Read the final cumulative stats BEFORE shutdown tears down the block
    // devices (which is when the telemetry counters go away).
    #[cfg(feature = "rw-telemetry")]
    {
        let _ = telemetry.join();
        logger.info(&format!(
            "certus-server: FINAL io-stats {}",
            format_io_stats(&dispatcher.read_write_stats())
        ));
    }

    let _ = dispatcher.shutdown();
    logger.info("certus-server: shutdown complete");
    Ok(())
}
