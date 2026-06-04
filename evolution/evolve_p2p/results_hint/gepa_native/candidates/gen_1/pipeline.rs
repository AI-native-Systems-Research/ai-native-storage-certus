//! Ring-buffer pipelined reader for SSD→DRAM→GPU transfers.
//!
//! Uses async NVMe reads with multiple in-flight commands and multiple CUDA
//! streams to maximize overlap between SSD I/O and GPU DMA copies.
//! Optimized for throughput and tail latency on cold lookups.

use std::sync::{Arc, Mutex};

use interfaces::{
    ClientChannels, Command, Completion, DmaBuffer, DispatcherError, GpuStream, IBlockDevice,
    IGpuServices,
};

use crate::io_segmenter;

/// Number of ring buffers for pipelined transfers.
/// Increased to allow deeper NVMe queue depth and better SSD utilization.
pub const PIPELINE_RING_SIZE: usize = 16;

/// Number of CUDA streams for overlapping H2D transfers.
const NUM_STREAMS: usize = 4;

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
/// Implements a sliding-window approach where NVMe reads and GPU copies are
/// fully overlapped for maximum throughput.
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

    // Sliding window: submit up to ring_size reads, then as each completes,
    // immediately copy and submit the next read.
    let mut inflight: std::collections::VecDeque<usize> =
        std::collections::VecDeque::with_capacity(ring_size);

    // Prime the pipeline with initial reads.
    for i in 0..ring_size {
        channels
            .command_tx
            .send(Command::ReadAsync {
                ns_id: 1,
                lba: segments[i].lba,
                buf: Arc::clone(&ring.buffers[i]),
                timeout_ms: READ_TIMEOUT_MS,
            })
            .map_err(|e| {
                DispatcherError::IoError(format!("ReadAsync send #{i}: {e}"))
            })?;
        inflight.push_back(i);
    }

    let mut next_to_submit = ring_size;
    let mut stream_idx = 0usize;

    for _completed in 0..num_chunks {
        // Wait for the oldest in-flight read.
        let slot_idx = match channels.completion_rx.recv() {
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

        // Determine which segment this slot was reading.
        // slot_idx maps to the segment that was assigned to it.
        // We need to track which segment each slot is reading.
        // Since we assign slots in order (slot i gets segment i, then ring wraps),
        // the segment index for this completion is determined by completion order.
        let seg_idx = _completed;
        let seg = &segments[seg_idx];
        let copy_len = seg.length.min(total_bytes.saturating_sub(seg.buffer_offset));
        let current_stream = streams[stream_idx % 2];

        let guard = ring.buffers[slot_idx].lock().unwrap();

        // memcpy ring buffer → memory-tier slot.
        if copy_len > 0 {
            std::ptr::copy_nonoverlapping(
                guard.as_ptr() as *const u8,
                mem_tier_ptr.add(seg.buffer_offset),
                copy_len,
            );
        }

        // Async DMA copy ring buffer → GPU.
        gpu.dma_copy_to_device_async(
            &guard,
            (gpu_dst as *mut u8).add(seg.buffer_offset) as *mut std::ffi::c_void,
            copy_len,
            current_stream,
        )
        .map_err(|e| {
            DispatcherError::IoError(format!(
                "GPU async DMA copy #{seg_idx} failed: {e}"
            ))
        })?;

        drop(guard);
        stream_idx += 1;

        // Submit next read into the freed slot immediately to keep SSD busy.
        if next_to_submit < num_chunks {
            channels
                .command_tx
                .send(Command::ReadAsync {
                    ns_id: 1,
                    lba: segments[next_to_submit].lba,
                    buf: Arc::clone(&ring.buffers[slot_idx]),
                    timeout_ms: READ_TIMEOUT_MS,
                })
                .map_err(|e| {
                    DispatcherError::IoError(format!(
                        "ReadAsync submit #{next_to_submit}: {e}"
                    ))
                })?;
            inflight.push_back(slot_idx);
            next_to_submit += 1;
        }

        // Periodically sync to prevent GPU command queue from growing unbounded
        // and to free ring slots (the GPU must finish before we can safely reuse
        // the pinned buffer for another read).
        if stream_idx % ring_size == 0 {
            gpu.stream_synchronize(streams[0])
                .map_err(|e| DispatcherError::IoError(format!("stream_synchronize failed: {e}")))?;
            gpu.stream_synchronize(streams[1])
                .map_err(|e| DispatcherError::IoError(format!("stream_synchronize failed: {e}")))?;
        }
    }

    // Final sync to ensure all GPU copies are complete.
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
/// This is the preferred fast path — eliminates the CPU memcpy entirely and allows
/// maximum overlap between SSD reads and GPU DMA transfers.
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

    // Use larger chunk size for better sequential read performance from NVMe.
    // NVMe SSDs achieve peak throughput with 128KB+ I/O sizes.
    let effective_chunk_size = chunk_size.max(131072); // At least 128KB per I/O

    let segments = io_segmenter::segment_io(
        start_lba,
        aligned_bytes,
        effective_chunk_size as u32,
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
            let ptr = mem_tier_ptr.add(seg.buffer_offset) as *mut std::ffi::c_void;
            let buf_size = seg.length.next_multiple_of(block_size);
            let buf = DmaBuffer::from_raw(ptr, buf_size, noop_free, -1)
                .map_err(|e| {
                    DispatcherError::AllocationFailed(format!("DmaBuffer wrap chunk: {e}"))
                })?;
            Ok(Arc::new(Mutex::new(buf)))
        })
        .collect::<Result<Vec<_>, DispatcherError>>()?;

    // Use deeper queue depth to keep SSD saturated.
    // NVMe Gen4 SSDs need QD>=32 for full bandwidth.
    let max_inflight = max_queue_depth.max(32).min(num_chunks);

    // Track in-flight segment indices in submission order (FIFO queue).
    let mut inflight: std::collections::VecDeque<usize> =
        std::collections::VecDeque::with_capacity(max_inflight);

    // Prime the sliding window with maximum in-flight reads.
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
    let mut gpu_copies_since_sync = 0usize;

    // Sliding-window pipeline: as each NVMe read completes, immediately issue
    // the GPU H2D copy for that segment and submit the next read. This maximizes
    // overlap between SSD I/O and GPU DMA.
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

        // Submit the next read immediately so SSD stays busy during GPU copy.
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

        // NVMe read for seg_idx is complete — GPU H2D copy can start now.
        let seg = &segments[seg_idx];
        let copy_len = seg.length.min(total_bytes.saturating_sub(seg.buffer_offset));
        let current_stream = streams[stream_idx % 2];

        let guard = chunk_bufs[seg_idx].lock().unwrap();
        gpu.dma_copy_to_device_async(
            &guard,
            (gpu_dst as *mut u8).add(seg.buffer_offset) as *mut std::ffi::c_void,
            copy_len,
            current_stream,
        )
        .map_err(|e| {
            DispatcherError::IoError(format!("GPU async DMA copy (seg {seg_idx}) failed: {e}"))
        })?;
        drop(guard);
        stream_idx += 1;
        gpu_copies_since_sync += 1;

        // Sync less frequently — only when GPU queue might overflow.
        // This reduces synchronization overhead and improves throughput.
        if gpu_copies_since_sync >= 32 {
            gpu.stream_synchronize(streams[0])
                .map_err(|e| DispatcherError::IoError(format!("stream_synchronize failed: {e}")))?;
            gpu.stream_synchronize(streams[1])
                .map_err(|e| DispatcherError::IoError(format!("stream_synchronize failed: {e}")))?;
            gpu_copies_since_sync = 0;
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pipeline_ring_size_is_reasonable() {
        let size = PIPELINE_RING_SIZE;
        assert!(size >= 2);
        assert!(size <= 32);
    }
}