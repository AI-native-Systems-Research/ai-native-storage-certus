//! GPU Services component for the Certus storage system.
//!
//! Provides the `IGpuServices` interface for CUDA initialization, GPU
//! hardware discovery, IPC handle deserialization, memory verification
//! and pinning, and DMA buffer creation.
//!
//! All GPU-dependent functionality is gated behind `--features gpu`.
//! Without the feature, the component builds but operations return an
//! error indicating GPU support was not compiled in.
//!
//! # Quick start
//!
//! ```no_run
//! use gpu_services::GpuServicesComponentV0;
//! use interfaces::IGpuServices;
//! use component_core::query_interface;
//!
//! let component = GpuServicesComponentV0::new_default();
//! let gpu = query_interface!(component, IGpuServices).unwrap();
//! gpu.initialize().unwrap();
//! let devices = gpu.get_devices().unwrap();
//! gpu.shutdown().unwrap();
//! ```

#[cfg(feature = "gpu")]
pub mod cuda_ffi;
#[cfg(feature = "gpu")]
mod device;
#[cfg(feature = "gpu")]
pub mod dma;
#[cfg(feature = "p2p")]
pub mod gdrcopy_ffi;
#[cfg(feature = "gpu")]
mod ipc;
#[cfg(feature = "gpu")]
mod memory;

use component_framework::define_component;
use interfaces::{GpuDeviceInfo, GpuDmaBuffer, GpuIpcHandle, IGpuServices, ILogger};

use std::sync::Mutex;

/// Internal component state tracking initialization and handles.
#[cfg(feature = "gpu")]
#[derive(Default)]
struct GpuState {
    initialized: bool,
    devices: Vec<GpuDeviceInfo>,
    /// Track which pointers have been verified.
    verified: std::collections::HashSet<usize>,
    /// Track which pointers have been pinned.
    pinned: std::collections::HashSet<usize>,
}

#[cfg(not(feature = "gpu"))]
#[derive(Default)]
struct GpuState;

define_component! {
    pub GpuServicesComponentV0 {
        version: "0.1.0",
        provides: [IGpuServices],
        receptacles: {
            logger: ILogger,
        },
        fields: {
            gpu_state: Mutex<GpuState>,
        },
    }
}

#[cfg(feature = "gpu")]
impl GpuServicesComponentV0 {
    fn state(&self) -> &Mutex<GpuState> {
        &self.gpu_state
    }
}

impl IGpuServices for GpuServicesComponentV0 {
    fn initialize(&self) -> Result<(), String> {
        #[cfg(not(feature = "gpu"))]
        {
            Err("GPU support not compiled (enable --features gpu)".to_string())
        }

        #[cfg(feature = "gpu")]
        {
            let mut state = self.state().lock().map_err(|e| e.to_string())?;
            if state.initialized {
                return Ok(());
            }

            if let Ok(log) = self.logger.get() {
                log.info("Initializing CUDA environment");
            }

            let devices = device::discover_devices()?;

            if let Ok(log) = self.logger.get() {
                log.info(&format!("Found {} qualifying GPU(s)", devices.len()));
            }

            state.devices = devices;
            state.initialized = true;
            Ok(())
        }
    }

    fn shutdown(&self) -> Result<(), String> {
        #[cfg(not(feature = "gpu"))]
        {
            Ok(())
        }

        #[cfg(feature = "gpu")]
        {
            let mut state = self.state().lock().map_err(|e| e.to_string())?;
            state.devices.clear();
            state.initialized = false;

            if let Ok(log) = self.logger.get() {
                log.info("GpuServices shut down");
            }
            Ok(())
        }
    }

    fn get_devices(&self) -> Result<Vec<GpuDeviceInfo>, String> {
        #[cfg(not(feature = "gpu"))]
        {
            Err("GPU support not compiled (enable --features gpu)".to_string())
        }

        #[cfg(feature = "gpu")]
        {
            let state = self.state().lock().map_err(|e| e.to_string())?;
            if !state.initialized {
                return Err("Not initialized: call initialize() first".to_string());
            }
            Ok(state.devices.clone())
        }
    }

