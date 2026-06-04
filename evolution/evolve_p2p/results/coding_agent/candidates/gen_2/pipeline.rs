//! Ring-buffer pipelined reader for SSD→DRAM→GPU transfers.
//!
//! Uses async NVMe reads with multiple in-flight commands and dual CUDA
//! streams to overlap SSD I/O with GPU DMA copies. Each completed chunk
//! is memcpy'd to the memory-tier slot (CPU) and simultaneously queued
//! for async H2D transfer to the GPU destination.

use std::sync::{Arc, Mutex};

use interfaces::{
    ClientChannels, Command, Completion, DmaBuffer, DispatcherError, GpuStream, IBlockDevice,
    IGpuServices,
};

use crate::io_segmenter;

/// Number of ring buffers for pipelined transfers.
pub const PIPELINE_RING_SIZE: usize = 8;

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
                })
                .map_err(|e| {
                    DispatcherError::IoError(format!("ReadAsync send #{}: {e}", chunk_idx + i))
                })?;
        }

        // Wait for all reads in this batch to complete.
        for _i in 0..batch_len {
            match channels.completion_rx.recv() {
                Ok(Completion::ReadDone { handle, result }) => {
                    result.map_err(|e| {
                        DispatcherError::IoError(format!(
                            "SSD read (handle {:?}): {e}",
                            handle
                        ))
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
            let copy_len = seg.length.min(total_bytes.saturating_sub(seg.buffer_offset));
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
            let buf = unsafe { DmaBuffer::from_raw(ptr, buf_size, noop_free, -1) }
                .map_err(|e| {
                    DispatcherError::AllocationFailed(format!("DmaBuffer wrap chunk: {e}"))
                })?;
            Ok(Arc::new(Mutex::new(buf)))
        })
        .collect::<Result<Vec<_>, DispatcherError>>()?;

    let max_inflight = max_queue_depth.min(num_chunks);

    // Track in-flight segment indices in submission order (FIFO queue).
    // Each NVMe completion is matched to the oldest outstanding segment,
    // which is valid because a single NVMe queue pair completes in FIFO order.
    let mut inflight: std::collections::VecDeque<usize> =
        std::collections::VecDeque::with_capacity(max_inflight);

    // Prime the sliding window.
    for i in 0..max_inflight {
        channels
            .command_tx
            .send(Command::ReadAsync {
                ns_id: 1,
                lba: segments[i].lba,
                buf: Arc::clone(&chunk_bufs[i]),
                timeout_ms: READ_TIMEOUT_MS,
            })
            .map_err(|e| DispatcherError::IoError(format!("ReadAsync send #{i}: {e}")))?;
        inflight.push_back(i);
    }

    let mut next_to_submit = max_inflight;
    let mut stream_idx = 0usize;

    // Track consecutive completed segments for coalesced GPU copies.
    // Instead of issuing one GPU copy per 128KB chunk, we accumulate consecutive
    // completions and issue larger copies to reduce CUDA API call overhead.
    let mut completed_bitmap = vec![false; num_chunks];
    let mut gpu_copy_frontier = 0usize;

    // Sliding-window pipeline: as each NVMe read completes, immediately submit
    // the next read and coalesce GPU copies for consecutive completed chunks.
    for _completed in 0..num_chunks {
        // Wait for the oldest in-flight read to finish.
        let seg_idx = match channels.completion_rx.recv() {
            Ok(Completion::ReadDone { handle, result }) => {
                result.map_err(|e| {
                    DispatcherError::IoError(format!("SSD read (handle {:?}): {e}", handle))
                })?;
                inflight.pop_front().unwrap()
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
        };

        // Submit the next read immediately so SSD I/O overlaps with the GPU copy.
        if next_to_submit < num_chunks {
            channels
                .command_tx
                .send(Command::ReadAsync {
                    ns_id: 1,
                    lba: segments[next_to_submit].lba,
                    buf: Arc::clone(&chunk_bufs[next_to_submit]),
                    timeout_ms: READ_TIMEOUT_MS,
                })
                .map_err(|e| {
                    DispatcherError::IoError(format!(
                        "ReadAsync submit #{next_to_submit}: {e}"
                    ))
                })?;
            inflight.push_back(next_to_submit);
            next_to_submit += 1;
        }

        // Mark this segment as done.
        completed_bitmap[seg_idx] = true;

        // Advance the GPU copy frontier: issue one coalesced copy for all
        // consecutive completed segments starting from the frontier.
        if seg_idx == gpu_copy_frontier || completed_bitmap[gpu_copy_frontier] {
            let coalesce_start = gpu_copy_frontier;
            while gpu_copy_frontier < num_chunks && completed_bitmap[gpu_copy_frontier] {
                gpu_copy_frontier += 1;
            }

            // Issue a single GPU copy covering segments [coalesce_start..gpu_copy_frontier).
            let start_offset = segments[coalesce_start].buffer_offset;
            let end_seg = &segments[gpu_copy_frontier - 1];
            let end_offset = end_seg.buffer_offset + end_seg.length;
            let copy_len = end_offset.min(total_bytes) - start_offset.min(total_bytes);

            if copy_len > 0 {
                let current_stream = streams[stream_idx % 2];
                let src_ptr = unsafe { mem_tier_ptr.add(start_offset) as *const std::ffi::c_void };
                let dst_ptr = unsafe { (gpu_dst as *mut u8).add(start_offset) as *mut std::ffi::c_void };
                gpu.memcpy_h2d_async(src_ptr, dst_ptr, copy_len, current_stream)
                    .map_err(|e| {
                        DispatcherError::IoError(format!("GPU async DMA copy failed: {e}"))
                    })?;
                stream_idx += 1;
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

/// Multi-object interleaved pipeline: processes multiple cold entries over a single
/// NVMe queue pair simultaneously, maximizing SSD queue utilization.
///
/// Instead of reading one 4MB object completely before starting the next, this
/// submits reads for all objects in a round-robin fashion and processes completions
/// as they arrive. This keeps the NVMe queue continuously full.
///
/// # Safety
///
/// All `mem_ptrs` and `gpu_dsts` must be valid pointers for their respective sizes.
#[allow(clippy::too_many_arguments)]
pub unsafe fn multi_object_pipeline(
    drive: &dyn IBlockDevice,
    gpu: &dyn IGpuServices,
    streams: &[GpuStream; 2],
    channels: &ClientChannels,
    objects: &[(
        *mut u8,            // mem_tier_ptr
        *mut std::ffi::c_void, // gpu_dst
        u64,                // start_lba
        usize,              // total_bytes
    )],
    chunk_size: usize,
    max_queue_depth: usize,
) -> Result<Vec<Result<(), DispatcherError>>, DispatcherError> {
    let block_size = drive.block_size() as usize;

    if objects.is_empty() {
        return Ok(Vec::new());
    }

    // Pre-compute segments and DmaBuffer wrappers for all objects.
    struct ObjState {
        segments: Vec<crate::io_segmenter::IoSegment>,
        chunk_bufs: Vec<Arc<Mutex<DmaBuffer>>>,
        total_bytes: usize,
        mem_tier_ptr: *mut u8,
        gpu_dst: *mut std::ffi::c_void,
        completed_bitmap: Vec<bool>,
        gpu_copy_frontier: usize,
        chunks_submitted: usize,
        chunks_completed: usize,
    }

    let mut obj_states: Vec<ObjState> = Vec::with_capacity(objects.len());
    let mut total_chunks: usize = 0;

    for &(mem_tier_ptr, gpu_dst, start_lba, total_bytes) in objects {
        let aligned_bytes = total_bytes.next_multiple_of(block_size);
        let segments = io_segmenter::segment_io(
            start_lba,
            aligned_bytes,
            chunk_size as u32,
            block_size as u32,
        );

        let chunk_bufs: Vec<Arc<Mutex<DmaBuffer>>> = segments
            .iter()
            .map(|seg| {
                let ptr = unsafe { mem_tier_ptr.add(seg.buffer_offset) as *mut std::ffi::c_void };
                let buf_size = seg.length.next_multiple_of(block_size);
                let buf = unsafe { DmaBuffer::from_raw(ptr, buf_size, noop_free, -1) }
                    .map_err(|e| {
                        DispatcherError::AllocationFailed(format!("DmaBuffer wrap chunk: {e}"))
                    })?;
                Ok(Arc::new(Mutex::new(buf)))
            })
            .collect::<Result<Vec<_>, DispatcherError>>()?;

        let num_segs = segments.len();
        total_chunks += num_segs;
        let completed_bitmap = vec![false; num_segs];

        obj_states.push(ObjState {
            segments,
            chunk_bufs,
            total_bytes,
            mem_tier_ptr,
            gpu_dst,
            completed_bitmap,
            gpu_copy_frontier: 0,
            chunks_submitted: 0,
            chunks_completed: 0,
        });
    }

    let max_inflight = max_queue_depth.min(total_chunks);

    // Track (obj_idx, seg_idx) in submission order (FIFO).
    let mut inflight: std::collections::VecDeque<(usize, usize)> =
        std::collections::VecDeque::with_capacity(max_inflight);

    // Prime the sliding window by submitting sequentially per object.
    // This maximizes sequential read prefetch in the SSD controller.
    let mut submitted = 0;
    'prime: for obj_idx in 0..obj_states.len() {
        let obj = &mut obj_states[obj_idx];
        while obj.chunks_submitted < obj.segments.len() {
            if submitted >= max_inflight {
                break 'prime;
            }
            let seg_idx = obj.chunks_submitted;
            channels
                .command_tx
                .send(Command::ReadAsync {
                    ns_id: 1,
                    lba: obj.segments[seg_idx].lba,
                    buf: Arc::clone(&obj.chunk_bufs[seg_idx]),
                    timeout_ms: READ_TIMEOUT_MS,
                })
                .map_err(|e| DispatcherError::IoError(format!("ReadAsync send: {e}")))?;
            inflight.push_back((obj_idx, seg_idx));
            obj.chunks_submitted += 1;
            submitted += 1;
        }
    }

    let mut stream_idx = 0usize;

    // Process all completions.
    for _ in 0..total_chunks {
        let (obj_idx, seg_idx) = match channels.completion_rx.recv() {
            Ok(Completion::ReadDone { handle, result }) => {
                result.map_err(|e| {
                    DispatcherError::IoError(format!("SSD read (handle {:?}): {e}", handle))
                })?;
                inflight.pop_front().unwrap()
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
        };

        // Submit next read — prefer the same object for sequential locality,
        // then fall through to the next object with remaining chunks.
        let mut replenished = false;
        for offset in 0..obj_states.len() {
            let try_idx = (obj_idx + offset) % obj_states.len();
            let obj = &mut obj_states[try_idx];
            if obj.chunks_submitted < obj.segments.len() {
                let next_seg = obj.chunks_submitted;
                channels
                    .command_tx
                    .send(Command::ReadAsync {
                        ns_id: 1,
                        lba: obj.segments[next_seg].lba,
                        buf: Arc::clone(&obj.chunk_bufs[next_seg]),
                        timeout_ms: READ_TIMEOUT_MS,
                    })
                    .map_err(|e| DispatcherError::IoError(format!("ReadAsync submit: {e}")))?;
                inflight.push_back((try_idx, next_seg));
                obj.chunks_submitted += 1;
                replenished = true;
                break;
            }
        }
        let _ = replenished;

        // Mark segment complete and advance GPU copy frontier.
        let obj = &mut obj_states[obj_idx];
        obj.completed_bitmap[seg_idx] = true;
        obj.chunks_completed += 1;

        // Coalesce consecutive completed segments into a single GPU copy.
        if obj.completed_bitmap[obj.gpu_copy_frontier] {
            let coalesce_start = obj.gpu_copy_frontier;
            while obj.gpu_copy_frontier < obj.segments.len()
                && obj.completed_bitmap[obj.gpu_copy_frontier]
            {
                obj.gpu_copy_frontier += 1;
            }

            let start_offset = obj.segments[coalesce_start].buffer_offset;
            let end_seg = &obj.segments[obj.gpu_copy_frontier - 1];
            let end_offset = end_seg.buffer_offset + end_seg.length;
            let copy_len = end_offset.min(obj.total_bytes) - start_offset.min(obj.total_bytes);

            if copy_len > 0 {
                let current_stream = streams[stream_idx % 2];
                let src_ptr = unsafe { obj.mem_tier_ptr.add(start_offset) as *const std::ffi::c_void };
                let dst_ptr = unsafe { (obj.gpu_dst as *mut u8).add(start_offset) as *mut std::ffi::c_void };
                gpu.memcpy_h2d_async(src_ptr, dst_ptr, copy_len, current_stream)
                    .map_err(|e| {
                        DispatcherError::IoError(format!("GPU async DMA copy failed: {e}"))
                    })?;
                stream_idx += 1;
            }
        }
    }

    // Sync both streams.
    for s in streams {
        gpu.stream_synchronize(*s)
            .map_err(|e| DispatcherError::IoError(format!("final stream_synchronize: {e}")))?;
    }

    // Forget all DmaBuffer wrappers (noop_free, but avoid double-free logic).
    for obj in obj_states {
        for buf in obj.chunk_bufs {
            std::mem::forget(Arc::try_unwrap(buf).ok());
        }
    }

    // All objects succeeded (errors are returned via ? above).
    Ok(objects.iter().map(|_| Ok(())).collect())
}

/// Read-then-copy variant: reads all NVMe chunks into memory-tier, then issues
/// a single large GPU H2D copy for the entire object.
///
/// This reduces CUDA API overhead (1 call vs N calls) at the cost of not
/// overlapping SSD reads with GPU DMA. Beneficial when the GPU copy is fast
/// relative to the SSD read latency (PCIe Gen4 x16 H2D >> NVMe read).
///
/// # Safety
///
/// Same as [`pipelined_ssd_to_gpu_zero_copy`].
#[allow(clippy::too_many_arguments)]
pub unsafe fn bulk_ssd_read_then_gpu_copy(
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
    let chunk_bufs: Vec<Arc<Mutex<DmaBuffer>>> = segments
        .iter()
        .map(|seg| {
            let ptr = unsafe { mem_tier_ptr.add(seg.buffer_offset) as *mut std::ffi::c_void };
            let buf_size = seg.length.next_multiple_of(block_size);
            let buf = unsafe { DmaBuffer::from_raw(ptr, buf_size, noop_free, -1) }
                .map_err(|e| {
                    DispatcherError::AllocationFailed(format!("DmaBuffer wrap chunk: {e}"))
                })?;
            Ok(Arc::new(Mutex::new(buf)))
        })
        .collect::<Result<Vec<_>, DispatcherError>>()?;

    let max_inflight = max_queue_depth.min(num_chunks);

    // Phase 1: Read all chunks from SSD into memory-tier using sliding window.
    let mut inflight: std::collections::VecDeque<usize> =
        std::collections::VecDeque::with_capacity(max_inflight);

    for i in 0..max_inflight {
        channels
            .command_tx
            .send(Command::ReadAsync {
                ns_id: 1,
                lba: segments[i].lba,
                buf: Arc::clone(&chunk_bufs[i]),
                timeout_ms: READ_TIMEOUT_MS,
            })
            .map_err(|e| DispatcherError::IoError(format!("ReadAsync send #{i}: {e}")))?;
        inflight.push_back(i);
    }

    let mut next_to_submit = max_inflight;

    for _completed in 0..num_chunks {
        match channels.completion_rx.recv() {
            Ok(Completion::ReadDone { handle, result }) => {
                result.map_err(|e| {
                    DispatcherError::IoError(format!("SSD read (handle {:?}): {e}", handle))
                })?;
                inflight.pop_front().unwrap();
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
        };

        if next_to_submit < num_chunks {
            channels
                .command_tx
                .send(Command::ReadAsync {
                    ns_id: 1,
                    lba: segments[next_to_submit].lba,
                    buf: Arc::clone(&chunk_bufs[next_to_submit]),
                    timeout_ms: READ_TIMEOUT_MS,
                })
                .map_err(|e| {
                    DispatcherError::IoError(format!(
                        "ReadAsync submit #{next_to_submit}: {e}"
                    ))
                })?;
            inflight.push_back(next_to_submit);
            next_to_submit += 1;
        }
    }

    // Phase 2: Single large GPU H2D copy from memory-tier to GPU.
    // The entire object is now contiguous in the memory-tier slot.
    gpu.memcpy_h2d_async(
        mem_tier_ptr as *const std::ffi::c_void,
        gpu_dst,
        total_bytes,
        streams[0],
    )
    .map_err(|e| {
        DispatcherError::IoError(format!("GPU H2D bulk copy failed: {e}"))
    })?;

    gpu.stream_synchronize(streams[0])
        .map_err(|e| DispatcherError::IoError(format!("final stream_synchronize: {e}")))?;

    // Forget all DmaBuffer wrappers (noop_free, but avoid double-free logic).
    for buf in chunk_bufs {
        std::mem::forget(Arc::try_unwrap(buf).ok());
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
