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

    // Sliding-window pipeline: as each NVMe read completes, immediately issue
    // the GPU H2D copy for that segment and submit the next read.  This overlaps
    // SSD I/O with GPU DMA instead of serialising them in two phases.
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
//! DMA buffer creation from GPU IPC handles.

#[cfg(feature = "gpu")]
use crate::cuda_ffi;

#[cfg(feature = "gpu")]
use interfaces::{GpuDmaBuffer, GpuIpcHandle};

// SPDK functions used by the general GPU+SPDK DMA path.
#[cfg(all(feature = "gpu", feature = "spdk"))]
extern "C" {
    fn spdk_mem_register(vaddr: *mut std::ffi::c_void, len: usize) -> std::os::raw::c_int;
    fn spdk_mem_unregister(vaddr: *mut std::ffi::c_void, len: usize) -> std::os::raw::c_int;
}

// DPDK/VFIO functions only used by the P2P (GDRCopy) path.
#[cfg(feature = "p2p")]
extern "C" {
    fn rte_extmem_register(
        va_addr: *mut std::ffi::c_void,
        len: usize,
        iova_addrs: *const u64,
        n_pages: std::os::raw::c_uint,
        page_sz: usize,
    ) -> std::os::raw::c_int;
    fn rte_extmem_unregister(va_addr: *mut std::ffi::c_void, len: usize) -> std::os::raw::c_int;
    fn rte_vfio_container_dma_map(
        container_fd: std::os::raw::c_int,
        vaddr: u64,
        iova: u64,
        len: u64,
    ) -> std::os::raw::c_int;
    fn rte_vfio_container_dma_unmap(
        container_fd: std::os::raw::c_int,
        vaddr: u64,
        iova: u64,
        len: u64,
    ) -> std::os::raw::c_int;
    fn spdk_vtophys(buf: *const std::ffi::c_void, size: *mut u64) -> u64;
}

/// Free function for GpuDmaBuffer that closes the CUDA IPC handle.
#[cfg(feature = "gpu")]
unsafe extern "C" fn cuda_ipc_close_mem_handle(ptr: *mut std::ffi::c_void) {
    // SAFETY: ptr was obtained from cudaIpcOpenMemHandle and has not been closed.
    unsafe {
        cuda_ffi::cudaIpcCloseMemHandle(ptr);
    }
}

/// Tracks registered GPU memory regions so free functions can look up sizes.
/// DmaBuffer's free_fn signature is `fn(*mut c_void)` — it doesn't receive
/// the size, so we store it here at registration time.
#[cfg(all(feature = "gpu", feature = "spdk"))]
static REGISTERED_REGIONS: std::sync::OnceLock<
    std::sync::Mutex<std::collections::HashMap<usize, usize>>,
> = std::sync::OnceLock::new();

#[cfg(all(feature = "gpu", feature = "spdk"))]
fn registered_regions() -> &'static std::sync::Mutex<std::collections::HashMap<usize, usize>> {
    REGISTERED_REGIONS.get_or_init(|| std::sync::Mutex::new(std::collections::HashMap::new()))
}

/// Free function that unregisters from SPDK, then closes the CUDA IPC handle.
///
/// Used when the memory was already pinned before `prepare_memory_for_spdk`
/// was called — we must not unpin memory that belongs to the originating process,
/// but we still need to unregister from SPDK.
#[cfg(all(feature = "gpu", feature = "spdk"))]
pub unsafe extern "C" fn spdk_unregister_and_ipc_close(ptr: *mut std::ffi::c_void) {
    unsafe {
        let size = registered_regions()
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .remove(&(ptr as usize))
            .unwrap_or(0);
        if size > 0 {
            spdk_mem_unregister(ptr, size);
        }
        cuda_ffi::cudaIpcCloseMemHandle(ptr);
    }
}

/// Free function that unregisters from SPDK, unpins memory, then closes IPC handle.
///
/// Used when `prepare_memory_for_spdk` itself pinned the memory — on drop
/// we must unregister from SPDK, undo the pin, then close the handle.
#[cfg(all(feature = "gpu", feature = "spdk"))]
pub unsafe extern "C" fn spdk_unregister_unpin_and_ipc_close(ptr: *mut std::ffi::c_void) {
    unsafe {
        let size = registered_regions()
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .remove(&(ptr as usize))
            .unwrap_or(0);
        if size > 0 {
            spdk_mem_unregister(ptr, size);
        }
        cuda_ffi::cudaHostUnregister(ptr);
        cuda_ffi::cudaIpcCloseMemHandle(ptr);
    }
}

/// Create an SPDK `DmaBuffer` from a GPU device pointer with the appropriate
/// free function based on whether the memory was already pinned.
///
/// Registers the GPU memory with SPDK (`spdk_mem_register`) so that SPDK's
/// vtophys translation works for DMA operations. Requires nvidia-peermem
/// kernel module for GPU BAR memory to be IOMMU-accessible.
///
/// * `was_already_pinned = true` → uses close-only free (no unpin on drop)
/// * `was_already_pinned = false` → uses unpin + close free (undo pin on drop)
#[cfg(all(feature = "gpu", feature = "spdk"))]
pub fn create_spdk_dma_buffer_from_gpu(
    ptr: *mut std::ffi::c_void,
    size: usize,
    was_already_pinned: bool,
) -> Result<interfaces::DmaBuffer, String> {
    // Register GPU memory with SPDK so vtophys can resolve the physical address.
    // This requires nvidia-peermem to be loaded for GPU device memory.
    // SAFETY: ptr is a valid device pointer of `size` bytes from cudaIpcOpenMemHandle.
    let rc = unsafe { spdk_mem_register(ptr, size) };
    if rc != 0 {
        return Err(format!(
            "spdk_mem_register failed (rc={}). Is nvidia-peermem loaded?",
            rc
        ));
    }

    // Record size so the free function can unregister the correct range.
    registered_regions()
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .insert(ptr as usize, size);

    let free_fn: unsafe extern "C" fn(*mut std::ffi::c_void) = if was_already_pinned {
        spdk_unregister_and_ipc_close
    } else {
        spdk_unregister_unpin_and_ipc_close
    };

    // SAFETY: ptr is a valid GPU device pointer obtained from cudaIpcOpenMemHandle,
    // now registered with SPDK for DMA. size is the correct allocation size.
    // free_fn handles SPDK unregister + cleanup based on pin state.
    // numa_node = -1 because GPU device memory has no CPU NUMA affinity.
    let result = unsafe {
        interfaces::DmaBuffer::from_raw(ptr, size, free_fn, -1)
            .map_err(|e| format!("DmaBuffer creation failed: {}", e))
    };

    if result.is_err() {
        registered_regions()
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .remove(&(ptr as usize));
        unsafe {
            spdk_mem_unregister(ptr, size);
        }
    }

    result
}

/// Free function that unregisters from SPDK then frees via cudaFree.
///
/// Used for directly-allocated GPU memory (cudaMalloc) rather than
/// IPC-opened handles.
#[cfg(all(feature = "gpu", feature = "spdk"))]
pub unsafe extern "C" fn spdk_unregister_and_cuda_free(ptr: *mut std::ffi::c_void) {
    unsafe {
        let size = registered_regions()
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .remove(&(ptr as usize))
            .unwrap_or(0);
        if size > 0 {
            spdk_mem_unregister(ptr, size);
        }
        cuda_ffi::cudaFree(ptr);
    }
}

/// Create an SPDK `DmaBuffer` from a `cudaMalloc`-allocated GPU pointer.
///
/// Registers the GPU memory with SPDK (`spdk_mem_register`) for DMA via
/// nvidia-peermem and returns a `DmaBuffer` that will unregister and
/// `cudaFree` on drop.
#[cfg(all(feature = "gpu", feature = "spdk"))]
pub fn create_spdk_dma_buffer_from_cuda_malloc(
    ptr: *mut std::ffi::c_void,
    size: usize,
) -> Result<interfaces::DmaBuffer, String> {
    // SAFETY: ptr is a valid device pointer of `size` bytes from cudaMalloc.
    let rc = unsafe { spdk_mem_register(ptr, size) };
    if rc != 0 {
        return Err(format!(
            "spdk_mem_register failed (rc={}). Is nvidia-peermem loaded?",
            rc
        ));
    }

    registered_regions()
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .insert(ptr as usize, size);

    // SAFETY: ptr is a valid GPU device pointer from cudaMalloc, now registered
    // with SPDK for DMA. free_fn handles SPDK unregister + cudaFree.
    // numa_node = -1 because GPU device memory has no CPU NUMA affinity.
    let result = unsafe {
        interfaces::DmaBuffer::from_raw(ptr, size, spdk_unregister_and_cuda_free, -1)
            .map_err(|e| format!("DmaBuffer creation failed: {}", e))
    };

    if result.is_err() {
        registered_regions()
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .remove(&(ptr as usize));
        unsafe {
            spdk_mem_unregister(ptr, size);
        }
    }

    result
}

/// Free function that unregisters from SPDK then frees via cudaFreeHost.
///
/// Used for pinned host memory allocated with `cudaHostAlloc`.
#[cfg(all(feature = "gpu", feature = "spdk"))]
pub unsafe extern "C" fn spdk_unregister_and_cuda_free_host(ptr: *mut std::ffi::c_void) {
    unsafe {
        let size = registered_regions()
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .remove(&(ptr as usize))
            .unwrap_or(0);
        if size > 0 {
            spdk_mem_unregister(ptr, size);
        }
        cuda_ffi::cudaFreeHost(ptr);
    }
}

/// Create an SPDK `DmaBuffer` from a `cudaHostAlloc`-allocated pinned host pointer.
///
/// This memory is accessible by both CPU and GPU (via `cudaHostGetDevicePointer`).
/// NVMe can DMA directly into it since it's pinned host memory with valid
/// physical addresses. The GPU accesses it via P2P mapping managed by the
/// nvidia driver (nvidia-peermem).
#[cfg(all(feature = "gpu", feature = "spdk"))]
pub fn create_spdk_dma_buffer_from_cuda_host_alloc(
    ptr: *mut std::ffi::c_void,
    size: usize,
) -> Result<interfaces::DmaBuffer, String> {
    // SAFETY: ptr is a valid pinned host pointer from cudaHostAlloc.
    // It has valid physical addresses resolvable by SPDK vtophys.
    let rc = unsafe { spdk_mem_register(ptr, size) };
    if rc != 0 {
        return Err(format!("spdk_mem_register failed (rc={})", rc));
    }

    registered_regions()
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .insert(ptr as usize, size);

    // SAFETY: ptr is a valid pinned host pointer, registered with SPDK.
    // free_fn handles SPDK unregister + cudaFreeHost.
    // numa_node = -1 (CUDA manages placement).
    let result = unsafe {
        interfaces::DmaBuffer::from_raw(ptr, size, spdk_unregister_and_cuda_free_host, -1)
            .map_err(|e| format!("DmaBuffer creation failed: {}", e))
    };

    if result.is_err() {
        registered_regions()
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .remove(&(ptr as usize));
        unsafe {
            spdk_mem_unregister(ptr, size);
        }
    }

    result
}

/// Tracks GDRCopy mapping state so free functions can perform full cleanup.
#[cfg(feature = "p2p")]
struct GdrMappingState {
    gdr: crate::gdrcopy_ffi::gdr_t,
    mh: crate::gdrcopy_ffi::gdr_mh_t,
    bar_ptr: *mut std::ffi::c_void,
    size: usize,
}

// SAFETY: GDRCopy handles are process-global and not thread-bound.
#[cfg(feature = "p2p")]
unsafe impl Send for GdrMappingState {}
#[cfg(feature = "p2p")]
unsafe impl Sync for GdrMappingState {}

#[cfg(feature = "p2p")]
static GDR_MAPPINGS: std::sync::OnceLock<
    std::sync::Mutex<std::collections::HashMap<usize, GdrMappingState>>,
> = std::sync::OnceLock::new();

#[cfg(feature = "p2p")]
fn gdr_mappings() -> &'static std::sync::Mutex<std::collections::HashMap<usize, GdrMappingState>> {
    GDR_MAPPINGS.get_or_init(|| std::sync::Mutex::new(std::collections::HashMap::new()))
}

/// Free function: unregisters from SPDK, unmaps GDRCopy BAR mapping,
/// unpins the GPU buffer, and closes the GDRCopy handle.
#[cfg(feature = "p2p")]
pub unsafe extern "C" fn spdk_unregister_gdr_unmap_and_close(ptr: *mut std::ffi::c_void) {
    unsafe {
        let state = gdr_mappings()
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .remove(&(ptr as usize));

        if let Some(s) = state {
            spdk_mem_unregister(s.bar_ptr, s.size);
            crate::gdrcopy_ffi::gdr_unmap(s.gdr, s.mh, s.bar_ptr, s.size);
            crate::gdrcopy_ffi::gdr_unpin_buffer(s.gdr, s.mh);
            crate::gdrcopy_ffi::gdr_close(s.gdr);
        }
    }
}

/// Create an SPDK `DmaBuffer` backed by a GDRCopy BAR1 mapping of GPU device memory.
///
/// This enables true NVMe→GPU P2P DMA: GDRCopy pins the GPU memory via
/// `nvidia_p2p_get_pages` and maps it through GPU BAR1, producing a CPU-visible
/// virtual address with valid pagemap entries pointing to GPU BAR1 physical
/// addresses. SPDK's vtophys can then resolve these for VFIO IOMMU DMA mapping.
///
/// The returned `DmaBuffer`'s pointer is the BAR1 mapping (for NVMe DMA targeting).
/// The actual GPU device pointer (`dev_ptr`) remains valid for CUDA access to the
/// same physical memory.
///
/// Requires:
///   - `gdrdrv` kernel module loaded
///   - `nvidia-peermem` kernel module loaded
///   - GPU memory allocated with `cudaMalloc` or opened via `cudaIpcOpenMemHandle`
///
/// On drop, the buffer unregisters from SPDK, unmaps BAR1, unpins GPU memory,
/// and closes the GDRCopy handle.
#[cfg(feature = "p2p")]
pub fn create_spdk_dma_buffer_from_gpu_bar(
    dev_ptr: *mut std::ffi::c_void,
    size: usize,
) -> Result<interfaces::DmaBuffer, String> {
    use crate::gdrcopy_ffi::*;

    // Align size up to GPU page boundary (64KB).
    let aligned_size = (size + GPU_PAGE_SIZE - 1) & !(GPU_PAGE_SIZE - 1);

    // SAFETY: Opens a connection to the gdrdrv kernel module.
    let gdr = unsafe { gdr_open() };
    if gdr.is_null() {
        return Err("gdr_open() failed — is gdrdrv kernel module loaded?".to_string());
    }

    // SAFETY: dev_ptr is a valid CUDA device pointer; size is the allocation size.
    let mut mh = gdr_mh_t::default();
    let rc = unsafe {
        gdr_pin_buffer(
            gdr,
            dev_ptr as std::os::raw::c_ulong,
            aligned_size,
            0,
            0,
            &mut mh,
        )
    };
    if rc != 0 {
        unsafe { gdr_close(gdr) };
        return Err(format!("gdr_pin_buffer failed (rc={})", rc));
    }

    // SAFETY: mh is a valid pinned handle from gdr_pin_buffer.
    let mut bar_ptr: *mut std::ffi::c_void = std::ptr::null_mut();
    let rc = unsafe { gdr_map(gdr, mh, &mut bar_ptr, aligned_size) };
    if rc != 0 {
        unsafe {
            gdr_unpin_buffer(gdr, mh);
            gdr_close(gdr);
        }
        return Err(format!("gdr_map failed (rc={})", rc));
    }

    if bar_ptr.is_null() {
        unsafe {
            gdr_unpin_buffer(gdr, mh);
            gdr_close(gdr);
        }
        return Err("gdr_map returned null pointer".to_string());
    }

    // Account for alignment offset: gdr_map may align the returned pointer
    // to the GPU page boundary. The offset between the aligned base and
    // the actual device pointer must be applied to bar_ptr.
    let offset = (dev_ptr as usize) & (GPU_PAGE_SIZE - 1);
    let effective_bar_ptr = unsafe { (bar_ptr as *mut u8).add(offset) as *mut std::ffi::c_void };

    // Register the BAR mapping with SPDK. The BAR pages have valid pagemap
    // entries pointing to GPU BAR1 physical addresses, so vtophys works.
    let rc = unsafe { spdk_mem_register(bar_ptr, aligned_size) };
    if rc != 0 {
        unsafe {
            gdr_unmap(gdr, mh, bar_ptr, aligned_size);
            gdr_unpin_buffer(gdr, mh);
            gdr_close(gdr);
        }
        return Err(format!(
            "spdk_mem_register on BAR mapping failed (rc={}). Is nvidia-peermem loaded?",
            rc
        ));
    }

    // Store state for cleanup. Key by the effective pointer (what DmaBuffer holds).
    gdr_mappings()
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .insert(
            effective_bar_ptr as usize,
            GdrMappingState {
                gdr,
                mh,
                bar_ptr,
                size: aligned_size,
            },
        );

    // SAFETY: effective_bar_ptr points into a valid BAR1 mapping registered with SPDK.
    // size is the user-requested size (may be less than aligned_size).
    // free_fn handles full cleanup. numa_node = -1 (GPU memory).
    let result = unsafe {
        interfaces::DmaBuffer::from_raw(
            effective_bar_ptr,
            size,
            spdk_unregister_gdr_unmap_and_close,
            -1,
        )
        .map_err(|e| format!("DmaBuffer creation failed: {}", e))
    };

    if result.is_err() {
        gdr_mappings()
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .remove(&(effective_bar_ptr as usize));
        unsafe {
            spdk_mem_unregister(bar_ptr, aligned_size);
            gdr_unmap(gdr, mh, bar_ptr, aligned_size);
            gdr_unpin_buffer(gdr, mh);
            gdr_close(gdr);
        }
    }

    result
}

/// Tracks physical DMA mappings (mmap'd VA → phys) for cleanup.
#[cfg(feature = "p2p")]
struct PhysMappingState {
    va: *mut std::ffi::c_void,
    phys_addr: u64,
    size: usize,
}

// SAFETY: The VA is process-global and not thread-bound.
#[cfg(feature = "p2p")]
unsafe impl Send for PhysMappingState {}
#[cfg(feature = "p2p")]
unsafe impl Sync for PhysMappingState {}

#[cfg(feature = "p2p")]
static PHYS_MAPPINGS: std::sync::OnceLock<
    std::sync::Mutex<std::collections::HashMap<usize, PhysMappingState>>,
> = std::sync::OnceLock::new();

#[cfg(feature = "p2p")]
fn phys_mappings() -> &'static std::sync::Mutex<std::collections::HashMap<usize, PhysMappingState>>
{
    PHYS_MAPPINGS.get_or_init(|| std::sync::Mutex::new(std::collections::HashMap::new()))
}

/// # Safety
///
/// `ptr` must be a VA previously created by `create_spdk_dma_buffer_from_phys`.
#[cfg(feature = "p2p")]
pub unsafe extern "C" fn vfio_unmap_extmem_munmap(ptr: *mut std::ffi::c_void) {
    unsafe {
        let state = phys_mappings()
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .remove(&(ptr as usize));

        if let Some(s) = state {
            rte_vfio_container_dma_unmap(-1, s.va as u64, s.phys_addr, s.size as u64);
            rte_extmem_unregister(s.va, s.size);
            libc::munmap(s.va, s.size);
        }
    }
}

