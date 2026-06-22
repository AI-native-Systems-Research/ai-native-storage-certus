//! Bounce-buffer vs GPUDirect P2P benchmark.
//!
//! Compares two NVMe→GPU transfer strategies using true async pipelining:
//! - **Bounce-buffer (BB)**: NVMe → CUDA-pinned host ring → cudaMemcpyAsync H2D → GPU
//! - **P2P (GDRCopy)**: NVMe → GPU BAR1 ring buffer → cudaMemcpyAsync D2D → GPU
//!
//! The bounce-buffer path uses `cudaHostAlloc` memory registered with SPDK,
//! ensuring `cudaMemcpyAsync` uses the GPU's DMA copy engine (truly async).
//! Two alternating CUDA streams overlap H2D copies with NVMe completion polling.

#![allow(clippy::arc_with_non_send_sync)]

use std::ffi::c_void;
use std::sync::{Arc, Mutex};
use std::time::Instant;

use clap::Parser;

use block_device_spdk_nvme::BlockDeviceSpdkNvmeComponent;
use component_core::binding::bind;
use component_core::iunknown::query;
use gpu_services::cuda_ffi;
use gpu_services::dma::{create_spdk_dma_buffer_from_cuda_host_alloc, create_spdk_dma_buffer_from_gpu_bar};
use interfaces::{Command, Completion, DmaBuffer, IBlockDevice};
use spdk_env::SPDKEnvComponent;

// ---------------------------------------------------------------------------
// CUDA stream FFI (not yet in gpu_services::cuda_ffi)
// ---------------------------------------------------------------------------

type CudaStream = *mut c_void;

extern "C" {
    fn cudaStreamCreate(stream: *mut CudaStream) -> cuda_ffi::cudaError_t;
    fn cudaStreamSynchronize(stream: CudaStream) -> cuda_ffi::cudaError_t;
    fn cudaStreamDestroy(stream: CudaStream) -> cuda_ffi::cudaError_t;
    fn cudaMemcpyAsync(
        dst: *mut c_void,
        src: *const c_void,
        count: usize,
        kind: std::os::raw::c_int,
        stream: CudaStream,
    ) -> cuda_ffi::cudaError_t;
}

// ---------------------------------------------------------------------------
// CLI
// ---------------------------------------------------------------------------

#[derive(Parser)]
#[command(
    name = "gpu-bb-vs-p2p",
    about = "Benchmark: pipelined bounce-buffer vs GPUDirect P2P NVMe→GPU transfers"
)]
struct Cli {
    /// NVMe PCI address (DDDD:BB:DD.F format); uses first device if omitted
    #[arg(long)]
    pci: Option<String>,

    /// Transfer chunk size in bytes (must not exceed NVMe MDTS, typically 128KiB)
    #[arg(long, default_value = "131072")]
    chunk_size: usize,

    /// Total stream size in bytes (split into chunk_size transfers)
    #[arg(long, default_value = "5242880")]
    stream_size: usize,

    /// Number of warmup iterations before measurement
    #[arg(long, default_value = "3")]
    warmup: usize,

    /// Number of measured iterations
    #[arg(long, default_value = "10")]
    iterations: usize,

    /// Ring buffer depth (number of staging buffers for pipelining)
    #[arg(long, default_value = "32")]
    ring_size: usize,
}

// ---------------------------------------------------------------------------
// SPDK/CUDA initialization
// ---------------------------------------------------------------------------

struct BenchContext {
    block_dev: Arc<BlockDeviceSpdkNvmeComponent>,
    #[allow(dead_code)]
    spdk_env: Arc<SPDKEnvComponent>,
    sector_size: usize,
    ns_id: u32,
}

fn kernel_module_loaded(name: &str) -> bool {
    std::fs::read_to_string("/proc/modules")
        .map(|s| s.lines().any(|line| line.starts_with(&format!("{name} "))))
        .unwrap_or(false)
}

