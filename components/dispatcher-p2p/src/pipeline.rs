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

impl std::fmt::Debug for PipelineRing {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PipelineRing")
            .field("chunk_size", &self.chunk_size)
            .finish_non_exhaustive()
    }
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

impl std::fmt::Debug for ColdReadJob {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ColdReadJob")
            .field("start_lba", &self.start_lba)
            .field("total_bytes", &self.total_bytes)
            .finish_non_exhaustive()
    }
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

/// Pipelined SSD→GPU P2P transfer of one object, using the pre-allocated GPU BAR1
/// staging ring: NVMe reads land in ring slots (GPU-resident BAR1 buffers), then
/// device-to-device copies move each slot's bytes to the client's GPU regions.
/// This eliminates the host DRAM bounce present in the zero-copy path.
///
/// A one-job [`pipelined_multi_object_p2p`]; see there for the ring discipline.
///
/// # Safety
///
/// - Every region must be a valid GPU destination for its `size` bytes, and the
///   sizes must sum to `total_bytes`.
/// - The P2P ring slots must be valid GPU BAR1-mapped, SPDK-registered buffers.
#[allow(clippy::too_many_arguments)]
pub unsafe fn pipelined_ssd_to_gpu_p2p(
    drive: &dyn IBlockDevice,
    _gpu: &dyn IGpuServices,
    ring: &crate::p2p_ring::P2pRing,
    partition: &crate::p2p_ring::ThreadPartition,
    channels: &ClientChannels,
    regions: &[P2pRegion],
    start_lba: u64,
    total_bytes: usize,
) -> Result<(), DispatcherError> {
    let job = P2pColdJob {
        regions: regions.to_vec(),
        start_lba,
        total_bytes,
    };
    // SAFETY: forwarded from this function's contract.
    unsafe { pipelined_multi_object_p2p(drive, ring, partition, channels, &[job]) }
        .pop()
        .unwrap_or(Ok(()))
}

/// One GPU destination of a P2P cold read: `size` bytes at `address`.
///
/// A block is one or more regions laid out back to back in the stored object:
/// region `L` holds object bytes `sum(sizes[0..L]) ..` (vLLM 0.23+ hands a block as
/// one region per layer under LBHNC; one region under BLHNC or vLLM 0.20/0.22).
#[derive(Clone, Copy, Debug)]
pub struct P2pRegion {
    pub address: *mut std::ffi::c_void,
    pub size: usize,
}

// SAFETY: `address` is a CUDA device pointer, valid from any host thread.
unsafe impl Send for P2pRegion {}

/// Describes a single object to be read from SSD via P2P into its client GPU regions.
pub struct P2pColdJob {
    /// The block's GPU regions, in object order; their sizes sum to `total_bytes`.
    pub regions: Vec<P2pRegion>,
    pub start_lba: u64,
    pub total_bytes: usize,
}

/// Queue the device-to-device copies of one ring slot's bytes to the regions they
/// cover: object bytes `[seg_offset, seg_offset + len)` from `src`, each part to its
/// region at the matching offset. A segment can straddle regions (a 128 KiB read
/// over 64 KiB per-layer regions covers two), so it may issue several copies.
///
/// # Safety
/// `src` must be valid for `len` bytes and every region for its `size` bytes, until
/// `stream` syncs.
/// The copies one segment needs: `(region index, offset in the region, offset in the
/// segment, length)` for every region the object bytes `[seg_offset, seg_offset +
/// len)` overlap, in order. Pure arithmetic, so it is unit-tested apart from CUDA.
pub(crate) fn segment_copies(
    region_sizes: impl IntoIterator<Item = usize>,
    seg_offset: usize,
    len: usize,
) -> Vec<(usize, usize, usize, usize)> {
    let seg_end = seg_offset + len;
    let mut out = Vec::new();
    let mut region_start = 0usize;
    for (i, size) in region_sizes.into_iter().enumerate() {
        let region_end = region_start + size;
        let lo = seg_offset.max(region_start);
        let hi = seg_end.min(region_end);
        if lo < hi {
            out.push((i, lo - region_start, lo - seg_offset, hi - lo));
        }
        if region_end >= seg_end {
            break;
        }
        region_start = region_end;
    }
    out
}

