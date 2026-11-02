//! Hardware benchmark for the dispatcher's zero-copy cold-read pipelines.
//!
//! Measures the three `dispatcher::pipeline` paths the dispatcher uses in
//! production, each at the queue depth its call site uses:
//!
//! | Path | Function | Used by | QD |
//! |---|---|---|---|
//! | single-key SSD→DRAM→GPU | `pipelined_ssd_to_gpu_zero_copy` | `lookup` | 16 |
//! | multi-object SSD→DRAM→GPU | `pipelined_multi_object_zero_copy` | `batch_lookup` | 128 |
//! | SSD→DRAM only | `pipelined_ssd_to_dram_only` | `promote_to_memory_tier` | 16 |
//!
//! All three read NVMe directly into a CUDA-pinned, SPDK-registered host
//! buffer (standing in for a memory-tier slot); the first two then issue
//! async H2D copies from that buffer on the ring's two CUDA streams.
//!
//! The benchmark only reads: it reads whatever is on the first NVMe device,
//! starting at LBA 0, and never writes to it.
//!
//! Requires: NVMe SSD bound to SPDK (VFIO), CUDA GPU, hugepages configured.
//! Run with:
//!   cargo bench -p dispatcher --features hardware-test --bench pipeline_hw_benchmark

#![cfg(feature = "hardware-test")]
#![allow(clippy::arc_with_non_send_sync)]

use std::ffi::c_void;
use std::sync::Arc;
use std::time::Instant;

use block_device_spdk_nvme::BlockDeviceSpdkNvmeComponent;
use component_core::binding::bind;
use component_core::query_interface;
use dispatcher::pipeline::{self, ColdReadJob, PipelineRing};
use gpu_services::cuda_ffi;
use gpu_services::GpuServicesComponent;
use interfaces::{ClientChannels, IBlockDevice, IBlockDeviceAdmin, IGpuServices, PciAddress};
use logger::LoggerComponent;
use spdk_env::SPDKEnvComponent;

/// Queue depth of the single-key `lookup` and `promote_to_memory_tier` paths.
const SINGLE_QUEUE_DEPTH: usize = 16;
/// Queue depth of the `batch_lookup` path.
const BATCH_QUEUE_DEPTH: usize = 128;
/// Objects per `batch_lookup` iteration.
const BATCH_OBJECTS: usize = 8;

const WARMUP_ITERS: usize = 5;
const MEASURED_ITERS: usize = 20;

/// Transfer sizes for the single-object paths.
const SINGLE_SIZES: &[usize] = &[
    128 * 1024,       // 128 KiB (single MDTS chunk)
    512 * 1024,       // 512 KiB
    1024 * 1024,      // 1 MiB
    4 * 1024 * 1024,  // 4 MiB
    16 * 1024 * 1024, // 16 MiB
    64 * 1024 * 1024, // 64 MiB
];

/// Per-object sizes for the batch path (`BATCH_OBJECTS` objects each).
const BATCH_SIZES: &[usize] = &[
    128 * 1024,      // 1 MiB per batch
    512 * 1024,      // 4 MiB per batch
    1024 * 1024,     // 8 MiB per batch
    4 * 1024 * 1024, // 32 MiB per batch
];

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
    let ienv =
        component_core::iunknown::query::<dyn spdk_env::ISPDKEnv + Send + Sync>(&*spdk_env_comp)
            .ok_or("ISPDKEnv query failed")?;
    ienv.init().map_err(|e| format!("SPDK init: {e}"))?;

    // Find first NVMe device
    let devices = ienv.devices();
    if devices.is_empty() {
        return Err("no NVMe devices found".into());
    }
    let spdk_addr = devices[0].address;
    eprintln!("NVMe: PCI={}, using first device", spdk_addr);

    // Initialize block device
    let admin = component_core::iunknown::query::<dyn IBlockDeviceAdmin + Send + Sync>(&*block_dev)
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
    let ibd = component_core::iunknown::query::<dyn IBlockDevice + Send + Sync>(&*block_dev)
        .ok_or("IBlockDevice query failed")?;
    let sector_size = ibd.block_size() as usize;
    let max_transfer = ibd.max_transfer_size() as usize;

    eprintln!(
        "  sector_size={}, max_transfer={} KiB",
        sector_size,
        max_transfer / 1024
    );

    // Initialize GPU services
    let igpu = query_interface!(gpu_comp, IGpuServices).ok_or("IGpuServices query failed")?;
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
// Transfer buffers
// ---------------------------------------------------------------------------

/// A CUDA-pinned + SPDK-registered host buffer (the memory-tier stand-in) and
/// a GPU destination buffer of the same size. Allocated once and reused by
/// every benchmark; freed on drop.
struct Buffers {
    host: *mut c_void,
    gpu: *mut c_void,
    bytes: usize,
    igpu: Arc<dyn IGpuServices + Send + Sync>,
}