fn initialize_stack(pci: Option<&str>) -> Result<BenchContext, String> {
    extern "C" {
        fn atexit(cb: extern "C" fn()) -> i32;
        fn _exit(status: i32) -> !;
    }
    extern "C" fn exit_hook() {
        unsafe { _exit(0) };
    }
    unsafe { atexit(exit_hook) };

    let mut device_count: std::os::raw::c_int = 0;
    let err = unsafe { cuda_ffi::cudaGetDeviceCount(&mut device_count) };
    if err != cuda_ffi::CUDA_SUCCESS || device_count == 0 {
        return Err("no CUDA GPU available".into());
    }
    unsafe { cuda_ffi::cudaSetDevice(0) };

    spdk_env::checks::check_vfio_available().map_err(|e| format!("VFIO: {e}"))?;
    spdk_env::checks::check_hugepages().map_err(|e| format!("hugepages: {e}"))?;

    let spdk_env_comp = SPDKEnvComponent::new_default();
    let block_dev = BlockDeviceSpdkNvmeComponent::new_default();
    let logger = logger::LoggerComponent::new_default();

    bind(&*spdk_env_comp, "ISPDKEnv", &*block_dev, "spdk_env")
        .map_err(|e| format!("bind spdk_env: {e}"))?;
    bind(&*logger, "ILogger", &*block_dev, "logger")
        .map_err(|e| format!("bind logger: {e}"))?;

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

    let ibd =
        query::<dyn IBlockDevice + Send + Sync>(&*block_dev).ok_or("IBlockDevice query failed")?;
    let channels = ibd
        .connect_client()
        .map_err(|e| format!("connect_client: {e}"))?;
    channels
        .command_tx
        .send(Command::NsProbe)
        .map_err(|e| format!("NsProbe send: {e}"))?;

    let namespaces = match channels.completion_rx.recv() {
        Ok(Completion::NsProbeResult { namespaces }) => namespaces,
        Ok(other) => return Err(format!("unexpected completion: {other:?}")),
        Err(e) => return Err(format!("NsProbe recv: {e}")),
    };
    drop(channels);

    if namespaces.is_empty() {
        return Err("no NVMe namespaces found".into());
    }

    let ns = &namespaces[0];
    eprintln!("NVMe: ns_id={}, sector_size={}, PCI={}", ns.ns_id, ns.sector_size, spdk_addr);
    eprintln!("GPU: {} device(s) available", device_count);

    Ok(BenchContext {
        block_dev,
        spdk_env: spdk_env_comp,
        sector_size: ns.sector_size as usize,
        ns_id: ns.ns_id,
    })
}

// ---------------------------------------------------------------------------
// Pipelined transfer with double-stream overlap
// ---------------------------------------------------------------------------

