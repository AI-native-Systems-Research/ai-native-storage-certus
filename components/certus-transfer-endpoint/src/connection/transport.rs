//! The transport seam: what the engine needs from a fabric, shaped like verbs.
//!
//! The seam is deliberately **verbs-shaped** — work requests with `wr_id`s,
//! posted receive buffers, completions polled per link — rather than "send a
//! message" / "write these bytes". The higher-level shape would hide exactly
//! the behaviour Principle VIII requires to be testable without a NIC: the
//! `wr_id` slab (T013), send-queue credits (T014), receive-buffer partitioning
//! and its RNR stall (FR-026), and the flush-on-ERROR drain (FR-023).
//!
//! Two implementations sit behind it: the in-process mock ([`super::mock`]),
//! compiled in **every** build because the default build serves over it
//! (research D14), and the verbs transport over `crate::rdma` (T053).
//! [`select_transport`] picks one; nothing returns "not initialized" merely
//! because `rdma` is off.
//!
//! # Threading
//!
//! One endpoint thread owns the [`Transport`] and every [`Link`], so every
//! method is non-blocking and takes `&mut self`. Each link has its own
//! completion queue, sized to its send plus receive depth, which is what makes
//! "drain this connection until every `wr_id` is accounted for" (FR-023)
//! self-contained and makes a CQ overrun impossible by construction.
//!
//! # Work-request ids
//!
//! A link's sends and receives complete into one queue, and a failed completion
//! does not say which queue it came from. So `wr_id`s must be unique across
//! *both* queues of a link; the slab (T013) allocates them that way.

// Consumed from T012 onward.
#![allow(dead_code)]

use std::any::Any;
use std::fmt;
use std::ops::BitOr;
use std::sync::atomic::{AtomicU64, Ordering};

use super::mock::{MockFabric, MockTransport};
use super::PeerId;

// --- errors ------------------------------------------------------------------

/// A failure in the transport, real or mock.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RdmaError {
    /// A verbs or rdma_cm call failed (or, in the mock, the call that would
    /// have). `errno` is the real cause: the `rdma_*` calls return -1 and set
    /// `errno`, the `ibv_post_*` calls return it, and the allocating calls
    /// return null and set it.
    Verb { op: &'static str, errno: i32 },
    /// A local buffer does not lie inside the memory region it was cut from.
    OutOfRegion {
        addr: u64,
        len: usize,
        region_addr: u64,
        region_len: usize,
    },
    /// An argument was rejected before any call was made.
    Invalid {
        op: &'static str,
        reason: &'static str,
    },
}

impl fmt::Display for RdmaError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            RdmaError::Verb { op, errno } => write!(
                f,
                "{op} failed: {}",
                std::io::Error::from_raw_os_error(*errno)
            ),
            RdmaError::OutOfRegion {
                addr,
                len,
                region_addr,
                region_len,
            } => write!(
                f,
                "buffer 0x{addr:x}+{len} is outside region 0x{region_addr:x}+{region_len}"
            ),
            RdmaError::Invalid { op, reason } => write!(f, "{op}: {reason}"),
        }
    }
}

impl std::error::Error for RdmaError {}

/// A batch post that stopped partway. Work requests before index `accepted`
/// were queued and will complete; the one at `accepted` and everything after
/// it were not.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PostError {
    pub accepted: usize,
    pub error: RdmaError,
}

impl fmt::Display for PostError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{} after {} accepted", self.error, self.accepted)
    }
}

impl std::error::Error for PostError {}

/// errno values the mock reports, matching what verbs returns for the same
/// condition.
pub(crate) mod errno {
    pub const ENOMEM: i32 = 12;
    pub const EINVAL: i32 = 22;
    pub const EADDRINUSE: i32 = 98;
}

// --- registration ------------------------------------------------------------

/// `ibv_reg_mr` access flags. Values are the verbs ones; `rdma.rs` asserts so.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Access(pub(crate) i32);

