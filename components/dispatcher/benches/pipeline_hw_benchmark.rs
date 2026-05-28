//! Hardware benchmark for the pipelined SSD→DRAM→GPU transfer path.
//!
//! Requires: NVMe SSD bound to SPDK (VFIO), CUDA GPU, hugepages configured.
//! Run with:
//!   cargo bench -p dispatcher-v1 --features hardware-test --bench pipeline_hw_benchmark

#![cfg(feature = "hardware-test")]
#![allow(clippy::arc_with_non_send_sync)]

use std::ffi::c_void;
use std::sync::Arc;
use std::time::Instant;

use block_device_spdk_nvme::BlockDeviceSpdkNvmeComponent;
use component_core::binding::bind;
use component_core::query_interface;
use gpu_services::cuda_ffi;
use gpu_services::GpuServicesComponent;
use interfaces::{ClientChannels, GpuStream, IBlockDevice, IBlockDeviceAdmin, IGpuServices, PciAddress};
use logger::LoggerComponent;
use spdk_env::SPDKEnvComponent;

// ---------------------------------------------------------------------------
// Hardware context
// ---------------------------------------------------------------------------

struct HwContext {
    block_dev: Arc<BlockDeviceSpdkNvmeComponent>,
    gpu_services: Arc<GpuServicesComponent>,
    #[allow(dead_code)]
    spdk_env: Arc<SPDKEnvComponent>,
    sector_size: usize,
    max_transfer: usize,
}

fn initialize_hw() -> Result<HwContext, String> {
    // CUDA init
    let mut device_count: std::os::raw::c_int = 0;
    let err = unsafe { cuda_ffi::cudaGetDeviceCount(&mut device_count) };
    if err != cuda_ffi::CUDA_SUCCESS || device_count == 0 {
        return Err("no CUDA GPU available".into());
    }
    let err = unsafe { cuda_ffi::cudaSetDevice(0) };
    if err != cuda_ffi::CUDA_SUCCESS {
        return Err(format!(
            "cudaSetDevice(0): {}",
            cuda_ffi::cuda_error_string(err)
        ));
    }

    // SPDK checks
    spdk_env::checks::check_vfio_available().map_err(|e| format!("VFIO: {e}"))?;
    spdk_env::checks::check_hugepages().map_err(|e| format!("hugepages: {e}"))?;

    // Components
    let spdk_env_comp = SPDKEnvComponent::new_default();
    let block_dev = BlockDeviceSpdkNvmeComponent::new_default();
    let gpu_comp = GpuServicesComponent::new_default();
    let logger_comp = LoggerComponent::new_default();

    // Wire SPDK env → block device
    bind(&*spdk_env_comp, "ISPDKEnv", &*block_dev, "spdk_env")
        .map_err(|e| format!("bind spdk_env: {e}"))?;
    bind(&*logger_comp, "ILogger", &*block_dev, "logger")
        .map_err(|e| format!("bind logger→block_dev: {e}"))?;

    // Initialize SPDK
    let ienv = component_core::iunknown::query::<dyn spdk_env::ISPDKEnv + Send + Sync>(
        &*spdk_env_comp,
    )
    .ok_or("ISPDKEnv query failed")?;
    ienv.init().map_err(|e| format!("SPDK init: {e}"))?;

    // Find first NVMe device
    let devices = ienv.devices();
    if devices.is_empty() {
        return Err("no NVMe devices found".into());
    }
    let spdk_addr = devices[0].address;
    eprintln!(
        "NVMe: PCI={}, using first device",
        spdk_addr
    );

    // Initialize block device
    let admin =
        component_core::iunknown::query::<dyn IBlockDeviceAdmin + Send + Sync>(&*block_dev)
            .ok_or("IBlockDeviceAdmin query failed")?;
    admin.set_pci_address(PciAddress {
        domain: spdk_addr.domain,
        bus: spdk_addr.bus,
        dev: spdk_addr.dev,
        func: spdk_addr.func,
    });
    admin
        .initialize()
        .map_err(|e| format!("block device init: {e}"))?;

    // Probe namespace
    let ibd =
        component_core::iunknown::query::<dyn IBlockDevice + Send + Sync>(&*block_dev)
            .ok_or("IBlockDevice query failed")?;
    let sector_size = ibd.block_size() as usize;
    let max_transfer = ibd.max_transfer_size() as usize;

    eprintln!(
        "  sector_size={}, max_transfer={} KiB",
        sector_size,
        max_transfer / 1024
    );

    // Initialize GPU services
    let igpu = query_interface!(gpu_comp, IGpuServices).unwrap();
    igpu.initialize()
        .map_err(|e| format!("GPU services init: {e}"))?;
    eprintln!("GPU: initialized ({} device(s))", device_count);

    Ok(HwContext {
        block_dev,
        gpu_services: gpu_comp,
        spdk_env: spdk_env_comp,
        sector_size,
        max_transfer,
    })
}

