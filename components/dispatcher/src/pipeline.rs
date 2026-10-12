//! Pipelined SSD→DRAM→GPU transfer engine for the cold lookup path.
//!
//! # Overview
//!
//! When a cache miss occurs (key is on SSD, not in memory-tier), data must be
//! read from NVMe and delivered to the client's GPU. This module implements
//! three progressively optimized pipeline strategies:
//!
//! 1. **`pipelined_ssd_to_gpu_zero_copy`** — Zero-copy single-object pipeline.
//!    NVMe reads directly into the CUDA-pinned memory-tier slot (no intermediate
//!    ring buffer, no CPU memcpy). Each completed segment is immediately DMA'd
//!    to GPU while the next NVMe read is in flight. Used by single-key `lookup()`.
//!
//! 2. **`pipelined_multi_object_zero_copy`** — Multi-object zero-copy pipeline.
//!    Processes N objects concurrently on the same NVMe queue, interleaving
//!    segments from different objects to hide per-object NVMe latency. Used by
//!    `batch_lookup` for parallel cold promotion of multiple keys.
//!
//! # Data flow (cold path)
//!
//! ```text
//! ┌─────────┐  ReadAsync   ┌───────────────────┐  cudaMemcpyAsync  ┌──────────┐
//! │ NVMe SSD│─────DMA──────▶ Memory-Tier Pool   │───────H2D─────────▶ GPU VRAM │
//! │         │              │ (CUDA+SPDK pinned) │                   │          │
//! └─────────┘              └───────────────────┘                   └──────────┘
//!       ▲                         │
//!       │ submit next read        │ on each completion
//!       └─────────────────────────┘
//! ```
//!
//! # Key design decisions
//!
//! - **NVMe queue depth saturation**: keeps `max_queue_depth` reads in flight at
//!   all times via a sliding-window submit/complete loop.
//! - **Dual CUDA streams**: alternates GPU DMA copies between two streams so
//!   the GPU copy engine can overlap transfers.
//! - **Periodic stream sync**: every 8 completions, both streams are synchronized
//!   to bound GPU-side queue depth and prevent unbounded memory pressure.
//! - **Tag-based completion routing**: each NVMe read carries a tag encoding
//!   `(object_index, segment_index)` so out-of-order completions are routed to
//!   the correct memory-tier offset and GPU destination.
//! - **Zero-copy via SPDK+CUDA co-registration**: the memory-tier pool is both
//!   `spdk_mem_register`'d (NVMe can DMA into it) and `cudaHostRegister`'d (GPU
//!   can DMA from it), eliminating all CPU-side data copies.

use std::sync::{Arc, Mutex};

use interfaces::{
    ClientChannels, Command, Completion, DispatcherError, DmaBuffer, GpuStream, IBlockDevice,
    IGpuServices,
};

use crate::io_segmenter;

/// Number of GPU DMA copies before a periodic stream synchronization.
/// Larger values reduce sync overhead at the cost of higher GPU-side memory
/// pressure. 32 amortizes the ~50µs sync cost across more pipeline stages.
pub const PIPELINE_RING_SIZE: usize = 32;

/// Timeout for async NVMe read operations (ms).
const READ_TIMEOUT_MS: u64 = 5000;

/// Fixed pool of pre-registered DMA buffers for staging cold loads when the
/// memory tier is saturated (all slots pinned by in-flight loads).
///
/// A cold load normally promotes into a memory-tier slot, but under pressure no
/// slot can be freed (see `evict_for_space`). Rather than fail the load — which
/// crashes vLLM (`assert transfer_result.success`) — we stage it through one of
/// these buffers: `SSD → staging → GPU`, then return the buffer to the pool
/// without caching. Loads therefore never fail on tier pressure.
///
/// Each buffer is CUDA-pinned + SPDK-registered (via `allocate_pinned_dma_buffer`)
/// so NVMe can DMA into it and the GPU can DMA from it, exactly like a tier slot.
/// `checkout` blocks until a buffer is free; this is deadlock-free because a
/// buffer is held only for the duration of one cold read and released
/// independently of any memory-tier pin.
pub struct StagingPool {
    // Buffers are owned here for their lifetime (freed + unregistered on drop);
    // callers only ever touch the raw pointers via a StagingLease.
    _buffers: Vec<DmaBuffer>,
    ptrs: Vec<*mut u8>,
    buf_bytes: usize,
    free: Mutex<Vec<usize>>,
    cv: std::sync::Condvar,
}

// SAFETY: the raw pointers refer to CUDA-pinned + SPDK-registered host memory
// that is valid for cross-thread DMA use for the pool's lifetime; the owning
// DmaBuffers are never mutated or moved after construction, and free-list access
// is serialized by the Mutex.
unsafe impl Send for StagingPool {}
unsafe impl Sync for StagingPool {}

impl StagingPool {
    /// Allocate `slots` registered buffers of `buf_bytes` each.
    pub fn new(
        gpu: &dyn IGpuServices,
        slots: usize,
        buf_bytes: usize,
    ) -> Result<Self, DispatcherError> {
        let mut buffers = Vec::with_capacity(slots);
        let mut ptrs = Vec::with_capacity(slots);
        for i in 0..slots {
            let buf = gpu.allocate_pinned_dma_buffer(buf_bytes).map_err(|e| {
                DispatcherError::AllocationFailed(format!("staging buffer {i} alloc: {e}"))
            })?;
            ptrs.push(buf.as_ptr() as *mut u8);
            buffers.push(buf);
        }
        Ok(Self {
            _buffers: buffers,
            ptrs,
            buf_bytes,
            free: Mutex::new((0..slots).collect()),
            cv: std::sync::Condvar::new(),
        })
    }

    /// Per-buffer capacity in bytes. A cold load larger than this can't be
    /// staged (the caller must fall back).
    pub fn buf_bytes(&self) -> usize {
        self.buf_bytes
    }

    /// Check out a free buffer, blocking until one is available. The returned
    /// lease owns a clone of the pool `Arc` (so it has no borrow lifetime and
    /// can be stored freely) and returns the buffer to the pool when dropped.
    pub fn checkout(self: &Arc<Self>) -> StagingLease {
        let mut free = self.free.lock().unwrap();
        while free.is_empty() {
            free = self.cv.wait(free).unwrap();
        }
        let idx = free.pop().unwrap();
        StagingLease {
            pool: Arc::clone(self),
            idx,
            ptr: self.ptrs[idx],
        }
    }
}

/// RAII lease of one staging buffer; returns it to the pool on drop.
pub struct StagingLease {
    pool: Arc<StagingPool>,
    idx: usize,
    ptr: *mut u8,
}

impl StagingLease {
    /// Raw pointer to the leased buffer (registered host memory).
    pub fn ptr(&self) -> *mut u8 {
        self.ptr
    }
}

impl Drop for StagingLease {
    fn drop(&mut self) {
        self.pool.free.lock().unwrap().push(self.idx);
        self.pool.cv.notify_one();
    }
}

/// Pre-allocated dual CUDA streams and chunk size for pipelined SSD→GPU transfers.
///
/// Constructed once at dispatcher init and reused across all cold-path reads.
pub struct PipelineRing {
    pub streams: [GpuStream; 2],
    pub chunk_size: usize,
    /// Staging pool for cold loads that can't get a memory-tier slot.
    pub staging: Option<Arc<StagingPool>>,
}

impl PipelineRing {
    /// Allocate CUDA streams and the cold-load staging pool for pipeline use.
    pub fn new(
        gpu: &dyn IGpuServices,
        chunk_size: usize,
        staging_slots: usize,
        staging_buf_bytes: usize,
    ) -> Result<Self, DispatcherError> {
        let stream_a = gpu
            .create_stream()
            .map_err(|e| DispatcherError::IoError(format!("create_stream failed: {e}")))?;
        let stream_b = gpu.create_stream().map_err(|e| {
            let _ = gpu.destroy_stream(stream_a);
            DispatcherError::IoError(format!("create_stream failed: {e}"))
        })?;

        let staging = if staging_slots > 0 {
            match StagingPool::new(gpu, staging_slots, staging_buf_bytes) {
                Ok(p) => Some(Arc::new(p)),
                Err(e) => {
                    let _ = gpu.destroy_stream(stream_a);
                    let _ = gpu.destroy_stream(stream_b);
                    return Err(e);
                }
            }
        } else {
            None
        };

        Ok(Self {
            streams: [stream_a, stream_b],
            chunk_size,
            staging,
        })
    }

    /// Destroy CUDA streams.
    pub fn destroy(self, gpu: &dyn IGpuServices) {
        let _ = gpu.destroy_stream(self.streams[0]);
        let _ = gpu.destroy_stream(self.streams[1]);
    }
}

/// No-op free function for DmaBuffer wrappers over memory-tier regions.
unsafe extern "C" fn noop_free(_ptr: *mut std::ffi::c_void) {}

