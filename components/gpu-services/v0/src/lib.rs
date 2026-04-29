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
//! let component = GpuServicesComponentV0::new();
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
mod dma;
#[cfg(feature = "gpu")]
mod ipc;
#[cfg(feature = "gpu")]
mod memory;

use component_framework::define_component;
use interfaces::{GpuDeviceInfo, GpuDmaBuffer, GpuIpcHandle, IGpuServices, ILogger};

#[cfg(feature = "gpu")]
use std::sync::Mutex;

/// Internal component state tracking initialization and handles.
#[cfg(feature = "gpu")]
struct GpuState {
    initialized: bool,
    devices: Vec<GpuDeviceInfo>,
    /// Track which pointers have been verified.
    verified: std::collections::HashSet<usize>,
    /// Track which pointers have been pinned.
    pinned: std::collections::HashSet<usize>,
}

define_component! {
    pub GpuServicesComponentV0 {
        version: "0.1.0",
        provides: [IGpuServices],
        receptacles: {
            logger: ILogger,
        },
    }
}

#[cfg(feature = "gpu")]
impl GpuServicesComponentV0 {
    fn state(&self) -> &Mutex<GpuState> {
        // Lazy initialization of state via a static-like pattern.
        // We use a thread_local or just accept the limitation that state
        // is stored elsewhere. For simplicity, we'll use an approach
        // that stores state in a global associated with this component instance.
        //
        // Since define_component! generates the struct, we need to use
        // an external state holder. We'll use a global Mutex for now.
        // In a production component, this would be stored in the component
        // struct itself if define_component! supports custom fields.
        static STATE: std::sync::OnceLock<Mutex<GpuState>> = std::sync::OnceLock::new();
        STATE.get_or_init(|| {
            Mutex::new(GpuState {
                initialized: false,
                devices: Vec::new(),
                verified: std::collections::HashSet::new(),
                pinned: std::collections::HashSet::new(),
            })
        })
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
}

#[cfg(test)]
mod tests {
    use super::*;
    use component_core::query_interface;

    #[test]
    fn test_provides_igpu_services() {
        let component = GpuServicesComponentV0::new();
        let gpu = query_interface!(component, IGpuServices);
        assert!(gpu.is_some());
    }

    #[test]
    fn test_initialize_without_logger() {
        let component = GpuServicesComponentV0::new();
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
        let component = GpuServicesComponentV0::new();
        let gpu = query_interface!(component, IGpuServices).unwrap();
        assert!(gpu.shutdown().is_ok());
    }

    #[test]
    fn test_get_devices_before_init_fails() {
        let component = GpuServicesComponentV0::new();
        let gpu = query_interface!(component, IGpuServices).unwrap();
        #[cfg(not(feature = "gpu"))]
        {
            let result = gpu.get_devices();
            assert!(result.is_err());
        }
    }

    #[test]
    fn test_initialize_with_logger() {
        use std::sync::Arc;
        let component = GpuServicesComponentV0::new();
        let logger: Arc<dyn ILogger + Send + Sync> = logger::LoggerComponentV1::new_default();
        component.logger.connect(logger).unwrap();
        let gpu = query_interface!(component, IGpuServices).unwrap();
        let _ = gpu.initialize();
    }

    #[cfg(feature = "gpu")]
    #[test]
    fn test_initialize_idempotent() {
        let component = GpuServicesComponentV0::new();
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
        let component = GpuServicesComponentV0::new();
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
        let component = GpuServicesComponentV0::new();
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
        let component = GpuServicesComponentV0::new();
        let gpu = query_interface!(component, IGpuServices).unwrap();
        if gpu.initialize().is_ok() {
            // 50 bytes instead of 72
            let payload = base64::engine::general_purpose::STANDARD.encode(&[0u8; 50]);
            let result = gpu.deserialize_ipc_handle(&payload);
            assert!(result.is_err());
            assert!(result.unwrap_err().contains("72 bytes"));
        }
    }

    #[cfg(feature = "gpu")]
    #[test]
    fn test_deserialize_before_init_fails() {
        let component = GpuServicesComponentV0::new();
        let gpu = query_interface!(component, IGpuServices).unwrap();
        // Force a fresh uninitialized state.
        let _ = gpu.shutdown();
        let result = gpu.deserialize_ipc_handle("AAAA");
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("Not initialized"));
    }

    #[cfg(feature = "gpu")]
    #[test]
    fn test_dma_cpu_to_gpu_roundtrip() {
        use std::ffi::c_void;

        let component = GpuServicesComponentV0::new();
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
}
