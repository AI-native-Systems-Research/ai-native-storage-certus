//! Multi-threaded IOPS benchmark for NVMe via SPDK.
//!
//! Each worker thread gets its own I/O queue pair and maintains a configurable
//! number of outstanding (async) I/Os. This directly exercises the NVMe device
//! at full queue depth without the overhead of the actor abstraction.
//!
//! # Configuration (environment variables)
//!
//! | Variable    | Default | Description                                |
//! |-------------|---------|--------------------------------------------|
//! | `THREADS`   | 1       | Number of worker threads (1 qpair each)    |
//! | `QD`        | 32      | Queue depth per thread (outstanding I/Os)  |
//! | `BLOCK_SIZE`| 1       | I/O size in sectors                        |
//! | `DURATION`  | 10      | Measurement duration in seconds            |
//! | `READ_PCT`  | 100     | Read percentage (0 = all writes)           |
//! | `RANDOM`    | 1       | 1 = random LBAs, 0 = sequential            |
//!
//! # Prerequisites
//!
//! - NVMe device(s) bound to `vfio-pci`
//! - Hugepages allocated (e.g. `echo 1024 > /proc/sys/vm/nr_hugepages`)
//! - Run with sufficient permissions (root or vfio group member)
//!
//! ```bash
//! THREADS=4 QD=32 DURATION=10 cargo run --example iops_bench
//! ```

use component_framework::iunknown::query;
use example_logger::{ILogger, LoggerComponent};
use spdk_env::{DmaBuffer, ISPDKEnv, SPDKEnvComponent};
use spdk_simple_block_device::io::{
    self, alloc_qpair, free_qpair, poll_completions, submit_read, submit_write,
};
use std::sync::atomic::{AtomicBool, AtomicI32, AtomicU64, Ordering};
use std::sync::{Arc, Barrier, RwLock};
use std::time::Instant;

fn env_or(key: &str, default: u64) -> u64 {
    std::env::var(key)
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(default)
}

/// Per-slot state for one outstanding I/O.
///
/// A pointer to this struct is passed as `cb_arg` to the NVMe completion
/// callback. The struct must not move while I/O is in-flight.
struct IoSlot {
    buf: DmaBuffer,
    done: AtomicBool,
    status: AtomicI32,
}

/// NVMe completion callback for benchmark slots.
///
/// # Safety
///
/// `cb_arg` must point to a valid `IoSlot` that outlives the I/O.
unsafe extern "C" fn bench_completion_cb(
    cb_arg: *mut std::ffi::c_void,
    cpl: *const spdk_sys::spdk_nvme_cpl,
) {
    let slot = &*(cb_arg as *const IoSlot);
    let cpl_status = if cpl.is_null() {
        -1
    } else {
        let status = (*cpl).__bindgen_anon_1.status;
        let raw: u16 = std::mem::transmute(status);
        let masked = (raw >> 1) & 0x3FFF;
        masked as i32
    };
    slot.status.store(cpl_status, Ordering::Release);
    slot.done.store(true, Ordering::Release);
}

/// Simple xorshift64* PRNG (fast, no alloc, good enough for LBA generation).
struct Rng(u64);

impl Rng {
    fn new(seed: u64) -> Self {
        Self(seed | 1)
    }

    fn next(&mut self) -> u64 {
        let mut x = self.0;
        x ^= x >> 12;
        x ^= x << 25;
        x ^= x >> 27;
        self.0 = x;
        x.wrapping_mul(0x2545F4914F6CDD1D)
    }
}

/// Submit one I/O for the given slot (read or write based on `is_read`).
///
/// # Safety
///
/// `slot_ptr` must point to a valid `IoSlot`. The slot's `done` flag must
/// already be set to `false` before calling.
unsafe fn submit_one(
    state: &io::InnerState,
    lba: u64,
    slot_ptr: *mut IoSlot,
    is_read: bool,
) {
    let cb_arg = slot_ptr as *mut std::ffi::c_void;
    if is_read {
        unsafe {
            submit_read(
                state,
                lba,
                &mut (*slot_ptr).buf,
                Some(bench_completion_cb),
                cb_arg,
            )
            .expect("submit_read failed");
        }
    } else {
        unsafe {
            submit_write(
                state,
                lba,
                &(*slot_ptr).buf,
                Some(bench_completion_cb),
                cb_arg,
            )
            .expect("submit_write failed");
        }
    }
}