/// Zero-copy pipeline: read from SSD directly into a memory-tier slot, stream to GPU.
///
/// Unlike [`pipelined_ssd_to_gpu`] which uses intermediate ring buffers and a CPU
/// memcpy per chunk, this reads NVMe directly into the memory-tier slot (which must
/// be CUDA-pinned + SPDK-registered), then issues async H2D from the same memory.
///
/// # Requirements
///
/// The memory-tier pool must have been registered via
/// `IGpuServices::register_host_memory` (i.e., `cudaHostRegister` + `spdk_mem_register`).
///
/// # Safety
///
/// - `mem_tier_ptr` must be a valid, SPDK-registered, CUDA-pinned pointer for `total_bytes`.
/// - `gpu_dst` must be a valid GPU destination pointer for `total_bytes`.
#[allow(clippy::too_many_arguments)]
pub unsafe fn pipelined_ssd_to_gpu_zero_copy(
    drive: &dyn IBlockDevice,
    gpu: &dyn IGpuServices,
    streams: &[GpuStream; 2],
    channels: &ClientChannels,
    mem_tier_ptr: *mut u8,
    gpu_dst: *mut std::ffi::c_void,
    start_lba: u64,
    total_bytes: usize,
    chunk_size: usize,
    max_queue_depth: usize,
) -> Result<(), DispatcherError> {
    let block_size = drive.block_size() as usize;
    let aligned_bytes = total_bytes.next_multiple_of(block_size);

    let segments = io_segmenter::segment_io(
        start_lba,
        aligned_bytes,
        chunk_size as u32,
        block_size as u32,
    );

    if segments.is_empty() {
        return Ok(());
    }

    let num_chunks = segments.len();

    // Create DmaBuffer wrappers for each chunk of the memory-tier slot.
    // These are noop-free since the memory-tier owns the allocation.
    let chunk_bufs: Vec<Arc<Mutex<DmaBuffer>>> = segments
        .iter()
        .map(|seg| {
            let ptr = unsafe { mem_tier_ptr.add(seg.buffer_offset) as *mut std::ffi::c_void };
            let buf_size = seg.length.next_multiple_of(block_size);
            let buf =
                unsafe { DmaBuffer::from_raw(ptr, buf_size, noop_free, -1) }.map_err(|e| {
                    DispatcherError::AllocationFailed(format!("DmaBuffer wrap chunk: {e}"))
                })?;
            Ok(Arc::new(Mutex::new(buf)))
        })
        .collect::<Result<Vec<_>, DispatcherError>>()?;

    // Submit ALL reads upfront with tag = segment index. The tag is echoed
    // back in the completion, allowing us to identify which segment completed
    // and issue its GPU DMA copy immediately — overlapping NVMe I/O with GPU
    // DMA transfers instead of waiting for an entire batch.
    let submit_limit = max_queue_depth.min(num_chunks).max(1);
    let mut submitted = 0usize;
    let mut completed = 0usize;

    // Prime the pipeline: submit up to submit_limit reads.
    while submitted < num_chunks && submitted < submit_limit {
        channels
            .command_tx
            .send(Command::ReadAsync {
                ns_id: 1,
                lba: segments[submitted].lba,
                buf: Arc::clone(&chunk_bufs[submitted]),
                timeout_ms: READ_TIMEOUT_MS,
                tag: submitted as u64,
            })
            .map_err(|e| DispatcherError::IoError(format!("ReadAsync send #{submitted}: {e}")))?;
        submitted += 1;
    }

    // Process completions: on each ReadDone, issue the GPU DMA for that segment,
    // then submit the next read if any remain.
    //
    // Drain until every *submitted* read is accounted for (`completed <
    // submitted`) — never returning early while reads are outstanding, or those
    // reads' completions would be orphaned in the client ring and block the
    // single-threaded block-device actor for the whole drive. On error we record
    // it, stop submitting new reads, and keep draining; the first error wins.
    let mut outcome: Result<(), DispatcherError> = Ok(());
    let mut stop_submitting = false;
    while completed < submitted {
        match channels.completion_rx.recv() {
            Ok(Completion::ReadDone {
                handle,
                tag,
                result,
            }) => {
                completed += 1;
                match result {
                    Err(e) => {
                        if outcome.is_ok() {
                            outcome = Err(DispatcherError::IoError(format!(
                                "SSD read (handle {handle:?}): {e}"
                            )));
                        }
                        stop_submitting = true;
                    }
                    Ok(()) => {
                        let seg_idx = tag as usize;
                        let seg = &segments[seg_idx];
                        // The SSD read already landed in the DRAM slot (chunk_bufs
                        // wrap mem_tier_ptr). A null gpu_dst means "fill the DRAM
                        // slot only" — the multi-region cold path scatters the slot
                        // to its N GPU allocations afterwards, so skip the fused
                        // copy here but keep draining/refilling the read pipeline.
                        if !gpu_dst.is_null() {
                            let copy_len = seg
                                .length
                                .min(total_bytes.saturating_sub(seg.buffer_offset));
                            let current_stream = streams[(completed - 1) % 2];

                            let guard = chunk_bufs[seg_idx].lock().unwrap();
                            let dma = gpu.dma_copy_to_device_async(
                                &guard,
                                unsafe {
                                    (gpu_dst as *mut u8).add(seg.buffer_offset)
                                        as *mut std::ffi::c_void
                                },
                                copy_len,
                                current_stream,
                            );
                            drop(guard);
                            if let Err(e) = dma {
                                if outcome.is_ok() {
                                    outcome = Err(DispatcherError::IoError(format!(
                                        "GPU async DMA copy (seg {seg_idx}) failed: {e}"
                                    )));
                                }
                                stop_submitting = true;
                            } else if completed % PIPELINE_RING_SIZE == 0 {
                                // Periodically bound GPU queue depth (best-effort).
                                let _ = gpu.stream_synchronize(streams[0]);
                                let _ = gpu.stream_synchronize(streams[1]);
                            }
                        }
                    }
                }

                // Keep the pipeline full unless we've hit an error.
                if !stop_submitting && submitted < num_chunks {
                    if channels
                        .command_tx
                        .send(Command::ReadAsync {
                            ns_id: 1,
                            lba: segments[submitted].lba,
                            buf: Arc::clone(&chunk_bufs[submitted]),
                            timeout_ms: READ_TIMEOUT_MS,
                            tag: submitted as u64,
                        })
                        .is_ok()
                    {
                        submitted += 1;
                    } else {
                        if outcome.is_ok() {
                            outcome = Err(DispatcherError::IoError("ReadAsync send failed".into()));
                        }
                        stop_submitting = true;
                    }
                }
            }
            Ok(Completion::Timeout { handle }) => {
                if outcome.is_ok() {
                    outcome = Err(DispatcherError::IoError(format!(
                        "NVMe read timeout (handle {handle:?})"
                    )));
                }
                stop_submitting = true;
                completed += 1;
            }
            Ok(other) => {
                if outcome.is_ok() {
                    outcome = Err(DispatcherError::IoError(format!(
                        "unexpected completion: {other:?}"
                    )));
                }
                stop_submitting = true;
                completed += 1;
            }
            Err(_) => {
                // Actor gone — no further completions will arrive; safe to stop.
                if outcome.is_ok() {
                    outcome = Err(DispatcherError::IoError(
                        "completion channel disconnected".into(),
                    ));
                }
                break;
            }
        }
    }

    // Sync both streams to ensure all in-flight GPU copies are complete
    // (best-effort; the read outcome above is authoritative).
    for s in streams {
        let _ = gpu.stream_synchronize(*s);
    }
    outcome?;

    // Forget all DmaBuffer wrappers (noop_free, but avoid double-free logic).
    for buf in chunk_bufs {
        std::mem::forget(Arc::try_unwrap(buf).ok());
    }

    Ok(())
}

/// Describes a single object to be read from SSD into memory-tier and GPU.
pub struct ColdReadJob {
    pub mem_ptr: *mut u8,
    pub gpu_dst: *mut std::ffi::c_void,
    pub start_lba: u64,
    pub total_bytes: usize,
}

// SAFETY: pointers are valid for the duration of the pipeline call.
unsafe impl Send for ColdReadJob {}

