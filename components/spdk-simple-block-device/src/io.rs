//! Low-level NVMe I/O operations for the simple block device.
//!
//! All functions in this module are standalone — they operate on raw
//! [`InnerState`] and [`ISPDKEnv`](spdk_env::ISPDKEnv) references so they can
//! be called from both the direct component API and the actor handler.
//!
//! SPDK is fundamentally asynchronous (submit + poll). We wrap each NVMe
//! command by submitting it with a completion callback and then busy-polling
//! `spdk_nvme_qpair_process_completions` until the callback fires.

use crate::error::BlockDeviceError;
use spdk_env::ISPDKEnv;
use std::ptr;
use std::sync::atomic::{AtomicBool, AtomicI32, Ordering};

/// Opaque state holding raw SPDK pointers for an open block device.
///
/// # Safety
///
/// These pointers are valid between [`open_device`] and [`close_device`].
/// The caller must ensure single-threaded access to the qpair (SPDK requirement).
pub struct InnerState {
    pub(crate) ctrlr: *mut spdk_sys::spdk_nvme_ctrlr,
    pub(crate) ns: *mut spdk_sys::spdk_nvme_ns,
    pub(crate) qpair: *mut spdk_sys::spdk_nvme_qpair,
    pub sector_size: u32,
    pub num_sectors: u64,
}

// SAFETY: InnerState is only accessed single-threaded — either through the
// Mutex in SimpleBlockDevice or exclusively on the actor thread.
unsafe impl Send for InnerState {}

/// Context passed to the NVMe probe callback to capture the first attached controller.
struct ProbeContext {
    ctrlr: *mut spdk_sys::spdk_nvme_ctrlr,
}

/// Probe local PCIe NVMe devices, attach the first controller found,
/// open namespace 1, and allocate an I/O queue pair.
///
/// The SPDK environment must be initialized before calling this function.
pub fn open_device(env: &dyn ISPDKEnv) -> Result<InnerState, BlockDeviceError> {
    if !env.is_initialized() {
        return Err(BlockDeviceError::EnvNotInitialized(
            "SPDK environment not initialized. Call ISPDKEnv::init() first.".into(),
        ));
    }

    log("Probing NVMe devices...");

    let mut ctx = ProbeContext {
        ctrlr: ptr::null_mut(),
    };

    // SAFETY: spdk_nvme_probe scans the local PCIe bus. We pass NULL trid to scan all.
    // The probe_cb and attach_cb are called synchronously during this call.
    let rc = unsafe {
        spdk_sys::spdk_nvme_probe(
            ptr::null(),
            &mut ctx as *mut ProbeContext as *mut std::ffi::c_void,
            Some(probe_cb),
            Some(attach_cb),
            None,
        )
    };

    if rc != 0 {
        return Err(BlockDeviceError::ProbeFailure(format!(
            "spdk_nvme_probe() returned {rc}."
        )));
    }

    if ctx.ctrlr.is_null() {
        return Err(BlockDeviceError::ProbeFailure(
            "No NVMe controllers found. Ensure devices are bound to vfio-pci.".into(),
        ));
    }

    log("NVMe controller attached. Opening namespace 1...");

    // Get namespace 1.
    // SAFETY: ctrlr is valid (just attached). Namespace IDs are 1-based.
    let ns = unsafe { spdk_sys::spdk_nvme_ctrlr_get_ns(ctx.ctrlr, 1) };
    if ns.is_null() {
        unsafe { spdk_sys::spdk_nvme_detach(ctx.ctrlr) };
        return Err(BlockDeviceError::NamespaceNotFound(
            "Namespace 1 not found on controller.".into(),
        ));
    }

    // SAFETY: ns pointer is valid.
    let active = unsafe { spdk_sys::spdk_nvme_ns_is_active(ns) };
    if !active {
        unsafe { spdk_sys::spdk_nvme_detach(ctx.ctrlr) };
        return Err(BlockDeviceError::NamespaceNotFound(
            "Namespace 1 exists but is not active.".into(),
        ));
    }

    let sector_size = unsafe { spdk_sys::spdk_nvme_ns_get_sector_size(ns) };
    let num_sectors = unsafe { spdk_sys::spdk_nvme_ns_get_num_sectors(ns) };

    log(&format!(
        "Namespace 1: sector_size={sector_size}, num_sectors={num_sectors}, \
         capacity={}MB",
        (num_sectors * sector_size as u64) / (1024 * 1024)
    ));

    // Allocate I/O queue pair with default options.
    // SAFETY: ctrlr is valid. NULL opts = use defaults.
    let qpair = unsafe { spdk_sys::spdk_nvme_ctrlr_alloc_io_qpair(ctx.ctrlr, ptr::null(), 0) };
    if qpair.is_null() {
        unsafe { spdk_sys::spdk_nvme_detach(ctx.ctrlr) };
        return Err(BlockDeviceError::QpairAllocationFailed(
            "spdk_nvme_ctrlr_alloc_io_qpair() returned NULL.".into(),
        ));
    }

    log("I/O queue pair allocated. Block device is open.");

    Ok(InnerState {
        ctrlr: ctx.ctrlr,
        ns,
        qpair,
        sector_size,
        num_sectors,
    })
}

