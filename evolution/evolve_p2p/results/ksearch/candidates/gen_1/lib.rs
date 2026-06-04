//! Dispatcher buffer management and promotion logic.
//!
//! This module manages the ring of DMA buffers and coordinates the
//! promote_and_serve path that moves data from SSD to GPU for inference.

use std::sync::{Arc, Mutex};

use interfaces::{
    ClientChannels, DmaBuffer, DispatcherError, GpuStream, IBlockDevice, IGpuServices,
};

use crate::pipeline::{
    self, GdsRing, PipelineRing, PIPELINE_RING_SIZE,
};
use crate::io_segmenter;

/// Default chunk size for DMA transfers (4 MiB for optimal PCIe throughput).
pub const DEFAULT_CHUNK_SIZE: usize = 4 * 1024 * 1024;

/// Maximum NVMe queue depth for the sliding-window pipeline.
pub const MAX_QUEUE_DEPTH: usize = 32;

/// Dispatcher state holding pre-allocated resources.
pub struct Dispatcher {
    /// Pipeline ring for host-bounce path (fallback).
    pub pipeline_ring: Option<PipelineRing>,
    /// GDS ring for P2P path (preferred).
    pub gds_ring: Option<GdsRing>,
    /// Chunk size used for transfers.
    pub chunk_size: usize,
    /// Whether GDS P2P path is available.
    pub gds_available: bool,
}

impl Dispatcher {
    /// Create a new Dispatcher with pre-allocated pipeline resources.
    ///
    /// Attempts to allocate GDS (GPU-direct) ring first. Falls back to
    /// host-pinned ring if GDS is unavailable.
    pub fn new(gpu: &dyn IGpuServices, chunk_size: usize) -> Result<Self, DispatcherError> {
        // Try GDS ring first.
        let gds_ring = GdsRing::new(gpu, chunk_size).ok();
        let gds_available = gds_ring.as_ref().map_or(false, |r| {
            // Check if at least one buffer is actually GPU-resident.
            r.buffers.iter().any(|b| {
                b.lock().map_or(false, |guard| guard.is_device_memory())
            })
        });

        // Always allocate host-pinned ring as fallback.
        let pipeline_ring = PipelineRing::new(gpu, chunk_size).ok();

        if gds_ring.is_none() && pipeline_ring.is_none() {
            return Err(DispatcherError::AllocationFailed(
                "Failed to allocate any pipeline ring".into(),
            ));
        }

        Ok(Self {
            pipeline_ring,
            gds_ring,
            chunk_size,
            gds_available,
        })
    }

    /// Destroy all allocated resources.
    pub fn destroy(self, gpu: &dyn IGpuServices) {
        if let Some(ring) = self.pipeline_ring {
            ring.destroy(gpu);
        }
        if let Some(ring) = self.gds_ring {
            ring.destroy(gpu);
        }
    }
}

/// Promote data from SSD to GPU, using the best available path.
///
/// Tries GDS P2P first (NVMe → GPU directly), falls back to host-bounce
/// (NVMe → host DRAM → GPU).
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
    // Prefer GDS P2P path if available.
    if dispatcher.gds_available {
        if let Some(ref gds_ring) = dispatcher.gds_ring {
            return unsafe {
                pipeline::pipelined_ssd_to_gpu_p2p(
                    drive,
                    gpu,
                    gds_ring,
                    mem_tier_ptr,
                    gpu_dst,
                    start_lba,
                    total_bytes,
                )
            };
        }
    }

    // Fall back to host-bounce pipeline.
    if let Some(ref ring) = dispatcher.pipeline_ring {
        return unsafe {
            pipeline::pipelined_ssd_to_gpu(
                drive,
                gpu,
                ring,
                mem_tier_ptr,
                gpu_dst,
                start_lba,
                total_bytes,
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