/// Multi-object pipelined SSD→GPU zero-copy transfer.
///
/// Processes multiple objects concurrently on the same NVMe channels,
/// interleaving reads across objects to hide per-object NVMe latency.
/// Each completion is identified by tag = `obj_idx * max_segments + seg_idx`.
///
/// # Safety
/// All `mem_ptr` and `gpu_dst` pointers in `jobs` must be valid for their
/// respective `total_bytes`.
#[allow(clippy::too_many_arguments)]
pub unsafe fn pipelined_multi_object_zero_copy(
    drive: &dyn IBlockDevice,
    gpu: &dyn IGpuServices,
    streams: &[GpuStream; 2],
    channels: &ClientChannels,
    jobs: &[ColdReadJob],
    chunk_size: usize,
    max_queue_depth: usize,
    metrics: Option<&dyn crate::metrics::PipelineMetrics>,
) -> Vec<Result<(), DispatcherError>> {
    let block_size = drive.block_size() as usize;
    let num_jobs = jobs.len();
    let mut results: Vec<Result<(), DispatcherError>> = vec![Ok(()); num_jobs];

    if num_jobs == 0 {
        return results;
    }

    // Segment all objects upfront.
    struct ObjSegments {
        segments: Vec<crate::io_segmenter::IoSegment>,
        chunk_bufs: Vec<Arc<Mutex<DmaBuffer>>>,
    }

    let mut all_objs: Vec<ObjSegments> = Vec::with_capacity(num_jobs);
    let mut total_segments = 0usize;

    for job in jobs {
        let aligned_bytes = job.total_bytes.next_multiple_of(block_size);
        let segments = io_segmenter::segment_io(
            job.start_lba,
            aligned_bytes,
            chunk_size as u32,
            block_size as u32,
        );

        let chunk_bufs: Vec<Arc<Mutex<DmaBuffer>>> = segments
            .iter()
            .map(|seg| {
                let ptr = unsafe { job.mem_ptr.add(seg.buffer_offset) as *mut std::ffi::c_void };
                let buf_size = seg.length.next_multiple_of(block_size);
                let buf = unsafe { DmaBuffer::from_raw(ptr, buf_size, noop_free, -1) }.unwrap();
                Arc::new(Mutex::new(buf))
            })
            .collect();

        total_segments += segments.len();
        all_objs.push(ObjSegments {
            segments,
            chunk_bufs,
        });
    }

    if total_segments == 0 {
        return results;
    }

    // Flatten all segments into a single work queue ordered by object then segment.
    // The tag encoding `obj_idx * max_segments_per_obj + seg_idx` lets us decode
    // which object and segment completed from each NVMe ReadDone event.
    let mut work: Vec<(usize, usize)> = Vec::with_capacity(total_segments);
    for (obj_idx, obj) in all_objs.iter().enumerate() {
        for seg_idx in 0..obj.segments.len() {
            work.push((obj_idx, seg_idx));
        }
    }

    let max_segments_per_obj = all_objs.iter().map(|o| o.segments.len()).max().unwrap_or(0);

    // Submit initial batch up to max_queue_depth.
    let submit_limit = max_queue_depth.min(work.len());
    let mut submitted = 0usize;
    let mut completed = 0usize;
    let mut stream_idx = 0usize;

    let t_submit_start = std::time::Instant::now();
    while submitted < submit_limit {
        let (obj_idx, seg_idx) = work[submitted];
        let obj = &all_objs[obj_idx];
        let tag = (obj_idx * max_segments_per_obj + seg_idx) as u64;

        if channels
            .command_tx
            .send(Command::ReadAsync {
                ns_id: 1,
                lba: obj.segments[seg_idx].lba,
                buf: Arc::clone(&obj.chunk_bufs[seg_idx]),
                timeout_ms: READ_TIMEOUT_MS,
                tag,
            })
            .is_err()
        {
            for result in results.iter_mut().take(num_jobs) {
                *result = Err(DispatcherError::IoError("channel send failed".into()));
            }
            return results;
        }
        submitted += 1;
    }
    let _t_initial_submit = t_submit_start.elapsed();
    let mut t_recv_ns: u64 = 0;
    let mut t_gpu_ns: u64 = 0;
    let mut t_sync_ns: u64 = 0;
    let mut _t_resub_ns: u64 = 0;

    // Coalesced per-object H2D: track remaining segments per object.
    // When an object's last segment completes, issue ONE H2D for the whole
    // contiguous DRAM slot. NVMe resubmission happens BEFORE H2D launch to
    // preserve NVMe/GPU overlap.
    let mut obj_remaining: Vec<usize> = all_objs.iter().map(|o| o.segments.len()).collect();
    let mut obj_failed: Vec<bool> = vec![false; num_jobs];

    let mut stop_submitting = false;
    while completed < submitted {
        let t0 = std::time::Instant::now();
        match channels.completion_rx.recv() {
            Ok(Completion::ReadDone { tag, result, .. }) => {
                t_recv_ns += t0.elapsed().as_nanos() as u64;
                completed += 1;

                let obj_idx = (tag as usize) / max_segments_per_obj;
                let seg_idx = (tag as usize) % max_segments_per_obj;

                // Validate tag bounds.
                if obj_idx >= num_jobs
                    || seg_idx >= all_objs.get(obj_idx).map_or(0, |o| o.segments.len())
                {
                    for r in results.iter_mut() {
                        if r.is_ok() {
                            *r = Err(DispatcherError::IoError("invalid completion tag".into()));
                        }
                    }
                    obj_failed.fill(true);
                    stop_submitting = true;
                } else if let Err(e) = result {
                    results[obj_idx] = Err(DispatcherError::IoError(format!(
                        "SSD read obj={obj_idx} seg={seg_idx}: {e}"
                    )));
                    obj_failed[obj_idx] = true;
                    obj_remaining[obj_idx] = obj_remaining[obj_idx].saturating_sub(1);
                } else {
                    obj_remaining[obj_idx] -= 1;
                }

                // Resubmit next NVMe read BEFORE H2D launch (preserve overlap).
                if !stop_submitting && submitted < work.len() {
                    let tr = std::time::Instant::now();
                    let (next_obj, next_seg) = work[submitted];
                    let next_obj_data = &all_objs[next_obj];
                    let next_tag = (next_obj * max_segments_per_obj + next_seg) as u64;

                    if channels
                        .command_tx
                        .send(Command::ReadAsync {
                            ns_id: 1,
                            lba: next_obj_data.segments[next_seg].lba,
                            buf: Arc::clone(&next_obj_data.chunk_bufs[next_seg]),
                            timeout_ms: READ_TIMEOUT_MS,
                            tag: next_tag,
                        })
                        .is_ok()
                    {
                        submitted += 1;
                    } else {
                        stop_submitting = true;
                    }
                    _t_resub_ns += tr.elapsed().as_nanos() as u64;
                }

                // Object fully read: issue ONE coalesced H2D for the entire slot.
                if obj_idx < num_jobs
                    && obj_remaining[obj_idx] == 0
                    && !obj_failed[obj_idx]
                    && !jobs[obj_idx].gpu_dst.is_null()
                {
                    let tg = std::time::Instant::now();
                    let job = &jobs[obj_idx];
                    let current_stream = streams[stream_idx % 2];

                    let dma_result = gpu.memcpy_h2d_async(
                        job.mem_ptr as *const std::ffi::c_void,
                        job.gpu_dst,
                        job.total_bytes,
                        current_stream,
                    );
                    if let Err(e) = dma_result {
                        results[obj_idx] = Err(DispatcherError::IoError(format!(
                            "GPU H2D obj={obj_idx}: {e}"
                        )));
                    }
                    t_gpu_ns += tg.elapsed().as_nanos() as u64;

                    stream_idx += 1;

                    // Periodic sync to bound GPU queue depth. Threshold is per
                    // completed-object H2D, not per segment.
                    if stream_idx % PIPELINE_RING_SIZE == 0 {
                        let ts = std::time::Instant::now();
                        let _ = gpu.stream_synchronize(streams[0]);
                        let _ = gpu.stream_synchronize(streams[1]);
                        t_sync_ns += ts.elapsed().as_nanos() as u64;
                    }
                }
            }
            Ok(Completion::Timeout { handle }) => {
                for r in results.iter_mut() {
                    if r.is_ok() {
                        *r = Err(DispatcherError::IoError(format!(
                            "NVMe read timeout (handle {handle:?})"
                        )));
                    }
                }
                obj_failed.fill(true);
                stop_submitting = true;
                completed += 1;
            }
            Ok(_) => {
                for r in results.iter_mut() {
                    if r.is_ok() {
                        *r = Err(DispatcherError::IoError(
                            "unexpected completion on cold-read channel".into(),
                        ));
                    }
                }
                obj_failed.fill(true);
                stop_submitting = true;
                completed += 1;
            }
            Err(_) => {
                for r in results.iter_mut() {
                    if r.is_ok() {
                        *r = Err(DispatcherError::IoError(
                            "block-device channel disconnected".into(),
                        ));
                    }
                }
                obj_failed.fill(true);
                break;
            }
        }
    }

    // Any work never submitted (we stopped early on error) has no data on the
    // GPU — mark those objects failed so they aren't reported as loaded.
    for &(obj_idx, _seg_idx) in work.iter().skip(submitted) {
        if results[obj_idx].is_ok() {
            results[obj_idx] = Err(DispatcherError::IoError(
                "cold read aborted before segment was submitted".into(),
            ));
        }
    }

    // Final stream sync.
    let ts_final = std::time::Instant::now();
    let _ = gpu.stream_synchronize(streams[0]);
    let _ = gpu.stream_synchronize(streams[1]);
    let t_final_sync_ns = ts_final.elapsed().as_nanos() as u64;
    t_sync_ns += t_final_sync_ns;

    #[cfg(feature = "pipeline-telemetry")]
    eprintln!(
        "[pipeline-perf] jobs={} segs={} submit={:.2}ms recv_wait={:.2}ms gpu_dma={:.2}ms sync={:.2}ms resub={:.2}ms final_sync={:.2}ms",
        num_jobs,
        total_segments,
        _t_initial_submit.as_secs_f64() * 1000.0,
        t_recv_ns as f64 / 1_000_000.0,
        t_gpu_ns as f64 / 1_000_000.0,
        (t_sync_ns - t_final_sync_ns) as f64 / 1_000_000.0,
        _t_resub_ns as f64 / 1_000_000.0,
        t_final_sync_ns as f64 / 1_000_000.0,
    );

    if let Some(m) = metrics {
        m.record_cold_ssd_read(0, t_recv_ns as f64 / 1000.0);
        m.record_cold_gpu_dma(t_gpu_ns as f64 / 1000.0);
        m.record_cold_stream_sync(t_sync_ns as f64 / 1000.0);
    }

    // Forget DmaBuffer wrappers (memory-tier owns the allocation).
    for obj in all_objs {
        for buf in obj.chunk_bufs {
            std::mem::forget(Arc::try_unwrap(buf).ok());
        }
    }

    results
}

