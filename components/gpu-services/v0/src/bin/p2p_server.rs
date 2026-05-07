//! GPU P2P DMA server: accepts CUDA IPC handles over a Unix socket
//! and performs NVMe → GPU VRAM DMA transfers.
//!
//! Supports three transfer modes for benchmarking:
//! - `bounce`: NVMe → host DMA buffer → cudaMemcpy H2D → client GPU buffer
//! - `p2p`: NVMe → pre-pinned GPU staging (GDRCopy, setup amortized) → D2D → client
//! - `p2p-cold`: NVMe → per-request GDRCopy pin/unpin → D2D → client (baseline)

use std::ffi::c_void;
use std::io::{BufRead, Write};
use std::os::unix::net::{UnixListener, UnixStream};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use clap::{Parser, ValueEnum};

use block_device_spdk_nvme::BlockDeviceSpdkNvmeComponentV1;
use component_core::binding::bind;
use component_core::iunknown::query;
use component_core::query_interface;
use gpu_services::cuda_ffi;
use gpu_services::dma::create_spdk_dma_buffer_from_gpu_bar;
use gpu_services::GpuServicesComponentV0;
use interfaces::{IBlockDevice, IGpuServices, ILogger};
use logger::LoggerComponentV1;
use spdk_env::SPDKEnvComponent;

#[derive(Clone, Copy, ValueEnum)]
enum TransferMode {
    /// NVMe → host bounce buffer → cudaMemcpy H2D → client GPU
    Bounce,
    /// NVMe → pre-pinned GPU staging (GDRCopy amortized) → D2D → client GPU
    P2p,
    /// NVMe → per-request GDRCopy pin/unpin → D2D → client GPU (cold baseline)
    P2pCold,
}

#[derive(Parser)]
#[command(name = "gpu-p2p-server", about = "NVMe → GPU P2P DMA server via Unix socket")]
struct Cli {
    /// Path to the Unix domain socket
    #[arg(long, default_value = "/tmp/gpu_p2p_server.sock")]
    socket: String,

    /// NVMe PCI address (DDDD:BB:DD.F); uses first device if omitted
    #[arg(long)]
    pci: Option<String>,

    /// Transfer mode
    #[arg(long, value_enum, default_value = "p2p")]
    mode: TransferMode,

    /// Pre-allocated staging buffer size (for p2p mode)
    #[arg(long, default_value = "131072")]
    staging_size: usize,

    /// Serve one client then exit
    #[arg(long)]
    once: bool,
}

struct ServerContext {
    block_dev: Arc<BlockDeviceSpdkNvmeComponentV1>,
    #[allow(dead_code)]
    spdk_env: Arc<SPDKEnvComponent>,
    #[allow(dead_code)]
    gpu_component: Arc<GpuServicesComponentV0>,
    logger: Arc<LoggerComponentV1>,
    sector_size: usize,
    ns_id: u32,
}

/// Pre-pinned GPU staging buffer (allocated once, reused across requests).
struct GpuStagingBuffer {
    dev_ptr: *mut c_void,
    dma_buf: Arc<Mutex<interfaces::DmaBuffer>>,
    capacity: usize,
}

// SAFETY: The GPU staging buffer is only accessed from the main thread.
unsafe impl Send for GpuStagingBuffer {}
unsafe impl Sync for GpuStagingBuffer {}

static SHUTDOWN: AtomicBool = AtomicBool::new(false);

extern "C" fn signal_handler(_: libc::c_int) {
    SHUTDOWN.store(true, Ordering::Relaxed);
}

fn kernel_module_loaded(name: &str) -> bool {
    std::fs::read_to_string("/proc/modules")
        .map(|s| s.lines().any(|line| line.starts_with(&format!("{name} "))))
        .unwrap_or(false)
}

