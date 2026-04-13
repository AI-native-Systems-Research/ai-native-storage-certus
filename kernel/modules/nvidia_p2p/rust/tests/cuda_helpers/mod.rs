//! CUDA runtime dynamic loading helpers for integration tests.
//!
//! Loads `libcudart.so` at runtime via dlopen so tests build without a CUDA SDK.
//! Provides RAII wrappers for GPU memory allocation and prerequisite checking.

use std::ffi::c_void;
use std::fs::OpenOptions;
use std::sync::Arc;

use libloading::{Library, Symbol};

/// Reason why a test should be skipped.
#[derive(Debug)]
#[allow(clippy::enum_variant_names)]
pub enum SkipReason {
    /// `libcudart.so` could not be loaded.
    NoCudaRuntime,
    /// `cudaMalloc` failed (no GPU or driver not loaded).
    NoGpu,
    /// `/dev/nvidia_p2p` could not be opened (kernel module not loaded).
    NoKernelModule,
    /// `/dev/nvidia_p2p` requires CAP_SYS_RAWIO (run with sudo).
    NoPermission,
}

impl std::fmt::Display for SkipReason {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            SkipReason::NoCudaRuntime => write!(f, "CUDA runtime not available, skipping"),
            SkipReason::NoGpu => write!(f, "No CUDA GPU available, skipping"),
            SkipReason::NoKernelModule => write!(f, "nvidia_p2p_pin module not loaded, skipping"),
            SkipReason::NoPermission => write!(f, "permission denied on /dev/nvidia_p2p (run with sudo), skipping"),
        }
    }
}

/// Type alias for cudaMalloc: `cudaError_t cudaMalloc(void **devPtr, size_t size)`
type CudaMallocFn = unsafe extern "C" fn(*mut *mut c_void, usize) -> i32;
/// Type alias for cudaFree: `cudaError_t cudaFree(void *devPtr)`
type CudaFreeFn = unsafe extern "C" fn(*mut c_void) -> i32;

/// Dynamically loaded CUDA runtime library.
pub struct CudaRuntime {
    _lib: Library,
    cuda_malloc: CudaMallocFn,
    cuda_free: CudaFreeFn,
}

impl CudaRuntime {
    /// Attempt to load the CUDA runtime library via dlopen.
    ///
    /// Tries the following paths in order:
    /// 1. `libcudart.so` (generic symlink)
    /// 2. `libcudart.so.12` (CUDA 12.x)
    /// 3. `libcudart.so.11.0` (CUDA 11.x)
    pub fn load() -> Result<Self, SkipReason> {
        let lib_names = ["libcudart.so", "libcudart.so.12", "libcudart.so.11.0"];

        let lib = lib_names
            .iter()
            .find_map(|name| unsafe { Library::new(name).ok() })
            .ok_or(SkipReason::NoCudaRuntime)?;

        let cuda_malloc: CudaMallocFn = unsafe {
            let sym: Symbol<CudaMallocFn> = lib
                .get(b"cudaMalloc\0")
                .map_err(|_| SkipReason::NoCudaRuntime)?;
            *sym
        };

        let cuda_free: CudaFreeFn = unsafe {
            let sym: Symbol<CudaFreeFn> = lib
                .get(b"cudaFree\0")
                .map_err(|_| SkipReason::NoCudaRuntime)?;
            *sym
        };

        Ok(CudaRuntime {
            _lib: lib,
            cuda_malloc,
            cuda_free,
        })
    }

    /// Allocate GPU device memory via `cudaMalloc`.
    pub fn malloc(self: &Arc<Self>, size: usize) -> Result<CudaMemory, SkipReason> {
        let mut devptr: *mut c_void = std::ptr::null_mut();
        let err = unsafe { (self.cuda_malloc)(&mut devptr, size) };
        if err != 0 {
            return Err(SkipReason::NoGpu);
        }
        Ok(CudaMemory {
            devptr,
            size,
            runtime: Arc::clone(self),
        })
    }
}

/// RAII wrapper for a CUDA device memory allocation.
///
/// Calls `cudaFree` on Drop. Any `PinnedMemory` referencing this device pointer
/// MUST be dropped before this struct.
pub struct CudaMemory {
    devptr: *mut c_void,
    size: usize,
    runtime: Arc<CudaRuntime>,
}

impl CudaMemory {
    /// Return the device pointer as a u64 (suitable for ioctl calls).
    pub fn devptr(&self) -> u64 {
        self.devptr as u64
    }

    /// Allocation size in bytes.
    #[allow(dead_code)]
    pub fn size(&self) -> usize {
        self.size
    }
}

impl Drop for CudaMemory {
    fn drop(&mut self) {
        if !self.devptr.is_null() {
            unsafe {
                (self.runtime.cuda_free)(self.devptr);
            }
        }
    }
}

/// Check all prerequisites for CUDA P2P integration tests.
///
/// 1. Load CUDA runtime via dlopen
/// 2. Allocate and immediately free 64KB of GPU memory (validates GPU presence)
/// 3. Open and immediately close `/dev/nvidia_p2p` (validates kernel module)
///
/// Returns the loaded `CudaRuntime` on success (wrapped in Arc for shared use),
/// or a `SkipReason` indicating the first failing prerequisite.
pub fn check_prerequisites() -> Result<Arc<CudaRuntime>, SkipReason> {
    let runtime = Arc::new(CudaRuntime::load()?);

    // Probe GPU: allocate 64KB and immediately free
    {
        let _probe = runtime.malloc(65536)?;
        // _probe dropped here, calling cudaFree
    }

    // Probe kernel module: open /dev/nvidia_p2p and immediately close
    OpenOptions::new()
        .read(true)
        .write(true)
        .open("/dev/nvidia_p2p")
        .map_err(|e| {
            if e.kind() == std::io::ErrorKind::PermissionDenied {
                SkipReason::NoPermission
            } else {
                SkipReason::NoKernelModule
            }
        })?;

    Ok(runtime)
}
