// --- FILE: pipeline.rs ---
//! Ring-buffer pipelined reader for SSD→DRAM→GPU transfers.
//!
//! Uses async NVMe reads with multiple in-flight commands and multiple CUDA
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
/// 32 buffers to maintain deep pipeline overlap with NVMe queue depth.
pub const PIPELINE_RING_SIZE: usize = 32;

/// Number of CUDA streams for overlapped H2D copies.
/// 8 streams keeps PCIe bus saturated with concurrent H2D transfers.
pub const NUM_STREAMS: usize = 8;

/// Timeout for async NVMe read operations (ms).
const READ_TIMEOUT_MS: u64 = 5000;

/// Pre-allocated ring of CUDA-pinned + SPDK-registered DMA buffers and CUDA streams.
///
/// Constructed once and reused across multiple `pipelined_ssd_to_gpu` calls
/// to avoid per-call `cudaHostAlloc`/`spdk_mem_register` overhead.
pub struct PipelineRing {
    pub buffers: Vec<Arc<Mutex<DmaBuffer>>>,
    pub streams: [GpuStream; NUM_STREAMS],
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

        let mut streams_vec = Vec::with_capacity(NUM_STREAMS);
        for i in 0..NUM_STREAMS {
            match gpu.create_stream() {
                Ok(s) => streams_vec.push(s),
                Err(e) => {
                    for s in &streams_vec {
                        let _ = gpu.destroy_stream(*s);
                    }
                    return Err(DispatcherError::IoError(format!(
                        "create_stream {i} failed: {e}"
                    )));
                }
            }
        }

        let streams: [GpuStream; NUM_STREAMS] = streams_vec.try_into().unwrap();

        Ok(Self {
            buffers,
            streams,
            chunk_size,
        })
    }

    /// Destroy CUDA streams. Buffers are freed on drop via their DmaBuffer free_fn.
    pub fn destroy(self, gpu: &dyn IGpuServices) {
        for s in &self.streams {
            let _ = gpu.destroy_stream(*s);
        }
    }
}

/// Pipeline-read from SSD into a memory-tier slot while streaming chunks to GPU.
///
/// Uses a pre-allocated [`PipelineRing`] to avoid per-call allocation overhead.
///
/// Algorithm:
/// 1. Prime the ring with `min(ring_size, num_chunks)` async NVMe reads
/// 2. For each completion:
///    a. memcpy ring buffer → memory-tier slot (CPU, immediate)
///    b. Issue cudaMemcpyAsync on stream[completed % NUM_STREAMS]
///    c. Submit next NVMe read into the freed slot
/// 3. Sync all streams
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

    // We use a sliding-window approach: submit up to ring_size reads,
    // then as each completes, do the H2D copy and submit the next read.
    // Track which ring slot maps to which segment index.
    let mut slot_to_seg: Vec<usize> = vec![0; ring_size];
    let mut inflight: std::collections::VecDeque<usize> =
        std::collections::VecDeque::with_capacity(ring_size);

    // Track which stream each slot's H2D copy was issued on, so we can
    // sync that stream before reusing the slot for a new NVMe read.
    let mut slot_last_stream: Vec<Option<usize>> = vec![None; ring_size];

    // Prime the pipeline with initial reads.
    for i in 0..ring_size {
        slot_to_seg[i] = i;
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
    let mut completed_count = 0usize;

    // Process completions in FIFO order (NVMe queue pair guarantees ordering).
    while completed_count < num_chunks {
        // Wait for the oldest in-flight read to finish.
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

        let seg_idx = slot_to_seg[slot_idx];
        let seg = &segments[seg_idx];
        let copy_len = seg.length.min(total_bytes.saturating_sub(seg.buffer_offset));
        let stream_index = completed_count % NUM_STREAMS;
        let current_stream = streams[stream_index];

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
                "GPU async DMA copy #{} failed: {e}",
                seg_idx
            ))
        })?;

        drop(guard);
        slot_last_stream[slot_idx] = Some(stream_index);
        completed_count += 1;

        // Submit next read into this now-free ring slot.
        if next_to_submit < num_chunks {
            // Ensure the H2D copy on this slot's stream has completed before
            // we overwrite the buffer with a new NVMe read.
            if let Some(s_idx) = slot_last_stream[slot_idx] {
                gpu.stream_synchronize(streams[s_idx]).map_err(|e| {
                    DispatcherError::IoError(format!("stream_synchronize slot reuse: {e}"))
                })?;
            }

            slot_to_seg[slot_idx] = next_to_submit;
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
    }

    // Sync all streams to ensure all GPU copies are complete.
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
    streams: &[GpuStream; NUM_STREAMS],
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
            let ptr = mem_tier_ptr.add(seg.buffer_offset) as *mut std::ffi::c_void;
            let buf_size = seg.length.next_multiple_of(block_size);
            let buf = DmaBuffer::from_raw(ptr, buf_size, noop_free, -1)
                .map_err(|e| {
                    DispatcherError::AllocationFailed(format!("DmaBuffer wrap chunk: {e}"))
                })?;
            Ok(Arc::new(Mutex::new(buf)))
        })
        .collect::<Result<Vec<_>, DispatcherError>>()?;

    let max_inflight = max_queue_depth.min(num_chunks);

    // Track in-flight segment indices in submission order (FIFO queue).
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

        // NVMe read for seg_idx is complete — GPU H2D copy can start now.
        let seg = &segments[seg_idx];
        let copy_len = seg.length.min(total_bytes.saturating_sub(seg.buffer_offset));
        let current_stream = streams[stream_idx % NUM_STREAMS];

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
    }

    // Sync all streams to ensure all GPU copies are complete.
    for s in streams {
        gpu.stream_synchronize(*s)
            .map_err(|e| DispatcherError::IoError(format!("final stream_synchronize: {e}")))?;
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
        assert!(size <= 64);
    }

    #[test]
    fn num_streams_is_reasonable() {
        assert!(NUM_STREAMS >= 2);
        assert!(NUM_STREAMS <= 16);
    }
}

