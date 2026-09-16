//! The CUDA runtime entry points the live path needs, declared locally.
//!
//! # Why these are declared here rather than imported
//!
//! `gpu-services` already wraps these calls, but its `IGpuServices` impl is only complete
//! when its `spdk` feature is on, because the trait's SPDK methods are
//! `#[cfg(feature = "spdk")]` in `interfaces`. Depending on it would unify cargo features
//! across the workspace and break any `cargo build` or `cargo doc` that also builds a
//! crate enabling `interfaces/spdk`. `apps/remote-lookup-bench` reached the same
//! conclusion and declares its own; this module follows it deliberately, so the two agree.
//!
//! The cost of that choice is a second copy of six declarations. The cost of the
//! alternative is a workspace whose feature resolution depends on which crates happen to
//! be in the build graph, which is the kind of coupling this project's component model
//! exists to avoid.
//!
//! # What the live path actually needs a GPU for
//!
//! Nothing it measures. The control plane carries keys and IPC handles only — a payload
//! never crosses the mailbox — so the *generator's* per-operation cost is independent of
//! block size. But `LOOKUP` and `COPY_TO_STORE` name a destination and a source, and both
//! are GPU memory: a load DMAs into a device buffer and a store copies out of one. So the
//! generator needs one allocation it can export as an IPC handle, and nothing more. See
//! [`crate::payload`], which owns it.
//!
//! This module is compiled only under the `live` feature, and `build.rs` emits the
//! `cudart` link directive under the same condition, so an emit-only build neither
//! references nor links CUDA.

use std::ffi::{c_char, c_int, c_void, CStr};

/// A CUDA runtime status code.
pub type CudaError = c_int;

/// `cudaSuccess`.
pub const SUCCESS: CudaError = 0;

/// `cudaMemcpyHostToDevice`.
pub const MEMCPY_HOST_TO_DEVICE: c_int = 1;

/// Bytes in a CUDA IPC memory handle, and in the wire handle table's entry for one.
pub const IPC_HANDLE_BYTES: usize = 64;

/// Opaque CUDA IPC memory handle.
///
/// `CUipcMemHandle` is 64 opaque bytes, which is also exactly what the mailbox's handle
/// table carries per entry — so [`IpcMemHandle::reserved`] goes onto the wire verbatim,
/// with no reinterpretation.
#[repr(C)]
#[derive(Clone, Copy)]
pub struct IpcMemHandle {
    /// The opaque bytes, passed through unchanged.
    pub reserved: [u8; IPC_HANDLE_BYTES],
}

impl Default for IpcMemHandle {
    fn default() -> Self {
        Self {
            reserved: [0u8; IPC_HANDLE_BYTES],
        }
    }
}

extern "C" {
    /// Select the device for subsequent calls on this thread.
    pub fn cudaSetDevice(device: c_int) -> CudaError;
    /// Allocate device memory.
    pub fn cudaMalloc(devptr: *mut *mut c_void, size: usize) -> CudaError;
    /// Free device memory.
    pub fn cudaFree(devptr: *mut c_void) -> CudaError;
    /// Export an allocation as an IPC handle another process can open.
    pub fn cudaIpcGetMemHandle(handle: *mut IpcMemHandle, devptr: *mut c_void) -> CudaError;
    /// Synchronous copy between host and device.
    pub fn cudaMemcpy(dst: *mut c_void, src: *const c_void, count: usize, kind: c_int)
        -> CudaError;
    /// Static description of a status code.
    pub fn cudaGetErrorString(error: CudaError) -> *const c_char;
}

/// Human-readable text for a CUDA status code.
///
/// Never fails: an unrecognised code is reported by number rather than swallowed, because
/// a setup failure with no explanation is the hardest kind to diagnose on a cluster node.
pub fn error_string(err: CudaError) -> String {
    // SAFETY: `cudaGetErrorString` returns a pointer to a static NUL-terminated string
    // for any code, including codes it does not recognise. The NULL check is defensive:
    // the contract does not permit it, and we would rather print a number than fault.
    unsafe {
        let p = cudaGetErrorString(err);
        if p.is_null() {
            format!("unknown CUDA error {err}")
        } else {
            CStr::from_ptr(p).to_string_lossy().into_owned()
        }
    }
}

/// Run a CUDA call and turn a non-success code into a described error.
///
/// `what` is the call as it should appear in the message, including its arguments: a bare
/// "cudaMalloc failed" tells an operator nothing about how much was asked for.
pub fn check(what: impl FnOnce() -> String, err: CudaError) -> Result<(), String> {
    if err == SUCCESS {
        Ok(())
    } else {
        Err(format!("{}: {}", what(), error_string(err)))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_ipc_handle_is_sixty_four_bytes_because_that_is_what_the_wire_carries() {
        // The mailbox's handle table entry is `[u8; 64]` plus an i32 device ordinal
        // (`decode_handle_batch` in shmq-dispatcher). If CUDA's handle were a different
        // width, the bytes would still be accepted and the server would open garbage.
        assert_eq!(IPC_HANDLE_BYTES, 64);
        assert_eq!(std::mem::size_of::<IpcMemHandle>(), 64);
        assert_eq!(IpcMemHandle::default().reserved.len(), 64);
    }

    #[test]
    fn a_success_code_is_not_an_error_and_a_failure_carries_its_context() {
        assert!(check(|| "cudaMalloc(8)".into(), SUCCESS).is_ok());
        let err = check(|| "cudaMalloc(8)".into(), 2).unwrap_err();
        assert!(err.starts_with("cudaMalloc(8): "), "got: {err}");
        // The description must come from CUDA rather than from us, so that a code we
        // have never seen still explains itself.
        assert!(!err.ends_with(": "), "no description was appended: {err}");
    }

    #[test]
    fn an_unknown_code_is_reported_by_number_rather_than_swallowed() {
        // A setup failure with no explanation is the hardest kind to diagnose remotely.
        let s = error_string(-424_242);
        assert!(!s.is_empty());
    }
}