/// Wrapper to send `*mut spdk_nvme_ctrlr` across threads.
///
/// # Safety
///
/// The pointer must remain valid for the lifetime of the receiving thread.
/// Each thread allocates its own qpair — controller/namespace are thread-safe
/// for this purpose.
struct SendCtrlr(*mut spdk_sys::spdk_nvme_ctrlr);
unsafe impl Send for SendCtrlr {}

struct SendNs(*mut spdk_sys::spdk_nvme_ns);
unsafe impl Send for SendNs {}

fn worker(
    thread_id: usize,
    ctrlr: SendCtrlr,
    ns: SendNs,
    sector_size: u32,
    num_sectors: u64,
    qd: usize,
    block_size: u32,
    duration_secs: u64,
    read_pct: u64,
    random: bool,
    barrier: Arc<Barrier>,
    total_ios: Arc<AtomicU64>,
) {
    let state = unsafe {
        alloc_qpair(ctrlr.0, ns.0, sector_size, num_sectors).expect("alloc_qpair failed")
    };

    let io_size = block_size as usize * sector_size as usize;
    let max_lba = num_sectors.saturating_sub(block_size as u64);

    let mut slots: Vec<IoSlot> = (0..qd)
        .map(|_| IoSlot {
            buf: DmaBuffer::new(io_size, sector_size as usize, None).expect("DMA alloc failed"),
            done: AtomicBool::new(true),
            status: AtomicI32::new(0),
        })
        .collect();

    // Fill write buffers with a pattern.
    for slot in &mut slots {
        for (i, byte) in slot.buf.as_mut_slice().iter_mut().enumerate() {
            *byte = ((i * 7 + 13) % 256) as u8;
        }
    }

    let mut rng = Rng::new(0xDEAD_BEEF_0000 + thread_id as u64);
    let mut seq_lba: u64 = 0;
    let mut io_count: u64 = 0;

    let next_lba = |rng: &mut Rng, seq: &mut u64| -> u64 {
        if random {
            rng.next() % (max_lba + 1)
        } else {
            let l = *seq;
            *seq = (*seq + block_size as u64) % (max_lba + 1);
            l
        }
    };

    // Wait for all threads to be ready.
    barrier.wait();
    let start = Instant::now();
    let deadline = std::time::Duration::from_secs(duration_secs);

    // Submit initial QD I/Os.
    // SAFETY: slots Vec doesn't move; each slot_ptr remains valid.
    let slots_base = slots.as_mut_ptr();
    for i in 0..qd {
        let lba = next_lba(&mut rng, &mut seq_lba);
        let is_read = (rng.next() % 100) < read_pct;
        unsafe {
            let slot_ptr = slots_base.add(i);
            (*slot_ptr).done.store(false, Ordering::Release);
            submit_one(&state, lba, slot_ptr, is_read);
        }
    }

    // Main I/O loop: poll completions, resubmit.
    while start.elapsed() < deadline {
        poll_completions(&state, 0);

        for i in 0..qd {
            unsafe {
                let slot_ptr = slots_base.add(i);
                if !(*slot_ptr).done.load(Ordering::Acquire) {
                    continue;
                }

                io_count += 1;

                let lba = next_lba(&mut rng, &mut seq_lba);
                let is_read = (rng.next() % 100) < read_pct;
                (*slot_ptr).done.store(false, Ordering::Release);
                submit_one(&state, lba, slot_ptr, is_read);
            }
        }
    }

    // Drain in-flight I/Os.
    loop {
        let all_done = (0..qd).all(|i| unsafe {
            (*slots_base.add(i)).done.load(Ordering::Acquire)
        });
        if all_done {
            break;
        }
        poll_completions(&state, 0);
    }

    let elapsed = start.elapsed();
    let iops = io_count as f64 / elapsed.as_secs_f64();
    println!(
        "  Thread {thread_id}: {iops:.0} IOPS ({io_count} I/Os in {:.2}s)",
        elapsed.as_secs_f64()
    );

    total_ios.fetch_add(io_count, Ordering::Relaxed);

    free_qpair(state);
}

