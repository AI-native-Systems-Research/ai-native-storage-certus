//! Raw FFI bindings to the SPDK environment and PCI device APIs.
//!
//! This crate provides auto-generated bindings via `bindgen` for the SPDK
//! functions needed by the `spdk-env` component: environment initialization,
//! PCI device enumeration, and device accessor functions.
//!
//! # Safety
//!
//! All functions in this crate are `unsafe` C FFI bindings. Callers must
//! ensure that SPDK has been properly initialized before calling device
//! enumeration or accessor functions.

#![allow(non_upper_case_globals)]
#![allow(non_camel_case_types)]
#![allow(non_snake_case)]
#![allow(dead_code)]
#![allow(clippy::all)]

include!(concat!(env!("OUT_DIR"), "/bindings.rs"));