impl Buffers {
    fn new(igpu: Arc<dyn IGpuServices + Send + Sync>, bytes: usize) -> Result<Self, String> {
        // SAFETY: plain allocation; checked for null below.
        let host = unsafe { libc::aligned_alloc(4096, bytes) };
        if host.is_null() {
            return Err(format!("aligned_alloc({bytes}) failed"));
        }
        // SAFETY: `host` is a fresh allocation of `bytes` bytes.
        unsafe { std::ptr::write_bytes(host as *mut u8, 0, bytes) };

        if let Err(e) = igpu.register_host_memory(host, bytes) {
            // SAFETY: `host` came from aligned_alloc and is not registered.
            unsafe { libc::free(host) };
            return Err(format!("register_host_memory: {e}"));
        }

        let mut gpu: *mut c_void = std::ptr::null_mut();
        // SAFETY: `gpu` is a valid out-pointer.
        let err = unsafe { cuda_ffi::cudaMalloc(&mut gpu, bytes) };
        if err != cuda_ffi::CUDA_SUCCESS {
            let _ = igpu.unregister_host_memory(host, bytes);
            // SAFETY: `host` came from aligned_alloc and is now unregistered.
            unsafe { libc::free(host) };
            return Err(format!(
                "cudaMalloc({bytes}): {}",
                cuda_ffi::cuda_error_string(err)
            ));
        }

        Ok(Self {
            host,
            gpu,
            bytes,
            igpu,
        })
    }

    fn host_at(&self, offset: usize) -> *mut u8 {
        debug_assert!(offset < self.bytes);
        // SAFETY: callers keep `offset` within the allocation.
        unsafe { (self.host as *mut u8).add(offset) }
    }

    fn gpu_at(&self, offset: usize) -> *mut c_void {
        debug_assert!(offset < self.bytes);
        // SAFETY: callers keep `offset` within the allocation.
        unsafe { (self.gpu as *mut u8).add(offset) as *mut c_void }
    }
}

impl Drop for Buffers {
    fn drop(&mut self) {
        // SAFETY: both pointers were allocated in `new` and are not used after
        // drop; all GPU work on them is synchronized by the benchmarks.
        unsafe { cuda_ffi::cudaFree(self.gpu) };
        let _ = self.igpu.unregister_host_memory(self.host, self.bytes);
        unsafe { libc::free(self.host) };
    }
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
            "  {:>24} | {:>8} KiB | mean {:>9.1} us | min {:>9.1} us | p50 {:>9.1} us | p99 {:>9.1} us | max {:>9.1} us | {:>8.1} MB/s",
            self.label,
            self.total_bytes / 1024,
            mean, min, p50, p99, max, throughput_mbs
        );
    }
}

/// Run `iter` for `WARMUP_ITERS` untimed and `MEASURED_ITERS` timed
/// iterations. Each timed iteration ends with `cudaDeviceSynchronize`, so any
/// GPU copy the path left in flight is included in its time.
fn measure(
    label: String,
    total_bytes: usize,
    mut iter: impl FnMut() -> Result<(), String>,
) -> Result<BenchResult, String> {
    for _ in 0..WARMUP_ITERS {
        iter().map_err(|e| format!("warmup: {e}"))?;
    }
    let mut times_us = Vec::with_capacity(MEASURED_ITERS);
    for i in 0..MEASURED_ITERS {
        let start = Instant::now();
        iter().map_err(|e| format!("iteration {i}: {e}"))?;
        // SAFETY: no preconditions.
        unsafe { cuda_ffi::cudaDeviceSynchronize() };
        times_us.push(start.elapsed().as_secs_f64() * 1_000_000.0);
    }
    Ok(BenchResult {
        label,
        total_bytes,
        times_us,
    })
}

/// Shared state for one benchmark run.
struct Bench<'a> {
    ibd: Arc<dyn IBlockDevice + Send + Sync>,
    igpu: Arc<dyn IGpuServices + Send + Sync>,
    ring: &'a PipelineRing,
    channels: &'a ClientChannels,
    buffers: &'a Buffers,
    sector_size: usize,
}