// ---------------------------------------------------------------------------
// Benchmark runner
// ---------------------------------------------------------------------------

struct BenchResult {
    label: String,
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
        let p50 = sorted[(sorted.len() as f64 * 0.5) as usize];
        let p99 = sorted[((sorted.len() as f64 * 0.99) as usize).min(sorted.len() - 1)];
        let max = sorted[sorted.len() - 1];
        let throughput_mbs = (self.total_bytes as f64 / (1024.0 * 1024.0)) / (mean / 1_000_000.0);

        println!(
            "  {:>20} | {:>8} KiB | mean {:>9.1} us | min {:>9.1} us | p50 {:>9.1} us | p99 {:>9.1} us | max {:>9.1} us | {:>8.1} MB/s",
            self.label,
            self.total_bytes / 1024,
            mean, min, p50, p99, max, throughput_mbs
        );
    }
}

const WARMUP_ITERS: usize = 5;
const MEASURED_ITERS: usize = 20;

fn run_pipeline_bench(
    ctx: &HwContext,
    ring: &dispatcher::pipeline::PipelineRing,
    total_bytes: usize,
) -> Result<BenchResult, String> {
    let ibd = component_core::iunknown::query::<dyn IBlockDevice + Send + Sync>(&*ctx.block_dev)
        .ok_or("IBlockDevice query failed")?;

    // Allocate DRAM memory-tier destination (page-aligned).
    let aligned_bytes = total_bytes.next_multiple_of(ctx.sector_size);
    let mem_tier_ptr = unsafe { libc::aligned_alloc(4096, aligned_bytes) };
    if mem_tier_ptr.is_null() {
        return Err("aligned_alloc failed for mem_tier".into());
    }
    unsafe { std::ptr::write_bytes(mem_tier_ptr as *mut u8, 0, aligned_bytes) };

    // Allocate GPU destination buffer.
    let mut gpu_dst: *mut c_void = std::ptr::null_mut();
    let err = unsafe { cuda_ffi::cudaMalloc(&mut gpu_dst, aligned_bytes) };
    if err != cuda_ffi::CUDA_SUCCESS {
        unsafe { libc::free(mem_tier_ptr) };
        return Err(format!(
            "cudaMalloc({} bytes): {}",
            aligned_bytes,
            cuda_ffi::cuda_error_string(err)
        ));
    }

    let igpu =
        query_interface!(ctx.gpu_services, IGpuServices).ok_or("IGpuServices query failed")?;

    let label = format!("pipeline_{}KiB", total_bytes / 1024);

    // Warmup
    for _ in 0..WARMUP_ITERS {
        unsafe {
            dispatcher::pipeline::pipelined_ssd_to_gpu(
                &*ibd,
                &*igpu,
                ring,
                mem_tier_ptr as *mut u8,
                gpu_dst,
                0,
                total_bytes,
            )
        }
        .map_err(|e| format!("warmup: {e}"))?;
    }

    // Measured iterations
    let mut times_us = Vec::with_capacity(MEASURED_ITERS);
    for i in 0..MEASURED_ITERS {
        let start = Instant::now();
        unsafe {
            dispatcher::pipeline::pipelined_ssd_to_gpu(
                &*ibd,
                &*igpu,
                ring,
                mem_tier_ptr as *mut u8,
                gpu_dst,
                0,
                total_bytes,
            )
        }
        .map_err(|e| format!("iteration {i}: {e}"))?;
        unsafe { cuda_ffi::cudaDeviceSynchronize() };
        times_us.push(start.elapsed().as_secs_f64() * 1_000_000.0);
    }

    // Cleanup
    unsafe {
        cuda_ffi::cudaFree(gpu_dst);
        libc::free(mem_tier_ptr);
    }

    Ok(BenchResult {
        label,
        total_bytes,
        times_us,
    })
}

