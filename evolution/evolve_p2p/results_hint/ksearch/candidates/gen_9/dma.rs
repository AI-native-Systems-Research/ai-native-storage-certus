//! DMA buffer creation functions for various memory types.

use interfaces::DmaBuffer;

/// Register a GPU BAR1 memory region with SPDK for direct NVMe DMA.
/// Call this once for a contiguous GPU allocation, then create lightweight
/// DmaBuffer wrappers for sub-regions.
#[cfg(feature = "p2p")]
pub fn register_gpu_bar_region(
    dev_ptr: *mut std::ffi::c_void,
    size: usize,
) -> Result<(), String> {
    if dev_ptr.is_null() {
        return Err("GPU device pointer is null".to_string());
    }
    if size == 0 {
        return Err("Buffer size must be non-zero".to_string());
    }

    let rc = unsafe { spdk_mem_register(dev_ptr, size) };
    if rc != 0 {
        return Err(format!(
            "spdk_mem_register failed for GPU BAR ptr {:?}, size {}: rc={}",
            dev_ptr, size, rc
        ));
    }
    Ok(())
}

/// Unregister a GPU BAR1 memory region from SPDK.
#[cfg(feature = "p2p")]
pub fn unregister_gpu_bar_region(dev_ptr: *mut std::ffi::c_void, size: usize) {
    if !dev_ptr.is_null() && size > 0 {
        unsafe { spdk_mem_unregister(dev_ptr, size) };
    }
}

/// Create an SPDK-registered DMA buffer backed by GPU BAR1 memory.
///
/// This enables GPUDirect Storage P2P: NVMe controllers can DMA directly
/// into this GPU memory region without involving host DRAM.
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

    let rc = unsafe { spdk_mem_register(dev_ptr, size) };
    if rc != 0 {
        return Err(format!(
            "spdk_mem_register failed for GPU BAR ptr {:?}, size {}: rc={}",
            dev_ptr, size, rc
        ));
    }

    let buf = unsafe { DmaBuffer::from_raw(dev_ptr, size, gpu_bar_dma_free, -1) }
        .map_err(|e| {
            unsafe { spdk_mem_unregister(dev_ptr, size) };
            format!("DmaBuffer::from_raw for GPU BAR failed: {e}")
        })?;

    Ok(buf)
}

#[cfg(feature = "p2p")]
unsafe extern "C" fn gpu_bar_dma_free(ptr: *mut std::ffi::c_void) {
    if !ptr.is_null() {
        // Best-effort unregister; size=0 means SPDK uses internal tracking
        let _ = spdk_mem_unregister(ptr, 0);
    }
}

extern "C" {
    #[cfg(feature = "p2p")]
    fn spdk_mem_register(
        vaddr: *mut std::ffi::c_void,
        len: usize,
    ) -> std::os::raw::c_int;

    #[cfg(feature = "p2p")]
    fn spdk_mem_unregister(
        vaddr: *mut std::ffi::c_void,
        len: usize,
    ) -> std::os::raw::c_int;
}

/// Create a standard CUDA-pinned, SPDK-registered DMA buffer on the host.
pub fn create_host_pinned_dma_buffer(size: usize) -> Result<DmaBuffer, String> {
    if size == 0 {
        return Err("Buffer size must be non-zero".to_string());
    }

    let mut ptr: *mut std::ffi::c_void = std::ptr::null_mut();
    let cuda_flags: std::os::raw::c_int = 0;
    let rc = unsafe { cuda_host_alloc(&mut ptr, size, cuda_flags) };
    if rc != 0 {
        return Err(format!("cudaHostAlloc failed: rc={}", rc));
    }

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

unsafe extern "C" fn host_pinned_dma_free(ptr: *mut std::ffi::c_void) {
    if !ptr.is_null() {
        let _ = spdk_mem_unregister_host(ptr);
        cuda_free_host(ptr);
    }
}

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