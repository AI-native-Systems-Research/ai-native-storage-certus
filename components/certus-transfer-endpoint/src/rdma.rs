//! Safe wrappers over the verbs FFI: protection domain, memory regions,
//! completion queues, connection-manager ids and events, queue pairs, posting
//! and completion polling.
//!
//! # Ownership, and why it is reference-counted
//!
//! The protection domain is owned by the **endpoint**, one per device, for the
//! endpoint's lifetime — memory regions are registered into it once and outlive
//! every queue pair, so an `rkey` already on the wire stays valid across a
//! reconnect (research D17). This is a deliberate departure from the predecessor
//! crates, where the PD lives in the connection and the pool is re-registered on
//! every connect.
//!
//! D17's cost is a destruction-order constraint: `ibv_dealloc_pd` fails `EBUSY`
//! while any queue pair or region references the PD, and `ibv_destroy_cq` while
//! any queue pair references the CQ. So every [`MemoryRegion`],
//! [`CompletionQueue`] and [`QueuePair`] holds an `Arc` of what it depends on,
//! and the order is enforced by reference counting rather than by convention.
//! A release that still fails is therefore a defect here, not a transient
//! condition, and panics (Principle XI).
//!
//! # What is not here
//!
//! * **No manual INIT → RTR → RTS.** `rdma_create_qp` and
//!   `rdma_connect`/`rdma_accept` drive those transitions; only the transition
//!   to ERROR is explicit ([`QueuePair::to_error`]), because it is what makes a
//!   late write impossible, which the deadline path depends on (FR-023).
//! * **No connect/accept state machine** — that is the verbs transport
//!   (`connection::verbs`), over the T010 seam.
//! * **No PD policy.** [`ProtectionDomain`] owns one PD on one device; the
//!   verbs transport makes exactly one, on the device it listens on.
//! * **No CQ-sharing policy here.** A queue pair takes any CQ for each side; the
//!   seam (`connection::transport`) decides one CQ per link.
//! * **No seam types.** Errors, access flags, QP state and completions are the
//!   seam's, shared with the mock; this module adds only what is verbs-specific.
//!
//! # Device context lifetime
//!
//! The `ibv_context` used here is librdmacm's per-device cache, reached through
//! a bound or resolved [`CmId`]. It is never closed by this crate: librdmacm
//! keeps it open for the life of the process and closes it only after the
//! device has been removed, which D17 already treats as fatal to every region
//! and queue pair. A PD must come from *this* context — `rdma_create_qp`
//! rejects a PD whose context pointer differs from the id's, even on the same
//! device — so opening a second context with `ibv_open_device` would not work.

// Consumed from T010/T053 onward.
#![allow(dead_code)]

use std::ffi::c_void;
use std::fmt;
use std::net::SocketAddrV4;
use std::os::raw::c_int;
use std::ptr;
use std::sync::Arc;

use crate::connection::transport::{
    check_range, wc_status_name, Access, Completed, Completion, CompletionError, PostError, QpCaps,
    QpState, RdmaError, SendOp,
};
use crate::ffi;

// --- errors ------------------------------------------------------------------

const EAGAIN: i32 = 11;

/// Capture `errno` for a call that just failed.
fn last_error(op: &'static str) -> RdmaError {
    RdmaError::Verb {
        op,
        errno: std::io::Error::last_os_error().raw_os_error().unwrap_or(0),
    }
}

/// Panic on a failed release unless already unwinding. With the `Arc`
/// ordering above, a release can only fail through a defect in this crate (or
/// device removal, which is fatal anyway), and Principle XI prefers a crash to
/// a leaked PD that later makes every `ibv_dealloc_pd` fail.
fn released(op: &'static str, ret: c_int) {
    if ret != 0 && !std::thread::panicking() {
        panic!("{op} failed during release: {ret} ({})", last_error(op));
    }
}

// --- protection domain -------------------------------------------------------

/// One protection domain on one device (D17).
pub struct ProtectionDomain {
    ctx: *mut ffi::ibv_context,
    pd: *mut ffi::ibv_pd,
}

// SAFETY: `ibv_alloc_pd`, `ibv_reg_mr`, `ibv_create_cq`, `rdma_create_qp` and
// `ibv_dealloc_pd` are thread-safe in libibverbs, and Rust never reads or
// writes through either pointer after construction; they are only passed back
// to those calls. Sharing one PD across per-peer threads is the point of D17.
unsafe impl Send for ProtectionDomain {}
// SAFETY: as for `Send` — `&ProtectionDomain` exposes no mutation.
unsafe impl Sync for ProtectionDomain {}

impl ProtectionDomain {
    /// Allocate a PD on the device `id` is bound or resolved to.
    pub fn new(id: &CmId) -> Result<Arc<Self>, RdmaError> {
        let ctx = id.verbs();
        if ctx.is_null() {
            return Err(RdmaError::Invalid {
                op: "ibv_alloc_pd",
                reason: "CM id is not bound or resolved to a device",
            });
        }
        // SAFETY: `ctx` is librdmacm's live device context (module docs).
        let pd = unsafe { ffi::ibv_alloc_pd(ctx) };
        if pd.is_null() {
            return Err(last_error("ibv_alloc_pd"));
        }
        Ok(Arc::new(ProtectionDomain { ctx, pd }))
    }

    /// Whether `id` is on this PD's device — the condition `rdma_create_qp`
    /// enforces, and the key T069 looks PDs up by.
    pub fn same_device(&self, id: &CmId) -> bool {
        self.ctx == id.verbs()
    }
}

impl Drop for ProtectionDomain {
    fn drop(&mut self) {
        // SAFETY: `pd` came from `ibv_alloc_pd` and is released once, here.
        // Every region, CQ and QP holds an `Arc<Self>`, so none remains.
        released("ibv_dealloc_pd", unsafe { ffi::ibv_dealloc_pd(self.pd) });
    }
}

// --- memory regions ----------------------------------------------------------