/// Perform a pipelined NVMe→GPU transfer using a ring of staging buffers
/// and two alternating CUDA streams for maximum overlap.
///
/// Algorithm:
/// 1. Prime the ring with `ring_size` async NVMe reads
/// 2. For each completion:
///    a. Issue cudaMemcpyAsync on stream[completed % 2]
///    b. Sync stream[(completed + 1) % 2] (the PREVIOUS stream) — ensures
///    the previous slot's H2D is done before we reuse that buffer
///    c. Resubmit next NVMe read into the freed ring slot
///
/// This means the current H2D runs concurrently with polling the next NVMe
/// completion, and the sync only happens when we actually need the buffer back.
#[allow(clippy::too_many_arguments)]
fn pipelined_transfer(
    ctx: &BenchContext,
    ring_bufs: &[Arc<Mutex<DmaBuffer>>],
    gpu_src_ptrs: &[*const c_void],
    gpu_dst: *mut c_void,
    num_chunks: usize,
    chunk_size: usize,
    total_size: usize,
    copy_kind: std::os::raw::c_int,
    streams: &[CudaStream; 2],
) -> Result<(), String> {
    let ring_size = ring_bufs.len();
    let sectors_per_chunk = chunk_size / ctx.sector_size;

    let ibd = query::<dyn IBlockDevice + Send + Sync>(&*ctx.block_dev)
        .ok_or("IBlockDevice query failed".to_string())?;
    let channels = ibd
        .connect_client()
        .map_err(|e| format!("connect_client: {e}"))?;

    // Prime the ring: submit initial async reads.
    let prime_count = ring_size.min(num_chunks);
    for i in 0..prime_count {
        let slot = i % ring_size;
        channels
            .command_tx
            .send(Command::ReadAsync {
                ns_id: ctx.ns_id,
                lba: (i as u64) * (sectors_per_chunk as u64),
                buf: Arc::clone(&ring_bufs[slot]),
                timeout_ms: 5000,
                tag: 0,
            })
            .map_err(|e| format!("ReadAsync send #{i}: {e}"))?;
    }

    let mut next_to_submit = prime_count;

    for completed in 0..num_chunks {
        // Wait for the next NVMe completion.
        match channels.completion_rx.recv() {
            Ok(Completion::ReadDone { result, .. }) => {
                result.map_err(|e| format!("NVMe read #{completed}: {e}"))?;
            }
            Ok(Completion::Timeout { handle }) => {
                return Err(format!("NVMe read timeout (handle {:?})", handle));
            }
            Ok(other) => {
                return Err(format!("unexpected completion: {other:?}"));
            }
            Err(e) => {
                return Err(format!("recv #{completed}: {e}"));
            }
        }

        let slot = completed % ring_size;
        let offset = completed * chunk_size;
        let this_chunk = std::cmp::min(chunk_size, total_size - offset);
        let current_stream = streams[completed % 2];

        // Issue async GPU copy on current stream.
        let err = unsafe {
            cudaMemcpyAsync(
                (gpu_dst as *mut u8).add(offset) as *mut c_void,
                gpu_src_ptrs[slot],
                this_chunk,
                copy_kind,
                current_stream,
            )
        };
        if err != cuda_ffi::CUDA_SUCCESS {
            return Err(format!(
                "cudaMemcpyAsync chunk #{completed}: {}",
                cuda_ffi::cuda_error_string(err)
            ));
        }

        // Sync the OTHER stream (previous copy) to ensure that ring slot is free.
        // On the first iteration there's nothing to sync, but cudaStreamSynchronize
        // on an idle stream is a no-op.
        let prev_stream = streams[(completed + 1) % 2];
        let err = unsafe { cudaStreamSynchronize(prev_stream) };
        if err != cuda_ffi::CUDA_SUCCESS {
            return Err(format!(
                "cudaStreamSynchronize: {}",
                cuda_ffi::cuda_error_string(err)
            ));
        }

        // Resubmit next NVMe read into the now-free ring slot.
        if next_to_submit < num_chunks {
            channels
                .command_tx
                .send(Command::ReadAsync {
                    ns_id: ctx.ns_id,
                    lba: (next_to_submit as u64) * (sectors_per_chunk as u64),
                    buf: Arc::clone(&ring_bufs[slot]),
                    timeout_ms: 5000,
                    tag: 0,
                })
                .map_err(|e| format!("ReadAsync send #{next_to_submit}: {e}"))?;
            next_to_submit += 1;
        }
    }

    // Sync both streams to ensure all GPU copies are complete.
    for s in streams {
        let err = unsafe { cudaStreamSynchronize(*s) };
        if err != cuda_ffi::CUDA_SUCCESS {
            return Err(format!(
                "final cudaStreamSynchronize: {}",
                cuda_ffi::cuda_error_string(err)
            ));
        }
    }

    Ok(())
}

// ---------------------------------------------------------------------------
// Bounce-buffer path: NVMe → CUDA-pinned host ring → cudaMemcpyAsync H2D → GPU
//
// Uses cudaHostAlloc + spdk_mem_register so the ring buffers are:
//   - Page-locked for CUDA (true async H2D via DMA engine)
//   - IOMMU-mapped for SPDK (NVMe can DMA directly into them)
// ---------------------------------------------------------------------------

struct BounceBufferState {
    ring_bufs: Vec<Arc<Mutex<DmaBuffer>>>,
    ring_ptrs: Vec<*const c_void>,
    gpu_dst: *mut c_void,
    streams: [CudaStream; 2],
}

unsafe impl Send for BounceBufferState {}

