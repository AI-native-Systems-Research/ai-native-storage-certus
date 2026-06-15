//! NVMe → BAR1 DMA isolation benchmark.
//!
//! Measures raw NVMe read throughput into two DMA target types using direct
//! SPDK FFI calls (bypasses the actor/channel model entirely):
//! - **host-ram**: CUDA-pinned host memory (baseline, matches spdk_nvme_perf)
//! - **bar1**: GDRCopy-mapped GPU BAR1 memory (the P2P cold-path DMA target)
//!
//! Each drive gets a dedicated CPU-pinned poller thread with its own qpair.
//! No D2D copies, no cudaMemcpy, no streams — pure NVMe DMA completion throughput.
//!
//! Purpose: isolate whether BAR1 DMA has overhead vs host-RAM DMA, and whether
//! that overhead scales with drive count (concurrent writers to the same GPU BAR1).
//!
//! Requires: gdrdrv kernel module (for BAR1 mode). nvidia_peermem may also be needed.

#![allow(clippy::arc_with_non_send_sync)]

use std::ffi::c_void;
use std::sync::atomic::{AtomicU32, Ordering};
use std::time::Instant;

use clap::Parser;

use gpu_services::cuda_ffi;
use gpu_services::dma::{
    create_spdk_dma_buffer_from_cuda_host_alloc, create_spdk_dma_buffer_from_gpu_bar,
};
use interfaces::DmaBuffer;

#[derive(Parser)]
#[command(
    name = "nvme-bar1-bench",
    about = "Isolated NVMe → BAR1 DMA throughput (direct SPDK, no actor overhead)"
)]
struct Cli {
    /// NVMe PCI addresses (repeat for multi-drive). Uses first N if --drive-count given.
    #[arg(long)]
    pci: Vec<String>,

    /// Use first N discovered drives (alternative to --pci)
    #[arg(long)]
    drive_count: Option<usize>,

    /// CUDA device index for BAR1 target
    #[arg(long, default_value = "0")]
    gpu: i32,

    /// Transfer chunk size in bytes (must be power of 2, ≤ NVMe MDTS)
    #[arg(long, default_value = "131072")]
    chunk_size: usize,

    /// Total bytes to read per drive per iteration (use ≥2G to bypass drive cache)
    #[arg(long, default_value = "2147483648")]
    total_bytes: usize,

    /// Queue depth per drive (in-flight NVMe commands per qpair)
    #[arg(long, default_value = "16")]
    queue_depth: usize,

    /// Warmup iterations
    #[arg(long, default_value = "3")]
    warmup: usize,

    /// Measured iterations
    #[arg(long, default_value = "20")]
    iterations: usize,
}

static COMPLETIONS: AtomicU32 = AtomicU32::new(0);

unsafe extern "C" fn read_completion_cb(
    _ctx: *mut c_void,
    _cpl: *const spdk_sys::spdk_nvme_cpl,
) {
    COMPLETIONS.fetch_add(1, Ordering::Relaxed);
}

struct DriveCtx {
    ctrlr: *mut spdk_sys::spdk_nvme_ctrlr,
    ns: *mut spdk_sys::spdk_nvme_ns,
    qpair: *mut spdk_sys::spdk_nvme_qpair,
    sector_size: usize,
    pci_addr: String,
}

struct SpdkState {
    drives: Vec<DriveCtx>,
    ctrlrs: Vec<*mut spdk_sys::spdk_nvme_ctrlr>,
}

fn kernel_module_loaded(name: &str) -> bool {
    std::fs::read_to_string("/proc/modules")
        .map(|s| s.lines().any(|line| line.starts_with(&format!("{name} "))))
        .unwrap_or(false)
}

unsafe extern "C" fn probe_cb(
    _cb_ctx: *mut c_void,
    _trid: *const spdk_sys::spdk_nvme_transport_id,
    _opts: *mut spdk_sys::spdk_nvme_ctrlr_opts,
) -> bool {
    true
}

static mut ATTACHED_CTRLRS: Vec<*mut spdk_sys::spdk_nvme_ctrlr> = Vec::new();

