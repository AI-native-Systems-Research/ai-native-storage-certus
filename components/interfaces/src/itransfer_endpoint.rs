//! `ITransferEndpoint`: explicit certus-to-certus KV cache transfer over a
//! symmetric RDMA endpoint.
//!
//! A destination reserves landing slots in its own memory tier, names a single
//! peer — chosen externally, by llm-d — and asks that peer for a run of block
//! keys. The source RDMA-writes the values it holds directly into those slots
//! and replies with a per-key outcome. There is no broadcast, no peer polling
//! and no reap: exactly one peer is asked, exactly once.
//!
//! # The endpoint is symmetric; the API is not
//!
//! One queue pair per peer *instance* pair, either end able to post work
//! requests, and the same connection carries traffic in both directions — an
//! instance may be destination for one transfer and source for another
//! simultaneously.
//!
//! The API is deliberately asymmetric all the same:
//!
//! | role | how it is driven |
//! |---|---|
//! | **destination** (wants values) | an explicit [`ITransferEndpoint::fetch`] |
//! | **source** (holds values) | **reactive** — inbound requests, no call |
//!
//! There is deliberately **no** `serve` method. Once initialized, an endpoint
//! answers peer requests from its own thread by consulting the dispatch map and
//! pushing. Serving is not something a caller schedules, so exposing it would
//! invite callers to try. The legacy [`ITransferEndpoint::push_async`] and
//! [`ITransferEndpoint::push`] are the exception: they are source-initiated
//! because their control plane arrives out of band.
//!
//! # A peer is an instance, not a host
//!
//! Several certus instances run per node — one per NUMA domain in the current
//! rigs — so a host alone is not an address. Every `peer`/`endpoint` argument is
//! a normalized `"host:port"`, and an address without a port is
//! [`TransferEndpointError::InvalidEndpoint`], never defaulted. Two instances on
//! one host differ only by port, hold separate connections, and may both be the
//! local host, which is also the single-host loopback verification path.
//!
//! # Transfers are invisible to vLLM
//!
//! A completed transfer simply makes keys resident in the local memory tier, so
//! the ordinary load path serves the GPU unmodified and a transferred hit is
//! indistinguishable from a local one.

use std::fmt;

use crate::idispatch_map::CacheKey;
// Reused verbatim rather than redefined — there is exactly one remote-region
// descriptor in the system. At cutover, when the legacy RDMA interfaces are
// retired, `RemoteRegion` should move into this module rather than be deleted.
use crate::iremote_lookup_rdma_initiator::RemoteRegion;

/// Per-entry result of a transfer, reported in request order.
///
/// A miss is an *outcome*, not an error: the named peer simply does not hold
/// that key, holds it at a different size, or cannot serve it in this build.
/// None of those cause further peer traffic — this mechanism is the alternative
/// to broadcast lookup, not a layer above it.
///
/// # Examples
///
/// ```
/// use interfaces::EntryOutcome;
///
/// // Outcomes are per entry and carry the tier that served them.
/// let outcomes = vec![EntryOutcome::HitMemoryTier, EntryOutcome::Miss];
/// assert_eq!(outcomes.iter().filter(|o| **o == EntryOutcome::Miss).count(), 1);
/// ```
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EntryOutcome {
    /// Served from the source's memory tier.
    HitMemoryTier,
    /// Served from the source's block tier, staged through a registered bounce
    /// buffer. Reported only when the device was actually read.
    HitBlockTier,
    /// The source does not hold it, holds it at a different size, or cannot
    /// serve it in this build — a build without the `spdk` feature reports a
    /// block-tier entry as a miss rather than failing.
    Miss,
}

/// Identifies one in-flight fetch, echoed in the peer's reply.
///
/// It exists to correlate a [`TransferEndpointError::Deadline`] or a log line
/// with the request that produced it — including telling the context that
/// actually expired apart from the siblings aborted with it when their shared
/// connection was torn down.
///
/// It is **not** a cancellation handle. Cancelling a fetch cannot be made safe
/// while the destination still holds the landing slots the peer may write into,
/// and holding them is the whole point of the teardown-before-reclaim barrier.
///
/// # Examples
///
/// ```
/// use interfaces::TransferContextId;
///
/// let id = TransferContextId(7);
/// assert_eq!(id.0, 7);
/// ```
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct TransferContextId(pub u64);