// The seam's `Access` carries the verbs values directly.
const _: () = {
    assert!(Access::LOCAL_WRITE.0 == ffi::IBV_ACCESS_LOCAL_WRITE);
    assert!(Access::REMOTE_WRITE.0 == ffi::IBV_ACCESS_REMOTE_WRITE);
    assert!(Access::REMOTE_READ.0 == ffi::IBV_ACCESS_REMOTE_READ);
};

/// A registration of caller-owned memory into a [`ProtectionDomain`].
///
/// `addr`, `len`, `lkey` and `rkey` are copied out at registration, so reading
/// them never touches the `ibv_mr`.
pub struct MemoryRegion {
    _pd: Arc<ProtectionDomain>,
    mr: *mut ffi::ibv_mr,
    addr: u64,
    len: usize,
    lkey: u32,
    rkey: u32,
}

// SAFETY: unlike the initiator's region, which was justified by being used only
// on the thread that created it, this one is registered once and used by every
// per-peer connection thread (D17). That is sound because the `ibv_mr` is
// immutable after `ibv_reg_mr`: its keys and bounds are copied into plain fields
// and the pointer is used only by `ibv_dereg_mr` in `Drop`, which has exclusive
// access. The memory the region covers is the caller's, governed by the
// contract on `register` and on each post, not by this type.
unsafe impl Send for MemoryRegion {}
// SAFETY: as for `Send` — `&MemoryRegion` exposes only the copied fields.
unsafe impl Sync for MemoryRegion {}

impl MemoryRegion {
    /// Register `[addr, addr + len)` with `access`.
    ///
    /// # Safety
    ///
    /// The memory must stay allocated until this region is dropped. The NIC
    /// may write it at any time the flags allow (a peer, under
    /// `REMOTE_WRITE`), so the caller must also not treat it as exclusively
    /// owned Rust data, e.g. hold a `&mut [u8]` over it, while that can happen.
    pub unsafe fn register(
        pd: &Arc<ProtectionDomain>,
        addr: *mut u8,
        len: usize,
        access: Access,
    ) -> Result<Self, RdmaError> {
        if addr.is_null() || len == 0 {
            return Err(RdmaError::Invalid {
                op: "ibv_reg_mr",
                reason: "empty or null region",
            });
        }
        // SAFETY: `pd` is live; the range is valid per this function's contract.
        let mr = ffi::ibv_reg_mr(pd.pd, addr as *mut c_void, len, access.0);
        if mr.is_null() {
            return Err(last_error("ibv_reg_mr"));
        }
        // SAFETY: `ibv_reg_mr` succeeded, so `mr` points to a filled `ibv_mr`.
        let (lkey, rkey) = ((*mr).lkey, (*mr).rkey);
        Ok(MemoryRegion {
            _pd: Arc::clone(pd),
            mr,
            addr: addr as u64,
            len,
            lkey,
            rkey,
        })
    }

    pub fn addr(&self) -> u64 {
        self.addr
    }

    pub fn len(&self) -> usize {
        self.len
    }

    pub fn lkey(&self) -> u32 {
        self.lkey
    }

    /// The key a peer needs to write here; what a landing-region handle and a
    /// wire entry carry.
    pub fn rkey(&self) -> u32 {
        self.rkey
    }

    /// A bounds-checked buffer `[addr, addr + len)` inside this region, for use
    /// as the local side of a work request. Addresses are absolute, as on the
    /// wire.
    pub fn local(&self, addr: u64, len: usize) -> Result<LocalBuf<'_>, RdmaError> {
        check_range(self.addr, self.len, addr, len)?;
        Ok(LocalBuf {
            region: self,
            addr,
            len: len as u32,
        })
    }
}

impl Drop for MemoryRegion {
    fn drop(&mut self) {
        // SAFETY: `mr` came from `ibv_reg_mr` and is released once, here.
        released("ibv_dereg_mr", unsafe { ffi::ibv_dereg_mr(self.mr) });
    }
}

/// A buffer inside a [`MemoryRegion`], checked at construction; the region
/// supplies the `lkey`, so a caller cannot pair a buffer with the wrong key.
#[derive(Clone, Copy)]
pub struct LocalBuf<'a> {
    region: &'a MemoryRegion,
    addr: u64,
    len: u32,
}

impl LocalBuf<'_> {
    pub fn raw(&self) -> RawSge {
        RawSge {
            addr: self.addr,
            len: self.len,
            lkey: self.region.lkey,
        }
    }
}

/// A scatter/gather entry as verbs takes it. Built only from an already
/// bounds-checked buffer: [`LocalBuf::raw`] here, or the seam's `Sge` once the
/// verbs transport (T053) has matched its `lkey` to a region it holds.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RawSge {
    pub addr: u64,
    pub len: u32,
    pub lkey: u32,
}

impl RawSge {
    fn to_ffi(self) -> ffi::ibv_sge {
        ffi::ibv_sge {
            addr: self.addr,
            length: self.len,
            lkey: self.lkey,
        }
    }
}

// --- completion queues -------------------------------------------------------

/// A completion queue on a PD's device.
pub struct CompletionQueue {
    _pd: Arc<ProtectionDomain>,
    cq: *mut ffi::ibv_cq,
    depth: usize,
}

// SAFETY: `ibv_poll_cq` and `ibv_destroy_cq` are thread-safe in libibverbs (the
// provider locks the CQ), and Rust never dereferences `cq`.
unsafe impl Send for CompletionQueue {}
// SAFETY: as for `Send`.
unsafe impl Sync for CompletionQueue {}

impl CompletionQueue {
    /// Create a CQ of at least `depth` entries on `pd`'s device. Held by the PD
    /// only to share its device context and destruction order.
    pub fn new(pd: &Arc<ProtectionDomain>, depth: usize) -> Result<Arc<Self>, RdmaError> {
        let cqe = c_int::try_from(depth).map_err(|_| RdmaError::Invalid {
            op: "ibv_create_cq",
            reason: "depth exceeds c_int",
        })?;
        // SAFETY: `pd.ctx` is the live device context; no channel, vector 0.
        let cq = unsafe { ffi::ibv_create_cq(pd.ctx, cqe, ptr::null_mut(), ptr::null_mut(), 0) };
        if cq.is_null() {
            return Err(last_error("ibv_create_cq"));
        }
        Ok(Arc::new(CompletionQueue {
            _pd: Arc::clone(pd),
            cq,
            depth,
        }))
    }

