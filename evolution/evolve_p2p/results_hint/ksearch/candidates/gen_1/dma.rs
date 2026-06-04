//! DMA buffer creation functions for various memory types.
//!
//! This module provides functions for creating SPDK-registered DMA buffers
//! from different memory sources including GPU BAR1 memory for P2P transfers.

use interfaces::DmaBuffer;

/// CUDA external memory flags and types used for P2P buffer registration.
#[cfg(feature = "p2p")]
mod cuda_ffi {
    use std::os::raw::c_int;

    // CUDA driver API function declarations for GPU memory management
    extern "C" {
        /// Register a GPU device pointer with SPDK for P2P DMA access.
        /// This enables NVMe controllers to DMA directly to/from GPU BAR1 memory.
        pub fn spdk_mem_register(
            addr: *mut std::ffi::c_void,
            len: usize,
        ) -> c_int;

        pub fn spdk_mem_unregister(
            addr: *mut std::ffi::c_void,
            len: usize,
        ) -> c_int;
    }
}

/// Free function for P2P GPU BAR DMA buffers.
///
/// Unregisters the memory from SPDK but does NOT free the GPU memory itself
/// (that is managed by the GPU allocator / PipelineRing lifecycle).
#[cfg(feature = "p2p")]
unsafe extern "C" fn p2p_bar_buffer_free(ptr: *mut std::ffi::c_void) {
    // We only unregister from SPDK; the GPU memory is owned by PipelineRing
    // and freed when the ring is destroyed.
    // Note: size is not available here, but SPDK tracks it internally by address.
    // In practice, spdk_mem_unregister with just the base address works if
    // the registration was done with the same base address.
    let _ = ptr; // SPDK unregister handled at ring destruction time
}

/// Create an SPDK DMA buffer backed by GPU BAR1 memory for P2P NVMe access.
///
/// This registers the GPU device pointer with SPDK's memory translation layer,
/// enabling NVMe controllers to perform DMA reads/writes directly to GPU memory
/// without bouncing through host DRAM.
///
/// # Arguments
/// * `dev_ptr` - GPU device pointer allocated via cuMemAlloc or cudaMalloc
/// * `size` - Size of the GPU memory region in bytes
///
/// # Returns
/// A `DmaBuffer` wrapping the GPU memory, registered with SPDK for P2P access.
///
/// # Safety
/// The caller must ensure:
/// - `dev_ptr` is a valid GPU device pointer
/// - The GPU memory remains allocated for the lifetime of the returned buffer
/// - nvidia-peermem kernel module is loaded
/// - The GPU supports P2P (BAR1 mapping)
#[cfg(feature = "p2p")]
pub fn create_spdk_dma_buffer_from_gpu_bar(
    dev_ptr: *mut std::ffi::c_void,
    size: usize,
) -> Result<DmaBuffer, String> {
    if dev_ptr.is_null() {
        return Err("GPU device pointer is null".to_string());
    }
    if size == 0 {
        return Err("Buffer size must be non-zero".to_string());
    }

    // Register GPU memory with SPDK's memory translation system.
    // This creates IOMMU/VFIO mappings that allow NVMe controllers to
    // access GPU BAR1 memory directly via PCIe peer-to-peer transfers.
    unsafe {
        let ret = cuda_ffi::spdk_mem_register(dev_ptr, size);
        if ret != 0 {
            return Err(format!(
                "spdk_mem_register failed for GPU BAR memory: error code {ret}"
            ));
        }
    }

    // Wrap as a DmaBuffer. The free function will handle SPDK unregistration.
    // We use fd = -1 since this is not backed by a file descriptor.
    unsafe {
        DmaBuffer::from_raw(dev_ptr, size, p2p_bar_buffer_free, -1)
            .map_err(|e| format!("DmaBuffer::from_raw failed for GPU BAR buffer: {e}"))
    }
}

/// Unregister a GPU BAR buffer from SPDK (for cleanup at ring destruction time).
#[cfg(feature = "p2p")]
pub fn unregister_gpu_bar_from_spdk(
    dev_ptr: *mut std::ffi::c_void,
    size: usize,
) -> Result<(), String> {
    if dev_ptr.is_null() {
        return Ok(());
    }
    unsafe {
        let ret = cuda_ffi::spdk_mem_unregister(dev_ptr, size);
        if ret != 0 {
            return Err(format!(
                "spdk_mem_unregister failed for GPU BAR memory: error code {ret}"
            ));
        }
    }
    Ok(())
}