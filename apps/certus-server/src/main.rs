//! Certus gRPC Server
//!
//! Exposes the IDispatcher interface to Python clients via gRPC.
//! Auto-initializes the Certus component stack on startup using
//! CLI-provided PCI addresses.

mod service;
#[cfg(feature = "otel")]
mod telemetry;

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use clap::Parser;
use tonic::transport::{Identity, Server, ServerTlsConfig};

use component_core::query_interface;
use interfaces::{
    DispatcherConfig, IDispatchMap, IDispatcher, IEvictionPolicy,
    IGpuServices, ILogger, IMemoryTier, IRemoteLookup, PciAddress,
};

use service::DispatcherService;

/// Certus gRPC server exposing the IDispatcher interface.
#[derive(Parser)]
#[command(name = "certus-server", about = "Certus dispatcher gRPC server")]
struct Cli {
    /// PCI address(es) of NVMe device(s) — may be specified multiple times.
    /// Mutually exclusive with --drive-count.
    #[arg(long = "device-pci")]
    device_pci: Vec<String>,

    /// Use the first N discovered NVMe drives (alternative to --device-pci).
    /// Requires SPDK to enumerate available devices at startup.
    #[arg(long = "drive-count", conflicts_with = "device_pci")]
    drive_count: Option<usize>,

    /// gRPC listen address
    #[arg(long = "listen", default_value = "0.0.0.0:50051")]
    listen: String,

    /// Memory-tier pool size (e.g. 256M, 1G, 512K). Defaults to 256M.
    #[arg(long = "memory-tier-size", value_parser = parse_size)]
    memory_tier_size: Option<usize>,

    /// Format extent managers on startup (destroys existing data).
    /// Without this flag, the server recovers previously stored extents.
    #[arg(long = "format")]
    format: bool,

    /// Path to TLS certificate file (enables TLS when provided with --tls-key)
    #[arg(long = "tls-cert")]
    tls_cert: Option<String>,

    /// Path to TLS private key file (enables TLS when provided with --tls-cert)
    #[arg(long = "tls-key")]
    tls_key: Option<String>,

    /// Pin each NVMe poller thread to a dedicated CPU core.
    /// Drive N is pinned to core (poller-base-cpu + N).
    /// Recommended: pick cores in the same NUMA zone as the drives
    /// (e.g. --poller-base-cpu 2 for drives on NUMA 0 with 4 drives → cores 2,3,4,5).
    #[arg(long = "poller-base-cpu")]
    poller_base_cpu: Option<usize>,

    /// Maximum eviction attempts before failing with pool-full error.
    #[arg(long = "max-eviction-attempts", default_value_t = 2048)]
    max_eviction_attempts: usize,

    /// Memory-tier utilization threshold (0.0–1.0) for background DRAM→SSD demotion.
    /// Disabled by default (0.0). Set to e.g. 0.8 to start demoting at 80% full.
    #[arg(long = "memory-tier-eviction-threshold", default_value_t = 0.0)]
    memory_tier_eviction_threshold: f64,

    /// OpenTelemetry OTLP endpoint (e.g. "http://localhost:4317").
    /// Enables metrics export when set. Requires --features otel.
    #[arg(long = "otel-endpoint")]
    otel_endpoint: Option<String>,

    /// Service name reported in OpenTelemetry metrics.
    #[arg(long = "otel-service-name", default_value = "certus-server")]
    otel_service_name: String,
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
        return Err(format!("invalid PCI address format '{addr}': expected DDDD:BB:DD.F"));
    }
    let domain = u32::from_str_radix(parts[0], 16)
        .map_err(|_| format!("invalid PCI domain in '{addr}'"))?;
    let bus = u8::from_str_radix(parts[1], 16)
        .map_err(|_| format!("invalid PCI bus in '{addr}'"))?;
    let dev_func: Vec<&str> = parts[2].split('.').collect();
    if dev_func.len() != 2 {
        return Err(format!("invalid PCI dev.func in '{addr}': expected DD.F"));
    }
    let dev = u8::from_str_radix(dev_func[0], 16)
        .map_err(|_| format!("invalid PCI device in '{addr}'"))?;
    let func = u8::from_str_radix(dev_func[1], 16)
        .map_err(|_| format!("invalid PCI function in '{addr}'"))?;
    Ok(PciAddress { domain, bus, dev, func })
}