fn main() {
    let threads = env_or("THREADS", 1) as usize;
    let qd = env_or("QD", 32) as usize;
    let block_size = env_or("BLOCK_SIZE", 1) as u32;
    let duration_secs = env_or("DURATION", 10);
    let read_pct = env_or("READ_PCT", 100).min(100);
    let random = env_or("RANDOM", 1) != 0;

    println!("=== IOPS Benchmark ===");
    println!(
        "Threads: {threads}, QD/thread: {qd}, Block size: {block_size} sector(s), Duration: {duration_secs}s"
    );
    println!(
        "Read%: {read_pct}, Pattern: {}\n",
        if random { "random" } else { "sequential" }
    );

    // --- Initialize SPDK environment ---

    let logger = LoggerComponent::new();
    let env_comp = SPDKEnvComponent::new(RwLock::new(Vec::new()), AtomicBool::new(false));

    let ilogger = query::<dyn ILogger + Send + Sync>(&*logger).expect("ILogger not found");
    env_comp
        .logger
        .connect(ilogger)
        .expect("env logger connect failed");

    env_comp.init().expect("SPDK environment init failed");
    println!(
        "SPDK environment initialized. Found {} device(s).",
        env_comp.device_count()
    );
    for dev in env_comp.devices() {
        println!("  Device: {} ({})", dev.address, dev.device_type);
    }

    // --- Open the primary device (probe + attach + first qpair) ---

    let env: Arc<dyn ISPDKEnv + Send + Sync> =
        query::<dyn ISPDKEnv + Send + Sync>(&*env_comp).expect("ISPDKEnv not found");

    let primary = io::open_device(&*env).expect("open_device failed");
    let sector_size = primary.sector_size;
    let num_sectors = primary.num_sectors;
    let ctrlr = primary.ctrlr;
    let ns = primary.ns;

    println!(
        "\nDevice open: sector_size={sector_size}, num_sectors={num_sectors}, capacity={}MB",
        (num_sectors * sector_size as u64) / (1024 * 1024)
    );
    println!("Starting benchmark...\n");

    // --- Spawn worker threads ---

    let barrier = Arc::new(Barrier::new(threads));
    let total_ios = Arc::new(AtomicU64::new(0));

    let handles: Vec<_> = (0..threads)
        .map(|tid| {
            let barrier = barrier.clone();
            let total_ios = total_ios.clone();
            let ctrlr_send = SendCtrlr(ctrlr);
            let ns_send = SendNs(ns);

            std::thread::Builder::new()
                .name(format!("iops-worker-{tid}"))
                .spawn(move || {
                    worker(
                        tid,
                        ctrlr_send,
                        ns_send,
                        sector_size,
                        num_sectors,
                        qd,
                        block_size,
                        duration_secs,
                        read_pct,
                        random,
                        barrier,
                        total_ios,
                    );
                })
                .expect("thread spawn failed")
        })
        .collect();

    let bench_start = Instant::now();
    for h in handles {
        h.join().expect("worker thread panicked");
    }
    let bench_elapsed = bench_start.elapsed();

    // --- Report ---

    let total = total_ios.load(Ordering::Relaxed);
    let total_iops = total as f64 / bench_elapsed.as_secs_f64();
    let bw_mbs = total_iops * (block_size as f64 * sector_size as f64) / (1024.0 * 1024.0);
    let avg_lat_us = if total > 0 {
        (threads as f64 * qd as f64) / total_iops * 1_000_000.0
    } else {
        0.0
    };

    println!();
    println!("Total: {total_iops:.0} IOPS ({bw_mbs:.1} MB/s)");
    println!("Avg latency: {avg_lat_us:.1} us");

    // --- Cleanup ---

    io::close_device(primary);
    println!("\n=== Done ===");
}