    /// Requested depth. Send-queue credits (T014) must not exceed it, or a full
    /// CQ overruns and the device moves every attached QP to ERROR.
    pub fn depth(&self) -> usize {
        self.depth
    }

    /// Reap up to `out.len()` ready completions without blocking; returns how
    /// many were written to the front of `out`, possibly zero. A failed
    /// completion is data, not `Err` — `Err` means `ibv_poll_cq` itself failed.
    pub fn poll(&self, out: &mut [WorkCompletion]) -> Result<usize, RdmaError> {
        let n = c_int::try_from(out.len()).unwrap_or(c_int::MAX);
        // SAFETY: `cq` is live, and `WorkCompletion` is `repr(transparent)` over
        // `ibv_wc`, so `out` is `n` writable `ibv_wc`s.
        let ret = unsafe { ffi::certus_te_poll_cq(self.cq, n, out.as_mut_ptr().cast()) };
        if ret < 0 {
            return Err(RdmaError::Verb {
                op: "ibv_poll_cq",
                errno: -ret,
            });
        }
        Ok(ret as usize)
    }
}

impl Drop for CompletionQueue {
    fn drop(&mut self) {
        // SAFETY: `cq` came from `ibv_create_cq` and is released once, here.
        // Every QP on it holds an `Arc<Self>`, so none remains.
        released("ibv_destroy_cq", unsafe { ffi::ibv_destroy_cq(self.cq) });
    }
}

/// One work completion, read in place from the CQ.
#[repr(transparent)]
pub struct WorkCompletion(ffi::ibv_wc);

impl Default for WorkCompletion {
    fn default() -> Self {
        // SAFETY: `ibv_wc` is plain integers; all-zero is a valid value.
        WorkCompletion(unsafe { std::mem::zeroed() })
    }
}

impl WorkCompletion {
    pub fn wr_id(&self) -> u64 {
        self.0.wr_id
    }

    pub fn status(&self) -> c_int {
        self.0.status
    }

    pub fn is_success(&self) -> bool {
        self.0.status == ffi::IBV_WC_SUCCESS
    }

    /// The expected status of everything still queued when a QP is forced to
    /// ERROR — what a post-deadline drain (FR-023) sees, as opposed to the
    /// one completion that names an original fault.
    pub fn is_flushed(&self) -> bool {
        self.0.status == ffi::IBV_WC_WR_FLUSH_ERR
    }

    /// Whether this completes a receive rather than a send or write. Only
    /// meaningful on success: on error the opcode field is undefined.
    pub fn is_recv(&self) -> bool {
        self.0.opcode & ffi::IBV_WC_RECV != 0
    }

    /// Bytes received; valid for successful receives only.
    pub fn byte_len(&self) -> u32 {
        self.0.byte_len
    }

    pub fn vendor_err(&self) -> u32 {
        self.0.vendor_err
    }

    pub fn qp_num(&self) -> u32 {
        self.0.qp_num
    }

    pub fn status_name(&self) -> &'static str {
        wc_status_name(self.0.status)
    }
}

impl WorkCompletion {
    /// The seam's form. The opcode is read only on success, where verbs
    /// defines it.
    pub fn to_completion(&self) -> Completion {
        let result = match self.0.status {
            ffi::IBV_WC_SUCCESS => Ok(if self.is_recv() {
                Completed::Recv {
                    byte_len: self.0.byte_len,
                }
            } else if self.0.opcode == ffi::IBV_WC_RDMA_WRITE {
                Completed::Write
            } else {
                Completed::Send
            }),
            ffi::IBV_WC_WR_FLUSH_ERR => Err(CompletionError::Flushed),
            status => Err(CompletionError::Failed {
                status,
                vendor_err: self.0.vendor_err,
            }),
        };
        Completion {
            wr_id: self.0.wr_id,
            result,
        }
    }
}

impl fmt::Debug for WorkCompletion {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("WorkCompletion")
            .field("wr_id", &self.0.wr_id)
            .field("status", &self.status_name())
            .field("opcode", &self.0.opcode)
            .field("vendor_err", &format_args!("0x{:x}", self.0.vendor_err))
            .field("byte_len", &self.0.byte_len)
            .field("qp_num", &self.0.qp_num)
            .finish()
    }
}

// --- connection manager ------------------------------------------------------

/// An `rdma_cm` event channel. Shared by `Arc` with every id created on it,
/// because the channel may only be destroyed once all its ids are.
pub struct EventChannel {
    ch: *mut ffi::rdma_event_channel,
}

// SAFETY: librdmacm serialises id creation and event retrieval on a channel
// internally; Rust only passes the pointer back to those calls.
unsafe impl Send for EventChannel {}
// SAFETY: as for `Send`.
unsafe impl Sync for EventChannel {}

impl EventChannel {
    pub fn new() -> Result<Arc<Self>, RdmaError> {
        // SAFETY: no preconditions.
        let ch = unsafe { ffi::rdma_create_event_channel() };
        if ch.is_null() {
            return Err(last_error("rdma_create_event_channel"));
        }
        Ok(Arc::new(EventChannel { ch }))
    }

    /// Make [`try_event`](Self::try_event) non-blocking. Once set,
    /// [`get_event`](Self::get_event) no longer waits either.
    pub fn set_nonblocking(&self) -> Result<(), RdmaError> {
        // SAFETY: `ch` is live.
        let ret = unsafe { ffi::certus_te_cm_channel_nonblocking(self.ch) };
        if ret != 0 {
            return Err(RdmaError::Verb {
                op: "fcntl(O_NONBLOCK)",
                errno: ret,
            });
        }
        Ok(())
    }