/// Handle for a registered region that peers may RDMA-write into.
///
/// Registration happens once, not per request, and lives in the endpoint's
/// long-lived protection domain, so the handle and the remote keys derived from
/// it stay valid across a reconnect. That differs from the predecessor crates,
/// which re-register on every connect.
///
/// # Examples
///
/// ```
/// use interfaces::LandingRegionHandle;
///
/// // Opaque: implementations mint handles, callers only pass them back.
/// let h = LandingRegionHandle::new(1);
/// assert_eq!(h.id(), 1);
/// ```
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct LandingRegionHandle(u64);

impl LandingRegionHandle {
    /// Mint a handle. Called by the implementing component, not by callers.
    pub fn new(id: u64) -> Self {
        Self(id)
    }

    /// The opaque identifier behind this handle.
    pub fn id(&self) -> u64 {
        self.0
    }
}

/// Errors returned by [`ITransferEndpoint`] operations.
///
/// These are *method-level* failures. Per-entry results — including a miss —
/// are reported as [`EntryOutcome`], not here.
///
/// # The two loud failures differ in blast radius
///
/// [`TransferEndpointError::Deadline`] and [`TransferEndpointError::Protocol`]
/// are both loud, and the distinction is the point rather than an accident:
///
/// | | `Deadline` | `Protocol` |
/// |---|---|---|
/// | what is known | nothing — the peer may be hung, partitioned, or about to write | the peer is alive and answered |
/// | queue pair | forced to ERROR and drained | untouched |
/// | other contexts | all aborted with it | unaffected |
/// | latency to detect | a deadline | a round trip |
///
/// Forcing the queue pair to ERROR is the only thing that makes a late write
/// impossible, so it is mandatory when the peer's state is unknown and
/// gratuitous when it is not.
///
/// # Examples
///
/// ```
/// use interfaces::TransferEndpointError;
///
/// let err = TransferEndpointError::InvalidEndpoint("no port".into());
/// assert!(err.to_string().contains("invalid endpoint"));
///
/// // A request too large for one message reports the limit it must chunk to.
/// let err = TransferEndpointError::RequestTooLarge { max_entries: 340 };
/// assert!(err.to_string().contains("340"));
/// ```
#[derive(Debug, Clone)]
pub enum TransferEndpointError {
    /// A required receptacle is unbound, or no landing region is registered.
    NotInitialized(String),
    /// The peer address could not be parsed as a normalized `"host:port"`.
    /// A missing port is an error, never a default.
    InvalidEndpoint(String),
    /// The request exceeds what one message can carry. The caller may chunk, or
    /// let the endpoint chunk internally. The limit is derived from the
    /// receiving peer's buffer, not assumed equal to the local one.
    RequestTooLarge {
        /// Entries that fit in one request to this peer.
        max_entries: usize,
    },
    /// Message credits are exhausted and the caller asked not to block.
    WouldBlock,
    /// The deadline expired with the request neither fulfilled nor refused.
    /// The queue pair has been forced to ERROR and this connection's other
    /// contexts were aborted with it. Carries key range, peer, chunk index and
    /// queue-pair state.
    Deadline(String),
    /// The peer answered our request with a protocol error: unknown opcode,
    /// unsupported version, malformed message, or more entries than its posted
    /// buffer holds. The peer is healthy and acted on nothing, so **the
    /// connection remains usable** and sibling contexts are unaffected. A
    /// *reply* we could not process is a teardown, reported as `Transport`.
    Protocol(String),
    /// The peer's supported protocol version range does not intersect the local
    /// one, so no connection was established and no request was issued. Carries
    /// peer and both ranges.
    IncompatiblePeer(String),
    /// The transport failed.
    Transport(String),
}

impl fmt::Display for TransferEndpointError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NotInitialized(msg) => write!(f, "not initialized: {msg}"),
            Self::InvalidEndpoint(msg) => write!(f, "invalid endpoint: {msg}"),
            Self::RequestTooLarge { max_entries } => {
                write!(f, "request too large: at most {max_entries} entries")
            }
            Self::WouldBlock => write!(f, "message credits exhausted and told not to block"),
            Self::Deadline(msg) => write!(f, "deadline expired: {msg}"),
            Self::Protocol(msg) => write!(f, "protocol error from peer: {msg}"),
            Self::IncompatiblePeer(msg) => write!(f, "incompatible peer: {msg}"),
            Self::Transport(msg) => write!(f, "transport: {msg}"),
        }
    }
}

