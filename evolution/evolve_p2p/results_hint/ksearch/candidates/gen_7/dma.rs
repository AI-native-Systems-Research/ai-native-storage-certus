//! DMA buffer creation functions for various memory types.
//!
//! This module provides allocation helpers for DMA buffers used in the
//! storage-to-GPU data transfer pipeline.

use interfaces::DmaBuffer;

/// Create an SPDK-registered DMA buffer backed by GPU BAR1 memory.
///
/// This enables GPUDirect Storage P2P: NVMe controllers can DMA directly
/// into this GPU memory region without involving host DRAM.
///
/// # Arguments
/// * `dev_ptr` - A valid GPU device pointer (must be within BAR1 aperture)
/// * `size` - Size in bytes of the DMA buffer region
///
/// # Safety
/// The caller must ensure `dev_ptr` is a valid GPU memory pointer that remains
/// valid for the lifetime of the returned DmaBuffer. The GPU memory must be
/// accessible via BAR1 for PCIe peer-to-peer DMA.
///
/// # Returns
/// A `DmaBuffer` wrapping the GPU memory region, registered with SPDK for
/// direct NVMe DMA access.
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

    // Register the GPU BAR1 memory region with SPDK's memory subsystem.
    // This allows SPDK's NVMe driver to use this address as a DMA target.
    // The nvidia-peermem kernel module translates GPU virtual addresses to
    // physical BAR1 addresses for the NVMe controller's DMA engine.
    let rc = unsafe { spdk_mem_register(dev_ptr, size) };
    if rc != 0 {
        return Err(format!(
            "spdk_mem_register failed for GPU BAR ptr {:?}, size {}: rc={}",
            dev_ptr, size, rc
        ));
    }

    // Create DmaBuffer with a free function that unregisters from SPDK.
    // The GPU memory itself is NOT freed here — it's owned by the caller.
    let buf = unsafe { DmaBuffer::from_raw(dev_ptr, size, gpu_bar_dma_free, -1) }
        .map_err(|e| {
            // Cleanup on failure.
            unsafe { spdk_mem_unregister(dev_ptr, size) };
            format!("DmaBuffer::from_raw for GPU BAR failed: {e}")
        })?;

    Ok(buf)
}

/// Free function for GPU BAR DMA buffers: unregisters from SPDK but does NOT
/// free the GPU memory (which is owned by the GPU allocation layer).
#[cfg(feature = "p2p")]
unsafe extern "C" fn gpu_bar_dma_free(ptr: *mut std::ffi::c_void) {
    if !ptr.is_null() {
        // Best-effort unregister; ignore errors during cleanup.
        // Note: we don't have the size here, so we pass 0 which SPDK
        // interprets as "unregister the region starting at this address".
        let _ = spdk_mem_unregister(ptr, 0);
    }
}

// SPDK memory registration FFI bindings.
extern "C" {
    /// Register a memory region with SPDK for DMA access.
    /// For GPU BAR1 memory, nvidia-peermem translates the address.
    #[cfg(feature = "p2p")]
    fn spdk_mem_register(
        vaddr: *mut std::ffi::c_void,
        len: usize,
    ) -> std::os::raw::c_int;

    /// Unregister a previously registered memory region.
    #[cfg(feature = "p2p")]
    fn spdk_mem_unregister(
        vaddr: *mut std::ffi::c_void,
        len: usize,
    ) -> std::os::raw::c_int;
}

/// Create a standard CUDA-pinned, SPDK-registered DMA buffer on the host.
/// This is the non-P2P fallback path.
pub fn create_host_pinned_dma_buffer(size: usize) -> Result<DmaBuffer, String> {
    if size == 0 {
        return Err("Buffer size must be non-zero".to_string());
    }

    // Allocate CUDA pinned memory.
    let mut ptr: *mut std::ffi::c_void = std::ptr::null_mut();
    let cuda_flags: std::os::raw::c_int = 0; // cudaHostAllocDefault
    let rc = unsafe { cuda_host_alloc(&mut ptr, size, cuda_flags) };
    if rc != 0 {
        return Err(format!("cudaHostAlloc failed: rc={}", rc));
    }

    // Register with SPDK for DMA access.
    let spdk_rc = unsafe { spdk_mem_register_host(ptr, size) };
    if spdk_rc != 0 {
        unsafe { cuda_free_host(ptr) };
        return Err(format!(
            "spdk_mem_register failed for pinned host memory: rc={}",
            spdk_rc
        ));
    }

    let buf = unsafe { DmaBuffer::from_raw(ptr, size, host_pinned_dma_free, -1) }.map_err(|e| {
        unsafe {
            spdk_mem_unregister_host(ptr);
            cuda_free_host(ptr);
        }
        format!("DmaBuffer::from_raw for host pinned failed: {e}")
    })?;

    Ok(buf)
}

/// Free function for host-pinned DMA buffers.
unsafe extern "C" fn host_pinned_dma_free(ptr: *mut std::ffi::c_void) {
    if !ptr.is_null() {
        let _ = spdk_mem_unregister_host(ptr);
        cuda_free_host(ptr);
    }
}

// Host memory FFI bindings (non-P2P path).
extern "C" {
    fn spdk_mem_register_host(
        vaddr: *mut std::ffi::c_void,
        len: usize,
    ) -> std::os::raw::c_int;
    fn spdk_mem_unregister_host(vaddr: *mut std::ffi::c_void) -> std::os::raw::c_int;
    fn cuda_host_alloc(
        ptr: *mut *mut std::ffi::c_void,
        size: usize,
        flags: std::os::raw::c_int,
    ) -> std::os::raw::c_int;
    fn cuda_free_host(ptr: *mut std::ffi::c_void);
}