fn initialize_stack(pci: Option<&str>) -> Result<ServerContext, String> {
    // Prevent SPDK atexit teardown crash.
    extern "C" {
        fn atexit(cb: extern "C" fn()) -> i32;
        fn _exit(status: i32) -> !;
    }
    extern "C" fn exit_hook() {
        unsafe { _exit(0) };
    }
    unsafe { atexit(exit_hook) };

    if !kernel_module_loaded("nvidia_peermem") {
        return Err("nvidia-peermem kernel module not loaded".into());
    }
    if !kernel_module_loaded("gdrdrv") {
        return Err("gdrdrv kernel module not loaded".into());
    }

    let mut device_count: std::os::raw::c_int = 0;
    let err = unsafe { cuda_ffi::cudaGetDeviceCount(&mut device_count) };
    if err != cuda_ffi::CUDA_SUCCESS || device_count == 0 {
        return Err("no CUDA GPU available".into());
    }

    spdk_env::checks::check_vfio_available().map_err(|e| format!("VFIO: {e}"))?;
    spdk_env::checks::check_hugepages().map_err(|e| format!("hugepages: {e}"))?;

    let spdk_env_comp = SPDKEnvComponent::new_default();
    let block_dev = BlockDeviceSpdkNvmeComponentV1::new_default();
    let logger = LoggerComponentV1::new_default();
    let gpu_component = GpuServicesComponentV0::new();

    bind(&*spdk_env_comp, "ISPDKEnv", &*block_dev, "spdk_env")
        .map_err(|e| format!("bind spdk_env: {e}"))?;
    bind(&*logger, "ILogger", &*block_dev, "logger").map_err(|e| format!("bind logger: {e}"))?;

    let logger_iface: Arc<dyn ILogger + Send + Sync> = LoggerComponentV1::new_default();
    gpu_component
        .logger
        .connect(logger_iface)
        .map_err(|e| format!("connect logger: {e}"))?;

    let gpu = query_interface!(gpu_component, IGpuServices)
        .ok_or("IGpuServices query failed")?;

    let ienv = query::<dyn spdk_env::ISPDKEnv + Send + Sync>(&*spdk_env_comp)
        .ok_or("ISPDKEnv query failed")?;
    ienv.init().map_err(|e| format!("SPDK init: {e}"))?;

    let devices = ienv.devices();
    if devices.is_empty() {
        return Err("no NVMe devices found".into());
    }

    let spdk_addr = if let Some(pci_str) = pci {
        devices
            .iter()
            .find(|d| format!("{}", d.address) == pci_str)
            .ok_or_else(|| format!("NVMe device {pci_str} not found"))?
            .address
    } else {
        devices[0].address
    };

    let addr = interfaces::PciAddress {
        domain: spdk_addr.domain,
        bus: spdk_addr.bus,
        dev: spdk_addr.dev,
        func: spdk_addr.func,
    };

    let admin =
        query::<dyn interfaces::iblock_device::IBlockDeviceAdmin + Send + Sync>(&*block_dev)
            .ok_or("IBlockDeviceAdmin query failed")?;
    admin.set_pci_address(addr);
    admin
        .initialize()
        .map_err(|e| format!("block device init: {e}"))?;

    gpu.initialize().map_err(|e| format!("CUDA init: {e}"))?;

    let ibd = query::<dyn IBlockDevice + Send + Sync>(&*block_dev)
        .ok_or("IBlockDevice query failed")?;
    let channels = ibd
        .connect_client()
        .map_err(|e| format!("connect_client: {e}"))?;
    channels
        .command_tx
        .send(interfaces::Command::NsProbe)
        .map_err(|e| format!("NsProbe send: {e}"))?;

    let namespaces = match channels.completion_rx.recv() {
        Ok(interfaces::Completion::NsProbeResult { namespaces }) => namespaces,
        Ok(other) => return Err(format!("unexpected completion: {other:?}")),
        Err(e) => return Err(format!("NsProbe recv: {e}")),
    };
    drop(channels);

    if namespaces.is_empty() {
        return Err("no NVMe namespaces found".into());
    }

    let ns = &namespaces[0];
    eprintln!(
        "Initialized: NVMe ns_id={}, sector_size={}, GPU devices={}",
        ns.ns_id, ns.sector_size, device_count
    );

    Ok(ServerContext {
        block_dev,
        spdk_env: spdk_env_comp,
        gpu_component,
        logger,
        sector_size: ns.sector_size as usize,
        ns_id: ns.ns_id,
    })
}

fn create_gpu_staging(size: usize) -> Result<GpuStagingBuffer, String> {
    let alloc_size = std::cmp::max(size, gpu_services::gdrcopy_ffi::GPU_PAGE_SIZE);
    let mut dev_ptr: *mut c_void = std::ptr::null_mut();
    let err = unsafe { cuda_ffi::cudaMalloc(&mut dev_ptr, alloc_size) };
    if err != cuda_ffi::CUDA_SUCCESS {
        return Err(format!(
            "cudaMalloc staging: {}",
            cuda_ffi::cuda_error_string(err)
        ));
    }

    let dma_buf = match create_spdk_dma_buffer_from_gpu_bar(dev_ptr, size) {
        Ok(buf) => buf,
        Err(e) => {
            unsafe { cuda_ffi::cudaFree(dev_ptr) };
            return Err(format!("GDRCopy/SPDK setup: {e}"));
        }
    };

    Ok(GpuStagingBuffer {
        dev_ptr,
        dma_buf: Arc::new(Mutex::new(dma_buf)),
        capacity: size,
    })
}