fn run_zero_copy_bench(
    ctx: &HwContext,
    streams: &[GpuStream; 2],
    channels: &ClientChannels,
    total_bytes: usize,
) -> Result<BenchResult, String> {
    let ibd = component_core::iunknown::query::<dyn IBlockDevice + Send + Sync>(&*ctx.block_dev)
        .ok_or("IBlockDevice query failed")?;
    let igpu =
        query_interface!(ctx.gpu_services, IGpuServices).ok_or("IGpuServices query failed")?;

    let aligned_bytes = total_bytes.next_multiple_of(ctx.sector_size);

    // Allocate CUDA-pinned + SPDK-registered memory-tier destination.
    let mem_tier_ptr = unsafe { libc::aligned_alloc(4096, aligned_bytes) };
    if mem_tier_ptr.is_null() {
        return Err("aligned_alloc failed for mem_tier".into());
    }
    unsafe { std::ptr::write_bytes(mem_tier_ptr as *mut u8, 0, aligned_bytes) };

    igpu.register_host_memory(mem_tier_ptr, aligned_bytes)
        .map_err(|e| {
            unsafe { libc::free(mem_tier_ptr) };
            format!("register_host_memory: {e}")
        })?;

    // Allocate GPU destination buffer.
    let mut gpu_dst: *mut c_void = std::ptr::null_mut();
    let err = unsafe { cuda_ffi::cudaMalloc(&mut gpu_dst, aligned_bytes) };
    if err != cuda_ffi::CUDA_SUCCESS {
        let _ = igpu.unregister_host_memory(mem_tier_ptr, aligned_bytes);
        unsafe { libc::free(mem_tier_ptr) };
        return Err(format!(
            "cudaMalloc({} bytes): {}",
            aligned_bytes,
            cuda_ffi::cuda_error_string(err)
        ));
    }

    let label = format!("zero_copy_{}KiB", total_bytes / 1024);
    let chunk_size = ctx.max_transfer;

    const MAX_QUEUE_DEPTH: usize = 4;

    // Warmup
    for _ in 0..WARMUP_ITERS {
        unsafe {
            dispatcher::pipeline::pipelined_ssd_to_gpu_zero_copy(
                &*ibd,
                &*igpu,
                streams,
                channels,
                mem_tier_ptr as *mut u8,
                gpu_dst,
                0,
                total_bytes,
                chunk_size,
                MAX_QUEUE_DEPTH,
            )
        }
        .map_err(|e| format!("warmup: {e}"))?;
    }

    // Measured iterations
    let mut times_us = Vec::with_capacity(MEASURED_ITERS);
    for i in 0..MEASURED_ITERS {
        let start = Instant::now();
        unsafe {
            dispatcher::pipeline::pipelined_ssd_to_gpu_zero_copy(
                &*ibd,
                &*igpu,
                streams,
                channels,
                mem_tier_ptr as *mut u8,
                gpu_dst,
                0,
                total_bytes,
                chunk_size,
                MAX_QUEUE_DEPTH,
            )
        }
        .map_err(|e| format!("iteration {i}: {e}"))?;
        unsafe { cuda_ffi::cudaDeviceSynchronize() };
        times_us.push(start.elapsed().as_secs_f64() * 1_000_000.0);
    }

    // Cleanup
    unsafe { cuda_ffi::cudaFree(gpu_dst) };
    let _ = igpu.unregister_host_memory(mem_tier_ptr, aligned_bytes);
    unsafe { libc::free(mem_tier_ptr) };

    Ok(BenchResult {
        label,
        total_bytes,
        times_us,
    })
}