    fn deserialize_ipc_handle(&self, base64_payload: &str) -> Result<GpuIpcHandle, String> {
        #[cfg(not(feature = "gpu"))]
        {
            let _ = base64_payload;
            Err("GPU support not compiled (enable --features gpu)".to_string())
        }

        #[cfg(feature = "gpu")]
        {
            let state = self.state().lock().map_err(|e| e.to_string())?;
            if !state.initialized {
                return Err("Not initialized: call initialize() first".to_string());
            }
            drop(state);

            let (handle_bytes, size) = ipc::decode_ipc_payload(base64_payload)?;
            let handle = ipc::open_ipc_handle(handle_bytes, size)?;

            if let Ok(log) = self.logger.get() {
                log.info(&format!("IPC handle deserialized: {} bytes", size));
            }

            Ok(handle)
        }
    }

    fn verify_memory(&self, handle: &GpuIpcHandle) -> Result<(), String> {
        #[cfg(not(feature = "gpu"))]
        {
            let _ = handle;
            Err("GPU support not compiled (enable --features gpu)".to_string())
        }

        #[cfg(feature = "gpu")]
        {
            memory::check_memory_attributes(handle.as_ptr())?;

            let key = handle.as_ptr() as usize;
            let mut state = self.state().lock().map_err(|e| e.to_string())?;
            state.verified.insert(key);

            if let Ok(log) = self.logger.get() {
                log.info("GPU memory verified: device type, contiguous");
            }

            Ok(())
        }
    }

    fn pin_memory(&self, handle: &GpuIpcHandle) -> Result<(), String> {
        #[cfg(not(feature = "gpu"))]
        {
            let _ = handle;
            Err("GPU support not compiled (enable --features gpu)".to_string())
        }

        #[cfg(feature = "gpu")]
        {
            let key = handle.as_ptr() as usize;
            let mut state = self.state().lock().map_err(|e| e.to_string())?;

            if state.pinned.contains(&key) {
                return Ok(());
            }

            // Verify if not already verified
            if !state.verified.contains(&key) {
                drop(state);
                memory::check_memory_attributes(handle.as_ptr())?;
                let mut state = self.state().lock().map_err(|e| e.to_string())?;
                state.verified.insert(key);
                state.pinned.insert(key);
            } else {
                state.pinned.insert(key);
            }

            if let Ok(log) = self.logger.get() {
                log.info("GPU memory pinned for DMA");
            }
            Ok(())
        }
    }

    fn unpin_memory(&self, handle: &GpuIpcHandle) -> Result<(), String> {
        #[cfg(not(feature = "gpu"))]
        {
            let _ = handle;
            Err("GPU support not compiled (enable --features gpu)".to_string())
        }

        #[cfg(feature = "gpu")]
        {
            let key = handle.as_ptr() as usize;
            let mut state = self.state().lock().map_err(|e| e.to_string())?;

            if !state.pinned.remove(&key) {
                return Err("Handle is not pinned".to_string());
            }

            Ok(())
        }
    }

    fn create_dma_buffer(&self, handle: GpuIpcHandle) -> Result<GpuDmaBuffer, String> {
        #[cfg(not(feature = "gpu"))]
        {
            let _ = handle;
            Err("GPU support not compiled (enable --features gpu)".to_string())
        }

        #[cfg(feature = "gpu")]
        {
            let key = handle.as_ptr() as usize;
            let state = self.state().lock().map_err(|e| e.to_string())?;

            if !state.verified.contains(&key) {
                return Err("Handle has not been verified".to_string());
            }
            if !state.pinned.contains(&key) {
                return Err("Handle has not been pinned".to_string());
            }
            drop(state);

            let buf = dma::create_gpu_dma_buffer(handle)?;

            if let Ok(log) = self.logger.get() {
                log.info(&format!("DMA buffer created: {} bytes", buf.len()));
            }

            Ok(buf)
        }
    }

