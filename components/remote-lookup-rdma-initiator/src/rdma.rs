//! RDMA resource management and connection handling.
//!
//! Provides both real RDMA operations (via rdma-core FFI) and mock
//! implementations for unit testing without hardware.

use std::ffi::{c_void, CString};
use std::os::raw::c_int;
use std::ptr;

use anyhow::{bail, Context, Result};

use crate::ffi;

const MAX_CQ_ENTRIES: c_int = 128;
const MAX_SEND_WR: u32 = 128;
const MAX_RECV_WR: u32 = 64;

/// Largest number of RDMA writes that may be outstanding on one queue pair at a
/// time. Every write is signaled, so the bound is whichever of the send queue and
/// the completion queue fills first.
///
/// This is the whole point of posting a window rather than one write at a time: a
/// 64 KiB write is ~2.6 µs of wire time on a 200G link but ~28 µs of post/poll
/// overhead, so serializing on each completion caps a flow near 9% of line rate
/// regardless of how much work the caller offers.
pub const WINDOW: usize = if (MAX_SEND_WR as usize) < (MAX_CQ_ENTRIES as usize) {
    MAX_SEND_WR as usize
} else {
    MAX_CQ_ENTRIES as usize
};

// Wall-clock cap on the busy-poll for a whole window's completions — not per
// completion, which would multiply the cap by the window depth and undo the fast
// reconnect this exists for. rdma_cm owns the QP's RTR/RTS transition and leaves
// the hardware ACK timeout at its (large) default, so the first write on a
// stale/idle warm connection would otherwise burn ~15s of retransmit before
// RETRY_EXC. This software cap abandons a stuck window far sooner, letting the
// caller's reconnect-and-replay path rebuild a fresh QP within the operation
// deadline. It sits far above healthy latency (a full 128 x 4 MiB window is
// ~milliseconds on 200G RoCE), so only a genuinely stuck window trips it. (The QP
// ACK timeout itself is not tunable here: `ibv_modify_qp` cannot change
// `timeout`/`retry_cnt` in the RTS→RTS transition, and rdma_cm has already
// reached RTS by ESTABLISHED.)
const POLL_TIMEOUT_SECS: u64 = 2;

/// A zeroed work completion, for buffers `ibv_poll_cq` is about to fill.
///
/// Written out field-by-field rather than derived because `ffi::ibv_wc` is a bare
/// `#[repr(C)]` mirror of the rdma-core struct, matching the rest of `ffi.rs`.
fn blank_wc() -> ffi::ibv_wc {
    ffi::ibv_wc {
        wr_id: 0,
        status: 0,
        opcode: 0,
        vendor_err: 0,
        byte_len: 0,
        imm_data: 0,
        qp_num: 0,
        src_qp: 0,
        wc_flags: 0,
        pkey_index: 0,
        slid: 0,
        sl: 0,
        dlid_path_bits: 0,
    }
}

/// Human-readable name for an `ibv_wc_status` code, so completion-error logs are
/// diagnosable without cross-referencing rdma-core headers. `RETRY_EXC_ERR`
/// (transport retry exhausted) and `RNR_RETRY_EXC_ERR` are the two that most
/// often signal a peer/QP problem rather than a local one.
fn wc_status_name(status: c_int) -> &'static str {
    match status {
        0 => "SUCCESS",
        1 => "LOC_LEN_ERR",
        2 => "LOC_QP_OP_ERR",
        3 => "LOC_EEC_OP_ERR",
        4 => "LOC_PROT_ERR",
        5 => "WR_FLUSH_ERR",
        6 => "MW_BIND_ERR",
        7 => "BAD_RESP_ERR",
        8 => "LOC_ACCESS_ERR",
        9 => "REM_INV_REQ_ERR",
        10 => "REM_ACCESS_ERR",
        11 => "REM_OP_ERR",
        12 => "RETRY_EXC_ERR",
        13 => "RNR_RETRY_EXC_ERR",
        14 => "LOC_RDD_VIOL_ERR",
        15 => "REM_INV_RD_REQ_ERR",
        16 => "REM_ABORT_ERR",
        17 => "INV_EECN_ERR",
        18 => "INV_EEC_STATE_ERR",
        19 => "FATAL_ERR",
        20 => "RESP_TIMEOUT_ERR",
        21 => "GENERAL_ERR",
        _ => "UNKNOWN",
    }
}

