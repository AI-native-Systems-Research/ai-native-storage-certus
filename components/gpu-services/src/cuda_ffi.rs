//! Raw CUDA runtime API FFI bindings.
//!
//! Contains only the minimal subset of CUDA functions required by this
//! component. Hand-written for auditability (see research.md R1).

#![allow(non_camel_case_types)]
#![allow(dead_code)]

use std::ffi::c_void;
use std::os::raw::{c_char, c_int, c_uint};

/// CUDA error codes (subset).
pub type cudaError_t = c_int;
pub const CUDA_SUCCESS: cudaError_t = 0;
pub const CUDA_ERROR_NOT_READY: cudaError_t = 600;
pub const CUDA_ERROR_HOST_MEMORY_ALREADY_REGISTERED: cudaError_t = 712;

/// CUDA IPC memory handle — 64 bytes opaque.
#[repr(C)]
#[derive(Clone, Copy, Debug)]
pub struct cudaIpcMemHandle_t {
    pub reserved: [u8; 64],
}

/// CUDA device properties (matches CUDA 13.0 layout through the fields we read).
#[repr(C)]
#[derive(Debug)]
pub struct cudaDeviceProp {
    pub name: [c_char; 256],
    _uuid: [u8; 16],
    _luid: [u8; 8],
    _luid_device_node_mask: u32,
    _pad_align: u32,
    pub total_global_mem: usize,
    _shared_mem_per_block: usize,
    _regs_per_block: c_int,
    _warp_size: c_int,
    _mem_pitch: usize,
    _max_threads_per_block: c_int,
    _max_threads_dim: [c_int; 3],
    _max_grid_size: [c_int; 3],
    _total_const_mem: usize,
    pub major: c_int,
    pub minor: c_int,
    // Padding to cover the remainder of the struct (well over 1KiB in CUDA 13.0).
    _pad: [u8; 4096],
}

/// CUDA memory type enum.
pub type cudaMemoryType = c_int;
pub const CUDA_MEMORY_TYPE_DEVICE: cudaMemoryType = 2;

/// CUDA pointer attributes.
#[repr(C)]
#[derive(Debug)]
pub struct cudaPointerAttributes {
    pub r#type: cudaMemoryType,
    pub device: c_int,
    pub device_pointer: *mut c_void,
    pub host_pointer: *mut c_void,
}

/// Flags for cudaIpcOpenMemHandle.
pub const CUDA_IPC_MEM_LAZY_ENABLE_PEER_ACCESS: c_int = 1;

/// cudaMemcpy direction constants.
pub const CUDA_MEMCPY_HOST_TO_DEVICE: c_int = 1;
pub const CUDA_MEMCPY_DEVICE_TO_HOST: c_int = 2;
pub const CUDA_MEMCPY_DEVICE_TO_DEVICE: c_int = 3;
/// `cudaMemcpyDefault`: the runtime infers the direction from the pointers
/// (unified virtual addressing).
pub const CUDA_MEMCPY_DEFAULT: c_int = 4;
/// First runtime whose `cudaMemcpyBatchAsync` has the signature declared below:
/// CUDA 12.8-12.9 take an extra `failIdx` argument.
pub const CUDA_MEMCPY_BATCH_MIN_RUNTIME: c_int = 13_000;

/// cudaHostAlloc flags.
pub const CUDA_HOST_ALLOC_DEFAULT: c_int = 0;
/// Memory is considered pinned by all CUDA contexts, not just the one current
/// at allocation time. Required so buffers allocated under device 0's context
/// remain page-locked (fast/zero-copy) for transfers to other GPUs.
pub const CUDA_HOST_ALLOC_PORTABLE: c_int = 1;
pub const CUDA_HOST_ALLOC_MAPPED: c_int = 2;

/// cudaHostRegister flags.
/// Registered memory is portable across all CUDA contexts (all devices), not
/// only the context current at registration time.
pub const CUDA_HOST_REGISTER_PORTABLE: c_int = 1;

/// Opaque CUDA stream handle.
pub type CudaStream = *mut c_void;

/// `cudaMemcpySrcAccessOrderStream`: the source is read in stream order.
pub const CUDA_MEMCPY_SRC_ACCESS_ORDER_STREAM: c_int = 1;

/// `struct cudaMemLocation` (`driver_types.h`, CUDA 13.x).
#[repr(C)]
#[derive(Clone, Copy, Debug, Default)]
pub struct cudaMemLocation {
    pub r#type: c_int,
    pub id: c_int,
}