impl BounceBufferState {
    fn new(ring_size: usize, chunk_size: usize, total_size: usize) -> Result<Self, String> {
        let mut ring_bufs = Vec::with_capacity(ring_size);
        let mut ring_ptrs = Vec::with_capacity(ring_size);

        for i in 0..ring_size {
            // Allocate CUDA-pinned host memory.
            let mut host_ptr: *mut c_void = std::ptr::null_mut();
            let err = unsafe {
                cuda_ffi::cudaHostAlloc(&mut host_ptr, chunk_size, cuda_ffi::CUDA_HOST_ALLOC_DEFAULT)
            };
            if err != cuda_ffi::CUDA_SUCCESS {
                return Err(format!(
                    "cudaHostAlloc ring #{i}: {}",
                    cuda_ffi::cuda_error_string(err)
                ));
            }

            // Register with SPDK for NVMe DMA access.
            match create_spdk_dma_buffer_from_cuda_host_alloc(host_ptr, chunk_size) {
                Ok(buf) => {
                    ring_ptrs.push(buf.as_ptr() as *const c_void);
                    ring_bufs.push(Arc::new(Mutex::new(buf)));
                }
                Err(e) => {
                    unsafe { cuda_ffi::cudaFreeHost(host_ptr) };
                    return Err(format!("SPDK register ring #{i}: {e}"));
                }
            }
        }

        let mut gpu_dst: *mut c_void = std::ptr::null_mut();
        let err = unsafe { cuda_ffi::cudaMalloc(&mut gpu_dst, total_size) };
        if err != cuda_ffi::CUDA_SUCCESS {
            return Err(format!("cudaMalloc: {}", cuda_ffi::cuda_error_string(err)));
        }

        let mut streams: [CudaStream; 2] = [std::ptr::null_mut(); 2];
        for (i, s) in streams.iter_mut().enumerate() {
            let err = unsafe { cudaStreamCreate(s) };
            if err != cuda_ffi::CUDA_SUCCESS {
                // Clean up already-created streams.
                for prev in &streams[..i] {
                    if !prev.is_null() {
                        unsafe { cudaStreamDestroy(*prev) };
                    }
                }
                unsafe { cuda_ffi::cudaFree(gpu_dst) };
                return Err(format!("cudaStreamCreate: {}", cuda_ffi::cuda_error_string(err)));
            }
        }

        Ok(Self {
            ring_bufs,
            ring_ptrs,
            gpu_dst,
            streams,
        })
    }

    fn run(
        &self,
        ctx: &BenchContext,
        num_chunks: usize,
        chunk_size: usize,
        total_size: usize,
    ) -> Result<(), String> {
        pipelined_transfer(
            ctx,
            &self.ring_bufs,
            &self.ring_ptrs,
            self.gpu_dst,
            num_chunks,
            chunk_size,
            total_size,
            cuda_ffi::CUDA_MEMCPY_HOST_TO_DEVICE,
            &self.streams,
        )
    }
}

impl Drop for BounceBufferState {
    fn drop(&mut self) {
        for s in &self.streams {
            if !s.is_null() {
                unsafe { cudaStreamDestroy(*s) };
            }
        }
        if !self.gpu_dst.is_null() {
            unsafe { cuda_ffi::cudaFree(self.gpu_dst) };
        }
        // ring_bufs drop handles spdk_mem_unregister + cudaFreeHost
    }
}

// ---------------------------------------------------------------------------
// P2P path: NVMe → GPU BAR1 ring (GDRCopy) → cudaMemcpyAsync D2D → GPU
// ---------------------------------------------------------------------------

struct P2pState {
    ring_bufs: Vec<Arc<Mutex<DmaBuffer>>>,
    dev_ptrs: Vec<*mut c_void>,
    ring_src_ptrs: Vec<*const c_void>,
    gpu_dst: *mut c_void,
    streams: [CudaStream; 2],
}

unsafe impl Send for P2pState {}