impl std::error::Error for TransferEndpointError {}

/// Completion callback for [`ITransferEndpoint::fetch`].
///
/// Receives exactly one [`EntryOutcome`] per requested key, in request order,
/// and is invoked **once** — after the last chunk, even when the endpoint split
/// the request internally. Boxed rather than generic so the interface stays
/// object-safe.
///
/// The callback runs on the endpoint's own thread, so it must not block: that
/// thread posts work requests and reaps completions for every context on the
/// connection.
///
/// # Examples
///
/// ```
/// use interfaces::{EntryOutcome, FetchCompletion};
///
/// let seen: std::sync::Arc<std::sync::Mutex<Vec<EntryOutcome>>> = Default::default();
/// let sink = std::sync::Arc::clone(&seen);
/// let cb: FetchCompletion = Box::new(move |outcomes| *sink.lock().unwrap() = outcomes);
///
/// cb(vec![EntryOutcome::HitMemoryTier]);
/// assert_eq!(*seen.lock().unwrap(), vec![EntryOutcome::HitMemoryTier]);
/// ```
pub type FetchCompletion = Box<dyn FnOnce(Vec<EntryOutcome>) + Send>;

/// A snapshot of endpoint counters.
///
/// Attempts are counted alongside completions deliberately: a metric set in
/// which an unexercised path is indistinguishable from a working one is not
/// acceptable for a component whose purpose is measurement.
///
/// Credit domains are reported separately because they bound genuinely
/// different resources and exhaust independently — conflating them would
/// over-restrict two of the three. Message credits are per direction, since one
/// connection carries both.
///
/// # Examples
///
/// ```
/// use interfaces::TransferStats;
///
/// let s = TransferStats::default();
/// assert_eq!(s.requests_attempted, 0);
/// // Attempted-versus-completed is what makes a dropped request visible.
/// assert_eq!(s.requests_attempted, s.requests_completed);
/// ```
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct TransferStats {
    /// Entries named across all requests.
    pub entries_requested: u64,
    /// Entries served from a memory tier.
    pub hit_memory_tier: u64,
    /// Entries served from a block tier.
    pub hit_block_tier: u64,
    /// Entries reported as a miss.
    pub miss: u64,
    /// Fetches begun.
    pub requests_attempted: u64,
    /// Fetches whose completion fired.
    pub requests_completed: u64,
    /// Messages sent, counting chunks separately.
    pub chunks_sent: u64,
    /// In-flight RDMA writes, which transitively bounds held read pins.
    pub send_queue_in_flight: u32,
    /// High-water mark of the above; compare against the configured limit.
    pub send_queue_peak_in_flight: u32,
    /// Our requests awaiting a reply.
    pub requests_outstanding_outbound: u32,
    /// Peer requests we are currently serving.
    pub requests_in_service_inbound: u32,
    /// Bounce buffers leased for block-tier serving.
    pub bounce_buffers_in_use: u32,
    /// High-water mark of the above.
    pub bounce_buffers_peak: u32,
    /// Deadlines that expired, each having torn down a connection.
    pub deadline_expiries: u64,
    /// Protocol error replies received or sent.
    pub protocol_faults: u64,
    /// Connections that negotiated below the local maximum version.
    pub version_downgrades: u64,
    /// Duplicate connections closed after a simultaneous-connect race.
    pub duplicate_connections_resolved: u64,
}