/// `struct cudaMemcpyAttributes` (`driver_types.h`, CUDA 13.x).
#[repr(C)]
#[derive(Clone, Copy, Debug, Default)]
pub struct cudaMemcpyAttributes {
    pub src_access_order: c_int,
    pub src_loc_hint: cudaMemLocation,
    pub dst_loc_hint: cudaMemLocation,
    pub flags: c_uint,
}

extern "C" {
    pub fn cudaGetDeviceCount(count: *mut c_int) -> cudaError_t;
    pub fn cudaRuntimeGetVersion(runtime_version: *mut c_int) -> cudaError_t;
    pub fn cudaGetDeviceProperties(prop: *mut cudaDeviceProp, device: c_int) -> cudaError_t;
    pub fn cudaGetDevice(device: *mut c_int) -> cudaError_t;
    pub fn cudaSetDevice(device: c_int) -> cudaError_t;
    pub fn cudaDeviceSynchronize() -> cudaError_t;
    pub fn cudaIpcGetMemHandle(handle: *mut cudaIpcMemHandle_t, devptr: *mut c_void)
        -> cudaError_t;
    pub fn cudaIpcOpenMemHandle(
        devptr: *mut *mut c_void,
        handle: cudaIpcMemHandle_t,
        flags: c_int,
    ) -> cudaError_t;
    pub fn cudaIpcCloseMemHandle(devptr: *mut c_void) -> cudaError_t;
    pub fn cudaPointerGetAttributes(
        attributes: *mut cudaPointerAttributes,
        ptr: *const c_void,
    ) -> cudaError_t;
    pub fn cudaHostRegister(ptr: *mut c_void, size: usize, flags: c_int) -> cudaError_t;
    pub fn cudaHostUnregister(ptr: *mut c_void) -> cudaError_t;
    pub fn cudaMalloc(devptr: *mut *mut c_void, size: usize) -> cudaError_t;
    pub fn cudaHostAlloc(phost: *mut *mut c_void, size: usize, flags: c_int) -> cudaError_t;
    pub fn cudaFreeHost(ptr: *mut c_void) -> cudaError_t;
    pub fn cudaMemcpy(
        dst: *mut c_void,
        src: *const c_void,
        count: usize,
        kind: c_int,
    ) -> cudaError_t;
    pub fn cudaFree(devptr: *mut c_void) -> cudaError_t;
    pub fn cudaHostGetDevicePointer(
        pdevice: *mut *mut c_void,
        phost: *mut c_void,
        flags: c_int,
    ) -> cudaError_t;
    pub fn cudaGetErrorString(error: cudaError_t) -> *const c_char;
    pub fn cudaDeviceGetPCIBusId(pci_bus_id: *mut c_char, len: c_int, device: c_int)
        -> cudaError_t;
    pub fn cudaStreamCreate(stream: *mut CudaStream) -> cudaError_t;
    pub fn cudaStreamQuery(stream: CudaStream) -> cudaError_t;
    pub fn cudaStreamSynchronize(stream: CudaStream) -> cudaError_t;
    pub fn cudaStreamDestroy(stream: CudaStream) -> cudaError_t;
    pub fn cudaMemcpyAsync(
        dst: *mut c_void,
        src: *const c_void,
        count: usize,
        kind: c_int,
        stream: CudaStream,
    ) -> cudaError_t;
    /// CUDA 13.x signature (no `failIdx`); `stream` must not be the legacy NULL stream.
    pub fn cudaMemcpyBatchAsync(
        dsts: *const *mut c_void,
        srcs: *const *const c_void,
        sizes: *const usize,
        count: usize,
        attrs: *mut cudaMemcpyAttributes,
        attrs_idxs: *mut usize,
        num_attrs: usize,
        stream: CudaStream,
    ) -> cudaError_t;
}

/// Translate a CUDA error code to a descriptive Rust String.
pub fn cuda_error_string(err: cudaError_t) -> String {
    if err == CUDA_SUCCESS {
        return "success".to_string();
    }
    // SAFETY: cudaGetErrorString returns a static string pointer for any error code value.
    let cstr = unsafe { cudaGetErrorString(err) };
    if cstr.is_null() {
        return format!("unknown CUDA error (code {})", err);
    }
    // SAFETY: cudaGetErrorString returns a valid null-terminated C string with static lifetime.
    let s = unsafe { std::ffi::CStr::from_ptr(cstr) };
    s.to_string_lossy().into_owned()
}
