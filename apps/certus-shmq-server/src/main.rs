//! Certus shared-memory-queue server (prototype).
//!
//! A drop-in alternative to the gRPC dispatcher server (`apps/certus-server`):
//! same component stack and same CUDA-IPC translation, but the control-plane
//! transport is the lock-free `/dev/shm` mailbox from the `shm-queue` crate
//! instead of tonic/gRPC. The KV bytes still move GPU↔DRAM↔SSD out of band via
//! CUDA/SPDK DMA — only the small control messages change transport.
//!
//! Concurrency model (see plan): a **single poller thread** busy-scans every
//! channel's request word (never sleeps, so the request path needs no futex) and
//! hands each ready request to a **blocking worker pool** over a crossbeam
//! channel. Workers run the (possibly multi-millisecond, SSD-cold) dispatch and
//! write the reply, so a slow `batch_lookup` on one channel never head-of-line
//! blocks the others — the concurrency tonic got for free via `spawn_blocking`.
//! Shared state (`ipc_cache`, `pending_stores`) lives in the [`Translator`]
//! under mutexes, so a Reserve on one channel and a Commit on another agree.

mod translate;
mod wire;

use std::io;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;
use std::thread;
use std::time::Duration;

use clap::Parser;

use component_core::query_interface;
use interfaces::{
    DispatcherConfig, IDispatchMap, IDispatcher, IEvictionPolicy, IGpuServices, ILogger,
    IMemoryTier, IRemoteLookup, PciAddress,
};

use translate::Translator;

/// Set once by the SIGINT/SIGTERM handler; polled by the poller and reaper.
static SHUTDOWN: AtomicBool = AtomicBool::new(false);

extern "C" fn handle_signal(_sig: libc::c_int) {
    // async-signal-safe: a single relaxed atomic store.
    SHUTDOWN.store(true, Ordering::SeqCst);
}

/// Certus shared-memory-queue server exposing the IDispatcher control plane.
#[derive(Parser)]
#[command(
    name = "certus-shmq-server",
    about = "Certus dispatcher server over a /dev/shm mailbox (gRPC-free prototype)"
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

/// Pin the current thread to `cpu`. Best-effort; errors are surfaced to caller.
fn pin_current_thread(cpu: usize) -> io::Result<()> {
    // SAFETY: cpu_set_t is a plain bitset; sched_setaffinity(0, ...) targets the
    // calling thread. All arguments are sized correctly.
    unsafe {
        let mut set: libc::cpu_set_t = std::mem::zeroed();
        libc::CPU_ZERO(&mut set);
        libc::CPU_SET(cpu, &mut set);
        if libc::sched_setaffinity(0, std::mem::size_of::<libc::cpu_set_t>(), &set) != 0 {
            return Err(io::Error::last_os_error());
        }
    }
    Ok(())
}