impl Access {
    /// Required for anything the NIC writes locally (receives), and by verbs
    /// alongside `REMOTE_WRITE`.
    pub const LOCAL_WRITE: Access = Access(1);
    /// A peer may RDMA-write here: landing regions.
    pub const REMOTE_WRITE: Access = Access(2);
    /// A peer may RDMA-read here. Nothing in this design reads remotely.
    pub const REMOTE_READ: Access = Access(4);

    pub fn contains(self, other: Access) -> bool {
        self.0 & other.0 == other.0
    }
}

impl BitOr for Access {
    type Output = Access;
    fn bitor(self, rhs: Access) -> Access {
        Access(self.0 | rhs.0)
    }
}

/// Identity of one [`Transport`] instance, so a buffer registered with one
/// cannot be posted on another's link.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct TransportId(u64);

impl TransportId {
    pub(crate) fn next() -> Self {
        static NEXT: AtomicU64 = AtomicU64::new(1);
        TransportId(NEXT.fetch_add(1, Ordering::Relaxed))
    }
}

/// A registration of caller-owned memory with a transport. Dropping it
/// deregisters; the registration itself (a verbs `MemoryRegion`, or the
/// mock's entry) lives inside and is reached only by its own transport.
pub struct Region {
    addr: u64,
    len: usize,
    lkey: u32,
    rkey: u32,
    owner: TransportId,
    registration: Box<dyn Any + Send + Sync>,
}

impl Region {
    pub(crate) fn new(
        addr: u64,
        len: usize,
        lkey: u32,
        rkey: u32,
        owner: TransportId,
        registration: Box<dyn Any + Send + Sync>,
    ) -> Self {
        Region {
            addr,
            len,
            lkey,
            rkey,
            owner,
            registration,
        }
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

    pub fn owner(&self) -> TransportId {
        self.owner
    }

    /// The transport-specific registration, for the transport that made it.
    pub(crate) fn registration<T: 'static>(&self) -> Option<&T> {
        self.registration.downcast_ref()
    }

    /// A bounds-checked buffer `[addr, addr + len)` inside this region, for the
    /// local side of a work request. Addresses are absolute, as on the wire.
    /// The check lives here, once, for both transports.
    pub fn sge(&self, addr: u64, len: usize) -> Result<Sge<'_>, RdmaError> {
        check_range(self.addr, self.len, addr, len)?;
        Ok(Sge {
            region: self,
            addr,
            len: len as u32,
        })
    }
}

/// A buffer inside a [`Region`], checked at construction. It borrows the
/// region, so the region — and therefore its `lkey` — is live at post time,
/// and a buffer cannot be paired with another region's key.
#[derive(Clone, Copy)]
pub struct Sge<'a> {
    region: &'a Region,
    addr: u64,
    len: u32,
}

impl<'a> Sge<'a> {
    pub fn region(&self) -> &'a Region {
        self.region
    }

    pub fn addr(&self) -> u64 {
        self.addr
    }

    pub fn len(&self) -> u32 {
        self.len
    }

    pub fn lkey(&self) -> u32 {
        self.region.lkey
    }
}

/// `[addr, addr + len)` lies within `[region_addr, region_addr + region_len)`
/// and `len` fits a scatter/gather entry. Overflow-safe.
pub(crate) fn check_range(
    region_addr: u64,
    region_len: usize,
    addr: u64,
    len: usize,
) -> Result<(), RdmaError> {
    let out = || RdmaError::OutOfRegion {
        addr,
        len,
        region_addr,
        region_len,
    };
    if len > u32::MAX as usize {
        return Err(out());
    }
    let offset = addr.checked_sub(region_addr).ok_or_else(out)?;
    let end = offset.checked_add(len as u64).ok_or_else(out)?;
    if end > region_len as u64 {
        return Err(out());
    }
    Ok(())
}

// --- work requests -----------------------------------------------------------

/// What a send-queue work request does.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SendOp {
    /// Two-sided: consumes one of the peer's posted receives (messages).
    Send,
    /// One-sided write into the peer's memory (values into landing slots).
    RdmaWrite { remote_addr: u64, rkey: u32 },
}