/// Close the block device: free the qpair and detach the controller.
pub fn close_device(state: InnerState) {
    log("Closing block device...");

    // SAFETY: qpair was allocated by alloc_io_qpair and has not been freed yet.
    unsafe {
        spdk_sys::spdk_nvme_ctrlr_free_io_qpair(state.qpair);
    }

    // SAFETY: ctrlr was attached by spdk_nvme_probe and has not been detached.
    unsafe {
        spdk_sys::spdk_nvme_detach(state.ctrlr);
    }

    log("Block device closed.");
}

/// Read `buf.len()` bytes starting at `lba`. Buffer must be a multiple of sector size.
///
/// Allocates a DMA buffer, submits the NVMe read, polls for completion,
/// then copies the data to the caller's buffer.
pub fn read_blocks(
    state: &InnerState,
    lba: u64,
    buf: &mut [u8],
) -> Result<(), BlockDeviceError> {
    let sector_size = state.sector_size as usize;
    if buf.is_empty() || buf.len() % sector_size != 0 {
        return Err(BlockDeviceError::BufferSizeMismatch(format!(
            "Buffer length {} is not a positive multiple of sector size {sector_size}.",
            buf.len()
        )));
    }
    let lba_count = (buf.len() / sector_size) as u32;

    // Allocate DMA-safe buffer.
    // SAFETY: spdk_dma_zmalloc returns hugepage-backed memory suitable for DMA.
    let dma_buf =
        unsafe { spdk_sys::spdk_dma_zmalloc(buf.len(), sector_size, ptr::null_mut()) };
    if dma_buf.is_null() {
        return Err(BlockDeviceError::DmaAllocationFailed(format!(
            "spdk_dma_zmalloc({}) returned NULL.",
            buf.len()
        )));
    }

    let done = AtomicBool::new(false);
    let status = AtomicI32::new(0);
    let ctx = CompletionContext {
        done: &done,
        status: &status,
    };

    // SAFETY: ns, qpair, and dma_buf are all valid. The callback context
    // lives on our stack and remains valid until we finish polling below.
    let rc = unsafe {
        spdk_sys::spdk_nvme_ns_cmd_read(
            state.ns,
            state.qpair,
            dma_buf,
            lba,
            lba_count,
            Some(io_completion_cb),
            &ctx as *const CompletionContext as *mut std::ffi::c_void,
            0,
        )
    };

    if rc != 0 {
        unsafe { spdk_sys::spdk_dma_free(dma_buf) };
        return Err(BlockDeviceError::ReadFailed(format!(
            "spdk_nvme_ns_cmd_read() returned {rc}."
        )));
    }

    // Busy-poll for completion.
    while !done.load(Ordering::Acquire) {
        // SAFETY: qpair is valid and we are the only thread using it.
        unsafe {
            spdk_sys::spdk_nvme_qpair_process_completions(state.qpair, 0);
        }
    }

    let cpl_status = status.load(Ordering::Acquire);
    if cpl_status != 0 {
        unsafe { spdk_sys::spdk_dma_free(dma_buf) };
        return Err(BlockDeviceError::ReadFailed(format!(
            "NVMe read completion status: {cpl_status:#x}."
        )));
    }

    // Copy from DMA buffer to caller's buffer.
    // SAFETY: dma_buf has buf.len() bytes, allocated above.
    unsafe {
        ptr::copy_nonoverlapping(dma_buf as *const u8, buf.as_mut_ptr(), buf.len());
        spdk_sys::spdk_dma_free(dma_buf);
    }

    Ok(())
}

