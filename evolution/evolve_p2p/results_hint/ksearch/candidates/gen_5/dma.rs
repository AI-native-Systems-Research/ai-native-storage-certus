//! DMA buffer creation functions for various memory types.
//!
//! This module provides functions for creating SPDK-registered DMA buffers
//! from different memory sources including GPU BAR1 memory for P2P transfers.

use interfaces::DmaBuffer;

/// SPDK memory registration functions for P2P DMA access.
#[cfg(feature = "p2p")]
mod spdk_ffi {
    use std::os::raw::c_int;

    extern "C" {
        /// Register a memory region with SPDK for DMA access.
        pub fn spdk_mem_register(
            addr: *mut std::ffi::c_void,
            len: usize,
        ) -> c_int;

        /// Unregister a previously registered memory region from SPDK.
        pub fn spdk_mem_unregister(
            addr: *mut std::ffi::c_void,
            len: usize,
        ) -> c_int;
    }
}

/// Free function for P2P GPU BAR DMA buffers.
///
/// No-op: GPU memory lifetime is managed by the caller (the CUDA allocation
/// that owns the gpu_dst pointer). We only unregister from SPDK here is not
/// needed since we do bulk unregister at cleanup time.
#[cfg(feature = "p2p")]
unsafe extern "C" fn p2p_bar_buffer_free(_ptr: *mut std::ffi::c_void) {
    // No-op: GPU memory lifetime is managed externally.
    // SPDK unregistration is handled separately.
}

/// Create an SPDK DMA buffer backed by GPU BAR1 memory for P2P NVMe access.
///
/// This registers the GPU device pointer with SPDK's memory translation layer,
/// enabling NVMe controllers to perform DMA reads/writes directly to GPU memory
/// without bouncing through host DRAM.
///
/// # Arguments
/// * `dev_ptr` - GPU device pointer (BAR1-mapped, CPU-accessible via nvidia-peermem)
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
        let ret = spdk_ffi::spdk_mem_register(dev_ptr, size);
        if ret != 0 {
            return Err(format!(
                "spdk_mem_register failed for GPU BAR memory at {:p} size {}: error code {}",
                dev_ptr, size, ret
            ));
        }
    }

    // Wrap as a DmaBuffer. The free function is a no-op since GPU memory
    // lifetime is managed externally. fd = -1 (not file-backed).
    unsafe {
        DmaBuffer::from_raw(dev_ptr, size, p2p_bar_buffer_free, -1)
            .map_err(|e| format!("DmaBuffer::from_raw failed for GPU BAR buffer: {e}"))
    }
}

/// Unregister a GPU BAR buffer from SPDK (for cleanup at ring destruction time).
///
/// Should be called before freeing the GPU memory to ensure SPDK no longer
/// references the memory region.
#[cfg(feature = "p2p")]
pub fn unregister_gpu_bar_from_spdk(
    dev_ptr: *mut std::ffi::c_void,
    size: usize,
) -> Result<(), String> {
    if dev_ptr.is_null() {
        return Ok(());
    }
    unsafe {
        let ret = spdk_ffi::spdk_mem_unregister(dev_ptr, size);
        if ret != 0 {
            return Err(format!(
                "spdk_mem_unregister failed for GPU BAR memory at {:p} size {}: error code {}",
                dev_ptr, size, ret
            ));
        }
    }
    Ok(())
}