impl P2pState {
    fn new(ring_size: usize, chunk_size: usize, total_size: usize) -> Result<Self, String> {
        if !kernel_module_loaded("nvidia_peermem") {
            return Err("nvidia-peermem kernel module not loaded".into());
        }
        if !kernel_module_loaded("gdrdrv") {
            return Err("gdrdrv kernel module not loaded".into());
        }

        let alloc_chunk = std::cmp::max(chunk_size, gpu_services::gdrcopy_ffi::GPU_PAGE_SIZE);
        let mut dev_ptrs: Vec<*mut c_void> = Vec::with_capacity(ring_size);
        let mut ring_bufs = Vec::with_capacity(ring_size);

        for i in 0..ring_size {
            let mut dev_ptr: *mut c_void = std::ptr::null_mut();
            let err = unsafe { cuda_ffi::cudaMalloc(&mut dev_ptr, alloc_chunk) };
            if err != cuda_ffi::CUDA_SUCCESS {
                for p in &dev_ptrs {
                    unsafe { cuda_ffi::cudaFree(*p) };
                }
                return Err(format!(
                    "cudaMalloc staging #{i}: {}",
                    cuda_ffi::cuda_error_string(err)
                ));
            }

            match create_spdk_dma_buffer_from_gpu_bar(dev_ptr, chunk_size) {
                Ok(buf) => {
                    dev_ptrs.push(dev_ptr);
                    ring_bufs.push(Arc::new(Mutex::new(buf)));
                }
                Err(e) => {
                    unsafe { cuda_ffi::cudaFree(dev_ptr) };
                    for p in &dev_ptrs {
                        unsafe { cuda_ffi::cudaFree(*p) };
                    }
                    return Err(format!("GDRCopy setup #{i}: {e}"));
                }
            }
        }

        let ring_src_ptrs: Vec<*const c_void> =
            dev_ptrs.iter().map(|p| *p as *const c_void).collect();

        let mut gpu_dst: *mut c_void = std::ptr::null_mut();
        let err = unsafe { cuda_ffi::cudaMalloc(&mut gpu_dst, total_size) };
        if err != cuda_ffi::CUDA_SUCCESS {
            drop(ring_bufs);
            for p in &dev_ptrs {
                unsafe { cuda_ffi::cudaFree(*p) };
            }
            return Err(format!(
                "cudaMalloc dest: {}",
                cuda_ffi::cuda_error_string(err)
            ));
        }

        let mut streams: [CudaStream; 2] = [std::ptr::null_mut(); 2];
        for (i, s) in streams.iter_mut().enumerate() {
            let err = unsafe { cudaStreamCreate(s) };
            if err != cuda_ffi::CUDA_SUCCESS {
                for prev in &streams[..i] {
                    if !prev.is_null() {
                        unsafe { cudaStreamDestroy(*prev) };
                    }
                }
                drop(ring_bufs);
                for p in &dev_ptrs {
                    unsafe { cuda_ffi::cudaFree(*p) };
                }
                unsafe { cuda_ffi::cudaFree(gpu_dst) };
                return Err(format!("cudaStreamCreate: {}", cuda_ffi::cuda_error_string(err)));
            }
        }

        Ok(Self {
            ring_bufs,
            dev_ptrs,
            ring_src_ptrs,
            gpu_dst,
            streams,
        })
    }

    fn run(
        &self,
        ctx: &BenchContext,
        num_chunks: usize,
        chunk_size: usize,
        total_size: usize,
    ) -> Result<(), String> {
        pipelined_transfer(
            ctx,
            &self.ring_bufs,
            &self.ring_src_ptrs,
            self.gpu_dst,
            num_chunks,
            chunk_size,
            total_size,
            cuda_ffi::CUDA_MEMCPY_DEVICE_TO_DEVICE,
            &self.streams,
        )
    }
}

impl Drop for P2pState {
    fn drop(&mut self) {
        for s in &self.streams {
            if !s.is_null() {
                unsafe { cudaStreamDestroy(*s) };
            }
        }
        self.ring_bufs.clear();
        for p in &self.dev_ptrs {
            unsafe { cuda_ffi::cudaFree(*p) };
        }
        if !self.gpu_dst.is_null() {
            unsafe { cuda_ffi::cudaFree(self.gpu_dst) };
        }
    }
}

// ---------------------------------------------------------------------------
// Measurement
// ---------------------------------------------------------------------------

struct BenchResult {
    label: &'static str,
    total_bytes: usize,
    times_us: Vec<f64>,
}

impl BenchResult {
    fn report(&self) {
        let n = self.times_us.len() as f64;
        let mean = self.times_us.iter().sum::<f64>() / n;
        let mut sorted = self.times_us.clone();
        sorted.sort_by(|a, b| a.partial_cmp(b).unwrap());
        let min = sorted[0];
        let max = sorted[sorted.len() - 1];
        let p50 = sorted[(sorted.len() as f64 * 0.5) as usize];
        let p99 = sorted[((sorted.len() as f64 * 0.99) as usize).min(sorted.len() - 1)];

        let throughput_mbs = (self.total_bytes as f64 / (1024.0 * 1024.0)) / (mean / 1_000_000.0);

        println!(
            "  {:12} | mean {:>9.1} us | min {:>9.1} us | p50 {:>9.1} us | p99 {:>9.1} us | max {:>9.1} us | {:.1} MB/s",
            self.label, mean, min, p50, p99, max, throughput_mbs
        );
    }
}