impl Bench<'_> {
    /// `lookup`: one object, SSD → memory-tier slot → GPU.
    fn single_zero_copy(&self, size: usize) -> Result<BenchResult, String> {
        measure(format!("lookup_{}KiB", size / 1024), size, || {
            // SAFETY: the host buffer is pinned + SPDK-registered and, like the
            // GPU buffer, at least `size` bytes (SINGLE_SIZES ≤ buffer size).
            unsafe {
                pipeline::pipelined_ssd_to_gpu_zero_copy(
                    &*self.ibd,
                    &*self.igpu,
                    &self.ring.streams,
                    self.channels,
                    self.buffers.host_at(0),
                    self.buffers.gpu_at(0),
                    0,
                    size,
                    self.ring.chunk_size,
                    SINGLE_QUEUE_DEPTH,
                )
            }
            .map_err(|e| e.to_string())
        })
    }

    /// `batch_lookup`: `BATCH_OBJECTS` objects at distinct LBAs, interleaved
    /// on one drive, each into its own slot and GPU destination.
    fn batch_zero_copy(&self, object_size: usize) -> Result<BenchResult, String> {
        let stride = object_size.next_multiple_of(self.sector_size);
        let lbas_per_object = (stride / self.sector_size) as u64;
        let jobs: Vec<ColdReadJob> = (0..BATCH_OBJECTS)
            .map(|i| ColdReadJob {
                mem_ptr: self.buffers.host_at(i * stride),
                gpu_dst: self.buffers.gpu_at(i * stride),
                start_lba: i as u64 * lbas_per_object,
                total_bytes: object_size,
                regions: &[],
            })
            .collect();
        let label = format!("batch_{}x{}KiB", BATCH_OBJECTS, object_size / 1024);
        measure(label, object_size * BATCH_OBJECTS, || {
            // SAFETY: each job's slot and destination are disjoint `stride`-byte
            // regions inside the buffers (BATCH_OBJECTS × BATCH_SIZES ≤ buffer size).
            let results = unsafe {
                pipeline::pipelined_multi_object_zero_copy(
                    &*self.ibd,
                    &*self.igpu,
                    &self.ring.streams,
                    self.channels,
                    &jobs,
                    self.ring.chunk_size,
                    BATCH_QUEUE_DEPTH,
                    None,
                )
            };
            match results.into_iter().find_map(Result::err) {
                Some(e) => Err(e.to_string()),
                None => Ok(()),
            }
        })
    }

    /// `promote_to_memory_tier`: one object, SSD → memory-tier slot, no GPU copy.
    fn dram_only(&self, size: usize) -> Result<BenchResult, String> {
        measure(format!("promote_{}KiB", size / 1024), size, || {
            // SAFETY: the host buffer is SPDK-registered and at least `size` bytes.
            unsafe {
                pipeline::pipelined_ssd_to_dram_only(
                    &*self.ibd,
                    self.channels,
                    self.buffers.host_at(0),
                    0,
                    size,
                    self.ring.chunk_size,
                    SINGLE_QUEUE_DEPTH,
                )
            }
            .map_err(|e| e.to_string())
        })
    }
}

fn run_all<F>(name: &str, sizes: &[usize], results: &mut Vec<BenchResult>, mut f: F)
where
    F: FnMut(usize) -> Result<BenchResult, String>,
{
    for &size in sizes {
        eprint!("  {name} {} KiB ... ", size / 1024);
        match f(size) {
            Ok(r) => {
                eprintln!("done");
                results.push(r);
            }
            Err(e) => eprintln!("FAILED: {e}"),
        }
    }
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

    let igpu = query_interface!(ctx.gpu_services, IGpuServices).expect("IGpuServices query failed");
    let ibd = component_core::iunknown::query::<dyn IBlockDevice + Send + Sync>(&*ctx.block_dev)
        .expect("IBlockDevice query failed");

    // The ring holds the two CUDA streams and the MDTS chunk size the
    // dispatcher builds at init. No cold-load staging pool: every benchmark
    // supplies its own registered buffer, as a memory-tier slot would.
    let ring = PipelineRing::new(&*igpu, ctx.max_transfer, 0, 0).expect("PipelineRing::new failed");

    let max_single = SINGLE_SIZES.iter().copied().max().unwrap_or(0);
    let max_batch = BATCH_SIZES
        .iter()
        .map(|s| s.next_multiple_of(ctx.sector_size) * BATCH_OBJECTS)
        .max()
        .unwrap_or(0);
    let buf_bytes = max_single.max(max_batch).next_multiple_of(ctx.sector_size);
    eprintln!(
        "Allocating {} MiB pinned host + GPU buffers...",
        buf_bytes / (1024 * 1024)
    );
    let buffers = Buffers::new(Arc::clone(&igpu), buf_bytes).expect("buffer allocation failed");
    let channels = ibd.connect_client().expect("connect_client failed");

    println!();
    println!("=== Zero-copy cold-read pipelines (dispatcher::pipeline) ===");
    println!("  max_transfer: {} KiB", ctx.max_transfer / 1024);
    println!("  sector_size:  {} bytes", ctx.sector_size);
    println!(
        "  queue depth:  {} (lookup, promote), {} (batch, {} objects)",
        SINGLE_QUEUE_DEPTH, BATCH_QUEUE_DEPTH, BATCH_OBJECTS
    );
    println!("  warmup:       {} iterations", WARMUP_ITERS);
    println!("  measured:     {} iterations", MEASURED_ITERS);
    println!();

    let bench = Bench {
        ibd: Arc::clone(&ibd),
        igpu: Arc::clone(&igpu),
        ring: &ring,
        channels: &channels,
        buffers: &buffers,
        sector_size: ctx.sector_size,
    };

    let mut results = Vec::new();
    run_all("lookup ", SINGLE_SIZES, &mut results, |s| {
        bench.single_zero_copy(s)
    });
    run_all("batch  ", BATCH_SIZES, &mut results, |s| {
        bench.batch_zero_copy(s)
    });
    run_all("promote", SINGLE_SIZES, &mut results, |s| {
        bench.dram_only(s)
    });

    println!();
    println!("--- Results ---");
    for r in &results {
        r.report();
    }
    println!();

    drop(bench);
    drop(buffers);
    ring.destroy(&*igpu);
}