/// Queue the device-to-device copies of one ring slot's bytes to the regions they
/// cover: object bytes `[seg_offset, seg_offset + len)` from `src`, each part to its
/// region at the matching offset ([`segment_copies`]). A segment can straddle
/// regions (a 128 KiB read over 64 KiB per-layer regions covers two), so it may
/// issue several copies.
///
/// # Safety
/// `src` must be valid for `len` bytes and every region for its `size` bytes, until
/// `stream` syncs.
pub(crate) unsafe fn copy_segment_to_regions(
    regions: &[P2pRegion],
    seg_offset: usize,
    src: *const u8,
    len: usize,
    stream: *mut std::ffi::c_void,
) -> Result<(), String> {
    use gpu_services::cuda_ffi;
    for (i, region_off, seg_off, n) in
        segment_copies(regions.iter().map(|r| r.size), seg_offset, len)
    {
        // SAFETY: `segment_copies` keeps [region_off, region_off + n) inside region i and
        // [seg_off, seg_off + n) inside the segment's `len` bytes from `src`.
        let err = unsafe {
            cuda_ffi::cudaMemcpyAsync(
                (regions[i].address as *mut u8).add(region_off) as *mut std::ffi::c_void,
                src.add(seg_off) as *const std::ffi::c_void,
                n,
                cuda_ffi::CUDA_MEMCPY_DEVICE_TO_DEVICE,
                stream,
            )
        };
        if err != cuda_ffi::CUDA_SUCCESS {
            return Err(cuda_ffi::cuda_error_string(err));
        }
    }
    Ok(())
}

impl std::fmt::Debug for P2pColdJob {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("P2pColdJob").finish_non_exhaustive()
    }
}

// SAFETY: the regions are CUDA device pointers, valid from any host thread.
unsafe impl Send for P2pColdJob {}

/// Which ring slot each in-flight read owns, and which slots may take a new read.
///
/// NVMe completes reads in any order, so a completion names its read by tag (its
/// work index), and the read's slot is looked up here rather than derived from how
/// many reads have completed. A slot whose D2D copy is queued is not free until the
/// copy's stream has synced ([`SlotTracker::release_copied`]); only then may a new
/// read overwrite it. Pure bookkeeping, so it is unit-tested apart from CUDA.
pub(crate) struct SlotTracker {
    free: Vec<usize>,
    slot_of: Vec<Option<usize>>,
    copied: Vec<usize>,
}

impl SlotTracker {
    /// Slots `first..first + count`, for `work_items` reads.
    pub(crate) fn new(first: usize, count: usize, work_items: usize) -> Self {
        Self {
            free: (first..first + count).rev().collect(),
            slot_of: vec![None; work_items],
            copied: Vec::with_capacity(count),
        }
    }

    /// A free slot for read `work`, or `None` when every slot is in flight or awaiting
    /// its copy's sync.
    pub(crate) fn acquire(&mut self, work: usize) -> Option<usize> {
        let slot = self.free.pop()?;
        self.slot_of[work] = Some(slot);
        Some(slot)
    }

    /// Read `work` completed: its slot, now holding that read's bytes. `None` for a
    /// tag that is out of range or not in flight (a duplicate or foreign completion).
    pub(crate) fn complete(&mut self, work: usize) -> Option<usize> {
        let slot = self.slot_of.get_mut(work)?.take()?;
        self.copied.push(slot);
        Some(slot)
    }

    /// Slots whose copies are queued but not yet synced.
    pub(crate) fn pending_copies(&self) -> usize {
        self.copied.len()
    }

    /// The copy streams have synced: every copied slot is free again.
    pub(crate) fn release_copied(&mut self) {
        self.free.append(&mut self.copied);
    }

    /// Slots ready for a new read.
    pub(crate) fn free_slots(&self) -> usize {
        self.free.len()
    }
}