    #[cfg(feature = "spdk")]
    #[allow(clippy::not_unsafe_ptr_arg_deref)]
    fn dma_copy_to_host(
        &self,
        src: *const std::ffi::c_void,
        dst: &interfaces::DmaBuffer,
        size: usize,
    ) -> Result<(), String> {
        if size > dst.len() {
            return Err(format!(
                "size ({size}) exceeds destination buffer length ({})",
                dst.len()
            ));
        }

        #[cfg(not(feature = "gpu"))]
        {
            let _ = src;
            Err("GPU support not compiled (enable --features gpu)".to_string())
        }

        #[cfg(feature = "gpu")]
        {
            let state = self.state().lock().map_err(|e| e.to_string())?;
            if !state.initialized {
                return Err("Not initialized: call initialize() first".to_string());
            }
            drop(state);

            // SAFETY: Caller guarantees src is a valid GPU device pointer covering
            // at least `size` bytes. dst.as_ptr() is a valid DMA host buffer and
            // we verified size <= dst.len() above.
            let err = unsafe {
                cuda_ffi::cudaMemcpy(
                    dst.as_ptr(),
                    src,
                    size,
                    cuda_ffi::CUDA_MEMCPY_DEVICE_TO_HOST,
                )
            };

            if err != cuda_ffi::CUDA_SUCCESS {
                return Err(format!(
                    "cudaMemcpy D2H failed: {}",
                    cuda_ffi::cuda_error_string(err)
                ));
            }

            Ok(())
        }
    }

    #[cfg(feature = "spdk")]
    fn prepare_memory_for_spdk(
        &self,
        base64_payload: &str,
        device_index: Option<u32>,
    ) -> Result<interfaces::DmaBuffer, String> {
        #[cfg(not(feature = "gpu"))]
        {
            let _ = (base64_payload, device_index);
            Err("GPU support not compiled (enable --features gpu)".to_string())
        }

        #[cfg(feature = "gpu")]
        {
            let state = self.state().lock().map_err(|e| e.to_string())?;
            if !state.initialized {
                return Err("Not initialized: call initialize() first".to_string());
            }
            drop(state);

            // Optionally set CUDA device context for multi-GPU systems.
            let original_device: Option<std::os::raw::c_int> = if let Some(idx) = device_index {
                let mut current: std::os::raw::c_int = 0;
                // SAFETY: current is a valid pointer to a local c_int.
                let err = unsafe { cuda_ffi::cudaGetDevice(&mut current) };
                if err != cuda_ffi::CUDA_SUCCESS {
                    return Err(format!(
                        "cudaGetDevice failed: {}",
                        cuda_ffi::cuda_error_string(err)
                    ));
                }
                // SAFETY: idx is a valid device ordinal provided by the caller.
                let err = unsafe { cuda_ffi::cudaSetDevice(idx as std::os::raw::c_int) };
                if err != cuda_ffi::CUDA_SUCCESS {
                    return Err(format!(
                        "cudaSetDevice({}) failed: {}",
                        idx,
                        cuda_ffi::cuda_error_string(err)
                    ));
                }
                Some(current)
            } else {
                None
            };

            // Helper to restore device context on error or success.
            let restore_device = |orig: Option<std::os::raw::c_int>| {
                if let Some(dev) = orig {
                    // SAFETY: dev was a valid device ordinal returned by cudaGetDevice.
                    unsafe {
                        cuda_ffi::cudaSetDevice(dev);
                    }
                }
            };

            // Decode and open the IPC handle.
            let (handle_bytes, size) = match ipc::decode_ipc_payload(base64_payload) {
                Ok(v) => v,
                Err(e) => {
                    restore_device(original_device);
                    return Err(e);
                }
            };

            let handle = match ipc::open_ipc_handle(handle_bytes, size) {
                Ok(h) => h,
                Err(e) => {
                    restore_device(original_device);
                    return Err(e);
                }
            };

            let ptr = handle.as_ptr();
            let buf_size = handle.size();

            // Check pin state: is this pointer already tracked as pinned?
            let was_already_pinned = {
                let state = self.state().lock().map_err(|e| {
                    // Rollback: close IPC handle on lock failure.
                    // SAFETY: ptr was obtained from cudaIpcOpenMemHandle.
                    unsafe {
                        cuda_ffi::cudaIpcCloseMemHandle(ptr);
                    }
                    restore_device(original_device);
                    e.to_string()
                })?;
                state.pinned.contains(&(ptr as usize))
            };

            // Conditionally pin and log the decision.
            if !was_already_pinned {
                // Verify memory is device type before pinning.
                if let Err(e) = memory::check_memory_attributes(ptr) {
                    // SAFETY: ptr was obtained from cudaIpcOpenMemHandle.
                    unsafe {
                        cuda_ffi::cudaIpcCloseMemHandle(ptr);
                    }
                    restore_device(original_device);
                    return Err(format!("Failed to pin memory: {}", e));
                }

                // Mark as pinned in component state.
                let mut state = self.state().lock().map_err(|e| {
                    // SAFETY: ptr was obtained from cudaIpcOpenMemHandle.
                    unsafe {
                        cuda_ffi::cudaIpcCloseMemHandle(ptr);
                    }
                    restore_device(original_device);
                    e.to_string()
                })?;
                state.verified.insert(ptr as usize);
                state.pinned.insert(ptr as usize);
                drop(state);

                if let Ok(log) = self.logger.get() {
                    log.info("prepare_memory_for_spdk: pinning GPU memory for DMA");
                }
            } else if let Ok(log) = self.logger.get() {
                log.info("prepare_memory_for_spdk: memory already pinned — skipping");
            }

            // Create the SPDK DmaBuffer with the pin-state-aware free function.
            let dma_buf =
                match dma::create_spdk_dma_buffer_from_gpu(ptr, buf_size, was_already_pinned) {
                    Ok(buf) => buf,
                    Err(e) => {
                        // Rollback: unpin if we pinned it, then close IPC handle.
                        if !was_already_pinned {
                            let mut state = self.state().lock().unwrap_or_else(|e| e.into_inner());
                            state.pinned.remove(&(ptr as usize));
                            state.verified.remove(&(ptr as usize));
                        }
                        // SAFETY: ptr was obtained from cudaIpcOpenMemHandle.
                        unsafe {
                            cuda_ffi::cudaIpcCloseMemHandle(ptr);
                        }
                        restore_device(original_device);
                        return Err(e);
                    }
                };

            // GpuIpcHandle has no Drop impl, so letting it go out of scope
            // does not close the IPC handle. Ownership of the pointer is now
            // held by DmaBuffer's free_fn.

            restore_device(original_device);

            if let Ok(log) = self.logger.get() {
                log.info(&format!(
                    "prepare_memory_for_spdk: DmaBuffer created ({} bytes)",
                    buf_size
                ));
            }

            Ok(dma_buf)
        }
    }

