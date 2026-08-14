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
//! use gpu_services::GpuServicesComponent;
//! use interfaces::IGpuServices;
//! use component_core::query_interface;
//!
//! let component = GpuServicesComponent::new_default();
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
    pub GpuServicesComponent {
        version: "0.1.0",
        provides: [IGpuServices],
        receptacles: {
            logger: ILogger,
        },
        fields: {
            gpu_state: Mutex<GpuState>,
            // `initialized` mirrors `GpuState::initialized` so the per-key copy
            // paths can read it without the mutex. See `is_initialized` below.
            initialized: std::sync::atomic::AtomicBool,
        },
    }
}

#[cfg(feature = "gpu")]
impl GpuServicesComponent {
    fn state(&self) -> &Mutex<GpuState> {
        &self.gpu_state
    }

    /// Lock-free `initialized` check for the per-key copy paths.
    ///
    /// The async copy methods run once per key on the request path from every
    /// worker thread. Taking the process-wide `gpu_state` mutex purely to read a
    /// bool made those calls serialize against each other for no reason. The
    /// atomic is written under that same lock in `initialize`/`shutdown`, so a
    /// reader still sees either the state before or the state after the
    /// transition — the same guarantee the lock provided.
    /// Only the `spdk`-gated async copy methods need it, so it does not exist in
    /// a gpu-only build.
    #[cfg(feature = "spdk")]
    fn is_initialized(&self) -> bool {
        self.initialized.load(std::sync::atomic::Ordering::Acquire)
    }
}

impl IGpuServices for GpuServicesComponent {
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
            self.initialized
                .store(true, std::sync::atomic::Ordering::Release);
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
            self.initialized
                .store(false, std::sync::atomic::Ordering::Release);

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