    /// The next CM event if one is pending, without waiting; the channel must
    /// be [non-blocking](Self::set_nonblocking).
    pub fn try_event(self: &Arc<Self>) -> Result<Option<CmEvent<'_>>, RdmaError> {
        match self.get_event() {
            Ok(ev) => Ok(Some(ev)),
            Err(RdmaError::Verb { errno, .. }) if errno == EAGAIN => Ok(None),
            Err(e) => Err(e),
        }
    }

    /// Block for the next CM event. The returned guard acks it on drop.
    ///
    /// Hold the guard only briefly: `rdma_destroy_id` waits until every event
    /// for that id has been acked, so dropping a [`CmId`] while holding an
    /// event for it deadlocks.
    pub fn get_event(self: &Arc<Self>) -> Result<CmEvent<'_>, RdmaError> {
        let mut ev: *mut ffi::rdma_cm_event = ptr::null_mut();
        // SAFETY: `ch` is live; `ev` is an out-pointer.
        if unsafe { ffi::rdma_get_cm_event(self.ch, &mut ev) } != 0 {
            return Err(last_error("rdma_get_cm_event"));
        }
        Ok(CmEvent { ev, channel: self })
    }
}

impl Drop for EventChannel {
    fn drop(&mut self) {
        // SAFETY: every id holds an `Arc<Self>` and every event borrows it, so
        // none remains; released once, here.
        unsafe { ffi::rdma_destroy_event_channel(self.ch) };
    }
}

/// An opaque identity for a [`CmId`], for matching events to the id they are
/// about. Never dereferenced.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct CmIdToken(usize);

/// A CM event, acked exactly once: on drop, or when consumed.
pub struct CmEvent<'a> {
    ev: *mut ffi::rdma_cm_event,
    channel: &'a Arc<EventChannel>,
}

impl CmEvent<'_> {
    /// The `RDMA_CM_EVENT_*` kind.
    pub fn kind(&self) -> c_int {
        // SAFETY: `ev` is live until acked, which only `drop` does.
        unsafe { (*self.ev).event }
    }

    /// The event's status; nonzero (a negative errno, or a reject reason) on
    /// the error kinds.
    pub fn status(&self) -> c_int {
        // SAFETY: as for `kind`.
        unsafe { (*self.ev).status }
    }

    /// The id this event is about.
    pub fn id_token(&self) -> CmIdToken {
        // SAFETY: as for `kind`.
        CmIdToken(unsafe { (*self.ev).id } as usize)
    }

    /// The connect `private_data` carried by this event, copied. librdmacm may
    /// zero-pad it past what the peer sent, so the content must be
    /// self-delimiting (D23's tie-break needs this).
    pub fn private_data(&self) -> Vec<u8> {
        // SAFETY: as for `kind`; for the connection-oriented kinds `param.conn`
        // is the active union member, and its pointer, if non-null, covers
        // `private_data_len` bytes owned by the event.
        unsafe {
            let conn = &(*self.ev).param.conn;
            if conn.private_data.is_null() {
                Vec::new()
            } else {
                std::slice::from_raw_parts(
                    conn.private_data as *const u8,
                    conn.private_data_len as usize,
                )
                .to_vec()
            }
        }
    }

    /// For `CONNECT_REQUEST`: take ownership of the new inbound id, along with
    /// the connect `private_data`, acking the event. The id must then be
    /// accepted or rejected. Any other kind is handed back unconsumed.
    pub fn into_connect_request(self) -> Result<(CmId, Vec<u8>), Self> {
        if self.kind() != ffi::RDMA_CM_EVENT_CONNECT_REQUEST {
            return Err(self);
        }
        let private_data = self.private_data();
        // SAFETY: on CONNECT_REQUEST, `ev.id` is a new id on this channel that
        // the application owns and must destroy; it stays valid after the ack.
        let id = unsafe { (*self.ev).id };
        let id = CmId {
            id,
            channel: Arc::clone(self.channel),
            qp: None,
        };
        drop(self); // ack before accepting, as librdmacm requires
        Ok((id, private_data))
    }
}

impl Drop for CmEvent<'_> {
    fn drop(&mut self) {
        // SAFETY: `ev` came from `rdma_get_cm_event` and is acked once, here.
        // The only failure is EINVAL on null, which `get_event` excludes.
        unsafe { ffi::rdma_ack_cm_event(self.ev) };
    }
}

/// Connection parameters for `rdma_connect` and `rdma_accept`.
#[derive(Debug, Clone, Copy)]
pub struct ConnParam<'a> {
    /// Carried to the peer's CONNECT_REQUEST (or ESTABLISHED, on accept).
    /// Over-length is an error, never truncated: the predecessor truncated
    /// silently, which for D23's tie-break would compare the wrong bytes.
    pub private_data: &'a [u8],
    pub responder_resources: u8,
    pub initiator_depth: u8,
    pub retry_count: u8,
    pub rnr_retry_count: u8,
}

impl ConnParam<'_> {
    fn to_ffi(self, op: &'static str) -> Result<ffi::rdma_conn_param, RdmaError> {
        let len = u8::try_from(self.private_data.len()).map_err(|_| RdmaError::Invalid {
            op,
            reason: "private_data exceeds 255 bytes",
        })?;
        Ok(ffi::rdma_conn_param {
            private_data: if len == 0 {
                ptr::null()
            } else {
                self.private_data.as_ptr().cast()
            },
            private_data_len: len,
            responder_resources: self.responder_resources,
            initiator_depth: self.initiator_depth,
            flow_control: 0,
            retry_count: self.retry_count,
            rnr_retry_count: self.rnr_retry_count,
            srq: 0,
            qp_num: 0,
        })
    }
}

fn sockaddr_v4(addr: SocketAddrV4) -> ffi::sockaddr_in {
    ffi::sockaddr_in {
        sin_family: ffi::AF_INET,
        sin_port: addr.port().to_be(),
        sin_addr: ffi::in_addr {
            s_addr: u32::from_ne_bytes(addr.ip().octets()),
        },
        sin_zero: [0; 8],
    }
}