    #[cfg(feature = "spdk")]
    #[allow(clippy::not_unsafe_ptr_arg_deref)]
    fn dma_copy_to_device(
        &self,
        src: &interfaces::DmaBuffer,
        dst: *mut std::ffi::c_void,
        size: usize,
    ) -> Result<(), String> {
        if size > src.len() {
            return Err(format!(
                "size ({size}) exceeds source buffer length ({})",
                src.len()
            ));
        }

        #[cfg(not(feature = "gpu"))]
        {
            let _ = dst;
            Err("GPU support not compiled (enable --features gpu)".to_string())
        }

        #[cfg(feature = "gpu")]
        {
            let state = self.state().lock().map_err(|e| e.to_string())?;
            if !state.initialized {
                return Err("Not initialized: call initialize() first".to_string());
            }
            drop(state);

            // SAFETY: Caller guarantees dst is a valid GPU device pointer covering
            // at least `size` bytes. src.as_ptr() is a valid DMA host buffer and
            // we verified size <= src.len() above.
            let err = unsafe {
                cuda_ffi::cudaMemcpy(
                    dst,
                    src.as_ptr() as *const std::ffi::c_void,
                    size,
                    cuda_ffi::CUDA_MEMCPY_HOST_TO_DEVICE,
                )
            };

            if err != cuda_ffi::CUDA_SUCCESS {
                return Err(format!(
                    "cudaMemcpy H2D failed: {}",
                    cuda_ffi::cuda_error_string(err)
                ));
            }

            Ok(())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use component_core::query_interface;

    #[test]
    fn test_provides_igpu_services() {
        let component = GpuServicesComponentV0::new_default();
        let gpu = query_interface!(component, IGpuServices);
        assert!(gpu.is_some());
    }

    #[test]
    fn test_initialize_without_logger() {
        let component = GpuServicesComponentV0::new_default();
        let gpu = query_interface!(component, IGpuServices).unwrap();
        // Without GPU feature or hardware, this will return an error.
        // With the feature but no hardware, CUDA init will fail gracefully.
        let result = gpu.initialize();
        #[cfg(not(feature = "gpu"))]
        assert!(result.is_err());
        #[cfg(feature = "gpu")]
        {
            // On a system with a GPU this succeeds; without it fails.
            // Either is acceptable for the test.
            let _ = result;
        }
    }

    #[test]
    fn test_shutdown_without_logger() {
        let component = GpuServicesComponentV0::new_default();
        let gpu = query_interface!(component, IGpuServices).unwrap();
        assert!(gpu.shutdown().is_ok());
    }

    #[test]
    fn test_get_devices_before_init_fails() {
        #[cfg(not(feature = "gpu"))]
        {
            let component = GpuServicesComponentV0::new_default();
            let gpu = query_interface!(component, IGpuServices).unwrap();
            let result = gpu.get_devices();
            assert!(result.is_err());
        }
    }

    #[test]
    fn test_initialize_with_logger() {
        use std::sync::Arc;
        let component = GpuServicesComponentV0::new_default();
        let logger: Arc<dyn ILogger + Send + Sync> = logger::LoggerComponentV1::new_default();
        component.logger.connect(logger).unwrap();
        let gpu = query_interface!(component, IGpuServices).unwrap();
        let _ = gpu.initialize();
    }

    #[cfg(feature = "gpu")]
    #[test]
    fn test_initialize_idempotent() {
        let component = GpuServicesComponentV0::new_default();
        let gpu = query_interface!(component, IGpuServices).unwrap();
        // First call may succeed or fail depending on hardware.
        let r1 = gpu.initialize();
        if r1.is_ok() {
            // Second call must also succeed (idempotent).
            assert!(gpu.initialize().is_ok());
        }
    }

    #[cfg(feature = "gpu")]
    #[test]
    fn test_shutdown_releases_state() {
        let component = GpuServicesComponentV0::new_default();
        let gpu = query_interface!(component, IGpuServices).unwrap();
        if gpu.initialize().is_ok() {
            assert!(gpu.shutdown().is_ok());
            // After shutdown, get_devices should fail.
            assert!(gpu.get_devices().is_err());
        }
    }

    #[cfg(feature = "gpu")]
    #[test]
    fn test_deserialize_invalid_base64() {
        let component = GpuServicesComponentV0::new_default();
        let gpu = query_interface!(component, IGpuServices).unwrap();
        if gpu.initialize().is_ok() {
            let result = gpu.deserialize_ipc_handle("not-valid-base64!!!");
            assert!(result.is_err());
            assert!(result.unwrap_err().contains("base64"));
        }
    }

    #[cfg(feature = "gpu")]
    #[test]
    fn test_deserialize_wrong_payload_size() {
        use base64::Engine;
        let component = GpuServicesComponentV0::new_default();
        let gpu = query_interface!(component, IGpuServices).unwrap();
        if gpu.initialize().is_ok() {
            // 50 bytes instead of 72
            let payload = base64::engine::general_purpose::STANDARD.encode([0u8; 50]);
            let result = gpu.deserialize_ipc_handle(&payload);
            assert!(result.is_err());
            assert!(result.unwrap_err().contains("72 bytes"));
        }
    }

    // #[cfg(feature = "gpu")]
    // #[test]
    // fn test_deserialize_before_init_fails() {
    //     let component = GpuServicesComponentV0::new_default();
    //     let gpu = query_interface!(component, IGpuServices).unwrap();
    //     // Force a fresh uninitialized state.
    //     let _ = gpu.shutdown();
    //     let result = gpu.deserialize_ipc_handle("AAAA");
    //     assert!(result.is_err());
    //     assert!(result.unwrap_err().contains("Not initialized"));
    // }

    #[cfg(feature = "gpu")]
    #[test]
    fn test_dma_cpu_to_gpu_roundtrip() {
        use std::ffi::c_void;

        let component = GpuServicesComponentV0::new_default();
        let gpu = query_interface!(component, IGpuServices).unwrap();
        if gpu.initialize().is_err() {
            return;
        }

        const SIZE: usize = 4096;

        // Allocate GPU memory
        let mut dev_ptr: *mut c_void = std::ptr::null_mut();
        // SAFETY: dev_ptr is a valid pointer to a local variable.
        let err = unsafe { cuda_ffi::cudaMalloc(&mut dev_ptr, SIZE) };
        assert_eq!(err, cuda_ffi::CUDA_SUCCESS, "cudaMalloc failed");
        assert!(!dev_ptr.is_null());

        // Prepare CPU source buffer with a known pattern
        let src: Vec<u8> = (0..SIZE).map(|i| (i % 251) as u8).collect();

        // Copy CPU → GPU (Host to Device)
        // SAFETY: dev_ptr is a valid device pointer of SIZE bytes; src is a valid host buffer.
        let err = unsafe {
            cuda_ffi::cudaMemcpy(
                dev_ptr,
                src.as_ptr() as *const c_void,
                SIZE,
                cuda_ffi::CUDA_MEMCPY_HOST_TO_DEVICE,
            )
        };
        assert_eq!(err, cuda_ffi::CUDA_SUCCESS, "cudaMemcpy H2D failed");

        // Copy GPU → CPU (Device to Host) into a fresh buffer
        let mut dst: Vec<u8> = vec![0u8; SIZE];
        // SAFETY: dst is a valid host buffer; dev_ptr is a valid device pointer of SIZE bytes.
        let err = unsafe {
            cuda_ffi::cudaMemcpy(
                dst.as_mut_ptr() as *mut c_void,
                dev_ptr as *const c_void,
                SIZE,
                cuda_ffi::CUDA_MEMCPY_DEVICE_TO_HOST,
            )
        };
        assert_eq!(err, cuda_ffi::CUDA_SUCCESS, "cudaMemcpy D2H failed");

        // Verify round-trip integrity
        assert_eq!(src, dst, "CPU→GPU→CPU round-trip data mismatch");

        // Free GPU memory
        // SAFETY: dev_ptr was allocated by cudaMalloc and has not been freed.
        let err = unsafe { cuda_ffi::cudaFree(dev_ptr) };
        assert_eq!(err, cuda_ffi::CUDA_SUCCESS, "cudaFree failed");

        let _ = gpu.shutdown();
    }

    #[cfg(all(feature = "gpu", feature = "spdk"))]
    #[test]
    fn test_prepare_memory_not_initialized() {
        let component = GpuServicesComponentV0::new_default();
        let gpu = query_interface!(component, IGpuServices).unwrap();
        let _ = gpu.shutdown();
        let result = gpu.prepare_memory_for_spdk("AAAA", None);
        assert!(result.is_err());
        assert!(
            result.unwrap_err().contains("Not initialized"),
            "Expected 'Not initialized' error"
        );
    }

    #[cfg(all(feature = "gpu", feature = "spdk"))]
    #[test]
    fn test_prepare_memory_invalid_base64() {
        let component = GpuServicesComponentV0::new_default();
        let gpu = query_interface!(component, IGpuServices).unwrap();
        if gpu.initialize().is_ok() {
            let result = gpu.prepare_memory_for_spdk("not-valid-base64!!!", None);
            assert!(result.is_err());
            assert!(result.unwrap_err().contains("base64"));
        }
    }

    #[cfg(all(feature = "gpu", feature = "spdk"))]
    #[test]
    fn test_prepare_memory_wrong_payload_size() {
        use base64::Engine;
        let component = GpuServicesComponentV0::new_default();
        let gpu = query_interface!(component, IGpuServices).unwrap();
        if gpu.initialize().is_ok() {
            let payload = base64::engine::general_purpose::STANDARD.encode([0u8; 50]);
            let result = gpu.prepare_memory_for_spdk(&payload, None);
            assert!(result.is_err(), "expected Err, got Ok");
            let err = result.unwrap_err();
            assert!(err.contains("72 bytes"), "expected '72 bytes' in error, got: {err:?}");
        }
    }

    #[cfg(all(feature = "gpu", feature = "spdk"))]
    #[test]
    fn test_prepare_memory_succeeds_without_logger() {
        let component = GpuServicesComponentV0::new_default();
        let gpu = query_interface!(component, IGpuServices).unwrap();
        if gpu.initialize().is_ok() {
            // With no logger connected, invalid payload should still return
            // a clear error (not panic due to missing logger).
            let result = gpu.prepare_memory_for_spdk("AAAA", None);
            assert!(result.is_err());
        }
    }

    #[cfg(all(feature = "gpu", feature = "spdk"))]
    #[test]
    fn test_prepare_memory_logs_with_logger() {
        use std::sync::Arc;
        let component = GpuServicesComponentV0::new_default();
        let logger_comp: Arc<dyn ILogger + Send + Sync> = logger::LoggerComponentV1::new_default();
        component.logger.connect(logger_comp).unwrap();
        let gpu = query_interface!(component, IGpuServices).unwrap();
        if gpu.initialize().is_ok() {
            // With logger connected, invalid payload still returns error
            // (the logger path doesn't interfere with error handling).
            let result = gpu.prepare_memory_for_spdk("AAAA", None);
            assert!(result.is_err());
        }
    }
}
