//! GPU device discovery logic.

#[cfg(feature = "gpu")]
use interfaces::GpuDeviceInfo;

#[cfg(feature = "gpu")]
use crate::cuda_ffi;

/// Discover all NVIDIA GPUs with compute capability >= 7.0.
#[cfg(feature = "gpu")]
pub fn discover_devices() -> Result<Vec<GpuDeviceInfo>, String> {
    let mut count: std::os::raw::c_int = 0;

    // SAFETY: count is a valid, aligned pointer to a stack-allocated c_int.
    let err = unsafe { cuda_ffi::cudaGetDeviceCount(&mut count) };
    if err != cuda_ffi::CUDA_SUCCESS {
        return Err(format!(
            "cudaGetDeviceCount failed: {}",
            cuda_ffi::cuda_error_string(err)
        ));
    }

    if count == 0 {
        return Err("No NVIDIA GPUs detected".to_string());
    }

    let mut devices = Vec::new();

    for i in 0..count {
        // SAFETY: zeroed memory is a valid representation for cudaDeviceProp.
        let mut prop: cuda_ffi::cudaDeviceProp = unsafe { std::mem::zeroed() };
        // SAFETY: prop is a valid pointer; i is in range [0, count).
        let err = unsafe { cuda_ffi::cudaGetDeviceProperties(&mut prop, i) };
        if err != cuda_ffi::CUDA_SUCCESS {
            continue;
        }

        // Filter: compute capability >= 7.0
        if prop.major < 7 {
            continue;
        }

        let name = {
            // SAFETY: cudaGetDeviceProperties null-terminates prop.name (256-byte buffer).
            let cstr = unsafe { std::ffi::CStr::from_ptr(prop.name.as_ptr()) };
            cstr.to_string_lossy().into_owned()
        };

        let mut pci_bus_id = [0i8; 64];
        // SAFETY: pci_bus_id is a valid 64-byte buffer; i is a valid device index.
        let pci_err = unsafe { cuda_ffi::cudaDeviceGetPCIBusId(pci_bus_id.as_mut_ptr(), 64, i) };
        let pci_str = if pci_err == cuda_ffi::CUDA_SUCCESS {
            // SAFETY: cudaDeviceGetPCIBusId null-terminates the output on success.
            let cstr = unsafe { std::ffi::CStr::from_ptr(pci_bus_id.as_ptr()) };
            cstr.to_string_lossy().into_owned()
        } else {
            "unknown".to_string()
        };

        devices.push(GpuDeviceInfo {
            device_index: i as u32,
            name,
            memory_bytes: prop.total_global_mem as u64,
            compute_major: prop.major as u32,
            compute_minor: prop.minor as u32,
            pci_bus_id: pci_str,
        });
    }

    if devices.is_empty() {
        return Err("No GPUs with compute capability 7.0+ (Volta or newer) detected".to_string());
    }

    Ok(devices)
}
