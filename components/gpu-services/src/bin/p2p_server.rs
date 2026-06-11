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

use component_core::binding::bind;
use component_core::iunknown::query;
use component_core::query_interface;
use gpu_services::cuda_ffi;
use gpu_services::dma::create_spdk_dma_buffer_from_gpu_bar;
use gpu_services::GpuServicesComponent;
use interfaces::{IBlockDevice, IGpuServices, ILogger};

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
    #[arg(long, default_value = "4194304")]
    staging_size: usize,

    /// NVMe I/O chunk size in bytes (must not exceed MDTS, typically 128KB)
    #[arg(long, default_value = "131072")]
    chunk_size: usize,

    /// Serve one client then exit
    #[arg(long)]
    once: bool,
}

struct ServerContext {
    #[allow(dead_code)]
    _block_dev: Arc<dyn component_core::IUnknown + Send + Sync>,
    block_dev: Arc<dyn IBlockDevice + Send + Sync>,
    #[allow(dead_code)]
    spdk_env: Arc<dyn spdk_env::ISPDKEnv + Send + Sync>,
    #[allow(dead_code)]
    gpu_component: Arc<GpuServicesComponent>,
    #[allow(dead_code)]
    logger: Arc<dyn ILogger + Send + Sync>,
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

/// Pool of chunk-sized GPU staging buffers for concurrent NVMe reads.
struct ChunkPool {
    buffers: Vec<GpuStagingBuffer>,
    chunk_size: usize,
}

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

    let spdk_env_comp = spdk_env::SPDKEnvComponent::new_default();
    let block_dev = block_device_spdk_nvme::BlockDeviceSpdkNvmeComponent::new_default();
    let logger = logger::LoggerComponent::new_default();
    let gpu_component = GpuServicesComponent::new_default();

    bind(&*spdk_env_comp, "ISPDKEnv", &*block_dev, "spdk_env")
        .map_err(|e| format!("bind spdk_env: {e}"))?;
    bind(&*logger, "ILogger", &*block_dev, "logger").map_err(|e| format!("bind logger: {e}"))?;