/// Human-readable name for an `ibv_qp_state` code (the `IBV_QPS_*` constants).
fn qp_state_name(state: c_int) -> &'static str {
    match state {
        ffi::IBV_QPS_RESET => "RESET",
        ffi::IBV_QPS_INIT => "INIT",
        ffi::IBV_QPS_RTR => "RTR",
        ffi::IBV_QPS_RTS => "RTS",
        ffi::IBV_QPS_SQD => "SQD",
        ffi::IBV_QPS_SQE => "SQE",
        ffi::IBV_QPS_ERR => "ERR",
        _ => "UNKNOWN",
    }
}

/// A registered memory region for RDMA operations.
///
/// Can own a buffer (allocated by `register_mr`) or borrow an external
/// pointer (registered by `register_existing_mr`).
pub struct MemoryRegion {
    mr: *mut ffi::ibv_mr,
    /// Owned buffer (None for externally-borrowed regions).
    pub buf: Vec<u8>,
    /// Address of the registered region (for borrowed regions, this is the external pointer).
    registered_addr: *const u8,
    /// Length of the registered region.
    registered_len: usize,
}

// SAFETY: MemoryRegion is only accessed from the thread that created it
// or via synchronization in Session. The underlying ibv_mr is thread-safe
// for the operations we perform (read addr/lkey/rkey).
unsafe impl Send for MemoryRegion {}

impl Drop for MemoryRegion {
    fn drop(&mut self) {
        if !self.mr.is_null() {
            // SAFETY: mr was allocated by ibv_reg_mr and is valid until deregistered.
            unsafe {
                ffi::ibv_dereg_mr(self.mr);
            }
        }
    }
}

impl MemoryRegion {
    pub fn addr(&self) -> u64 {
        self.registered_addr as u64
    }

    pub fn rkey(&self) -> u32 {
        // SAFETY: mr is valid (checked non-null in register_mr).
        unsafe { (*self.mr).rkey }
    }

    pub fn lkey(&self) -> u32 {
        // SAFETY: mr is valid (checked non-null in register_mr).
        unsafe { (*self.mr).lkey }
    }

    pub fn len(&self) -> usize {
        self.registered_len
    }

    pub fn is_empty(&self) -> bool {
        self.registered_len == 0
    }
}

/// An RDMA connection (server or client side).
pub struct RdmaConnection {
    cm_id: *mut ffi::rdma_cm_id,
    pd: *mut ffi::ibv_pd,
    cq: *mut ffi::ibv_cq,
    qp: *mut ffi::ibv_qp,
    channel: *mut ffi::rdma_event_channel,
}

// SAFETY: RdmaConnection is accessed from a single session task.
// The underlying RDMA resources are thread-safe for our usage pattern
// (one poster, one poller, same thread).
unsafe impl Send for RdmaConnection {}

impl Drop for RdmaConnection {
    fn drop(&mut self) {
        // SAFETY: All pointers were allocated by rdma-core and are valid.
        unsafe {
            if !self.cm_id.is_null() {
                ffi::rdma_disconnect(self.cm_id);
                if !self.qp.is_null() {
                    ffi::rdma_destroy_qp(self.cm_id);
                }
                ffi::rdma_destroy_id(self.cm_id);
            }
            if !self.cq.is_null() {
                ffi::ibv_destroy_cq(self.cq);
            }
            if !self.pd.is_null() {
                ffi::ibv_dealloc_pd(self.pd);
            }
            if !self.channel.is_null() {
                ffi::rdma_destroy_event_channel(self.channel);
            }
        }
    }
}

impl RdmaConnection {
    /// Return the current queue-pair state (one of the `IBV_QPS_*` constants).
    pub fn qp_state(&self) -> c_int {
        // SAFETY: qp is valid for the lifetime of the connection (checked
        // non-null at creation in create_qp).
        unsafe { (*self.qp).state }
    }

    /// Returns `true` unless the queue pair has entered the error state.
    ///
    /// A QP in `IBV_QPS_ERR` cannot post further work requests successfully;
    /// callers should tear the connection down and reconnect.
    pub fn is_qp_healthy(&self) -> bool {
        self.qp_state() != ffi::IBV_QPS_ERR
    }

    /// Register a new memory region for RDMA access (allocates buffer).
    pub fn register_mr(&self, size: usize) -> Result<MemoryRegion> {
        let mut buf = vec![0u8; size];
        let access = ffi::IBV_ACCESS_LOCAL_WRITE
            | ffi::IBV_ACCESS_REMOTE_WRITE
            | ffi::IBV_ACCESS_REMOTE_READ;

        let addr = buf.as_mut_ptr();
        // SAFETY: pd is valid, buf is a valid allocation of `size` bytes.
        let mr = unsafe { ffi::ibv_reg_mr(self.pd, addr as *mut c_void, size, access) };
        if mr.is_null() {
            bail!("ibv_reg_mr failed");
        }

        Ok(MemoryRegion {
            mr,
            buf,
            registered_addr: addr,
            registered_len: size,
        })
    }