fn run_benchmark<F>(
    label: &'static str,
    warmup: usize,
    iterations: usize,
    total_bytes: usize,
    mut f: F,
) -> BenchResult
where
    F: FnMut() -> Result<(), String>,
{
    for _ in 0..warmup {
        if let Err(e) = f() {
            eprintln!("ERROR during warmup ({}): {}", label, e);
            std::process::exit(1);
        }
    }

    let mut times_us = Vec::with_capacity(iterations);
    for i in 0..iterations {
        let start = Instant::now();
        if let Err(e) = f() {
            eprintln!("ERROR during iteration {} ({}): {}", i, label, e);
            std::process::exit(1);
        }
        unsafe { cuda_ffi::cudaDeviceSynchronize() };
        let elapsed = start.elapsed();
        times_us.push(elapsed.as_secs_f64() * 1_000_000.0);
    }

    BenchResult {
        label,
        total_bytes,
        times_us,
    }
}

// ---------------------------------------------------------------------------
// Main
// ---------------------------------------------------------------------------

fn main() {
    let cli = Cli::parse();

    if cli.chunk_size == 0 || (cli.chunk_size & (cli.chunk_size - 1)) != 0 {
        eprintln!("error: chunk_size must be a power of 2");
        std::process::exit(1);
    }
    if cli.stream_size < cli.chunk_size {
        eprintln!("error: stream_size must be >= chunk_size");
        std::process::exit(1);
    }
    if cli.ring_size < 2 {
        eprintln!("error: ring_size must be >= 2");
        std::process::exit(1);
    }

    let num_chunks = cli.stream_size.div_ceil(cli.chunk_size);
    let total_size = num_chunks * cli.chunk_size;
    let ring_size = cli.ring_size.min(num_chunks);

    eprintln!("Initializing SPDK/CUDA stack...");
    let ctx = match initialize_stack(cli.pci.as_deref()) {
        Ok(c) => c,
        Err(e) => {
            eprintln!("FATAL: {e}");
            std::process::exit(1);
        }
    };

    println!();
    println!("=== Bounce-Buffer vs GPUDirect P2P Benchmark ===");
    println!("  chunk_size:   {} KiB", cli.chunk_size / 1024);
    println!("  stream_size:  {} KiB ({} chunks)", total_size / 1024, num_chunks);
    println!("  ring_size:    {} buffers", ring_size);
    println!("  warmup:       {} iterations", cli.warmup);
    println!("  measured:     {} iterations", cli.iterations);
    println!();

    // --- Bounce-buffer benchmark ---
    eprintln!("Setting up bounce-buffer path ({} CUDA-pinned ring buffers, 2 streams)...", ring_size);
    let bb = match BounceBufferState::new(ring_size, cli.chunk_size, total_size) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("FATAL (bounce-buffer setup): {e}");
            std::process::exit(1);
        }
    };

    let bb_result = run_benchmark("bounce-buf", cli.warmup, cli.iterations, total_size, || {
        bb.run(&ctx, num_chunks, cli.chunk_size, total_size)
    });

    // --- P2P benchmark ---
    eprintln!("Setting up P2P path ({} GDRCopy ring buffers, 2 streams)...", ring_size);
    let p2p = match P2pState::new(ring_size, cli.chunk_size, total_size) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("FATAL (P2P setup): {e}");
            eprintln!("  Ensure nvidia-peermem and gdrdrv kernel modules are loaded.");
            std::process::exit(1);
        }
    };

    let p2p_result = run_benchmark("p2p-direct", cli.warmup, cli.iterations, total_size, || {
        p2p.run(&ctx, num_chunks, cli.chunk_size, total_size)
    });

    // --- Results ---
    println!(
        "Results (NVMe → GPU, {} KiB stream, {} KiB chunks, ring={}):",
        total_size / 1024,
        cli.chunk_size / 1024,
        ring_size
    );
    bb_result.report();
    p2p_result.report();

    let bb_mean = bb_result.times_us.iter().sum::<f64>() / bb_result.times_us.len() as f64;
    let p2p_mean = p2p_result.times_us.iter().sum::<f64>() / p2p_result.times_us.len() as f64;
    println!();
    if p2p_mean < bb_mean {
        println!("  P2P is {:.2}x faster than bounce-buffer", bb_mean / p2p_mean);
    } else {
        println!(
            "  Bounce-buffer is {:.2}x faster than P2P",
            p2p_mean / bb_mean
        );
    }

    drop(bb);
    drop(p2p);
}