/// One object of a scatter-gather batch: the whole memory-tier slot and the
/// GPU destination of its coalesced H2D.
pub struct ColdObject {
    pub mem_ptr: *mut u8,
    /// GPU destination for the whole object; null means "fill the DRAM slot
    /// only" (the caller scatters a multi-region object afterwards).
    pub gpu_dst: *mut std::ffi::c_void,
    pub total_bytes: usize,
}

// SAFETY: pointers are valid for the duration of the pipeline call.
unsafe impl Send for ColdObject {}

/// One drive's stripe of an object for the scatter-gather pipeline.
pub struct StripeReadJob {
    /// Index of the owning object in the `objects` table.
    pub obj_idx: usize,
    /// Destination of this stripe inside the object's memory-tier slot.
    pub mem_ptr: *mut u8,
    pub start_lba: u64,
    /// Stripe length in bytes (block-aligned).
    pub len: usize,
}

// SAFETY: pointers are valid for the duration of the pipeline call.
unsafe impl Send for StripeReadJob {}

/// Per-drive work descriptor for the scatter-gather pipeline.
pub struct DriveWork<'a> {
    pub channels: &'a ClientChannels,
    pub drive: &'a dyn IBlockDevice,
    /// Stripes this drive reads, object-major in the same object order on
    /// every drive so objects complete progressively.
    pub jobs: &'a [StripeReadJob],
}

/// Segments per sub-object H2D group (512 KiB at the 128 KiB chunk).
pub(crate) const H2D_GROUP_SEGMENTS: usize = 4;
/// Estimated H2D copy-engine rate (22.5 GB/s) for the backlog estimate.
const H2D_EST_BYTES_PER_NS: f64 = 22.5;
/// Estimated fixed cost per H2D copy for the backlog estimate.
const H2D_EST_PER_COPY_NS: u64 = 2_000;
/// Estimated H2D backlog below which ready sub-object groups are copied.
const H2D_LOW_WATER_NS: u64 = 25_000;
/// Per-drive in-flight read cap of the `batch_lookup` scatter.
pub(crate) const BATCH_SCATTER_PER_DRIVE_QD_CAP: usize = 24;
/// Fewest drives at which a policy's per-drive cap applies. The cap trades a
/// little NVMe rate for lower per-command latency, which pays off only when
/// the drives together outrun H2D (measured at 4 drives). With fewer drives
/// each drive's proportional share is the whole budget's 1/N, and capping it
/// costs throughput: at 1 drive, 128 → 24 in flight lost 14% cold load.
pub(crate) const PER_DRIVE_QD_CAP_MIN_DRIVES: usize = 4;

/// Sub-object H2D launch policy of the scatter-gather pipeline.
#[derive(Debug, Clone, Copy)]
pub(crate) struct H2dPolicy {
    /// Ready groups are copied only while the estimated H2D backlog is below
    /// this; 0 never copies groups, `u64::MAX` copies every group once ready.
    pub(crate) low_water_ns: u64,
}

impl H2dPolicy {
    /// One coalesced whole-object copy per object.
    pub(crate) const NEVER_FINE: Self = Self { low_water_ns: 0 };
    /// Ready groups copied early while the backlog is below 25 µs.
    pub(crate) const BACKLOG_AWARE: Self = Self {
        low_water_ns: H2D_LOW_WATER_NS,
    };
    #[cfg(test)]
    pub(crate) const ALWAYS_FINE: Self = Self {
        low_water_ns: u64::MAX,
    };
}

/// H2D launch policy and per-drive window of the scatter-gather pipeline.
#[derive(Debug, Clone, Copy)]
pub(crate) struct ScatterPolicy {
    pub(crate) h2d: H2dPolicy,
    /// Upper bound on in-flight reads per drive; `None` keeps the
    /// proportional share of `max_queue_depth`. Applies only with at least
    /// [`PER_DRIVE_QD_CAP_MIN_DRIVES`] drives.
    pub(crate) per_drive_qd_cap: Option<usize>,
}

/// The public [`scatter_gather_multi_drive_zero_copy`] behaviour: one
/// whole-object H2D per object, proportional queue depth, no cap.
pub(crate) const LEGACY_POLICY: ScatterPolicy = ScatterPolicy {
    h2d: H2dPolicy::NEVER_FINE,
    per_drive_qd_cap: None,
};

/// `batch_lookup`'s multi-object scatter: backlog-aware group copies and, with
/// at least [`PER_DRIVE_QD_CAP_MIN_DRIVES`] drives, at most
/// [`BATCH_SCATTER_PER_DRIVE_QD_CAP`] reads in flight per drive.
pub(crate) const BATCH_SCATTER_POLICY: ScatterPolicy = ScatterPolicy {
    h2d: H2dPolicy::BACKLOG_AWARE,
    per_drive_qd_cap: Some(BATCH_SCATTER_PER_DRIVE_QD_CAP),
};

/// The per-drive cap `policy` applies with `num_drives` drives.
pub(crate) fn effective_qd_cap(policy: ScatterPolicy, num_drives: usize) -> Option<usize> {
    policy
        .per_drive_qd_cap
        .filter(|_| num_drives >= PER_DRIVE_QD_CAP_MIN_DRIVES)
}

/// Per-drive in-flight read budget: `max_queue_depth` split proportionally to
/// each drive's segment count (at least 1, at most its segment count), then
/// trimmed back to the total budget from the last drive, then capped at
/// `per_drive_cap`.
pub(crate) fn split_queue_depth(
    max_queue_depth: usize,
    drive_segments: impl Iterator<Item = usize> + Clone,
    per_drive_cap: Option<usize>,
) -> Vec<usize> {
    let grand_total_segments: usize = drive_segments.clone().sum::<usize>().max(1);
    // Distribute queue depth proportionally to each drive's segment count.
    let mut per_drive_qd: Vec<usize> = drive_segments
        .map(|segs| {
            let share =
                (max_queue_depth as u64 * segs as u64 / grand_total_segments as u64) as usize;
            share.max(1).min(segs)
        })
        .collect();
    // Ensure we don't exceed total budget.
    let total_budget: usize = per_drive_qd.iter().sum();
    if total_budget > max_queue_depth {
        let excess = total_budget - max_queue_depth;
        let mut remaining = excess;
        for qd in per_drive_qd.iter_mut().rev() {
            if remaining == 0 {
                break;
            }
            let reduce = remaining.min(qd.saturating_sub(1));
            *qd -= reduce;
            remaining -= reduce;
        }
    }
    if let Some(cap) = per_drive_cap {
        for qd in per_drive_qd.iter_mut() {
            *qd = (*qd).min(cap.max(1));
        }
    }
    per_drive_qd
}

/// `H2D_GROUP_SEGMENTS` consecutive segments of one stripe, as an in-object
/// byte range clamped to the object's `total_bytes`.
struct H2dGroup {
    obj: usize,
    off: usize,
    len: usize,
    remaining: usize,
    launched: bool,
}

/// Decides when the scatter-gather pipeline copies which bytes of an object
/// to the GPU.
///
/// An object that completes with none of its groups copied gets ONE
/// whole-object copy. While the caller-side estimate of the H2D backlog is
/// below the policy's low-water mark, ready groups (every segment reaped)
/// are copied early, in object order, as maximal contiguous runs; an object
/// that completes after some groups were copied gets the rest as runs.
/// Nothing is copied for a failed object. With a low-water mark of 0 no
/// groups are built at all.
pub(crate) struct H2dPlanner {
    low_water_ns: u64,
    busy_until_ns: u64,
    groups: Vec<H2dGroup>,
    /// Group indices sorted by (object, offset); object `o` owns
    /// `order[obj_start[o]..obj_start[o + 1]]`.
    order: Vec<usize>,
    obj_start: Vec<usize>,
    obj_total: Vec<usize>,
    obj_has_dst: Vec<bool>,
    /// Stripe outside its slot: whole-object copy only.
    obj_coarse: Vec<bool>,
    obj_any_launched: Vec<bool>,
    /// Ready, not yet launched groups per object.
    obj_ready: Vec<usize>,
    /// Bit `o` set iff `obj_ready[o] > 0`.
    ready_mask: Vec<u64>,
    ready_total: usize,
}

