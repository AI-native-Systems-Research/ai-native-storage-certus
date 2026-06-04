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
    /// Whether GDS P2P path is available.
    pub p2p_available: bool,
}

impl Dispatcher {
    /// Create a new Dispatcher with pre-allocated pipeline resources.
    pub fn new(gpu: &dyn IGpuServices, chunk_size: usize) -> Result<Self, DispatcherError> {
        let pipeline_ring = PipelineRing::new(gpu, chunk_size)?;

        // Probe whether GPU DMA buffers can be allocated for P2P.
        let p2p_available = match gpu.allocate_gpu_dma_buffer(chunk_size) {
            Ok(_buf) => {
                // Buffer will be freed on drop.
                true
            }
            Err(_) => false,
        };

        Ok(Self {
            pipeline_ring: Some(pipeline_ring),
            chunk_size,
            p2p_available,
        })
    }

    /// Destroy all allocated resources.
    pub fn destroy(self, gpu: &dyn IGpuServices) {
        if let Some(ring) = self.pipeline_ring {
            ring.destroy(gpu);
        }
    }
}

/// Promote data from SSD to GPU, selecting the best available path.
///
/// Tries GDS P2P first (if available), falls back to host-bounce pipeline.
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
        if dispatcher.p2p_available {
            // Try P2P path first.
            match unsafe {
                pipeline::pipelined_ssd_to_gpu_p2p(
                    drive, gpu, ring, mem_tier_ptr, gpu_dst, start_lba, total_bytes,
                )
            } {
                Ok(()) => return Ok(()),
                Err(_) => {
                    // Fall through to standard path.
                }
            }
        }

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
    streams: &[GpuStream],
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