/// Multi-object P2P pipeline: interleaves segments from N objects across the BAR1
/// ring for maximum NVMe queue utilization. Each completion is tag-decoded to route
/// the D2D copy to the correct client GPU destination.
///
/// # Ring discipline
///
/// Each read is tagged with its work index, and a [`SlotTracker`] records the slot it
/// was issued into. NVMe completes reads out of order, so the slot a completion's
/// bytes are in comes from the tracker, not from a completion count. A slot is
/// reused only after the stream holding its D2D copy has synced. The copy streams
/// are synced when half the partition's slots await a sync, or when no slot is free
/// and reads remain, so the other half keeps the NVMe queue fed in the meantime.
///
/// # Safety
///
/// - Every job's regions must be valid GPU destinations for their sizes, summing to
///   the job's `total_bytes`.
/// - The P2P ring slots must be valid GPU BAR1-mapped, SPDK-registered buffers.
#[allow(clippy::too_many_arguments)]
pub unsafe fn pipelined_multi_object_p2p(
    drive: &dyn IBlockDevice,
    ring: &crate::p2p_ring::P2pRing,
    partition: &crate::p2p_ring::ThreadPartition,
    channels: &ClientChannels,
    jobs: &[P2pColdJob],
) -> Vec<Result<(), DispatcherError>> {
    use gpu_services::cuda_ffi;

    let num_jobs = jobs.len();
    let mut results: Vec<Result<(), DispatcherError>> = vec![Ok(()); num_jobs];
    if num_jobs == 0 {
        return results;
    }

    let block_size = drive.block_size() as usize;
    let all_streams = ring.streams();
    let num_streams = all_streams.len().max(1);

    // Every (object, segment) read, in submission order; a read's tag is its index.
    let mut work: Vec<(usize, crate::io_segmenter::IoSegment)> = Vec::new();
    for (obj_idx, job) in jobs.iter().enumerate() {
        let aligned_bytes = job.total_bytes.next_multiple_of(block_size);
        for seg in crate::io_segmenter::segment_io(
            job.start_lba,
            aligned_bytes,
            ring.slot_size as u32,
            block_size as u32,
        ) {
            work.push((obj_idx, seg));
        }
    }
    if work.is_empty() {
        return results;
    }

    let ring_size = partition.effective_qd.max(1);
    let mut slots = SlotTracker::new(partition.ring_offset, ring_size, work.len());
    // Sync once half the slots await it, so the other half stays in flight meanwhile.
    let sync_batch = ring_size.div_ceil(2);
    let mut submitted = 0usize;
    let mut in_flight = 0usize;
    let mut copies = 0usize;

    // Fail every job still `Ok`.
    let fail_all = |results: &mut Vec<Result<(), DispatcherError>>, msg: String| {
        for r in results.iter_mut().filter(|r| r.is_ok()) {
            *r = Err(DispatcherError::IoError(msg.clone()));
        }
    };
    let sync_streams = || -> Result<(), String> {
        for s in all_streams {
            // SAFETY: the ring's streams live as long as the ring.
            let err = unsafe { cuda_ffi::cudaStreamSynchronize(s.0) };
            if err != cuda_ffi::CUDA_SUCCESS {
                return Err(cuda_ffi::cuda_error_string(err));
            }
        }
        Ok(())
    };

    loop {
        // Issue reads into free slots.
        while submitted < work.len() {
            let Some(slot) = slots.acquire(submitted) else {
                break;
            };
            if channels
                .command_tx
                .send(Command::ReadAsync {
                    ns_id: 1,
                    lba: work[submitted].1.lba,
                    buf: Arc::clone(ring.slot(slot)),
                    timeout_ms: READ_TIMEOUT_MS,
                    tag: submitted as u64,
                })
                .is_err()
            {
                fail_all(&mut results, "P2P ReadAsync send failed".into());
                // Reads already issued still land in their slots; let them finish
                // before the ring is reused.
                drain_completions(channels, in_flight);
                let _ = sync_streams();
                return results;
            }
            submitted += 1;
            in_flight += 1;
        }
        if in_flight == 0 {
            break;
        }

        match channels.completion_rx.recv() {
            Ok(Completion::ReadDone { tag, result, .. }) => {
                let work_idx = tag as usize;
                let Some(slot) = slots.complete(work_idx) else {
                    fail_all(
                        &mut results,
                        format!("P2P completion with unknown tag {tag}"),
                    );
                    drain_completions(channels, in_flight);
                    let _ = sync_streams();
                    return results;
                };
                in_flight -= 1;
                let (obj_idx, seg) = &work[work_idx];
                let job = &jobs[*obj_idx];
                match result {
                    Err(e) => {
                        if results[*obj_idx].is_ok() {
                            results[*obj_idx] = Err(DispatcherError::IoError(format!(
                                "P2P SSD read obj={obj_idx} lba={}: {e}",
                                seg.lba
                            )));
                        }
                    }
                    Ok(()) if results[*obj_idx].is_ok() => {
                        let copy_len = seg
                            .length
                            .min(job.total_bytes.saturating_sub(seg.buffer_offset));
                        let stream = all_streams[copies % num_streams];
                        copies += 1;
                        // D2D async copies: BAR1 ring slot → the client GPU region(s)
                        // this segment covers.
                        // SAFETY: the slot holds this read's bytes until the slot is
                        // released, which waits for this stream to sync; the regions
                        // are valid per this function's contract.
                        if let Err(e) = unsafe {
                            copy_segment_to_regions(
                                &job.regions,
                                seg.buffer_offset,
                                ring.dev_ptrs[slot] as *const u8,
                                copy_len,
                                stream.0,
                            )
                        } {
                            results[*obj_idx] = Err(DispatcherError::IoError(format!(
                                "P2P D2D cudaMemcpyAsync obj={obj_idx}: {e}"
                            )));
                        }
                    }
                    // The object already failed; its remaining bytes are not copied.
                    Ok(()) => {}
                }
            }
            Ok(Completion::Timeout { handle }) => {
                // The timed-out read may still land in its slot later, so the ring
                // is not reused by this call; fail what is left.
                fail_all(
                    &mut results,
                    format!("P2P NVMe timeout (handle {handle:?})"),
                );
                in_flight -= 1;
                drain_completions(channels, in_flight);
                let _ = sync_streams();
                return results;
            }
            Ok(other) => {
                fail_all(
                    &mut results,
                    format!("P2P unexpected completion: {other:?}"),
                );
                drain_completions(channels, in_flight);
                let _ = sync_streams();
                return results;
            }
            Err(_) => {
                fail_all(&mut results, "P2P completion channel disconnected".into());
                let _ = sync_streams();
                return results;
            }
        }

        // Recycle copied slots once enough await a sync, or when reads are blocked on
        // a free slot.
        let starved = submitted < work.len() && slots.free_slots() == 0;
        if slots.pending_copies() >= sync_batch || (starved && slots.pending_copies() > 0) {
            if let Err(e) = sync_streams() {
                fail_all(&mut results, format!("P2P cudaStreamSynchronize: {e}"));
                drain_completions(channels, in_flight);
                return results;
            }
            slots.release_copied();
        }
    }

    // Final sync: every queued D2D copy lands before the caller sees success.
    if let Err(e) = sync_streams() {
        fail_all(
            &mut results,
            format!("P2P final cudaStreamSynchronize: {e}"),
        );
    }
    results
}

