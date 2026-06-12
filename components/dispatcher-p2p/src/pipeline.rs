//! Ring-buffer pipelined reader for SSD→DRAM→GPU transfers.
//!
//! Uses async NVMe reads with multiple in-flight commands and dual CUDA
//! streams to overlap SSD I/O with GPU DMA copies. Each completed chunk
//! is memcpy'd to the memory-tier slot (CPU) and simultaneously queued
//! for async H2D transfer to the GPU destination.

use std::sync::{Arc, Mutex};

use interfaces::{
    ClientChannels, Command, Completion, DispatcherError, DmaBuffer, GpuStream, IBlockDevice,
    IGpuServices,
};

use crate::io_segmenter;

/// Number of ring buffers for pipelined transfers.
pub const PIPELINE_RING_SIZE: usize = 16;

/// Timeout for async NVMe read operations (ms).
const READ_TIMEOUT_MS: u64 = 5000;

/// Pre-allocated ring of CUDA-pinned + SPDK-registered DMA buffers and CUDA streams.
///
/// Constructed once and reused across multiple `pipelined_ssd_to_gpu` calls
/// to avoid per-call `cudaHostAlloc`/`spdk_mem_register` overhead.
pub struct PipelineRing {
    pub buffers: Vec<Arc<Mutex<DmaBuffer>>>,
    pub streams: [GpuStream; 2],
    pub chunk_size: usize,
}

impl PipelineRing {
    /// Allocate a new pipeline ring with CUDA-pinned, SPDK-registered buffers.
    pub fn new(gpu: &dyn IGpuServices, chunk_size: usize) -> Result<Self, DispatcherError> {
        let buffers: Vec<Arc<Mutex<DmaBuffer>>> = (0..PIPELINE_RING_SIZE)
            .map(|_| {
                gpu.allocate_pinned_dma_buffer(chunk_size)
                    .map(|b| Arc::new(Mutex::new(b)))
                    .map_err(|e| {
                        DispatcherError::AllocationFailed(format!("pipeline ring buffer: {e}"))
                    })
            })
            .collect::<Result<Vec<_>, _>>()?;

        let stream_a = gpu
            .create_stream()
            .map_err(|e| DispatcherError::IoError(format!("create_stream failed: {e}")))?;
        let stream_b = gpu.create_stream().map_err(|e| {
            let _ = gpu.destroy_stream(stream_a);
            DispatcherError::IoError(format!("create_stream failed: {e}"))
        })?;

        Ok(Self {
            buffers,
            streams: [stream_a, stream_b],
            chunk_size,
        })
    }

    /// Destroy CUDA streams. Buffers are freed on drop via their DmaBuffer free_fn.
    pub fn destroy(self, gpu: &dyn IGpuServices) {
        let _ = gpu.destroy_stream(self.streams[0]);
        let _ = gpu.destroy_stream(self.streams[1]);
    }
}