unsafe extern "C" fn attach_cb(
    _cb_ctx: *mut c_void,
    _trid: *const spdk_sys::spdk_nvme_transport_id,
    ctrlr: *mut spdk_sys::spdk_nvme_ctrlr,
    _opts: *const spdk_sys::spdk_nvme_ctrlr_opts,
) {
    ATTACHED_CTRLRS.push(ctrlr);
}

fn initialize_spdk(
    pci_addrs: &[String],
    drive_count: Option<usize>,
    gpu_index: i32,
) -> Result<SpdkState, String> {
    let mut device_count: std::os::raw::c_int = 0;
    let err = unsafe { cuda_ffi::cudaGetDeviceCount(&mut device_count) };
    if err != cuda_ffi::CUDA_SUCCESS || device_count == 0 {
        return Err("no CUDA GPU available".into());
    }
    if gpu_index >= device_count {
        return Err(format!("GPU {} out of range ({} available)", gpu_index, device_count));
    }
    unsafe { cuda_ffi::cudaSetDevice(gpu_index) };
    eprintln!("GPU: device {} selected ({} available)", gpu_index, device_count);

    let spdk_env_comp = spdk_env::SPDKEnvComponent::new_default();
    let ienv = component_core::iunknown::query::<dyn spdk_env::ISPDKEnv + Send + Sync>(
        &*spdk_env_comp,
    )
    .ok_or("ISPDKEnv query failed")?;
    ienv.init().map_err(|e| format!("SPDK init: {e}"))?;

    // Probe all devices
    let mut trid: spdk_sys::spdk_nvme_transport_id = unsafe { std::mem::zeroed() };
    trid.trtype = spdk_sys::spdk_nvme_transport_type_SPDK_NVME_TRANSPORT_PCIE;
    unsafe { ATTACHED_CTRLRS.clear() };
    let rc = unsafe {
        spdk_sys::spdk_nvme_probe(
            &trid,
            std::ptr::null_mut(),
            Some(probe_cb),
            Some(attach_cb),
            None,
        )
    };
    if rc != 0 {
        return Err(format!("spdk_nvme_probe failed (rc={rc})"));
    }

    let all_ctrlrs = unsafe { ATTACHED_CTRLRS.clone() };
    if all_ctrlrs.is_empty() {
        return Err("no NVMe controllers found".into());
    }
    eprintln!("Discovered {} NVMe controller(s)", all_ctrlrs.len());

    // Determine which controllers to use
    let n = if !pci_addrs.is_empty() {
        pci_addrs.len().min(all_ctrlrs.len())
    } else {
        drive_count.unwrap_or(1).min(all_ctrlrs.len())
    };
    let target_ctrlrs = all_ctrlrs[..n].to_vec();

    // Set up qpairs for each target controller
    let mut drives = Vec::with_capacity(target_ctrlrs.len());
    for &ctrlr in &target_ctrlrs {
        let num_ns = unsafe { spdk_sys::spdk_nvme_ctrlr_get_num_ns(ctrlr) };
        let mut ns: *mut spdk_sys::spdk_nvme_ns = std::ptr::null_mut();
        for ns_id in 1..=num_ns {
            let ns_ptr = unsafe { spdk_sys::spdk_nvme_ctrlr_get_ns(ctrlr, ns_id) };
            if !ns_ptr.is_null() && unsafe { spdk_sys::spdk_nvme_ns_is_active(ns_ptr) } {
                ns = ns_ptr;
                break;
            }
        }
        if ns.is_null() {
            eprintln!("WARNING: controller has no active namespace, skipping");
            continue;
        }

        let sector_size = unsafe { spdk_sys::spdk_nvme_ns_get_sector_size(ns) } as usize;

        let qpair = unsafe {
            spdk_sys::spdk_nvme_ctrlr_alloc_io_qpair(ctrlr, std::ptr::null(), 0)
        };
        if qpair.is_null() {
            return Err("failed to allocate IO qpair".into());
        }

        let pci_addr = format!("drive-{}", drives.len());

        eprintln!("  Drive: {} (sector_size={})", pci_addr, sector_size);
        drives.push(DriveCtx { ctrlr, ns, qpair, sector_size, pci_addr });
    }

    if drives.is_empty() {
        return Err("no usable drives".into());
    }

    Ok(SpdkState { drives, ctrlrs: all_ctrlrs })
}