    /// Register an existing memory address as an RDMA memory region.
    /// The caller retains ownership of the memory — it must remain valid
    /// until this MemoryRegion is dropped (which deregisters the MR).
    pub fn register_existing_mr(&self, addr: *const u8, len: usize) -> Result<MemoryRegion> {
        let access = ffi::IBV_ACCESS_LOCAL_WRITE
            | ffi::IBV_ACCESS_REMOTE_WRITE
            | ffi::IBV_ACCESS_REMOTE_READ;

        // SAFETY: pd is valid, addr points to `len` bytes of valid memory
        // owned by the caller (e.g., memory-tier pool).
        let mr = unsafe { ffi::ibv_reg_mr(self.pd, addr as *mut c_void, len, access) };
        if mr.is_null() {
            bail!(
                "ibv_reg_mr failed for existing address {:p} len {}",
                addr,
                len
            );
        }

        Ok(MemoryRegion {
            mr,
            buf: Vec::new(),
            registered_addr: addr,
            registered_len: len,
        })
    }

    /// Post one RDMA Write from within the pre-registered pool MR **without**
    /// waiting for its completion, so a whole window can be in flight at once.
    ///
    /// `wr_id` is echoed back by [`reap`](Self::reap); callers pass the write's
    /// index within its window. Posting can fail locally (a full send queue), in
    /// which case nothing was queued for this write.
    ///
    /// # Safety
    ///
    /// `local_addr` must point to `len` valid bytes inside `pool_mr`'s registered
    /// region, and must stay valid until the matching completion is reaped — the
    /// NIC reads the buffer asynchronously.
    pub unsafe fn post_write_from_pool(
        &self,
        pool_mr: &MemoryRegion,
        local_addr: *const u8,
        len: usize,
        remote_addr: u64,
        rkey: u32,
        wr_id: u64,
    ) -> Result<()> {
        self.post_write(local_addr, len, pool_mr.lkey(), remote_addr, rkey, wr_id)
    }

    /// Shared posting path for both MR flavors.
    ///
    /// # Safety
    ///
    /// As [`post_write_from_pool`](Self::post_write_from_pool); `lkey` must belong
    /// to the MR covering `local_addr`.
    unsafe fn post_write(
        &self,
        local_addr: *const u8,
        len: usize,
        lkey: u32,
        remote_addr: u64,
        rkey: u32,
        wr_id: u64,
    ) -> Result<()> {
        let ret = ffi::rdma_test_rdma_write(
            self.qp,
            local_addr as *mut c_void,
            len as u32,
            lkey,
            remote_addr,
            rkey,
            wr_id,
        );
        if ret != 0 {
            bail!(
                "ibv_post_send (RDMA_WRITE) failed: {} (wr_id={}, len={}, \
                 remote_addr=0x{:x})",
                ret,
                wr_id,
                len,
                remote_addr
            );
        }
        Ok(())
    }

    /// Reap exactly `count` completions, failing on the first unsuccessful one.
    ///
    /// The poll-timeout cap bounds the *whole* call, not each completion, so a deep
    /// window cannot stretch the abandon-and-reconnect deadline.
    ///
    /// Returning `Err` means the window is lost, not that one member of it is: a
    /// failing RDMA_WRITE drives the queue pair into the error state, which flushes
    /// every other outstanding work request with `WR_FLUSH_ERR`. There is
    /// deliberately no attempt to sort the original failure from the flushed
    /// bystanders — the caller replays the whole window instead, which is safe
    /// because the remote landing buffers stay reserved and unpublished until the
    /// caller reports per-key status.
    pub fn reap(&self, count: usize) -> Result<()> {
        if count == 0 {
            return Ok(());
        }

        let mut wcs: Vec<ffi::ibv_wc> = (0..count).map(|_| blank_wc()).collect();
        let start = std::time::Instant::now();
        let timeout = std::time::Duration::from_secs(POLL_TIMEOUT_SECS);
        let mut reaped = 0usize;

        while reaped < count {
            let want = (count - reaped) as c_int;
            // SAFETY: cq is valid and wcs has `count` entries, so writing up to
            // `want` entries starting at `reaped` stays in bounds.
            let ret = unsafe { ffi::rdma_test_poll_cq(self.cq, want, wcs[reaped..].as_mut_ptr()) };
            if ret < 0 {
                bail!("ibv_poll_cq failed: {}", ret);
            }
            if ret > 0 {
                let got = ret as usize;
                for wc in &wcs[reaped..reaped + got] {
                    if wc.status != ffi::IBV_WC_SUCCESS {
                        // SAFETY: qp is valid for the connection's lifetime.
                        let qp_state = unsafe { (*self.qp).state };
                        bail!(
                            "work completion error: status={} ({}), opcode={}, \
                             vendor_err=0x{:x}, wr_id={}, wc.qp_num={}, qp_state={} ({}), \
                             window={}, reaped={}",
                            wc.status,
                            wc_status_name(wc.status),
                            wc.opcode,
                            wc.vendor_err,
                            wc.wr_id,
                            wc.qp_num,
                            qp_state,
                            qp_state_name(qp_state),
                            count,
                            reaped,
                        );
                    }
                }
                reaped += got;
                continue;
            }
            if start.elapsed() > timeout {
                bail!(
                    "reap timed out after {}s with {}/{} completions",
                    POLL_TIMEOUT_SECS,
                    reaped,
                    count
                );
            }
            std::hint::spin_loop();
        }
        Ok(())
    }