/// An `rdma_cm` id, and the queue pair created on it, if any.
///
/// The QP lives inside the id because `rdma_destroy_qp` takes the id: it must
/// be destroyed first, and this type does that in `Drop`.
pub struct CmId {
    id: *mut ffi::rdma_cm_id,
    channel: Arc<EventChannel>,
    qp: Option<QueuePair>,
}

// SAFETY: an id and its QP are driven from one thread at a time (`&mut self`
// for every state change); moving that ownership between threads is fine.
// Deliberately not `Sync`: concurrent connect/disconnect on one id is not.
unsafe impl Send for CmId {}

impl CmId {
    /// Create a reliable-connected (`RDMA_PS_TCP`) id on `channel`.
    pub fn new(channel: &Arc<EventChannel>) -> Result<Self, RdmaError> {
        let mut id: *mut ffi::rdma_cm_id = ptr::null_mut();
        // SAFETY: `channel` is live; `id` is an out-pointer.
        let ret =
            unsafe { ffi::rdma_create_id(channel.ch, &mut id, ptr::null_mut(), ffi::RDMA_PS_TCP) };
        if ret != 0 {
            return Err(last_error("rdma_create_id"));
        }
        Ok(CmId {
            id,
            channel: Arc::clone(channel),
            qp: None,
        })
    }

    /// librdmacm's device context, null until bound or resolved.
    fn verbs(&self) -> *mut ffi::ibv_context {
        // SAFETY: `id` is live.
        unsafe { (*self.id).verbs }
    }

    pub fn token(&self) -> CmIdToken {
        CmIdToken(self.id as usize)
    }

    /// Bind to a local address; port 0 lets the OS choose (see
    /// [`local_port`](Self::local_port)). Binding to a device's address is what
    /// associates the id with that device.
    pub fn bind(&mut self, addr: SocketAddrV4) -> Result<(), RdmaError> {
        let mut sin = sockaddr_v4(addr);
        // SAFETY: `id` is live; `sin` is a valid `sockaddr_in` for the call.
        let ret =
            unsafe { ffi::rdma_bind_addr(self.id, (&mut sin as *mut ffi::sockaddr_in).cast()) };
        if ret != 0 {
            return Err(last_error("rdma_bind_addr"));
        }
        Ok(())
    }

    pub fn listen(&mut self, backlog: c_int) -> Result<(), RdmaError> {
        // SAFETY: `id` is live.
        if unsafe { ffi::rdma_listen(self.id, backlog) } != 0 {
            return Err(last_error("rdma_listen"));
        }
        Ok(())
    }

    /// The bound local port, in host order.
    pub fn local_port(&self) -> u16 {
        // SAFETY: `id` is live; the call returns network order.
        u16::from_be(unsafe { ffi::rdma_get_src_port(self.id) })
    }

    /// Start resolving `dst`; completes with `ADDR_RESOLVED` or `ADDR_ERROR`.
    pub fn resolve_addr(&mut self, dst: SocketAddrV4, timeout_ms: c_int) -> Result<(), RdmaError> {
        let mut dst = sockaddr_v4(dst);
        // SAFETY: `id` is live; null source lets rdma_cm choose; `dst` is valid
        // for the call, which copies it.
        let ret = unsafe {
            ffi::rdma_resolve_addr(
                self.id,
                ptr::null_mut(),
                (&mut dst as *mut ffi::sockaddr_in).cast(),
                timeout_ms,
            )
        };
        if ret != 0 {
            return Err(last_error("rdma_resolve_addr"));
        }
        Ok(())
    }

    /// Start resolving the route; completes with `ROUTE_RESOLVED` or
    /// `ROUTE_ERROR`.
    pub fn resolve_route(&mut self, timeout_ms: c_int) -> Result<(), RdmaError> {
        // SAFETY: `id` is live.
        if unsafe { ffi::rdma_resolve_route(self.id, timeout_ms) } != 0 {
            return Err(last_error("rdma_resolve_route"));
        }
        Ok(())
    }

    pub fn connect(&mut self, param: ConnParam<'_>) -> Result<(), RdmaError> {
        let mut p = param.to_ffi("rdma_connect")?;
        // SAFETY: `id` is live; `p` and its private data outlive this call,
        // which copies them into the request.
        if unsafe { ffi::rdma_connect(self.id, &mut p) } != 0 {
            return Err(last_error("rdma_connect"));
        }
        Ok(())
    }

    pub fn accept(&mut self, param: ConnParam<'_>) -> Result<(), RdmaError> {
        let mut p = param.to_ffi("rdma_accept")?;
        // SAFETY: as for `connect`.
        if unsafe { ffi::rdma_accept(self.id, &mut p) } != 0 {
            return Err(last_error("rdma_accept"));
        }
        Ok(())
    }

    /// Reject an inbound request, telling the peer why in `private_data` —
    /// how the simultaneous-connect loser learns it lost (D23).
    pub fn reject(&mut self, private_data: &[u8]) -> Result<(), RdmaError> {
        let len = u8::try_from(private_data.len()).map_err(|_| RdmaError::Invalid {
            op: "rdma_reject",
            reason: "private_data exceeds 255 bytes",
        })?;
        let data = if len == 0 {
            ptr::null()
        } else {
            private_data.as_ptr().cast()
        };
        // SAFETY: `id` is live; `data` covers `len` bytes for the call.
        if unsafe { ffi::rdma_reject(self.id, data, len) } != 0 {
            return Err(last_error("rdma_reject"));
        }
        Ok(())
    }

    /// Start a disconnect. This does **not** by itself stop writes already in
    /// flight from landing; for that, [`QueuePair::to_error`] (FR-023).
    pub fn disconnect(&mut self) -> Result<(), RdmaError> {
        // SAFETY: `id` is live.
        if unsafe { ffi::rdma_disconnect(self.id) } != 0 {
            return Err(last_error("rdma_disconnect"));
        }
        Ok(())
    }

