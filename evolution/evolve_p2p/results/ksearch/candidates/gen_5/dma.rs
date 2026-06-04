//! DMA buffer creation functions for various memory types.

use interfaces::{DmaBuffer, IGpuServices};

/// Allocate a CUDA-pinned, SPDK-registered DMA buffer on the host.
pub fn allocate_pinned_dma_buffer(
    gpu: &dyn IGpuServices,
    size: usize,
) -> Result<DmaBuffer, String> {
    gpu.allocate_pinned_dma_buffer(size)
}

/// Allocate a GPU-resident DMA buffer for GPUDirect Storage P2P transfers.
///
/// Falls back to pinned host buffer if GPU DMA buffer allocation fails.
pub fn allocate_gpu_dma_buffer(
    gpu: &dyn IGpuServices,
    size: usize,
) -> Result<DmaBuffer, String> {
    match gpu.allocate_gpu_dma_buffer(size) {
        Ok(buf) => Ok(buf),
        Err(e) => {
            eprintln!(
                "GPU DMA buffer allocation failed ({e}), falling back to host-pinned"
            );
            gpu.allocate_pinned_dma_buffer(size)
        }
    }
}

/// Register an existing host memory region for DMA access.
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