fn parse_client_payload(stream: &mut UnixStream) -> Result<([u8; 64], usize), String> {
    let mut reader = std::io::BufReader::new(stream.try_clone().map_err(|e| e.to_string())?);
    let mut line = String::new();
    reader
        .read_line(&mut line)
        .map_err(|e| format!("read payload: {e}"))?;
    let line = line.trim();

    if line.is_empty() {
        return Err("empty payload".into());
    }

    let payload = base64::Engine::decode(&base64::engine::general_purpose::STANDARD, line)
        .map_err(|e| format!("base64 decode: {e}"))?;
    if payload.len() != 72 {
        return Err(format!("payload must be 72 bytes, got {}", payload.len()));
    }

    let mut ipc_handle_bytes = [0u8; 64];
    ipc_handle_bytes.copy_from_slice(&payload[..64]);
    let size = u64::from_le_bytes(payload[64..72].try_into().unwrap()) as usize;

    Ok((ipc_handle_bytes, size))
}

fn open_ipc_handle(ipc_handle_bytes: &[u8; 64]) -> Result<*mut c_void, String> {
    let mut ipc_handle = cuda_ffi::cudaIpcMemHandle_t {
        reserved: [0u8; 64],
    };
    ipc_handle.reserved.copy_from_slice(ipc_handle_bytes);

    let mut dev_ptr: *mut c_void = std::ptr::null_mut();
    let err = unsafe {
        cuda_ffi::cudaIpcOpenMemHandle(
            &mut dev_ptr,
            ipc_handle,
            cuda_ffi::CUDA_IPC_MEM_LAZY_ENABLE_PEER_ACCESS,
        )
    };
    if err != cuda_ffi::CUDA_SUCCESS {
        return Err(format!(
            "cudaIpcOpenMemHandle: {}",
            cuda_ffi::cuda_error_string(err)
        ));
    }
    Ok(dev_ptr)
}

/// Bounce mode: NVMe → host DMA buffer → cudaMemcpy H2D → client GPU.
fn handle_bounce(
    stream: &mut UnixStream,
    ctx: &ServerContext,
) -> Result<String, String> {
    let (ipc_handle_bytes, size) = parse_client_payload(stream)?;
    let client_dev_ptr = open_ipc_handle(&ipc_handle_bytes)?;

    // Allocate host DMA buffer from SPDK hugepages.
    let host_buf = interfaces::DmaBuffer::new(size, ctx.sector_size, None)
        .map_err(|e| {
            unsafe { cuda_ffi::cudaIpcCloseMemHandle(client_dev_ptr) };
            format!("DMA alloc: {e}")
        })?;

    // NVMe read into host buffer.
    let ibd = query::<dyn IBlockDevice + Send + Sync>(&*ctx.block_dev)
        .ok_or("IBlockDevice query failed".to_string())?;
    let channels = ibd
        .connect_client()
        .map_err(|e| format!("connect_client: {e}"))?;

    let host_buf = Arc::new(Mutex::new(host_buf));
    channels
        .command_tx
        .send(interfaces::Command::ReadSync {
            ns_id: ctx.ns_id,
            lba: 0,
            buf: Arc::clone(&host_buf),
        })
        .map_err(|e| format!("ReadSync send: {e}"))?;

    match channels.completion_rx.recv() {
        Ok(interfaces::Completion::ReadDone { result, .. }) => {
            result.map_err(|e| format!("NVMe read: {e}"))?;
        }
        Ok(other) => {
            drop(host_buf);
            unsafe { cuda_ffi::cudaIpcCloseMemHandle(client_dev_ptr) };
            return Err(format!("unexpected completion: {other:?}"));
        }
        Err(e) => {
            drop(host_buf);
            unsafe { cuda_ffi::cudaIpcCloseMemHandle(client_dev_ptr) };
            return Err(format!("recv: {e}"));
        }
    }

    // cudaMemcpy H2D: host buffer → client GPU buffer.
    let buf_guard = host_buf.lock().unwrap();
    let err = unsafe {
        cuda_ffi::cudaMemcpy(
            client_dev_ptr,
            buf_guard.as_ptr() as *const c_void,
            size,
            cuda_ffi::CUDA_MEMCPY_HOST_TO_DEVICE,
        )
    };
    drop(buf_guard);
    drop(host_buf);

    unsafe { cuda_ffi::cudaIpcCloseMemHandle(client_dev_ptr) };

    if err != cuda_ffi::CUDA_SUCCESS {
        return Err(format!(
            "cudaMemcpy H2D: {}",
            cuda_ffi::cuda_error_string(err)
        ));
    }

    Ok(format!("OK {} bytes (bounce)", size))
}