// --- FILE: lib.rs ---
//! Dispatcher buffer management and promote_and_serve entry point.
//!
//! Manages the memory-tier ring allocation, pipeline ring lifecycle,
//! and orchestrates the SSD→DRAM→GPU data path.

use std::sync::{Arc, Mutex, Once};

use interfaces::{
    DmaBuffer, DispatcherError, IBlockDevice, IGpuServices,
};

use crate::pipeline::{PipelineRing, NUM_STREAMS, PIPELINE_RING_SIZE};

/// Default chunk size for pipeline transfers (4 MiB for optimal PCIe throughput).
pub const DEFAULT_CHUNK_SIZE: usize = 4 * 1024 * 1024;

/// Global pipeline ring singleton for reuse across calls.
static RING_INIT: Once = Once::new();
static mut GLOBAL_RING: Option<PipelineRing> = None;

/// Get or initialize the global pipeline ring.
///
/// # Safety
/// Must be called from a single-threaded context or protected by external sync.
pub unsafe fn get_or_init_ring(gpu: &dyn IGpuServices) -> Result<&'static PipelineRing, DispatcherError> {
    let mut init_err: Option<DispatcherError> = None;

    RING_INIT.call_once(|| {
        match PipelineRing::new(gpu, DEFAULT_CHUNK_SIZE) {
            Ok(ring) => {
                GLOBAL_RING = Some(ring);
            }
            Err(e) => {
                init_err = Some(e);
            }
        }
    });

    if let Some(e) = init_err {
        return Err(e);
    }

    GLOBAL_RING.as_ref().ok_or_else(|| {
        DispatcherError::AllocationFailed("pipeline ring not initialized".into())
    })
}

/// Promote data from SSD to GPU memory, serving it for inference.
///
/// This is the main entry point for the data transfer path:
/// 1. Allocates/reuses pipeline ring buffers
/// 2. Reads data from NVMe SSD into host DRAM (memory-tier slot)
/// 3. Streams data to GPU via async H2D copies
///
/// # Safety
/// - `mem_tier_ptr` must be valid for writes of `total_bytes` (aligned to block size)
/// - `gpu_dst` must be a valid GPU memory pointer for `total_bytes`
pub unsafe fn promote_and_serve(
    drive: &dyn IBlockDevice,
    gpu: &dyn IGpuServices,
    mem_tier_ptr: *mut u8,
    gpu_dst: *mut std::ffi::c_void,
    start_lba: u64,
    total_bytes: usize,
) -> Result<(), DispatcherError> {
    let ring = get_or_init_ring(gpu)?;

    crate::pipeline::pipelined_ssd_to_gpu(
        drive,
        gpu,
        ring,
        mem_tier_ptr,
        gpu_dst,
        start_lba,
        total_bytes,
    )
}

/// Promote data using the zero-copy path (requires registered memory-tier).
///
/// # Safety
/// - `mem_tier_ptr` must be CUDA-pinned + SPDK-registered
/// - `gpu_dst` must be a valid GPU memory pointer
#[allow(clippy::too_many_arguments)]
pub unsafe fn promote_and_serve_zero_copy(
    drive: &dyn IBlockDevice,
    gpu: &dyn IGpuServices,
    mem_tier_ptr: *mut u8,
    gpu_dst: *mut std::ffi::c_void,
    start_lba: u64,
    total_bytes: usize,
    chunk_size: Option<usize>,
    max_queue_depth: Option<usize>,
) -> Result<(), DispatcherError> {
    let chunk = chunk_size.unwrap_or(DEFAULT_CHUNK_SIZE);
    let qd = max_queue_depth.unwrap_or(PIPELINE_RING_SIZE);

    // Create streams for zero-copy path
    let mut streams_vec = Vec::with_capacity(NUM_STREAMS);
    for i in 0..NUM_STREAMS {
        match gpu.create_stream() {
            Ok(s) => streams_vec.push(s),
            Err(e) => {
                for s in &streams_vec {
                    let _ = gpu.destroy_stream(*s);
                }
                return Err(DispatcherError::IoError(format!(
                    "create_stream {i} failed: {e}"
                )));
            }
        }
    }
    let streams: [interfaces::GpuStream; NUM_STREAMS] = streams_vec.try_into().unwrap();

    let channels = drive
        .connect_client()
        .map_err(|e| DispatcherError::IoError(format!("connect_client failed: {e}")))?;

    let result = crate::pipeline::pipelined_ssd_to_gpu_zero_copy(
        drive,
        gpu,
        &streams,
        &channels,
        mem_tier_ptr,
        gpu_dst,
        start_lba,
        total_bytes,
        chunk,
        qd,
    );

    // Cleanup streams
    for s in &streams {
        let _ = gpu.destroy_stream(*s);
    }

    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_chunk_size_is_4mib() {
        assert_eq!(DEFAULT_CHUNK_SIZE, 4 * 1024 * 1024);
    }
}