/// Per-drive poller thread context.
struct DriveWork {
    ctrlr: *mut spdk_sys::spdk_nvme_ctrlr,
    ns: *mut spdk_sys::spdk_nvme_ns,
    bufs: *const *mut c_void,
    num_bufs: usize,
    sector_size: usize,
    chunk_size: usize,
    num_chunks: usize,
    queue_depth: usize,
}

// SAFETY: raw pointers are valid for the lifetime of the benchmark.
// SPDK ctrlr/ns are shared read-only; qpair is allocated per-thread.
unsafe impl Send for DriveWork {}
unsafe impl Sync for DriveWork {}

/// Single-drive poller: allocates its own qpair, runs submit/poll loop.
fn drive_poller(work: DriveWork) -> Result<(), String> {
    // Allocate qpair on THIS thread (SPDK requirement)
    let qpair = unsafe {
        spdk_sys::spdk_nvme_ctrlr_alloc_io_qpair(work.ctrlr, std::ptr::null(), 0)
    };
    if qpair.is_null() {
        return Err("failed to allocate qpair on poller thread".into());
    }

    let sectors_per_chunk = (work.chunk_size / work.sector_size) as u32;
    let qd = work.queue_depth.min(work.num_bufs).min(work.num_chunks);
    let bufs = unsafe { std::slice::from_raw_parts(work.bufs, work.num_bufs) };

    let mut submitted = 0usize;
    let mut completed = 0usize;
    let mut lba = 0u64;

    // Prime
    while submitted < qd {
        let slot = submitted % work.num_bufs;
        let rc = unsafe {
            spdk_sys::spdk_nvme_ns_cmd_read(
                work.ns,
                qpair,
                bufs[slot],
                lba,
                sectors_per_chunk,
                Some(read_completion_cb),
                std::ptr::null_mut(),
                0,
            )
        };
        if rc != 0 {
            unsafe { spdk_sys::spdk_nvme_ctrlr_free_io_qpair(qpair) };
            return Err(format!("submit failed (rc={rc})"));
        }
        lba += sectors_per_chunk as u64;
        submitted += 1;
    }

    // Poll + resubmit
    while completed < work.num_chunks {
        let n = unsafe {
            spdk_sys::spdk_nvme_qpair_process_completions(qpair, 0)
        };
        if n < 0 {
            unsafe { spdk_sys::spdk_nvme_ctrlr_free_io_qpair(qpair) };
            return Err(format!("process_completions error (rc={n})"));
        }
        if n > 0 {
            for _ in 0..n {
                completed += 1;
                if submitted < work.num_chunks {
                    let slot = submitted % work.num_bufs;
                    let rc = unsafe {
                        spdk_sys::spdk_nvme_ns_cmd_read(
                            work.ns,
                            qpair,
                            bufs[slot],
                            lba,
                            sectors_per_chunk,
                            Some(read_completion_cb),
                            std::ptr::null_mut(),
                            0,
                        )
                    };
                    if rc != 0 {
                        unsafe { spdk_sys::spdk_nvme_ctrlr_free_io_qpair(qpair) };
                        return Err(format!("resubmit failed (rc={rc})"));
                    }
                    lba += sectors_per_chunk as u64;
                    submitted += 1;
                }
            }
        }
    }

    unsafe { spdk_sys::spdk_nvme_ctrlr_free_io_qpair(qpair) };
    Ok(())
}

