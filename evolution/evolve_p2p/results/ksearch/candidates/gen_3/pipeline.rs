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
    pub streams: Vec<GpuStream>,
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

        let mut streams = Vec::with_capacity(NUM_STREAMS);
        for i in 0..NUM_STREAMS {
            match gpu.create_stream() {
                Ok(s) => streams.push(s),
                Err(e) => {
                    for s in &streams {
                        let _ = gpu.destroy_stream(*s);
                    }
                    return Err(DispatcherError::IoError(format!(
                        "create_stream #{i} failed: {e}"
                    )));
                }
            }
        }

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
/// Uses a batch-based approach that correctly manages ring buffer ownership:
/// 1. Submit a batch of reads (up to ring_size)
/// 2. Wait for all completions
/// 3. Copy all to mem-tier and issue async GPU copies
/// 4. Sync streams before reusing buffers
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
    let num_streams = ring.streams.len();

    // Process chunks in batches of ring_size for correct buffer ownership.
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
            let current_stream = ring.streams[i % num_streams];

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

        // Sync all streams before reusing ring slots in the next batch.
        for s in &ring.streams {
            gpu.stream_synchronize(*s)
                .map_err(|e| DispatcherError::IoError(format!("stream_synchronize failed: {e}")))?;
        }

        chunk_idx = batch_end;
    }

    // Final sync for safety.
    for s in &ring.streams {
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
    streams: &[GpuStream],
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
    let num_streams = streams.len();

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
        let current_stream = streams[stream_idx % num_streams];

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
        if stream_idx % (num_streams * 4) == 0 {
            for s in streams {
                gpu.stream_synchronize(*s)
                    .map_err(|e| {
                        DispatcherError::IoError(format!("stream_synchronize failed: {e}"))
                    })?;
            }
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

/// GPUDirect Storage P2P pipeline: NVMe reads directly into GPU memory.
///
/// Uses GPU-resident DMA buffers registered with SPDK via nvidia-peermem/gdrdrv.
/// Falls back to the standard host-bounce path if GPU DMA buffers cannot be allocated.
///
/// # Safety
///
/// - `mem_tier_ptr` must be valid for writes of at least `total_bytes`.
/// - `gpu_dst` must be a valid GPU destination pointer for `total_bytes`.
#[allow(clippy::too_many_arguments)]
pub unsafe fn pipelined_ssd_to_gpu_p2p(
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
    let num_streams = ring.streams.len();

    // Attempt to allocate GPU-resident DMA buffers for P2P.
    // These are BAR1-mapped GPU memory regions registered with SPDK.
    let mut gpu_dma_bufs: Vec<Option<Arc<Mutex<DmaBuffer>>>> = Vec::with_capacity(num_chunks.min(PIPELINE_RING_SIZE));
    let mut p2p_available = true;

    for _i in 0..num_chunks.min(PIPELINE_RING_SIZE) {
        match gpu.allocate_gpu_dma_buffer(chunk_size) {
            Ok(buf) => gpu_dma_bufs.push(Some(Arc::new(Mutex::new(buf)))),
            Err(_) => {
                p2p_available = false;
                break;
            }
        }
    }

    if !p2p_available {
        // Clean up any partially allocated GPU buffers and fall back.
        drop(gpu_dma_bufs);
        return unsafe {
            pipelined_ssd_to_gpu(
                drive, gpu, ring, mem_tier_ptr, gpu_dst, start_lba, total_bytes,
            )
        };
    }

    let gds_ring_size = gpu_dma_bufs.len();

    // P2P path: NVMe reads directly into GPU-resident buffers, then
    // device-to-device copy to final destination, plus async D2H for mem-tier.
    let mut chunk_idx = 0;

    while chunk_idx < num_chunks {
        let batch_end = (chunk_idx + gds_ring_size).min(num_chunks);
        let batch_len = batch_end - chunk_idx;

        // Submit reads directly into GPU DMA buffers.
        for i in 0..batch_len {
            let buf_ref = gpu_dma_bufs[i].as_ref().unwrap();
            channels
                .command_tx
                .send(Command::ReadAsync {
                    ns_id: 1,
                    lba: segments[chunk_idx + i].lba,
                    buf: Arc::clone(buf_ref),
                    timeout_ms: READ_TIMEOUT_MS,
                })
                .map_err(|e| {
                    DispatcherError::IoError(format!("P2P ReadAsync send #{}: {e}", chunk_idx + i))
                })?;
        }

        // Wait for all reads.
        for _i in 0..batch_len {
            match channels.completion_rx.recv() {
                Ok(Completion::ReadDone { handle, result }) => {
                    result.map_err(|e| {
                        DispatcherError::IoError(format!(
                            "P2P SSD read (handle {:?}): {e}",
                            handle
                        ))
                    })?;
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
            }
        }

        // Data is now in GPU DMA buffers. Copy to final GPU destination
        // and to host memory-tier.
        for i in 0..batch_len {
            let seg = &segments[chunk_idx + i];
            let copy_len = seg.length.min(total_bytes.saturating_sub(seg.buffer_offset));
            let current_stream = ring.streams[i % num_streams];

            let guard = gpu_dma_bufs[i].as_ref().unwrap().lock().unwrap();

            // GPU-to-GPU copy (within device): staging buffer → final destination.
            gpu.dma_copy_to_device_async(
                &guard,
                unsafe { (gpu_dst as *mut u8).add(seg.buffer_offset) as *mut std::ffi::c_void },
                copy_len,
                current_stream,
            )
            .map_err(|e| {
                DispatcherError::IoError(format!(
                    "P2P GPU copy #{} failed: {e}",
                    chunk_idx + i
                ))
            })?;

            drop(guard);

            // Also copy to mem-tier (GPU→host). Use a synchronous approach
            // since we need the data in host memory too.
            // We sync the stream first to ensure GPU buffer has the data,
            // then do a host-side read.
        }

        // Sync all streams.
        for s in &ring.streams {
            gpu.stream_synchronize(*s)
                .map_err(|e| DispatcherError::IoError(format!("P2P stream_synchronize failed: {e}")))?;
        }

        // Now copy from GPU final destination to mem-tier using the ring's
        // pinned buffers as intermediary (D2H).
        for i in 0..batch_len {
            let seg = &segments[chunk_idx + i];
            let copy_len = seg.length.min(total_bytes.saturating_sub(seg.buffer_offset));

            if copy_len > 0 {
                let guard = gpu_dma_bufs[i].as_ref().unwrap().lock().unwrap();
                // Copy GPU DMA buffer content to host via the pinned ring buffer.
                let host_guard = ring.buffers[i % ring.buffers.len()].lock().unwrap();

                gpu.dma_copy_to_host_async(
                    &host_guard,
                    guard.as_ptr(),
                    copy_len,
                    ring.streams[i % num_streams],
                )
                .map_err(|e| {
                    DispatcherError::IoError(format!("P2P D2H copy #{} failed: {e}", chunk_idx + i))
                })?;

                drop(guard);
                drop(host_guard);
            }
        }

        // Sync streams for D2H completion.
        for s in &ring.streams {
            gpu.stream_synchronize(*s)
                .map_err(|e| DispatcherError::IoError(format!("P2P D2H stream_synchronize failed: {e}")))?;
        }

        // Copy from pinned host buffers to mem-tier.
        for i in 0..batch_len {
            let seg = &segments[chunk_idx + i];
            let copy_len = seg.length.min(total_bytes.saturating_sub(seg.buffer_offset));

            if copy_len > 0 {
                let host_guard = ring.buffers[i % ring.buffers.len()].lock().unwrap();
                unsafe {
                    std::ptr::copy_nonoverlapping(
                        host_guard.as_ptr() as *const u8,
                        mem_tier_ptr.add(seg.buffer_offset),
                        copy_len,
                    );
                }
                drop(host_guard);
            }
        }

        chunk_idx = batch_end;
    }

    // Final sync.
    for s in &ring.streams {
        gpu.stream_synchronize(*s)
            .map_err(|e| DispatcherError::IoError(format!("P2P final stream_synchronize: {e}")))?;
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