    /// Create the id's RC queue pair in `pd`, completing sends into `send_cq`
    /// and receives into `recv_cq` (which may be the same CQ). Every work
    /// request chooses whether it is signaled (`sq_sig_all = 0`).
    ///
    /// The id must be on `pd`'s device (checked here, so the error says so).
    /// rdma_cm then moves the QP through its states; once created it can
    /// accept receives, before the connection completes.
    pub fn create_qp(
        &mut self,
        pd: &Arc<ProtectionDomain>,
        send_cq: &Arc<CompletionQueue>,
        recv_cq: &Arc<CompletionQueue>,
        caps: QpCaps,
    ) -> Result<&QueuePair, RdmaError> {
        if self.qp.is_some() {
            return Err(RdmaError::Invalid {
                op: "rdma_create_qp",
                reason: "id already has a queue pair",
            });
        }
        if !pd.same_device(self) {
            return Err(RdmaError::Invalid {
                op: "rdma_create_qp",
                reason: "protection domain is on another device than the id",
            });
        }
        let mut attr = ffi::ibv_qp_init_attr {
            qp_context: ptr::null_mut(),
            send_cq: send_cq.cq,
            recv_cq: recv_cq.cq,
            srq: ptr::null_mut(),
            cap: ffi::ibv_qp_cap {
                max_send_wr: caps.max_send_wr,
                max_recv_wr: caps.max_recv_wr,
                max_send_sge: 1,
                max_recv_sge: 1,
                max_inline_data: 0,
            },
            qp_type: ffi::IBV_QPT_RC,
            sq_sig_all: 0,
        };
        // SAFETY: `id`, `pd` and both CQs are live and on one device; `attr` is
        // valid for the call, which writes the granted capacities back into it.
        if unsafe { ffi::rdma_create_qp(self.id, pd.pd, &mut attr) } != 0 {
            return Err(last_error("rdma_create_qp"));
        }
        // SAFETY: `rdma_create_qp` succeeded, so `id.qp` is the new QP.
        let qp = unsafe { (*self.id).qp };
        Ok(self.qp.insert(QueuePair {
            id: self.id,
            qp,
            granted: QpCaps {
                max_send_wr: attr.cap.max_send_wr,
                max_recv_wr: attr.cap.max_recv_wr,
            },
            _pd: Arc::clone(pd),
            _send_cq: Arc::clone(send_cq),
            _recv_cq: Arc::clone(recv_cq),
        }))
    }

    pub fn qp(&self) -> Option<&QueuePair> {
        self.qp.as_ref()
    }

    /// Whether this id is bound or resolved to a device yet.
    pub fn has_device(&self) -> bool {
        !self.verbs().is_null()
    }
}

impl Drop for CmId {
    fn drop(&mut self) {
        // QP first: `rdma_destroy_qp` takes the id. Its `Drop` releases it.
        self.qp = None;
        // SAFETY: `id` came from `rdma_create_id` or a CONNECT_REQUEST and is
        // released once, here. Destroying a connected id also disconnects it.
        released("rdma_destroy_id", unsafe { ffi::rdma_destroy_id(self.id) });
    }
}

// --- queue pairs -------------------------------------------------------------

fn qp_state(s: c_int) -> QpState {
    {
        match s {
            ffi::IBV_QPS_RESET => QpState::Reset,
            ffi::IBV_QPS_INIT => QpState::Init,
            ffi::IBV_QPS_RTR => QpState::Rtr,
            ffi::IBV_QPS_RTS => QpState::Rts,
            ffi::IBV_QPS_SQD => QpState::Sqd,
            ffi::IBV_QPS_SQE => QpState::Sqe,
            ffi::IBV_QPS_ERR => QpState::Err,
            other => QpState::Unknown(other),
        }
    }
}

/// An RC queue pair, owned by its [`CmId`].
pub struct QueuePair {
    id: *mut ffi::rdma_cm_id,
    qp: *mut ffi::ibv_qp,
    granted: QpCaps,
    _pd: Arc<ProtectionDomain>,
    _send_cq: Arc<CompletionQueue>,
    _recv_cq: Arc<CompletionQueue>,
}

impl QueuePair {
    pub fn qp_num(&self) -> u32 {
        // SAFETY: `qp` is live while its id is; `qp_num` is fixed at creation.
        unsafe { (*self.qp).qp_num }
    }

    /// Depths the device actually granted, which may exceed those requested.
    /// Credits (T014) are sized from these.
    pub fn granted(&self) -> QpCaps {
        self.granted
    }

    /// The QP's state as the **device** reports it. Not `ibv_qp::state`, which
    /// the initiator's `is_qp_healthy` reads: that is libibverbs' cache of the
    /// last state software set, and does not see the device forcing ERROR.
    pub fn state(&self) -> Result<QpState, RdmaError> {
        // SAFETY: `qp` is live.
        let ret = unsafe { ffi::certus_te_query_qp_state(self.qp) };
        if ret < 0 {
            return Err(RdmaError::Verb {
                op: "ibv_query_qp",
                errno: -ret,
            });
        }
        Ok(qp_state(ret))
    }

    /// Force the QP to ERROR. Afterwards no further write it carries can land
    /// in local memory, and every queued work request completes with
    /// `WR_FLUSH_ERR`. FR-023's order is: this, drain the CQ until every
    /// outstanding `wr_id` is accounted for, *then* reclaim.
    pub fn to_error(&self) -> Result<(), RdmaError> {
        // SAFETY: `qp` is live; the transition is legal from any state.
        let ret = unsafe { ffi::certus_te_qp_to_error(self.qp) };
        if ret != 0 {
            return Err(RdmaError::Verb {
                op: "ibv_modify_qp(ERR)",
                errno: ret,
            });
        }
        Ok(())
    }