    let logger: Arc<dyn ILogger + Send + Sync> = logger;
    gpu_component
        .logger
        .connect(Arc::clone(&logger))
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
        _block_dev: block_dev as Arc<dyn component_core::IUnknown + Send + Sync>,
        block_dev: ibd,
        spdk_env: spdk_env_comp as Arc<dyn spdk_env::ISPDKEnv + Send + Sync>,
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

fn create_chunk_pool(total_size: usize, chunk_size: usize) -> Result<ChunkPool, String> {
    let num_chunks = (total_size + chunk_size - 1) / chunk_size;
    let mut buffers = Vec::with_capacity(num_chunks);
    for i in 0..num_chunks {
        match create_gpu_staging(chunk_size) {
            Ok(buf) => buffers.push(buf),
            Err(e) => {
                // Clean up already-allocated buffers.
                for b in buffers {
                    unsafe { cuda_ffi::cudaFree(b.dev_ptr) };
                }
                return Err(format!("chunk pool alloc #{i}: {e}"));
            }
        }
    }
    Ok(ChunkPool {
        buffers,
        chunk_size,
    })
}

/// Issue concurrent async NVMe reads via BatchSubmit for all chunks.
fn do_chunked_read(
    ctx: &ServerContext,
    dma_bufs: &[Arc<Mutex<interfaces::DmaBuffer>>],
    base_lba: u64,
    chunk_size: usize,
) -> Result<(), String> {
    let sectors_per_chunk = chunk_size / ctx.sector_size;

    let channels = ctx.block_dev
        .connect_client()
        .map_err(|e| format!("connect_client: {e}"))?;

    let ops: Vec<interfaces::Command> = dma_bufs
        .iter()
        .enumerate()
        .map(|(i, buf)| interfaces::Command::ReadAsync {
            ns_id: ctx.ns_id,
            lba: base_lba + (i as u64 * sectors_per_chunk as u64),
            buf: Arc::clone(buf),
            timeout_ms: 5000,
            tag: 0,
        })
        .collect();

    let num_ops = ops.len();

    channels
        .command_tx
        .send(interfaces::Command::BatchSubmit { ops })
        .map_err(|e| format!("BatchSubmit send: {e}"))?;

    // Receive all completions.
    for _ in 0..num_ops {
        match channels.completion_rx.recv() {
            Ok(interfaces::Completion::ReadDone { result, .. }) => {
                result.map_err(|e| format!("NVMe async read: {e}"))?;
            }
            Ok(interfaces::Completion::Timeout { handle }) => {
                return Err(format!("NVMe read timeout (handle {:?})", handle));
            }
            Ok(other) => {
                return Err(format!("unexpected completion: {other:?}"));
            }
            Err(e) => {
                return Err(format!("recv: {e}"));
            }
        }
    }

    Ok(())
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

/// Bounce mode: NVMe → host DMA buffers (chunked) → cudaMemcpy H2D → client GPU.
fn handle_bounce(
    stream: &mut UnixStream,
    ctx: &ServerContext,
    chunk_size: usize,
) -> Result<String, String> {
    let (ipc_handle_bytes, size) = parse_client_payload(stream)?;
    let client_dev_ptr = open_ipc_handle(&ipc_handle_bytes)?;

    let num_chunks = (size + chunk_size - 1) / chunk_size;

    // Allocate one host DMA buffer per chunk from SPDK hugepages.
    let mut host_bufs: Vec<Arc<Mutex<interfaces::DmaBuffer>>> = Vec::with_capacity(num_chunks);
    for i in 0..num_chunks {
        let this_chunk = std::cmp::min(chunk_size, size - i * chunk_size);
        match interfaces::DmaBuffer::new(this_chunk, ctx.sector_size, None) {
            Ok(buf) => host_bufs.push(Arc::new(Mutex::new(buf))),
            Err(e) => {
                drop(host_bufs);
                unsafe { cuda_ffi::cudaIpcCloseMemHandle(client_dev_ptr) };
                return Err(format!("DMA alloc chunk #{i}: {e}"));
            }
        }
    }

    // Concurrent NVMe reads into all chunks.
    if let Err(e) = do_chunked_read(ctx, &host_bufs, 0, chunk_size) {
        drop(host_bufs);
        unsafe { cuda_ffi::cudaIpcCloseMemHandle(client_dev_ptr) };
        return Err(e);
    }

    // cudaMemcpy H2D per chunk at correct offset.
    for (i, buf) in host_bufs.iter().enumerate() {
        let offset = i * chunk_size;
        let this_chunk = std::cmp::min(chunk_size, size - offset);
        let buf_guard = buf.lock().unwrap();
        let err = unsafe {
            cuda_ffi::cudaMemcpy(
                (client_dev_ptr as *mut u8).add(offset) as *mut c_void,
                buf_guard.as_ptr() as *const c_void,
                this_chunk,
                cuda_ffi::CUDA_MEMCPY_HOST_TO_DEVICE,
            )
        };
        drop(buf_guard);
        if err != cuda_ffi::CUDA_SUCCESS {
            drop(host_bufs);
            unsafe { cuda_ffi::cudaIpcCloseMemHandle(client_dev_ptr) };
            return Err(format!(
                "cudaMemcpy H2D chunk #{i}: {}",
                cuda_ffi::cuda_error_string(err)
            ));
        }
    }

    drop(host_bufs);
    unsafe { cuda_ffi::cudaIpcCloseMemHandle(client_dev_ptr) };

    Ok(format!("OK {} bytes (bounce, {} chunks)", size, num_chunks))
}

/// P2P mode with pre-pinned chunk pool: NVMe → GPU staging chunks → D2D → client.
fn handle_p2p(
    stream: &mut UnixStream,
    ctx: &ServerContext,
    pool: &ChunkPool,
) -> Result<String, String> {
    let (ipc_handle_bytes, size) = parse_client_payload(stream)?;

    let total_capacity: usize = pool.buffers.iter().map(|b| b.capacity).sum();
    if size > total_capacity {
        return Err(format!(
            "requested {} exceeds pool capacity {}",
            size, total_capacity
        ));
    }

    let num_chunks = (size + pool.chunk_size - 1) / pool.chunk_size;
    let client_dev_ptr = open_ipc_handle(&ipc_handle_bytes)?;

    // Collect DMA buffer refs for the chunks we need.
    let dma_bufs: Vec<Arc<Mutex<interfaces::DmaBuffer>>> = pool.buffers[..num_chunks]
        .iter()
        .map(|b| Arc::clone(&b.dma_buf))
        .collect();

    // Concurrent NVMe reads into pre-pinned GPU staging chunks.
    if let Err(e) = do_chunked_read(ctx, &dma_bufs, 0, pool.chunk_size) {
        unsafe { cuda_ffi::cudaIpcCloseMemHandle(client_dev_ptr) };
        return Err(e);
    }

    // D2D copy per chunk: staging chunk → client at correct offset.
    for (i, staging_buf) in pool.buffers[..num_chunks].iter().enumerate() {
        let offset = i * pool.chunk_size;
        let this_chunk = std::cmp::min(pool.chunk_size, size - offset);
        let err = unsafe {
            cuda_ffi::cudaMemcpy(
                (client_dev_ptr as *mut u8).add(offset) as *mut c_void,
                staging_buf.dev_ptr as *const c_void,
                this_chunk,
                cuda_ffi::CUDA_MEMCPY_DEVICE_TO_DEVICE,
            )
        };
        if err != cuda_ffi::CUDA_SUCCESS {
            unsafe { cuda_ffi::cudaIpcCloseMemHandle(client_dev_ptr) };
            return Err(format!(
                "cudaMemcpy D2D chunk #{i}: {}",
                cuda_ffi::cuda_error_string(err)
            ));
        }
    }

    unsafe { cuda_ffi::cudaIpcCloseMemHandle(client_dev_ptr) };

    Ok(format!("OK {} bytes (p2p, {} chunks)", size, num_chunks))
}

/// P2P cold mode: per-request GDRCopy pin/unpin (baseline, measures setup overhead).
fn handle_p2p_cold(
    stream: &mut UnixStream,
    ctx: &ServerContext,
    chunk_size: usize,
) -> Result<String, String> {
    let (ipc_handle_bytes, size) = parse_client_payload(stream)?;
    let client_dev_ptr = open_ipc_handle(&ipc_handle_bytes)?;

    let num_chunks = (size + chunk_size - 1) / chunk_size;

    // Allocate + pin fresh GPU staging buffers per request (one per chunk).
    let mut staging_bufs: Vec<GpuStagingBuffer> = Vec::with_capacity(num_chunks);
    for i in 0..num_chunks {
        let this_chunk = std::cmp::min(chunk_size, size - i * chunk_size);
        match create_gpu_staging(this_chunk) {
            Ok(buf) => staging_bufs.push(buf),
            Err(e) => {
                for b in &staging_bufs {
                    unsafe { cuda_ffi::cudaFree(b.dev_ptr) };
                }
                unsafe { cuda_ffi::cudaIpcCloseMemHandle(client_dev_ptr) };
                return Err(format!("cold staging alloc #{i}: {e}"));
            }
        }
    }

    // Collect DMA buffer refs.
    let dma_bufs: Vec<Arc<Mutex<interfaces::DmaBuffer>>> =
        staging_bufs.iter().map(|b| Arc::clone(&b.dma_buf)).collect();

    // Concurrent NVMe reads into per-request staging chunks.
    if let Err(e) = do_chunked_read(ctx, &dma_bufs, 0, chunk_size) {
        for b in &staging_bufs {
            unsafe { cuda_ffi::cudaFree(b.dev_ptr) };
        }
        unsafe { cuda_ffi::cudaIpcCloseMemHandle(client_dev_ptr) };
        return Err(e);
    }

    // D2D copy per chunk: staging → client at correct offset.
    for (i, buf) in staging_bufs.iter().enumerate() {
        let offset = i * chunk_size;
        let this_chunk = std::cmp::min(chunk_size, size - offset);
        let err = unsafe {
            cuda_ffi::cudaMemcpy(
                (client_dev_ptr as *mut u8).add(offset) as *mut c_void,
                buf.dev_ptr as *const c_void,
                this_chunk,
                cuda_ffi::CUDA_MEMCPY_DEVICE_TO_DEVICE,
            )
        };
        if err != cuda_ffi::CUDA_SUCCESS {
            for b in &staging_bufs {
                unsafe { cuda_ffi::cudaFree(b.dev_ptr) };
            }
            unsafe { cuda_ffi::cudaIpcCloseMemHandle(client_dev_ptr) };
            return Err(format!(
                "cudaMemcpy D2D chunk #{i}: {}",
                cuda_ffi::cuda_error_string(err)
            ));
        }
    }

    // Cleanup: drop DMA bufs (unpins GDRCopy), free staging GPU memory.
    drop(dma_bufs);
    for b in staging_bufs {
        unsafe { cuda_ffi::cudaFree(b.dev_ptr) };
    }
    unsafe { cuda_ffi::cudaIpcCloseMemHandle(client_dev_ptr) };

    Ok(format!(
        "OK {} bytes (p2p-cold, {} chunks)",
        size, num_chunks
    ))
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

    // Pre-allocate chunk pool for p2p mode.
    let chunk_pool = match cli.mode {
        TransferMode::P2p => {
            let num_chunks = (cli.staging_size + cli.chunk_size - 1) / cli.chunk_size;
            eprintln!(
                "Pre-allocating chunk pool: {} x {} byte GPU staging buffers...",
                num_chunks, cli.chunk_size
            );
            match create_chunk_pool(cli.staging_size, cli.chunk_size) {
                Ok(pool) => {
                    eprintln!(
                        "Chunk pool ready: {} buffers (GDRCopy pinned, SPDK registered)",
                        pool.buffers.len()
                    );
                    Some(pool)
                }
                Err(e) => {
                    eprintln!("FATAL: chunk pool setup: {e}");
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

    eprintln!(
        "Listening on {} (mode={}, chunk_size={})",
        cli.socket, mode_str, cli.chunk_size
    );

    loop {
        if SHUTDOWN.load(Ordering::Relaxed) {
            eprintln!("Shutting down...");
            break;
        }

        match listener.accept() {
            Ok((mut stream, _)) => {
                let result = match cli.mode {
                    TransferMode::Bounce => {
                        handle_bounce(&mut stream, &ctx, cli.chunk_size)
                    }
                    TransferMode::P2p => {
                        handle_p2p(&mut stream, &ctx, chunk_pool.as_ref().unwrap())
                    }
                    TransferMode::P2pCold => {
                        handle_p2p_cold(&mut stream, &ctx, cli.chunk_size)
                    }
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

    drop(chunk_pool);
    let _ = std::fs::remove_file(&cli.socket);
}
