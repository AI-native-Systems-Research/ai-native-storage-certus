//! DMA buffer creation functions for various memory types.
//!
//! Provides allocation routines for:
//! - CUDA-pinned + SPDK-registered host DMA buffers (for host-bounce path)
//! - GPU-resident + SPDK-registered DMA buffers (for GDS P2P path)
//! - Registered host memory regions (for zero-copy path)

use interfaces::{DmaBuffer, IGpuServices};

/// Allocate a CUDA-pinned, SPDK-registered DMA buffer on the host.
///
/// This is the standard buffer type for the host-bounce pipeline path.
/// The buffer is allocated with `cudaHostAlloc` (pinned, portable, mapped)
/// and registered with `spdk_mem_register` for NVMe DMA access.
pub fn allocate_pinned_dma_buffer(
    gpu: &dyn IGpuServices,
    size: usize,
) -> Result<DmaBuffer, String> {
    gpu.allocate_pinned_dma_buffer(size)
}

/// Allocate a GPU-resident DMA buffer for GPUDirect Storage P2P transfers.
///
/// This buffer resides in GPU device memory (BAR1-mapped) and is registered
/// with SPDK so that NVMe controllers can DMA directly into GPU memory
/// via nvidia-peermem/gdrdrv. This eliminates the host-DRAM bounce entirely.
///
/// Requirements:
/// - nvidia-peermem kernel module loaded
/// - gdrdrv kernel module loaded
/// - GPU must support P2P DMA (A30 does via BAR1)
///
/// Falls back to pinned host buffer if GPU DMA buffer allocation fails.
pub fn allocate_gpu_dma_buffer(
    gpu: &dyn IGpuServices,
    size: usize,
) -> Result<DmaBuffer, String> {
    // Try GPU-direct allocation first.
    match gpu.allocate_gpu_dma_buffer(size) {
        Ok(buf) => Ok(buf),
        Err(e) => {
            // Fall back to host-pinned.
            eprintln!(
                "GPU DMA buffer allocation failed ({e}), falling back to host-pinned"
            );
            gpu.allocate_pinned_dma_buffer(size)
        }
    }
}

/// Register an existing host memory region for DMA access.
///
/// This registers the memory with both CUDA (`cudaHostRegister`) and SPDK
/// (`spdk_mem_register`) so it can be used as both an NVMe DMA target and
/// a source for async H2D copies without additional staging buffers.
///
/// # Safety
///
/// The pointer must be valid for the given size and must remain valid
/// for the lifetime of the registration.
pub unsafe fn register_host_memory(
    gpu: &dyn IGpuServices,
    ptr: *mut std::ffi::c_void,
    size: usize,
) -> Result<(), String> {
    gpu.register_host_memory(ptr, size)
}

/// Unregister a previously registered host memory region.
///
/// # Safety
///
/// Must only be called with a pointer that was previously registered.
pub unsafe fn unregister_host_memory(
    gpu: &dyn IGpuServices,
    ptr: *mut std::ffi::c_void,
    size: usize,
) -> Result<(), String> {
    gpu.unregister_host_memory(ptr, size)
}