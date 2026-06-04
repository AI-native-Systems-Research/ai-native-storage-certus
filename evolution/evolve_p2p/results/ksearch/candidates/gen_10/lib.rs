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