    /// Post `reqs` as one chained list. On a partial failure, the error names
    /// how many were accepted; those will complete, the rest were never queued.
    ///
    /// # Safety
    ///
    /// Every local buffer must stay valid, and its region registered, until the
    /// request's completion is reaped or the QP is flushed and drained. For a
    /// `Send` the NIC reads the buffer asynchronously, so it must not be
    /// modified in that window either. Bounds and keys were checked when each
    /// [`RawSge`] was built; this contract is only about *time*, which Rust
    /// cannot see.
    pub unsafe fn post_send(&self, reqs: &[VerbsSend]) -> Result<(), PostError> {
        if reqs.is_empty() {
            return Ok(());
        }
        let mut sges: Vec<ffi::ibv_sge> = reqs.iter().map(|r| r.local.to_ffi()).collect();
        let mut wrs: Vec<ffi::ibv_send_wr> = Vec::with_capacity(reqs.len());
        for (r, sge) in reqs.iter().zip(sges.iter_mut()) {
            // SAFETY: `ibv_send_wr` is integers, raw pointers and unions of
            // them; all-zero is valid and is what the unused fields must be.
            let mut wr: ffi::ibv_send_wr = std::mem::zeroed();
            wr.wr_id = r.wr_id;
            wr.sg_list = sge;
            wr.num_sge = 1;
            wr.send_flags = if r.signaled {
                ffi::IBV_SEND_SIGNALED
            } else {
                0
            };
            match r.op {
                SendOp::Send => wr.opcode = ffi::IBV_WR_SEND,
                SendOp::RdmaWrite { remote_addr, rkey } => {
                    wr.opcode = ffi::IBV_WR_RDMA_WRITE;
                    wr.wr.rdma = ffi::ibv_send_wr_rdma { remote_addr, rkey };
                }
            }
            wrs.push(wr);
        }
        let base = wrs.as_mut_ptr();
        for i in 1..wrs.len() {
            wrs[i - 1].next = base.add(i);
        }
        let mut bad: *mut ffi::ibv_send_wr = ptr::null_mut();
        // SAFETY: `qp` is live; `wrs`/`sges` are a well-formed chain that
        // outlives the call (the device copies descriptors at post time); the
        // buffers they name are covered by this function's contract.
        let ret = ffi::certus_te_post_send(self.qp, base, &mut bad);
        post_result("ibv_post_send", ret, base, bad, wrs.len())
    }

    /// Post receive buffers as one chained list; partial failure as for
    /// [`post_send`](Self::post_send).
    ///
    /// # Safety
    ///
    /// Every buffer must stay valid, registered, and unread by anyone else
    /// until its completion is reaped or the QP is flushed and drained: the
    /// NIC writes it at an unknown time in that window.
    pub unsafe fn post_recv(&self, reqs: &[VerbsRecv]) -> Result<(), PostError> {
        if reqs.is_empty() {
            return Ok(());
        }
        let mut sges: Vec<ffi::ibv_sge> = reqs.iter().map(|r| r.buf.to_ffi()).collect();
        let mut wrs: Vec<ffi::ibv_recv_wr> = reqs
            .iter()
            .zip(sges.iter_mut())
            .map(|(r, sge)| ffi::ibv_recv_wr {
                wr_id: r.wr_id,
                next: ptr::null_mut(),
                sg_list: sge,
                num_sge: 1,
            })
            .collect();
        let base = wrs.as_mut_ptr();
        for i in 1..wrs.len() {
            wrs[i - 1].next = base.add(i);
        }
        let mut bad: *mut ffi::ibv_recv_wr = ptr::null_mut();
        // SAFETY: as for `post_send`.
        let ret = ffi::certus_te_post_recv(self.qp, base, &mut bad);
        post_result("ibv_post_recv", ret, base, bad, wrs.len())
    }
}

impl Drop for QueuePair {
    fn drop(&mut self) {
        // SAFETY: the QP belongs to `id`, which outlives this (see `CmId`'s
        // `Drop`); destroyed once, here. Returns nothing to check.
        unsafe { ffi::rdma_destroy_qp(self.id) };
    }
}

/// Map a post's return and `bad_wr` to how many requests were accepted.
fn post_result<T>(
    op: &'static str,
    ret: c_int,
    base: *mut T,
    bad: *mut T,
    n: usize,
) -> Result<(), PostError> {
    if ret == 0 {
        return Ok(());
    }
    // `bad` points into the chain at the first request not queued; a provider
    // that leaves it null or outside the chain gets the conservative answer.
    let accepted = (bad as usize)
        .checked_sub(base as usize)
        .map(|b| b / std::mem::size_of::<T>())
        .filter(|&i| i < n)
        .unwrap_or(0);
    Err(PostError {
        accepted,
        error: RdmaError::Verb { op, errno: ret },
    })
}

/// One send-side work request, as verbs posts it. The seam's `SendRequest`
/// maps onto this in the verbs transport (T053).
#[derive(Debug, Clone, Copy)]
pub struct VerbsSend {
    /// Echoed in the completion; the `wr_id` slab (T013) allocates it.
    pub wr_id: u64,
    pub op: SendOp,
    pub local: RawSge,
    /// Whether to generate a completion on success. An unsignaled request
    /// still completes on error, and still occupies the send queue until a
    /// later signaled one completes.
    pub signaled: bool,
}

/// One receive buffer, as verbs posts it.
#[derive(Debug, Clone, Copy)]
pub struct VerbsRecv {
    pub wr_id: u64,
    pub buf: RawSge,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn private_data_over_255_rejected_not_truncated() {
        let data = [0u8; 256];
        let p = ConnParam {
            private_data: &data,
            responder_resources: 1,
            initiator_depth: 1,
            retry_count: 7,
            rnr_retry_count: 7,
        };
        assert!(matches!(p.to_ffi("t"), Err(RdmaError::Invalid { .. })));
        let p = ConnParam {
            private_data: &data[..255],
            ..p
        };
        assert_eq!(p.to_ffi("t").unwrap().private_data_len, 255);
    }

