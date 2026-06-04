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