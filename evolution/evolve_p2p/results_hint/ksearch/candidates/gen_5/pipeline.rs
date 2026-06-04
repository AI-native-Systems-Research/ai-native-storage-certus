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
pub struct PipelineRing {
    pub buffers: Vec<Arc<Mutex<DmaBuffer>>>,
    pub streams: [GpuStream; 2],
    pub chunk_size: usize,
}

impl PipelineRing {
    /// Allocate a new pipeline ring with CUDA-pinned, SPDK-registered buffers.
    pub fn new(gpu: &dyn IGpuServices, chunk_size: usize) -> Result<Self, DispatcherError> {
        let stream_a = gpu
            .create_stream()
            .map_err(|e| DispatcherError::IoError(format!("create_stream failed: {e}")))?;
        let stream_b = gpu.create_stream().map_err(|e| {
            let _ = gpu.destroy_stream(stream_a);
            DispatcherError::IoError(format!("create_stream failed: {e}"))
        })?;

        let buffers: Vec<Arc<Mutex<DmaBuffer>>> = (0..PIPELINE_RING_SIZE)
            .map(|_| {
                gpu.allocate_pinned_dma_buffer(chunk_size)
                    .map(|b| Arc::new(Mutex::new(b)))
                    .map_err(|e| {
                        DispatcherError::AllocationFailed(format!("pipeline ring buffer: {e}"))
                    })
            })
            .collect::<Result<Vec<_>, _>>()?;

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

/// No-op free function for DmaBuffer wrappers over memory regions we don't own.
unsafe extern "C" fn noop_free(_ptr: *mut std::ffi::c_void) {}

/// Pipeline-read from SSD into GPU memory via P2P when available, with memory-tier copy.
///
/// # P2P Path (feature = "p2p")
/// NVMe DMAs directly into GPU BAR1 memory (the final gpu_dst), completely
/// bypassing host DRAM for the SSD→GPU path. The memory-tier copy is done
/// by reading back from the BAR1-mapped address (CPU-accessible via PCIe).
///
/// # Non-P2P Path
/// Traditional: SSD → host ring buffer → memcpy to mem_tier + async H2D to GPU.
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
    let streams = &ring.streams;

    // ===== P2P PATH =====
    // DMA directly into gpu_dst segments. No ring buffer bounce, no H2D copy.
    #[cfg(feature = "p2p")]
    {
        // Create DmaBuffer wrappers for each segment of gpu_dst.
        // These allow NVMe to DMA directly into GPU BAR1 memory.
        let gpu_chunk_bufs: Vec<Arc<Mutex<DmaBuffer>>> = segments
            .iter()
            .map(|seg| {
                let ptr = (gpu_dst as *mut u8).add(seg.buffer_offset) as *mut std::ffi::c_void;
                let buf_size = seg.length.next_multiple_of(block_size);
                gpu_services::dma::create_spdk_dma_buffer_from_gpu_bar(ptr, buf_size)
                    .map(|b| Arc::new(Mutex::new(b)))
                    .map_err(|e| {
                        DispatcherError::AllocationFailed(format!(
                            "P2P DmaBuffer for gpu_dst segment: {e}"
                        ))
                    })
            })
            .collect::<Result<Vec<_>, DispatcherError>>()?;

        let max_inflight = PIPELINE_RING_SIZE.min(num_chunks);

        let mut inflight: std::collections::VecDeque<usize> =
            std::collections::VecDeque::with_capacity(max_inflight);

        // Prime the sliding window.
        for i in 0..max_inflight {
            channels
                .command_tx
                .send(Command::ReadAsync {
                    ns_id: 1,
                    lba: segments[i].lba,
                    buf: Arc::clone(&gpu_chunk_bufs[i]),
                    timeout_ms: READ_TIMEOUT_MS,
                })
                .map_err(|e| DispatcherError::IoError(format!("ReadAsync send #{i}: {e}")))?;
            inflight.push_back(i);
        }

        let mut next_to_submit = max_inflight;

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

            // Submit next read immediately to keep NVMe queue full.
            if next_to_submit < num_chunks {
                channels
                    .command_tx
                    .send(Command::ReadAsync {
                        ns_id: 1,
                        lba: segments[next_to_submit].lba,
                        buf: Arc::clone(&gpu_chunk_bufs[next_to_submit]),
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

            // Data is now in GPU memory via P2P DMA. Copy to memory-tier
            // by reading from the BAR1-mapped GPU address (CPU-accessible).
            let seg = &segments[seg_idx];
            let copy_len = seg.length.min(total_bytes.saturating_sub(seg.buffer_offset));
            if copy_len > 0 {
                let guard = gpu_chunk_bufs[seg_idx].lock().unwrap();
                std::ptr::copy_nonoverlapping(
                    guard.as_ptr() as *const u8,
                    mem_tier_ptr.add(seg.buffer_offset),
                    copy_len,
                );
                drop(guard);
            }
        }

        // Forget DmaBuffer wrappers (noop free internally, avoid cleanup issues).
        for buf in gpu_chunk_bufs {
            if let Ok(inner) = Arc::try_unwrap(buf) {
                std::mem::forget(inner.into_inner().unwrap());
            }
        }

        return Ok(());
    }

    // ===== NON-P2P PATH (fallback) =====
    #[cfg(not(feature = "p2p"))]
    {
        let ring_size = ring.buffers.len().min(num_chunks);

        let mut inflight: std::collections::VecDeque<(usize, usize)> =
            std::collections::VecDeque::with_capacity(ring_size);

        let initial_batch = ring_size.min(num_chunks);
        for i in 0..initial_batch {
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
            inflight.push_back((i, i));
        }

        let mut next_to_submit = initial_batch;
        let mut completed_count = 0usize;

        for _completed in 0..num_chunks {
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

            let guard = ring.buffers[ring_slot].lock().unwrap();

            if copy_len > 0 {
                std::ptr::copy_nonoverlapping(
                    guard.as_ptr() as *const u8,
                    mem_tier_ptr.add(seg.buffer_offset),
                    copy_len,
                );
            }

            gpu.dma_copy_to_device_async(
                &guard,
                (gpu_dst as *mut u8).add(seg.buffer_offset) as *mut std::ffi::c_void,
                copy_len,
                current_stream,
            )
            .map_err(|e| {
                DispatcherError::IoError(format!(
                    "GPU async DMA copy (seg {seg_idx}) failed: {e}"
                ))
            })?;

            drop(guard);
            completed_count += 1;

            if completed_count % ring_size == 0 {
                gpu.stream_synchronize(streams[0]).map_err(|e| {
                    DispatcherError::IoError(format!("stream_synchronize failed: {e}"))
                })?;
                gpu.stream_synchronize(streams[1]).map_err(|e| {
                    DispatcherError::IoError(format!("stream_synchronize failed: {e}"))
                })?;
            }

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

        for s in streams {
            gpu.stream_synchronize(*s).map_err(|e| {
                DispatcherError::IoError(format!("final stream_synchronize: {e}"))
            })?;
        }

        Ok(())
    }
}

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