/// P2P mode with pre-pinned staging: NVMe → GPU staging → D2D → client.
fn handle_p2p(
    stream: &mut UnixStream,
    ctx: &ServerContext,
    staging: &GpuStagingBuffer,
) -> Result<String, String> {
    let (ipc_handle_bytes, size) = parse_client_payload(stream)?;

    if size > staging.capacity {
        return Err(format!(
            "requested {} exceeds staging capacity {}",
            size, staging.capacity
        ));
    }

    let client_dev_ptr = open_ipc_handle(&ipc_handle_bytes)?;

    // NVMe read into pre-pinned GPU staging buffer.
    let ibd = query::<dyn IBlockDevice + Send + Sync>(&*ctx.block_dev)
        .ok_or("IBlockDevice query failed".to_string())?;
    let channels = ibd
        .connect_client()
        .map_err(|e| format!("connect_client: {e}"))?;

    channels
        .command_tx
        .send(interfaces::Command::ReadSync {
            ns_id: ctx.ns_id,
            lba: 0,
            buf: Arc::clone(&staging.dma_buf),
        })
        .map_err(|e| format!("ReadSync send: {e}"))?;

    match channels.completion_rx.recv() {
        Ok(interfaces::Completion::ReadDone { result, .. }) => {
            result.map_err(|e| format!("NVMe read: {e}"))?;
        }
        Ok(other) => {
            unsafe { cuda_ffi::cudaIpcCloseMemHandle(client_dev_ptr) };
            return Err(format!("unexpected completion: {other:?}"));
        }
        Err(e) => {
            unsafe { cuda_ffi::cudaIpcCloseMemHandle(client_dev_ptr) };
            return Err(format!("recv: {e}"));
        }
    }

    // D2D copy: staging → client.
    let err = unsafe {
        cuda_ffi::cudaMemcpy(
            client_dev_ptr,
            staging.dev_ptr as *const c_void,
            size,
            cuda_ffi::CUDA_MEMCPY_DEVICE_TO_DEVICE,
        )
    };

    unsafe { cuda_ffi::cudaIpcCloseMemHandle(client_dev_ptr) };

    if err != cuda_ffi::CUDA_SUCCESS {
        return Err(format!(
            "cudaMemcpy D2D: {}",
            cuda_ffi::cuda_error_string(err)
        ));
    }

    Ok(format!("OK {} bytes (p2p)", size))
}

/// P2P cold mode: per-request GDRCopy pin/unpin (baseline, measures setup overhead).
fn handle_p2p_cold(
    stream: &mut UnixStream,
    ctx: &ServerContext,
) -> Result<String, String> {
    let (ipc_handle_bytes, size) = parse_client_payload(stream)?;
    let client_dev_ptr = open_ipc_handle(&ipc_handle_bytes)?;

    // Allocate + pin a fresh GPU staging buffer per request.
    let alloc_size = std::cmp::max(size, gpu_services::gdrcopy_ffi::GPU_PAGE_SIZE);
    let mut staging_ptr: *mut c_void = std::ptr::null_mut();
    let err = unsafe { cuda_ffi::cudaMalloc(&mut staging_ptr, alloc_size) };
    if err != cuda_ffi::CUDA_SUCCESS {
        unsafe { cuda_ffi::cudaIpcCloseMemHandle(client_dev_ptr) };
        return Err(format!(
            "cudaMalloc staging: {}",
            cuda_ffi::cuda_error_string(err)
        ));
    }

    let dma_buf = match create_spdk_dma_buffer_from_gpu_bar(staging_ptr, size) {
        Ok(buf) => buf,
        Err(e) => {
            unsafe {
                cuda_ffi::cudaFree(staging_ptr);
                cuda_ffi::cudaIpcCloseMemHandle(client_dev_ptr);
            }
            return Err(format!("GDRCopy/SPDK setup: {e}"));
        }
    };

    // NVMe read into per-request staging.
    let ibd = query::<dyn IBlockDevice + Send + Sync>(&*ctx.block_dev)
        .ok_or("IBlockDevice query failed".to_string())?;
    let channels = ibd
        .connect_client()
        .map_err(|e| format!("connect_client: {e}"))?;

    let dma_buf = Arc::new(Mutex::new(dma_buf));
    channels
        .command_tx
        .send(interfaces::Command::ReadSync {
            ns_id: ctx.ns_id,
            lba: 0,
            buf: Arc::clone(&dma_buf),
        })
        .map_err(|e| format!("ReadSync send: {e}"))?;

    match channels.completion_rx.recv() {
        Ok(interfaces::Completion::ReadDone { result, .. }) => {
            result.map_err(|e| format!("NVMe read: {e}"))?;
        }
        Ok(other) => {
            drop(dma_buf);
            unsafe {
                cuda_ffi::cudaFree(staging_ptr);
                cuda_ffi::cudaIpcCloseMemHandle(client_dev_ptr);
            }
            return Err(format!("unexpected completion: {other:?}"));
        }
        Err(e) => {
            drop(dma_buf);
            unsafe {
                cuda_ffi::cudaFree(staging_ptr);
                cuda_ffi::cudaIpcCloseMemHandle(client_dev_ptr);
            }
            return Err(format!("recv: {e}"));
        }
    }

    // D2D copy: staging → client.
    let err = unsafe {
        cuda_ffi::cudaMemcpy(
            client_dev_ptr,
            staging_ptr as *const c_void,
            size,
            cuda_ffi::CUDA_MEMCPY_DEVICE_TO_DEVICE,
        )
    };

    drop(dma_buf);
    unsafe {
        cuda_ffi::cudaFree(staging_ptr);
        cuda_ffi::cudaIpcCloseMemHandle(client_dev_ptr);
    }

    if err != cuda_ffi::CUDA_SUCCESS {
        return Err(format!(
            "cudaMemcpy D2D: {}",
            cuda_ffi::cuda_error_string(err)
        ));
    }

    Ok(format!("OK {} bytes (p2p-cold)", size))
}