/// Wait out `n` reads still in flight after an error, so none lands in a ring slot
/// after this call has handed the ring back. Gives up on a disconnected channel, or after twice the read timeout.
fn drain_completions(channels: &ClientChannels, n: usize) {
    let deadline =
        std::time::Instant::now() + std::time::Duration::from_millis(2 * READ_TIMEOUT_MS);
    let mut left = n;
    while left > 0 && std::time::Instant::now() < deadline {
        match channels.completion_rx.try_recv() {
            Ok(_) => left -= 1,
            Err(component_core::channel::ChannelError::Empty) => std::thread::yield_now(),
            Err(_) => break,
        }
    }
}

/// Describes a single object to be promoted from SSD into the memory-tier (no GPU).
pub struct DramPromoteJob {
    pub mem_ptr: *mut u8,
    pub start_lba: u64,
    pub total_bytes: usize,
}

impl std::fmt::Debug for DramPromoteJob {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("DramPromoteJob")
            .field("start_lba", &self.start_lba)
            .field("total_bytes", &self.total_bytes)
            .finish_non_exhaustive()
    }
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

#[cfg(test)]
mod segment_copy_tests {
    use super::segment_copies;

    const KIB: usize = 1024;

    #[test]
    fn one_region_is_one_copy_at_the_segment_offset() {
        // N == 1: the old behaviour, one copy to gpu_dst + seg_offset.
        assert_eq!(
            segment_copies([2048 * KIB], 256 * KIB, 128 * KIB),
            vec![(0, 256 * KIB, 0, 128 * KIB)]
        );
    }

    #[test]
    fn a_128k_segment_over_64k_regions_covers_two() {
        // Llama-3-8B LBHNC: 32 x 64 KiB; the third 128 KiB segment is regions 4 and 5.
        let regions = [64 * KIB; 32];
        assert_eq!(
            segment_copies(regions, 256 * KIB, 128 * KIB),
            vec![(4, 0, 0, 64 * KIB), (5, 0, 64 * KIB, 64 * KIB)]
        );
    }