impl H2dPlanner {
    /// `objects` yields `(total_bytes, has_gpu_dst)` per object.
    pub(crate) fn new(
        policy: H2dPolicy,
        objects: impl ExactSizeIterator<Item = (usize, bool)>,
    ) -> Self {
        let n = objects.len();
        let mut obj_total = Vec::with_capacity(n);
        let mut obj_has_dst = Vec::with_capacity(n);
        for (total, has_dst) in objects {
            obj_total.push(total);
            obj_has_dst.push(has_dst);
        }
        Self {
            low_water_ns: policy.low_water_ns,
            busy_until_ns: 0,
            groups: Vec::new(),
            order: Vec::new(),
            obj_start: Vec::new(),
            obj_total,
            obj_has_dst,
            obj_coarse: vec![false; n],
            obj_any_launched: vec![false; n],
            obj_ready: vec![0; n],
            ready_mask: vec![0; n.div_ceil(64)],
            ready_total: 0,
        }
    }

    /// Cut one stripe's segments into groups. `stripe_off` is the stripe's
    /// offset inside object `obj`. Returns the index of the stripe's first
    /// group (segment `s` maps to group `base + s / H2D_GROUP_SEGMENTS`), or
    /// `None` if the object has no GPU destination or groups are never
    /// copied.
    pub(crate) fn add_job_groups(
        &mut self,
        obj: usize,
        stripe_off: usize,
        segments: &[io_segmenter::IoSegment],
    ) -> Option<usize> {
        if !self.obj_has_dst[obj] || self.low_water_ns == 0 {
            return None;
        }
        let total = self.obj_total[obj];
        let base = self.groups.len();
        for chunk in segments.chunks(H2D_GROUP_SEGMENTS) {
            let first = &chunk[0];
            let last = &chunk[chunk.len() - 1];
            let start = (stripe_off + first.buffer_offset).min(total);
            let end = (stripe_off + last.buffer_offset + last.length).min(total);
            self.groups.push(H2dGroup {
                obj,
                off: start,
                len: end - start,
                remaining: chunk.len(),
                // Nothing to copy: count it as launched.
                launched: end == start,
            });
        }
        Some(base)
    }

    /// Restrict object `obj` to the whole-object copy.
    pub(crate) fn make_coarse(&mut self, obj: usize) {
        self.obj_coarse[obj] = true;
    }

    /// Build the per-object offset order once every stripe is added.
    pub(crate) fn finish(&mut self) {
        let n = self.obj_total.len();
        self.obj_start = vec![0; n + 1];
        for g in &self.groups {
            self.obj_start[g.obj + 1] += 1;
        }
        for o in 0..n {
            self.obj_start[o + 1] += self.obj_start[o];
        }
        let mut fill = self.obj_start.clone();
        self.order = vec![0; self.groups.len()];
        for (gi, g) in self.groups.iter().enumerate() {
            self.order[fill[g.obj]] = gi;
            fill[g.obj] += 1;
        }
        for o in 0..n {
            let groups = &self.groups;
            self.order[self.obj_start[o]..self.obj_start[o + 1]]
                .sort_unstable_by_key(|&gi| groups[gi].off);
        }
    }

    /// Account one reaped `Ok` segment of group `gid`.
    pub(crate) fn seg_done(&mut self, gid: usize) {
        let g = &mut self.groups[gid];
        if g.remaining == 0 {
            return;
        }
        g.remaining -= 1;
        if g.remaining == 0 && !g.launched {
            let obj = g.obj;
            if self.obj_ready[obj] == 0 {
                self.ready_mask[obj / 64] |= 1 << (obj % 64);
            }
            self.obj_ready[obj] += 1;
            self.ready_total += 1;
        }
    }

    /// Launch whatever is due after a reap (and its resubmit). `completed` is
    /// the object whose last segment this reap was. `emit(obj, off, len)`
    /// issues one copy and returns `false` if it failed, which marks the
    /// object failed.
    pub(crate) fn after_reap(
        &mut self,
        completed: Option<usize>,
        clock: impl Fn() -> u64,
        failed: &mut [bool],
        emit: &mut impl FnMut(usize, usize, usize) -> bool,
    ) {
        if let Some(obj) = completed {
            if !failed[obj] && self.obj_has_dst[obj] {
                let now = clock();
                if self.obj_any_launched[obj] {
                    self.launch_runs(obj, now, false, failed, emit);
                } else {
                    for i in self.obj_start[obj]..self.obj_start[obj + 1] {
                        self.groups[self.order[i]].launched = true;
                    }
                    self.drop_ready(obj);
                    self.obj_any_launched[obj] = true;
                    self.charge(now, self.obj_total[obj]);
                    if !emit(obj, 0, self.obj_total[obj]) {
                        failed[obj] = true;
                    }
                }
                return;
            }
        }
        if self.ready_total == 0 {
            return;
        }
        let now = clock();
        if self.backlog(now) >= self.low_water_ns {
            return;
        }
        let mut from = 0;
        while self.ready_total > 0 {
            let Some(obj) = self.next_ready(from) else {
                break;
            };
            if failed[obj] || self.obj_coarse[obj] {
                self.drop_ready(obj);
            } else if self.launch_runs(obj, now, true, failed, emit) {
                return;
            }
            from = obj + 1;
        }
    }

    /// Launch the ready, unlaunched groups of `obj` as maximal contiguous
    /// runs. With `stop_at_low_water`, returns `true` once the backlog
    /// estimate reaches the low-water mark.
    fn launch_runs(
        &mut self,
        obj: usize,
        now: u64,
        stop_at_low_water: bool,
        failed: &mut [bool],
        emit: &mut impl FnMut(usize, usize, usize) -> bool,
    ) -> bool {
        let end = self.obj_start[obj + 1];
        let mut i = self.obj_start[obj];
        while i < end {
            if !self.is_ready(self.order[i]) {
                i += 1;
                continue;
            }
            let off = self.groups[self.order[i]].off;
            let mut run_end = off + self.groups[self.order[i]].len;
            let mut j = i + 1;
            while j < end
                && self.is_ready(self.order[j])
                && self.groups[self.order[j]].off == run_end
            {
                run_end += self.groups[self.order[j]].len;
                j += 1;
            }
            for k in i..j {
                self.groups[self.order[k]].launched = true;
            }
            self.take_ready(obj, j - i);
            self.obj_any_launched[obj] = true;
            self.charge(now, run_end - off);
            if !emit(obj, off, run_end - off) {
                failed[obj] = true;
                self.drop_ready(obj);
                return false;
            }
            if stop_at_low_water && self.backlog(now) >= self.low_water_ns {
                return true;
            }
            i = j;
        }
        false
    }

    fn is_ready(&self, gid: usize) -> bool {
        let g = &self.groups[gid];
        g.remaining == 0 && !g.launched
    }

    /// First object at or after `from` with ready groups.
    fn next_ready(&self, from: usize) -> Option<usize> {
        let mut w = from / 64;
        if w >= self.ready_mask.len() {
            return None;
        }
        let mut bits = self.ready_mask[w] & (u64::MAX << (from % 64));
        loop {
            if bits != 0 {
                return Some(w * 64 + bits.trailing_zeros() as usize);
            }
            w += 1;
            if w >= self.ready_mask.len() {
                return None;
            }
            bits = self.ready_mask[w];
        }
    }

    /// Count `k` of `obj`'s ready groups as launched.
    fn take_ready(&mut self, obj: usize, k: usize) {
        self.obj_ready[obj] -= k;
        self.ready_total -= k;
        if self.obj_ready[obj] == 0 {
            self.ready_mask[obj / 64] &= !(1 << (obj % 64));
        }
    }

    /// Forget `obj`'s ready groups; they are never launched by the sweep.
    fn drop_ready(&mut self, obj: usize) {
        self.take_ready(obj, self.obj_ready[obj]);
    }

    fn charge(&mut self, now: u64, bytes: usize) {
        self.busy_until_ns = self.busy_until_ns.max(now)
            + (bytes as f64 / H2D_EST_BYTES_PER_NS) as u64
            + H2D_EST_PER_COPY_NS;
    }

    fn backlog(&self, now: u64) -> u64 {
        self.busy_until_ns.saturating_sub(now)
    }
}

