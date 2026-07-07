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
pub const PIPELINE_RING_SIZE: usize = 8;

/// Timeout for async NVMe read operations (ms).
const READ_TIMEOUT_MS: u64 = 5000;

/// Pre-allocated dual CUDA streams and chunk size for pipelined SSD→GPU transfers.
///
/// Constructed once at dispatcher init and reused across all cold-path reads.
pub struct PipelineRing {
    pub streams: [GpuStream; 2],
    pub chunk_size: usize,
}

impl PipelineRing {
    /// Allocate CUDA streams for pipeline use.
    pub fn new(gpu: &dyn IGpuServices, chunk_size: usize) -> Result<Self, DispatcherError> {
        let stream_a = gpu
            .create_stream()
            .map_err(|e| DispatcherError::IoError(format!("create_stream failed: {e}")))?;
        let stream_b = gpu.create_stream().map_err(|e| {
            let _ = gpu.destroy_stream(stream_a);
            DispatcherError::IoError(format!("create_stream failed: {e}"))
        })?;

        Ok(Self {
            streams: [stream_a, stream_b],
            chunk_size,
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

    // Process completions: on each ReadDone, issue the GPU DMA for that
    // segment immediately, then submit the next read if any remain.
    while completed < num_chunks {
        match channels.completion_rx.recv() {
            Ok(Completion::ReadDone {
                handle,
                tag,
                result,
            }) => {
                result.map_err(|e| {
                    DispatcherError::IoError(format!("SSD read (handle {:?}): {e}", handle))
                })?;

                let seg_idx = tag as usize;
                let seg = &segments[seg_idx];
                let copy_len = seg
                    .length
                    .min(total_bytes.saturating_sub(seg.buffer_offset));
                let current_stream = streams[completed % 2];

                let guard = chunk_bufs[seg_idx].lock().unwrap();
                gpu.dma_copy_to_device_async(
                    &guard,
                    unsafe { (gpu_dst as *mut u8).add(seg.buffer_offset) as *mut std::ffi::c_void },
                    copy_len,
                    current_stream,
                )
                .map_err(|e| {
                    DispatcherError::IoError(format!(
                        "GPU async DMA copy (seg {seg_idx}) failed: {e}"
                    ))
                })?;
                drop(guard);

                completed += 1;

                // Sync the "older" stream periodically to bound GPU queue depth.
                if completed % PIPELINE_RING_SIZE == 0 {
                    gpu.stream_synchronize(streams[0]).map_err(|e| {
                        DispatcherError::IoError(format!("stream_synchronize failed: {e}"))
                    })?;
                    gpu.stream_synchronize(streams[1]).map_err(|e| {
                        DispatcherError::IoError(format!("stream_synchronize failed: {e}"))
                    })?;
                }

                // Submit next read to keep the pipeline full.
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

    // Sync both streams to ensure all GPU copies are complete.
    for s in streams {
        gpu.stream_synchronize(*s)
            .map_err(|e| DispatcherError::IoError(format!("final stream_synchronize: {e}")))?;
    }

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

    // Main pipeline loop: for each NVMe completion —
    //   1. Decode tag → (object_idx, segment_idx)
    //   2. Issue async H2D GPU DMA from memory-tier slot to client GPU
    //   3. Sync both CUDA streams every PIPELINE_RING_SIZE completions
    //   4. Submit the next NVMe read to keep the queue saturated
    while completed < work.len() {
        let t0 = std::time::Instant::now();
        match channels.completion_rx.recv() {
            Ok(Completion::ReadDone { tag, result, .. }) => {
                t_recv_ns += t0.elapsed().as_nanos() as u64;

                let obj_idx = (tag as usize) / max_segments_per_obj;
                let seg_idx = (tag as usize) % max_segments_per_obj;

                if let Err(e) = result {
                    results[obj_idx] = Err(DispatcherError::IoError(format!(
                        "SSD read obj={obj_idx} seg={seg_idx}: {e}"
                    )));
                    completed += 1;
                } else {
                    completed += 1;

                    let tg = std::time::Instant::now();
                    let job = &jobs[obj_idx];
                    let obj = &all_objs[obj_idx];
                    let seg = &obj.segments[seg_idx];
                    let copy_len = seg
                        .length
                        .min(job.total_bytes.saturating_sub(seg.buffer_offset));
                    let current_stream = streams[stream_idx % 2];

                    let guard = obj.chunk_bufs[seg_idx].lock().unwrap();
                    let dma_result = gpu.dma_copy_to_device_async(
                        &guard,
                        unsafe {
                            (job.gpu_dst as *mut u8).add(seg.buffer_offset) as *mut std::ffi::c_void
                        },
                        copy_len,
                        current_stream,
                    );
                    drop(guard);

                    if let Err(e) = dma_result {
                        results[obj_idx] = Err(DispatcherError::IoError(format!(
                            "GPU DMA obj={obj_idx} seg={seg_idx}: {e}"
                        )));
                    }
                    t_gpu_ns += tg.elapsed().as_nanos() as u64;

                    stream_idx += 1;

                    if stream_idx % PIPELINE_RING_SIZE == 0 {
                        let ts = std::time::Instant::now();
                        let _ = gpu.stream_synchronize(streams[0]);
                        let _ = gpu.stream_synchronize(streams[1]);
                        t_sync_ns += ts.elapsed().as_nanos() as u64;
                    }
                }

                if submitted < work.len() {
                    let tr = std::time::Instant::now();
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
                    _t_resub_ns += tr.elapsed().as_nanos() as u64;
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
        assert!(size <= 16);
    }
}
