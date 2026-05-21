//! IPC handle deserialization from base64 payloads.

#[cfg(feature = "gpu")]
use crate::cuda_ffi;

#[cfg(feature = "gpu")]
use interfaces::GpuIpcHandle;

/// Decode a base64 payload into raw IPC handle bytes and size.
#[cfg(feature = "gpu")]
pub fn decode_ipc_payload(base64_str: &str) -> Result<([u8; 64], u64), String> {
    use base64::Engine;
    let bytes = base64::engine::general_purpose::STANDARD
        .decode(base64_str)
        .map_err(|e| format!("Invalid base64: {}", e))?;

    if bytes.len() != 72 {
        return Err(format!(
            "Payload must be exactly 72 bytes, got {}",
            bytes.len()
        ));
    }

    let mut handle_bytes = [0u8; 64];
    handle_bytes.copy_from_slice(&bytes[0..64]);

    let size = u64::from_le_bytes(
        bytes[64..72]
            .try_into()
            .map_err(|_| "Failed to parse size bytes")?,
    );

    if size == 0 {
        return Err("Buffer size must be > 0".to_string());
    }

    Ok((handle_bytes, size))
}

/// Open a CUDA IPC memory handle and return a GpuIpcHandle.
#[cfg(feature = "gpu")]
pub fn open_ipc_handle(handle_bytes: [u8; 64], size: u64) -> Result<GpuIpcHandle, String> {
    let cuda_handle = cuda_ffi::cudaIpcMemHandle_t {
        reserved: handle_bytes,
    };

    let mut dev_ptr: *mut std::ffi::c_void = std::ptr::null_mut();

    // SAFETY: cudaIpcOpenMemHandle writes a device pointer to dev_ptr.
    // The handle bytes represent a valid exported IPC handle from another process.
    let err = unsafe {
        cuda_ffi::cudaIpcOpenMemHandle(
            &mut dev_ptr,
            cuda_handle,
            cuda_ffi::CUDA_IPC_MEM_LAZY_ENABLE_PEER_ACCESS,
        )
    };

    if err != cuda_ffi::CUDA_SUCCESS {
        return Err(format!(
            "cudaIpcOpenMemHandle failed: {}",
            cuda_ffi::cuda_error_string(err)
        ));
    }

    if dev_ptr.is_null() {
        return Err("cudaIpcOpenMemHandle returned null pointer".to_string());
    }

    Ok(GpuIpcHandle::new(dev_ptr, size as usize))
}
