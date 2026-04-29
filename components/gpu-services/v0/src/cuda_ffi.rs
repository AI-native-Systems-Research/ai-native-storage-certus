//! Raw CUDA runtime API FFI bindings.
//!
//! Contains only the minimal subset of CUDA functions required by this
//! component. Hand-written for auditability (see research.md R1).

#![allow(non_camel_case_types)]
#![allow(dead_code)]

use std::ffi::c_void;
use std::os::raw::{c_char, c_int};

/// CUDA error codes (subset).
pub type cudaError_t = c_int;
pub const CUDA_SUCCESS: cudaError_t = 0;

/// CUDA IPC memory handle — 64 bytes opaque.
#[repr(C)]
#[derive(Clone, Copy)]
pub struct cudaIpcMemHandle_t {
    pub reserved: [u8; 64],
}

/// CUDA device properties (matches CUDA 13.0 layout through the fields we read).
#[repr(C)]
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

extern "C" {
    pub fn cudaGetDeviceCount(count: *mut c_int) -> cudaError_t;
    pub fn cudaGetDeviceProperties(prop: *mut cudaDeviceProp, device: c_int) -> cudaError_t;
    pub fn cudaSetDevice(device: c_int) -> cudaError_t;
    pub fn cudaDeviceSynchronize() -> cudaError_t;
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
    pub fn cudaMemcpy(
        dst: *mut c_void,
        src: *const c_void,
        count: usize,
        kind: c_int,
    ) -> cudaError_t;
    pub fn cudaFree(devptr: *mut c_void) -> cudaError_t;
    pub fn cudaGetErrorString(error: cudaError_t) -> *const c_char;
    pub fn cudaDeviceGetPCIBusId(pci_bus_id: *mut c_char, len: c_int, device: c_int)
        -> cudaError_t;
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
