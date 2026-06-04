// --- FILE: pipeline.rs ---
//! Ring-buffer pipelined reader for SSD→DRAM→GPU transfers.
//!
//! Uses async NVMe reads with multiple in-flight commands and multiple CUDA
//! streams to overlap SSD I/O with GPU DMA copies.

use std::sync::{Arc, Mutex};

use interfaces::{
    ClientChannels, Command, Completion, DmaBuffer, DispatcherError, GpuStream, IBlockDevice,
    IGpuServices,
};

use crate::io_segmenter;

/// Number of ring buffers for pipelined transfers.
pub const PIPELINE_RING_SIZE: usize = 8;

/// Number of CUDA streams for overlapped H2D copies.
pub const NUM_STREAMS: usize = 4;

/// Timeout for async NVMe read operations (ms).
const READ_TIMEOUT_MS: u64 = 5000;

/// Pre-allocated ring of CUDA-pinned + SPDK-registered DMA buffers and CUDA streams.
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
/// Algorithm:
/// 1. Submit a batch of reads (up to ring_size) into ring slots
/// 2. Wait for all completions in the batch
/// 3. memcpy each to memory-tier and issue async H2D
/// 4. Sync streams before reusing ring slots
///
/// # Safety
///
/// - `mem_tier_ptr` must be valid for writes of at least `total_bytes`.
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

    // Process chunks in batches of ring_size.
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

    // Final sync.
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

    for _completed in 0..num_chunks {
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

        // Submit the next read immediately so SSD I/O overlaps with GPU copy.
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

        // GPU H2D copy.
        let seg = &segments[seg_idx];
        let copy_len = seg.length.min(total_bytes.saturating_sub(seg.buffer_offset));
        let current_stream = streams[stream_idx % 2];

        let guard = chunk_bufs[seg_idx].lock().unwrap();
        gpu.dma_copy_to_device_async(
            &guard,
            unsafe { (gpu_dst as *mut u8).add(seg.buffer_offset) as *mut std::ffi::c_void },
            copy_len,
            current_stream,
        )
        .map_err(|e| {
            DispatcherError::IoError(format!("GPU async DMA copy (seg {seg_idx}) failed: {e}"))
        })?;
        drop(guard);
        stream_idx += 1;

        // Periodically sync to bound GPU command queue depth.
        if stream_idx % 16 == 0 {
            gpu.stream_synchronize(streams[0])
                .map_err(|e| DispatcherError::IoError(format!("stream_synchronize failed: {e}")))?;
            gpu.stream_synchronize(streams[1])
                .map_err(|e| DispatcherError::IoError(format!("stream_synchronize failed: {e}")))?;
        }
    }

    // Final sync.
    for s in streams {
        gpu.stream_synchronize(*s)
            .map_err(|e| DispatcherError::IoError(format!("final stream_synchronize: {e}")))?;
    }

    // Forget DmaBuffer wrappers (noop_free).
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

// --- FILE: lib.rs ---
//! Dispatcher buffer management and promotion logic.
//!
//! This module manages the ring of DMA buffers and coordinates the
//! promote_and_serve path that moves data from SSD to GPU for inference.

use std::sync::{Arc, Mutex};

use interfaces::{
    ClientChannels, DmaBuffer, DispatcherError, GpuStream, IBlockDevice, IGpuServices,
};

use crate::pipeline::{
    self, PipelineRing, PIPELINE_RING_SIZE,
};
use crate::io_segmenter;

/// Default chunk size for DMA transfers (4 MiB for optimal PCIe throughput).
pub const DEFAULT_CHUNK_SIZE: usize = 4 * 1024 * 1024;

/// Maximum NVMe queue depth for the sliding-window pipeline.
pub const MAX_QUEUE_DEPTH: usize = 32;

/// Dispatcher state holding pre-allocated resources.
pub struct Dispatcher {
    /// Pipeline ring for transfers.
    pub pipeline_ring: Option<PipelineRing>,
    /// Chunk size used for transfers.
    pub chunk_size: usize,
}

impl Dispatcher {
    /// Create a new Dispatcher with pre-allocated pipeline resources.
    pub fn new(gpu: &dyn IGpuServices, chunk_size: usize) -> Result<Self, DispatcherError> {
        let pipeline_ring = PipelineRing::new(gpu, chunk_size)?;

        Ok(Self {
            pipeline_ring: Some(pipeline_ring),
            chunk_size,
        })
    }

    /// Destroy all allocated resources.
    pub fn destroy(self, gpu: &dyn IGpuServices) {
        if let Some(ring) = self.pipeline_ring {
            ring.destroy(gpu);
        }
    }
}

