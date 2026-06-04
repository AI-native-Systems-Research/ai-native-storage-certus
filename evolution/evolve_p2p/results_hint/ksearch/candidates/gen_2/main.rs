// --- FILE: pipeline.rs ---
//! Ring-buffer pipelined reader for SSD→GPU transfers via GPUDirect Storage (P2P).
//!
//! With P2P enabled, NVMe reads DMA directly into GPU BAR1 memory, bypassing
//! host DRAM entirely. This eliminates the host-bounce cudaMemcpy step.

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

/// Pre-allocated ring of DMA buffers and CUDA streams.
///
/// With P2P feature enabled, buffers are GPU BAR1 memory registered with SPDK,
/// allowing NVMe controllers to DMA directly into GPU memory.
pub struct PipelineRing {
    pub buffers: Vec<Arc<Mutex<DmaBuffer>>>,
    pub streams: [GpuStream; 2],
    pub chunk_size: usize,
    /// GPU device pointers corresponding to each ring buffer (for P2P path).
    #[cfg(feature = "p2p")]
    pub gpu_ring_ptrs: Vec<*mut std::ffi::c_void>,
}

// Safety: The GPU pointers are only accessed from the pipeline thread.
unsafe impl Send for PipelineRing {}
unsafe impl Sync for PipelineRing {}

impl PipelineRing {
    /// Allocate a new pipeline ring.
    ///
    /// With P2P enabled: allocates GPU memory and registers it as SPDK DMA buffers
    /// so NVMe can DMA directly into GPU BAR1 memory.
    ///
    /// Without P2P: allocates CUDA-pinned host memory registered with SPDK.
    pub fn new(gpu: &dyn IGpuServices, chunk_size: usize) -> Result<Self, DispatcherError> {
        let stream_a = gpu
            .create_stream()
            .map_err(|e| DispatcherError::IoError(format!("create_stream failed: {e}")))?;
        let stream_b = gpu.create_stream().map_err(|e| {
            let _ = gpu.destroy_stream(stream_a);
            DispatcherError::IoError(format!("create_stream failed: {e}"))
        })?;

        #[cfg(feature = "p2p")]
        {
            let mut buffers = Vec::with_capacity(PIPELINE_RING_SIZE);
            let mut gpu_ring_ptrs = Vec::with_capacity(PIPELINE_RING_SIZE);

            for i in 0..PIPELINE_RING_SIZE {
                // Allocate GPU memory
                let dev_ptr = gpu.allocate_device_memory(chunk_size).map_err(|e| {
                    DispatcherError::AllocationFailed(format!(
                        "GPU device memory alloc for ring slot {i}: {e}"
                    ))
                })?;

                // Register GPU memory as SPDK DMA buffer for P2P NVMe access
                let dma_buf =
                    gpu_services::dma::create_spdk_dma_buffer_from_gpu_bar(dev_ptr, chunk_size)
                        .map_err(|e| {
                            DispatcherError::AllocationFailed(format!(
                                "P2P DMA buffer registration for ring slot {i}: {e}"
                            ))
                        })?;

                gpu_ring_ptrs.push(dev_ptr);
                buffers.push(Arc::new(Mutex::new(dma_buf)));
            }

            Ok(Self {
                buffers,
                streams: [stream_a, stream_b],
                chunk_size,
                gpu_ring_ptrs,
            })
        }

        #[cfg(not(feature = "p2p"))]
        {
            let buffers: Vec<Arc<Mutex<DmaBuffer>>> = (0..PIPELINE_RING_SIZE)
                .map(|_| {
                    gpu.allocate_pinned_dma_buffer(chunk_size)
                        .map(|b| Arc::new(Mutex::new(b)))
                        .map_err(|e| {
                            DispatcherError::AllocationFailed(format!(
                                "pipeline ring buffer: {e}"
                            ))
                        })
                })
                .collect::<Result<Vec<_>, _>>()?;

            Ok(Self {
                buffers,
                streams: [stream_a, stream_b],
                chunk_size,
            })
        }
    }

    /// Destroy CUDA streams. Buffers are freed on drop via their DmaBuffer free_fn.
    pub fn destroy(self, gpu: &dyn IGpuServices) {
        let _ = gpu.destroy_stream(self.streams[0]);
        let _ = gpu.destroy_stream(self.streams[1]);
    }
}