fn initialize_component_stack(
    device_pci_addrs: &[String],
    drive_count: Option<usize>,
    memory_tier_size: usize,
    format: bool,
    poller_base_cpu: Option<usize>,
    max_eviction_attempts: usize,
    memory_tier_eviction_threshold: f64,
) -> Result<(Arc<dyn IDispatcher + Send + Sync>, Arc<dyn ILogger + Send + Sync>, Vec<String>, Arc<dispatcher::DispatcherComponent>), String> {
    let logger: Arc<dyn ILogger + Send + Sync> = logger::LoggerComponent::new_default();

    logger.info("certus-server: initializing SPDK environment...");
    let spdk_comp = spdk_env::SPDKEnvComponent::new_default();
    let spdk_iface = query_interface!(spdk_comp, spdk_env::ISPDKEnv)
        .ok_or("failed to query ISPDKEnv")?;
    spdk_iface.init().map_err(|e| format!("SPDK init failed: {e}"))?;

    // Resolve device addresses: use explicit list or auto-select from discovered devices.
    // NVMe PCI class code: 0x010802 (Mass Storage Controller, NVM Express).
    const NVME_CLASS_CODE: u32 = 0x010802;
    let device_pci_addrs = if device_pci_addrs.is_empty() {
        let count = drive_count.unwrap_or(1);
        let devices = spdk_iface.devices();
        let mut nvme_devices: Vec<_> = devices
            .iter()
            .filter(|d| d.id.class_id == NVME_CLASS_CODE)
            .collect();
        // Prioritize NUMA node 0 devices first.
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
            "certus-server: auto-selected {} drive(s): {:?}",
            count, addrs
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
    gpu.initialize().map_err(|e| format!("GPU init failed: {e}"))?;

    // --- Create eviction policy ---
    let ep_comp = eviction_policy_lru::EvictionPolicyLruComponent::new_default();
    ep_comp
        .logger
        .connect(Arc::clone(&logger))
        .map_err(|e| format!("eviction-policy logger bind: {e}"))?;
    let eviction_policy: Arc<dyn IEvictionPolicy + Send + Sync> =
        query_interface!(ep_comp, IEvictionPolicy)
            .ok_or("failed to query IEvictionPolicy")?;

    // --- Create dispatch map ---
    logger.info("certus-server: initializing dispatch map...");
    let dm_comp = dispatch_map::DispatchMapComponent::new(
        dispatch_map::DispatchMapState::default(),
    );
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

    // --- Create memory-tier ---
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

    // Bind memory-tier pool to the NUMA node of the first selected drive.
    let mt_numa_node: Option<i32> = device_pci_addrs.first().and_then(|first_addr| {
        spdk_iface
            .devices()
            .iter()
            .find(|d| d.address.to_string() == *first_addr)
            .map(|d| d.numa_node)
    });
    mt.initialize(memory_tier_size, mt_numa_node)
        .map_err(|e| format!("MemoryTier init failed: {e}"))?;

    // Register the memory-tier pool with CUDA for pinned DMA transfers.
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

    // --- Create remote lookup ---
    let rl_comp = remote_lookup::RemoteLookupComponent::new();
    rl_comp
        .logger
        .connect(Arc::clone(&logger))
        .map_err(|e| format!("failed to bind remote_lookup logger: {e}"))?;
    let remote_lookup: Arc<dyn IRemoteLookup + Send + Sync> =
        query_interface!(rl_comp, IRemoteLookup).ok_or("failed to query IRemoteLookup")?;

    // --- Create dispatcher ---
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

fn resolve_device_addresses(cli: &Cli) -> Result<Vec<String>, String> {
    if !cli.device_pci.is_empty() {
        for addr in &cli.device_pci {
            validate_pci_address(addr)?;
        }
        Ok(cli.device_pci.clone())
    } else if cli.drive_count.is_some() {
        // Deferred — resolved after SPDK init in initialize_component_stack.
        Ok(Vec::new())
    } else {
        Err("either --device-pci or --drive-count must be specified".into())
    }
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let cli = Cli::parse();

    let device_pci = resolve_device_addresses(&cli)
        .map_err(Box::<dyn std::error::Error>::from)?;

    const DEFAULT_MEMORY_TIER_SIZE: usize = 2 * 1024 * 1024 * 1024; // 2 GiB
    let pool_size = cli.memory_tier_size.unwrap_or(DEFAULT_MEMORY_TIER_SIZE);
    let (dispatcher, logger, device_pci, disp_comp) = initialize_component_stack(
        &device_pci, cli.drive_count, pool_size, cli.format, cli.poller_base_cpu,
        cli.max_eviction_attempts, cli.memory_tier_eviction_threshold,
    )?;

    logger.info(&format!("certus-server: devices={:?}", device_pci));
    logger.info(&format!(
        "certus-server: memory-tier-size={} MiB",
        pool_size / (1024 * 1024)
    ));
    if cli.format {
        logger.info("certus-server: --format specified, extent managers will be reformatted");
    } else {
        logger.info("certus-server: recovering extents from disk (use --format for clean slate)");
    }

    let eviction_rx = disp_comp.create_eviction_channel(16384);
    let eviction_dropped = Arc::new(std::sync::atomic::AtomicU64::new(0));

    #[cfg(feature = "otel")]
    let svc = {
        let svc = DispatcherService::new(
            Arc::clone(&dispatcher),
            eviction_rx,
            Arc::clone(&eviction_dropped),
        );
        if let Some(ref endpoint) = cli.otel_endpoint {
            let metrics = telemetry::Metrics::init(endpoint, &cli.otel_service_name)?;
            logger.info(&format!("certus-server: OTel metrics exporting to {endpoint}"));
            disp_comp.set_pipeline_metrics(Arc::new(metrics.pipeline.clone()));
            svc.with_metrics(metrics)
        } else {
            svc
        }
    };

    #[cfg(not(feature = "otel"))]
    let svc = {
        if cli.otel_endpoint.is_some() {
            logger.warn(
                "certus-server: --otel-endpoint specified but binary not compiled with --features otel"
            );
        }
        let _ = &disp_comp;
        DispatcherService::new(
            Arc::clone(&dispatcher),
            eviction_rx,
            Arc::clone(&eviction_dropped),
        )
    };

    let addr = cli.listen.parse()?;

    // Build server with optional TLS
    let mut server = Server::builder();
    if let (Some(cert_path), Some(key_path)) = (&cli.tls_cert, &cli.tls_key) {
        let cert = tokio::fs::read(cert_path).await?;
        let key = tokio::fs::read(key_path).await?;
        let identity = Identity::from_pem(cert, key);
        server = server.tls_config(ServerTlsConfig::new().identity(identity))?;
        logger.info("certus-server: TLS enabled");
    }

    logger.info(&format!("certus-server: listening on {addr}"));

    let shutdown_flag = Arc::new(AtomicBool::new(false));
    let flag_clone = Arc::clone(&shutdown_flag);
    let shutdown_logger = Arc::clone(&logger);

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
            shutdown_logger.info("certus-server: shutting down...");
        })
        .await?;

    // Shutdown dispatcher
    let _ = dispatcher.shutdown();
    logger.info("certus-server: shutdown complete");

    Ok(())
}