// ---------------------------------------------------------------------------
// Main (criterion harness not used — direct timing like gpu-bb-vs-p2p)
// ---------------------------------------------------------------------------

fn main() {
    // Install atexit hook for clean SPDK shutdown.
    extern "C" {
        fn atexit(cb: extern "C" fn()) -> i32;
        fn _exit(status: i32) -> !;
    }
    extern "C" fn exit_hook() {
        unsafe { _exit(0) };
    }
    unsafe { atexit(exit_hook) };

    eprintln!("Initializing hardware stack...");
    let ctx = match initialize_hw() {
        Ok(c) => c,
        Err(e) => {
            eprintln!("FATAL: {e}");
            std::process::exit(1);
        }
    };

    // Pre-allocate the pipeline ring (CUDA-pinned + SPDK-registered buffers + CUDA streams).
    let igpu =
        query_interface!(ctx.gpu_services, IGpuServices).expect("IGpuServices query failed");
    eprintln!("Allocating pipeline ring ({} CUDA-pinned buffers, {} KiB each)...",
        dispatcher::pipeline::PIPELINE_RING_SIZE, ctx.max_transfer / 1024);
    let ring = dispatcher::pipeline::PipelineRing::new(&*igpu, ctx.max_transfer)
        .expect("PipelineRing allocation failed");

    let stream_sizes: &[usize] = &[
        128 * 1024,       // 128 KiB (single MDTS chunk)
        512 * 1024,       // 512 KiB
        1024 * 1024,      // 1 MiB
        4 * 1024 * 1024,  // 4 MiB
        16 * 1024 * 1024, // 16 MiB
        64 * 1024 * 1024, // 64 MiB
    ];

    println!();
    println!("=== Pipelined SSD→DRAM→GPU Benchmark (dispatcher::pipeline) ===");
    println!("  max_transfer: {} KiB", ctx.max_transfer / 1024);
    println!("  sector_size:  {} bytes", ctx.sector_size);
    println!(
        "  ring_size:    {} buffers",
        dispatcher::pipeline::PIPELINE_RING_SIZE
    );
    println!("  warmup:       {} iterations", WARMUP_ITERS);
    println!("  measured:     {} iterations", MEASURED_ITERS);
    println!();

    let mut results = Vec::new();

    for &size in stream_sizes {
        eprint!("  ring-buffer {} KiB ... ", size / 1024);
        match run_pipeline_bench(&ctx, &ring, size) {
            Ok(r) => {
                eprintln!("done");
                results.push(r);
            }
            Err(e) => {
                eprintln!("FAILED: {e}");
            }
        }
    }

    // Zero-copy benchmark: register memory, cache channels, use direct path.
    let ibd = component_core::iunknown::query::<dyn IBlockDevice + Send + Sync>(&*ctx.block_dev)
        .expect("IBlockDevice query failed");
    let channels = ibd.connect_client().expect("connect_client failed");
    let streams = &ring.streams;

    println!();
    for &size in stream_sizes {
        eprint!("  zero-copy  {} KiB ... ", size / 1024);
        match run_zero_copy_bench(&ctx, streams, &channels, size) {
            Ok(r) => {
                eprintln!("done");
                results.push(r);
            }
            Err(e) => {
                eprintln!("FAILED: {e}");
            }
        }
    }

    println!();
    println!("--- Results ---");
    for r in &results {
        r.report();
    }
    println!();
}