    #[test]
    fn every_byte_of_a_block_lands_exactly_once_in_order() {
        // 32 x 64 KiB regions read as 128 KiB segments, plus an unaligned segment size:
        // the copies, laid end to end, must tile the block with no gap or overlap.
        for seg in [128 * KIB, 100 * KIB, 4 * KIB, 2048 * KIB] {
            let regions = [64 * KIB; 32];
            let total = 32 * 64 * KIB;
            let mut covered = vec![0u8; total];
            let mut off = 0;
            while off < total {
                let len = seg.min(total - off);
                let mut seg_pos = 0;
                for (i, region_off, seg_off, n) in segment_copies(regions, off, len) {
                    assert_eq!(seg_off, seg_pos, "copies are contiguous within a segment");
                    seg_pos += n;
                    for b in &mut covered[i * 64 * KIB + region_off..][..n] {
                        *b += 1;
                    }
                }
                assert_eq!(seg_pos, len, "a segment's copies cover all of it");
                off += len;
            }
            assert!(
                covered.iter().all(|&c| c == 1),
                "seg {seg}: each byte exactly once"
            );
        }
    }

    #[test]
    fn uneven_regions_and_a_zero_size_region() {
        // Regions of different sizes, one empty: the empty one gets no copy.
        let regions = [3 * KIB, 0, 5 * KIB, 8 * KIB];
        assert_eq!(
            segment_copies(regions, 2 * KIB, 4 * KIB),
            vec![(0, 2 * KIB, 0, KIB), (2, 0, KIB, 3 * KIB)]
        );
    }

    #[test]
    fn a_segment_past_the_regions_copies_only_what_fits() {
        // A read rounded up to the device block size is longer than the object.
        assert_eq!(
            segment_copies([4 * KIB], 0, 8 * KIB),
            vec![(0, 0, 0, 4 * KIB)]
        );
    }
}

#[cfg(test)]
mod slot_tracker_tests {
    use super::SlotTracker;

    /// Drive the tracker like the pipeline does, completing in-flight reads in the
    /// order `pick` chooses, and check each completion gets the slot its read used,
    /// and no slot is read into while a read or an unsynced copy holds it.
    fn simulate(ring: usize, work: usize, pick: impl Fn(usize, usize) -> usize) {
        let first = 7;
        let mut t = SlotTracker::new(first, ring, work);
        let sync_batch = ring.div_ceil(2);
        let mut issued_into = vec![usize::MAX; work];
        let mut in_flight: Vec<usize> = Vec::new();
        let mut busy = std::collections::HashSet::new(); // read or unsynced copy
        let mut copied: Vec<usize> = Vec::new();
        let (mut submitted, mut done, mut step) = (0, 0, 0);
        while done < work {
            while submitted < work {
                let Some(slot) = t.acquire(submitted) else {
                    break;
                };
                assert!((first..first + ring).contains(&slot));
                assert!(busy.insert(slot), "slot {slot} reused while held");
                issued_into[submitted] = slot;
                in_flight.push(submitted);
                submitted += 1;
            }
            let w = in_flight.remove(pick(step, in_flight.len()) % in_flight.len());
            step += 1;
            assert_eq!(
                t.complete(w),
                Some(issued_into[w]),
                "read {w} copied from wrong slot"
            );
            assert_eq!(t.complete(w), None, "duplicate completion accepted");
            copied.push(issued_into[w]);
            done += 1;
            let starved = submitted < work && t.free_slots() == 0;
            if t.pending_copies() >= sync_batch || (starved && t.pending_copies() > 0) {
                t.release_copied();
                for s in copied.drain(..) {
                    busy.remove(&s);
                }
            }
        }
        assert!(in_flight.is_empty());
    }

    #[test]
    fn in_order_completions() {
        simulate(16, 100, |_, _| 0);
    }

    #[test]
    fn newest_first_completions() {
        simulate(16, 100, |_, n| n - 1);
    }

    #[test]
    fn scrambled_completions() {
        for ring in [1, 2, 3, 16] {
            for work in [1, 5, 16, 17, 257] {
                simulate(ring, work, |step, n| (step * 7919 + 13) % n);
            }
        }
    }

    #[test]
    fn unknown_tag_is_rejected() {
        let mut t = SlotTracker::new(0, 4, 3);
        assert_eq!(t.complete(5), None);
        assert_eq!(t.complete(0), None, "never issued");
    }

    #[test]
    fn slots_return_only_after_release() {
        let mut t = SlotTracker::new(0, 2, 4);
        let a = t.acquire(0).unwrap();
        let b = t.acquire(1).unwrap();
        assert_eq!(t.acquire(2), None);
        assert_eq!(t.complete(1), Some(b));
        assert_eq!(t.acquire(2), None, "copied slot reused before its sync");
        t.release_copied();
        assert_eq!(t.acquire(2), Some(b));
        assert_eq!(t.complete(0), Some(a));
    }
}