/// Promote data from SSD to GPU using the pipelined host-bounce path.
///
/// # Safety
///
/// - `mem_tier_ptr` must be valid for writes of at least `total_bytes`.
/// - `gpu_dst` must be a valid GPU destination pointer for `total_bytes`.
#[allow(clippy::too_many_arguments)]
pub unsafe fn promote_and_serve(
    drive: &dyn IBlockDevice,
    gpu: &dyn IGpuServices,
    dispatcher: &Dispatcher,
    mem_tier_ptr: *mut u8,
    gpu_dst: *mut std::ffi::c_void,
    start_lba: u64,
    total_bytes: usize,
) -> Result<(), DispatcherError> {
    if let Some(ref ring) = dispatcher.pipeline_ring {
        return unsafe {
            pipeline::pipelined_ssd_to_gpu(
                drive, gpu, ring, mem_tier_ptr, gpu_dst, start_lba, total_bytes,
            )
        };
    }

    Err(DispatcherError::IoError(
        "No pipeline ring available".into(),
    ))
}

/// Promote using the zero-copy path (for pre-registered memory-tier regions).
///
/// # Safety
///
/// - `mem_tier_ptr` must be SPDK-registered and CUDA-pinned.
/// - `gpu_dst` must be a valid GPU destination pointer.
#[allow(clippy::too_many_arguments)]
pub unsafe fn promote_and_serve_zero_copy(
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
    unsafe {
        pipeline::pipelined_ssd_to_gpu_zero_copy(
            drive,
            gpu,
            streams,
            channels,
            mem_tier_ptr,
            gpu_dst,
            start_lba,
            total_bytes,
            chunk_size,
            MAX_QUEUE_DEPTH,
        )
    }
}

// --- FILE: dma.rs ---
//! DMA buffer creation functions for various memory types.
//!
//! Provides allocation routines for:
//! - CUDA-pinned + SPDK-registered host DMA buffers (for host-bounce path)
//! - GPU-resident + SPDK-registered DMA buffers (for GDS P2P path)
//! - Registered host memory regions (for zero-copy path)

use interfaces::{DmaBuffer, IGpuServices};

/// Allocate a CUDA-pinned, SPDK-registered DMA buffer on the host.
///
/// This is the standard buffer type for the host-bounce pipeline path.
/// The buffer is allocated with `cudaHostAlloc` (pinned, portable, mapped)
/// and registered with `spdk_mem_register` for NVMe DMA access.
pub fn allocate_pinned_dma_buffer(
    gpu: &dyn IGpuServices,
    size: usize,
) -> Result<DmaBuffer, String> {
    gpu.allocate_pinned_dma_buffer(size)
}

/// Allocate a GPU-resident DMA buffer for GPUDirect Storage P2P transfers.
///
/// This buffer resides in GPU device memory (BAR1-mapped) and is registered
/// with SPDK so that NVMe controllers can DMA directly into GPU memory
/// via nvidia-peermem/gdrdrv. This eliminates the host-DRAM bounce entirely.
///
/// Requirements:
/// - nvidia-peermem kernel module loaded
/// - gdrdrv kernel module loaded
/// - GPU must support P2P DMA (A30 does via BAR1)
///
/// Falls back to pinned host buffer if GPU DMA buffer allocation fails.
pub fn allocate_gpu_dma_buffer(
    gpu: &dyn IGpuServices,
    size: usize,
) -> Result<DmaBuffer, String> {
    match gpu.allocate_gpu_dma_buffer(size) {
        Ok(buf) => Ok(buf),
        Err(e) => {
            eprintln!(
                "GPU DMA buffer allocation failed ({e}), falling back to host-pinned"
            );
            gpu.allocate_pinned_dma_buffer(size)
        }
    }
}

/// Register an existing host memory region for DMA access.
///
/// This registers the memory with both CUDA (`cudaHostRegister`) and SPDK
/// (`spdk_mem_register`) so it can be used as both an NVMe DMA target and
/// a source for async H2D copies without additional staging buffers.
///
/// # Safety
///
/// The pointer must be valid for the given size and must remain valid
/// for the lifetime of the registration.
pub unsafe fn register_host_memory(
    gpu: &dyn IGpuServices,
    ptr: *mut std::ffi::c_void,
    size: usize,
) -> Result<(), String> {
    gpu.register_host_memory(ptr, size)
}

/// Unregister a previously registered host memory region.
///
/// # Safety
///
/// Must only be called with a pointer that was previously registered.
pub unsafe fn unregister_host_memory(
    gpu: &dyn IGpuServices,
    ptr: *mut std::ffi::c_void,
    size: usize,
) -> Result<(), String> {
    gpu.unregister_host_memory(ptr, size)
}