/// One send-queue work request.
#[derive(Clone, Copy)]
pub struct SendRequest<'a> {
    /// Echoed in the completion; the `wr_id` slab (T013) allocates it.
    pub wr_id: u64,
    pub op: SendOp,
    pub local: Sge<'a>,
    /// Whether to generate a completion on success. An unsignaled request
    /// still completes on error, holds its send-queue slot until a later
    /// completion on that queue is polled, and on success never appears at
    /// all — not even in a post-ERROR drain.
    pub signaled: bool,
}

/// One receive buffer.
#[derive(Clone, Copy)]
pub struct RecvRequest<'a> {
    pub wr_id: u64,
    pub buf: Sge<'a>,
}

/// Requested (or, from [`Link::granted`], granted) work-request depths.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct QpCaps {
    pub max_send_wr: u32,
    pub max_recv_wr: u32,
}

/// `enum ibv_qp_state`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum QpState {
    Reset,
    Init,
    Rtr,
    Rts,
    Sqd,
    Sqe,
    Err,
    Unknown(i32),
}

// --- completions -------------------------------------------------------------

/// One work completion.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Completion {
    pub wr_id: u64,
    pub result: Result<Completed, CompletionError>,
}

/// What a successful work request was. Known only on success: verbs leaves the
/// opcode undefined on error, so a failed completion is attributed by `wr_id`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Completed {
    Send,
    Write,
    Recv { byte_len: u32 },
}

/// Why a work request failed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CompletionError {
    /// Queued when the QP entered ERROR — what a post-deadline drain (FR-023)
    /// expects, as opposed to the one completion naming an original fault.
    Flushed,
    /// Any other `ibv_wc_status`; see [`wc`] for the ones the mock produces.
    Failed { status: i32, vendor_err: u32 },
}

impl CompletionError {
    pub fn status_name(&self) -> &'static str {
        match self {
            CompletionError::Flushed => wc_status_name(wc::WR_FLUSH_ERR),
            CompletionError::Failed { status, .. } => wc_status_name(*status),
        }
    }
}

/// `enum ibv_wc_status` values used outside `rdma.rs`. `wrapper.c` asserts
/// them against the header, since nothing in `ffi.rs` mirrors them.
pub mod wc {
    pub const SUCCESS: i32 = 0;
    pub const LOC_LEN_ERR: i32 = 1;
    pub const WR_FLUSH_ERR: i32 = 5;
    pub const REM_INV_REQ_ERR: i32 = 9;
    pub const REM_ACCESS_ERR: i32 = 10;
    pub const RETRY_EXC_ERR: i32 = 12;
    pub const RNR_RETRY_EXC_ERR: i32 = 13;
}

/// Name of an `ibv_wc_status`, so a completion error is diagnosable from the
/// log alone. `RETRY_EXC_ERR` and `RNR_RETRY_EXC_ERR` are the two that usually
/// mean a peer problem rather than a local one.
pub(crate) fn wc_status_name(status: i32) -> &'static str {
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
        22 => "TM_ERR",
        23 => "TM_RNDV_INCOMPLETE",
        _ => "UNKNOWN",
    }
}

// --- connection events -------------------------------------------------------

/// A link, named by the transport that owns it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct LinkId(pub(crate) u64);

/// An inbound connect awaiting [`Transport::accept`] or [`Transport::reject`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct RequestId(pub(crate) u64);

/// Connect `private_data` limits for the reliable-connected port space, from
/// `rdma_connect(3)` and `rdma_accept(3)`; a reject carries the IB CM REJ
/// payload. Received private data is **zero-padded** to these sizes, so its
/// content must be self-delimiting (D23).
pub const MAX_CONNECT_PRIVATE_DATA: usize = 56;
pub const MAX_ACCEPT_PRIVATE_DATA: usize = 196;
pub const MAX_REJECT_PRIVATE_DATA: usize = 148;

