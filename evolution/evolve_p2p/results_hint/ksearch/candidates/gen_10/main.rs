// --- FILE: pipeline.rs ---
//! Ring-buffer pipelined reader for SSD→GPU transfers.
//!
//! Supports three modes:
//! 1. Standard: SSD → pinned host ring → memcpy to mem-tier + async H2D to GPU
//! 2. Zero-copy: SSD → registered mem-tier → async H2D to GPU
//! 3. P2P (GPUDirect Storage): SSD → GPU BAR1 memory directly (no host bounce)

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

/// Maximum NVMe queue depth for saturating Gen4 drives.
const MAX_NVME_QUEUE_DEPTH: usize = 64;

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

    let mut chunk_idx = 0;

    while chunk_idx < num_chunks {
        let batch_end = (chunk_idx + ring_size).min(num_chunks);
        let batch_len = batch_end - chunk_idx;

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

        for i in 0..batch_len {
            let seg = &segments[chunk_idx + i];
            let copy_len = seg.length.min(total_bytes.saturating_sub(seg.buffer_offset));
            let current_stream = streams[i % 2];

            let guard = ring.buffers[i].lock().unwrap();

            if copy_len > 0 {
                unsafe {
                    std::ptr::copy_nonoverlapping(
                        guard.as_ptr() as *const u8,
                        mem_tier_ptr.add(seg.buffer_offset),
                        copy_len,
                    );
                }
            }

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

        gpu.stream_synchronize(streams[0])
            .map_err(|e| DispatcherError::IoError(format!("stream_synchronize failed: {e}")))?;
        gpu.stream_synchronize(streams[1])
            .map_err(|e| DispatcherError::IoError(format!("stream_synchronize failed: {e}")))?;

        chunk_idx = batch_end;
    }

    for s in streams {
        gpu.stream_synchronize(*s)
            .map_err(|e| DispatcherError::IoError(format!("final stream_synchronize: {e}")))?;
    }

    Ok(())
}

/// No-op free function for DmaBuffer wrappers over memory-tier regions.
unsafe extern "C" fn noop_free(_ptr: *mut std::ffi::c_void) {}

/// Zero-copy pipeline: read from SSD directly into a memory-tier slot, stream to GPU.
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

    // Cap effective QD at both the caller's limit and hardware max, then at num_chunks.
    let effective_qd = max_queue_depth.min(MAX_NVME_QUEUE_DEPTH).min(num_chunks);

    let mut inflight: std::collections::VecDeque<usize> =
        std::collections::VecDeque::with_capacity(effective_qd);

    // Prime the sliding window.
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

        // Issue async H2D copy for the completed segment.
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

    // Forget DmaBuffer wrappers (noop_free, memory owned by mem-tier).
    for buf in chunk_bufs {
        std::mem::forget(Arc::try_unwrap(buf).ok());
    }

    Ok(())
}

/// P2P pipeline: read from SSD directly into GPU BAR1 memory, bypassing host DRAM.
///
/// Uses GPUDirect Storage: the NVMe controller DMAs directly into GPU memory
/// via PCIe peer-to-peer, eliminating the host DRAM bounce buffer entirely.
/// After all reads complete, optionally copies data back to mem_tier for serving.
///
/// Uses QD64 sliding window to saturate the NVMe controller.
#[cfg(feature = "p2p")]
#[allow(clippy::too_many_arguments)]
pub unsafe fn pipelined_ssd_to_gpu_p2p(
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

    // Register the GPU destination region with SPDK for P2P DMA.
    let register_size = aligned_bytes.next_multiple_of(block_size);
    gpu_services::dma::register_gpu_bar_region(gpu_dst, register_size)
        .map_err(|e| DispatcherError::AllocationFailed(format!("GPU BAR register: {e}")))?;

    // Create lightweight DmaBuffer wrappers per segment pointing into GPU memory.
    let chunk_bufs: Vec<Arc<Mutex<DmaBuffer>>> = segments
        .iter()
        .map(|seg| {
            let ptr = unsafe { (gpu_dst as *mut u8).add(seg.buffer_offset) as *mut std::ffi::c_void };
            let buf_size = seg.length.next_multiple_of(block_size);
            let buf = unsafe { DmaBuffer::from_raw(ptr, buf_size, noop_free, -1) }
                .map_err(|e| {
                    DispatcherError::AllocationFailed(format!("DmaBuffer wrap GPU chunk: {e}"))
                })?;
            Ok(Arc::new(Mutex::new(buf)))
        })
        .collect::<Result<Vec<_>, DispatcherError>>()?;

    // Use full QD64 to saturate the NVMe controller at Gen4 speeds.
    let effective_qd = MAX_NVME_QUEUE_DEPTH.min(num_chunks);

    let mut inflight: std::collections::VecDeque<usize> =
        std::collections::VecDeque::with_capacity(effective_qd);

    // Prime the sliding window with QD64 in-flight reads.
    for i in 0..effective_qd {
        channels
            .command_tx
            .send(Command::ReadAsync {
                ns_id: 1,
                lba: segments[i].lba,
                buf: Arc::clone(&chunk_bufs[i]),
                timeout_ms: READ_TIMEOUT_MS,
            })
            .map_err(|e| DispatcherError::IoError(format!("ReadAsync P2P send #{i}: {e}")))?;
        inflight.push_back(i);
    }

    let mut next_to_submit = effective_qd;

    // Sliding window: as each NVMe read completes, submit the next.
    // No GPU H2D copy needed — data lands in GPU memory via P2P DMA.
    for _completed in 0..num_chunks {
        let _seg_idx = match channels.completion_rx.recv() {
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

        // Submit next read immediately to keep the pipeline full.
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
                        "ReadAsync P2P submit #{next_to_submit}: {e}"
                    ))
                })?;
            inflight.push_back(next_to_submit);
            next_to_submit += 1;
        }
    }

    // All P2P reads complete. Data is now in GPU memory.
    // Copy GPU → host mem-tier for serving (D2H async copy).
    if !mem_tier_ptr.is_null() && total_bytes > 0 {
        // Use D2H copy via stream to populate mem_tier for CPU serving.
        gpu.dma_copy_from_device_async(
            gpu_dst,
            mem_tier_ptr as *mut std::ffi::c_void,
            total_bytes,
            streams[0],
        )
        .map_err(|e| {
            DispatcherError::IoError(format!("P2P D2H copy to mem_tier failed: {e}"))
        })?;
        gpu.stream_synchronize(streams[0])
            .map_err(|e| DispatcherError::IoError(format!("P2P D2H sync failed: {e}")))?;
    }

    // Forget DmaBuffer wrappers (noop_free, memory owned by GPU allocation).
    for buf in chunk_bufs {
        std::mem::forget(Arc::try_unwrap(buf).ok());
    }

    // Unregister the GPU region from SPDK.
    gpu_services::dma::unregister_gpu_bar_region(gpu_dst, register_size);

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
//! Provides allocation routines for:
//! - Standard CUDA-pinned + SPDK-registered host buffers
//! - GPU BAR1 memory registration for GPUDirect Storage P2P