// --- FILE: dma.rs ---
//! DMA buffer creation functions for various memory types.
//!
//! Provides helper functions to allocate DMA buffers suitable for
//! SPDK NVMe operations and CUDA H2D transfers.

use interfaces::{DmaBuffer, DispatcherError, IGpuServices};

/// Allocate a CUDA-pinned + SPDK-registered DMA buffer.
///
/// This buffer is suitable for both NVMe DMA targets and as a source
/// for cudaMemcpyAsync H2D transfers. Uses `cudaHostAlloc` with
/// `cudaHostAllocMapped | cudaHostAllocPortable` flags and registers
/// the resulting memory with SPDK via `spdk_mem_register`.
///
/// # Arguments
/// * `gpu` - GPU services interface for CUDA allocation
/// * `size` - Buffer size in bytes (will be aligned to page boundary)
///
/// # Returns
/// A `DmaBuffer` that is both CUDA-pinned and SPDK-registered.
pub fn allocate_pinned_dma_buffer(
    gpu: &dyn IGpuServices,
    size: usize,
) -> Result<DmaBuffer, DispatcherError> {
    // Align size to 4KiB page boundary for optimal DMA performance
    let aligned_size = size.next_multiple_of(4096);

    gpu.allocate_pinned_dma_buffer(aligned_size)
        .map_err(|e| DispatcherError::AllocationFailed(format!("pinned DMA buffer: {e}")))
}

/// Allocate a GPU device memory buffer.
///
/// Returns a device pointer suitable as the destination for H2D copies.
///
/// # Arguments
/// * `gpu` - GPU services interface
/// * `size` - Buffer size in bytes
pub fn allocate_gpu_buffer(
    gpu: &dyn IGpuServices,
    size: usize,
) -> Result<*mut std::ffi::c_void, DispatcherError> {
    let aligned_size = size.next_multiple_of(4096);

    gpu.allocate_device_memory(aligned_size)
        .map_err(|e| DispatcherError::AllocationFailed(format!("GPU device buffer: {e}")))
}

/// Register an existing host memory region for CUDA pinning and SPDK DMA.
///
/// This is used for the memory-tier pool: large pre-allocated regions
/// that need to be both CUDA-registered (for async H2D) and SPDK-registered
/// (for NVMe DMA targets in zero-copy mode).
///
/// # Safety
/// - `ptr` must be a valid pointer to `size` bytes of allocated memory
/// - The memory must remain allocated for the lifetime of the registration
pub unsafe fn register_host_memory(
    gpu: &dyn IGpuServices,
    ptr: *mut u8,
    size: usize,
) -> Result<(), DispatcherError> {
    gpu.register_host_memory(ptr as *mut std::ffi::c_void, size)
        .map_err(|e| DispatcherError::AllocationFailed(format!("register host memory: {e}")))
}

/// Unregister a previously registered host memory region.
///
/// # Safety
/// - `ptr` must have been previously registered with `register_host_memory`
pub unsafe fn unregister_host_memory(
    gpu: &dyn IGpuServices,
    ptr: *mut u8,
    size: usize,
) -> Result<(), DispatcherError> {
    gpu.unregister_host_memory(ptr as *mut std::ffi::c_void, size)
        .map_err(|e| DispatcherError::AllocationFailed(format!("unregister host memory: {e}")))
}

/// Create a batch of pinned DMA buffers for ring-buffer usage.
///
/// Allocates `count` buffers of `size` bytes each, all CUDA-pinned
/// and SPDK-registered.
pub fn allocate_ring_buffers(
    gpu: &dyn IGpuServices,
    count: usize,
    size: usize,
) -> Result<Vec<DmaBuffer>, DispatcherError> {
    let mut buffers = Vec::with_capacity(count);
    for i in 0..count {
        match allocate_pinned_dma_buffer(gpu, size) {
            Ok(buf) => buffers.push(buf),
            Err(e) => {
                // Drop already-allocated buffers (their free_fn handles cleanup)
                drop(buffers);
                return Err(DispatcherError::AllocationFailed(format!(
                    "ring buffer {i}/{count}: {e}"
                )));
            }
        }
    }
    Ok(buffers)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn alignment_works() {
        assert_eq!(4000usize.next_multiple_of(4096), 4096);
        assert_eq!(4096usize.next_multiple_of(4096), 4096);
        assert_eq!(8192usize.next_multiple_of(4096), 8192);
    }
}