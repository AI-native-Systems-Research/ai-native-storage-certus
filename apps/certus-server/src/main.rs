//! Certus gRPC Server
//!
//! Exposes the IDispatcher interface to Python clients via gRPC.
//! Auto-initializes the Certus component stack on startup using
//! CLI-provided PCI addresses.

mod service;

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use clap::Parser;
use tonic::transport::{Identity, Server, ServerTlsConfig};

use component_core::query_interface;
use interfaces::{
    DmaAllocFn, DmaBuffer, DispatcherConfig, IDispatchMap, IDispatcher, IGpuServices, ILogger,
    IMemoryTier, PciAddress,
};

use service::DispatcherService;

/// Certus gRPC server exposing the IDispatcher interface.
#[derive(Parser)]
#[command(name = "certus-server", about = "Certus dispatcher gRPC server")]
struct Cli {
    /// PCI address(es) of NVMe device(s) — may be specified multiple times
    #[arg(long = "device-pci", required = true)]
    device_pci: Vec<String>,

    /// gRPC listen address
    #[arg(long = "listen", default_value = "0.0.0.0:50051")]
    listen: String,

    /// Path to TLS certificate file (enables TLS when provided with --tls-key)
    #[arg(long = "tls-cert")]
    tls_cert: Option<String>,

    /// Path to TLS private key file (enables TLS when provided with --tls-cert)
    #[arg(long = "tls-key")]
    tls_key: Option<String>,
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
) -> Result<Arc<dyn IDispatcher + Send + Sync>, String> {
    eprintln!("certus-server: initializing SPDK environment...");
    let spdk_comp = spdk_env::SPDKEnvComponent::new_default();
    let spdk_iface = query_interface!(spdk_comp, spdk_env::ISPDKEnv)
        .ok_or("failed to query ISPDKEnv")?;
    spdk_iface.init().map_err(|e| format!("SPDK init failed: {e}"))?;

    let logger: Arc<dyn ILogger + Send + Sync> = logger::LoggerComponent::new_default();

    eprintln!("certus-server: initializing GPU services...");
    let gpu_comp = gpu_services::GpuServicesComponent::new_default();
    gpu_comp
        .logger
        .connect(Arc::clone(&logger))
        .map_err(|e| format!("gpu logger bind: {e}"))?;
    let gpu: Arc<dyn IGpuServices + Send + Sync> =
        query_interface!(gpu_comp, IGpuServices).ok_or("failed to query IGpuServices")?;
    gpu.initialize().map_err(|e| format!("GPU init failed: {e}"))?;

    // --- Create dispatch map (no persistence — starts fresh each time) ---
    eprintln!("certus-server: initializing dispatch map...");
    let dm_comp = dispatch_map::DispatchMapComponent::new(
        dispatch_map::DispatchMapState::default(),
    );
    dm_comp
        .logger
        .connect(Arc::clone(&logger))
        .map_err(|e| format!("dispatch map logger bind: {e}"))?;

    let dm: Arc<dyn IDispatchMap + Send + Sync> =
        query_interface!(dm_comp, IDispatchMap).ok_or("failed to query IDispatchMap")?;
    let dma_alloc: DmaAllocFn = Arc::new(move |size, align, _numa| {
        DmaBuffer::new(size, align, None).map_err(|e| e.to_string())
    });
    dm.set_dma_alloc(dma_alloc);
    dm.initialize()
        .map_err(|e| format!("DispatchMap init failed: {e}"))?;

    // --- Create memory-tier ---
    eprintln!("certus-server: initializing memory-tier...");
    let mt_comp = memory_tier::MemoryTierComponent::new_default();
    mt_comp
        .logger
        .connect(Arc::clone(&logger))
        .map_err(|e| format!("memory-tier logger bind: {e}"))?;
    let mt: Arc<dyn IMemoryTier + Send + Sync> =
        query_interface!(mt_comp, IMemoryTier).ok_or("failed to query IMemoryTier")?;
    mt.initialize(memory_tier::DEFAULT_POOL_SIZE)
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
            eprintln!(
                "certus-server: WARNING: cudaHostRegister failed (err={err}), \
                 memory-tier transfers will use staged path"
            );
        } else {
            eprintln!(
                "certus-server: memory-tier pool registered with CUDA ({} MiB pinned)",
                pool_size / (1024 * 1024)
            );
        }
    }

    // --- Create dispatcher ---
    eprintln!("certus-server: initializing dispatcher...");
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

    let dispatcher: Arc<dyn IDispatcher + Send + Sync> =
        query_interface!(disp_comp, IDispatcher).ok_or("failed to query IDispatcher")?;

    dispatcher
        .initialize(DispatcherConfig {
            data_pci_addrs: device_pci_addrs.to_vec(),
            ..Default::default()
        })
        .map_err(|e| format!("Dispatcher init failed: {e}"))?;

    eprintln!("certus-server: component stack initialized");
    Ok(dispatcher)
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let cli = Cli::parse();

    // Validate PCI addresses
    for addr in &cli.device_pci {
        validate_pci_address(addr)?;
    }

    eprintln!("certus-server: devices={:?}", cli.device_pci);

    // Initialize Certus component stack
    let dispatcher = initialize_component_stack(&cli.device_pci)?;
    let dispatcher_mutex = Arc::new(Mutex::new(dispatcher));

    let svc = DispatcherService::new(Arc::clone(&dispatcher_mutex));

    let addr = cli.listen.parse()?;

    // Build server with optional TLS
    let mut server = Server::builder();
    if let (Some(cert_path), Some(key_path)) = (&cli.tls_cert, &cli.tls_key) {
        let cert = tokio::fs::read(cert_path).await?;
        let key = tokio::fs::read(key_path).await?;
        let identity = Identity::from_pem(cert, key);
        server = server.tls_config(ServerTlsConfig::new().identity(identity))?;
        eprintln!("certus-server: TLS enabled");
    }

    eprintln!("certus-server: listening on {addr}");

    let shutdown_flag = Arc::new(AtomicBool::new(false));
    let flag_clone = Arc::clone(&shutdown_flag);

    server
        .add_service(service::dispatcher_server(svc))
        .serve_with_shutdown(addr, async move {
            tokio::signal::ctrl_c().await.ok();
            flag_clone.store(true, Ordering::Release);
            eprintln!("\ncertus-server: shutting down...");
        })
        .await?;

    // Shutdown dispatcher
    let disp = dispatcher_mutex.lock().unwrap();
    let _ = disp.shutdown();
    eprintln!("certus-server: shutdown complete");

    Ok(())
}