/// Multi-drive pipelined read with per-drive poller threads.
/// Each thread allocates its own qpair. No barrier — threads run all
/// iterations back-to-back. Main thread measures total wall-clock for
/// warmup+iterations chunks across all drives.
fn multi_drive_bench(
    drives: &[DriveCtx],
    bufs: &[Vec<*mut c_void>],
    num_chunks_per_drive: usize,
    chunk_size: usize,
    queue_depth: usize,
    warmup: usize,
    iterations: usize,
) -> Result<Vec<f64>, String> {
    use std::thread;

    let num_drives = drives.len();
    let total_runs = warmup + iterations;
    let chunks_total = num_chunks_per_drive * total_runs;

    let t_start = Instant::now();

    let handles: Vec<_> = drives
        .iter()
        .enumerate()
        .map(|(d, drive)| {
            let ctrlr = drive.ctrlr as usize;
            let ns = drive.ns as usize;
            let buf_ptr = bufs[d].as_ptr() as usize;
            let num_bufs = bufs[d].len();
            let sector_size = drive.sector_size;
            thread::spawn(move || -> Result<(), String> {
                // Pin to dedicated CPU core (starting at core 2 to avoid core 0/1)
                unsafe {
                    let mut cpuset: libc::cpu_set_t = std::mem::zeroed();
                    libc::CPU_SET(d + 2, &mut cpuset);
                    libc::sched_setaffinity(0, std::mem::size_of::<libc::cpu_set_t>(), &cpuset);
                }

                let ctrlr = ctrlr as *mut spdk_sys::spdk_nvme_ctrlr;
                let ns = ns as *mut spdk_sys::spdk_nvme_ns;
                let bufs = unsafe { std::slice::from_raw_parts(buf_ptr as *const *mut c_void, num_bufs) };

                let qpair = unsafe {
                    spdk_sys::spdk_nvme_ctrlr_alloc_io_qpair(ctrlr, std::ptr::null(), 0)
                };
                if qpair.is_null() {
                    return Err("failed to allocate qpair".into());
                }

                let sectors_per_chunk = (chunk_size / sector_size) as u32;
                let qd = queue_depth.min(num_bufs).min(num_chunks_per_drive);

                // Run all iterations without barriers
                let mut submitted = 0usize;
                let mut completed = 0usize;
                let mut lba = 0u64;

                // Prime
                while submitted < qd {
                    let slot = submitted % num_bufs;
                    let rc = unsafe {
                        spdk_sys::spdk_nvme_ns_cmd_read(
                            ns, qpair, bufs[slot], lba,
                            sectors_per_chunk, Some(read_completion_cb),
                            std::ptr::null_mut(), 0,
                        )
                    };
                    if rc != 0 {
                        unsafe { spdk_sys::spdk_nvme_ctrlr_free_io_qpair(qpair) };
                        return Err(format!("submit failed (rc={rc})"));
                    }
                    lba += sectors_per_chunk as u64;
                    submitted += 1;
                }

                while completed < chunks_total {
                    let n = unsafe {
                        spdk_sys::spdk_nvme_qpair_process_completions(qpair, 0)
                    };
                    if n > 0 {
                        for _ in 0..n {
                            completed += 1;
                            if submitted < chunks_total {
                                let slot = submitted % num_bufs;
                                let rc = unsafe {
                                    spdk_sys::spdk_nvme_ns_cmd_read(
                                        ns, qpair, bufs[slot], lba,
                                        sectors_per_chunk, Some(read_completion_cb),
                                        std::ptr::null_mut(), 0,
                                    )
                                };
                                if rc != 0 {
                                    unsafe { spdk_sys::spdk_nvme_ctrlr_free_io_qpair(qpair) };
                                    return Err(format!("resubmit failed (rc={rc})"));
                                }
                                lba += sectors_per_chunk as u64;
                                submitted += 1;
                            }
                        }
                    }
                }

                unsafe { spdk_sys::spdk_nvme_ctrlr_free_io_qpair(qpair) };
                Ok(())
            })
        })
        .collect();

    for (d, h) in handles.into_iter().enumerate() {
        h.join()
            .map_err(|_| format!("drive {} thread panicked", d))?
            .map_err(|e| format!("drive {}: {}", d, e))?;
    }

    let elapsed = t_start.elapsed();
    let total_bytes_all = (num_chunks_per_drive * chunk_size * num_drives * total_runs) as f64;
    let measured_bytes = (num_chunks_per_drive * chunk_size * num_drives * iterations) as f64;

    // Estimate per-iteration time from total wall clock
    let us_per_iter = elapsed.as_secs_f64() * 1_000_000.0 / total_runs as f64;
    let times_us: Vec<f64> = vec![us_per_iter; iterations];

    let _ = total_bytes_all;
    let _ = measured_bytes;
    Ok(times_us)
}

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
        let max = sorted[sorted.len() - 1];
        let gbps = (self.total_bytes as f64) / (mean / 1_000_000.0) / 1e9;

        println!(
            "  {:12} | mean {:>8.1} us | min {:>8.1} | p50 {:>8.1} | max {:>8.1} | {:>5.2} GB/s",
            self.label, mean, min, p50, max, gbps
        );
    }

    fn mean_gbps(&self) -> f64 {
        let mean = self.times_us.iter().sum::<f64>() / self.times_us.len() as f64;
        (self.total_bytes as f64) / (mean / 1_000_000.0) / 1e9
    }
}