/// Multi-drive scatter-gather SSD→GPU zero-copy transfer.
///
/// Fans out NVMe reads to all drives simultaneously and fans in completions
/// via a multiplexed poll loop on the caller thread. An object may be striped
/// across several drives: its remaining segment count is tracked across all
/// drives, and when its last NVMe segment completes (on whichever drive) the
/// fused H2D DMA for the whole object is issued immediately — preserving the
/// NVMe/H2D overlap that is load-bearing. `max_queue_depth` is split across
/// the drives by segment count, with no per-drive cap.
///
/// This public entry point always uses this whole-object rule. The
/// crate-private `batch_lookup` scatter (`scatter_gather_multi_drive_with_policy`
/// with `BATCH_SCATTER_POLICY`) instead copies ready 512 KiB groups early
/// while the H2D backlog is low and caps in-flight reads at 24 per drive.
///
/// Completions are reaped continuously: H2Ds queue on the pipe streams
/// without any intermediate host sync, and one final dual sync covers the
/// whole batch before returning. This is safe because each object's H2D
/// source is its own memory-tier slot, so there is no buffer reuse to guard.
///
/// A failed stripe read fails only its object. A drive-wide failure (timeout,
/// unexpected completion, invalid tag or disconnect) fails every object with
/// a stripe on that drive. A failed object never gets an H2D.
///
/// Returns one `Result` per entry of `objects`.
///
/// # Safety
/// All `mem_ptr` and `gpu_dst` pointers in `objects` must be valid for their
/// respective `total_bytes`, and every stripe `mem_ptr` must be valid for its
/// `len` bytes.
#[allow(clippy::too_many_arguments)]
pub unsafe fn scatter_gather_multi_drive_zero_copy(
    gpu: &dyn IGpuServices,
    streams: &[GpuStream; 2],
    objects: &[ColdObject],
    drive_works: &[DriveWork<'_>],
    chunk_size: usize,
    max_queue_depth: usize,
    metrics: Option<&dyn crate::metrics::PipelineMetrics>,
) -> Vec<Result<(), DispatcherError>> {
    // SAFETY: forwarded caller contract.
    unsafe {
        scatter_gather_multi_drive_with_policy(
            gpu,
            streams,
            objects,
            drive_works,
            chunk_size,
            max_queue_depth,
            metrics,
            LEGACY_POLICY,
        )
    }
}

/// [`scatter_gather_multi_drive_zero_copy`] with an explicit H2D launch
/// policy and per-drive window.
///
/// Under [`LEGACY_POLICY`] this is exactly the public function. Under a
/// policy with a non-zero low-water mark the H2D launch is backlog-aware: the
/// caller keeps an estimate of the copy engine's backlog
/// (`busy_until += bytes / 22.5 GB/s + 2 µs` per copy), and while it is below
/// the mark the 512 KiB groups (4 segments of one stripe) whose reads have
/// all landed `Ok` are copied early, in object order, as maximal contiguous
/// runs, stopping once the estimate is back at the mark. An object that
/// completes after some of its groups were copied gets its remaining bytes
/// as contiguous runs; one with nothing copied early still gets ONE
/// whole-object copy. Every byte of a successful object is copied exactly
/// once, and every launch follows the reaped drive's resubmit. With
/// `per_drive_qd_cap`, at most that many reads are in flight per drive.
///
/// Nothing more is copied for an object once it has failed; groups copied
/// before the failure stay copied, so a failed object returns `Err` but its
/// GPU destination may hold partial bytes.
///
/// # Safety
/// Same contract as [`scatter_gather_multi_drive_zero_copy`].
#[allow(clippy::too_many_arguments)]
pub(crate) unsafe fn scatter_gather_multi_drive_with_policy(
    gpu: &dyn IGpuServices,
    streams: &[GpuStream; 2],
    objects: &[ColdObject],
    drive_works: &[DriveWork<'_>],
    chunk_size: usize,
    max_queue_depth: usize,
    metrics: Option<&dyn crate::metrics::PipelineMetrics>,
    policy: ScatterPolicy,
) -> Vec<Result<(), DispatcherError>> {
    let num_drives = drive_works.len();
    let num_objects = objects.len();
    let mut results: Vec<Result<(), DispatcherError>> = vec![Ok(()); num_objects];

    if num_drives == 0 || num_objects == 0 {
        return results;
    }

    struct DriveSegments {
        objs: Vec<ObjSegs>,
        total_segments: usize,
        work: Vec<(usize, usize)>,
        max_segments_per_obj: usize,
    }

    struct ObjSegs {
        segments: Vec<crate::io_segmenter::IoSegment>,
        chunk_bufs: Vec<Arc<Mutex<DmaBuffer>>>,
        /// First H2D group of this stripe (`None`: no groups).
        h2d_group_base: Option<usize>,
    }

    let mut per_drive: Vec<DriveSegments> = Vec::with_capacity(num_drives);
    let mut grand_total_segments: usize = 0;
    // Remaining segments per object, counted across all drives.
    let mut obj_remaining: Vec<usize> = vec![0; num_objects];
    let mut obj_failed: Vec<bool> = vec![false; num_objects];
    let mut h2d = H2dPlanner::new(
        policy.h2d,
        objects
            .iter()
            .map(|o| (o.total_bytes, !o.gpu_dst.is_null())),
    );

    for dw in drive_works {
        let block_size = dw.drive.block_size() as usize;
        let mut objs: Vec<ObjSegs> = Vec::with_capacity(dw.jobs.len());
        let mut total_segments = 0usize;
        let mut work: Vec<(usize, usize)> = Vec::new();

        for (job_idx, job) in dw.jobs.iter().enumerate() {
            let aligned_bytes = job.len.next_multiple_of(block_size);
            let segments = io_segmenter::segment_io(
                job.start_lba,
                aligned_bytes,
                chunk_size as u32,
                block_size as u32,
            );
            let chunk_bufs: Vec<Arc<Mutex<DmaBuffer>>> = segments
                .iter()
                .map(|seg| {
                    let ptr =
                        unsafe { job.mem_ptr.add(seg.buffer_offset) as *mut std::ffi::c_void };
                    let buf_size = seg.length.next_multiple_of(block_size);
                    let buf = unsafe { DmaBuffer::from_raw(ptr, buf_size, noop_free, -1) }.unwrap();
                    Arc::new(Mutex::new(buf))
                })
                .collect();
            for seg_idx in 0..segments.len() {
                work.push((job_idx, seg_idx));
            }
            total_segments += segments.len();
            if let Some(r) = obj_remaining.get_mut(job.obj_idx) {
                *r += segments.len();
            }
            let h2d_group_base = objects.get(job.obj_idx).and_then(|obj| {
                // The stripe's offset inside its object's slot, checked in
                // release builds too; a stripe outside the slot falls back
                // to the whole-object copy.
                let aligned_obj = obj.total_bytes.next_multiple_of(block_size);
                let stripe_off = (job.mem_ptr as usize)
                    .checked_sub(obj.mem_ptr as usize)
                    .filter(|off| off.saturating_add(aligned_bytes) <= aligned_obj);
                match stripe_off {
                    Some(off) => h2d.add_job_groups(job.obj_idx, off, &segments),
                    None => {
                        h2d.make_coarse(job.obj_idx);
                        None
                    }
                }
            });
            objs.push(ObjSegs {
                segments,
                chunk_bufs,
                h2d_group_base,
            });
        }

        let max_segments_per_obj = objs.iter().map(|o| o.segments.len()).max().unwrap_or(0);
        grand_total_segments += total_segments;
        per_drive.push(DriveSegments {
            objs,
            total_segments,
            work,
            max_segments_per_obj,
        });
    }

    if grand_total_segments == 0 {
        return results;
    }
    h2d.finish();

    // Fail every object that has a stripe on drive `d` (drive-wide failure).
    fn fail_drive_objects(
        dw: &DriveWork<'_>,
        results: &mut [Result<(), DispatcherError>],
        obj_failed: &mut [bool],
        err: impl Fn() -> DispatcherError,
    ) {
        for job in dw.jobs {
            if let Some(r) = results.get_mut(job.obj_idx) {
                if r.is_ok() {
                    *r = Err(err());
                }
                obj_failed[job.obj_idx] = true;
            }
        }
    }

    // Proportional split of the shared budget, then the policy's per-drive
    // cap (only with enough drives): fan-out and the 1-for-1 resubmit keep at
    // most this many in flight.
    let per_drive_qd: Vec<usize> = split_queue_depth(
        max_queue_depth,
        per_drive.iter().map(|ds| ds.total_segments),
        effective_qd_cap(policy, num_drives),
    );

    // Per-drive submission/completion state.
    let mut submitted: Vec<usize> = vec![0; num_drives];
    let mut completed: Vec<usize> = vec![0; num_drives];
    let mut total_submitted: usize = 0;
    let mut total_completed: usize = 0;
    let mut stop_submitting: Vec<bool> = vec![false; num_drives];

    let mut stream_idx: usize = 0;

    // Tag encoding (per drive): job_idx * max_segs + seg_idx. The drive is
    // implicit from which channel completed; the object comes from the job.

    // Fan-out: submit initial segments for each drive.
    for d in 0..num_drives {
        let ds = &per_drive[d];
        let dw = &drive_works[d];
        let limit = per_drive_qd[d].min(ds.work.len());

        while submitted[d] < limit {
            let (job_idx, seg_idx) = ds.work[submitted[d]];
            let obj = &ds.objs[job_idx];
            let tag = (job_idx * ds.max_segments_per_obj + seg_idx) as u64;

            if dw
                .channels
                .command_tx
                .send(Command::ReadAsync {
                    ns_id: 1,
                    lba: obj.segments[seg_idx].lba,
                    buf: Arc::clone(&obj.chunk_bufs[seg_idx]),
                    timeout_ms: READ_TIMEOUT_MS,
                    tag,
                })
                .is_err()
            {
                fail_drive_objects(dw, &mut results, &mut obj_failed, || {
                    DispatcherError::IoError("channel send failed".into())
                });
                stop_submitting[d] = true;
                break;
            }
            submitted[d] += 1;
            total_submitted += 1;
        }
    }

    let mut t_recv_ns: u64 = 0;
    let mut t_gpu_ns: u64 = 0;
    // Covers only the final dual sync; the poll loop never blocks on the GPU.
    let mut t_sync_ns: u64 = 0;
    // Monotonic clock of the H2D backlog estimate.
    let h2d_epoch = std::time::Instant::now();
    let h2d_clock = || h2d_epoch.elapsed().as_nanos() as u64;

    // Multiplexed completion loop: poll all drives' completion_rx channels.
    let mut poll_idx: usize = 0;
    let mut idle_spins: u32 = 0;
    const SPIN_LIMIT: u32 = 64;
    const YIELD_LIMIT: u32 = 256;

    while total_completed < total_submitted {
        let d = poll_idx % num_drives;
        poll_idx += 1;

        if completed[d] >= submitted[d] {
            if poll_idx % num_drives == 0 {
                idle_spins += 1;
                if idle_spins < SPIN_LIMIT {
                    std::hint::spin_loop();
                } else if idle_spins < YIELD_LIMIT {
                    std::thread::yield_now();
                } else {
                    std::thread::park_timeout(std::time::Duration::from_micros(1));
                }
            }
            continue;
        }

        let ds = &per_drive[d];
        let dw = &drive_works[d];
        let t0 = std::time::Instant::now();

        match dw.channels.completion_rx.try_recv() {
            Ok(Completion::ReadDone { tag, result, .. }) => {
                t_recv_ns += t0.elapsed().as_nanos() as u64;
                idle_spins = 0;
                completed[d] += 1;
                total_completed += 1;

                let job_idx = (tag as usize) / ds.max_segments_per_obj;
                let seg_idx = (tag as usize) % ds.max_segments_per_obj;

                let mut done_obj: Option<usize> = None;
                if job_idx >= dw.jobs.len()
                    || seg_idx >= ds.objs.get(job_idx).map_or(0, |o| o.segments.len())
                    || dw.jobs[job_idx].obj_idx >= num_objects
                {
                    fail_drive_objects(dw, &mut results, &mut obj_failed, || {
                        DispatcherError::IoError("invalid completion tag".into())
                    });
                    stop_submitting[d] = true;
                } else {
                    let obj_idx = dw.jobs[job_idx].obj_idx;
                    if let Err(e) = result {
                        if results[obj_idx].is_ok() {
                            results[obj_idx] = Err(DispatcherError::IoError(format!(
                                "SSD read drive={d} obj={obj_idx} seg={seg_idx}: {e}"
                            )));
                        }
                        obj_failed[obj_idx] = true;
                    } else if let Some(base) = ds.objs[job_idx].h2d_group_base {
                        h2d.seg_done(base + seg_idx / H2D_GROUP_SEGMENTS);
                    }
                    obj_remaining[obj_idx] = obj_remaining[obj_idx].saturating_sub(1);
                    done_obj = Some(obj_idx);
                }

                // Resubmit next segment for this drive BEFORE H2D (preserve overlap).
                if !stop_submitting[d] && submitted[d] < ds.work.len() {
                    let (next_job, next_seg) = ds.work[submitted[d]];
                    let next_obj_data = &ds.objs[next_job];
                    let next_tag = (next_job * ds.max_segments_per_obj + next_seg) as u64;

                    if dw
                        .channels
                        .command_tx
                        .send(Command::ReadAsync {
                            ns_id: 1,
                            lba: next_obj_data.segments[next_seg].lba,
                            buf: Arc::clone(&next_obj_data.chunk_bufs[next_seg]),
                            timeout_ms: READ_TIMEOUT_MS,
                            tag: next_tag,
                        })
                        .is_ok()
                    {
                        submitted[d] += 1;
                        total_submitted += 1;
                    } else {
                        stop_submitting[d] = true;
                        // Objects with unsubmitted segments on this drive can
                        // no longer complete: copy none of their groups.
                        for &(job_idx, _) in ds.work.iter().skip(submitted[d]) {
                            if let Some(f) = obj_failed.get_mut(dw.jobs[job_idx].obj_idx) {
                                *f = true;
                            }
                        }
                    }
                }

                // Object fully read on every drive: copy the whole slot (or
                // its not yet copied runs); otherwise, if the policy allows
                // and the H2D backlog is low, copy ready groups early.
                if let Some(obj_idx) = done_obj {
                    let completed = (obj_remaining[obj_idx] == 0).then_some(obj_idx);
                    h2d.after_reap(
                        completed,
                        h2d_clock,
                        &mut obj_failed,
                        &mut |o: usize, off: usize, len: usize| {
                            let tg = std::time::Instant::now();
                            let obj = &objects[o];
                            let current_stream = streams[stream_idx % 2];

                            // SAFETY: the planner's ranges lie within
                            // [0, total_bytes), which both the slot and the
                            // GPU destination cover.
                            let dma_result = unsafe {
                                gpu.memcpy_h2d_async(
                                    obj.mem_ptr.add(off) as *const std::ffi::c_void,
                                    (obj.gpu_dst as *mut u8).add(off) as *mut std::ffi::c_void,
                                    len,
                                    current_stream,
                                )
                            };
                            let ok = match dma_result {
                                Ok(()) => true,
                                Err(e) => {
                                    results[o] = Err(DispatcherError::IoError(format!(
                                        "GPU H2D obj={o} off={off}: {e}"
                                    )));
                                    false
                                }
                            };
                            t_gpu_ns += tg.elapsed().as_nanos() as u64;

                            stream_idx += 1;
                            ok
                        },
                    );
                }
            }
            Ok(Completion::Timeout { handle }) => {
                idle_spins = 0;
                completed[d] += 1;
                total_completed += 1;
                fail_drive_objects(dw, &mut results, &mut obj_failed, || {
                    DispatcherError::IoError(format!(
                        "NVMe read timeout drive={d} (handle {handle:?})"
                    ))
                });
                stop_submitting[d] = true;
            }
            Ok(_) => {
                idle_spins = 0;
                completed[d] += 1;
                total_completed += 1;
                fail_drive_objects(dw, &mut results, &mut obj_failed, || {
                    DispatcherError::IoError("unexpected completion on cold-read channel".into())
                });
                stop_submitting[d] = true;
            }
            Err(component_core::channel::ChannelError::Empty) => {
                // Nothing ready on this drive; continue polling others.
                if poll_idx % num_drives == 0 {
                    idle_spins += 1;
                    if idle_spins < SPIN_LIMIT {
                        std::hint::spin_loop();
                    } else if idle_spins < YIELD_LIMIT {
                        std::thread::yield_now();
                    } else {
                        std::thread::park_timeout(std::time::Duration::from_micros(1));
                    }
                }
            }
            Err(_) => {
                // Channel closed — drive actor gone.
                fail_drive_objects(dw, &mut results, &mut obj_failed, || {
                    DispatcherError::IoError("block-device channel disconnected".into())
                });
                stop_submitting[d] = true;
                // Mark all remaining as completed so we don't spin.
                let delta = submitted[d] - completed[d];
                completed[d] = submitted[d];
                total_completed += delta;
            }
        }
    }

    // Mark objects with unsubmitted stripe segments as failed.
    for d in 0..num_drives {
        let ds = &per_drive[d];
        let dw = &drive_works[d];
        for &(job_idx, _) in ds.work.iter().skip(submitted[d]) {
            let obj_idx = dw.jobs[job_idx].obj_idx;
            if let Some(r) = results.get_mut(obj_idx) {
                if r.is_ok() {
                    *r = Err(DispatcherError::IoError(
                        "cold read aborted before segment was submitted".into(),
                    ));
                }
            }
        }
    }

    // Final stream sync.
    let ts_final = std::time::Instant::now();
    let _ = gpu.stream_synchronize(streams[0]);
    let _ = gpu.stream_synchronize(streams[1]);
    let t_final_sync_ns = ts_final.elapsed().as_nanos() as u64;
    t_sync_ns += t_final_sync_ns;

    if let Some(m) = metrics {
        m.record_cold_ssd_read(0, t_recv_ns as f64 / 1000.0);
        m.record_cold_gpu_dma(t_gpu_ns as f64 / 1000.0);
        m.record_cold_stream_sync(t_sync_ns as f64 / 1000.0);
    }

    // Forget DmaBuffer wrappers (memory-tier owns the allocation).
    for ds in per_drive {
        for obj in ds.objs {
            for buf in obj.chunk_bufs {
                std::mem::forget(Arc::try_unwrap(buf).ok());
            }
        }
    }

    results
}

/// Describes a single object to be promoted from SSD into the memory-tier (no GPU).
pub struct DramPromoteJob {
    pub mem_ptr: *mut u8,
    pub start_lba: u64,
    pub total_bytes: usize,
}

// SAFETY: pointers are valid for the duration of the pipeline call.
unsafe impl Send for DramPromoteJob {}

/// Read from SSD directly into a memory-tier slot without any GPU DMA.
///
/// This is the promote-only variant of [`pipelined_ssd_to_gpu_zero_copy`].
/// It keeps `max_queue_depth` NVMe reads in flight but performs no GPU
/// transfers — used by `promote_to_memory_tier` for pre-warming entries.
///
/// # Safety
/// - `mem_tier_ptr` must be a valid, SPDK-registered pointer for `total_bytes`.
#[allow(clippy::too_many_arguments)]
pub unsafe fn pipelined_ssd_to_dram_only(
    drive: &dyn IBlockDevice,
    channels: &ClientChannels,
    mem_tier_ptr: *mut u8,
    start_lba: u64,
    total_bytes: usize,
    chunk_size: usize,
    max_queue_depth: usize,
) -> Result<(), DispatcherError> {
    let block_size = drive.block_size() as usize;
    let aligned_bytes = total_bytes.next_multiple_of(block_size);

    let segments = io_segmenter::segment_io(
        start_lba,
        aligned_bytes,
        chunk_size as u32,
        block_size as u32,
    );

    if segments.is_empty() {
        return Ok(());
    }

    let num_chunks = segments.len();

    let chunk_bufs: Vec<Arc<Mutex<DmaBuffer>>> = segments
        .iter()
        .map(|seg| {
            let ptr = unsafe { mem_tier_ptr.add(seg.buffer_offset) as *mut std::ffi::c_void };
            let buf_size = seg.length.next_multiple_of(block_size);
            let buf =
                unsafe { DmaBuffer::from_raw(ptr, buf_size, noop_free, -1) }.map_err(|e| {
                    DispatcherError::AllocationFailed(format!("DmaBuffer wrap chunk: {e}"))
                })?;
            Ok(Arc::new(Mutex::new(buf)))
        })
        .collect::<Result<Vec<_>, DispatcherError>>()?;

    let submit_limit = max_queue_depth.min(num_chunks).max(1);
    let mut submitted = 0usize;
    let mut completed = 0usize;

    while submitted < num_chunks && submitted < submit_limit {
        channels
            .command_tx
            .send(Command::ReadAsync {
                ns_id: 1,
                lba: segments[submitted].lba,
                buf: Arc::clone(&chunk_bufs[submitted]),
                timeout_ms: READ_TIMEOUT_MS,
                tag: submitted as u64,
            })
            .map_err(|e| DispatcherError::IoError(format!("ReadAsync send #{submitted}: {e}")))?;
        submitted += 1;
    }

    while completed < num_chunks {
        match channels.completion_rx.recv() {
            Ok(Completion::ReadDone { tag: _, result, .. }) => {
                result.map_err(|e| DispatcherError::IoError(format!("SSD read failed: {e}")))?;

                completed += 1;

                if submitted < num_chunks {
                    channels
                        .command_tx
                        .send(Command::ReadAsync {
                            ns_id: 1,
                            lba: segments[submitted].lba,
                            buf: Arc::clone(&chunk_bufs[submitted]),
                            timeout_ms: READ_TIMEOUT_MS,
                            tag: submitted as u64,
                        })
                        .map_err(|e| {
                            DispatcherError::IoError(format!("ReadAsync send #{submitted}: {e}"))
                        })?;
                    submitted += 1;
                }
            }
            Ok(Completion::Timeout { handle }) => {
                return Err(DispatcherError::IoError(format!(
                    "NVMe read timeout (handle {:?})",
                    handle
                )));
            }
            Ok(other) => {
                return Err(DispatcherError::IoError(format!(
                    "unexpected completion: {other:?}"
                )));
            }
            Err(_) => {
                return Err(DispatcherError::IoError(
                    "completion channel disconnected".into(),
                ));
            }
        }
    }

    for buf in chunk_bufs {
        std::mem::forget(Arc::try_unwrap(buf).ok());
    }

    Ok(())
}

/// Multi-object pipelined SSD→DRAM transfer without GPU DMA.
///
/// Processes multiple objects concurrently on the same NVMe channels.
/// This is the promote-only variant of [`pipelined_multi_object_zero_copy`].
///
/// # Safety
/// All `mem_ptr` pointers in `jobs` must be valid for their respective `total_bytes`.
#[allow(clippy::too_many_arguments)]
pub unsafe fn pipelined_multi_ssd_to_dram_only(
    drive: &dyn IBlockDevice,
    channels: &ClientChannels,
    jobs: &[DramPromoteJob],
    chunk_size: usize,
    max_queue_depth: usize,
) -> Vec<Result<(), DispatcherError>> {
    let block_size = drive.block_size() as usize;
    let num_jobs = jobs.len();
    let mut results: Vec<Result<(), DispatcherError>> = vec![Ok(()); num_jobs];

    if num_jobs == 0 {
        return results;
    }

    struct ObjSegments {
        segments: Vec<crate::io_segmenter::IoSegment>,
        chunk_bufs: Vec<Arc<Mutex<DmaBuffer>>>,
    }

    let mut all_objs: Vec<ObjSegments> = Vec::with_capacity(num_jobs);
    let mut total_segments = 0usize;

    for job in jobs {
        let aligned_bytes = job.total_bytes.next_multiple_of(block_size);
        let segments = io_segmenter::segment_io(
            job.start_lba,
            aligned_bytes,
            chunk_size as u32,
            block_size as u32,
        );

        let chunk_bufs: Vec<Arc<Mutex<DmaBuffer>>> = segments
            .iter()
            .map(|seg| {
                let ptr = unsafe { job.mem_ptr.add(seg.buffer_offset) as *mut std::ffi::c_void };
                let buf_size = seg.length.next_multiple_of(block_size);
                let buf = unsafe { DmaBuffer::from_raw(ptr, buf_size, noop_free, -1) }.unwrap();
                Arc::new(Mutex::new(buf))
            })
            .collect();

        total_segments += segments.len();
        all_objs.push(ObjSegments {
            segments,
            chunk_bufs,
        });
    }

    if total_segments == 0 {
        return results;
    }

    let mut work: Vec<(usize, usize)> = Vec::with_capacity(total_segments);
    for (obj_idx, obj) in all_objs.iter().enumerate() {
        for seg_idx in 0..obj.segments.len() {
            work.push((obj_idx, seg_idx));
        }
    }

    let max_segments_per_obj = all_objs.iter().map(|o| o.segments.len()).max().unwrap_or(0);

    let submit_limit = max_queue_depth.min(work.len());
    let mut submitted = 0usize;
    let mut completed = 0usize;

    while submitted < submit_limit {
        let (obj_idx, seg_idx) = work[submitted];
        let obj = &all_objs[obj_idx];
        let tag = (obj_idx * max_segments_per_obj + seg_idx) as u64;

        if channels
            .command_tx
            .send(Command::ReadAsync {
                ns_id: 1,
                lba: obj.segments[seg_idx].lba,
                buf: Arc::clone(&obj.chunk_bufs[seg_idx]),
                timeout_ms: READ_TIMEOUT_MS,
                tag,
            })
            .is_err()
        {
            for r in results.iter_mut() {
                *r = Err(DispatcherError::IoError("channel send failed".into()));
            }
            return results;
        }
        submitted += 1;
    }

    while completed < work.len() {
        match channels.completion_rx.recv() {
            Ok(Completion::ReadDone { tag, result, .. }) => {
                let obj_idx = (tag as usize) / max_segments_per_obj;
                let seg_idx = (tag as usize) % max_segments_per_obj;
                let _ = seg_idx;

                if let Err(e) = result {
                    results[obj_idx] = Err(DispatcherError::IoError(format!(
                        "SSD read obj={obj_idx}: {e}"
                    )));
                }

                completed += 1;

                if submitted < work.len() {
                    let (next_obj, next_seg) = work[submitted];
                    let next_obj_data = &all_objs[next_obj];
                    let next_tag = (next_obj * max_segments_per_obj + next_seg) as u64;

                    let _ = channels.command_tx.send(Command::ReadAsync {
                        ns_id: 1,
                        lba: next_obj_data.segments[next_seg].lba,
                        buf: Arc::clone(&next_obj_data.chunk_bufs[next_seg]),
                        timeout_ms: READ_TIMEOUT_MS,
                        tag: next_tag,
                    });
                    submitted += 1;
                }
            }
            Ok(Completion::Timeout { handle }) => {
                for r in results.iter_mut() {
                    if r.is_ok() {
                        *r = Err(DispatcherError::IoError(format!(
                            "NVMe read timeout (handle {:?})",
                            handle
                        )));
                    }
                }
                break;
            }
            Ok(_) | Err(_) => {
                for r in results.iter_mut() {
                    if r.is_ok() {
                        *r = Err(DispatcherError::IoError(
                            "unexpected completion or channel disconnect".into(),
                        ));
                    }
                }
                break;
            }
        }
    }

    for obj in all_objs {
        for buf in obj.chunk_bufs {
            std::mem::forget(Arc::try_unwrap(buf).ok());
        }
    }

    results
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pipeline_ring_size_is_reasonable() {
        let size = PIPELINE_RING_SIZE;
        assert!(size >= 2);
        assert!(size <= 64);
    }
}