fn main() {
    let cli = Cli::parse();

    eprintln!("gpu-p2p-server: initializing SPDK/CUDA stack...");
    let ctx = match initialize_stack(cli.pci.as_deref()) {
        Ok(c) => c,
        Err(e) => {
            eprintln!("FATAL: {e}");
            std::process::exit(1);
        }
    };

    // Pre-allocate GPU staging buffer for p2p mode.
    let staging = match cli.mode {
        TransferMode::P2p => {
            eprintln!(
                "Pre-allocating {} byte GPU staging buffer...",
                cli.staging_size
            );
            match create_gpu_staging(cli.staging_size) {
                Ok(s) => {
                    eprintln!("GPU staging ready (GDRCopy pinned, SPDK registered)");
                    Some(s)
                }
                Err(e) => {
                    eprintln!("FATAL: staging setup: {e}");
                    std::process::exit(1);
                }
            }
        }
        _ => None,
    };

    let mode_str = match cli.mode {
        TransferMode::Bounce => "bounce",
        TransferMode::P2p => "p2p",
        TransferMode::P2pCold => "p2p-cold",
    };

    // Signal handling.
    unsafe {
        libc::signal(libc::SIGINT, signal_handler as libc::sighandler_t);
        libc::signal(libc::SIGTERM, signal_handler as libc::sighandler_t);
    }

    // Bind socket.
    let _ = std::fs::remove_file(&cli.socket);
    let listener = match UnixListener::bind(&cli.socket) {
        Ok(l) => l,
        Err(e) => {
            eprintln!("FATAL: bind {}: {e}", cli.socket);
            std::process::exit(1);
        }
    };
    listener.set_nonblocking(true).ok();

    eprintln!("Listening on {} (mode={})", cli.socket, mode_str);

    loop {
        if SHUTDOWN.load(Ordering::Relaxed) {
            eprintln!("Shutting down...");
            break;
        }

        match listener.accept() {
            Ok((mut stream, _)) => {
                let result = match cli.mode {
                    TransferMode::Bounce => handle_bounce(&mut stream, &ctx),
                    TransferMode::P2p => {
                        handle_p2p(&mut stream, &ctx, staging.as_ref().unwrap())
                    }
                    TransferMode::P2pCold => handle_p2p_cold(&mut stream, &ctx),
                };
                match result {
                    Ok(msg) => {
                        let _ = writeln!(stream, "{msg}");
                    }
                    Err(e) => {
                        eprintln!("  ERROR: {e}");
                        let _ = writeln!(stream, "ERROR: {e}");
                    }
                }
                if cli.once {
                    break;
                }
            }
            Err(ref e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                std::thread::sleep(std::time::Duration::from_micros(100));
            }
            Err(e) => {
                eprintln!("accept error: {e}");
                break;
            }
        }
    }

    drop(staging);
    let _ = std::fs::remove_file(&cli.socket);
}