component_macros::define_interface! {
    pub ITransferEndpoint {
        /// Register a region that peers may RDMA-write into.
        ///
        /// Must be called before any peer request can be satisfied against the
        /// region. Registration happens once — not per request — and is what
        /// makes a remote key valid.
        ///
        /// The region must be both SPDK-DMA-able and RDMA-registrable; in
        /// practice it is allocated by `spdk_zmalloc` on the local NUMA node, as
        /// the memory tier already is. Registration goes into the endpoint's
        /// long-lived protection domain and outlives every queue pair, so the
        /// returned handle and its remote keys survive a reconnect.
        ///
        /// # Errors
        ///
        /// [`TransferEndpointError::NotInitialized`] if the endpoint has no
        /// device open, or [`TransferEndpointError::Transport`] if registration
        /// fails.
        fn register_landing_region(
            &self,
            base: *mut u8,
            len: usize,
        ) -> Result<LandingRegionHandle, TransferEndpointError>;

        /// DESTINATION SIDE. Ask one named peer for a key run, to be written
        /// into landing slots the caller has already reserved.
        ///
        /// Returns as soon as the request is sent; `on_complete` fires later
        /// with one [`EntryOutcome`] per requested key, in request order. The
        /// endpoint may split the request internally — against the *peer's*
        /// advertised limit, which is not assumed equal to the local one — and
        /// `on_complete` still fires exactly once, after the last chunk.
        ///
        /// Blocks if message credits are exhausted. No request is sent before
        /// the connection's capability exchange has completed.
        ///
        /// A miss does **not** trigger any further peer traffic: this endpoint
        /// never contacts an instance the caller did not name.
        ///
        /// # Completion guarantee
        ///
        /// If this returns `Ok`, `on_complete` is invoked exactly once on every
        /// outcome — full success, all-miss, protocol error, deadline, or
        /// teardown with the context outstanding. If it returns `Err`, the
        /// callback is dropped without being invoked, so a callback that
        /// releases resources must do so on drop as well as on call.
        ///
        /// # Errors
        ///
        /// [`TransferEndpointError::InvalidEndpoint`] if `peer` is not a
        /// normalized `"host:port"`; [`TransferEndpointError::IncompatiblePeer`]
        /// if its version range does not intersect the local one;
        /// [`TransferEndpointError::RequestTooLarge`] if the caller asked not to
        /// chunk; [`TransferEndpointError::NotInitialized`] if no landing region
        /// is registered.
        fn fetch(
            &self,
            peer: &str,
            items: &[(CacheKey, RemoteRegion)],
            on_complete: FetchCompletion,
        ) -> Result<TransferContextId, TransferEndpointError>;

        /// LEGACY SUBSET. Source-initiated push with no messaging, for callers
        /// whose control plane is elsewhere.
        ///
        /// Signature-compatible with `IRemoteLookupRdmaInitiator::push_async` so
        /// an existing caller can be re-pointed with a receptacle change and no
        /// logic change. The one difference is the per-item type:
        /// [`EntryOutcome`] adds the tier distinction to `PushStatus`, and can
        /// collapse back at the boundary for a caller that does not want it.
        ///
        /// # Errors
        ///
        /// As [`fetch`](Self::fetch), minus the version and chunking cases.
        fn push_async(
            &self,
            endpoint: &str,
            items: &[(CacheKey, RemoteRegion)],
            on_complete: FetchCompletion,
        ) -> Result<(), TransferEndpointError>;

        /// Blocking convenience over [`push_async`](Self::push_async), with one
        /// outcome per item in order.
        ///
        /// It holds the calling thread for the whole transfer, so it must not be
        /// used from a thread that also needs to make progress on other work —
        /// in particular, not from a completion callback.
        ///
        /// # Errors
        ///
        /// As [`push_async`](Self::push_async).
        fn push(
            &self,
            endpoint: &str,
            items: &[(CacheKey, RemoteRegion)],
        ) -> Result<Vec<EntryOutcome>, TransferEndpointError>;

        /// Tear down a peer's queue pair, transitioning it to ERROR and draining
        /// its completions.
        ///
        /// MUST complete before the caller reclaims any landing slot that peer
        /// could still write into — teardown before reclaim is what makes a late
        /// write impossible, and nothing weaker suffices. Because one connection
        /// carries every context to a peer, this aborts all of them, not only
        /// the one the caller had in mind.
        ///
        /// # Errors
        ///
        /// [`TransferEndpointError::InvalidEndpoint`] if `peer` does not parse.
        /// Disconnecting an unknown peer is not an error.
        fn disconnect(&self, peer: &str) -> Result<(), TransferEndpointError>;

        /// Snapshot the endpoint's counters.
        fn stats(&self) -> TransferStats;
    }
}