/// Connection-manager events, delivered by [`Transport::poll_events`].
///
/// Accepting or rejecting a [`ConnectRequest`](TransportEvent::ConnectRequest)
/// is the caller's decision, because D23's simultaneous-connect tie-break is
/// made at accept time from the request's `private_data`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TransportEvent {
    /// A peer wants to connect. Its identity is whatever its `private_data`
    /// says: the source address carries an ephemeral port, not an instance.
    ConnectRequest {
        request: RequestId,
        private_data: Vec<u8>,
    },
    /// The link is connected and both ends may post. On the initiating side
    /// `private_data` is the acceptor's; on the accepting side it is empty.
    Established { link: LinkId, private_data: Vec<u8> },
    /// The peer rejected our connect, saying why. The link's QP is in ERROR.
    Rejected { link: LinkId, private_data: Vec<u8> },
    /// The connect could not be completed (no listener, unreachable, route).
    ConnectError { link: LinkId, reason: String },
    /// Either end disconnected. Queued work on *this* end is not flushed by
    /// the event: call [`Link::to_error`] or [`Link::disconnect`].
    Disconnected { link: LinkId },
}

// --- the seam ----------------------------------------------------------------

/// One peer link: an RC queue pair and its completion queue.
pub trait Link {
    /// The QP's state as the device reports it (never a cached value).
    fn state(&self) -> Result<QpState, RdmaError>;

    /// Depths actually granted; credits (T014) are sized from these.
    fn granted(&self) -> QpCaps;

    /// Force ERROR: afterwards nothing this link carries can land in local
    /// memory, and every queued request completes [`Flushed`]. FR-023's order
    /// is this, drain until every `wr_id` is accounted for, *then* reclaim.
    ///
    /// [`Flushed`]: CompletionError::Flushed
    fn to_error(&mut self) -> Result<(), RdmaError>;

    /// Disconnect. On InfiniBand/RoCE librdmacm also forces ERROR, but rely on
    /// [`to_error`](Self::to_error) for FR-023, not on this.
    fn disconnect(&mut self) -> Result<(), RdmaError>;