    fn create_stream(&self) -> Result<interfaces::GpuStream, String> {
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
            drop(state);

            let mut stream: cuda_ffi::CudaStream = std::ptr::null_mut();
            // SAFETY: stream is a valid pointer to a local CudaStream.
            let err = unsafe { cuda_ffi::cudaStreamCreate(&mut stream) };
            if err != cuda_ffi::CUDA_SUCCESS {
                return Err(format!(
                    "cudaStreamCreate failed: {}",
                    cuda_ffi::cuda_error_string(err)
                ));
            }
            Ok(interfaces::GpuStream(stream))
        }
    }

    fn set_device(&self, device: i32) -> Result<(), String> {
        #[cfg(not(feature = "gpu"))]
        {
            let _ = device;
            Err("GPU support not compiled (enable --features gpu)".to_string())
        }

        #[cfg(feature = "gpu")]
        {
            let state = self.state().lock().map_err(|e| e.to_string())?;
            if !state.initialized {
                return Err("Not initialized: call initialize() first".to_string());
            }
            drop(state);

            // SAFETY: cudaSetDevice takes a device ordinal; validity is checked
            // by the runtime, which returns an error for out-of-range values.
            let err = unsafe { cuda_ffi::cudaSetDevice(device) };
            if err != cuda_ffi::CUDA_SUCCESS {
                return Err(format!(
                    "cudaSetDevice({device}) failed: {}",
                    cuda_ffi::cuda_error_string(err)
                ));
            }
            Ok(())
        }
    }

    fn device_of_ptr(&self, ptr: *const std::ffi::c_void) -> Result<i32, String> {
        #[cfg(not(feature = "gpu"))]
        {
            let _ = ptr;
            Err("GPU support not compiled (enable --features gpu)".to_string())
        }

        #[cfg(feature = "gpu")]
        {
            let state = self.state().lock().map_err(|e| e.to_string())?;
            if !state.initialized {
                return Err("Not initialized: call initialize() first".to_string());
            }
            drop(state);

            let mut attr = cuda_ffi::cudaPointerAttributes {
                r#type: -1,
                device: -1,
                device_pointer: std::ptr::null_mut(),
                host_pointer: std::ptr::null_mut(),
            };
            // SAFETY: attr is a valid out-param; ptr is only inspected, not
            // dereferenced, by cudaPointerGetAttributes.
            let err = unsafe {
                cuda_ffi::cudaPointerGetAttributes(&mut attr, ptr as *mut std::ffi::c_void)
            };
            if err != cuda_ffi::CUDA_SUCCESS {
                return Err(format!(
                    "cudaPointerGetAttributes failed: {}",
                    cuda_ffi::cuda_error_string(err)
                ));
            }
            // type 0 (unregistered) / 1 (host) have no device association.
            if attr.r#type == cuda_ffi::CUDA_MEMORY_TYPE_DEVICE {
                Ok(attr.device)
            } else {
                Ok(-1)
            }
        }
    }

    fn destroy_stream(&self, stream: interfaces::GpuStream) -> Result<(), String> {
        #[cfg(not(feature = "gpu"))]
        {
            let _ = stream;
            Err("GPU support not compiled (enable --features gpu)".to_string())
        }

        #[cfg(feature = "gpu")]
        {
            // SAFETY: stream.0 was obtained from cudaStreamCreate.
            let err = unsafe { cuda_ffi::cudaStreamDestroy(stream.0) };
            if err != cuda_ffi::CUDA_SUCCESS {
                return Err(format!(
                    "cudaStreamDestroy failed: {}",
                    cuda_ffi::cuda_error_string(err)
                ));
            }
            Ok(())
        }
    }

    fn stream_query(&self, stream: interfaces::GpuStream) -> Result<bool, String> {
        #[cfg(not(feature = "gpu"))]
        {
            let _ = stream;
            Err("GPU support not compiled (enable --features gpu)".to_string())
        }

        #[cfg(feature = "gpu")]
        {
            // SAFETY: stream.0 was obtained from cudaStreamCreate.
            let err = unsafe { cuda_ffi::cudaStreamQuery(stream.0) };
            if err == cuda_ffi::CUDA_SUCCESS {
                Ok(true)
            } else if err == cuda_ffi::CUDA_ERROR_NOT_READY {
                Ok(false)
            } else {
                Err(format!(
                    "cudaStreamQuery failed: {}",
                    cuda_ffi::cuda_error_string(err)
                ))
            }
        }
    }

    fn stream_synchronize(&self, stream: interfaces::GpuStream) -> Result<(), String> {
        #[cfg(not(feature = "gpu"))]
        {
            let _ = stream;
            Err("GPU support not compiled (enable --features gpu)".to_string())
        }

        #[cfg(feature = "gpu")]
        {
            // SAFETY: stream.0 was obtained from cudaStreamCreate.
            let err = unsafe { cuda_ffi::cudaStreamSynchronize(stream.0) };
            if err != cuda_ffi::CUDA_SUCCESS {
                return Err(format!(
                    "cudaStreamSynchronize failed: {}",
                    cuda_ffi::cuda_error_string(err)
                ));
            }
            Ok(())
        }
    }

    #[cfg(feature = "spdk")]
    #[allow(clippy::not_unsafe_ptr_arg_deref)]
    fn dma_copy_to_device_async(
        &self,
        src: &interfaces::DmaBuffer,
        dst: *mut std::ffi::c_void,
        size: usize,
        stream: interfaces::GpuStream,
    ) -> Result<(), String> {
        if size > src.len() {
            return Err(format!(
                "size ({size}) exceeds source buffer length ({})",
                src.len()
            ));
        }

        #[cfg(not(feature = "gpu"))]
        {
            let _ = (dst, stream);
            Err("GPU support not compiled (enable --features gpu)".to_string())
        }

        #[cfg(feature = "gpu")]
        {
            // Lock-free: this runs once per key on the request path from every
            // worker thread, and the mutex made those calls serialize on a bool.
            if !self.is_initialized() {
                return Err("Not initialized: call initialize() first".to_string());
            }

            // SAFETY: Caller guarantees dst is a valid GPU device pointer covering
            // at least `size` bytes. src.as_ptr() is a valid DMA host buffer,
            // size <= src.len(), and stream.0 is a valid CUDA stream.
            let err = unsafe {
                cuda_ffi::cudaMemcpyAsync(
                    dst,
                    src.as_ptr() as *const std::ffi::c_void,
                    size,
                    cuda_ffi::CUDA_MEMCPY_HOST_TO_DEVICE,
                    stream.0,
                )
            };

            if err != cuda_ffi::CUDA_SUCCESS {
                return Err(format!(
                    "cudaMemcpyAsync H2D failed: {}",
                    cuda_ffi::cuda_error_string(err)
                ));
            }

            Ok(())
        }
    }

    #[cfg(feature = "spdk")]
    #[allow(clippy::not_unsafe_ptr_arg_deref)]
    fn memcpy_h2d_async(
        &self,
        src: *const std::ffi::c_void,
        dst: *mut std::ffi::c_void,
        size: usize,
        stream: interfaces::GpuStream,
    ) -> Result<(), String> {
        #[cfg(not(feature = "gpu"))]
        {
            let _ = (src, dst, size, stream);
            Err("GPU support not compiled (enable --features gpu)".to_string())
        }

        #[cfg(feature = "gpu")]
        {
            // Lock-free: this runs once per key on the request path from every
            // worker thread, and the mutex made those calls serialize on a bool.
            if !self.is_initialized() {
                return Err("Not initialized: call initialize() first".to_string());
            }

            let err = unsafe {
                cuda_ffi::cudaMemcpyAsync(
                    dst,
                    src,
                    size,
                    cuda_ffi::CUDA_MEMCPY_HOST_TO_DEVICE,
                    stream.0,
                )
            };

            if err != cuda_ffi::CUDA_SUCCESS {
                return Err(format!(
                    "cudaMemcpyAsync H2D failed: {}",
                    cuda_ffi::cuda_error_string(err)
                ));
            }

            Ok(())
        }
    }

    #[cfg(feature = "spdk")]
    #[allow(clippy::not_unsafe_ptr_arg_deref)]
    fn dma_copy_to_host_async(
        &self,
        src: *const std::ffi::c_void,
        dst: &interfaces::DmaBuffer,
        size: usize,
        stream: interfaces::GpuStream,
    ) -> Result<(), String> {
        if size > dst.len() {
            return Err(format!(
                "size ({size}) exceeds destination buffer length ({})",
                dst.len()
            ));
        }

        #[cfg(not(feature = "gpu"))]
        {
            let _ = (src, stream);
            Err("GPU support not compiled (enable --features gpu)".to_string())
        }

        #[cfg(feature = "gpu")]
        {
            // Lock-free: this runs once per key on the request path from every
            // worker thread, and the mutex made those calls serialize on a bool.
            if !self.is_initialized() {
                return Err("Not initialized: call initialize() first".to_string());
            }

            // SAFETY: Caller guarantees src is a valid GPU device pointer covering
            // at least `size` bytes. dst.as_ptr() is a valid pinned DMA host buffer,
            // size <= dst.len(), and stream.0 is a valid CUDA stream.
            let err = unsafe {
                cuda_ffi::cudaMemcpyAsync(
                    dst.as_ptr(),
                    src,
                    size,
                    cuda_ffi::CUDA_MEMCPY_DEVICE_TO_HOST,
                    stream.0,
                )
            };

            if err != cuda_ffi::CUDA_SUCCESS {
                return Err(format!(
                    "cudaMemcpyAsync D2H failed: {}",
                    cuda_ffi::cuda_error_string(err)
                ));
            }

            Ok(())
        }
    }

    #[cfg(feature = "spdk")]
    #[allow(clippy::not_unsafe_ptr_arg_deref)]
    fn memcpy_d2h_async(
        &self,
        src: *const std::ffi::c_void,
        dst: *mut std::ffi::c_void,
        size: usize,
        stream: interfaces::GpuStream,
    ) -> Result<(), String> {
        #[cfg(not(feature = "gpu"))]
        {
            let _ = (src, dst, size, stream);
            Err("GPU support not compiled (enable --features gpu)".to_string())
        }

        #[cfg(feature = "gpu")]
        {
            // Lock-free: this runs once per key on the request path from every
            // worker thread, and the mutex made those calls serialize on a bool.
            if !self.is_initialized() {
                return Err("Not initialized: call initialize() first".to_string());
            }

            let err = unsafe {
                cuda_ffi::cudaMemcpyAsync(
                    dst,
                    src,
                    size,
                    cuda_ffi::CUDA_MEMCPY_DEVICE_TO_HOST,
                    stream.0,
                )
            };

            if err != cuda_ffi::CUDA_SUCCESS {
                return Err(format!(
                    "cudaMemcpyAsync D2H failed: {}",
                    cuda_ffi::cuda_error_string(err)
                ));
            }

            Ok(())
        }
    }

    #[cfg(feature = "spdk")]
    fn allocate_pinned_dma_buffer(&self, size: usize) -> Result<interfaces::DmaBuffer, String> {
        #[cfg(not(feature = "gpu"))]
        {
            let _ = size;
            Err("GPU support not compiled (enable --features gpu)".to_string())
        }

        #[cfg(feature = "gpu")]
        {
            let state = self.state().lock().map_err(|e| e.to_string())?;
            if !state.initialized {
                return Err("Not initialized: call initialize() first".to_string());
            }
            drop(state);

            // SAFETY: cudaHostAlloc allocates page-locked host memory.
            let mut host_ptr: *mut std::ffi::c_void = std::ptr::null_mut();
            let err = unsafe {
                cuda_ffi::cudaHostAlloc(&mut host_ptr, size, cuda_ffi::CUDA_HOST_ALLOC_DEFAULT)
            };
            if err != cuda_ffi::CUDA_SUCCESS {
                return Err(format!(
                    "cudaHostAlloc({} bytes) failed: {}",
                    size,
                    cuda_ffi::cuda_error_string(err)
                ));
            }

            match dma::create_spdk_dma_buffer_from_cuda_host_alloc(host_ptr, size) {
                Ok(buf) => Ok(buf),
                Err(e) => {
                    // SAFETY: host_ptr was allocated by cudaHostAlloc.
                    unsafe { cuda_ffi::cudaFreeHost(host_ptr) };
                    Err(format!("SPDK register for pinned buffer: {e}"))
                }
            }
        }
    }

    #[cfg(feature = "spdk")]
    fn register_host_memory(&self, ptr: *mut std::ffi::c_void, size: usize) -> Result<(), String> {
        #[cfg(not(feature = "gpu"))]
        {
            let _ = (ptr, size);
            Err("GPU support not compiled (enable --features gpu)".to_string())
        }

        #[cfg(feature = "gpu")]
        {
            let state = self.state().lock().map_err(|e| e.to_string())?;
            if !state.initialized {
                return Err("Not initialized: call initialize() first".to_string());
            }
            drop(state);

            // SAFETY: ptr is valid for `size` bytes and page-aligned (caller contract).
            let err = unsafe { cuda_ffi::cudaHostRegister(ptr, size, 0) };
            let we_registered_cuda = err == cuda_ffi::CUDA_SUCCESS;
            if err != cuda_ffi::CUDA_SUCCESS
                && err != cuda_ffi::CUDA_ERROR_HOST_MEMORY_ALREADY_REGISTERED
            {
                return Err(format!(
                    "cudaHostRegister({} bytes) failed: {}",
                    size,
                    cuda_ffi::cuda_error_string(err)
                ));
            }

            extern "C" {
                fn spdk_mem_register(
                    vaddr: *mut std::ffi::c_void,
                    len: usize,
                ) -> std::os::raw::c_int;
            }

            let rc = unsafe { spdk_mem_register(ptr, size) };
            if rc != 0 && rc != -16 {
                // rc == -16 (EBUSY): memory already registered with SPDK (e.g. allocated
                // via spdk_zmalloc) — treat as success.
                if we_registered_cuda {
                    unsafe { cuda_ffi::cudaHostUnregister(ptr) };
                }
                return Err(format!("spdk_mem_register failed (rc={})", rc));
            }

            Ok(())
        }
    }

    #[cfg(feature = "spdk")]
    fn unregister_host_memory(
        &self,
        ptr: *mut std::ffi::c_void,
        size: usize,
    ) -> Result<(), String> {
        #[cfg(not(feature = "gpu"))]
        {
            let _ = (ptr, size);
            Err("GPU support not compiled (enable --features gpu)".to_string())
        }

        #[cfg(feature = "gpu")]
        {
            extern "C" {
                fn spdk_mem_unregister(
                    vaddr: *mut std::ffi::c_void,
                    len: usize,
                ) -> std::os::raw::c_int;
            }

            let rc = unsafe { spdk_mem_unregister(ptr, size) };
            if rc != 0 {
                return Err(format!("spdk_mem_unregister failed (rc={})", rc));
            }

            // SAFETY: ptr was previously registered with cudaHostRegister.
            let err = unsafe { cuda_ffi::cudaHostUnregister(ptr) };
            if err != cuda_ffi::CUDA_SUCCESS {
                return Err(format!(
                    "cudaHostUnregister failed: {}",
                    cuda_ffi::cuda_error_string(err)
                ));
            }

            Ok(())
        }
    }

    fn memcpy_batch_async(
        &self,
        ops: &[interfaces::GpuMemcpyBatchOp],
        stream: interfaces::GpuStream,
    ) -> Result<(), String> {
        #[cfg(not(feature = "gpu"))]
        {
            let _ = (ops, stream);
            Err("GPU support not compiled (enable --features gpu)".to_string())
        }

        #[cfg(feature = "gpu")]
        {
            if ops.is_empty() {
                return Ok(());
            }
            if !self.is_initialized() {
                return Err("Not initialized: call initialize() first".to_string());
            }

            // Try cuMemcpyBatchAsync (CUDA 12.8+) for non-null streams.
            if !stream.0.is_null() {
                if let Some(result) = self.try_batch_async(ops, stream) {
                    return result;
                }
            }

            // Fallback: individual cudaMemcpyAsync per op.
            for (i, op) in ops.iter().enumerate() {
                let kind = if op.src_access_order == interfaces::GpuMemcpySrcAccessOrder::Any {
                    cuda_ffi::CUDA_MEMCPY_HOST_TO_DEVICE
                } else {
                    cuda_ffi::CUDA_MEMCPY_DEVICE_TO_HOST
                };
                let err = unsafe {
                    cuda_ffi::cudaMemcpyAsync(op.dst, op.src, op.size, kind, stream.0)
                };
                if err != cuda_ffi::CUDA_SUCCESS {
                    return Err(format!(
                        "cudaMemcpyAsync failed at op {i}: {}",
                        cuda_ffi::cuda_error_string(err)
                    ));
                }
            }
            Ok(())
        }
    }
}