/// Resolve a virtual address to its physical/IOVA address via SPDK's vtophys.
///
/// The buffer must already be registered with SPDK (e.g., via `spdk_mem_register`
/// or allocated via `spdk_dma_zmalloc`). Returns the physical address on success,
/// or `None` if translation fails (SPDK_VTOPHYS_ERROR = 0xFFFFFFFFFFFFFFFF).
///
/// # Safety
///
/// `ptr` must point to SPDK-registered memory of at least `size` bytes.
#[cfg(feature = "p2p")]
pub unsafe fn get_phys_addr(ptr: *const std::ffi::c_void, size: usize) -> Option<u64> {
    const SPDK_VTOPHYS_ERROR: u64 = 0xFFFF_FFFF_FFFF_FFFF;
    let mut sz = size as u64;
    let phys = unsafe { spdk_vtophys(ptr, &mut sz) };
    if phys == SPDK_VTOPHYS_ERROR {
        None
    } else {
        Some(phys)
    }
}

/// Create an SPDK `DmaBuffer` targeting a known physical address via DPDK IOMMU mapping.
///
/// This is used for cross-process GPU P2P DMA: the remote process (Python client)
/// allocates GPU memory, pins it with GDRCopy, and extracts the GPU BAR1 physical
/// address from pagemap. This function then:
/// 1. mmap's anonymous pages as a local VA placeholder
/// 2. Registers the VA with DPDK, associating it with the GPU BAR physical IOVA
/// 3. Programs the VFIO IOMMU to allow NVMe DMA to the physical address
///
/// When the NVMe controller performs DMA to this buffer, SPDK's vtophys resolves
/// the VA to the GPU BAR1 physical address, and the IOMMU permits the access.
///
/// On drop: VFIO DMA unmap → DPDK extmem unregister → munmap.
#[cfg(feature = "p2p")]
pub fn create_spdk_dma_buffer_from_phys(
    phys_addr: u64,
    size: usize,
) -> Result<interfaces::DmaBuffer, String> {
    // SAFETY: mmap anonymous private pages as a VA placeholder.
    let va = unsafe {
        libc::mmap(
            std::ptr::null_mut(),
            size,
            libc::PROT_READ | libc::PROT_WRITE,
            libc::MAP_PRIVATE | libc::MAP_ANONYMOUS,
            -1,
            0,
        )
    };
    if va == libc::MAP_FAILED {
        return Err("mmap anonymous pages failed".to_string());
    }

    // Tell DPDK that this VA range maps to the given physical/IOVA address.
    let iova_array: [u64; 1] = [phys_addr];
    let rc = unsafe { rte_extmem_register(va, size, iova_array.as_ptr(), 1, size) };
    if rc != 0 {
        unsafe { libc::munmap(va, size) };
        return Err(format!("rte_extmem_register failed (rc={})", rc));
    }

    // Program the VFIO IOMMU: allow NVMe DMA to the GPU BAR physical address.
    let rc = unsafe { rte_vfio_container_dma_map(-1, va as u64, phys_addr, size as u64) };
    if rc != 0 {
        unsafe {
            rte_extmem_unregister(va, size);
            libc::munmap(va, size);
        }
        return Err(format!("rte_vfio_container_dma_map failed (rc={})", rc));
    }

    // Store state for cleanup.
    phys_mappings()
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .insert(
            va as usize,
            PhysMappingState {
                va,
                phys_addr,
                size,
            },
        );

    // SAFETY: va is a valid mmap'd pointer registered with DPDK for the given IOVA.
    // NVMe DMA to this buffer will hit the GPU BAR1 physical address.
    let result = unsafe {
        interfaces::DmaBuffer::from_raw(va, size, vfio_unmap_extmem_munmap, -1)
            .map_err(|e| format!("DmaBuffer creation failed: {}", e))
    };

    if result.is_err() {
        phys_mappings()
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .remove(&(va as usize));
        unsafe {
            rte_vfio_container_dma_unmap(-1, va as u64, phys_addr, size as u64);
            rte_extmem_unregister(va, size);
            libc::munmap(va, size);
        }
    }

    result
}

/// Create an SPDK `DmaBuffer` from an existing BAR VA using direct DPDK IOMMU programming.
///
/// Unlike `create_spdk_dma_buffer_from_gpu_bar` which uses `spdk_mem_register`, this
/// function uses the raw DPDK APIs (`rte_extmem_register` + `rte_vfio_container_dma_map`)
/// to program the IOMMU. This is the mechanism used in cross-process P2P DMA where
/// the storage server needs to program DMA access to GPU BAR pages.
///
/// The BAR VA must already be mapped in the calling process's page table (e.g., via
/// GDRCopy `gdr_map`). The IOVA used is the VA itself (identity mapping).
///
/// On drop: VFIO DMA unmap → DPDK extmem unregister. Does NOT munmap (caller owns the VA).
///
/// # Safety
///
/// `bar_ptr` must be a valid pointer to a GDRCopy BAR mapping of at least `size` bytes.
#[cfg(feature = "p2p")]
pub unsafe fn create_spdk_dma_buffer_from_bar_direct(
    bar_ptr: *mut std::ffi::c_void,
    size: usize,
) -> Result<interfaces::DmaBuffer, String> {
    let iova = bar_ptr as u64;

    // Tell DPDK that vtophys(bar_ptr) = iova (VA-mode identity).
    // Use system page size for alignment (GDRCopy BAR mappings are 4K-aligned).
    let page_sz: usize = 4096;
    let n_pages = size.div_ceil(page_sz);
    let iova_array: Vec<u64> = (0..n_pages).map(|i| iova + (i * page_sz) as u64).collect();
    let rc = unsafe {
        rte_extmem_register(
            bar_ptr,
            size,
            iova_array.as_ptr(),
            n_pages as std::os::raw::c_uint,
            page_sz,
        )
    };
    if rc != 0 {
        let errno = std::io::Error::last_os_error();
        return Err(format!(
            "rte_extmem_register failed (rc={}, errno={}, va={:?}, size={}, page_sz={}, n_pages={})",
            rc, errno, bar_ptr, size, page_sz, n_pages
        ));
    }

    // Program the VFIO IOMMU: allow NVMe DMA to the BAR pages at this VA.
    let rc = unsafe { rte_vfio_container_dma_map(-1, bar_ptr as u64, iova, size as u64) };
    if rc != 0 {
        unsafe { rte_extmem_unregister(bar_ptr, size) };
        return Err(format!("rte_vfio_container_dma_map failed (rc={})", rc));
    }

    // Store state for cleanup (no munmap — caller owns the BAR mapping).
    phys_mappings()
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .insert(
            bar_ptr as usize,
            PhysMappingState {
                va: bar_ptr,
                phys_addr: iova,
                size,
            },
        );

    // SAFETY: bar_ptr is a valid GDRCopy BAR mapping, now IOMMU-registered for DMA.
    // free_fn handles VFIO unmap + extmem unregister (no munmap).
    let result = unsafe {
        interfaces::DmaBuffer::from_raw(bar_ptr, size, vfio_unmap_extmem_only, -1)
            .map_err(|e| format!("DmaBuffer creation failed: {}", e))
    };

    if result.is_err() {
        phys_mappings()
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .remove(&(bar_ptr as usize));
        unsafe {
            rte_vfio_container_dma_unmap(-1, bar_ptr as u64, iova, size as u64);
            rte_extmem_unregister(bar_ptr, size);
        }
    }

    result
}

/// # Safety
///
/// `ptr` must be a BAR VA previously registered via `create_spdk_dma_buffer_from_bar_direct`.
#[cfg(feature = "p2p")]
pub unsafe extern "C" fn vfio_unmap_extmem_only(ptr: *mut std::ffi::c_void) {
    unsafe {
        let state = phys_mappings()
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .remove(&(ptr as usize));

        if let Some(s) = state {
            rte_vfio_container_dma_unmap(-1, s.va as u64, s.phys_addr, s.size as u64);
            rte_extmem_unregister(s.va, s.size);
        }
    }
}

/// Create a GpuDmaBuffer from a verified and pinned IPC handle.
///
/// The caller is responsible for ensuring the handle has been verified
/// and pinned (tracked externally by the component state).
#[cfg(feature = "gpu")]
pub fn create_gpu_dma_buffer(handle: GpuIpcHandle) -> Result<GpuDmaBuffer, String> {
    if handle.as_ptr().is_null() {
        return Err("Handle has null pointer".to_string());
    }

    // SAFETY: The handle has been verified (device memory) and pinned
    // (tracked by component state). The pointer is valid for handle.size()
    // bytes. cuda_ipc_close_mem_handle correctly frees via cudaIpcCloseMemHandle.
    let buf =
        unsafe { GpuDmaBuffer::new(handle.as_ptr(), handle.size(), cuda_ipc_close_mem_handle) };

    // GpuIpcHandle has no Drop impl so letting it go out of scope is safe.
    // GpuDmaBuffer now owns the pointer via its free_fn.

    Ok(buf)
}


// --- FILE: lib.rs ---
//! Dispatcher component for the Certus storage system.
//!
//! Orchestrates cache operations (populate, lookup, check, remove) using
//! a DRAM memory-tier with LRU eviction and write-through to SSD.
//! Coordinates N data block devices with N extent managers for persistent storage.
//!
//! Provides the [`IDispatcher`] interface with receptacles for
//! [`ILogger`], [`IDispatchMap`], and [`IMemoryTier`].

mod background;
pub mod io_segmenter;
pub mod pipeline;

use parking_lot::RwLock;
use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;
use std::sync::Mutex;

use component_framework::define_component;
use interfaces::{
    CacheKey, ClientChannels, Command, Completion, DispatcherConfig, DispatcherError, DmaAllocFn,
    DmaBuffer, FormatParams, GpuStream, IBlockDevice, IBlockDeviceAdmin, IDispatchMap, IDispatcher,
    IExtentManager, IGpuServices, ILogger, IMemoryTier, IpcHandle, LookupResult, PciAddress,
    WriteHandle,
};

use block_device_spdk_nvme::BlockDeviceSpdkNvmeComponent;
use component_core::binding::bind;
use component_core::query_interface;
use extent_manager::ExtentManager;
use spdk_env::ISPDKEnv;

use crate::background::{BackgroundEvictor, BackgroundWriter, EvictorConfig, WriteJob};

/// A pending store awaiting commit or cancel.
///
/// Created by `prepare_store` and consumed by either `commit_store` (writes
/// the buffer to SSD and publishes the extent) or `cancel_store` (drops the
/// handle, which auto-aborts the reservation).
struct PendingWrite {
    /// Extent reservation handle; calling `publish()` commits, dropping aborts.
    write_handle: WriteHandle,
    /// DMA buffer the caller writes data into between prepare and commit.
    buffer: Arc<DmaBuffer>,
    /// Original (unaligned) data size in bytes.
    size: u32,
    /// Index into `data_drives` identifying the target SSD.
    drive_idx: usize,
}

/// Holds one (block-device, extent-manager) pair for a data drive.
#[allow(dead_code)]
struct DataDrive {
    _block_dev: Arc<dyn component_core::IUnknown + Send + Sync>,
    block_dev_admin: Arc<dyn IBlockDeviceAdmin + Send + Sync>,
    block_dev_iface: Arc<dyn IBlockDevice + Send + Sync>,
    extent_mgr: Arc<ExtentManager>,
    cached_channels: Option<ClientChannels>,
}

define_component! {
    pub DispatcherComponent {
        version: "0.1.0",
        provides: [IDispatcher],
        receptacles: {
            logger: ILogger,
            dispatch_map: IDispatchMap,
            gpu_services: IGpuServices,
            spdk_env: ISPDKEnv,
            memory_tier: IMemoryTier,
        },
        fields: {
            initialized: AtomicBool,
            bg_writer: Mutex<Option<BackgroundWriter>>,
            bg_evictor: Mutex<Option<BackgroundEvictor>>,
            data_drives: RwLock<Vec<DataDrive>>,
            pending_writes: Mutex<HashMap<CacheKey, PendingWrite>>,
            pipeline_ring: RwLock<Option<pipeline::PipelineRing>>,
            warm_stream: AtomicU64,
        },
    }
}

unsafe extern "C" fn libc_free(ptr: *mut std::ffi::c_void) {
    unsafe { libc::free(ptr) };
}

/// No-op free function for temporary DmaBuffer wrappers around memory-tier pointers.
/// The memory-tier component owns the memory; this wrapper must not free it.
unsafe extern "C" fn noop_free(_ptr: *mut std::ffi::c_void) {}

impl DispatcherComponent {
    fn log_info(&self, msg: &str) {
        if let Ok(logger) = self.logger.get() {
            logger.info(msg);
        }
    }

    #[allow(dead_code)]
    fn log_error(&self, msg: &str) {
        if let Ok(logger) = self.logger.get() {
            logger.error(msg);
        }
    }

    fn drive_index(key: CacheKey, num_drives: usize) -> usize {
        key as usize % num_drives
    }

    fn ensure_initialized(&self) -> Result<(), DispatcherError> {
        if !self.initialized.load(Ordering::Acquire) {
            return Err(DispatcherError::NotInitialized(
                "dispatcher not initialized".into(),
            ));
        }
        Ok(())
    }