    /// Drain any pending completions from the CQ.
    pub fn drain_cq(&self) {
        let mut wc = blank_wc();
        loop {
            // SAFETY: cq is valid.
            let ret = unsafe { ffi::rdma_test_poll_cq(self.cq, 1, &mut wc) };
            if ret <= 0 {
                break;
            }
        }
    }
}

// --- Connection establishment ---

fn wait_for_event(
    channel: *mut ffi::rdma_event_channel,
    expected: c_int,
) -> Result<*mut ffi::rdma_cm_event> {
    let mut event: *mut ffi::rdma_cm_event = ptr::null_mut();
    // SAFETY: channel is valid, event is an out-pointer.
    let ret = unsafe { ffi::rdma_get_cm_event(channel, &mut event) };
    if ret != 0 {
        bail!("rdma_get_cm_event failed: {}", ret);
    }
    // SAFETY: event is valid after successful rdma_get_cm_event.
    let event_type = unsafe { (*event).event };
    if event_type != expected {
        unsafe { ffi::rdma_ack_cm_event(event) };
        bail!(
            "unexpected CM event: got {}, expected {}",
            event_type,
            expected
        );
    }
    Ok(event)
}

fn create_qp(
    cm_id: *mut ffi::rdma_cm_id,
    pd: *mut ffi::ibv_pd,
    cq: *mut ffi::ibv_cq,
) -> Result<*mut ffi::ibv_qp> {
    let mut init_attr = ffi::ibv_qp_init_attr {
        qp_context: ptr::null_mut(),
        send_cq: cq,
        recv_cq: cq,
        srq: ptr::null_mut(),
        cap: ffi::ibv_qp_cap {
            max_send_wr: MAX_SEND_WR,
            max_recv_wr: MAX_RECV_WR,
            max_send_sge: 1,
            max_recv_sge: 1,
            max_inline_data: 0,
        },
        qp_type: ffi::IBV_QPT_RC,
        sq_sig_all: 0,
    };

    // SAFETY: cm_id, pd are valid pointers.
    let ret = unsafe { ffi::rdma_create_qp(cm_id, pd, &mut init_attr) };
    if ret != 0 {
        bail!("rdma_create_qp failed: {}", ret);
    }

    // SAFETY: cm_id is valid after successful rdma_create_qp.
    let qp = unsafe { (*cm_id).qp };
    if qp.is_null() {
        bail!("rdma_create_qp returned success but QP is null");
    }
    Ok(qp)
}

/// Connect to a remote RDMA handler as a client.
/// Per-phase timing of [`client_connect`]'s `rdma_cm` round-trips, in
/// microseconds. Fed into the initiator telemetry to attribute cold-connect
/// latency. `handshake_us` covers QP/CQ/PD setup plus the connect handshake.
#[derive(Debug, Clone, Copy, Default)]
pub struct CmTiming {
    pub resolve_addr_us: u64,
    pub resolve_route_us: u64,
    pub handshake_us: u64,
}