    #[test]
    fn accepted_count_from_bad_wr() {
        let mut v = [0u64; 4];
        let base = v.as_mut_ptr();
        // SAFETY: in bounds of `v`.
        let bad = unsafe { base.add(2) };
        let e = post_result("t", 12, base, bad, 4).unwrap_err();
        assert_eq!(e.accepted, 2);
        assert_eq!(
            post_result("t", 12, base, ptr::null_mut(), 4)
                .unwrap_err()
                .accepted,
            0
        );
        assert!(post_result("t", 0, base, ptr::null_mut(), 4).is_ok());
    }

    // --- hardware: need an RDMA device bound to CERTUS_RDMA_TEST_IP ----------

    fn test_ip() -> SocketAddrV4 {
        let ip = std::env::var("CERTUS_RDMA_TEST_IP")
            .expect("set CERTUS_RDMA_TEST_IP to a local RDMA (RoCE) IPv4 address");
        SocketAddrV4::new(ip.parse().expect("CERTUS_RDMA_TEST_IP is not IPv4"), 0)
    }

    /// A bound id plus PD and CQ on its device.
    fn device() -> (
        Arc<EventChannel>,
        CmId,
        Arc<ProtectionDomain>,
        Arc<CompletionQueue>,
    ) {
        let ch = EventChannel::new().unwrap();
        let mut id = CmId::new(&ch).unwrap();
        id.bind(test_ip()).unwrap();
        assert_ne!(id.local_port(), 0);
        let pd = ProtectionDomain::new(&id).unwrap();
        let cq = CompletionQueue::new(&pd, 16).unwrap();
        (ch, id, pd, cq)
    }

    const CAPS: QpCaps = QpCaps {
        max_send_wr: 8,
        max_recv_wr: 8,
    };

    #[test]
    #[ignore = "needs an RDMA device; set CERTUS_RDMA_TEST_IP"]
    fn region_outlives_dropped_pd_handle() {
        let (_ch, id, pd, cq) = device();
        let mut buf = vec![0u8; 4096];
        // SAFETY: `buf` outlives `mr`, and nothing posts against it.
        let mr = unsafe {
            MemoryRegion::register(&pd, buf.as_mut_ptr(), buf.len(), Access::LOCAL_WRITE)
        }
        .unwrap();
        assert!(pd.same_device(&id));
        drop(cq);
        drop(pd); // the region and nothing else keeps the PD alive now
        assert_eq!(mr.addr(), buf.as_ptr() as u64);
        assert!(mr.local(mr.addr(), 4096).is_ok());
        assert!(mr.local(mr.addr() + 1, 4096).is_err());
        drop(mr); // deregisters, then deallocates the PD; panics if either fails
    }

    #[test]
    #[ignore = "needs an RDMA device; set CERTUS_RDMA_TEST_IP"]
    fn error_transition_flushes_posted_receives() {
        let (_ch, mut id, pd, cq) = device();
        let mut buf = vec![0u8; 4096];
        // SAFETY: `buf` outlives `mr` and the QP, which are dropped first.
        let mr = unsafe {
            MemoryRegion::register(&pd, buf.as_mut_ptr(), buf.len(), Access::LOCAL_WRITE)
        }
        .unwrap();
        let qp = id.create_qp(&pd, &cq, &cq, CAPS).unwrap();
        assert!(qp.granted().max_recv_wr >= CAPS.max_recv_wr);
        // Unconnected: rdma_cm leaves an RC QP in INIT, ready for receives.
        assert_eq!(qp.state().unwrap(), QpState::Init);

        let reqs: Vec<_> = (0..3u64)
            .map(|i| VerbsRecv {
                wr_id: 100 + i,
                buf: mr.local(mr.addr() + i * 1024, 1024).unwrap().raw(),
            })
            .collect();
        // SAFETY: `buf`/`mr` outlive the drain below.
        unsafe { qp.post_recv(&reqs) }.unwrap();

        qp.to_error().unwrap();
        assert_eq!(qp.state().unwrap(), QpState::Err);

        let mut wcs: Vec<WorkCompletion> = (0..8).map(|_| WorkCompletion::default()).collect();
        let mut seen = Vec::new();
        for _ in 0..1000 {
            let n = cq.poll(&mut wcs).unwrap();
            seen.extend(wcs[..n].iter().map(|wc| {
                assert!(wc.is_flushed(), "{wc:?}");
                assert_eq!(wc.to_completion().result, Err(CompletionError::Flushed));
                wc.wr_id()
            }));
            if seen.len() == 3 {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(1));
        }
        seen.sort_unstable();
        assert_eq!(seen, [100, 101, 102]);
    }

    #[test]
    #[ignore = "needs an RDMA device; set CERTUS_RDMA_TEST_IP"]
    fn send_on_unconnected_qp_reports_accepted_count() {
        let (_ch, mut id, pd, cq) = device();
        let mut buf = vec![0u8; 4096];
        // SAFETY: `buf` outlives `mr` and the QP.
        let mr = unsafe {
            MemoryRegion::register(&pd, buf.as_mut_ptr(), buf.len(), Access::LOCAL_WRITE)
        }
        .unwrap();
        let qp = id.create_qp(&pd, &cq, &cq, CAPS).unwrap();
        let local = mr.local(mr.addr(), 64).unwrap().raw();
        let reqs: Vec<_> = (0..3u64)
            .map(|i| VerbsSend {
                wr_id: i,
                op: SendOp::Send,
                local,
                signaled: true,
            })
            .collect();
        // A QP in INIT cannot send; the first request is refused, none queued.
        // SAFETY: nothing is queued, so nothing outlives the call.
        let err = unsafe { qp.post_send(&reqs) }.unwrap_err();
        assert_eq!(err.accepted, 0, "{err}");
    }

    #[test]
    #[ignore = "needs an RDMA device; set CERTUS_RDMA_TEST_IP"]
    fn second_queue_pair_on_one_id_rejected() {
        let (_ch, mut id, pd, cq) = device();
        id.create_qp(&pd, &cq, &cq, CAPS).unwrap();
        assert!(matches!(
            id.create_qp(&pd, &cq, &cq, CAPS),
            Err(RdmaError::Invalid { .. })
        ));
    }
}
