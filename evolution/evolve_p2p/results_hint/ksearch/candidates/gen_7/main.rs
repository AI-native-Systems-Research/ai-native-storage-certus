// --- FILE: pipeline.rs ---
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

/// Maximum NVMe queue depth for saturating drive bandwidth at Gen4 speeds.
const MAX_NVME_QUEUE_DEPTH: usize = 64;

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

    // Use QD64 to saturate NVMe Gen4 bandwidth.
    let effective_qd = max_queue_depth.max(MAX_NVME_QUEUE_DEPTH).min(num_chunks);

    // Track in-flight segment indices in submission order (FIFO queue).
    let mut inflight: std::collections::VecDeque<usize> =
        std::collections::VecDeque::with_capacity(effective_qd);

    // Prime the sliding window with up to effective_qd in-flight reads.
    for i in 0..effective_qd {
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

    let mut next_to_submit = effective_qd;
    let mut stream_idx = 0usize;

    // Sliding-window pipeline: as each NVMe read completes, immediately issue
    // the GPU H2D copy for that segment and submit the next read.
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

        // Submit the next read immediately so SSD I/O overlaps with the GPU
        // copy we're about to issue.
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
            unsafe { (gpu_dst as *mut u8).add(seg.buffer_offset) as *mut std::ffi::c_void },
            copy_len,
            current_stream,
        )
        .map_err(|e| {
            DispatcherError::IoError(format!("GPU async DMA copy (seg {seg_idx}) failed: {e}"))
        })?;
        drop(guard);
        stream_idx += 1;

        // Periodically sync to bound the GPU command queue depth.
        if stream_idx % 16 == 0 {
            gpu.stream_synchronize(streams[0])
                .map_err(|e| DispatcherError::IoError(format!("stream_synchronize failed: {e}")))?;
            gpu.stream_synchronize(streams[1])
                .map_err(|e| DispatcherError::IoError(format!("stream_synchronize failed: {e}")))?;
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

/// P2P pipeline: read from SSD directly into GPU BAR1 memory, bypassing host DRAM.
///
/// This implements GPUDirect Storage P2P where NVMe controllers DMA directly into
/// GPU memory. The host CPU is not involved in the data path at all.
///
/// # Safety
///
/// - `gpu_dst` must be a valid GPU destination pointer for `total_bytes`.
/// - `mem_tier_ptr` if non-null, will receive a copy from GPU after transfer completes.
#[cfg(feature = "p2p")]
#[allow(clippy::too_many_arguments)]
pub unsafe fn pipelined_ssd_to_gpu_p2p(
    drive: &dyn IBlockDevice,
    _gpu: &dyn IGpuServices,
    streams: &[GpuStream; 2],
    channels: &ClientChannels,
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

    // Create GPU BAR1 DMA buffers for each segment — NVMe will DMA directly here.
    // We reuse a fixed pool of BAR1 buffer registrations sized to the queue depth
    // to avoid per-segment registration overhead.
    let effective_qd = MAX_NVME_QUEUE_DEPTH.min(num_chunks);

    // Pre-register a ring of GPU BAR1 DMA buffers sized to effective_qd.
    // Each buffer covers one chunk_size region. We map them to rotating offsets.
    let ring_bufs: Vec<Arc<Mutex<DmaBuffer>>> = (0..effective_qd)
        .map(|i| {
            let seg = &segments[i];
            let gpu_chunk_ptr = unsafe { (gpu_dst as *mut u8).add(seg.buffer_offset) as *mut std::ffi::c_void };
            let buf_size = seg.length.next_multiple_of(block_size);
            let buf = gpu_services::dma::create_spdk_dma_buffer_from_gpu_bar(gpu_chunk_ptr, buf_size)
                .map_err(|e| {
                    DispatcherError::AllocationFailed(format!("GPU BAR DMA buffer: {e}"))
                })?;
            Ok(Arc::new(Mutex::new(buf)))
        })
        .collect::<Result<Vec<_>, DispatcherError>>()?;

    // For segments beyond the initial ring, we need to create buffers on-demand
    // or reuse. Since each segment maps to a unique GPU offset, we create all upfront.
    let remaining_bufs: Vec<Arc<Mutex<DmaBuffer>>> = if num_chunks > effective_qd {
        (effective_qd..num_chunks)
            .map(|i| {
                let seg = &segments[i];
                let gpu_chunk_ptr = unsafe { (gpu_dst as *mut u8).add(seg.buffer_offset) as *mut std::ffi::c_void };
                let buf_size = seg.length.next_multiple_of(block_size);
                let buf = gpu_services::dma::create_spdk_dma_buffer_from_gpu_bar(gpu_chunk_ptr, buf_size)
                    .map_err(|e| {
                        DispatcherError::AllocationFailed(format!("GPU BAR DMA buffer (remaining): {e}"))
                    })?;
                Ok(Arc::new(Mutex::new(buf)))
            })
            .collect::<Result<Vec<_>, DispatcherError>>()?
    } else {
        Vec::new()
    };

    // Combined buffer access by index.
    let get_buf = |idx: usize| -> &Arc<Mutex<DmaBuffer>> {
        if idx < effective_qd {
            &ring_bufs[idx]
        } else {
            &remaining_bufs[idx - effective_qd]
        }
    };

    let mut inflight: std::collections::VecDeque<usize> =
        std::collections::VecDeque::with_capacity(effective_qd);

    // Prime the sliding window.
    for i in 0..effective_qd {
        channels
            .command_tx
            .send(Command::ReadAsync {
                ns_id: 1,
                lba: segments[i].lba,
                buf: Arc::clone(get_buf(i)),
                timeout_ms: READ_TIMEOUT_MS,
            })
            .map_err(|e| DispatcherError::IoError(format!("ReadAsync P2P send #{i}: {e}")))?;
        inflight.push_back(i);
    }

    let mut next_to_submit = effective_qd;

    // P2P sliding window: NVMe DMAs directly into GPU BAR1 memory.
    // No GPU H2D copy needed — data lands directly in GPU memory.
    for _completed in 0..num_chunks {
        let seg_idx = match channels.completion_rx.recv() {
            Ok(Completion::ReadDone { handle, result }) => {
                result.map_err(|e| {
                    DispatcherError::IoError(format!("P2P SSD read (handle {:?}): {e}", handle))
                })?;
                inflight.pop_front().unwrap()
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
        };

        // Submit the next read immediately to keep the NVMe queue full.
        if next_to_submit < num_chunks {
            channels
                .command_tx
                .send(Command::ReadAsync {
                    ns_id: 1,
                    lba: segments[next_to_submit].lba,
                    buf: Arc::clone(get_buf(next_to_submit)),
                    timeout_ms: READ_TIMEOUT_MS,
                })
                .map_err(|e| {
                    DispatcherError::IoError(format!(
                        "ReadAsync P2P submit #{next_to_submit}: {e}"
                    ))
                })?;
            inflight.push_back(next_to_submit);
            next_to_submit += 1;
        }

        // Data for seg_idx is now in GPU memory — no additional copy needed.
        let _ = seg_idx;
    }

    // Suppress unused variable warning for streams (kept for API compatibility).
    let _ = streams;

    // Forget DMA buffer wrappers (GPU memory is owned by the caller).
    for buf in ring_bufs {
        std::mem::forget(Arc::try_unwrap(buf).ok());
    }
    for buf in remaining_bufs {
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

    #[test]
    fn max_queue_depth_is_64() {
        assert_eq!(MAX_NVME_QUEUE_DEPTH, 64);
    }
}

// --- FILE: dma.rs ---
//! DMA buffer creation functions for various memory types.
//!
//! This module provides allocation helpers for DMA buffers used in the
//! storage-to-GPU data transfer pipeline.

use interfaces::DmaBuffer;

/// Create an SPDK-registered DMA buffer backed by GPU BAR1 memory.
///
/// This enables GPUDirect Storage P2P: NVMe controllers can DMA directly
/// into this GPU memory region without involving host DRAM.
///
/// # Arguments
/// * `dev_ptr` - A valid GPU device pointer (must be within BAR1 aperture)
/// * `size` - Size in bytes of the DMA buffer region
///
/// # Safety
/// The caller must ensure `dev_ptr` is a valid GPU memory pointer that remains
/// valid for the lifetime of the returned DmaBuffer. The GPU memory must be
/// accessible via BAR1 for PCIe peer-to-peer DMA.
///
/// # Returns
/// A `DmaBuffer` wrapping the GPU memory region, registered with SPDK for
/// direct NVMe DMA access.
#[cfg(feature = "p2p")]
pub fn create_spdk_dma_buffer_from_gpu_bar(
    dev_ptr: *mut std::ffi::c_void,
    size: usize,
) -> Result<DmaBuffer, String> {
    if dev_ptr.is_null() {
        return Err("GPU device pointer is null".to_string());
    }
    if size == 0 {
        return Err("Buffer size must be non-zero".to_string());
    }

    // Register the GPU BAR1 memory region with SPDK's memory subsystem.
    // This allows SPDK's NVMe driver to use this address as a DMA target.
    // The nvidia-peermem kernel module translates GPU virtual addresses to
    // physical BAR1 addresses for the NVMe controller's DMA engine.
    let rc = unsafe { spdk_mem_register(dev_ptr, size) };
    if rc != 0 {
        return Err(format!(
            "spdk_mem_register failed for GPU BAR ptr {:?}, size {}: rc={}",
            dev_ptr, size, rc
        ));
    }

    // Create DmaBuffer with a free function that unregisters from SPDK.
    // The GPU memory itself is NOT freed here — it's owned by the caller.
    let buf = unsafe { DmaBuffer::from_raw(dev_ptr, size, gpu_bar_dma_free, -1) }
        .map_err(|e| {
            // Cleanup on failure.
            unsafe { spdk_mem_unregister(dev_ptr, size) };
            format!("DmaBuffer::from_raw for GPU BAR failed: {e}")
        })?;

    Ok(buf)
}

/// Free function for GPU BAR DMA buffers: unregisters from SPDK but does NOT
/// free the GPU memory (which is owned by the GPU allocation layer).
#[cfg(feature = "p2p")]
unsafe extern "C" fn gpu_bar_dma_free(ptr: *mut std::ffi::c_void) {
    if !ptr.is_null() {
        // Best-effort unregister; ignore errors during cleanup.
        // Note: we don't have the size here, so we pass 0 which SPDK
        // interprets as "unregister the region starting at this address".
        let _ = spdk_mem_unregister(ptr, 0);
    }
}

// SPDK memory registration FFI bindings.
extern "C" {
    /// Register a memory region with SPDK for DMA access.
    /// For GPU BAR1 memory, nvidia-peermem translates the address.
    #[cfg(feature = "p2p")]
    fn spdk_mem_register(
        vaddr: *mut std::ffi::c_void,
        len: usize,
    ) -> std::os::raw::c_int;

    /// Unregister a previously registered memory region.
    #[cfg(feature = "p2p")]
    fn spdk_mem_unregister(
        vaddr: *mut std::ffi::c_void,
        len: usize,
    ) -> std::os::raw::c_int;
}

/// Create a standard CUDA-pinned, SPDK-registered DMA buffer on the host.
/// This is the non-P2P fallback path.
pub fn create_host_pinned_dma_buffer(size: usize) -> Result<DmaBuffer, String> {
    if size == 0 {
        return Err("Buffer size must be non-zero".to_string());
    }

    // Allocate CUDA pinned memory.
    let mut ptr: *mut std::ffi::c_void = std::ptr::null_mut();
    let cuda_flags: std::os::raw::c_int = 0; // cudaHostAllocDefault
    let rc = unsafe { cuda_host_alloc(&mut ptr, size, cuda_flags) };
    if rc != 0 {
        return Err(format!("cudaHostAlloc failed: rc={}", rc));
    }

    // Register with SPDK for DMA access.
    let spdk_rc = unsafe { spdk_mem_register_host(ptr, size) };
    if spdk_rc != 0 {
        unsafe { cuda_free_host(ptr) };
        return Err(format!(
            "spdk_mem_register failed for pinned host memory: rc={}",
            spdk_rc
        ));
    }

    let buf = unsafe { DmaBuffer::from_raw(ptr, size, host_pinned_dma_free, -1) }.map_err(|e| {
        unsafe {
            spdk_mem_unregister_host(ptr);
            cuda_free_host(ptr);
        }
        format!("DmaBuffer::from_raw for host pinned failed: {e}")
    })?;

    Ok(buf)
}

/// Free function for host-pinned DMA buffers.
unsafe extern "C" fn host_pinned_dma_free(ptr: *mut std::ffi::c_void) {
    if !ptr.is_null() {
        let _ = spdk_mem_unregister_host(ptr);
        cuda_free_host(ptr);
    }
}

// Host memory FFI bindings (non-P2P path).
extern "C" {
    fn spdk_mem_register_host(
        vaddr: *mut std::ffi::c_void,
        len: usize,
    ) -> std::os::raw::c_int;
    fn spdk_mem_unregister_host(vaddr: *mut std::ffi::c_void) -> std::os::raw::c_int;
    fn cuda_host_alloc(
        ptr: *mut *mut std::ffi::c_void,
        size: usize,
        flags: std::os::raw::c_int,
    ) -> std::os::raw::c_int;
    fn cuda_free_host(ptr: *mut std::ffi::c_void);
}