#[cfg(feature = "gpu")]
impl GpuServicesComponent {
    /// Attempt cuMemcpyBatchAsync via dynamic symbol resolution.
    /// Returns None if the symbol is unavailable (pre-12.8 driver).
    fn try_batch_async(
        &self,
        ops: &[interfaces::GpuMemcpyBatchOp],
        stream: interfaces::GpuStream,
    ) -> Option<Result<(), String>> {
        use std::sync::OnceLock;

        // cuMemcpyBatchAsync signature (CUDA driver API, 12.8+):
        //   CUresult cuMemcpyBatchAsync(
        //     CUdeviceptr *dstPtrArray, CUdeviceptr *srcPtrArray,
        //     size_t *sizeArray, size_t count,
        //     CUmemcpyAttributes *attrArray, size_t *attrIdxArray,
        //     size_t numAttrs, size_t *failIdx, CUstream stream)
        type BatchFn = unsafe extern "C" fn(
            dsts: *const *mut std::ffi::c_void,
            srcs: *const *const std::ffi::c_void,
            sizes: *const usize,
            count: usize,
            attrs: *const CuMemcpyAttributes,
            attr_idxs: *const usize,
            num_attrs: usize,
            fail_idx: *mut usize,
            stream: *mut std::ffi::c_void,
        ) -> i32;

        #[repr(C)]
        #[derive(Clone, Copy)]
        struct CuMemLocation {
            type_: i32,
            id: i32,
        }

        #[repr(C)]
        #[derive(Clone, Copy)]
        struct CuMemcpyAttributes {
            src_access_order: i32,
            src_loc_hint: CuMemLocation,
            dst_loc_hint: CuMemLocation,
            flags: u32,
        }

        const CU_MEMCPY_SRC_ACCESS_ORDER_STREAM: i32 = 0;
        const CU_MEMCPY_SRC_ACCESS_ORDER_ANY: i32 = 2;

        static BATCH_FN: OnceLock<Option<BatchFn>> = OnceLock::new();
        let batch_fn = BATCH_FN.get_or_init(|| {
            extern "C" {
                fn cuGetProcAddress(
                    symbol: *const std::ffi::c_char,
                    pfn: *mut *mut std::ffi::c_void,
                    cuda_version: i32,
                    flags: u64,
                    status: *mut i32,
                ) -> i32;
            }
            let symbol = b"cuMemcpyBatchAsync\0";
            let mut fn_ptr: *mut std::ffi::c_void = std::ptr::null_mut();
            let mut status: i32 = 0;
            let res = unsafe {
                cuGetProcAddress(
                    symbol.as_ptr() as *const std::ffi::c_char,
                    &mut fn_ptr,
                    12080, // minimum CUDA version
                    0,     // CU_GET_PROC_ADDRESS_DEFAULT
                    &mut status,
                )
            };
            if res != 0 || fn_ptr.is_null() {
                None
            } else {
                Some(unsafe { std::mem::transmute::<*mut std::ffi::c_void, BatchFn>(fn_ptr) })
            }
        });

        let batch_fn = (*batch_fn)?;

        let n = ops.len();
        let mut srcs: Vec<*const std::ffi::c_void> = Vec::with_capacity(n);
        let mut dsts: Vec<*mut std::ffi::c_void> = Vec::with_capacity(n);
        let mut sizes: Vec<usize> = Vec::with_capacity(n);

        for op in ops {
            srcs.push(op.src);
            dsts.push(op.dst);
            sizes.push(op.size);
        }

        // Build attribute runs: group consecutive ops with the same access order.
        let mut attrs: Vec<CuMemcpyAttributes> = Vec::new();
        let mut attr_idxs: Vec<usize> = Vec::new();

        for (i, op) in ops.iter().enumerate() {
            let order = match op.src_access_order {
                interfaces::GpuMemcpySrcAccessOrder::Any => CU_MEMCPY_SRC_ACCESS_ORDER_ANY,
                interfaces::GpuMemcpySrcAccessOrder::Stream => CU_MEMCPY_SRC_ACCESS_ORDER_STREAM,
            };
            if i == 0 || order != attrs.last().unwrap().src_access_order {
                attr_idxs.push(i);
                attrs.push(CuMemcpyAttributes {
                    src_access_order: order,
                    src_loc_hint: CuMemLocation { type_: 0, id: 0 },
                    dst_loc_hint: CuMemLocation { type_: 0, id: 0 },
                    flags: 0,
                });
            }
        }

        let mut fail_idx: usize = 0;
        let err = unsafe {
            batch_fn(
                dsts.as_ptr(),
                srcs.as_ptr(),
                sizes.as_ptr(),
                n,
                attrs.as_ptr(),
                attr_idxs.as_ptr(),
                attrs.len(),
                &mut fail_idx,
                stream.0,
            )
        };

        Some(if err == 0 {
            Ok(())
        } else {
            Err(format!(
                "cuMemcpyBatchAsync failed at index {fail_idx} with error {err}"
            ))
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use component_core::query_interface;
    use std::sync::Arc;

    fn make_logger() -> Arc<dyn ILogger + Send + Sync> {
        logger::LoggerComponent::new_default()
    }

    #[test]
    fn test_provides_igpu_services() {
        let component = GpuServicesComponent::new_default();
        let gpu = query_interface!(component, IGpuServices);
        assert!(gpu.is_some());
    }

    #[test]
    fn test_initialize_without_logger() {
        let component = GpuServicesComponent::new_default();
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
        let component = GpuServicesComponent::new_default();
        let gpu = query_interface!(component, IGpuServices).unwrap();
        assert!(gpu.shutdown().is_ok());
    }

    #[test]
    fn test_get_devices_before_init_fails() {
        #[cfg(not(feature = "gpu"))]
        {
            let component = GpuServicesComponent::new_default();
            let gpu = query_interface!(component, IGpuServices).unwrap();
            let result = gpu.get_devices();
            assert!(result.is_err());
        }
    }

    #[test]
    fn test_initialize_with_logger() {
        let component = GpuServicesComponent::new_default();
        component.logger.connect(make_logger()).unwrap();
        let gpu = query_interface!(component, IGpuServices).unwrap();
        let _ = gpu.initialize();
    }

    #[cfg(feature = "gpu")]
    #[test]
    fn test_initialize_idempotent() {
        let component = GpuServicesComponent::new_default();
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
        let component = GpuServicesComponent::new_default();
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
        let component = GpuServicesComponent::new_default();
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
        let component = GpuServicesComponent::new_default();
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
    //     let component = GpuServicesComponent::new_default();
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

        let component = GpuServicesComponent::new_default();
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
        let component = GpuServicesComponent::new_default();
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
        let component = GpuServicesComponent::new_default();
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
        let component = GpuServicesComponent::new_default();
        let gpu = query_interface!(component, IGpuServices).unwrap();
        if gpu.initialize().is_ok() {
            let payload = base64::engine::general_purpose::STANDARD.encode([0u8; 50]);
            let result = gpu.prepare_memory_for_spdk(&payload, None);
            assert!(result.is_err(), "expected Err, got Ok");
            let err = result.unwrap_err();
            assert!(
                err.contains("72 bytes"),
                "expected '72 bytes' in error, got: {err:?}"
            );
        }
    }

    #[cfg(all(feature = "gpu", feature = "spdk"))]
    #[test]
    fn test_prepare_memory_succeeds_without_logger() {
        let component = GpuServicesComponent::new_default();
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
        let component = GpuServicesComponent::new_default();
        component.logger.connect(make_logger()).unwrap();
        let gpu = query_interface!(component, IGpuServices).unwrap();
        if gpu.initialize().is_ok() {
            // With logger connected, invalid payload still returns error
            // (the logger path doesn't interfere with error handling).
            let result = gpu.prepare_memory_for_spdk("AAAA", None);
            assert!(result.is_err());
        }
    }
}
