//! GPU memory verification operations.

#[cfg(feature = "gpu")]
use crate::cuda_ffi;

/// Verify that a GPU pointer refers to device memory (contiguous, pinned).
#[cfg(feature = "gpu")]
pub fn check_memory_attributes(ptr: *mut std::ffi::c_void) -> Result<(), String> {
    if ptr.is_null() {
        return Err("Null handle pointer".to_string());
    }

    // SAFETY: zeroed memory is a valid representation for cudaPointerAttributes.
    let mut attrs: cuda_ffi::cudaPointerAttributes = unsafe { std::mem::zeroed() };
    // SAFETY: attrs is a valid pointer to a zeroed struct; ptr is non-null (checked above).
    let err =
        unsafe { cuda_ffi::cudaPointerGetAttributes(&mut attrs, ptr as *const std::ffi::c_void) };

    if err != cuda_ffi::CUDA_SUCCESS {
        return Err(format!(
            "cudaPointerGetAttributes failed: {}",
            cuda_ffi::cuda_error_string(err)
        ));
    }

    if attrs.r#type != cuda_ffi::CUDA_MEMORY_TYPE_DEVICE {
        return Err(format!(
            "Memory is not device type (type={}), expected device memory",
            attrs.r#type
        ));
    }

    Ok(())
}
