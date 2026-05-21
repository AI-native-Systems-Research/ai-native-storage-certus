//! Raw GDRCopy FFI bindings (minimal subset for GPU BAR1 mapping).

#![allow(non_camel_case_types)]

use std::ffi::c_void;
use std::os::raw::{c_int, c_ulong};

pub type gdr_t = *mut c_void;

#[repr(C)]
#[derive(Clone, Copy, Default)]
pub struct gdr_mh_t {
    pub h: c_ulong,
}

pub const GPU_PAGE_SHIFT: u32 = 16;
pub const GPU_PAGE_SIZE: usize = 1 << GPU_PAGE_SHIFT;

extern "C" {
    pub fn gdr_open() -> gdr_t;
    pub fn gdr_close(g: gdr_t) -> c_int;
    pub fn gdr_pin_buffer(
        g: gdr_t,
        addr: c_ulong,
        size: usize,
        p2p_token: u64,
        va_space: u32,
        handle: *mut gdr_mh_t,
    ) -> c_int;
    pub fn gdr_unpin_buffer(g: gdr_t, handle: gdr_mh_t) -> c_int;
    pub fn gdr_map(g: gdr_t, handle: gdr_mh_t, va: *mut *mut c_void, size: usize) -> c_int;
    pub fn gdr_unmap(g: gdr_t, handle: gdr_mh_t, va: *mut c_void, size: usize) -> c_int;
}