/// Write `buf.len()` bytes starting at `lba`. Buffer must be a multiple of sector size.
///
/// Copies the caller's data into a DMA buffer, submits the NVMe write,
/// and polls for completion.
pub fn write_blocks(
    state: &InnerState,
    lba: u64,
    buf: &[u8],
) -> Result<(), BlockDeviceError> {
    let sector_size = state.sector_size as usize;
    if buf.is_empty() || buf.len() % sector_size != 0 {
        return Err(BlockDeviceError::BufferSizeMismatch(format!(
            "Buffer length {} is not a positive multiple of sector size {sector_size}.",
            buf.len()
        )));
    }
    let lba_count = (buf.len() / sector_size) as u32;

    // Allocate DMA-safe buffer and copy data in.
    // SAFETY: spdk_dma_zmalloc returns hugepage-backed memory suitable for DMA.
    let dma_buf =
        unsafe { spdk_sys::spdk_dma_zmalloc(buf.len(), sector_size, ptr::null_mut()) };
    if dma_buf.is_null() {
        return Err(BlockDeviceError::DmaAllocationFailed(format!(
            "spdk_dma_zmalloc({}) returned NULL.",
            buf.len()
        )));
    }

    // SAFETY: dma_buf has buf.len() bytes of valid memory.
    unsafe {
        ptr::copy_nonoverlapping(buf.as_ptr(), dma_buf as *mut u8, buf.len());
    }

    let done = AtomicBool::new(false);
    let status = AtomicI32::new(0);
    let ctx = CompletionContext {
        done: &done,
        status: &status,
    };

    // SAFETY: ns, qpair, and dma_buf are all valid. The callback context
    // lives on our stack and remains valid until we finish polling below.
    let rc = unsafe {
        spdk_sys::spdk_nvme_ns_cmd_write(
            state.ns,
            state.qpair,
            dma_buf,
            lba,
            lba_count,
            Some(io_completion_cb),
            &ctx as *const CompletionContext as *mut std::ffi::c_void,
            0,
        )
    };

    if rc != 0 {
        unsafe { spdk_sys::spdk_dma_free(dma_buf) };
        return Err(BlockDeviceError::WriteFailed(format!(
            "spdk_nvme_ns_cmd_write() returned {rc}."
        )));
    }

    // Busy-poll for completion.
    while !done.load(Ordering::Acquire) {
        // SAFETY: qpair is valid and we are the only thread using it.
        unsafe {
            spdk_sys::spdk_nvme_qpair_process_completions(state.qpair, 0);
        }
    }

    unsafe { spdk_sys::spdk_dma_free(dma_buf) };

    let cpl_status = status.load(Ordering::Acquire);
    if cpl_status != 0 {
        return Err(BlockDeviceError::WriteFailed(format!(
            "NVMe write completion status: {cpl_status:#x}."
        )));
    }

    Ok(())
}

// ---------------------------------------------------------------------------
// Internal helpers
// ---------------------------------------------------------------------------

/// Context passed through the NVMe I/O completion callback.
struct CompletionContext<'a> {
    done: &'a AtomicBool,
    status: &'a AtomicI32,
}

/// NVMe I/O completion callback. Sets the done flag and records the status.
///
/// # Safety
///
/// Called from C code via `spdk_nvme_qpair_process_completions`.
/// `cb_arg` must point to a valid `CompletionContext` on the caller's stack.
unsafe extern "C" fn io_completion_cb(
    cb_arg: *mut std::ffi::c_void,
    cpl: *const spdk_sys::spdk_nvme_cpl,
) {
    let ctx = &*(cb_arg as *const CompletionContext);
    let cpl_status = if cpl.is_null() {
        -1
    } else {
        let status = (*cpl).__bindgen_anon_1.status;
        let raw: u16 = std::mem::transmute(status);
        // Mask off the phase bit (bit 0) and DNR bit (bit 15).
        let masked = (raw >> 1) & 0x3FFF;
        masked as i32
    };
    ctx.status.store(cpl_status, Ordering::Release);
    ctx.done.store(true, Ordering::Release);
}

/// NVMe probe callback: accept all controllers.
///
/// # Safety
///
/// Called from C code via `spdk_nvme_probe`.
unsafe extern "C" fn probe_cb(
    _cb_ctx: *mut std::ffi::c_void,
    _trid: *const spdk_sys::spdk_nvme_transport_id,
    _opts: *mut spdk_sys::spdk_nvme_ctrlr_opts,
) -> bool {
    true
}

/// NVMe attach callback: capture the first controller.
///
/// # Safety
///
/// Called from C code via `spdk_nvme_probe`.
unsafe extern "C" fn attach_cb(
    cb_ctx: *mut std::ffi::c_void,
    _trid: *const spdk_sys::spdk_nvme_transport_id,
    ctrlr: *mut spdk_sys::spdk_nvme_ctrlr,
    _opts: *const spdk_sys::spdk_nvme_ctrlr_opts,
) {
    let ctx = &mut *(cb_ctx as *mut ProbeContext);
    if ctx.ctrlr.is_null() {
        ctx.ctrlr = ctrlr;
    }
}

fn log(msg: &str) {
    eprintln!("[spdk-simple-block-device] {msg}");
}
