//! Minimal CUDA FFI bindings for IPC handle management.
#![allow(non_camel_case_types)]

use std::ffi::c_void;
use std::os::raw::c_int;

pub type cudaError_t = c_int;
pub const CUDA_SUCCESS: cudaError_t = 0;
pub const CUDA_IPC_MEM_LAZY_ENABLE_PEER_ACCESS: c_int = 1;

#[repr(C)]
pub struct cudaIpcMemHandle_t {
    pub reserved: [u8; 64],
}

extern "C" {
    pub fn cudaSetDevice(device: c_int) -> cudaError_t;
    pub fn cudaIpcOpenMemHandle(
        devptr: *mut *mut c_void,
        handle: cudaIpcMemHandle_t,
        flags: c_int,
    ) -> cudaError_t;
    pub fn cudaIpcCloseMemHandle(devptr: *mut c_void) -> cudaError_t;
}

pub fn cuda_error_string(err: cudaError_t) -> String {
    format!("CUDA error {err}")
}
