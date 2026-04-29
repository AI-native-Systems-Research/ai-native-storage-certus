//! DMA buffer creation from GPU IPC handles.

#[cfg(feature = "gpu")]
use crate::cuda_ffi;

#[cfg(feature = "gpu")]
use interfaces::{GpuDmaBuffer, GpuIpcHandle};

/// Free function for GpuDmaBuffer that closes the CUDA IPC handle.
#[cfg(feature = "gpu")]
unsafe extern "C" fn cuda_ipc_close_mem_handle(ptr: *mut std::ffi::c_void) {
    // SAFETY: ptr was obtained from cudaIpcOpenMemHandle and has not been closed.
    unsafe {
        cuda_ffi::cudaIpcCloseMemHandle(ptr);
    }
}

/// Create a GpuDmaBuffer from a verified and pinned IPC handle.
///
/// The caller is responsible for ensuring the handle has been verified
/// and pinned (tracked externally by the component state).
#[cfg(feature = "gpu")]
pub fn create_gpu_dma_buffer(handle: GpuIpcHandle) -> Result<GpuDmaBuffer, String> {
    if handle.as_ptr().is_null() {
        return Err("Handle has null pointer".to_string());
    }

    // SAFETY: The handle has been verified (device memory) and pinned
    // (tracked by component state). The pointer is valid for handle.size()
    // bytes. cuda_ipc_close_mem_handle correctly frees via cudaIpcCloseMemHandle.
    let buf =
        unsafe { GpuDmaBuffer::new(handle.as_ptr(), handle.size(), cuda_ipc_close_mem_handle) };

    // Forget the handle so its Drop doesn't try to close the IPC handle.
    // GpuDmaBuffer now owns the pointer via its free_fn.
    std::mem::forget(handle);

    Ok(buf)
}