    /// Write `buffer` contents to SSD using MDTS-aware segmented I/O.
    ///
    /// Splits the write into segments that respect the drive's maximum transfer
    /// size, allocates per-segment DMA buffers, and issues synchronous writes.
    fn write_buffer_to_ssd(
        drive: &dyn IBlockDevice,
        buffer: &DmaBuffer,
        start_lba: u64,
        total_bytes: usize,
    ) -> Result<(), DispatcherError> {
        let block_size = drive.block_size() as usize;
        let max_transfer = drive.max_transfer_size();
        let numa_node = drive.numa_node();
        let aligned_bytes = total_bytes.next_multiple_of(block_size);

        let channels = drive
            .connect_client()
            .map_err(|e| DispatcherError::IoError(format!("connect_client failed: {e}")))?;

        let segments =
            io_segmenter::segment_io(start_lba, aligned_bytes, max_transfer, block_size as u32);

        for seg in &segments {
            let seg_buf = DmaBuffer::new(seg.length, block_size, Some(numa_node)).map_err(|e| {
                DispatcherError::AllocationFailed(format!("DMA segment buffer: {e}"))
            })?;

            let copy_len = seg
                .length
                .min(total_bytes.saturating_sub(seg.buffer_offset));
            if copy_len > 0 {
                unsafe {
                    std::ptr::copy_nonoverlapping(
                        (buffer.as_ptr() as *const u8).add(seg.buffer_offset),
                        seg_buf.as_ptr() as *mut u8,
                        copy_len,
                    );
                }
            }

            let seg_buf = Arc::new(seg_buf);
            channels
                .command_tx
                .send(Command::WriteSync {
                    ns_id: 1,
                    lba: seg.lba,
                    buf: seg_buf,
                })
                .map_err(|_| DispatcherError::IoError("send WriteSync failed".into()))?;

            match channels.completion_rx.recv() {
                Ok(Completion::WriteDone { result, .. }) => {
                    result
                        .map_err(|e| DispatcherError::IoError(format!("SSD write failed: {e}")))?;
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
        Ok(())
    }

    /// Promote an SSD-resident entry back into the memory-tier and serve to GPU.
    ///
    /// Uses pipelined chunked reads: SSD→DRAM (memory-tier) while streaming
    /// chunks from DRAM→GPU.
    fn promote_and_serve(
        &self,
        key: CacheKey,
        offset: u64,
        ipc_handle: &IpcHandle,
        gpu: &Arc<dyn IGpuServices + Send + Sync>,
        dm: &Arc<dyn IDispatchMap + Send + Sync>,
        mt: &Arc<dyn IMemoryTier + Send + Sync>,
    ) -> Result<(), DispatcherError> {
        let total_bytes = ipc_handle.size as usize;

        // Evict if needed to make space.
        Self::evict_for_space(dm, mt, ipc_handle.size)?;

        // Insert into memory-tier.
        let mem_ptr = mt.insert(key, ipc_handle.size).map_err(|e| {
            DispatcherError::AllocationFailed(format!("promote insert failed: {e}"))
        })?;

        // Read from SSD into memory-tier using pipelined reader.
        let drives = self.data_drives.read();
        if drives.is_empty() {
            // No hardware: just copy zeros to GPU (test/staging-only mode).
            let aligned = total_bytes.next_multiple_of(4096).max(4096);
            let temp_buf = unsafe {
                DmaBuffer::from_raw(mem_ptr as *mut std::ffi::c_void, aligned, noop_free, -1)
            }
            .map_err(|e| DispatcherError::IoError(format!("DmaBuffer wrap failed: {e}")))?;
            let result = gpu.dma_copy_to_device(
                &temp_buf,
                ipc_handle.address as *mut std::ffi::c_void,
                total_bytes,
            );
            std::mem::forget(temp_buf);
            // Register promoted entry in dispatch-map.
            let _ = dm.create_memory_tier_entry(key, mem_ptr, ipc_handle.size);
            let _ = dm.release_write(key);
            return result.map_err(|e| {
                DispatcherError::IoError(format!("GPU DMA copy (promote) failed: {e}"))
            });
        }

        let idx = Self::drive_index(key, drives.len());
        let drive = &drives[idx];
        let block_size = drive.block_dev_iface.block_size();
        let start_lba = offset / block_size as u64;
        let block_dev = Arc::clone(&drive.block_dev_iface);

        // Use cached channels if available, otherwise create new ones.
        let channels = match &drive.cached_channels {
            Some(ch) => ch,
            None => {
                drop(drives);
                return Err(DispatcherError::IoError(
                    "no cached channels for drive".into(),
                ));
            }
        };

        // Zero-copy pipelined reader: NVMe → memory-tier slot → GPU (no intermediate ring copy).
        // SAFETY: mem_ptr is a valid, CUDA-pinned, SPDK-registered memory-tier slot.
        // ipc_handle.address is a valid GPU destination pointer.
        let ring_guard = self.pipeline_ring.read();
        let ring_ref = ring_guard
            .as_ref()
            .ok_or_else(|| DispatcherError::NotInitialized("pipeline ring not allocated".into()))?;
        unsafe {
            pipeline::pipelined_ssd_to_gpu_zero_copy(
                &*block_dev,
                &**gpu,
                &ring_ref.streams,
                channels,
                mem_ptr,
                ipc_handle.address as *mut std::ffi::c_void,
                start_lba,
                total_bytes,
                ring_ref.chunk_size,
                16,
            )?;
        }
        drop(ring_guard);
        drop(drives);

        // Update dispatch-map: remove old BlockDevice entry and create fresh MemoryTier.
        // Since we released the read ref before calling this method, we can remove
        // and re-register.
        let _ = dm.remove(key);
        dm.create_memory_tier_entry(key, mem_ptr, ipc_handle.size)
            .map_err(|e| DispatcherError::IoError(format!("promote re-register failed: {e}")))?;
        // Set the ssd_offset since data is still on SSD.
        let _ = dm.convert_to_storage(key, offset);
        let _ = dm.release_write(key);

        Ok(())
    }

    /// Evict entries from the memory-tier until enough space is available.
    ///
    /// Each evicted entry transitions from MemoryTier to BlockDevice in the
    /// dispatch-map. If write-through hasn't completed (no ssd_offset), the
    /// dispatch-map entry is removed entirely so lookups get NotExist rather
    /// than a dangling memory-tier pointer.
    fn evict_for_space(
        dm: &Arc<dyn IDispatchMap + Send + Sync>,
        mt: &Arc<dyn IMemoryTier + Send + Sync>,
        needed: u32,
    ) -> Result<(), DispatcherError> {
        // Under high concurrency (many threads promoting cold entries simultaneously),
        // scanning many candidates per attempt causes severe MT lock contention because
        // oldest_keys(N) holds the lock while scanning N entries.  Use a tiny scan
        // window and prefer blind LRU as the primary fast path — one O(1) lock
        // acquisition per iteration keeps contention proportional to thread count.
        const MAX_SCAN: usize = 4;
        const MAX_ATTEMPTS: usize = 512;

        let mut attempts = 0usize;
        while mt.used() + needed as usize > mt.capacity() {
            attempts += 1;
            if attempts > MAX_ATTEMPTS {
                return Err(DispatcherError::AllocationFailed(
                    "memory-tier full: eviction did not free enough space".into(),
                ));
            }

            // Every 8th attempt probe a small batch for a clean eviction
            // (write-through complete, no data loss).  All other iterations
            // fall straight through to blind LRU to minimise lock hold time.
            let evict_key = if attempts % 8 == 0 {
                let candidates = mt.oldest_keys(MAX_SCAN);
                candidates.iter().find(|&&k| dm.is_evictable(k)).copied()
            } else {
                None
            };

            match evict_key {
                Some(key) => {
                    // Another thread may have concurrently evicted this key.
                    if mt.remove(key).is_ok() {
                        let _ = dm.convert_memory_tier_to_block(key);
                    }
                }
                None => {
                    // Blind LRU: O(1) under the MT lock. Data loss is acceptable
                    // under pressure; entries still in flight on SSD are removed
                    // from the dispatch-map so stale lookups get NotExist.
                    if let Some(evicted_key) = mt.evict_lru() {
                        if dm.convert_memory_tier_to_block(evicted_key).is_err() {
                            let _ = dm.remove(evicted_key);
                        }
                    }
                }
            }
        }
        Ok(())
    }

    fn process_write_job(
        dm: &Arc<dyn IDispatchMap + Send + Sync>,
        mt: &Arc<dyn IMemoryTier + Send + Sync>,
        drives: &[Arc<dyn IBlockDevice + Send + Sync>],
        extent_mgrs: &[Arc<ExtentManager>],
        job: WriteJob,
    ) {
        // Get the memory-tier pointer without refreshing LRU — the write-through
        // must not prevent this entry from being evicted under memory pressure.
        let (mem_ptr, _size) = match mt.peek(job.key) {
            Some(v) => v,
            None => {
                let _ = dm.release_read(job.key);
                return;
            }
        };

        if drives.is_empty() {
            // No block devices: mark as converted with a synthetic offset.
            let block_offset = job.key * 4096;
            let _ = dm.convert_to_storage(job.key, block_offset);
            return;
        }

        let drive_idx = job.device_index % drives.len();
        let drive = &drives[drive_idx];
        let block_size = drive.block_size() as usize;
        let total_bytes = job.size as usize;
        let aligned_bytes = total_bytes.next_multiple_of(block_size);

        // Wrap memory-tier pointer as a temporary DmaBuffer (noop free).
        // SAFETY: mem_ptr is valid for at least `aligned_bytes` and owned by memory-tier.
        let temp_buf = match unsafe {
            DmaBuffer::from_raw(
                mem_ptr as *mut std::ffi::c_void,
                aligned_bytes,
                noop_free,
                -1,
            )
        } {
            Ok(buf) => buf,
            Err(_) => {
                let _ = dm.release_read(job.key);
                return;
            }
        };

        // Allocate extent via the extent manager.
        let em = &extent_mgrs[drive_idx % extent_mgrs.len()];
        let iem = match query_interface!(em, IExtentManager) {
            Some(i) => i,
            None => {
                let _ = dm.release_read(job.key);
                return;
            }
        };
        let write_handle = match iem.reserve_extent(job.key, aligned_bytes as u32) {
            Ok(wh) => wh,
            Err(_) => {
                let _ = dm.release_read(job.key);
                return;
            }
        };

        let block_offset = write_handle.extent_offset();
        let start_lba = block_offset / block_size as u64;

        if Self::write_buffer_to_ssd(&**drive, &temp_buf, start_lba, total_bytes).is_err() {
            let _ = dm.release_read(job.key);
            return; // write_handle drops → abort
        }

        // Prevent the noop-free DmaBuffer from being dropped normally.
        std::mem::forget(temp_buf);

        // Data written successfully — commit the extent metadata.
        // convert_to_storage also decrements the read reference.
        let _ = write_handle.publish();
        let _ = dm.convert_to_storage(job.key, block_offset);
        let _ = dm.release_read(job.key);
    }
}

impl DispatcherComponent {
    fn parse_pci_addr(s: &str) -> Result<PciAddress, DispatcherError> {
        let parts: Vec<&str> = s.split(':').collect();
        if parts.len() != 3 {
            return Err(DispatcherError::InvalidParameter(format!(
                "invalid PCI address format: {s}"
            )));
        }
        let domain = u32::from_str_radix(parts[0], 16).map_err(|_| {
            DispatcherError::InvalidParameter(format!("invalid PCI domain: {}", parts[0]))
        })?;
        let bus = u8::from_str_radix(parts[1], 16).map_err(|_| {
            DispatcherError::InvalidParameter(format!("invalid PCI bus: {}", parts[1]))
        })?;
        let dev_func: Vec<&str> = parts[2].split('.').collect();
        if dev_func.len() != 2 {
            return Err(DispatcherError::InvalidParameter(format!(
                "invalid PCI dev.func: {}",
                parts[2]
            )));
        }
        let dev = u8::from_str_radix(dev_func[0], 16).map_err(|_| {
            DispatcherError::InvalidParameter(format!("invalid PCI dev: {}", dev_func[0]))
        })?;
        let func = u8::from_str_radix(dev_func[1], 16).map_err(|_| {
            DispatcherError::InvalidParameter(format!("invalid PCI func: {}", dev_func[1]))
        })?;
        Ok(PciAddress {
            domain,
            bus,
            dev,
            func,
        })
    }

    #[allow(clippy::type_complexity)]
    fn create_block_device(
        &self,
        i: usize,
        poller_base_cpu: Option<usize>,
        spdk_env: &Arc<dyn ISPDKEnv + Send + Sync>,
        logger: &Arc<dyn ILogger + Send + Sync>,
        pci_addr: PciAddress,
        addr_str: &str,
    ) -> Result<
        (
            Arc<dyn component_core::IUnknown + Send + Sync>,
            Arc<dyn IBlockDeviceAdmin + Send + Sync>,
            Arc<dyn IBlockDevice + Send + Sync>,
        ),
        DispatcherError,
    > {
        let block_dev = BlockDeviceSpdkNvmeComponent::new_default();
        block_dev
            .spdk_env
            .connect(Arc::clone(spdk_env))
            .map_err(|e| {
                DispatcherError::IoError(format!("failed to wire spdk_env for data drive {i}: {e}"))
            })?;
        block_dev.logger.connect(Arc::clone(logger)).map_err(|e| {
            DispatcherError::IoError(format!("failed to wire logger for data drive {i}: {e}"))
        })?;
        let admin = query_interface!(block_dev, IBlockDeviceAdmin).ok_or_else(|| {
            DispatcherError::IoError(format!(
                "failed to query IBlockDeviceAdmin for data drive {i}"
            ))
        })?;
        admin.set_pci_address(pci_addr);
        if let Some(base) = poller_base_cpu {
            admin.set_actor_cpu(base + i);
        }
        admin.initialize().map_err(|e| {
            DispatcherError::IoError(format!(
                "failed to initialize block device at {addr_str}: {e}"
            ))
        })?;
        let ibd = query_interface!(block_dev, IBlockDevice).ok_or_else(|| {
            DispatcherError::IoError(format!("failed to query IBlockDevice for data drive {i}"))
        })?;
        Ok((
            block_dev as Arc<dyn component_core::IUnknown + Send + Sync>,
            admin,
            ibd,
        ))
    }

    fn create_data_drives(
        &self,
        config: &DispatcherConfig,
    ) -> Result<Vec<DataDrive>, DispatcherError> {
        let spdk_env = self
            .spdk_env
            .get()
            .map_err(|_| DispatcherError::NotInitialized("spdk_env not bound".into()))?;

        let logger = self
            .logger
            .get()
            .map_err(|_| DispatcherError::NotInitialized("logger not bound".into()))?;

        let mut drives = Vec::with_capacity(config.data_pci_addrs.len());

        for (i, addr_str) in config.data_pci_addrs.iter().enumerate() {
            let pci_addr = Self::parse_pci_addr(addr_str)?;

            let (block_dev_component, admin, ibd) =
                self.create_block_device(i, config.poller_base_cpu, &spdk_env, &logger, pci_addr, addr_str)?;

            let extent_mgr = ExtentManager::new_inner();

            let numa_node = ibd.numa_node();
            let dma_alloc: DmaAllocFn = Arc::new(move |size, align, _numa| {
                DmaBuffer::new(size, align, Some(numa_node)).map_err(|e| e.to_string())
            });
            extent_mgr.set_dma_alloc(dma_alloc);

            extent_mgr
                .logger
                .connect(Arc::clone(&logger) as Arc<dyn ILogger + Send + Sync>)
                .map_err(|e| {
                    DispatcherError::IoError(format!(
                        "failed to wire logger for extent manager {i}: {e}"
                    ))
                })?;

            bind(
                &*block_dev_component,
                "IBlockDevice",
                &*extent_mgr as &dyn component_core::IUnknown,
                "metadata_device",
            )
            .map_err(|e| {
                DispatcherError::IoError(format!(
                    "failed to bind block device to extent manager {i}: {e}"
                ))
            })?;

            let iem = query_interface!(extent_mgr, IExtentManager).ok_or_else(|| {
                DispatcherError::IoError(format!(
                    "failed to query IExtentManager for data drive {i}"
                ))
            })?;
            let sector_size = ibd.block_size();
            let num_sectors = ibd.num_sectors(1).unwrap_or(0);
            let data_disk_size = num_sectors * sector_size as u64;
            let defaults = FormatParams::default();
            let region_size = data_disk_size / defaults.region_count as u64;
            // Slab must fit within a buddy-allocated region. Use 1/16 of region
            // (rounded to a power-of-2 in blocks) to allow many size classes.
            let blocks_in_region = region_size / sector_size as u64;
            let target_slab_blocks = blocks_in_region / 16;
            let slab_size = if target_slab_blocks > 0 {
                let pow2 = 1u64 << (63 - target_slab_blocks.leading_zeros());
                (pow2 * sector_size as u64).min(defaults.slab_size)
            } else {
                defaults.slab_size
            };
            let max_extent_size = (slab_size.min(defaults.max_extent_size as u64)) as u32;
            if config.format_on_init {
                iem.format(FormatParams {
                    data_disk_size,
                    sector_size,
                    slab_size,
                    max_extent_size,
                    ..defaults
                })
                .map_err(|e| {
                    DispatcherError::IoError(format!(
                        "failed to format extent manager for data drive {i}: {e}"
                    ))
                })?;
            } else {
                iem.initialize().map_err(|e| {
                    DispatcherError::IoError(format!(
                        "failed to recover extent manager for data drive {i}: {e}"
                    ))
                })?;
            }

            let cpu_msg = config.poller_base_cpu
                .map(|base| format!(", poller pinned to CPU {}", base + i))
                .unwrap_or_default();
            self.log_info(&format!(
                "dispatcher: data drive {i} initialized at {addr_str}{cpu_msg}"
            ));

            let cached_channels = ibd.connect_client().ok();

            drives.push(DataDrive {
                _block_dev: block_dev_component,
                block_dev_admin: admin,
                block_dev_iface: ibd,
                extent_mgr,
                cached_channels,
            });
        }

        Ok(drives)
    }
}

impl IDispatcher for DispatcherComponent {
    fn initialize(&self, config: DispatcherConfig) -> Result<(), DispatcherError> {
        self.log_info("dispatcher: initializing");

        self.dispatch_map
            .get()
            .map_err(|_| DispatcherError::NotInitialized("dispatch_map not bound".into()))?;

        self.memory_tier
            .get()
            .map_err(|_| DispatcherError::NotInitialized("memory_tier not bound".into()))?;

        if config.data_pci_addrs.is_empty() {
            return Err(DispatcherError::InvalidParameter(
                "data_pci_addrs must not be empty".into(),
            ));
        }

        // Create N block devices and N extent managers from config.
        // If spdk_env is not connected, skip drive creation (memory-tier-only mode).
        if self.spdk_env.is_connected() {
            let drives = self.create_data_drives(&config)?;
            *self.data_drives.write() = drives;

            // Rebuild dispatch-map from recovered extents when not formatting.
            if !config.format_on_init {
                let t0 = std::time::Instant::now();
                let dm = self.dispatch_map.get().map_err(|_| {
                    DispatcherError::NotInitialized("dispatch_map not bound".into())
                })?;
                let mut recovered: u64 = 0;
                let drives_guard = self.data_drives.read();
                for drive in drives_guard.iter() {
                    let iem =
                        query_interface!(drive.extent_mgr, IExtentManager).ok_or_else(|| {
                            DispatcherError::IoError(
                                "failed to query IExtentManager during recovery".into(),
                            )
                        })?;
                    iem.for_each_extent(&mut |extent| {
                        let _ = dm.recover_extent(extent.key, extent.offset, extent.size);
                        recovered += 1;
                    });
                }
                drop(drives_guard);
                let elapsed = t0.elapsed();
                self.log_info(&format!(
                    "dispatcher: dispatch-map recovered {recovered} extents from disk ({elapsed:.2?})"
                ));
            }

            // Pre-allocate pipeline ring for promote_and_serve (CUDA-pinned + SPDK-registered).
            if let Ok(gpu) = self.gpu_services.get() {
                let chunk_size = {
                    let dd = self.data_drives.read();
                    dd.first()
                        .map(|d| d.block_dev_iface.max_transfer_size() as usize)
                        .unwrap_or(131072)
                };
                match pipeline::PipelineRing::new(&*gpu, chunk_size) {
                    Ok(ring) => {
                        *self.pipeline_ring.write() = Some(ring);
                    }
                    Err(e) => {
                        self.log_info(&format!(
                            "pipeline ring allocation failed (non-fatal): {e:?}"
                        ));
                    }
                }

                // Dedicated CUDA stream for warm-path DMA (avoids pipeline_ring lock).
                match gpu.create_stream() {
                    Ok(stream) => {
                        self.warm_stream.store(stream.0 as u64, Ordering::Release);
                    }
                    Err(e) => {
                        self.log_info(&format!("warm stream allocation failed (non-fatal): {e}"));
                    }
                }

                // Register memory-tier pool as CUDA-pinned + SPDK DMA-capable
                // for zero-copy NVMe reads and async GPU transfers.
                if let Ok(mt) = self.memory_tier.get() {
                    if let Some((pool_ptr, pool_size)) = mt.pool_info() {
                        match gpu.register_host_memory(pool_ptr as *mut std::ffi::c_void, pool_size)
                        {
                            Ok(()) => {
                                self.log_info(&format!(
                                    "dispatcher: registered memory-tier pool ({} MiB) for zero-copy DMA",
                                    pool_size / (1024 * 1024)
                                ));
                            }
                            Err(e) => {
                                self.log_info(&format!(
                                    "memory-tier pool registration failed (non-fatal): {e}"
                                ));
                            }
                        }
                    }
                }
            }
        }

        let dm_for_writer = self
            .dispatch_map
            .get()
            .map_err(|_| DispatcherError::NotInitialized("dispatch_map not bound".into()))?;

        let mt_for_writer = self
            .memory_tier
            .get()
            .map_err(|_| DispatcherError::NotInitialized("memory_tier not bound".into()))?;

        // Collect block device interfaces and extent managers for the background writer.
        let bg_drives: Vec<Arc<dyn IBlockDevice + Send + Sync>> = {
            let dd = self.data_drives.read();
            dd.iter().map(|d| Arc::clone(&d.block_dev_iface)).collect()
        };
        let bg_extent_mgrs: Vec<Arc<ExtentManager>> = {
            let dd = self.data_drives.read();
            dd.iter().map(|d| Arc::clone(&d.extent_mgr)).collect()
        };

        let writer = BackgroundWriter::start(move |job: WriteJob| {
            Self::process_write_job(
                &dm_for_writer,
                &mt_for_writer,
                &bg_drives,
                &bg_extent_mgrs,
                job,
            );
        });

        *self.bg_writer.lock().unwrap() = Some(writer);

        // Start background SSD evictor if drives exist and threshold is configured.
        if config.ssd_eviction_threshold > 0.0 {
            let dm_for_evictor = self
                .dispatch_map
                .get()
                .map_err(|_| DispatcherError::NotInitialized("dispatch_map not bound".into()))?;
            let mt_for_evictor = self
                .memory_tier
                .get()
                .map_err(|_| DispatcherError::NotInitialized("memory_tier not bound".into()))?;
            let evictor_extent_mgrs: Vec<Arc<ExtentManager>> = {
                let dd = self.data_drives.read();
                dd.iter().map(|d| Arc::clone(&d.extent_mgr)).collect()
            };
            let evictor_logger = self.logger.get().ok();

            if !evictor_extent_mgrs.is_empty() {
                let evictor = BackgroundEvictor::start(
                    dm_for_evictor,
                    mt_for_evictor,
                    evictor_extent_mgrs,
                    EvictorConfig {
                        threshold: config.ssd_eviction_threshold,
                        low_watermark: config.ssd_eviction_low_watermark,
                        batch_size: config.ssd_eviction_batch_size,
                        interval: std::time::Duration::from_secs(config.ssd_eviction_interval_secs),
                    },
                    evictor_logger,
                );
                *self.bg_evictor.lock().unwrap() = Some(evictor);
            }
        }

        self.initialized.store(true, Ordering::Release);

        self.log_info("dispatcher: initialized");
        Ok(())
    }

    fn shutdown(&self) -> Result<(), DispatcherError> {
        self.log_info("dispatcher: shutting down");

        if let Some(mut evictor) = self.bg_evictor.lock().unwrap().take() {
            evictor.shutdown();
        }

        if let Some(mut writer) = self.bg_writer.lock().unwrap().take() {
            writer.shutdown();
        }

        self.pending_writes.lock().unwrap().clear();

        // Checkpoint all extent managers to persist metadata before teardown.
        {
            let drives = self.data_drives.read();
            for (i, drive) in drives.iter().enumerate() {
                if let Some(iem) = query_interface!(drive.extent_mgr, IExtentManager) {
                    if let Err(e) = iem.checkpoint() {
                        self.log_error(&format!(
                            "dispatcher: extent manager {i} checkpoint failed: {e}"
                        ));
                    }
                }
            }
        }

        // Unregister memory-tier pool from CUDA/SPDK before tearing down.
        if let (Ok(gpu), Ok(mt)) = (self.gpu_services.get(), self.memory_tier.get()) {
            if let Some((pool_ptr, pool_size)) = mt.pool_info() {
                let _ = gpu.unregister_host_memory(pool_ptr as *mut std::ffi::c_void, pool_size);
            }
        }

        // Destroy warm stream and pipeline ring.
        if let Ok(gpu) = self.gpu_services.get() {
            let raw = self.warm_stream.swap(0, Ordering::AcqRel);
            if raw != 0 {
                let _ = gpu.destroy_stream(GpuStream(raw as *mut std::ffi::c_void));
            }
            let ring_opt = self.pipeline_ring.write().take();
            if let Some(ring) = ring_opt {
                ring.destroy(&*gpu);
            }
        }

        // Shut down block devices in reverse order
        let drives = {
            let mut g = self.data_drives.write();
            let taken = std::mem::take(&mut *g);
            taken
        };
        for (i, drive) in drives.iter().enumerate().rev() {
            if let Err(e) = drive.block_dev_admin.shutdown() {
                self.log_error(&format!(
                    "dispatcher: failed to shut down data drive {i}: {e}"
                ));
            }
        }

        self.initialized.store(false, Ordering::Release);
        self.log_info("dispatcher: shut down");
        Ok(())
    }

    fn lookup(&self, key: CacheKey, ipc_handle: IpcHandle) -> Result<(), DispatcherError> {
        let stream = self.lookup_async(key, ipc_handle)?;
        if !stream.0.is_null() {
            let gpu = self
                .gpu_services
                .get()
                .map_err(|_| DispatcherError::NotInitialized("gpu_services not bound".into()))?;
            gpu.stream_synchronize(stream)
                .map_err(|e| DispatcherError::IoError(format!("stream_synchronize failed: {e}")))?;
        }
        Ok(())
    }

    fn batch_lookup(
        &self,
        entries: &[(CacheKey, IpcHandle)],
    ) -> Vec<Result<(), DispatcherError>> {
        if entries.is_empty() {
            return Vec::new();
        }

        let init_check = self.ensure_initialized();
        if let Err(e) = init_check {
            return entries.iter().map(|_| Err(e.clone())).collect();
        }

        let dm = match self.dispatch_map.get() {
            Ok(dm) => dm,
            Err(_) => {
                let e = DispatcherError::NotInitialized("dispatch_map not bound".into());
                return entries.iter().map(|_| Err(e.clone())).collect();
            }
        };
        let mt = match self.memory_tier.get() {
            Ok(mt) => mt,
            Err(_) => {
                let e = DispatcherError::NotInitialized("memory_tier not bound".into());
                return entries.iter().map(|_| Err(e.clone())).collect();
            }
        };
        let gpu = match self.gpu_services.get() {
            Ok(gpu) => gpu,
            Err(_) => {
                let e = DispatcherError::NotInitialized("gpu_services not bound".into());
                return entries.iter().map(|_| Err(e.clone())).collect();
            }
        };

        let mut results: Vec<Option<Result<(), DispatcherError>>> = vec![None; entries.len()];

        // Classify entries and handle fast paths inline.
        struct ColdEntry {
            idx: usize,
            key: CacheKey,
            offset: u64,
            ipc_handle_addr: *mut u8,
            ipc_handle_size: u32,
        }
        // SAFETY: ColdEntry contains a raw pointer from IpcHandle (GPU device pointer).
        // These pointers are valid across threads — CUDA IPC handles are designed for
        // cross-process/thread use. We only read the pointer value to pass to CUDA APIs.
        unsafe impl Send for ColdEntry {}
        unsafe impl Sync for ColdEntry {}

        let mut cold_entries: Vec<ColdEntry> = Vec::new();

        for (i, (key, ipc_handle)) in entries.iter().enumerate() {
            let key = *key;
            match dm.lookup(key) {
                Ok(lookup_result) => match lookup_result {
                    LookupResult::NotExist => {
                        results[i] = Some(Err(DispatcherError::KeyNotFound(key)));
                    }
                    LookupResult::MismatchSize => {
                        let _ = dm.release_read(key);
                        results[i] = Some(Err(DispatcherError::InvalidParameter(
                            "size mismatch on lookup".into(),
                        )));
                    }
                    LookupResult::MemoryTier { pointer, size } => {
                        let copy_size = (ipc_handle.size as usize).min(size as usize);
                        let raw = self.warm_stream.load(Ordering::Acquire);
                        let res = if raw != 0 {
                            let s = GpuStream(raw as *mut std::ffi::c_void);
                            gpu.memcpy_h2d_async(
                                pointer as *const std::ffi::c_void,
                                ipc_handle.address as *mut std::ffi::c_void,
                                copy_size,
                                s,
                            )
                            .map_err(|e| {
                                DispatcherError::IoError(format!(
                                    "GPU DMA copy (memory-tier→device) failed: {e}"
                                ))
                            })
                            .and_then(|_| {
                                gpu.stream_synchronize(s).map_err(|e| {
                                    DispatcherError::IoError(format!(
                                        "stream_synchronize failed: {e}"
                                    ))
                                })
                            })
                        } else {
                            let aligned = copy_size.next_multiple_of(4096).max(4096);
                            let temp_buf = unsafe {
                                DmaBuffer::from_raw(
                                    pointer as *mut std::ffi::c_void,
                                    aligned,
                                    noop_free,
                                    -1,
                                )
                            }
                            .map_err(|e| {
                                DispatcherError::IoError(format!("DmaBuffer wrap failed: {e}"))
                            });
                            match temp_buf {
                                Ok(buf) => {
                                    let r = gpu.dma_copy_to_device(
                                        &buf,
                                        ipc_handle.address as *mut std::ffi::c_void,
                                        copy_size,
                                    )
                                    .map_err(|e| {
                                        DispatcherError::IoError(format!(
                                            "GPU DMA copy (memory-tier→device) failed: {e}"
                                        ))
                                    });
                                    std::mem::forget(buf);
                                    r
                                }
                                Err(e) => Err(e),
                            }
                        };
                        let _ = dm.release_read(key);
                        mt.touch(key);
                        results[i] = Some(res);
                    }
                    LookupResult::Staging { buffer } => {
                        let res = gpu
                            .dma_copy_to_device(
                                &buffer,
                                ipc_handle.address as *mut std::ffi::c_void,
                                ipc_handle.size as usize,
                            )
                            .map_err(|e| {
                                DispatcherError::IoError(format!(
                                    "GPU DMA copy (staging→device) failed: {e}"
                                ))
                            });
                        let _ = dm.release_read(key);
                        results[i] = Some(res);
                    }
                    LookupResult::BlockDevice { offset } => {
                        let _ = dm.release_read(key);
                        cold_entries.push(ColdEntry {
                            idx: i,
                            key,
                            offset,
                            ipc_handle_addr: ipc_handle.address,
                            ipc_handle_size: ipc_handle.size,
                        });
                    }
                },
                Err(_) => {
                    results[i] = Some(Err(DispatcherError::KeyNotFound(key)));
                }
            }
        }

        // Promote cold entries in parallel — multiple queue threads per drive.
        // Each thread gets its own NVMe queue pair and CUDA streams, enabling
        // concurrent reads on the same physical drive.
        if !cold_entries.is_empty() {
            const MAX_QUEUES_PER_DRIVE: usize = 2;

            let chunk_size = {
                let ring_guard = self.pipeline_ring.read();
                ring_guard.as_ref().map_or(131072, |r| r.chunk_size)
            };

            let drives = self.data_drives.read();
            let num_drives = drives.len();

            if num_drives == 0 {
                for entry in &cold_entries {
                    Self::evict_for_space(&dm, &mt, entry.ipc_handle_size).ok();
                    let res = mt.insert(entry.key, entry.ipc_handle_size).map(|mem_ptr| {
                        let _ = dm.create_memory_tier_entry(entry.key, mem_ptr, entry.ipc_handle_size);
                        let _ = dm.release_write(entry.key);
                    }).map_err(|e| {
                        DispatcherError::AllocationFailed(format!("promote insert failed: {e}"))
                    });
                    results[entry.idx] = Some(res);
                }
            } else {
                // Group cold entries by target drive.
                let mut per_drive: Vec<Vec<usize>> = vec![Vec::new(); num_drives];
                for (ci, entry) in cold_entries.iter().enumerate() {
                    let drive_idx = Self::drive_index(entry.key, num_drives);
                    per_drive[drive_idx].push(ci);
                }

                std::thread::scope(|s| {
                    let mut thread_handles: Vec<
                        std::thread::ScopedJoinHandle<Vec<(usize, Result<(), DispatcherError>)>>,
                    > = Vec::new();

                    for (drive_idx, entry_indices) in per_drive.iter().enumerate() {
                        if entry_indices.is_empty() {
                            continue;
                        }

                        // Split this drive's entries across multiple queue threads.
                        let num_queues = MAX_QUEUES_PER_DRIVE.min(entry_indices.len());
                        let chunks: Vec<&[usize]> = entry_indices
                            .chunks((entry_indices.len() + num_queues - 1) / num_queues)
                            .collect();

                        let queue_depth = 16 / num_queues;

                        for chunk in chunks {
                            let dm_ref = &dm;
                            let mt_ref = &mt;
                            let gpu_ref = &gpu;
                            let drives_ref = &drives;
                            let cold_ref = &cold_entries;
                            let indices = chunk.to_vec();

                            let handle = s.spawn(move || {
                                let drive = &drives_ref[drive_idx];
                                let block_size = drive.block_dev_iface.block_size();

                                let channels =
                                    drive.block_dev_iface.connect_client().map_err(|e| {
                                        DispatcherError::IoError(format!(
                                            "connect_client failed: {e}"
                                        ))
                                    });
                                let streams_result = gpu_ref.create_stream().and_then(|a| {
                                    gpu_ref.create_stream().map(|b| [a, b]).map_err(|e| {
                                        let _ = gpu_ref.destroy_stream(a);
                                        e
                                    })
                                });

                                let mut batch_results: Vec<(usize, Result<(), DispatcherError>)> =
                                    Vec::with_capacity(indices.len());

                                let (channels, streams) = match (channels, streams_result) {
                                    (Ok(ch), Ok(st)) => (ch, st),
                                    (Err(e), _) => {
                                        for &ci in &indices {
                                            batch_results.push((ci, Err(e.clone())));
                                        }
                                        return batch_results;
                                    }
                                    (_, Err(e)) => {
                                        let err = DispatcherError::IoError(format!(
                                            "create_stream failed: {e}"
                                        ));
                                        for &ci in &indices {
                                            batch_results.push((ci, Err(err.clone())));
                                        }
                                        return batch_results;
                                    }
                                };

                                for &ci in &indices {
                                    let entry = &cold_ref[ci];
                                    let ipc = IpcHandle {
                                        address: entry.ipc_handle_addr,
                                        size: entry.ipc_handle_size,
                                    };
                                    let total_bytes = ipc.size as usize;

                                    let res = (|| -> Result<(), DispatcherError> {
                                        Self::evict_for_space(dm_ref, mt_ref, ipc.size)?;

                                        let mem_ptr =
                                            mt_ref.insert(entry.key, ipc.size).map_err(|e| {
                                                DispatcherError::AllocationFailed(format!(
                                                    "promote insert failed: {e}"
                                                ))
                                            })?;

                                        let start_lba = entry.offset / block_size as u64;

                                        let pipeline_result = unsafe {
                                            pipeline::pipelined_ssd_to_gpu_zero_copy(
                                                &*drive.block_dev_iface,
                                                &**gpu_ref,
                                                &streams,
                                                &channels,
                                                mem_ptr,
                                                ipc.address as *mut std::ffi::c_void,
                                                start_lba,
                                                total_bytes,
                                                chunk_size,
                                                queue_depth,
                                            )
                                        };

                                        pipeline_result?;

                                        let _ = dm_ref.remove(entry.key);
                                        dm_ref
                                            .create_memory_tier_entry(entry.key, mem_ptr, ipc.size)
                                            .map_err(|e| {
                                                DispatcherError::IoError(format!(
                                                    "promote re-register failed: {e}"
                                                ))
                                            })?;
                                        let _ =
                                            dm_ref.convert_to_storage(entry.key, entry.offset);
                                        let _ = dm_ref.release_write(entry.key);

                                        Ok(())
                                    })();

                                    batch_results.push((ci, res));
                                }

                                let _ = gpu_ref.destroy_stream(streams[0]);
                                let _ = gpu_ref.destroy_stream(streams[1]);

                                batch_results
                            });

                            thread_handles.push(handle);
                        }
                    }

                    // Collect results from all threads.
                    for handle in thread_handles {
                        let batch_results = handle.join().unwrap_or_else(|_| Vec::new());
                        for (ci, res) in batch_results {
                            results[cold_entries[ci].idx] = Some(res);
                        }
                    }
                });
            }
        }

        results.into_iter().map(|r| r.unwrap()).collect()
    }

    fn lookup_async(
        &self,
        key: CacheKey,
        ipc_handle: IpcHandle,
    ) -> Result<GpuStream, DispatcherError> {
        self.ensure_initialized()?;

        let dm = self
            .dispatch_map
            .get()
            .map_err(|_| DispatcherError::NotInitialized("dispatch_map not bound".into()))?;

        let mt = self
            .memory_tier
            .get()
            .map_err(|_| DispatcherError::NotInitialized("memory_tier not bound".into()))?;

        let result = dm.lookup(key);

        let gpu = self
            .gpu_services
            .get()
            .map_err(|_| DispatcherError::NotInitialized("gpu_services not bound".into()))?;

        let null_stream = GpuStream(std::ptr::null_mut());

        match result {
            Ok(lookup_result) => {
                use interfaces::LookupResult;
                match lookup_result {
                    LookupResult::NotExist => Err(DispatcherError::KeyNotFound(key)),
                    LookupResult::MismatchSize => {
                        let _ = dm.release_read(key);
                        Err(DispatcherError::InvalidParameter(
                            "size mismatch on lookup".into(),
                        ))
                    }
                    LookupResult::MemoryTier { pointer, size } => {
                        let copy_size = (ipc_handle.size as usize).min(size as usize);

                        // Use dedicated warm stream (lock-free AtomicU64 load).
                        let raw = self.warm_stream.load(Ordering::Acquire);
                        if raw != 0 {
                            let s = GpuStream(raw as *mut std::ffi::c_void);
                            gpu.memcpy_h2d_async(
                                pointer as *const std::ffi::c_void,
                                ipc_handle.address as *mut std::ffi::c_void,
                                copy_size,
                                s,
                            )
                            .map_err(|e| {
                                let _ = dm.release_read(key);
                                DispatcherError::IoError(format!(
                                    "GPU DMA copy (memory-tier→device) failed: {e}"
                                ))
                            })?;
                            let _ = dm.release_read(key);
                            mt.touch(key);
                            Ok(s)
                        } else {
                            // Fallback: sync copy via DmaBuffer wrapper.
                            let aligned = copy_size.next_multiple_of(4096).max(4096);
                            let temp_buf = unsafe {
                                DmaBuffer::from_raw(
                                    pointer as *mut std::ffi::c_void,
                                    aligned,
                                    noop_free,
                                    -1,
                                )
                            }
                            .map_err(|e| {
                                let _ = dm.release_read(key);
                                DispatcherError::IoError(format!("DmaBuffer wrap failed: {e}"))
                            })?;
                            let copy_result = gpu.dma_copy_to_device(
                                &temp_buf,
                                ipc_handle.address as *mut std::ffi::c_void,
                                copy_size,
                            );
                            std::mem::forget(temp_buf);
                            let _ = dm.release_read(key);
                            mt.touch(key);
                            copy_result.map_err(|e| {
                                DispatcherError::IoError(format!(
                                    "GPU DMA copy (memory-tier→device) failed: {e}"
                                ))
                            })?;
                            Ok(null_stream)
                        }
                    }
                    LookupResult::Staging { buffer } => {
                        let copy_result = gpu.dma_copy_to_device(
                            &buffer,
                            ipc_handle.address as *mut std::ffi::c_void,
                            ipc_handle.size as usize,
                        );
                        let _ = dm.release_read(key);
                        copy_result.map_err(|e| {
                            DispatcherError::IoError(format!(
                                "GPU DMA copy (staging→device) failed: {e}"
                            ))
                        })?;
                        Ok(null_stream)
                    }
                    LookupResult::BlockDevice { offset } => {
                        let _ = dm.release_read(key);
                        self.promote_and_serve(key, offset, &ipc_handle, &gpu, &dm, &mt)?;
                        Ok(null_stream)
                    }
                }
            }
            Err(_) => Err(DispatcherError::KeyNotFound(key)),
        }
    }

    fn check(&self, key: CacheKey) -> Result<bool, DispatcherError> {
        self.ensure_initialized()?;

        let dm = self
            .dispatch_map
            .get()
            .map_err(|_| DispatcherError::NotInitialized("dispatch_map not bound".into()))?;

        match dm.lookup(key) {
            Ok(result) => {
                use interfaces::LookupResult;
                let exists = !matches!(result, LookupResult::NotExist);
                if exists {
                    let _ = dm.release_read(key);
                }
                Ok(exists)
            }
            Err(_) => Ok(false),
        }
    }

    fn remove(&self, key: CacheKey) -> Result<(), DispatcherError> {
        self.ensure_initialized()?;

        let dm = self
            .dispatch_map
            .get()
            .map_err(|_| DispatcherError::NotInitialized("dispatch_map not bound".into()))?;

        // Lookup the entry to determine its location (waits for any active writer).
        let block_offset = match dm.lookup(key) {
            Ok(LookupResult::BlockDevice { offset }) => {
                let _ = dm.release_read(key);
                Some(offset)
            }
            Ok(_) => {
                let _ = dm.release_read(key);
                None
            }
            Err(_) => return Err(DispatcherError::KeyNotFound(key)),
        };

        // Remove from memory-tier if present.
        if let Ok(mt) = self.memory_tier.get() {
            let _ = mt.remove(key);
        }

        // Remove from dispatch-map (fails if another reference was taken in the
        // window after we released ours — acceptable race, caller can retry).
        dm.remove(key)
            .map_err(|_| DispatcherError::KeyNotFound(key))?;

        if let Some(offset) = block_offset {
            let drives = self.data_drives.read();
            let idx = Self::drive_index(key, drives.len().max(1));
            if let Some(drive) = drives.get(idx) {
                if let Some(iem) = query_interface!(drive.extent_mgr, IExtentManager) {
                    let _ = iem.remove_extent(offset);
                }
            }
        }

        Ok(())
    }

    fn populate(&self, key: CacheKey, ipc_handle: IpcHandle) -> Result<(), DispatcherError> {
        self.ensure_initialized()?;

        if ipc_handle.size == 0 {
            return Err(DispatcherError::InvalidParameter(
                "IPC handle size must be > 0".into(),
            ));
        }

        let dm = self
            .dispatch_map
            .get()
            .map_err(|_| DispatcherError::NotInitialized("dispatch_map not bound".into()))?;

        let mt = self
            .memory_tier
            .get()
            .map_err(|_| DispatcherError::NotInitialized("memory_tier not bound".into()))?;

        // Evict from memory-tier if needed to make space.
        Self::evict_for_space(&dm, &mt, ipc_handle.size)?;

        // Allocate a slot in the memory-tier.
        let mem_ptr = mt.insert(key, ipc_handle.size).map_err(|e| match e {
            interfaces::MemoryTierError::AlreadyExists(k) => DispatcherError::AlreadyExists(k),
            interfaces::MemoryTierError::PoolFull => {
                DispatcherError::AllocationFailed("memory-tier pool full after eviction".into())
            }
            other => DispatcherError::AllocationFailed(other.to_string()),
        })?;

        // Create a temporary DmaBuffer wrapping the memory-tier slot for GPU DMA.
        let aligned_size = (ipc_handle.size as usize).next_multiple_of(4096);
        // SAFETY: mem_ptr is valid for aligned_size bytes, owned by memory-tier.
        let temp_buf = unsafe {
            DmaBuffer::from_raw(
                mem_ptr as *mut std::ffi::c_void,
                aligned_size,
                noop_free,
                -1,
            )
        }
        .map_err(|e| {
            let _ = mt.remove(key);
            DispatcherError::AllocationFailed(format!("DmaBuffer wrap failed: {e}"))
        })?;

        let gpu = self
            .gpu_services
            .get()
            .map_err(|_| DispatcherError::NotInitialized("gpu_services not bound".into()))?;

        // DMA copy from GPU to memory-tier slot.
        gpu.dma_copy_to_host(
            ipc_handle.address as *const std::ffi::c_void,
            &temp_buf,
            ipc_handle.size as usize,
        )
        .map_err(|e| {
            let _ = mt.remove(key);
            DispatcherError::IoError(format!("GPU DMA copy failed: {e}"))
        })?;

        // Don't let the noop-free wrapper be dropped (it would call noop_free, which is fine,
        // but let's be explicit).
        std::mem::forget(temp_buf);

        // Register in dispatch-map as memory-tier entry.
        dm.create_memory_tier_entry(key, mem_ptr, ipc_handle.size)
            .map_err(|e| match e {
                interfaces::DispatchMapError::AlreadyExists(k) => {
                    let _ = mt.remove(key);
                    DispatcherError::AlreadyExists(k)
                }
                other => {
                    let _ = mt.remove(key);
                    DispatcherError::IoError(other.to_string())
                }
            })?;

        // Downgrade write ref to read ref for background writer.
        dm.downgrade_reference(key)
            .map_err(|e| DispatcherError::IoError(e.to_string()))?;

        // Enqueue background write-through to SSD.
        let num_drives = {
            let dd = self.data_drives.read();
            dd.len().max(1)
        };
        let guard = self.bg_writer.lock().unwrap();
        if let Some(ref writer) = *guard {
            let _ = writer.enqueue(WriteJob {
                key,
                size: ipc_handle.size,
                device_index: Self::drive_index(key, num_drives),
            });
        }

        Ok(())
    }

    fn prepare_store(&self, key: CacheKey, size: u32) -> Result<Arc<DmaBuffer>, DispatcherError> {
        self.ensure_initialized()?;
        self.log_info(&format!("dispatcher: prepare_store key={key} size={size}"));

        if size == 0 {
            return Err(DispatcherError::InvalidParameter("size must be > 0".into()));
        }

        let dm = self
            .dispatch_map
            .get()
            .map_err(|_| DispatcherError::NotInitialized("dispatch_map not bound".into()))?;

        // Register the key in the dispatch map (prevents duplicates, makes check() visible).
        // Uses create_staging as a lightweight reservation for the direct-write path.
        let _staging = dm.create_staging(key, 1).map_err(|e| match e {
            interfaces::DispatchMapError::AlreadyExists(k) => DispatcherError::AlreadyExists(k),
            other => DispatcherError::IoError(other.to_string()),
        })?;

        // Determine target drive and allocate extent.
        let drives = self.data_drives.read();
        let num_drives = drives.len().max(1);
        let drive_idx = Self::drive_index(key, num_drives);

        let (block_size, numa_node) = if let Some(drive) = drives.get(drive_idx) {
            (
                drive.block_dev_iface.block_size() as usize,
                drive.block_dev_iface.numa_node(),
            )
        } else {
            (4096, -1)
        };

        let extent_mgrs: Vec<Arc<ExtentManager>> =
            drives.iter().map(|d| Arc::clone(&d.extent_mgr)).collect();
        drop(drives);

        let aligned_size = (size as usize).next_multiple_of(block_size);

        // Reserve extent via extent manager (if available).
        let write_handle = if let Some(em) = extent_mgrs.get(drive_idx) {
            if let Some(iem) = query_interface!(em, IExtentManager) {
                match iem.reserve_extent(key, aligned_size as u32) {
                    Ok(wh) => Some(wh),
                    Err(e) => {
                        let _ = dm.remove(key);
                        return Err(DispatcherError::AllocationFailed(format!(
                            "reserve_extent failed: {e}"
                        )));
                    }
                }
            } else {
                None
            }
        } else {
            None
        };

        // Allocate DMA buffer for the caller to write into.
        let buf = match DmaBuffer::new(aligned_size, block_size, Some(numa_node)) {
            Ok(b) => b,
            Err(_) => {
                // Fallback for environments without SPDK DMA (e.g., staging-only mode).
                let ptr = unsafe { libc::aligned_alloc(block_size, aligned_size) };
                if ptr.is_null() {
                    let _ = dm.remove(key);
                    return Err(DispatcherError::AllocationFailed(
                        "aligned_alloc failed".into(),
                    ));
                }
                unsafe { std::ptr::write_bytes(ptr as *mut u8, 0, aligned_size) };
                unsafe {
                    DmaBuffer::from_raw(ptr, aligned_size, libc_free, -1).map_err(|e| {
                        let _ = dm.remove(key);
                        DispatcherError::AllocationFailed(format!(
                            "DMA buffer from_raw failed: {e}"
                        ))
                    })?
                }
            }
        };

        let buf = Arc::new(buf);

        // Store the pending write for later commit/cancel.
        if let Some(wh) = write_handle {
            self.pending_writes.lock().unwrap().insert(
                key,
                PendingWrite {
                    write_handle: wh,
                    buffer: Arc::clone(&buf),
                    size,
                    drive_idx,
                },
            );
        }

        Ok(buf)
    }

    fn commit_store(&self, key: CacheKey) -> Result<(), DispatcherError> {
        self.ensure_initialized()?;
        self.log_info(&format!("dispatcher: commit_store key={key}"));

        let pending = self
            .pending_writes
            .lock()
            .unwrap()
            .remove(&key)
            .ok_or(DispatcherError::KeyNotFound(key))?;

        let drives = self.data_drives.read();
        let drive = drives.get(pending.drive_idx).ok_or_else(|| {
            DispatcherError::IoError("data drive not available for commit".into())
        })?;

        let block_size = drive.block_dev_iface.block_size() as usize;
        let block_dev_iface = Arc::clone(&drive.block_dev_iface);
        drop(drives);

        let block_offset = pending.write_handle.extent_offset();
        let start_lba = block_offset / block_size as u64;
        let total_bytes = pending.size as usize;

        Self::write_buffer_to_ssd(&*block_dev_iface, &pending.buffer, start_lba, total_bytes)?;

        // Data written — publish extent and register in dispatch map.
        let _ = pending.write_handle.publish();

        let dm = self
            .dispatch_map
            .get()
            .map_err(|_| DispatcherError::NotInitialized("dispatch_map not bound".into()))?;

        dm.convert_to_storage(key, block_offset)
            .map_err(|e| DispatcherError::IoError(format!("convert_to_storage failed: {e}")))?;

        let _ = dm.release_write(key);

        Ok(())
    }

    fn cancel_store(&self, key: CacheKey) -> Result<(), DispatcherError> {
        self.ensure_initialized()?;
        self.log_info(&format!("dispatcher: cancel_store key={key}"));

        self.pending_writes
            .lock()
            .unwrap()
            .remove(&key)
            .ok_or(DispatcherError::KeyNotFound(key))?;

        // PendingWrite dropped here — WriteHandle::drop calls abort automatically.

        // Remove the dispatch map entry created by prepare_store.
        let dm = self
            .dispatch_map
            .get()
            .map_err(|_| DispatcherError::NotInitialized("dispatch_map not bound".into()))?;
        let _ = dm.remove(key);

        Ok(())
    }

    fn touch(&self, key: CacheKey) -> Result<(), DispatcherError> {
        self.ensure_initialized()?;

        let dm = self
            .dispatch_map
            .get()
            .map_err(|_| DispatcherError::NotInitialized("dispatch_map not bound".into()))?;

        dm.touch(key).map_err(|_| DispatcherError::KeyNotFound(key))
    }

    fn clear_memory_tier(&self) -> Result<usize, DispatcherError> {
        self.ensure_initialized()?;

        let dm = self
            .dispatch_map
            .get()
            .map_err(|_| DispatcherError::NotInitialized("dispatch_map not bound".into()))?;

        let mt = self
            .memory_tier
            .get()
            .map_err(|_| DispatcherError::NotInitialized("memory_tier not bound".into()))?;

        let mut count = 0;
        while let Some(key) = mt.evict_lru() {
            if dm.convert_memory_tier_to_block(key).is_err() {
                let _ = dm.remove(key);
            }
            count += 1;
        }
        Ok(count)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use component_core::query_interface;
    use std::collections::{HashMap, HashSet};
    use std::sync::Arc;
    use std::thread;

    use interfaces::{
        CacheKey, DispatchMapError, DmaAllocFn, DmaBuffer, GpuDeviceInfo, GpuDmaBuffer,
        GpuIpcHandle, GpuStream, IMemoryTier, LookupResult, MemoryTierError,
    };

    // -----------------------------------------------------------------------
    // Test infrastructure
    // -----------------------------------------------------------------------

    unsafe extern "C" fn dma_free(ptr: *mut std::ffi::c_void) {
        // SAFETY: ptr was allocated with libc::aligned_alloc in alloc_dma_buffer.
        unsafe { libc::free(ptr) };
    }

    fn alloc_dma_buffer(size: usize) -> Arc<DmaBuffer> {
        let sz = size.max(4096);
        let aligned_sz = sz.next_multiple_of(4096);
        let ptr = unsafe { libc::aligned_alloc(4096, aligned_sz) };
        assert!(
            !ptr.is_null(),
            "aligned_alloc failed for {aligned_sz} bytes"
        );
        unsafe { std::ptr::write_bytes(ptr as *mut u8, 0, aligned_sz) };
        let buf = unsafe { DmaBuffer::from_raw(ptr, aligned_sz, dma_free, -1) }.unwrap();
        Arc::new(buf)
    }

    // --- MockMemoryTier ---

    struct MockMtSlot {
        offset: usize,
        size: u32,
    }

    struct MockMemoryTier {
        inner: Mutex<MockMtInner>,
    }

    struct MockMtInner {
        pool: Vec<u8>,
        slots: HashMap<CacheKey, MockMtSlot>,
        used: usize,
        capacity: usize,
        fail_insert: bool,
    }

    impl MockMemoryTier {
        fn new(capacity: usize) -> Self {
            Self {
                inner: Mutex::new(MockMtInner {
                    pool: vec![0u8; capacity],
                    slots: HashMap::new(),
                    used: 0,
                    capacity,
                    fail_insert: false,
                }),
            }
        }

        fn with_fail_insert(capacity: usize) -> Self {
            Self {
                inner: Mutex::new(MockMtInner {
                    pool: vec![0u8; capacity],
                    slots: HashMap::new(),
                    used: 0,
                    capacity,
                    fail_insert: true,
                }),
            }
        }
    }

    impl IMemoryTier for MockMemoryTier {
        fn initialize(&self, _pool_size: usize) -> Result<(), MemoryTierError> {
            Ok(())
        }

        fn insert(&self, key: CacheKey, size: u32) -> Result<*mut u8, MemoryTierError> {
            let mut inner = self.inner.lock().unwrap();
            if inner.fail_insert {
                return Err(MemoryTierError::PoolFull);
            }
            if inner.slots.contains_key(&key) {
                return Err(MemoryTierError::AlreadyExists(key));
            }
            let aligned = (size as usize).next_multiple_of(4096);
            if inner.used + aligned > inner.capacity {
                return Err(MemoryTierError::PoolFull);
            }
            let offset = inner.used;
            inner.used += aligned;
            inner.slots.insert(key, MockMtSlot { offset, size });
            let ptr = unsafe { inner.pool.as_mut_ptr().add(offset) };
            Ok(ptr)
        }

        fn get(&self, key: CacheKey) -> Option<(*mut u8, u32)> {
            let inner = self.inner.lock().unwrap();
            inner.slots.get(&key).map(|slot| {
                let ptr = unsafe { (inner.pool.as_ptr() as *mut u8).add(slot.offset) };
                (ptr, slot.size)
            })
        }

        fn peek(&self, key: CacheKey) -> Option<(*mut u8, u32)> {
            self.get(key)
        }

        fn oldest_keys(&self, n: usize) -> Vec<CacheKey> {
            let inner = self.inner.lock().unwrap();
            inner.slots.keys().take(n).copied().collect()
        }

        fn evict_lru(&self) -> Option<CacheKey> {
            let mut inner = self.inner.lock().unwrap();
            let key = inner.slots.keys().next().copied()?;
            let slot = inner.slots.remove(&key).unwrap();
            let aligned = (slot.size as usize).next_multiple_of(4096);
            inner.used = inner.used.saturating_sub(aligned);
            Some(key)
        }

        fn remove(&self, key: CacheKey) -> Result<(), MemoryTierError> {
            let mut inner = self.inner.lock().unwrap();
            match inner.slots.remove(&key) {
                Some(slot) => {
                    let aligned = (slot.size as usize).next_multiple_of(4096);
                    inner.used = inner.used.saturating_sub(aligned);
                    Ok(())
                }
                None => Err(MemoryTierError::KeyNotFound(key)),
            }
        }

        fn touch(&self, _key: CacheKey) {}

        fn contains(&self, key: CacheKey) -> bool {
            self.inner.lock().unwrap().slots.contains_key(&key)
        }

        fn capacity(&self) -> usize {
            self.inner.lock().unwrap().capacity
        }

        fn used(&self) -> usize {
            self.inner.lock().unwrap().used
        }

        fn pool_info(&self) -> Option<(*mut u8, usize)> {
            let inner = self.inner.lock().unwrap();
            Some((inner.pool.as_ptr() as *mut u8, inner.capacity))
        }

        fn clear(&self) -> Result<usize, MemoryTierError> {
            let mut inner = self.inner.lock().unwrap();
            let count = inner.slots.len();
            inner.slots.clear();
            inner.used = 0;
            Ok(count)
        }
    }

    // --- MockDispatchMap ---

    enum MockEntryLocation {
        Staging {
            buffer: Arc<DmaBuffer>,
        },
        MemoryTier {
            pointer: *mut u8,
            size: u32,
            ssd_offset: Option<u64>,
        },
    }

    // SAFETY: pointers in MemoryTier refer to MockMemoryTier pool (test-only).
    unsafe impl Send for MockEntryLocation {}
    unsafe impl Sync for MockEntryLocation {}

    struct MockEntry {
        location: MockEntryLocation,
        write_ref: bool,
        read_refs: u32,
    }

    struct MockDmInner {
        entries: HashMap<CacheKey, MockEntry>,
        mismatch_keys: HashSet<CacheKey>,
    }

    struct MockDispatchMap {
        inner: Mutex<MockDmInner>,
    }

    impl MockDispatchMap {
        fn new() -> Self {
            Self {
                inner: Mutex::new(MockDmInner {
                    entries: HashMap::new(),
                    mismatch_keys: HashSet::new(),
                }),
            }
        }

        fn entry_count(&self) -> usize {
            self.inner.lock().unwrap().entries.len()
        }

        fn set_mismatch_key(&self, key: CacheKey) {
            self.inner.lock().unwrap().mismatch_keys.insert(key);
        }

        fn convert_entry_to_block(&self, key: CacheKey, offset: u64) {
            let mut inner = self.inner.lock().unwrap();
            if let Some(entry) = inner.entries.get_mut(&key) {
                entry.location = MockEntryLocation::MemoryTier {
                    pointer: std::ptr::null_mut(),
                    size: 0,
                    ssd_offset: Some(offset),
                };
            }
        }
    }

    impl IDispatchMap for MockDispatchMap {
        fn set_dma_alloc(&self, _alloc: DmaAllocFn) {}

        fn initialize(&self) -> Result<(), DispatchMapError> {
            Ok(())
        }

        fn create_staging(
            &self,
            key: CacheKey,
            size: u32,
        ) -> Result<Arc<DmaBuffer>, DispatchMapError> {
            let mut inner = self.inner.lock().unwrap();
            if inner.entries.contains_key(&key) {
                return Err(DispatchMapError::AlreadyExists(key));
            }
            let buffer = alloc_dma_buffer(size as usize * 4096);
            inner.entries.insert(
                key,
                MockEntry {
                    location: MockEntryLocation::Staging {
                        buffer: Arc::clone(&buffer),
                    },
                    write_ref: true,
                    read_refs: 0,
                },
            );
            Ok(buffer)
        }

        fn lookup(&self, key: CacheKey) -> Result<LookupResult, DispatchMapError> {
            let inner = self.inner.lock().unwrap();
            if inner.mismatch_keys.contains(&key) {
                return Ok(LookupResult::MismatchSize);
            }
            match inner.entries.get(&key) {
                None => Ok(LookupResult::NotExist),
                Some(entry) => match &entry.location {
                    MockEntryLocation::Staging { buffer } => Ok(LookupResult::Staging {
                        buffer: Arc::clone(buffer),
                    }),
                    MockEntryLocation::MemoryTier {
                        pointer,
                        size,
                        ssd_offset,
                    } => match ssd_offset {
                        Some(offset) if pointer.is_null() => {
                            Ok(LookupResult::BlockDevice { offset: *offset })
                        }
                        _ => Ok(LookupResult::MemoryTier {
                            pointer: *pointer,
                            size: *size,
                        }),
                    },
                },
            }
        }

        fn convert_to_storage(&self, key: CacheKey, offset: u64) -> Result<(), DispatchMapError> {
            let mut inner = self.inner.lock().unwrap();
            match inner.entries.get_mut(&key) {
                None => Err(DispatchMapError::KeyNotFound(key)),
                Some(entry) => {
                    match &mut entry.location {
                        MockEntryLocation::MemoryTier { ssd_offset, .. } => {
                            *ssd_offset = Some(offset);
                        }
                        MockEntryLocation::Staging { .. } => {
                            entry.location = MockEntryLocation::MemoryTier {
                                pointer: std::ptr::null_mut(),
                                size: 0,
                                ssd_offset: Some(offset),
                            };
                        }
                    }
                    Ok(())
                }
            }
        }

        fn take_read(&self, key: CacheKey) -> Result<(), DispatchMapError> {
            let mut inner = self.inner.lock().unwrap();
            match inner.entries.get_mut(&key) {
                None => Err(DispatchMapError::KeyNotFound(key)),
                Some(entry) => {
                    entry.read_refs += 1;
                    Ok(())
                }
            }
        }

        fn take_write(&self, key: CacheKey) -> Result<(), DispatchMapError> {
            let mut inner = self.inner.lock().unwrap();
            match inner.entries.get_mut(&key) {
                None => Err(DispatchMapError::KeyNotFound(key)),
                Some(entry) => {
                    if entry.write_ref {
                        return Err(DispatchMapError::ActiveReferences(key));
                    }
                    entry.write_ref = true;
                    Ok(())
                }
            }
        }

        fn release_read(&self, key: CacheKey) -> Result<(), DispatchMapError> {
            let mut inner = self.inner.lock().unwrap();
            match inner.entries.get_mut(&key) {
                None => Err(DispatchMapError::KeyNotFound(key)),
                Some(entry) => {
                    entry.read_refs = entry.read_refs.saturating_sub(1);
                    Ok(())
                }
            }
        }

        fn release_write(&self, key: CacheKey) -> Result<(), DispatchMapError> {
            let mut inner = self.inner.lock().unwrap();
            match inner.entries.get_mut(&key) {
                None => Err(DispatchMapError::KeyNotFound(key)),
                Some(entry) => {
                    entry.write_ref = false;
                    Ok(())
                }
            }
        }

        fn downgrade_reference(&self, key: CacheKey) -> Result<(), DispatchMapError> {
            let mut inner = self.inner.lock().unwrap();
            match inner.entries.get_mut(&key) {
                None => Err(DispatchMapError::NoWriteReference(key)),
                Some(entry) => {
                    entry.write_ref = false;
                    entry.read_refs += 1;
                    Ok(())
                }
            }
        }

        fn remove(&self, key: CacheKey) -> Result<(), DispatchMapError> {
            let mut inner = self.inner.lock().unwrap();
            match inner.entries.get(&key) {
                None => Err(DispatchMapError::KeyNotFound(key)),
                Some(entry) => {
                    if entry.read_refs > 0 || entry.write_ref {
                        return Err(DispatchMapError::ActiveReferences(key));
                    }
                    inner.entries.remove(&key);
                    Ok(())
                }
            }
        }

        fn touch(&self, key: CacheKey) -> Result<(), DispatchMapError> {
            let inner = self.inner.lock().unwrap();
            if inner.entries.contains_key(&key) {
                Ok(())
            } else {
                Err(DispatchMapError::KeyNotFound(key))
            }
        }

        fn oldest_keys(&self, n: usize) -> Vec<CacheKey> {
            let inner = self.inner.lock().unwrap();
            inner.entries.keys().copied().take(n).collect()
        }

        fn create_memory_tier_entry(
            &self,
            key: CacheKey,
            pointer: *mut u8,
            size: u32,
        ) -> Result<(), DispatchMapError> {
            let mut inner = self.inner.lock().unwrap();
            if inner.entries.contains_key(&key) {
                return Err(DispatchMapError::AlreadyExists(key));
            }
            inner.entries.insert(
                key,
                MockEntry {
                    location: MockEntryLocation::MemoryTier {
                        pointer,
                        size,
                        ssd_offset: None,
                    },
                    write_ref: true,
                    read_refs: 0,
                },
            );
            Ok(())
        }

        fn convert_memory_tier_to_block(&self, key: CacheKey) -> Result<(), DispatchMapError> {
            let mut inner = self.inner.lock().unwrap();
            match inner.entries.get_mut(&key) {
                None => Err(DispatchMapError::KeyNotFound(key)),
                Some(entry) => match &entry.location {
                    MockEntryLocation::MemoryTier {
                        ssd_offset: Some(offset),
                        ..
                    } => {
                        let off = *offset;
                        entry.location = MockEntryLocation::MemoryTier {
                            pointer: std::ptr::null_mut(),
                            size: 0,
                            ssd_offset: Some(off),
                        };
                        Ok(())
                    }
                    _ => Err(DispatchMapError::InvalidState("no ssd_offset set".into())),
                },
            }
        }

        fn is_evictable(&self, key: CacheKey) -> bool {
            let inner = self.inner.lock().unwrap();
            match inner.entries.get(&key) {
                Some(entry) => matches!(
                    entry.location,
                    MockEntryLocation::MemoryTier {
                        ssd_offset: Some(_),
                        ..
                    }
                ),
                None => false,
            }
        }

        fn recover_extent(
            &self,
            key: CacheKey,
            offset: u64,
            _size_blocks: u32,
        ) -> Result<(), DispatchMapError> {
            let mut inner = self.inner.lock().unwrap();
            if inner.entries.contains_key(&key) {
                return Err(DispatchMapError::AlreadyExists(key));
            }
            inner.entries.insert(
                key,
                MockEntry {
                    location: MockEntryLocation::MemoryTier {
                        pointer: std::ptr::null_mut(),
                        size: 0,
                        ssd_offset: Some(offset),
                    },
                    write_ref: false,
                    read_refs: 0,
                },
            );
            Ok(())
        }
    }

    struct MockLogger;

    impl ILogger for MockLogger {
        fn error(&self, _msg: &str) {}
        fn warn(&self, _msg: &str) {}
        fn info(&self, _msg: &str) {}
        fn debug(&self, _msg: &str) {}
    }

    struct MockGpuServices;

    impl IGpuServices for MockGpuServices {
        fn initialize(&self) -> Result<(), String> {
            Ok(())
        }
        fn shutdown(&self) -> Result<(), String> {
            Ok(())
        }
        fn get_devices(&self) -> Result<Vec<GpuDeviceInfo>, String> {
            Ok(vec![])
        }
        fn deserialize_ipc_handle(&self, _base64_payload: &str) -> Result<GpuIpcHandle, String> {
            Err("mock: not implemented".into())
        }
        fn verify_memory(&self, _handle: &GpuIpcHandle) -> Result<(), String> {
            Ok(())
        }
        fn pin_memory(&self, _handle: &GpuIpcHandle) -> Result<(), String> {
            Ok(())
        }
        fn unpin_memory(&self, _handle: &GpuIpcHandle) -> Result<(), String> {
            Ok(())
        }
        fn create_dma_buffer(&self, _handle: GpuIpcHandle) -> Result<GpuDmaBuffer, String> {
            Err("mock: not implemented".into())
        }
        fn dma_copy_to_host(
            &self,
            src: *const std::ffi::c_void,
            dst: &DmaBuffer,
            size: usize,
        ) -> Result<(), String> {
            // SAFETY: src is a valid host pointer (from IpcHandle) and dst is a valid DmaBuffer.
            unsafe {
                std::ptr::copy_nonoverlapping(src as *const u8, dst.as_ptr() as *mut u8, size);
            }
            Ok(())
        }
        fn dma_copy_to_device(
            &self,
            src: &DmaBuffer,
            dst: *mut std::ffi::c_void,
            size: usize,
        ) -> Result<(), String> {
            // SAFETY: src is a valid DmaBuffer and dst is a valid host pointer (from IpcHandle).
            unsafe {
                std::ptr::copy_nonoverlapping(src.as_ptr() as *const u8, dst as *mut u8, size);
            }
            Ok(())
        }
        fn prepare_memory_for_spdk(
            &self,
            _base64_payload: &str,
            _device_index: Option<u32>,
        ) -> Result<DmaBuffer, String> {
            Err("mock: not implemented".into())
        }
        fn create_stream(&self) -> Result<GpuStream, String> {
            Ok(GpuStream(0x1 as *mut std::ffi::c_void))
        }
        fn destroy_stream(&self, _stream: GpuStream) -> Result<(), String> {
            Ok(())
        }
        fn stream_synchronize(&self, _stream: GpuStream) -> Result<(), String> {
            Ok(())
        }
        fn dma_copy_to_device_async(
            &self,
            src: &DmaBuffer,
            dst: *mut std::ffi::c_void,
            size: usize,
            _stream: GpuStream,
        ) -> Result<(), String> {
            // SAFETY: In tests, both src and dst are valid host pointers.
            unsafe {
                std::ptr::copy_nonoverlapping(src.as_ptr() as *const u8, dst as *mut u8, size);
            }
            Ok(())
        }
        fn memcpy_h2d_async(
            &self,
            src: *const std::ffi::c_void,
            dst: *mut std::ffi::c_void,
            size: usize,
            _stream: GpuStream,
        ) -> Result<(), String> {
            unsafe {
                std::ptr::copy_nonoverlapping(src as *const u8, dst as *mut u8, size);
            }
            Ok(())
        }
        fn allocate_pinned_dma_buffer(&self, size: usize) -> Result<DmaBuffer, String> {
            DmaBuffer::new(size, 4096, None).map_err(|e| e.to_string())
        }
        fn register_host_memory(
            &self,
            _ptr: *mut std::ffi::c_void,
            _size: usize,
        ) -> Result<(), String> {
            Ok(())
        }
        fn unregister_host_memory(
            &self,
            _ptr: *mut std::ffi::c_void,
            _size: usize,
        ) -> Result<(), String> {
            Ok(())
        }
    }

    fn setup_initialized() -> (Arc<DispatcherComponent>, Arc<MockDispatchMap>) {
        let dm = Arc::new(MockDispatchMap::new());
        let logger: Arc<dyn ILogger + Send + Sync> = Arc::new(MockLogger);
        let gpu: Arc<dyn IGpuServices + Send + Sync> = Arc::new(MockGpuServices);
        let mt: Arc<dyn IMemoryTier + Send + Sync> = Arc::new(MockMemoryTier::new(1024 * 1024));
        let c = DispatcherComponent::new(
            AtomicBool::new(false),
            Mutex::new(None),
            Mutex::new(None),
            RwLock::new(Vec::new()),
            Mutex::new(HashMap::new()),
            RwLock::new(None),
            AtomicU64::new(0),
        );
        c.dispatch_map
            .connect(Arc::clone(&dm) as Arc<dyn IDispatchMap + Send + Sync>)
            .unwrap();
        c.logger.connect(logger).unwrap();
        c.gpu_services.connect(gpu).unwrap();
        c.memory_tier.connect(mt).unwrap();

        let d = query_interface!(c, IDispatcher).unwrap();
        d.initialize(DispatcherConfig {
            data_pci_addrs: vec!["0000:02:00.0".to_string()],
            ..Default::default()
        })
        .unwrap();

        (c, dm)
    }

    fn make_handle(buf: &mut [u8]) -> IpcHandle {
        IpcHandle {
            address: buf.as_mut_ptr(),
            size: buf.len() as u32,
        }
    }

    // -----------------------------------------------------------------------
    // Pre-initialization tests (existing)
    // -----------------------------------------------------------------------

    #[test]
    fn component_creation() {
        let _c = DispatcherComponent::new(
            AtomicBool::new(false),
            Mutex::new(None),
            Mutex::new(None),
            RwLock::new(Vec::new()),
            Mutex::new(HashMap::new()),
            RwLock::new(None),
            AtomicU64::new(0),
        );
    }

    #[test]
    fn query_idispatcher() {
        let c = DispatcherComponent::new(
            AtomicBool::new(false),
            Mutex::new(None),
            Mutex::new(None),
            RwLock::new(Vec::new()),
            Mutex::new(HashMap::new()),
            RwLock::new(None),
            AtomicU64::new(0),
        );
        let d = query_interface!(c, IDispatcher);
        assert!(d.is_some());
    }

    #[test]
    fn initialize_without_receptacles_fails() {
        let c = DispatcherComponent::new(
            AtomicBool::new(false),
            Mutex::new(None),
            Mutex::new(None),
            RwLock::new(Vec::new()),
            Mutex::new(HashMap::new()),
            RwLock::new(None),
            AtomicU64::new(0),
        );
        let d = query_interface!(c, IDispatcher).unwrap();
        let config = DispatcherConfig {
            data_pci_addrs: vec!["0000:02:00.0".to_string()],
            ..Default::default()
        };
        let err = d.initialize(config);
        assert!(matches!(err, Err(DispatcherError::NotInitialized(_))));
    }

    #[test]
    fn initialize_with_empty_pci_addrs_fails() {
        let c = DispatcherComponent::new(
            AtomicBool::new(false),
            Mutex::new(None),
            Mutex::new(None),
            RwLock::new(Vec::new()),
            Mutex::new(HashMap::new()),
            RwLock::new(None),
            AtomicU64::new(0),
        );
        let d = query_interface!(c, IDispatcher).unwrap();
        let config = DispatcherConfig {
            data_pci_addrs: vec![],
            ..Default::default()
        };
        // This will fail with NotInitialized since dispatch_map isn't bound
        let err = d.initialize(config);
        assert!(err.is_err());
    }

    #[test]
    fn lookup_before_initialize_fails() {
        let c = DispatcherComponent::new(
            AtomicBool::new(false),
            Mutex::new(None),
            Mutex::new(None),
            RwLock::new(Vec::new()),
            Mutex::new(HashMap::new()),
            RwLock::new(None),
            AtomicU64::new(0),
        );
        let d = query_interface!(c, IDispatcher).unwrap();
        let mut buf = vec![0u8; 4096];
        let handle = IpcHandle {
            address: buf.as_mut_ptr(),
            size: 4096,
        };
        let err = d.lookup(42, handle);
        assert!(matches!(err, Err(DispatcherError::NotInitialized(_))));
    }

    #[test]
    fn check_before_initialize_fails() {
        let c = DispatcherComponent::new(
            AtomicBool::new(false),
            Mutex::new(None),
            Mutex::new(None),
            RwLock::new(Vec::new()),
            Mutex::new(HashMap::new()),
            RwLock::new(None),
            AtomicU64::new(0),
        );
        let d = query_interface!(c, IDispatcher).unwrap();
        let err = d.check(42);
        assert!(matches!(err, Err(DispatcherError::NotInitialized(_))));
    }

    #[test]
    fn remove_before_initialize_fails() {
        let c = DispatcherComponent::new(
            AtomicBool::new(false),
            Mutex::new(None),
            Mutex::new(None),
            RwLock::new(Vec::new()),
            Mutex::new(HashMap::new()),
            RwLock::new(None),
            AtomicU64::new(0),
        );
        let d = query_interface!(c, IDispatcher).unwrap();
        let err = d.remove(42);
        assert!(matches!(err, Err(DispatcherError::NotInitialized(_))));
    }

    #[test]
    fn populate_before_initialize_fails() {
        let c = DispatcherComponent::new(
            AtomicBool::new(false),
            Mutex::new(None),
            Mutex::new(None),
            RwLock::new(Vec::new()),
            Mutex::new(HashMap::new()),
            RwLock::new(None),
            AtomicU64::new(0),
        );
        let d = query_interface!(c, IDispatcher).unwrap();
        let mut buf = vec![0u8; 4096];
        let handle = IpcHandle {
            address: buf.as_mut_ptr(),
            size: 4096,
        };
        let err = d.populate(42, handle);
        assert!(matches!(err, Err(DispatcherError::NotInitialized(_))));
    }

    #[test]
    fn populate_with_zero_size_fails() {
        let c = DispatcherComponent::new(
            AtomicBool::new(false),
            Mutex::new(None),
            Mutex::new(None),
            RwLock::new(Vec::new()),
            Mutex::new(HashMap::new()),
            RwLock::new(None),
            AtomicU64::new(0),
        );
        let d = query_interface!(c, IDispatcher).unwrap();
        // Even though not initialized, zero-size check comes after init check.
        // This test verifies the parameter validation exists in the code path.
        let mut buf = vec![0u8; 0];
        let handle = IpcHandle {
            address: buf.as_mut_ptr(),
            size: 0,
        };
        let err = d.populate(42, handle);
        // Will fail with NotInitialized since that check comes first
        assert!(err.is_err());
    }

    #[test]
    fn shutdown_without_initialize_succeeds() {
        let c = DispatcherComponent::new(
            AtomicBool::new(false),
            Mutex::new(None),
            Mutex::new(None),
            RwLock::new(Vec::new()),
            Mutex::new(HashMap::new()),
            RwLock::new(None),
            AtomicU64::new(0),
        );
        let d = query_interface!(c, IDispatcher).unwrap();
        assert!(d.shutdown().is_ok());
    }

    #[test]
    fn double_shutdown_succeeds() {
        let c = DispatcherComponent::new(
            AtomicBool::new(false),
            Mutex::new(None),
            Mutex::new(None),
            RwLock::new(Vec::new()),
            Mutex::new(HashMap::new()),
            RwLock::new(None),
            AtomicU64::new(0),
        );
        let d = query_interface!(c, IDispatcher).unwrap();
        assert!(d.shutdown().is_ok());
        assert!(d.shutdown().is_ok());
    }

    #[test]
    fn concurrent_pre_init_calls_from_multiple_threads() {
        let c = Arc::new(DispatcherComponent::new(
            AtomicBool::new(false),
            Mutex::new(None),
            Mutex::new(None),
            RwLock::new(Vec::new()),
            Mutex::new(HashMap::new()),
            RwLock::new(None),
            AtomicU64::new(0),
        ));

        let handles: Vec<_> = (0..4)
            .map(|_| {
                let comp = Arc::clone(&c);
                thread::spawn(move || {
                    let d = query_interface!(comp, IDispatcher).unwrap();
                    assert!(matches!(
                        d.check(1),
                        Err(DispatcherError::NotInitialized(_))
                    ));
                    assert!(matches!(
                        d.remove(1),
                        Err(DispatcherError::NotInitialized(_))
                    ));
                })
            })
            .collect();

        for h in handles {
            h.join().unwrap();
        }
    }

    // -----------------------------------------------------------------------
    // Initialized dispatcher tests (with mock dispatch map)
    // -----------------------------------------------------------------------

    #[test]
    fn initialize_with_dispatch_map_succeeds() {
        let (c, _dm) = setup_initialized();
        let d = query_interface!(c, IDispatcher).unwrap();
        assert!(d.shutdown().is_ok());
    }

    #[test]
    fn initialize_empty_addrs_with_dispatch_map() {
        let dm: Arc<dyn IDispatchMap + Send + Sync> = Arc::new(MockDispatchMap::new());
        let mt: Arc<dyn IMemoryTier + Send + Sync> = Arc::new(MockMemoryTier::new(1024 * 1024));
        let c = DispatcherComponent::new(
            AtomicBool::new(false),
            Mutex::new(None),
            Mutex::new(None),
            RwLock::new(Vec::new()),
            Mutex::new(HashMap::new()),
            RwLock::new(None),
            AtomicU64::new(0),
        );
        c.dispatch_map.connect(dm).unwrap();
        c.memory_tier.connect(mt).unwrap();

        let d = query_interface!(c, IDispatcher).unwrap();
        let config = DispatcherConfig {
            data_pci_addrs: vec![],
            ..Default::default()
        };
        let err = d.initialize(config);
        assert!(matches!(err, Err(DispatcherError::InvalidParameter(_))));
    }

    #[test]
    fn initialize_multiple_pci_addrs() {
        let dm: Arc<dyn IDispatchMap + Send + Sync> = Arc::new(MockDispatchMap::new());
        let logger: Arc<dyn ILogger + Send + Sync> = Arc::new(MockLogger);
        let mt: Arc<dyn IMemoryTier + Send + Sync> = Arc::new(MockMemoryTier::new(1024 * 1024));
        let c = DispatcherComponent::new(
            AtomicBool::new(false),
            Mutex::new(None),
            Mutex::new(None),
            RwLock::new(Vec::new()),
            Mutex::new(HashMap::new()),
            RwLock::new(None),
            AtomicU64::new(0),
        );
        c.dispatch_map.connect(dm).unwrap();
        c.logger.connect(logger).unwrap();
        c.memory_tier.connect(mt).unwrap();

        let d = query_interface!(c, IDispatcher).unwrap();
        d.initialize(DispatcherConfig {
            data_pci_addrs: vec![
                "0000:02:00.0".to_string(),
                "0000:03:00.0".to_string(),
                "0000:04:00.0".to_string(),
            ],
            ..Default::default()
        })
        .unwrap();
        d.shutdown().unwrap();
    }

    #[test]
    fn populate_succeeds_after_init() {
        let (c, dm) = setup_initialized();
        let d = query_interface!(c, IDispatcher).unwrap();
        let mut buf = vec![0u8; 4096];
        assert!(d.populate(1, make_handle(&mut buf)).is_ok());
        assert_eq!(dm.entry_count(), 1);
        d.shutdown().unwrap();
    }

    #[test]
    fn populate_zero_size_returns_invalid_parameter_after_init() {
        let (c, _dm) = setup_initialized();
        let d = query_interface!(c, IDispatcher).unwrap();
        let mut buf = vec![0u8; 0];
        let handle = IpcHandle {
            address: buf.as_mut_ptr(),
            size: 0,
        };
        let err = d.populate(1, handle);
        assert!(matches!(err, Err(DispatcherError::InvalidParameter(_))));
        d.shutdown().unwrap();
    }

    #[test]
    fn populate_duplicate_key_returns_already_exists() {
        let (c, _dm) = setup_initialized();
        let d = query_interface!(c, IDispatcher).unwrap();
        let mut buf1 = vec![0u8; 4096];
        d.populate(1, make_handle(&mut buf1)).unwrap();

        let mut buf2 = vec![0u8; 4096];
        let err = d.populate(1, make_handle(&mut buf2));
        assert!(matches!(err, Err(DispatcherError::AlreadyExists(1))));
        d.shutdown().unwrap();
    }

    #[test]
    fn populate_allocation_failure() {
        let dm: Arc<dyn IDispatchMap + Send + Sync> = Arc::new(MockDispatchMap::new());
        let logger: Arc<dyn ILogger + Send + Sync> = Arc::new(MockLogger);
        let gpu: Arc<dyn IGpuServices + Send + Sync> = Arc::new(MockGpuServices);
        let mt: Arc<dyn IMemoryTier + Send + Sync> =
            Arc::new(MockMemoryTier::with_fail_insert(1024 * 1024));
        let c = DispatcherComponent::new(
            AtomicBool::new(false),
            Mutex::new(None),
            Mutex::new(None),
            RwLock::new(Vec::new()),
            Mutex::new(HashMap::new()),
            RwLock::new(None),
            AtomicU64::new(0),
        );
        c.dispatch_map.connect(dm).unwrap();
        c.logger.connect(logger).unwrap();
        c.gpu_services.connect(gpu).unwrap();
        c.memory_tier.connect(mt).unwrap();

        let d = query_interface!(c, IDispatcher).unwrap();
        d.initialize(DispatcherConfig {
            data_pci_addrs: vec!["0000:02:00.0".to_string()],
            ..Default::default()
        })
        .unwrap();

        let mut buf = vec![0u8; 4096];
        let err = d.populate(1, make_handle(&mut buf));
        assert!(matches!(err, Err(DispatcherError::AllocationFailed(_))));
        d.shutdown().unwrap();
    }

    #[test]
    fn populate_non_block_aligned_size() {
        let (c, _dm) = setup_initialized();
        let d = query_interface!(c, IDispatcher).unwrap();
        let mut buf = vec![0u8; 5000];
        let handle = IpcHandle {
            address: buf.as_mut_ptr(),
            size: 5000,
        };
        assert!(d.populate(1, handle).is_ok());
        d.shutdown().unwrap();
    }

    #[test]
    fn populate_enqueues_many_writes() {
        let (c, dm) = setup_initialized();
        let d = query_interface!(c, IDispatcher).unwrap();

        for i in 0..100 {
            let mut buf = vec![0u8; 4096];
            d.populate(i, make_handle(&mut buf)).unwrap();
        }
        assert_eq!(dm.entry_count(), 100);
        d.shutdown().unwrap();
    }

    #[test]
    fn lookup_memory_tier_hit() {
        let (c, _dm) = setup_initialized();
        let d = query_interface!(c, IDispatcher).unwrap();

        let mut buf = vec![0xABu8; 4096];
        d.populate(1, make_handle(&mut buf)).unwrap();

        let mut buf2 = vec![0u8; 4096];
        assert!(d.lookup(1, make_handle(&mut buf2)).is_ok());
        // Verify GPU received the data (mock copies bytes directly).
        assert_eq!(buf2[0], 0xAB);
        d.shutdown().unwrap();
    }

    #[test]
    fn lookup_block_device_promote_without_hardware() {
        let (c, dm) = setup_initialized();
        let d = query_interface!(c, IDispatcher).unwrap();

        let mut buf = vec![0u8; 4096];
        d.populate(1, make_handle(&mut buf)).unwrap();

        // Simulate eviction: remove from memory-tier and convert dispatch-map to BlockDevice.
        let mt = c.memory_tier.get().unwrap();
        let _ = mt.remove(1);
        dm.convert_entry_to_block(1, 0x1000);

        // Without hardware, promote_and_serve enters the no-drives path
        // which copies zeros to GPU and re-registers the entry.
        let mut buf2 = vec![0u8; 4096];
        let result = d.lookup(1, make_handle(&mut buf2));
        assert!(
            result.is_ok(),
            "promote without hardware should succeed, got: {result:?}"
        );
        d.shutdown().unwrap();
    }

    #[test]
    fn lookup_key_not_found() {
        let (c, _dm) = setup_initialized();
        let d = query_interface!(c, IDispatcher).unwrap();
        let mut buf = vec![0u8; 4096];
        let err = d.lookup(999, make_handle(&mut buf));
        assert!(matches!(err, Err(DispatcherError::KeyNotFound(999))));
        d.shutdown().unwrap();
    }

    #[test]
    fn lookup_mismatch_size_returns_invalid_parameter() {
        let (c, dm) = setup_initialized();
        let d = query_interface!(c, IDispatcher).unwrap();

        let mut buf = vec![0u8; 4096];
        d.populate(1, make_handle(&mut buf)).unwrap();

        dm.set_mismatch_key(1);

        let mut buf2 = vec![0u8; 4096];
        let err = d.lookup(1, make_handle(&mut buf2));
        assert!(matches!(err, Err(DispatcherError::InvalidParameter(_))));
        d.shutdown().unwrap();
    }

    #[test]
    fn check_existing_returns_true() {
        let (c, _dm) = setup_initialized();
        let d = query_interface!(c, IDispatcher).unwrap();
        let mut buf = vec![0u8; 4096];
        d.populate(1, make_handle(&mut buf)).unwrap();
        assert!(d.check(1).unwrap());
        d.shutdown().unwrap();
    }

    #[test]
    fn check_nonexistent_returns_false() {
        let (c, _dm) = setup_initialized();
        let d = query_interface!(c, IDispatcher).unwrap();
        assert!(!d.check(999).unwrap());
        d.shutdown().unwrap();
    }

    #[test]
    fn remove_existing_succeeds() {
        let (c, dm) = setup_initialized();
        let d = query_interface!(c, IDispatcher).unwrap();
        let mut buf = vec![0u8; 4096];
        d.populate(1, make_handle(&mut buf)).unwrap();
        assert_eq!(dm.entry_count(), 1);
        assert!(d.remove(1).is_ok());
        assert_eq!(dm.entry_count(), 0);
        d.shutdown().unwrap();
    }

    #[test]
    fn remove_nonexistent_returns_key_not_found() {
        let (c, _dm) = setup_initialized();
        let d = query_interface!(c, IDispatcher).unwrap();
        let err = d.remove(999);
        assert!(matches!(err, Err(DispatcherError::KeyNotFound(999))));
        d.shutdown().unwrap();
    }

    #[test]
    fn full_lifecycle_populate_check_remove() {
        let (c, dm) = setup_initialized();
        let d = query_interface!(c, IDispatcher).unwrap();

        let mut buf = vec![0u8; 8192];
        d.populate(42, make_handle(&mut buf)).unwrap();
        assert_eq!(dm.entry_count(), 1);

        assert!(d.check(42).unwrap());
        assert!(!d.check(99).unwrap());

        assert!(d.remove(42).is_ok());
        assert_eq!(dm.entry_count(), 0);

        assert!(!d.check(42).unwrap());

        d.shutdown().unwrap();
    }

    #[test]
    fn operations_after_shutdown_fail() {
        let (c, _dm) = setup_initialized();
        let d = query_interface!(c, IDispatcher).unwrap();
        d.shutdown().unwrap();

        let mut buf = vec![0u8; 4096];
        assert!(matches!(
            d.populate(1, make_handle(&mut buf)),
            Err(DispatcherError::NotInitialized(_))
        ));
        assert!(matches!(
            d.check(1),
            Err(DispatcherError::NotInitialized(_))
        ));
        let mut buf2 = vec![0u8; 4096];
        assert!(matches!(
            d.lookup(1, make_handle(&mut buf2)),
            Err(DispatcherError::NotInitialized(_))
        ));
        assert!(matches!(
            d.remove(1),
            Err(DispatcherError::NotInitialized(_))
        ));
    }

    #[test]
    fn reinitialize_after_shutdown() {
        let (c, _dm) = setup_initialized();
        let d = query_interface!(c, IDispatcher).unwrap();
        d.shutdown().unwrap();

        d.initialize(DispatcherConfig {
            data_pci_addrs: vec!["0000:02:00.0".to_string()],
            ..Default::default()
        })
        .unwrap();

        assert!(!d.check(1).unwrap());
        d.shutdown().unwrap();
    }

    #[test]
    fn concurrent_checks_on_initialized_dispatcher() {
        let (c, _dm) = setup_initialized();

        let handles: Vec<_> = (0..8)
            .map(|i| {
                let comp = Arc::clone(&c);
                thread::spawn(move || {
                    let d = query_interface!(comp, IDispatcher).unwrap();
                    for k in 0..10 {
                        let result = d.check(i * 100 + k);
                        assert!(result.is_ok());
                        assert!(!result.unwrap());
                    }
                })
            })
            .collect();

        for h in handles {
            h.join().unwrap();
        }

        let d = query_interface!(c, IDispatcher).unwrap();
        d.shutdown().unwrap();
    }

    #[test]
    fn concurrent_populate_different_keys() {
        let (c, dm) = setup_initialized();

        let handles: Vec<_> = (0..4)
            .map(|t| {
                let comp = Arc::clone(&c);
                thread::spawn(move || {
                    let d = query_interface!(comp, IDispatcher).unwrap();
                    for i in 0..5 {
                        let key = t * 100 + i;
                        let mut buf = vec![0u8; 4096];
                        d.populate(key, make_handle(&mut buf)).unwrap();
                    }
                })
            })
            .collect();

        for h in handles {
            h.join().unwrap();
        }

        assert_eq!(dm.entry_count(), 20);

        let d = query_interface!(c, IDispatcher).unwrap();
        d.shutdown().unwrap();
    }

    // -----------------------------------------------------------------------
    // Eviction tests (memory-tier pool pressure)
    // -----------------------------------------------------------------------

    #[test]
    fn evict_for_space_evicts_when_pool_full() {
        let dm: Arc<dyn IDispatchMap + Send + Sync> = Arc::new(MockDispatchMap::new());
        // Small pool: 16 KiB total (can hold 4 × 4 KiB entries).
        let mt: Arc<dyn IMemoryTier + Send + Sync> = Arc::new(MockMemoryTier::new(16384));

        // Insert 4 entries into the memory-tier directly.
        for key in 0..4u64 {
            mt.insert(key, 4096).unwrap();
            dm.create_memory_tier_entry(key, std::ptr::null_mut(), 4096)
                .unwrap();
            dm.release_write(key).unwrap();
            // Set ssd_offset so convert_memory_tier_to_block can succeed.
            dm.convert_to_storage(key, key * 4096).unwrap();
        }

        // Pool is now full (16384 used). Trying to add 4096 more should evict.
        DispatcherComponent::evict_for_space(&dm, &mt, 4096).unwrap();

        // At least one entry was evicted from memory-tier.
        assert!(mt.used() + 4096 <= mt.capacity());
    }

    #[test]
    fn evict_for_space_noop_when_space_available() {
        let dm: Arc<dyn IDispatchMap + Send + Sync> = Arc::new(MockDispatchMap::new());
        let mt: Arc<dyn IMemoryTier + Send + Sync> = Arc::new(MockMemoryTier::new(1024 * 1024));

        // Insert one 4 KiB entry.
        mt.insert(0, 4096).unwrap();
        dm.create_memory_tier_entry(0, std::ptr::null_mut(), 4096)
            .unwrap();
        dm.release_write(0).unwrap();

        // Plenty of space, no eviction needed.
        DispatcherComponent::evict_for_space(&dm, &mt, 4096).unwrap();

        assert!(mt.contains(0), "entry should not be evicted");
    }

    #[test]
    fn populate_triggers_eviction_on_full_pool() {
        let dm = Arc::new(MockDispatchMap::new());
        let logger: Arc<dyn ILogger + Send + Sync> = Arc::new(MockLogger);
        let gpu: Arc<dyn IGpuServices + Send + Sync> = Arc::new(MockGpuServices);
        // Pool can hold exactly 2 × 4 KiB entries.
        let mt: Arc<dyn IMemoryTier + Send + Sync> = Arc::new(MockMemoryTier::new(8192));
        let c = DispatcherComponent::new(
            AtomicBool::new(false),
            Mutex::new(None),
            Mutex::new(None),
            RwLock::new(Vec::new()),
            Mutex::new(HashMap::new()),
            RwLock::new(None),
            AtomicU64::new(0),
        );
        c.dispatch_map
            .connect(Arc::clone(&dm) as Arc<dyn IDispatchMap + Send + Sync>)
            .unwrap();
        c.logger.connect(logger).unwrap();
        c.gpu_services.connect(gpu).unwrap();
        c.memory_tier.connect(mt).unwrap();

        let d = query_interface!(c, IDispatcher).unwrap();
        d.initialize(DispatcherConfig {
            data_pci_addrs: vec!["0000:02:00.0".to_string()],
            ..Default::default()
        })
        .unwrap();

        // Fill the pool with 2 entries.
        let mut buf = vec![0u8; 4096];
        d.populate(1, make_handle(&mut buf)).unwrap();
        let mut buf2 = vec![0u8; 4096];
        d.populate(2, make_handle(&mut buf2)).unwrap();

        // Third populate should trigger eviction of one entry and succeed.
        let mut buf3 = vec![0u8; 4096];
        d.populate(3, make_handle(&mut buf3)).unwrap();

        // Total entries in dispatch-map: at most 3 (one may have been converted to block).
        assert!(dm.entry_count() <= 3);

        d.shutdown().unwrap();
    }

    #[test]
    fn prepare_store_returns_dma_buffer() {
        let (c, _dm) = setup_initialized();
        let d = query_interface!(c, IDispatcher).unwrap();

        let dma_buf = d.prepare_store(99, 4096).unwrap();
        assert!(dma_buf.len() >= 4096);

        // The prepared key is visible via check().
        assert!(d.check(99).unwrap());

        // Duplicate prepare_store on the same key fails.
        let err = d.prepare_store(99, 4096);
        assert!(matches!(err, Err(DispatcherError::AlreadyExists(99))));

        d.shutdown().unwrap();
    }

    // -----------------------------------------------------------------------
    // Background SSD Evictor tests
    // -----------------------------------------------------------------------

    #[test]
    fn evictor_get_evictable_offset_block_device() {
        let dm = Arc::new(MockDispatchMap::new());
        let dm_iface: Arc<dyn IDispatchMap + Send + Sync> = Arc::clone(&dm) as _;

        // Insert an entry that looks like BlockDevice (null pointer + ssd_offset).
        dm.inner.lock().unwrap().entries.insert(
            1,
            MockEntry {
                location: MockEntryLocation::MemoryTier {
                    pointer: std::ptr::null_mut(),
                    size: 4096,
                    ssd_offset: Some(8192),
                },
                write_ref: false,
                read_refs: 0,
            },
        );

        let offset = crate::background::BackgroundEvictor::get_evictable_offset(&dm_iface, 1);
        assert_eq!(offset, Some(8192));
    }

    #[test]
    fn evictor_get_evictable_offset_skips_memory_tier() {
        let dm = Arc::new(MockDispatchMap::new());
        let dm_iface: Arc<dyn IDispatchMap + Send + Sync> = Arc::clone(&dm) as _;

        // Insert a MemoryTier entry (non-null pointer = still hot in DRAM).
        let mut buf = vec![0u8; 4096];
        dm.inner.lock().unwrap().entries.insert(
            2,
            MockEntry {
                location: MockEntryLocation::MemoryTier {
                    pointer: buf.as_mut_ptr(),
                    size: 4096,
                    ssd_offset: Some(16384),
                },
                write_ref: false,
                read_refs: 0,
            },
        );

        let offset = crate::background::BackgroundEvictor::get_evictable_offset(&dm_iface, 2);
        assert_eq!(offset, None, "memory-tier entries should not be evictable");
        std::mem::forget(buf);
    }

    #[test]
    fn evictor_get_evictable_offset_skips_nonexistent() {
        let dm = Arc::new(MockDispatchMap::new());
        let dm_iface: Arc<dyn IDispatchMap + Send + Sync> = Arc::clone(&dm) as _;

        let offset = crate::background::BackgroundEvictor::get_evictable_offset(&dm_iface, 99);
        assert_eq!(offset, None);
    }

    #[test]
    fn evictor_full_eviction_cycle() {
        let dm = Arc::new(MockDispatchMap::new());
        let dm_iface: Arc<dyn IDispatchMap + Send + Sync> = Arc::clone(&dm) as _;
        let mt = Arc::new(MockMemoryTier::new(1024 * 1024));
        let mt_iface: Arc<dyn IMemoryTier + Send + Sync> = Arc::clone(&mt) as _;

        // Insert 10 entries all in BlockDevice state (null pointer + ssd_offset).
        for key in 0..10u64 {
            dm.inner.lock().unwrap().entries.insert(
                key,
                MockEntry {
                    location: MockEntryLocation::MemoryTier {
                        pointer: std::ptr::null_mut(),
                        size: 4096,
                        ssd_offset: Some(key * 4096),
                    },
                    write_ref: false,
                    read_refs: 0,
                },
            );
        }

        assert_eq!(dm.entry_count(), 10);

        // Simulate evictor logic: get oldest keys, filter, remove.
        let candidates = dm_iface.oldest_keys(5);
        assert_eq!(candidates.len(), 5);

        for key in &candidates {
            let offset =
                crate::background::BackgroundEvictor::get_evictable_offset(&dm_iface, *key);
            assert!(offset.is_some(), "key {key} should be evictable");

            let _ = mt_iface.remove(*key);
            dm_iface.remove(*key).unwrap();
        }

        assert_eq!(
            dm.entry_count(),
            5,
            "5 entries should remain after evicting 5"
        );
    }

    #[test]
    fn evictor_skips_entries_with_active_references() {
        let dm = Arc::new(MockDispatchMap::new());
        let dm_iface: Arc<dyn IDispatchMap + Send + Sync> = Arc::clone(&dm) as _;

        // Insert a BlockDevice entry.
        dm.inner.lock().unwrap().entries.insert(
            1,
            MockEntry {
                location: MockEntryLocation::MemoryTier {
                    pointer: std::ptr::null_mut(),
                    size: 4096,
                    ssd_offset: Some(4096),
                },
                write_ref: false,
                read_refs: 0,
            },
        );

        // Take two read references — one simulates a concurrent reader,
        // the other will be consumed by get_evictable_offset's release_read.
        dm_iface.take_read(1).unwrap();
        dm_iface.take_read(1).unwrap();

        // get_evictable_offset sees BlockDevice and returns Some(offset),
        // releasing one read ref internally.
        let offset = crate::background::BackgroundEvictor::get_evictable_offset(&dm_iface, 1);
        assert_eq!(offset, Some(4096));

        // Remove fails because one read ref remains (the concurrent reader).
        let remove_result = dm_iface.remove(1);
        assert!(
            remove_result.is_err(),
            "remove should fail with active references"
        );

        // Entry still exists.
        assert!(dm.inner.lock().unwrap().entries.contains_key(&1));

        // Release the concurrent reader's ref and retry.
        dm_iface.release_read(1).unwrap();
        dm_iface.remove(1).unwrap();
        assert!(!dm.inner.lock().unwrap().entries.contains_key(&1));
    }

    #[test]
    fn evictor_start_and_shutdown() {
        let dm: Arc<dyn IDispatchMap + Send + Sync> = Arc::new(MockDispatchMap::new());
        let mt: Arc<dyn IMemoryTier + Send + Sync> = Arc::new(MockMemoryTier::new(1024 * 1024));

        let mut evictor = crate::background::BackgroundEvictor::start(
            dm,
            mt,
            vec![],
            crate::background::EvictorConfig {
                threshold: 0.9,
                low_watermark: 0.8,
                batch_size: 10,
                interval: std::time::Duration::from_millis(50),
            },
            None,
        );

        std::thread::sleep(std::time::Duration::from_millis(200));
        evictor.shutdown();
    }
}


// --- FILE: lib_extract.rs ---
// --- Section: imports and struct ---
//! Dispatcher component for the Certus storage system.
//!
//! Orchestrates cache operations (populate, lookup, check, remove) using
//! a DRAM memory-tier with LRU eviction and write-through to SSD.
//! Coordinates N data block devices with N extent managers for persistent storage.
//!
//! Provides the [`IDispatcher`] interface with receptacles for
//! [`ILogger`], [`IDispatchMap`], and [`IMemoryTier`].

mod background;
pub mod io_segmenter;
pub mod pipeline;

use parking_lot::RwLock;
use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;
use std::sync::Mutex;

use component_framework::define_component;
use interfaces::{
    CacheKey, ClientChannels, Command, Completion, DispatcherConfig, DispatcherError, DmaAllocFn,
    DmaBuffer, FormatParams, GpuStream, IBlockDevice, IBlockDeviceAdmin, IDispatchMap, IDispatcher,
    IExtentManager, IGpuServices, ILogger, IMemoryTier, IpcHandle, LookupResult, PciAddress,
    WriteHandle,
};

use block_device_spdk_nvme::BlockDeviceSpdkNvmeComponent;
use component_core::binding::bind;
use component_core::query_interface;
use extent_manager::ExtentManager;
use spdk_env::ISPDKEnv;

use crate::background::{BackgroundEvictor, BackgroundWriter, EvictorConfig, WriteJob};

/// A pending store awaiting commit or cancel.
///
/// Created by `prepare_store` and consumed by either `commit_store` (writes
/// the buffer to SSD and publishes the extent) or `cancel_store` (drops the
/// handle, which auto-aborts the reservation).
struct PendingWrite {
    /// Extent reservation handle; calling `publish()` commits, dropping aborts.
    write_handle: WriteHandle,
    /// DMA buffer the caller writes data into between prepare and commit.
    buffer: Arc<DmaBuffer>,
    /// Original (unaligned) data size in bytes.
    size: u32,
    /// Index into `data_drives` identifying the target SSD.
    drive_idx: usize,
}

/// Holds one (block-device, extent-manager) pair for a data drive.
#[allow(dead_code)]
struct DataDrive {
    _block_dev: Arc<dyn component_core::IUnknown + Send + Sync>,
    block_dev_admin: Arc<dyn IBlockDeviceAdmin + Send + Sync>,
    block_dev_iface: Arc<dyn IBlockDevice + Send + Sync>,
    extent_mgr: Arc<ExtentManager>,
    cached_channels: Option<ClientChannels>,
}

define_component! {
    pub DispatcherComponent {
        version: "0.1.0",
        provides: [IDispatcher],
        receptacles: {
            logger: ILogger,
            dispatch_map: IDispatchMap,
            gpu_services: IGpuServices,
            spdk_env: ISPDKEnv,
            memory_tier: IMemoryTier,
        },
        fields: {
            initialized: AtomicBool,
            bg_writer: Mutex<Option<BackgroundWriter>>,
            bg_evictor: Mutex<Option<BackgroundEvictor>>,
            data_drives: RwLock<Vec<DataDrive>>,
            pending_writes: Mutex<HashMap<CacheKey, PendingWrite>>,
            pipeline_ring: RwLock<Option<pipeline::PipelineRing>>,
            warm_stream: AtomicU64,
        },
    }
}

unsafe extern "C" fn libc_free(ptr: *mut std::ffi::c_void) {
    unsafe { libc::free(ptr) };
}

/// No-op free function for temporary DmaBuffer wrappers around memory-tier pointers.
/// The memory-tier component owns the memory; this wrapper must not free it.
unsafe extern "C" fn noop_free(_ptr: *mut std::ffi::c_void) {}

impl DispatcherComponent {
    fn log_info(&self, msg: &str) {
        if let Ok(logger) = self.logger.get() {
            logger.info(msg);
        }
    }

    #[allow(dead_code)]

// --- Section: promote_and_serve (single-object path) ---
    fn promote_and_serve(
        &self,
        key: CacheKey,
        offset: u64,
        ipc_handle: &IpcHandle,
        gpu: &Arc<dyn IGpuServices + Send + Sync>,
        dm: &Arc<dyn IDispatchMap + Send + Sync>,
        mt: &Arc<dyn IMemoryTier + Send + Sync>,
    ) -> Result<(), DispatcherError> {
        let total_bytes = ipc_handle.size as usize;

        // Evict if needed to make space.
        Self::evict_for_space(dm, mt, ipc_handle.size)?;

        // Insert into memory-tier.
        let mem_ptr = mt.insert(key, ipc_handle.size).map_err(|e| {
            DispatcherError::AllocationFailed(format!("promote insert failed: {e}"))
        })?;

        // Read from SSD into memory-tier using pipelined reader.
        let drives = self.data_drives.read();
        if drives.is_empty() {
            // No hardware: just copy zeros to GPU (test/staging-only mode).
            let aligned = total_bytes.next_multiple_of(4096).max(4096);
            let temp_buf = unsafe {
                DmaBuffer::from_raw(mem_ptr as *mut std::ffi::c_void, aligned, noop_free, -1)
            }
            .map_err(|e| DispatcherError::IoError(format!("DmaBuffer wrap failed: {e}")))?;
            let result = gpu.dma_copy_to_device(
                &temp_buf,
                ipc_handle.address as *mut std::ffi::c_void,
                total_bytes,
            );
            std::mem::forget(temp_buf);
            // Register promoted entry in dispatch-map.
            let _ = dm.create_memory_tier_entry(key, mem_ptr, ipc_handle.size);
            let _ = dm.release_write(key);
            return result.map_err(|e| {
                DispatcherError::IoError(format!("GPU DMA copy (promote) failed: {e}"))
            });
        }

        let idx = Self::drive_index(key, drives.len());
        let drive = &drives[idx];
        let block_size = drive.block_dev_iface.block_size();
        let start_lba = offset / block_size as u64;
        let block_dev = Arc::clone(&drive.block_dev_iface);

        // Use cached channels if available, otherwise create new ones.
        let channels = match &drive.cached_channels {
            Some(ch) => ch,
            None => {
                drop(drives);
                return Err(DispatcherError::IoError(
                    "no cached channels for drive".into(),
                ));
            }
        };

        // Zero-copy pipelined reader: NVMe → memory-tier slot → GPU (no intermediate ring copy).
        // SAFETY: mem_ptr is a valid, CUDA-pinned, SPDK-registered memory-tier slot.
        // ipc_handle.address is a valid GPU destination pointer.
        let ring_guard = self.pipeline_ring.read();
        let ring_ref = ring_guard
            .as_ref()
            .ok_or_else(|| DispatcherError::NotInitialized("pipeline ring not allocated".into()))?;
        unsafe {
            pipeline::pipelined_ssd_to_gpu_zero_copy(
                &*block_dev,
                &**gpu,
                &ring_ref.streams,
                channels,
                mem_ptr,
                ipc_handle.address as *mut std::ffi::c_void,
                start_lba,
                total_bytes,
                ring_ref.chunk_size,
                16,
            )?;
        }
        drop(ring_guard);
        drop(drives);

        // Update dispatch-map: remove old BlockDevice entry and create fresh MemoryTier.
        // Since we released the read ref before calling this method, we can remove
        // and re-register.
        let _ = dm.remove(key);

// --- Section: PipelineRing initialization ---
                self.log_info(&format!(
                    "dispatcher: dispatch-map recovered {recovered} extents from disk ({elapsed:.2?})"
                ));
            }

            // Pre-allocate pipeline ring for promote_and_serve (CUDA-pinned + SPDK-registered).
            if let Ok(gpu) = self.gpu_services.get() {
                let chunk_size = {
                    let dd = self.data_drives.read();
                    dd.first()
                        .map(|d| d.block_dev_iface.max_transfer_size() as usize)
                        .unwrap_or(131072)
                };
                match pipeline::PipelineRing::new(&*gpu, chunk_size) {
                    Ok(ring) => {
                        *self.pipeline_ring.write() = Some(ring);
                    }
                    Err(e) => {
                        self.log_info(&format!(
                            "pipeline ring allocation failed (non-fatal): {e:?}"
                        ));

// --- Section: batch_lookup (hot path — queue allocation + pipeline call) ---
    fn batch_lookup(
        &self,
        entries: &[(CacheKey, IpcHandle)],
    ) -> Vec<Result<(), DispatcherError>> {
        if entries.is_empty() {
            return Vec::new();
        }

        let init_check = self.ensure_initialized();
        if let Err(e) = init_check {
            return entries.iter().map(|_| Err(e.clone())).collect();
        }

        let dm = match self.dispatch_map.get() {
            Ok(dm) => dm,
            Err(_) => {
                let e = DispatcherError::NotInitialized("dispatch_map not bound".into());
                return entries.iter().map(|_| Err(e.clone())).collect();
            }
        };
        let mt = match self.memory_tier.get() {
            Ok(mt) => mt,
            Err(_) => {
                let e = DispatcherError::NotInitialized("memory_tier not bound".into());
                return entries.iter().map(|_| Err(e.clone())).collect();
            }
        };
        let gpu = match self.gpu_services.get() {
            Ok(gpu) => gpu,
            Err(_) => {
                let e = DispatcherError::NotInitialized("gpu_services not bound".into());
                return entries.iter().map(|_| Err(e.clone())).collect();
            }
        };

        let mut results: Vec<Option<Result<(), DispatcherError>>> = vec![None; entries.len()];

        // Classify entries and handle fast paths inline.
        struct ColdEntry {
            idx: usize,
            key: CacheKey,
            offset: u64,
            ipc_handle_addr: *mut u8,
            ipc_handle_size: u32,
        }
        // SAFETY: ColdEntry contains a raw pointer from IpcHandle (GPU device pointer).
        // These pointers are valid across threads — CUDA IPC handles are designed for
        // cross-process/thread use. We only read the pointer value to pass to CUDA APIs.
        unsafe impl Send for ColdEntry {}
        unsafe impl Sync for ColdEntry {}

        let mut cold_entries: Vec<ColdEntry> = Vec::new();

        for (i, (key, ipc_handle)) in entries.iter().enumerate() {
            let key = *key;
            match dm.lookup(key) {
                Ok(lookup_result) => match lookup_result {
                    LookupResult::NotExist => {
                        results[i] = Some(Err(DispatcherError::KeyNotFound(key)));
                    }
                    LookupResult::MismatchSize => {
                        let _ = dm.release_read(key);
                        results[i] = Some(Err(DispatcherError::InvalidParameter(
                            "size mismatch on lookup".into(),
                        )));
                    }
                    LookupResult::MemoryTier { pointer, size } => {
                        let copy_size = (ipc_handle.size as usize).min(size as usize);
                        let raw = self.warm_stream.load(Ordering::Acquire);
                        let res = if raw != 0 {
                            let s = GpuStream(raw as *mut std::ffi::c_void);
                            gpu.memcpy_h2d_async(
                                pointer as *const std::ffi::c_void,
                                ipc_handle.address as *mut std::ffi::c_void,
                                copy_size,
                                s,
                            )
                            .map_err(|e| {
                                DispatcherError::IoError(format!(
                                    "GPU DMA copy (memory-tier→device) failed: {e}"
                                ))
                            })
                            .and_then(|_| {
                                gpu.stream_synchronize(s).map_err(|e| {
                                    DispatcherError::IoError(format!(
                                        "stream_synchronize failed: {e}"
                                    ))
                                })
                            })
                        } else {
                            let aligned = copy_size.next_multiple_of(4096).max(4096);
                            let temp_buf = unsafe {
                                DmaBuffer::from_raw(
                                    pointer as *mut std::ffi::c_void,
                                    aligned,
                                    noop_free,
                                    -1,
                                )
                            }
                            .map_err(|e| {
                                DispatcherError::IoError(format!("DmaBuffer wrap failed: {e}"))
                            });
                            match temp_buf {
                                Ok(buf) => {
                                    let r = gpu.dma_copy_to_device(
                                        &buf,
                                        ipc_handle.address as *mut std::ffi::c_void,
                                        copy_size,
                                    )
                                    .map_err(|e| {
                                        DispatcherError::IoError(format!(
                                            "GPU DMA copy (memory-tier→device) failed: {e}"
                                        ))
                                    });
                                    std::mem::forget(buf);
                                    r
                                }
                                Err(e) => Err(e),
                            }
                        };
                        let _ = dm.release_read(key);
                        mt.touch(key);
                        results[i] = Some(res);
                    }
                    LookupResult::Staging { buffer } => {
                        let res = gpu
                            .dma_copy_to_device(
                                &buffer,
                                ipc_handle.address as *mut std::ffi::c_void,
                                ipc_handle.size as usize,
                            )
                            .map_err(|e| {
                                DispatcherError::IoError(format!(
                                    "GPU DMA copy (staging→device) failed: {e}"
                                ))
                            });
                        let _ = dm.release_read(key);
                        results[i] = Some(res);
                    }
                    LookupResult::BlockDevice { offset } => {
                        let _ = dm.release_read(key);
                        cold_entries.push(ColdEntry {
                            idx: i,
                            key,
                            offset,
                            ipc_handle_addr: ipc_handle.address,
                            ipc_handle_size: ipc_handle.size,
                        });
                    }
                },
                Err(_) => {
                    results[i] = Some(Err(DispatcherError::KeyNotFound(key)));
                }
            }
        }

        // Promote cold entries in parallel — multiple queue threads per drive.
        // Each thread gets its own NVMe queue pair and CUDA streams, enabling
        // concurrent reads on the same physical drive.
        if !cold_entries.is_empty() {
            const MAX_QUEUES_PER_DRIVE: usize = 2;

            let chunk_size = {
                let ring_guard = self.pipeline_ring.read();
                ring_guard.as_ref().map_or(131072, |r| r.chunk_size)
            };

            let drives = self.data_drives.read();
            let num_drives = drives.len();

            if num_drives == 0 {
                for entry in &cold_entries {
                    Self::evict_for_space(&dm, &mt, entry.ipc_handle_size).ok();
                    let res = mt.insert(entry.key, entry.ipc_handle_size).map(|mem_ptr| {
                        let _ = dm.create_memory_tier_entry(entry.key, mem_ptr, entry.ipc_handle_size);
                        let _ = dm.release_write(entry.key);
                    }).map_err(|e| {
                        DispatcherError::AllocationFailed(format!("promote insert failed: {e}"))
                    });
                    results[entry.idx] = Some(res);
                }
            } else {
                // Group cold entries by target drive.
                let mut per_drive: Vec<Vec<usize>> = vec![Vec::new(); num_drives];
                for (ci, entry) in cold_entries.iter().enumerate() {
                    let drive_idx = Self::drive_index(entry.key, num_drives);
                    per_drive[drive_idx].push(ci);
                }

                std::thread::scope(|s| {
                    let mut thread_handles: Vec<
                        std::thread::ScopedJoinHandle<Vec<(usize, Result<(), DispatcherError>)>>,
                    > = Vec::new();

                    for (drive_idx, entry_indices) in per_drive.iter().enumerate() {
                        if entry_indices.is_empty() {
                            continue;
                        }

                        // Split this drive's entries across multiple queue threads.
                        let num_queues = MAX_QUEUES_PER_DRIVE.min(entry_indices.len());
                        let chunks: Vec<&[usize]> = entry_indices
                            .chunks((entry_indices.len() + num_queues - 1) / num_queues)
                            .collect();

                        let queue_depth = 16 / num_queues;

                        for chunk in chunks {
                            let dm_ref = &dm;
                            let mt_ref = &mt;
                            let gpu_ref = &gpu;
                            let drives_ref = &drives;
                            let cold_ref = &cold_entries;
                            let indices = chunk.to_vec();

                            let handle = s.spawn(move || {
                                let drive = &drives_ref[drive_idx];
                                let block_size = drive.block_dev_iface.block_size();

                                let channels =
                                    drive.block_dev_iface.connect_client().map_err(|e| {
                                        DispatcherError::IoError(format!(
                                            "connect_client failed: {e}"
                                        ))
                                    });
                                let streams_result = gpu_ref.create_stream().and_then(|a| {
                                    gpu_ref.create_stream().map(|b| [a, b]).map_err(|e| {
                                        let _ = gpu_ref.destroy_stream(a);
                                        e
                                    })
                                });

                                let mut batch_results: Vec<(usize, Result<(), DispatcherError>)> =
                                    Vec::with_capacity(indices.len());

                                let (channels, streams) = match (channels, streams_result) {
                                    (Ok(ch), Ok(st)) => (ch, st),
                                    (Err(e), _) => {
                                        for &ci in &indices {
                                            batch_results.push((ci, Err(e.clone())));
                                        }
                                        return batch_results;
                                    }
                                    (_, Err(e)) => {
                                        let err = DispatcherError::IoError(format!(
                                            "create_stream failed: {e}"
                                        ));
                                        for &ci in &indices {
                                            batch_results.push((ci, Err(err.clone())));
                                        }
                                        return batch_results;
                                    }
                                };

                                for &ci in &indices {
                                    let entry = &cold_ref[ci];
                                    let ipc = IpcHandle {
                                        address: entry.ipc_handle_addr,
                                        size: entry.ipc_handle_size,
                                    };
                                    let total_bytes = ipc.size as usize;

                                    let res = (|| -> Result<(), DispatcherError> {
                                        Self::evict_for_space(dm_ref, mt_ref, ipc.size)?;

                                        let mem_ptr =
                                            mt_ref.insert(entry.key, ipc.size).map_err(|e| {
                                                DispatcherError::AllocationFailed(format!(
                                                    "promote insert failed: {e}"
                                                ))
                                            })?;

                                        let start_lba = entry.offset / block_size as u64;

                                        let pipeline_result = unsafe {
                                            pipeline::pipelined_ssd_to_gpu_zero_copy(
                                                &*drive.block_dev_iface,
                                                &**gpu_ref,
                                                &streams,
                                                &channels,
                                                mem_ptr,
                                                ipc.address as *mut std::ffi::c_void,
                                                start_lba,
                                                total_bytes,
                                                chunk_size,
                                                queue_depth,
                                            )
                                        };

                                        pipeline_result?;

                                        let _ = dm_ref.remove(entry.key);
                                        dm_ref
                                            .create_memory_tier_entry(entry.key, mem_ptr, ipc.size)
                                            .map_err(|e| {
                                                DispatcherError::IoError(format!(
                                                    "promote re-register failed: {e}"
                                                ))
                                            })?;
                                        let _ =
                                            dm_ref.convert_to_storage(entry.key, entry.offset);
                                        let _ = dm_ref.release_write(entry.key);

                                        Ok(())
                                    })();

                                    batch_results.push((ci, res));
                                }

                                let _ = gpu_ref.destroy_stream(streams[0]);
                                let _ = gpu_ref.destroy_stream(streams[1]);

                                batch_results
                            });


// --- Section: fallback path ---
                        })?;
                        Ok(null_stream)
                    }
                    LookupResult::BlockDevice { offset } => {
                        let _ = dm.release_read(key);
                        self.promote_and_serve(key, offset, &ipc_handle, &gpu, &dm, &mt)?;
                        Ok(null_stream)
                    }
                }
            }
            Err(_) => Err(DispatcherError::KeyNotFound(key)),
        }
    }

    fn check(&self, key: CacheKey) -> Result<bool, DispatcherError> {
        self.ensure_initialized()?;

        let dm = self
            .dispatch_map
            .get()
            .map_err(|_| DispatcherError::NotInitialized("dispatch_map not bound".into()))?;