fn main() {
    let cli = Cli::parse();

    if cli.chunk_size == 0 || (cli.chunk_size & (cli.chunk_size - 1)) != 0 {
        eprintln!("error: chunk_size must be a power of 2");
        std::process::exit(1);
    }
    if cli.total_bytes < cli.chunk_size {
        eprintln!("error: total_bytes must be >= chunk_size");
        std::process::exit(1);
    }

    let num_chunks_per_drive = cli.total_bytes.div_ceil(cli.chunk_size);
    let total_per_drive = num_chunks_per_drive * cli.chunk_size;

    eprintln!("Initializing SPDK/CUDA...");
    let state = match initialize_spdk(&cli.pci, cli.drive_count, cli.gpu) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("FATAL: {e}");
            std::process::exit(1);
        }
    };

    let num_drives = state.drives.len();
    let total_bytes = total_per_drive * num_drives;

    println!();
    println!("=== NVMe -> BAR1 DMA Isolation Benchmark (direct SPDK) ===");
    println!("  drives:       {}", num_drives);
    println!("  chunk_size:   {} KiB", cli.chunk_size / 1024);
    println!("  total_bytes:  {} MiB per drive ({} chunks)", total_per_drive / (1024 * 1024), num_chunks_per_drive);
    println!("  total_data:   {} MiB ({} drives)", total_bytes / (1024 * 1024), num_drives);
    println!("  queue_depth:  {} per drive", cli.queue_depth);
    println!("  warmup:       {}", cli.warmup);
    println!("  iterations:   {}", cli.iterations);
    println!();

    let ring_size = cli.queue_depth;

    // --- Host-RAM mode ---
    eprintln!("Setting up host-ram buffers ({} per drive)...", ring_size);
    let mut host_ptrs: Vec<Vec<*mut c_void>> = Vec::new();
    let mut _host_bufs: Vec<Vec<DmaBuffer>> = Vec::new();

    for _ in 0..num_drives {
        let mut ptrs = Vec::with_capacity(ring_size);
        let mut bufs = Vec::with_capacity(ring_size);
        for i in 0..ring_size {
            let mut ptr: *mut c_void = std::ptr::null_mut();
            let err = unsafe {
                cuda_ffi::cudaHostAlloc(&mut ptr, cli.chunk_size, cuda_ffi::CUDA_HOST_ALLOC_DEFAULT)
            };
            if err != cuda_ffi::CUDA_SUCCESS {
                eprintln!("FATAL: cudaHostAlloc #{i}: {}", cuda_ffi::cuda_error_string(err));
                std::process::exit(1);
            }
            match create_spdk_dma_buffer_from_cuda_host_alloc(ptr, cli.chunk_size) {
                Ok(buf) => {
                    ptrs.push(buf.as_ptr());
                    bufs.push(buf);
                }
                Err(e) => {
                    eprintln!("FATAL: SPDK register: {e}");
                    std::process::exit(1);
                }
            }
        }
        host_ptrs.push(ptrs);
        _host_bufs.push(bufs);
    }

    let host_times = multi_drive_bench(
        &state.drives, &host_ptrs, num_chunks_per_drive,
        cli.chunk_size, cli.queue_depth, cli.warmup, cli.iterations,
    ).unwrap_or_else(|e| { eprintln!("FATAL: {e}"); std::process::exit(1); });
    let host_result = BenchResult { label: "host-ram".into(), total_bytes, times_us: host_times };

    // --- BAR1 mode ---
    if !kernel_module_loaded("gdrdrv") {
        eprintln!("WARNING: gdrdrv not loaded — skipping BAR1 mode");
        println!("Results:");
        host_result.report();
        println!("\n  BAR1 mode: SKIPPED (gdrdrv not loaded)");
        std::process::exit(0);
    }

    eprintln!("Setting up bar1 buffers ({} per drive, GDRCopy-mapped)...", ring_size);
    let alloc_chunk = std::cmp::max(cli.chunk_size, gpu_services::gdrcopy_ffi::GPU_PAGE_SIZE);
    let mut bar1_ptrs: Vec<Vec<*mut c_void>> = Vec::new();
    let mut _bar1_bufs: Vec<Vec<DmaBuffer>> = Vec::new();
    let mut bar1_dev_ptrs: Vec<Vec<*mut c_void>> = Vec::new();

    for _ in 0..num_drives {
        let mut ptrs = Vec::with_capacity(ring_size);
        let mut bufs = Vec::with_capacity(ring_size);
        let mut dev_ptrs = Vec::with_capacity(ring_size);
        for i in 0..ring_size {
            let mut dev_ptr: *mut c_void = std::ptr::null_mut();
            let err = unsafe { cuda_ffi::cudaMalloc(&mut dev_ptr, alloc_chunk) };
            if err != cuda_ffi::CUDA_SUCCESS {
                eprintln!("FATAL: cudaMalloc #{i}: {}", cuda_ffi::cuda_error_string(err));
                std::process::exit(1);
            }
            match create_spdk_dma_buffer_from_gpu_bar(dev_ptr, cli.chunk_size) {
                Ok(buf) => {
                    ptrs.push(buf.as_ptr());
                    dev_ptrs.push(dev_ptr);
                    bufs.push(buf);
                }
                Err(e) => {
                    unsafe { cuda_ffi::cudaFree(dev_ptr) };
                    eprintln!("FATAL: GDRCopy #{i}: {e}");
                    std::process::exit(1);
                }
            }
        }
        bar1_ptrs.push(ptrs);
        _bar1_bufs.push(bufs);
        bar1_dev_ptrs.push(dev_ptrs);
    }

    let bar1_times = multi_drive_bench(
        &state.drives, &bar1_ptrs, num_chunks_per_drive,
        cli.chunk_size, cli.queue_depth, cli.warmup, cli.iterations,
    ).unwrap_or_else(|e| { eprintln!("FATAL: {e}"); std::process::exit(1); });
    let bar1_result = BenchResult { label: "bar1".into(), total_bytes, times_us: bar1_times };

    // --- Results ---
    println!("Results ({} drive(s), {} MiB total per iteration):", num_drives, total_bytes / (1024*1024));
    host_result.report();
    bar1_result.report();
    println!();

    let host_gbps = host_result.mean_gbps();
    let bar1_gbps = bar1_result.mean_gbps();
    let overhead_pct = (1.0 - bar1_gbps / host_gbps) * 100.0;

    if overhead_pct > 0.0 {
        println!("  BAR1 overhead: {:.1}% slower than host-ram", overhead_pct);
    } else {
        println!("  BAR1 is {:.1}% faster than host-ram", overhead_pct.abs());
    }
    println!("  Per-drive: host-ram {:.2} GB/s, bar1 {:.2} GB/s", host_gbps / num_drives as f64, bar1_gbps / num_drives as f64);

    // Cleanup
    drop(_bar1_bufs);
    for dev_ptrs in &bar1_dev_ptrs {
        for p in dev_ptrs {
            unsafe { cuda_ffi::cudaFree(*p) };
        }
    }
    drop(_host_bufs);

    for drive in &state.drives {
        unsafe { spdk_sys::spdk_nvme_ctrlr_free_io_qpair(drive.qpair) };
    }
    for &ctrlr in &state.ctrlrs {
        unsafe { let _ = spdk_sys::spdk_nvme_detach(ctrlr); };
    }
}