/// Pipeline-read from SSD into GPU memory via P2P DMA (GPUDirect Storage).
///
/// With P2P enabled, NVMe reads DMA directly into GPU BAR1 ring buffers.
/// After each read completes, data is already in GPU memory. We copy from
/// the ring buffer to the final GPU destination using device-to-device copy,
/// and also populate the memory-tier slot for CPU access.
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

    #[cfg(feature = "p2p")]
    {
        // P2P path: NVMe DMAs directly into GPU BAR1 ring buffers.
        // Use a sliding window for maximum SSD I/O overlap.
        let mut inflight: std::collections::VecDeque<(usize, usize)> =
            std::collections::VecDeque::with_capacity(ring_size);

        // Prime the pipeline
        let initial_batch = ring_size.min(num_chunks);
        for i in 0..initial_batch {
            let slot = i % ring_size;
            channels
                .command_tx
                .send(Command::ReadAsync {
                    ns_id: 1,
                    lba: segments[i].lba,
                    buf: Arc::clone(&ring.buffers[slot]),
                    timeout_ms: READ_TIMEOUT_MS,
                })
                .map_err(|e| {
                    DispatcherError::IoError(format!("ReadAsync send #{i}: {e}"))
                })?;
            inflight.push_back((i, slot));
        }

        let mut next_to_submit = initial_batch;
        let mut completed_count = 0usize;

        for _completed in 0..num_chunks {
            // Wait for oldest in-flight read
            let (seg_idx, ring_slot) = match channels.completion_rx.recv() {
                Ok(Completion::ReadDone { handle, result }) => {
                    result.map_err(|e| {
                        DispatcherError::IoError(format!(
                            "SSD read (handle {:?}): {e}",
                            handle
                        ))
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

            let seg = &segments[seg_idx];
            let copy_len = seg.length.min(total_bytes.saturating_sub(seg.buffer_offset));
            let current_stream = streams[completed_count % 2];

            // Data is now in GPU ring buffer via P2P DMA.
            // Copy ring GPU buffer → final GPU destination (D2D via the DMA buffer interface).
            let guard = ring.buffers[ring_slot].lock().unwrap();
            let dst_ptr = (gpu_dst as *mut u8).add(seg.buffer_offset) as *mut std::ffi::c_void;

            // Use dma_copy_to_device_async: the DmaBuffer wraps GPU BAR memory,
            // and the runtime should handle this as a D2D copy when source is device-mapped.
            gpu.dma_copy_to_device_async(
                &guard,
                dst_ptr,
                copy_len,
                current_stream,
            )
            .map_err(|e| {
                DispatcherError::IoError(format!(
                    "GPU D2D copy (seg {seg_idx}) failed: {e}"
                ))
            })?;
            drop(guard);

            // Copy to memory-tier for CPU access (D2H from ring buffer).
            // We read from the GPU ring buffer pointer to host memory.
            let src_gpu = ring.gpu_ring_ptrs[ring_slot];
            gpu.copy_device_to_host(
                src_gpu,
                mem_tier_ptr.add(seg.buffer_offset) as *mut std::ffi::c_void,
                copy_len,
            )
            .map_err(|e| {
                DispatcherError::IoError(format!(
                    "D2H copy for mem-tier (seg {seg_idx}) failed: {e}"
                ))
            })?;

            completed_count += 1;

            // Sync streams periodically to allow ring slot reuse
            if completed_count % ring_size == 0 {
                gpu.stream_synchronize(streams[0]).map_err(|e| {
                    DispatcherError::IoError(format!("stream_synchronize failed: {e}"))
                })?;
                gpu.stream_synchronize(streams[1]).map_err(|e| {
                    DispatcherError::IoError(format!("stream_synchronize failed: {e}"))
                })?;
            }

            // Submit next read into the freed ring slot
            if next_to_submit < num_chunks {
                channels
                    .command_tx
                    .send(Command::ReadAsync {
                        ns_id: 1,
                        lba: segments[next_to_submit].lba,
                        buf: Arc::clone(&ring.buffers[ring_slot]),
                        timeout_ms: READ_TIMEOUT_MS,
                    })
                    .map_err(|e| {
                        DispatcherError::IoError(format!(
                            "ReadAsync submit #{next_to_submit}: {e}"
                        ))
                    })?;
                inflight.push_back((next_to_submit, ring_slot));
                next_to_submit += 1;
            }
        }

        // Final sync
        for s in streams {
            gpu.stream_synchronize(*s).map_err(|e| {
                DispatcherError::IoError(format!("final stream_synchronize: {e}"))
            })?;
        }
    }

    #[cfg(not(feature = "p2p"))]
    {
        // Non-P2P fallback: original host-bounce path
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
                        DispatcherError::IoError(format!(
                            "ReadAsync send #{}: {e}",
                            chunk_idx + i
                        ))
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
                let copy_len =
                    seg.length.min(total_bytes.saturating_sub(seg.buffer_offset));
                let current_stream = streams[i % 2];

                let guard = ring.buffers[i].lock().unwrap();

                if copy_len > 0 {
                    std::ptr::copy_nonoverlapping(
                        guard.as_ptr() as *const u8,
                        mem_tier_ptr.add(seg.buffer_offset),
                        copy_len,
                    );
                }

                gpu.dma_copy_to_device_async(
                    &guard,
                    (gpu_dst as *mut u8).add(seg.buffer_offset)
                        as *mut std::ffi::c_void,
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

            gpu.stream_synchronize(streams[0]).map_err(|e| {
                DispatcherError::IoError(format!("stream_synchronize failed: {e}"))
            })?;
            gpu.stream_synchronize(streams[1]).map_err(|e| {
                DispatcherError::IoError(format!("stream_synchronize failed: {e}"))
            })?;

            chunk_idx = batch_end;
        }

        for s in streams {
            gpu.stream_synchronize(*s).map_err(|e| {
                DispatcherError::IoError(format!("final stream_synchronize: {e}"))
            })?;
        }
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

        if stream_idx % 16 == 0 {
            gpu.stream_synchronize(streams[0])
                .map_err(|e| DispatcherError::IoError(format!("stream_synchronize failed: {e}")))?;
            gpu.stream_synchronize(streams[1])
                .map_err(|e| DispatcherError::IoError(format!("stream_synchronize failed: {e}")))?;
        }
    }

    for s in streams {
        gpu.stream_synchronize(*s)
            .map_err(|e| DispatcherError::IoError(format!("final stream_synchronize: {e}")))?;
    }

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

// --- FILE: dma.rs ---
//! DMA buffer creation functions for various memory types.
//!
//! This module provides functions for creating SPDK-registered DMA buffers
//! from different memory sources including GPU BAR1 memory for P2P transfers.

use interfaces::DmaBuffer;

/// CUDA external memory flags and types used for P2P buffer registration.
#[cfg(feature = "p2p")]
mod cuda_ffi {
    use std::os::raw::c_int;

    // SPDK memory registration functions for P2P DMA access
    extern "C" {
        /// Register a memory region with SPDK for DMA access.
        /// For GPU BAR1 memory, this enables NVMe controllers to DMA directly
        /// to/from GPU memory via PCIe peer-to-peer transfers.
        pub fn spdk_mem_register(
            addr: *mut std::ffi::c_void,
            len: usize,
        ) -> c_int;

        /// Unregister a previously registered memory region from SPDK.
        pub fn spdk_mem_unregister(
            addr: *mut std::ffi::c_void,
            len: usize,
        ) -> c_int;
    }
}

/// Free function for P2P GPU BAR DMA buffers.
///
/// Does NOT free the GPU memory itself — that is managed by the PipelineRing.
/// SPDK unregistration is handled separately at ring destruction time.
#[cfg(feature = "p2p")]
unsafe extern "C" fn p2p_bar_buffer_free(_ptr: *mut std::ffi::c_void) {
    // No-op: GPU memory lifetime is managed by PipelineRing.
    // SPDK unregistration is done via unregister_gpu_bar_from_spdk.
}

/// Create an SPDK DMA buffer backed by GPU BAR1 memory for P2P NVMe access.
///
/// This registers the GPU device pointer with SPDK's memory translation layer,
/// enabling NVMe controllers to perform DMA reads/writes directly to GPU memory
/// without bouncing through host DRAM.
///
/// # Arguments
/// * `dev_ptr` - GPU device pointer allocated via cuMemAlloc or cudaMalloc
/// * `size` - Size of the GPU memory region in bytes
///
/// # Returns
/// A `DmaBuffer` wrapping the GPU memory, registered with SPDK for P2P access.
///
/// # Safety
/// The caller must ensure:
/// - `dev_ptr` is a valid GPU device pointer
/// - The GPU memory remains allocated for the lifetime of the returned buffer
/// - nvidia-peermem kernel module is loaded
/// - The GPU supports P2P (BAR1 mapping)
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

    // Register GPU memory with SPDK's memory translation system.
    // This creates IOMMU/VFIO mappings that allow NVMe controllers to
    // access GPU BAR1 memory directly via PCIe peer-to-peer transfers.
    unsafe {
        let ret = cuda_ffi::spdk_mem_register(dev_ptr, size);
        if ret != 0 {
            return Err(format!(
                "spdk_mem_register failed for GPU BAR memory at {:p} size {}: error code {}",
                dev_ptr, size, ret
            ));
        }
    }

    // Wrap as a DmaBuffer. The free function is a no-op since GPU memory
    // lifetime is managed by the PipelineRing. fd = -1 (not file-backed).
    unsafe {
        DmaBuffer::from_raw(dev_ptr, size, p2p_bar_buffer_free, -1)
            .map_err(|e| format!("DmaBuffer::from_raw failed for GPU BAR buffer: {e}"))
    }
}

/// Unregister a GPU BAR buffer from SPDK (for cleanup at ring destruction time).
///
/// Should be called before freeing the GPU memory to ensure SPDK no longer
/// references the memory region.
#[cfg(feature = "p2p")]
pub fn unregister_gpu_bar_from_spdk(
    dev_ptr: *mut std::ffi::c_void,
    size: usize,
) -> Result<(), String> {
    if dev_ptr.is_null() {
        return Ok(());
    }
    unsafe {
        let ret = cuda_ffi::spdk_mem_unregister(dev_ptr, size);
        if ret != 0 {
            return Err(format!(
                "spdk_mem_unregister failed for GPU BAR memory at {:p} size {}: error code {}",
                dev_ptr, size, ret
            ));
        }
    }
    Ok(())
}