/// Pipeline-read from SSD into a memory-tier slot while streaming chunks to GPU.
///
/// Uses a pre-allocated [`PipelineRing`] to avoid per-call allocation overhead.
///
/// Algorithm (mirrors gpu-bb-vs-p2p benchmark):
/// 1. Prime the ring with `min(ring_size, num_chunks)` async NVMe reads
/// 2. For each completion:
///    a. memcpy ring buffer → memory-tier slot (CPU, immediate)
///    b. Issue cudaMemcpyAsync on stream[completed % 2]
///    c. Sync stream[(completed+1) % 2] (previous copy) — frees that ring slot
///    d. Resubmit next NVMe read into the freed slot
/// 3. Sync both streams
///
/// # Safety
///
/// - `mem_tier_ptr` must be valid for writes of at least `total_bytes` (aligned up to block size).
/// - `gpu_dst` must be a valid GPU destination pointer for `total_bytes`.
pub unsafe fn pipelined_ssd_to_gpu(
    drive: &dyn IBlockDevice,
    gpu: &dyn IGpuServices,
    ring: &PipelineRing,
    mem_tier_ptr: *mut u8,
    gpu_dst: *mut std::ffi::c_void,
    start_lba: u64,
    total_bytes: usize,
) -> Result<(), DispatcherError> {
    let block_size = drive.block_size() as usize;
    let chunk_size = ring.chunk_size;
    let aligned_bytes = total_bytes.next_multiple_of(block_size);

    let channels: ClientChannels = drive
        .connect_client()
        .map_err(|e| DispatcherError::IoError(format!("connect_client failed: {e}")))?;

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
    let ring_size = ring.buffers.len().min(num_chunks);
    let streams = &ring.streams;

    // Process chunks in batches of ring_size. Each batch submits reads into
    // distinct ring slots, waits for all to complete (order-independent since
    // each slot is unique within the batch), then copies to memory-tier and GPU.
    let mut chunk_idx = 0;

    while chunk_idx < num_chunks {
        let batch_end = (chunk_idx + ring_size).min(num_chunks);
        let batch_len = batch_end - chunk_idx;

        // Submit this batch's reads into ring slots.
        for i in 0..batch_len {
            channels
                .command_tx
                .send(Command::ReadAsync {
                    ns_id: 1,
                    lba: segments[chunk_idx + i].lba,
                    buf: Arc::clone(&ring.buffers[i]),
                    timeout_ms: READ_TIMEOUT_MS,
                    tag: 0,
                })
                .map_err(|e| {
                    DispatcherError::IoError(format!("ReadAsync send #{}: {e}", chunk_idx + i))
                })?;
        }

        // Wait for all reads in this batch to complete.
        for _i in 0..batch_len {
            match channels.completion_rx.recv() {
                Ok(Completion::ReadDone { handle, result, .. }) => {
                    result.map_err(|e| {
                        DispatcherError::IoError(format!("SSD read (handle {:?}): {e}", handle))
                    })?;
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

        // All reads for this batch are complete. Process in order.
        for i in 0..batch_len {
            let seg = &segments[chunk_idx + i];
            let copy_len = seg
                .length
                .min(total_bytes.saturating_sub(seg.buffer_offset));
            let current_stream = streams[i % 2];

            let guard = ring.buffers[i].lock().unwrap();

            // memcpy ring buffer → memory-tier slot.
            if copy_len > 0 {
                unsafe {
                    std::ptr::copy_nonoverlapping(
                        guard.as_ptr() as *const u8,
                        mem_tier_ptr.add(seg.buffer_offset),
                        copy_len,
                    );
                }
            }

            // Async DMA copy ring buffer → GPU.
            gpu.dma_copy_to_device_async(
                &guard,
                unsafe { (gpu_dst as *mut u8).add(seg.buffer_offset) as *mut std::ffi::c_void },
                copy_len,
                current_stream,
            )
            .map_err(|e| {
                DispatcherError::IoError(format!(
                    "GPU async DMA copy #{} failed: {e}",
                    chunk_idx + i
                ))
            })?;

            drop(guard);
        }

        // Sync both streams before reusing ring slots in the next batch.
        gpu.stream_synchronize(streams[0])
            .map_err(|e| DispatcherError::IoError(format!("stream_synchronize failed: {e}")))?;
        gpu.stream_synchronize(streams[1])
            .map_err(|e| DispatcherError::IoError(format!("stream_synchronize failed: {e}")))?;

        chunk_idx = batch_end;
    }

    // Sync both streams to ensure all GPU copies are complete.
    for s in streams {
        gpu.stream_synchronize(*s)
            .map_err(|e| DispatcherError::IoError(format!("final stream_synchronize: {e}")))?;
    }

    Ok(())
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

    // Build a flat list of (obj_idx, seg_idx) work items.
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

    #[cfg(feature = "pipeline-telemetry")]
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
            for r in results.iter_mut() {
                *r = Err(DispatcherError::IoError("channel send failed".into()));
            }
            return results;
        }
        submitted += 1;
    }
    #[cfg(feature = "pipeline-telemetry")]
    let t_initial_submit = t_submit_start.elapsed();
    #[cfg(feature = "pipeline-telemetry")]
    let mut t_recv_ns: u64 = 0;
    #[cfg(feature = "pipeline-telemetry")]
    let mut t_gpu_ns: u64 = 0;
    #[cfg(feature = "pipeline-telemetry")]
    let mut t_sync_ns: u64 = 0;
    #[cfg(feature = "pipeline-telemetry")]
    let mut t_resub_ns: u64 = 0;

    while completed < work.len() {
        #[cfg(feature = "pipeline-telemetry")]
        let t0 = std::time::Instant::now();
        match channels.completion_rx.recv() {
            Ok(Completion::ReadDone { tag, result, .. }) => {
                #[cfg(feature = "pipeline-telemetry")]
                {
                    t_recv_ns += t0.elapsed().as_nanos() as u64;
                }

                let obj_idx = (tag as usize) / max_segments_per_obj;
                let seg_idx = (tag as usize) % max_segments_per_obj;

                if let Err(e) = result {
                    results[obj_idx] = Err(DispatcherError::IoError(format!(
                        "SSD read obj={obj_idx} seg={seg_idx}: {e}"
                    )));
                    completed += 1;
                } else {
                    completed += 1;

                    #[cfg(feature = "pipeline-telemetry")]
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
                    #[cfg(feature = "pipeline-telemetry")]
                    {
                        t_gpu_ns += tg.elapsed().as_nanos() as u64;
                    }

                    stream_idx += 1;

                    if stream_idx % PIPELINE_RING_SIZE == 0 {
                        #[cfg(feature = "pipeline-telemetry")]
                        let ts = std::time::Instant::now();
                        let _ = gpu.stream_synchronize(streams[0]);
                        let _ = gpu.stream_synchronize(streams[1]);
                        #[cfg(feature = "pipeline-telemetry")]
                        {
                            t_sync_ns += ts.elapsed().as_nanos() as u64;
                        }
                    }
                }

                if submitted < work.len() {
                    #[cfg(feature = "pipeline-telemetry")]
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
                    #[cfg(feature = "pipeline-telemetry")]
                    {
                        t_resub_ns += tr.elapsed().as_nanos() as u64;
                    }
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
    #[cfg(feature = "pipeline-telemetry")]
    let ts_final = std::time::Instant::now();
    let _ = gpu.stream_synchronize(streams[0]);
    let _ = gpu.stream_synchronize(streams[1]);
    #[cfg(feature = "pipeline-telemetry")]
    {
        let t_final_sync_ns = ts_final.elapsed().as_nanos() as u64;
        eprintln!(
            "[pipeline-perf] jobs={} segs={} submit={:.2}ms recv_wait={:.2}ms gpu_dma={:.2}ms sync={:.2}ms resub={:.2}ms final_sync={:.2}ms",
            num_jobs,
            total_segments,
            t_initial_submit.as_secs_f64() * 1000.0,
            t_recv_ns as f64 / 1_000_000.0,
            t_gpu_ns as f64 / 1_000_000.0,
            t_sync_ns as f64 / 1_000_000.0,
            t_resub_ns as f64 / 1_000_000.0,
            t_final_sync_ns as f64 / 1_000_000.0,
        );
    }

    // Forget DmaBuffer wrappers (memory-tier owns the allocation).
    for obj in all_objs {
        for buf in obj.chunk_bufs {
            std::mem::forget(Arc::try_unwrap(buf).ok());
        }
    }

    results
}

/// Pipelined SSD→GPU P2P transfer using pre-allocated GPU BAR1 staging ring.
///
/// Reads NVMe data directly into P2P ring slots (GPU-resident BAR1 buffers),
/// then issues D2D copies from staging to the client's final GPU destination.
/// This eliminates the host DRAM bounce present in the zero-copy path.
///
/// # Algorithm
///
/// 1. Prime: submit up to `effective_qd` async NVMe reads into ring slots
/// 2. On each NVMe completion: issue D2D copy on alternating CUDA stream,
///    submit next read into a recycled slot
/// 3. Sync both streams every `ring_size/2` completions before recycling
/// 4. Finalize: sync both streams after all chunks complete
///
/// # Safety
///
/// - `gpu_dst` must be a valid GPU destination pointer for `total_bytes`.
/// - The P2P ring slots must be valid GPU BAR1-mapped, SPDK-registered buffers.
#[allow(clippy::too_many_arguments)]
pub unsafe fn pipelined_ssd_to_gpu_p2p(
    drive: &dyn IBlockDevice,
    gpu: &dyn IGpuServices,
    ring: &crate::p2p_ring::P2pRing,
    partition: &crate::p2p_ring::ThreadPartition,
    channels: &ClientChannels,
    gpu_dst: *mut std::ffi::c_void,
    start_lba: u64,
    total_bytes: usize,
) -> Result<(), DispatcherError> {
    let block_size = drive.block_size() as usize;
    let chunk_size = ring.slot_size;
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
    let streams = ring.streams();
    let effective_qd = partition.effective_qd;
    let ring_offset = partition.ring_offset;

    // Use the pre-allocated BAR1-mapped ring buffers directly.
    let slot_bufs: Vec<&Arc<Mutex<DmaBuffer>>> = (0..effective_qd)
        .map(|i| ring.slot(ring_offset + i))
        .collect();

    let submit_limit = effective_qd.min(num_chunks);
    let mut submitted = 0usize;
    let mut completed = 0usize;

    // Prime the pipeline.
    while submitted < submit_limit {
        let slot_idx = submitted % effective_qd;
        channels
            .command_tx
            .send(Command::ReadAsync {
                ns_id: 1,
                lba: segments[submitted].lba,
                buf: Arc::clone(slot_bufs[slot_idx]),
                timeout_ms: READ_TIMEOUT_MS,
                tag: submitted as u64,
            })
            .map_err(|e| {
                DispatcherError::IoError(format!("P2P ReadAsync send #{submitted}: {e}"))
            })?;
        submitted += 1;
    }

    // Process completions.
    while completed < num_chunks {
        match channels.completion_rx.recv() {
            Ok(Completion::ReadDone { tag, result, .. }) => {
                result.map_err(|e| {
                    DispatcherError::IoError(format!("P2P SSD read (seg {}): {e}", tag))
                })?;

                let seg_idx = tag as usize;
                let seg = &segments[seg_idx];
                let copy_len = seg
                    .length
                    .min(total_bytes.saturating_sub(seg.buffer_offset));
                let current_stream = streams[completed % 2];
                let slot_idx = seg_idx % effective_qd;

                // D2D copy: GPU ring slot → client GPU destination.
                let src_dev_ptr = unsafe {
                    (ring.dev_ptrs[ring_offset + slot_idx] as *const u8) as *const std::ffi::c_void
                };
                let dst_ptr = unsafe {
                    (gpu_dst as *mut u8).add(seg.buffer_offset) as *mut std::ffi::c_void
                };
                let err = unsafe {
                    gpu_services::cuda_ffi::cudaMemcpyAsync(
                        dst_ptr,
                        src_dev_ptr,
                        copy_len,
                        gpu_services::cuda_ffi::CUDA_MEMCPY_DEVICE_TO_DEVICE,
                        current_stream.0,
                    )
                };
                if err != gpu_services::cuda_ffi::CUDA_SUCCESS {
                    return Err(DispatcherError::IoError(format!(
                        "P2P D2D cudaMemcpyAsync (seg {seg_idx}) failed: {}",
                        gpu_services::cuda_ffi::cuda_error_string(err)
                    )));
                }

                completed += 1;

                // Sync both streams periodically to ensure slots are safe to recycle.
                if completed % (effective_qd / 2).max(1) == 0 {
                    gpu.stream_synchronize(streams[0]).map_err(|e| {
                        DispatcherError::IoError(format!("P2P stream_sync[0]: {e}"))
                    })?;
                    gpu.stream_synchronize(streams[1]).map_err(|e| {
                        DispatcherError::IoError(format!("P2P stream_sync[1]: {e}"))
                    })?;
                }

                // Submit next read into recycled slot.
                if submitted < num_chunks {
                    let next_slot = submitted % effective_qd;
                    channels
                        .command_tx
                        .send(Command::ReadAsync {
                            ns_id: 1,
                            lba: segments[submitted].lba,
                            buf: Arc::clone(slot_bufs[next_slot]),
                            timeout_ms: READ_TIMEOUT_MS,
                            tag: submitted as u64,
                        })
                        .map_err(|e| {
                            DispatcherError::IoError(format!(
                                "P2P ReadAsync resend #{submitted}: {e}"
                            ))
                        })?;
                    submitted += 1;
                }
            }
            Ok(Completion::Timeout { handle }) => {
                return Err(DispatcherError::IoError(format!(
                    "P2P NVMe read timeout (handle {:?})",
                    handle
                )));
            }
            Ok(other) => {
                return Err(DispatcherError::IoError(format!(
                    "P2P unexpected completion: {other:?}"
                )));
            }
            Err(_) => {
                return Err(DispatcherError::IoError(
                    "P2P completion channel disconnected".into(),
                ));
            }
        }
    }

    // Final sync both streams.
    for s in streams {
        gpu.stream_synchronize(*s)
            .map_err(|e| DispatcherError::IoError(format!("P2P final sync: {e}")))?;
    }

    Ok(())
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