/// Mirror of `certus-server/src/main.rs::initialize_component_stack`. Builds the
/// full component stack and returns the dispatcher facade plus the dispatcher
/// component (for the eviction channel). Kept in lockstep with the gRPC server;
/// the only omission is anything gRPC/tonic-specific (there is none here).
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

    logger.info("certus-shmq-server: initializing SPDK environment...");
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
            "certus-shmq-server: auto-selected {count} drive(s): {addrs:?}"
        ));
        addrs
    } else {
        device_pci_addrs.to_vec()
    };

    logger.info("certus-shmq-server: initializing GPU services...");
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

    logger.info("certus-shmq-server: initializing dispatch map...");
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

    logger.info("certus-shmq-server: initializing memory-tier...");
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
                "certus-shmq-server: cudaHostRegister failed (err={err}), \
                 memory-tier transfers will use staged path"
            ));
        } else {
            logger.info(&format!(
                "certus-shmq-server: memory-tier pool registered with CUDA ({} MiB pinned)",
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

    logger.info("certus-shmq-server: initializing dispatcher...");
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

    logger.info("certus-shmq-server: component stack initialized");
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

    logger.info(&format!("certus-shmq-server: devices={device_pci:?}"));
    logger.info(&format!(
        "certus-shmq-server: memory-tier-size={} MiB",
        pool_size / (1024 * 1024)
    ));
    if cli.format {
        logger.info("certus-shmq-server: --format specified, extent managers will be reformatted");
    } else {
        logger.info(
            "certus-shmq-server: recovering extents from disk (use --format for clean slate)",
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
        "certus-shmq-server: mailbox {} channels={} cap_req={} cap_resp={}",
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

    // Worker pool: one worker per channel, blocking on the request queue.
    let (tx, rx) = crossbeam_channel::unbounded::<shm_queue::PolledRequest>();
    let mut workers = Vec::with_capacity(cli.channels);
    for w in 0..cli.channels {
        let rx = rx.clone();
        let server = Arc::clone(&server);
        let tr = translator.clone();
        workers.push(
            thread::Builder::new()
                .name(format!("shmq-worker-{w}"))
                .spawn(move || {
                    while let Ok(req) = rx.recv() {
                        match tr.dispatch(req.opcode, &req.payload) {
                            Ok(blob) => server.reply(req.channel, req.seq, wire::STATUS_OK, &blob),
                            Err(e) => {
                                let msg = e.to_string();
                                server.reply(
                                    req.channel,
                                    req.seq,
                                    wire::STATUS_ERROR,
                                    msg.as_bytes(),
                                );
                            }
                        }
                    }
                })
                .expect("spawn worker"),
        );
    }
    drop(rx); // only workers hold receivers now

    // Reservation-timeout reaper: reclaim Reserve-without-Commit leaks.
    let reserve_timeout = Duration::from_secs(cli.reserve_timeout_secs);
    let reaper = {
        let tr = translator.clone();
        let logger = Arc::clone(&logger);
        thread::Builder::new()
            .name("shmq-reaper".into())
            .spawn(move || {
                // Poll SHUTDOWN every 500ms; sweep for stale reservations ~5s.
                let tick = Duration::from_millis(500);
                let mut since_sweep = Duration::ZERO;
                while !SHUTDOWN.load(Ordering::Relaxed) {
                    thread::sleep(tick);
                    since_sweep += tick;
                    if since_sweep >= Duration::from_secs(5) {
                        since_sweep = Duration::ZERO;
                        let n = tr.reap_stale_reservations(reserve_timeout);
                        if n > 0 {
                            logger.warn(&format!(
                                "certus-shmq-server: reclaimed {n} stale reservation(s) \
                                 (uncommitted > {}s)",
                                reserve_timeout.as_secs()
                            ));
                        }
                    }
                }
            })
            .expect("spawn reaper")
    };

    // Poller thread: busy-scan every channel, hand ready requests to workers.
    let poller = {
        let server = Arc::clone(&server);
        let logger = Arc::clone(&logger);
        let poller_cpu = cli.shmq_poller_cpu;
        thread::Builder::new()
            .name("shmq-poller".into())
            .spawn(move || {
                if let Some(cpu) = poller_cpu {
                    match pin_current_thread(cpu) {
                        Ok(()) => {
                            logger.info(&format!("certus-shmq-server: poller pinned to CPU {cpu}"))
                        }
                        Err(e) => logger.warn(&format!(
                            "certus-shmq-server: poller pin to CPU {cpu} failed: {e}"
                        )),
                    }
                }
                let mut last_seen = server.seq_baseline();
                let mut sweeps: u64 = 0;
                while !SHUTDOWN.load(Ordering::Relaxed) {
                    let mut idle = true;
                    for (ch, seen) in last_seen.iter_mut().enumerate() {
                        if let Some(req) = server.take_request(ch, seen) {
                            idle = false;
                            if tx.send(req).is_err() {
                                return; // workers gone
                            }
                        }
                    }
                    sweeps = sweeps.wrapping_add(1);
                    if sweeps % 4096 == 0 {
                        server.heartbeat();
                    }
                    if idle {
                        std::hint::spin_loop();
                    }
                }
            })
            .expect("spawn poller")
    };

    logger.info("certus-shmq-server: serving (Ctrl-C to stop)");

    // Wait for shutdown, then tear down in order: poller stops sending, its `tx`
    // drops, workers drain and exit on the closed channel, reaper wakes and exits.
    poller.join().expect("join poller");
    for w in workers {
        let _ = w.join();
    }
    let _ = reaper.join();

    let _ = dispatcher.shutdown();
    logger.info("certus-shmq-server: shutdown complete");
    Ok(())
}