pub fn client_connect(
    addr: &str,
    port: u16,
    private_data: &[u8],
) -> Result<(RdmaConnection, CmTiming)> {
    // SAFETY: No preconditions.
    let channel = unsafe { ffi::rdma_create_event_channel() };
    if channel.is_null() {
        bail!("rdma_create_event_channel failed");
    }

    let mut cm_id: *mut ffi::rdma_cm_id = ptr::null_mut();
    // SAFETY: channel is valid.
    let ret =
        unsafe { ffi::rdma_create_id(channel, &mut cm_id, ptr::null_mut(), ffi::RDMA_PS_TCP) };
    if ret != 0 {
        bail!("rdma_create_id failed: {}", ret);
    }

    let addr_c = CString::new(addr).context("invalid address")?;
    let mut sin = ffi::sockaddr_in {
        sin_family: ffi::AF_INET,
        sin_port: unsafe { ffi::htons(port) },
        sin_addr: ffi::in_addr {
            s_addr: unsafe { ffi::inet_addr(addr_c.as_ptr()) },
        },
        sin_zero: [0; 8],
    };

    let mut src_sin = ffi::sockaddr_in {
        sin_family: ffi::AF_INET,
        sin_port: 0,
        sin_addr: ffi::in_addr { s_addr: 0 },
        sin_zero: [0; 8],
    };

    // SAFETY: cm_id is valid, sin is initialized.
    let addr_start = std::time::Instant::now();
    let ret = unsafe {
        ffi::rdma_resolve_addr(
            cm_id,
            &mut src_sin as *mut ffi::sockaddr_in as *mut ffi::sockaddr,
            &mut sin as *mut ffi::sockaddr_in as *mut ffi::sockaddr,
            2000,
        )
    };
    if ret != 0 {
        bail!("rdma_resolve_addr failed: {}", ret);
    }

    let event = wait_for_event(channel, ffi::RDMA_CM_EVENT_ADDR_RESOLVED)?;
    unsafe { ffi::rdma_ack_cm_event(event) };
    let resolve_addr_us = addr_start.elapsed().as_micros() as u64;

    // SAFETY: cm_id has resolved address.
    let route_start = std::time::Instant::now();
    let ret = unsafe { ffi::rdma_resolve_route(cm_id, 2000) };
    if ret != 0 {
        bail!("rdma_resolve_route failed: {}", ret);
    }

    let event = wait_for_event(channel, ffi::RDMA_CM_EVENT_ROUTE_RESOLVED)?;
    unsafe { ffi::rdma_ack_cm_event(event) };
    let resolve_route_us = route_start.elapsed().as_micros() as u64;

    // Everything from here through ESTABLISHED is the handshake bucket (QP/CQ/PD
    // setup plus the connect round-trip).
    let handshake_start = std::time::Instant::now();
    // SAFETY: cm_id has resolved route, verbs context is available.
    let ctx = unsafe { (*cm_id).verbs };
    if ctx.is_null() {
        bail!("no verbs context on CM ID");
    }

    let pd = unsafe { ffi::ibv_alloc_pd(ctx) };
    if pd.is_null() {
        bail!("ibv_alloc_pd failed");
    }

    let cq =
        unsafe { ffi::ibv_create_cq(ctx, MAX_CQ_ENTRIES, ptr::null_mut(), ptr::null_mut(), 0) };
    if cq.is_null() {
        bail!("ibv_create_cq failed");
    }

    let qp = create_qp(cm_id, pd, cq)?;

    // Stamp this node's zyre PeerId into the connect private_data so the remote
    // responder can correlate the inbound QP to this peer. private_data_len is a
    // u8; a zyre UUID is well under 255 bytes. An empty id leaves it unstamped.
    let pd_len = private_data.len().min(u8::MAX as usize);
    let mut conn_param = ffi::rdma_conn_param {
        private_data: if pd_len == 0 {
            ptr::null()
        } else {
            private_data.as_ptr() as *const c_void
        },
        private_data_len: pd_len as u8,
        responder_resources: 1,
        initiator_depth: 1,
        flow_control: 0,
        retry_count: 7,
        rnr_retry_count: 7,
        srq: 0,
        qp_num: 0,
    };

    // SAFETY: cm_id and conn_param are valid; private_data (if any) outlives this
    // synchronous rdma_connect call, which copies it into the connect request.
    let ret = unsafe { ffi::rdma_connect(cm_id, &mut conn_param) };
    if ret != 0 {
        bail!("rdma_connect failed: {}", ret);
    }

    let event = wait_for_event(channel, ffi::RDMA_CM_EVENT_ESTABLISHED)?;
    unsafe { ffi::rdma_ack_cm_event(event) };
    let handshake_us = handshake_start.elapsed().as_micros() as u64;

    let conn = RdmaConnection {
        cm_id,
        pd,
        cq,
        qp,
        channel,
    };
    conn.drain_cq();
    let timing = CmTiming {
        resolve_addr_us,
        resolve_route_us,
        handshake_us,
    };
    Ok((conn, timing))
}