    /// Post `reqs` as one chained list; a partial failure names how many were
    /// accepted.
    ///
    /// # Safety
    ///
    /// Every local buffer must stay valid, and its region registered, until the
    /// request's completion is reaped or the link is forced to ERROR and
    /// drained; for a `Send` it must also not be modified in that window.
    /// Bounds and keys are already checked by [`Region::sge`]; this contract is
    /// only about *time*.
    unsafe fn post_send(&mut self, reqs: &[SendRequest<'_>]) -> Result<(), PostError>;

    /// Post receive buffers as one chained list.
    ///
    /// # Safety
    ///
    /// Every buffer must stay valid, registered, and untouched by anyone else
    /// until its completion is reaped or the link is forced to ERROR and
    /// drained: the peer's send lands in it at an unknown time in that window.
    unsafe fn post_recv(&mut self, reqs: &[RecvRequest<'_>]) -> Result<(), PostError>;

    /// Move up to `max` ready completions onto `out` without blocking;
    /// returns how many. A failed work request is a [`Completion`], not `Err`.
    fn poll(&mut self, out: &mut Vec<Completion>, max: usize) -> Result<usize, RdmaError>;
}

/// The fabric as one endpoint sees it.
pub trait Transport: Send {
    fn id(&self) -> TransportId;

    /// Accept inbound connects addressed to `local`, this endpoint's instance.
    fn listen(&mut self, local: &PeerId) -> Result<(), RdmaError>;

    /// Register `[addr, addr + len)`. `REMOTE_WRITE` requires `LOCAL_WRITE`,
    /// as verbs does.
    ///
    /// # Safety
    ///
    /// The memory must stay allocated until the returned [`Region`] is
    /// dropped, and the caller must not hold Rust references over any part a
    /// peer or a posted receive may write while that can happen.
    unsafe fn register(
        &mut self,
        addr: *mut u8,
        len: usize,
        access: Access,
    ) -> Result<Region, RdmaError>;

    /// Start connecting to `peer`; the outcome arrives as an event naming the
    /// returned link. `private_data` is at most
    /// [`MAX_CONNECT_PRIVATE_DATA`] bytes.
    fn connect(
        &mut self,
        peer: &PeerId,
        private_data: &[u8],
        caps: QpCaps,
    ) -> Result<LinkId, RdmaError>;

    /// Accept an inbound request, returning its link.
    fn accept(
        &mut self,
        request: RequestId,
        private_data: &[u8],
        caps: QpCaps,
    ) -> Result<LinkId, RdmaError>;

    /// Refuse an inbound request, telling the peer why (D23's loser).
    fn reject(&mut self, request: RequestId, private_data: &[u8]) -> Result<(), RdmaError>;

    /// Move every pending connection event onto `out`, without blocking.
    fn poll_events(&mut self, out: &mut Vec<TransportEvent>);

    fn link(&mut self, link: LinkId) -> Option<&mut dyn Link>;

    /// Destroy a link. A connected peer sees
    /// [`Disconnected`](TransportEvent::Disconnected).
    fn close(&mut self, link: LinkId);
}

// --- selection ---------------------------------------------------------------

/// Which transport an endpoint runs over.
pub enum TransportChoice {
    /// The build's own: verbs with `rdma`, the process-wide mock fabric
    /// without it (D14).
    BuildDefault,
    /// An explicit mock fabric, in any build — so tests can isolate
    /// themselves and parallel tests cannot collide on a [`PeerId`].
    Mock(MockFabric),
}

/// Construct the chosen transport.
pub fn select_transport(choice: TransportChoice) -> Result<Box<dyn Transport>, RdmaError> {
    match choice {
        TransportChoice::Mock(fabric) => Ok(Box::new(MockTransport::new(fabric))),
        TransportChoice::BuildDefault => build_default(),
    }
}

#[cfg(not(feature = "rdma"))]
fn build_default() -> Result<Box<dyn Transport>, RdmaError> {
    Ok(Box::new(MockTransport::new(MockFabric::process())))
}

/// Until T053 an `rdma` build has no default transport. That is reported, not
/// papered over with the mock: an `rdma` build silently running in-process
/// would yield plausible numbers from no fabric at all (Principle XI).
#[cfg(feature = "rdma")]
fn build_default() -> Result<Box<dyn Transport>, RdmaError> {
    Err(RdmaError::Invalid {
        op: "select_transport",
        reason: "the verbs transport lands in T053; until then `rdma` has no default",
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn range_inside_and_at_edges() {
        assert!(check_range(0x1000, 0x100, 0x1000, 0x100).is_ok());
        assert!(check_range(0x1000, 0x100, 0x10ff, 1).is_ok());
        assert!(check_range(0x1000, 0x100, 0x1100, 0).is_ok());
    }

    #[test]
    fn range_outside_rejected() {
        assert!(check_range(0x1000, 0x100, 0x0fff, 1).is_err());
        assert!(check_range(0x1000, 0x100, 0x10ff, 2).is_err());
        assert!(check_range(0x1000, 0x100, 0x1101, 0).is_err());
    }

    #[test]
    fn range_overflow_rejected() {
        assert!(check_range(0, usize::MAX, u64::MAX, 2).is_err());
        assert!(check_range(0, usize::MAX, 0, u32::MAX as usize + 1).is_err());
    }

    #[test]
    fn access_contains() {
        let a = Access::LOCAL_WRITE | Access::REMOTE_WRITE;
        assert!(a.contains(Access::REMOTE_WRITE));
        assert!(a.contains(Access::LOCAL_WRITE | Access::REMOTE_WRITE));
        assert!(!a.contains(Access::REMOTE_READ));
    }

    #[cfg(not(feature = "rdma"))]
    #[test]
    fn default_build_selects_a_working_mock() {
        let t = select_transport(TransportChoice::BuildDefault).expect("default build serves");
        drop(t);
    }

    #[cfg(feature = "rdma")]
    #[test]
    fn rdma_build_has_no_silent_default_before_t053() {
        assert!(matches!(
            select_transport(TransportChoice::BuildDefault),
            Err(RdmaError::Invalid { .. })
        ));
        // An explicit mock still works in this build.
        assert!(select_transport(TransportChoice::Mock(MockFabric::new())).is_ok());
    }
}
