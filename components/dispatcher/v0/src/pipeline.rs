//! Ring-buffer pipelined reader for SSD→DRAM→GPU transfers.
//!
//! Reads data from SSD in MDTS-sized chunks using a ring of DMA buffers,
//! copying each completed chunk to both the memory-tier slot and the GPU
//! destination. This pipelines SSD reads with DRAM→GPU DMA transfers.

use std::sync::{Arc, Mutex};

use interfaces::{
    ClientChannels, Command, Completion, DmaBuffer, DispatcherError, IBlockDevice, IGpuServices,
};

use crate::io_segmenter;

/// Number of ring buffers for pipelined transfers.
pub const PIPELINE_RING_SIZE: usize = 4;

/// Pipeline-read from SSD into a memory-tier slot while streaming chunks to GPU.
///
/// For each chunk:
/// 1. Issue SSD read into ring buffer
/// 2. On completion, copy chunk to memory-tier slot
/// 3. DMA-copy chunk to GPU destination
///
/// This overlaps SSD I/O with GPU DMA by working on different chunks concurrently.
/// # Safety
///
/// - `mem_tier_ptr` must be valid for writes of at least `total_bytes` (aligned up to block size).
/// - `gpu_dst` must be a valid GPU destination pointer for `total_bytes`.
pub unsafe fn pipelined_ssd_to_gpu(
    drive: &dyn IBlockDevice,
    gpu: &dyn IGpuServices,
    mem_tier_ptr: *mut u8,
    gpu_dst: *mut std::ffi::c_void,
    start_lba: u64,
    total_bytes: usize,
    numa_node: i32,
) -> Result<(), DispatcherError> {
    let block_size = drive.block_size() as usize;
    let chunk_size = drive.max_transfer_size() as usize;
    let aligned_bytes = total_bytes.next_multiple_of(block_size);

    let channels: ClientChannels = drive.connect_client().map_err(|e| {
        DispatcherError::IoError(format!("connect_client failed: {e}"))
    })?;

    let segments = io_segmenter::segment_io(start_lba, aligned_bytes, chunk_size as u32, block_size as u32);

    // Allocate ring of DMA buffers.
    let ring: Vec<Arc<Mutex<DmaBuffer>>> = (0..PIPELINE_RING_SIZE.min(segments.len()))
        .map(|_| {
            DmaBuffer::new(chunk_size, block_size, Some(numa_node))
                .map(|b| Arc::new(Mutex::new(b)))
                .map_err(|e| {
                    DispatcherError::AllocationFailed(format!("pipeline ring buffer: {e}"))
                })
        })
        .collect::<Result<Vec<_>, _>>()?;

    for (i, seg) in segments.iter().enumerate() {
        let ring_idx = i % ring.len();
        let ring_buf = Arc::clone(&ring[ring_idx]);

        // Issue SSD read.
        channels
            .command_tx
            .send(Command::ReadSync {
                ns_id: 1,
                lba: seg.lba,
                buf: ring_buf.clone(),
            })
            .map_err(|_| DispatcherError::IoError("send ReadSync failed".into()))?;

        match channels.completion_rx.recv() {
            Ok(Completion::ReadDone { result, .. }) => {
                result.map_err(|e| {
                    DispatcherError::IoError(format!("SSD read failed: {e}"))
                })?;
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

        // Copy from ring buffer to memory-tier slot.
        let copy_len = seg.length.min(total_bytes.saturating_sub(seg.buffer_offset));
        let guard = ring_buf.lock().unwrap();
        if copy_len > 0 {
            // SAFETY: mem_tier_ptr + buffer_offset is within the memory-tier slot.
            // guard.as_ptr() is valid for seg.length bytes.
            unsafe {
                std::ptr::copy_nonoverlapping(
                    guard.as_ptr() as *const u8,
                    mem_tier_ptr.add(seg.buffer_offset),
                    copy_len,
                );
            }
        }

        // DMA-copy this chunk to GPU.
        let gpu_offset = seg.buffer_offset;
        gpu.dma_copy_to_device(
            &guard,
            // SAFETY: gpu_dst + gpu_offset is within the caller's GPU buffer.
            unsafe { (gpu_dst as *mut u8).add(gpu_offset) as *mut std::ffi::c_void },
            copy_len,
        )
        .map_err(|e| {
            DispatcherError::IoError(format!("GPU DMA copy (pipeline chunk) failed: {e}"))
        })?;

        drop(guard);
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