use interfaces::DmaBuffer;

/// Register a GPU BAR1 memory region with SPDK for direct NVMe P2P DMA.
///
/// After registration, NVMe controllers can DMA directly into this GPU memory
/// region without involving host DRAM. Call once for a contiguous GPU allocation,
/// then create lightweight DmaBuffer wrappers for sub-regions.
///
/// # Safety
/// `dev_ptr` must be a valid CUDA device pointer with at least `size` bytes allocated.
#[cfg(feature = "p2p")]
pub fn register_gpu_bar_region(
    dev_ptr: *mut std::ffi::c_void,
    size: usize,
) -> Result<(), String> {
    if dev_ptr.is_null() {
        return Err("GPU device pointer is null".to_string());
    }
    if size == 0 {
        return Err("Buffer size must be non-zero".to_string());
    }

    let rc = unsafe { spdk_mem_register(dev_ptr, size) };
    if rc != 0 {
        return Err(format!(
            "spdk_mem_register failed for GPU BAR ptr {:?}, size {}: rc={}",
            dev_ptr, size, rc
        ));
    }
    Ok(())
}

/// Unregister a GPU BAR1 memory region from SPDK.
///
/// Must be called with the same pointer and size used in `register_gpu_bar_region`.
#[cfg(feature = "p2p")]
pub fn unregister_gpu_bar_region(dev_ptr: *mut std::ffi::c_void, size: usize) {
    if !dev_ptr.is_null() && size > 0 {
        unsafe { spdk_mem_unregister(dev_ptr, size) };
    }
}

/// Create an SPDK-registered DMA buffer backed by GPU BAR1 memory.
///
/// This enables GPUDirect Storage P2P: NVMe controllers can DMA directly
/// into this GPU memory region without involving host DRAM.
///
/// The returned DmaBuffer owns the SPDK registration and will unregister on drop.
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

    let rc = unsafe { spdk_mem_register(dev_ptr, size) };
    if rc != 0 {
        return Err(format!(
            "spdk_mem_register failed for GPU BAR ptr {:?}, size {}: rc={}",
            dev_ptr, size, rc
        ));
    }

    let buf = unsafe { DmaBuffer::from_raw(dev_ptr, size, gpu_bar_dma_free, -1) }
        .map_err(|e| {
            unsafe { spdk_mem_unregister(dev_ptr, size) };
            format!("DmaBuffer::from_raw for GPU BAR failed: {e}")
        })?;

    Ok(buf)
}

/// Free function for GPU BAR DMA buffers: unregisters from SPDK.
/// Does NOT free the GPU memory itself (owned by the CUDA allocator).
#[cfg(feature = "p2p")]
unsafe extern "C" fn gpu_bar_dma_free(ptr: *mut std::ffi::c_void) {
    if !ptr.is_null() {
        // Best-effort unregister; size=0 means SPDK uses internal tracking.
        let _ = spdk_mem_unregister(ptr, 0);
    }
}

#[cfg(feature = "p2p")]
extern "C" {
    fn spdk_mem_register(
        vaddr: *mut std::ffi::c_void,
        len: usize,
    ) -> std::os::raw::c_int;

    fn spdk_mem_unregister(
        vaddr: *mut std::ffi::c_void,
        len: usize,
    ) -> std::os::raw::c_int;
}

/// Create a standard CUDA-pinned, SPDK-registered DMA buffer on the host.
///
/// Used for the traditional SSD → host DRAM → GPU path.
pub fn create_host_pinned_dma_buffer(size: usize) -> Result<DmaBuffer, String> {
    if size == 0 {
        return Err("Buffer size must be non-zero".to_string());
    }

    let mut ptr: *mut std::ffi::c_void = std::ptr::null_mut();
    let cuda_flags: std::os::raw::c_int = 0;
    let rc = unsafe { cuda_host_alloc(&mut ptr, size, cuda_flags) };
    if rc != 0 {
        return Err(format!("cudaHostAlloc failed: rc={}", rc));
    }

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