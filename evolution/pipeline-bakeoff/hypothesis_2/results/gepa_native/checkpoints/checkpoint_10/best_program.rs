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

// ===== EVOLVE-BLOCK: CONSTANTS =====
// These constants control pipeline behavior. Evolution may change values
// or add new constants (e.g., adaptive thresholds).

/// Number of ring buffers for pipelined transfers.
pub const PIPELINE_RING_SIZE: usize = 8;

/// Timeout for async NVMe read operations (ms).
const READ_TIMEOUT_MS: u64 = 5000;

/// Queue depth for zero-copy pipeline (max concurrent NVMe reads).
/// Higher values increase device utilization but consume more memory-tier bandwidth.
/// QD=32 matches raw NVMe optimal queue depth for Gen4 drives.
const ZERO_COPY_DEPTH: usize = 32;

/// How often to synchronize CUDA streams (every N completions).
/// Lower values reduce GPU command queue depth but add sync overhead.
/// Set high to minimize sync stalls; final sync catches everything.
const SYNC_FREQUENCY: usize = 128;

// ===== END EVOLVE-BLOCK: CONSTANTS =====

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

    // Prime the ring: submit initial async reads.
    let prime_count = ring_size.min(num_chunks);
    for i in 0..prime_count {
        let slot = i % ring_size;
        channels
            .command_tx
            .send(Command::ReadAsync {
                ns_id: 1,
                lba: segments[i].lba,
                buf: Arc::clone(&ring.buffers[slot]),
                timeout_ms: READ_TIMEOUT_MS,
            })
            .map_err(|e| DispatcherError::IoError(format!("ReadAsync send #{i}: {e}")))?;
    }

    let mut next_to_submit = prime_count;

    // Steady-state: process completions, issue async GPU copies, resubmit reads.
    for completed in 0..num_chunks {
        match channels.completion_rx.recv() {
            Ok(Completion::ReadDone { result, .. }) => {
                result.map_err(|e| {
                    DispatcherError::IoError(format!("SSD read #{completed}: {e}"))
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

        let slot = completed % ring_size;
        let seg = &segments[completed];
        let copy_len = seg.length.min(total_bytes.saturating_sub(seg.buffer_offset));
        let current_stream = streams[completed % 2];

        let guard = ring.buffers[slot].lock().unwrap();

        // memcpy ring buffer → memory-tier slot (CPU copy, immediate).
        if copy_len > 0 {
            unsafe {
                std::ptr::copy_nonoverlapping(
                    guard.as_ptr() as *const u8,
                    mem_tier_ptr.add(seg.buffer_offset),
                    copy_len,
                );
            }
        }

        // async DMA copy ring buffer → GPU on current stream.
        gpu.dma_copy_to_device_async(
            &guard,
            unsafe { (gpu_dst as *mut u8).add(seg.buffer_offset) as *mut std::ffi::c_void },
            copy_len,
            current_stream,
        )
        .map_err(|e| {
            DispatcherError::IoError(format!("GPU async DMA copy #{completed} failed: {e}"))
        })?;

        // Batch-sync both streams once per ring cycle.
        if (completed + 1) % ring_size == 0 {
            gpu.stream_synchronize(streams[0])
                .map_err(|e| DispatcherError::IoError(format!("stream_synchronize failed: {e}")))?;
            gpu.stream_synchronize(streams[1])
                .map_err(|e| DispatcherError::IoError(format!("stream_synchronize failed: {e}")))?;
        }

        drop(guard);

        // Resubmit next NVMe read into the now-free ring slot.
        if next_to_submit < num_chunks {
            channels
                .command_tx
                .send(Command::ReadAsync {
                    ns_id: 1,
                    lba: segments[next_to_submit].lba,
                    buf: Arc::clone(&ring.buffers[slot]),
                    timeout_ms: READ_TIMEOUT_MS,
                })
                .map_err(|e| {
                    DispatcherError::IoError(format!(
                        "ReadAsync resubmit #{next_to_submit}: {e}"
                    ))
                })?;
            next_to_submit += 1;
        }
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

// ===== EVOLVE-BLOCK: ZERO_COPY_PIPELINE =====
// This is the primary evolution target. The function reads from NVMe directly
// into a memory-tier slot (no intermediate buffer), then streams to GPU.
//
// Key tunables within this function:
// - ZERO_COPY_DEPTH: how many NVMe reads in flight (higher = more device utilization)
// - SYNC_FREQUENCY: how often to sync CUDA streams (tradeoff: queue depth vs overhead)
// - Stream assignment: currently round-robin (completed % 2)
// - Completion processing: currently sequential
//
// What evolution might discover:
// - Higher ZERO_COPY_DEPTH (32-64) to match raw NVMe QD=32 performance
// - Adaptive sync frequency based on num_chunks (sync at end for small transfers)
// - More CUDA streams (3-4) for better GPU DMA overlap
// - Batched completion processing (drain multiple completions before issuing DMAs)
// - Size-adaptive strategy (different logic for 8-chunk vs 128-chunk transfers)

/// Zero-copy pipeline: read from SSD directly into a memory-tier slot, stream to GPU.
///
/// Unlike [`pipelined_ssd_to_gpu`] which uses intermediate ring buffers and a CPU
/// memcpy per chunk, this reads NVMe directly into the memory-tier slot (which must
/// be CUDA-pinned + SPDK-registered), then issues async H2D from the same memory.
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

    // Prime: submit initial async reads directly into memory-tier chunk offsets.
    let max_inflight = ZERO_COPY_DEPTH.min(num_chunks);
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
    }

    let mut next_to_submit = max_inflight;

    // Process completions: after each NVMe read completes into memory-tier,
    // issue async H2D from the same memory-tier offset to GPU.
    // Use a single stream for very small transfers (≤8 chunks = 1 MiB)
    // to reduce stream switching overhead. Larger transfers benefit from dual streams.
    let use_single_stream = num_chunks <= 8;

    for completed in 0..num_chunks {
        match channels.completion_rx.recv() {
            Ok(Completion::ReadDone { result, .. }) => {
                result.map_err(|e| {
                    DispatcherError::IoError(format!("SSD read #{completed}: {e}"))
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

        let seg = &segments[completed];
        let copy_len = seg.length.min(total_bytes.saturating_sub(seg.buffer_offset));
        let current_stream = if use_single_stream {
            streams[0]
        } else {
            streams[completed % 2]
        };

        // Async H2D: memory-tier chunk → GPU (same memory NVMe just wrote into).
        let guard = chunk_bufs[completed].lock().unwrap();
        gpu.dma_copy_to_device_async(
            &guard,
            unsafe { (gpu_dst as *mut u8).add(seg.buffer_offset) as *mut std::ffi::c_void },
            copy_len,
            current_stream,
        )
        .map_err(|e| {
            DispatcherError::IoError(format!("GPU async DMA copy #{completed} failed: {e}"))
        })?;
        drop(guard);

        // Only sync for very large transfers (>128 chunks, i.e., >16 MiB) to prevent
        // GPU command queue overflow. For typical workloads (≤128 chunks), the final
        // sync is sufficient and avoids stalling the pipeline mid-transfer.
        if num_chunks > 128 && (completed + 1) % SYNC_FREQUENCY == 0 {
            gpu.stream_synchronize(streams[0])
                .map_err(|e| DispatcherError::IoError(format!("stream_synchronize failed: {e}")))?;
            gpu.stream_synchronize(streams[1])
                .map_err(|e| DispatcherError::IoError(format!("stream_synchronize failed: {e}")))?;
        }

        // Submit next NVMe read (into the next memory-tier chunk).
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
                        "ReadAsync resubmit #{next_to_submit}: {e}"
                    ))
                })?;
            next_to_submit += 1;
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

// ===== END EVOLVE-BLOCK: ZERO_COPY_PIPELINE =====

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
