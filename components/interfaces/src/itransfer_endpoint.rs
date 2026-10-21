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
/// `Failed` is kept apart from `Miss` deliberately. A miss says the source does
/// not have the entry; a failure says it was not delivered for a reason that
/// has nothing to do with what the source holds. A caller that cannot tell the
/// two apart would treat a broken connection as a cold cache (Principle XI).
///
/// # Examples
///
/// ```
/// use interfaces::EntryOutcome;
///
/// // Outcomes are per entry and carry the tier that served them.
/// let outcomes = vec![
///     EntryOutcome::HitMemoryTier,
///     EntryOutcome::Miss,
///     EntryOutcome::Failed,
/// ];
/// assert_eq!(outcomes.iter().filter(|o| **o == EntryOutcome::Miss).count(), 1);
/// // Only a hit delivered data.
/// assert_eq!(outcomes.iter().filter(|o| o.is_hit()).count(), 1);
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
    /// Not delivered for a reason other than residency: the connection could
    /// not be made or was lost, the write was refused or flushed, a protocol
    /// fault, a deadline, or the source's own lookup failed. Nothing usable
    /// arrived for the entry; unlike a miss, asking again may succeed.
    Failed,
}

impl EntryOutcome {
    /// Whether data for the entry arrived.
    pub fn is_hit(self) -> bool {
        matches!(
            self,
            EntryOutcome::HitMemoryTier | EntryOutcome::HitBlockTier
        )
    }
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
/// The handle carries what a caller needs to name a slot inside the region to a
/// peer: its address, length and `rkey`. [`region_at`](Self::region_at) builds
/// the [`RemoteRegion`] for one slot, bounds-checked.
///
/// # Examples
///
/// ```
/// use interfaces::LandingRegionHandle;
///
/// // Implementations mint handles; callers read them and pass them back.
/// let h = LandingRegionHandle::new(1, 0x1000, 4096, 0x42);
/// assert_eq!((h.id(), h.addr(), h.len(), h.rkey()), (1, 0x1000, 4096, 0x42));
///
/// let slot = h.region_at(1024, 512).unwrap();
/// assert_eq!((slot.addr, slot.length, slot.rkey), (0x1400, 512, 0x42));
/// // A slot past the end of the region is refused, not truncated.
/// assert!(h.region_at(4000, 512).is_none());
/// ```
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct LandingRegionHandle {
    id: u64,
    addr: u64,
    len: usize,
    rkey: u32,
}

impl LandingRegionHandle {
    /// Mint a handle. Called by the implementing component, not by callers.
    pub fn new(id: u64, addr: u64, len: usize, rkey: u32) -> Self {
        LandingRegionHandle {
            id,
            addr,
            len,
            rkey,
        }
    }

    /// The endpoint's identifier for this registration.
    pub fn id(&self) -> u64 {
        self.id
    }

    /// Start of the registered region.
    pub fn addr(&self) -> u64 {
        self.addr
    }

    /// Length of the registered region in bytes.
    pub fn len(&self) -> usize {
        self.len
    }

    /// Whether the region is empty. Never true for a handle an endpoint
    /// returned: empty registrations are refused.
    pub fn is_empty(&self) -> bool {
        self.len == 0
    }

    /// The remote key a peer needs to write into the region.
    pub fn rkey(&self) -> u32 {
        self.rkey
    }

    /// The [`RemoteRegion`] for the `len` bytes at `offset` into the region,
    /// or `None` if they do not lie wholly inside it.
    pub fn region_at(&self, offset: usize, len: u32) -> Option<RemoteRegion> {
        let end = offset.checked_add(len as usize)?;
        if end > self.len {
            return None;
        }
        Some(RemoteRegion {
            addr: self.addr.checked_add(offset as u64)?,
            rkey: self.rkey,
            length: len,
        })
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
/// ```
#[derive(Debug, Clone)]
pub enum TransferEndpointError {
    /// A required receptacle is unbound, or no landing region is registered.
    NotInitialized(String),
    /// The peer address could not be parsed as a normalized `"host:port"`.
    /// A missing port is an error, never a default.
    InvalidEndpoint(String),
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
            Self::Deadline(msg) => write!(f, "deadline expired: {msg}"),
            Self::Protocol(msg) => write!(f, "protocol error from peer: {msg}"),
            Self::IncompatiblePeer(msg) => write!(f, "incompatible peer: {msg}"),
            Self::Transport(msg) => write!(f, "transport: {msg}"),
        }
    }
}

impl std::error::Error for TransferEndpointError {}

/// What a request delivers to its [`FetchCompletion`]: one [`EntryOutcome`]
/// per requested key, in request order — or, for a fetch, the reason no reply
/// arrived.
///
/// A hit is reported only when the write is **certain** to have landed: its
/// completion succeeded, which on a reliable connection means the peer's NIC
/// acknowledged it. A failed or flushed write is uncertain — it may have
/// landed in part, or whole with only the acknowledgement lost — and is never
/// a hit. Everything that is not a hit must be treated as not landed.
pub type FetchResult = Result<Vec<EntryOutcome>, TransferEndpointError>;

/// Completion callback for [`ITransferEndpoint::fetch`],
/// [`push_async`](ITransferEndpoint::push_async) and, behind it,
/// [`push`](ITransferEndpoint::push).
///
/// Invoked **once** — after the last chunk, even when the endpoint split the
/// request internally. For a **fetch**:
///
/// * `Ok(outcomes)` — the peer replied. Each hit's write completed before the
///   reply was sent, so its slot is safe to commit; every other outcome means
///   abort that slot.
/// * `Err(..)` — no usable reply: [`TransferEndpointError::Deadline`] (none
///   came in time), [`TransferEndpointError::Protocol`] (the peer refused the
///   request), [`TransferEndpointError::Transport`] (the connection could not
///   be made, or was torn down — including as a sibling of another request's
///   deadline). **Every** slot of the request must be treated as not landed,
///   even when the endpoint split it and some chunks were answered: a fetch
///   with a hole is reported as failing, so a prefix is never half-filled. The callback fires only
///   after this end's queue pair to the peer has been forced to ERROR and
///   drained, so no write can still land: the slots are safe to reclaim.
///
/// For a **push** the outcome is always per entry, since some entries may have
/// landed before a connection was lost: `Ok`, with a hit for each write that
/// certainly landed and `Failed` for the rest. `Err` only if the push could
/// not start at all.
///
/// Boxed rather than generic so the interface stays object-safe. The callback
/// runs on the endpoint's own thread, so it must not block: that thread posts
/// work requests and reaps completions for every context on the connection.
///
/// # Examples
///
/// ```
/// use interfaces::{EntryOutcome, FetchCompletion, FetchResult, TransferEndpointError};
///
/// let seen: std::sync::Arc<std::sync::Mutex<Option<FetchResult>>> = Default::default();
/// let sink = std::sync::Arc::clone(&seen);
/// let cb: FetchCompletion = Box::new(move |r| *sink.lock().unwrap() = Some(r));
///
/// cb(Err(TransferEndpointError::Deadline("no reply from 10.0.0.2:7000".into())));
/// let got = seen.lock().unwrap().take().unwrap();
/// assert!(matches!(got, Err(TransferEndpointError::Deadline(_))));
/// # let _ = EntryOutcome::Miss;
/// ```
pub type FetchCompletion = Box<dyn FnOnce(FetchResult) + Send>;

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
    /// Entries of this end's own requests that came back from a peer's memory
    /// tier. Serving a peer's request is not counted here.
    pub hit_memory_tier: u64,
    /// Entries served from a block tier.
    pub hit_block_tier: u64,
    /// Entries reported as a miss.
    pub miss: u64,
    /// Entries reported as failed: not delivered for a reason other than
    /// residency. Kept apart from `miss` so a broken path is never counted as
    /// a cold cache.
    pub failed: u64,
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
    /// Bounce leases holding a block-tier entry's data: read, and waiting
    /// for or in its write. A read still in progress is not counted here.
    pub bounce_buffers_in_use: u32,
    /// High-water mark of the above.
    pub bounce_buffers_peak: u32,
    /// Block-tier reads handed to the dispatcher.
    pub block_reads_attempted: u64,
    /// Of those, reads that filled a lease.
    pub block_reads_completed: u64,
    /// Of those, reads that failed or could never fit the bounce reserve; each
    /// entry is reported `Failed`.
    pub block_reads_failed: u64,
    /// Block-tier entries put back in the queue for want of a free reader or
    /// bounce space, their pin released, to be looked up again. Not failures.
    pub block_deferrals: u64,
    /// Deadlines that expired, each having torn down a connection.
    pub deadline_expiries: u64,
    /// Protocol error replies received or sent.
    pub protocol_faults: u64,
    /// Connections that negotiated below the local maximum version.
    pub version_downgrades: u64,
    /// Duplicate connections closed after a simultaneous-connect race.
    pub duplicate_connections_resolved: u64,
}

/// Start-up configuration for an endpoint, given through
/// [`ITransferEndpointAdmin::configure`] before
/// [`initialize`](ITransferEndpointAdmin::initialize).
///
/// # Examples
///
/// ```
/// use interfaces::EndpointConfig;
///
/// let c = EndpointConfig::new("10.0.0.102:7000");
/// assert_eq!(c.local, "10.0.0.102:7000");
/// assert_eq!(c.send_credits, 8);
/// // A receive is posted per message credit (the reply reserve) and per
/// // request the peer may send (the quota).
/// assert!(c.max_recv_wr >= c.message_credits + c.inbound_quota);
/// assert!(c.actor_cpu.is_none());
/// ```
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EndpointConfig {
    /// This instance's own `"host:port"`: the address peers dial, and the name
    /// it identifies itself by when it connects. Every instance must be named
    /// by the same string everywhere (a peer's dialed name is checked against
    /// the name it answers to), and a missing port is an error.
    pub local: String,
    /// Send-queue work requests the device is asked to accept per connection.
    pub max_send_wr: u32,
    /// Receive work requests the device is asked to accept per connection.
    pub max_recv_wr: u32,
    /// Send-queue credits per connection: in-flight send-queue requests, and
    /// so held read pins. Clamped to what the device grants. Default 8: the
    /// bandwidth-delay product at 200 Gb/s and the measured write round trip
    /// asks for 3 at 64 KiB entries and 2 at 2.25 MiB; 8 keeps headroom for
    /// the endpoint thread's polling, which the derivation does not model
    /// (certus-transfer-endpoint research D10).
    pub send_credits: u32,
    /// Message credits per connection: requests this end may have outstanding
    /// against the peer. Also the size of this end's reply reserve — one
    /// posted receive per credit, since each request draws exactly one reply.
    /// The peer's advertised quota may lower the effective value further.
    /// Default 4: two keep the link busy — one message on the wire, one
    /// waiting at the source — and two more cover the source's per-message
    /// service time at its worst measured (D10).
    pub message_credits: u32,
    /// The inbound-request quota this end advertises: how many requests the
    /// peer may have outstanding against it. One posted receive each. Default
    /// 4, matching the peers' `message_credits`: a larger quota buys nothing a
    /// peer will use and lets one peer queue more work here.
    pub inbound_quota: u32,
    /// CPU to pin the endpoint thread to, normally one on the NIC's NUMA node.
    pub actor_cpu: Option<usize>,
    /// The destination-side **reply deadline**: how long a fetch may wait for
    /// its reply, measured from when its request is handed to the transport —
    /// not from the call, since a request waiting for a credit is queued, not
    /// stuck. `None` disables it. Distinct from any source-side stall cap on
    /// uncompleted writes, which is not this and is not configured here
    /// (FR-016).
    pub fetch_deadline: Option<std::time::Duration>,
    /// What a reply deadline does on expiry.
    pub deadline_action: DeadlineAction,
    /// Concurrent block-tier device reads, each on its own reader thread
    /// (`spdk` builds; ignored otherwise). With `send_credits`, it bounds the
    /// read pins held at once. Must be at least 1.
    pub block_readers: u32,
}

/// What a fetch's reply deadline does when it expires (FR-016).
///
/// # Examples
///
/// ```
/// use interfaces::DeadlineAction;
///
/// assert_eq!(DeadlineAction::default(), DeadlineAction::FailLoud);
/// ```
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum DeadlineAction {
    /// Log the key range, peer, chunk and queue-pair state, tear the
    /// connection down — forcing ERROR before anything is reclaimed — and fail
    /// the request with [`TransferEndpointError::Deadline`]. Every other
    /// request on the connection is aborted with it, and the blast radius is
    /// logged.
    #[default]
    FailLoud,
    /// Report what arrived and let the caller recompute. **Deferred, not
    /// implemented**: configuring it is refused, never silently treated as
    /// `FailLoud`.
    ReportPartial,
}

impl EndpointConfig {
    /// Defaults for everything but the local address.
    pub fn new(local: impl Into<String>) -> Self {
        EndpointConfig {
            local: local.into(),
            max_send_wr: 128,
            max_recv_wr: 64,
            send_credits: 8,
            message_credits: 4,
            inbound_quota: 4,
            actor_cpu: None,
            fetch_deadline: Some(std::time::Duration::from_secs(5)),
            deadline_action: DeadlineAction::FailLoud,
            block_readers: 4,
        }
    }
}

component_macros::define_interface! {
    pub ITransferEndpoint {
        /// Register a region that peers may RDMA-write into, returning the
        /// handle that names slots inside it.
        ///
        /// Must be called before any peer request can be satisfied against the
        /// region. Registration happens once — not per request — and is what
        /// makes a remote key valid. It lasts for the endpoint's life: there is
        /// no deregistration, because releasing a region safely means proving
        /// no peer can still write into it.
        ///
        /// The region must be both SPDK-DMA-able and RDMA-registrable; in
        /// practice it is allocated by `spdk_zmalloc` on the local NUMA node, as
        /// the memory tier already is. Registration goes into the endpoint's
        /// long-lived protection domain and outlives every queue pair, so the
        /// returned handle and its remote keys survive a reconnect.
        ///
        /// **Precondition** (this method cannot be `unsafe`, so it is stated
        /// here): `[base, base + len)` must stay allocated until
        /// [`ITransferEndpointAdmin::shutdown`] returns, and must not be held as
        /// a Rust reference while a peer may write into it.
        ///
        /// # Errors
        ///
        /// [`TransferEndpointError::NotInitialized`] if the endpoint is not
        /// running; [`TransferEndpointError::Transport`] if the range is null,
        /// empty, overlaps the memory-tier pool or another landing region, or
        /// the registration itself fails.
        fn register_landing_region(
            &self,
            base: *mut u8,
            len: usize,
        ) -> Result<LandingRegionHandle, TransferEndpointError>;

        /// DESTINATION SIDE. Ask one named peer for a key run, to be written
        /// into landing slots the caller has already reserved.
        ///
        /// Returns as soon as the request is queued; `on_complete` fires later
        /// with one [`EntryOutcome`] per requested key, in request order. Any
        /// length is accepted: the endpoint splits the request into chunks of
        /// the *peer's* advertised `max_entries` — not assumed equal to the
        /// local one — each its own message, sent in key order, and
        /// `on_complete` still fires exactly once, after the last chunk.
        ///
        /// Never blocks the caller and never fails for want of a message
        /// credit: chunks wait inside the endpoint until a credit is free. No
        /// request is sent before the connection's capability exchange has
        /// completed.
        ///
        /// A miss does **not** trigger any further peer traffic: this endpoint
        /// never contacts an instance the caller did not name.
        ///
        /// # Completion guarantee
        ///
        /// If this returns `Ok`, `on_complete` is invoked exactly once on every
        /// outcome — `Ok` for a reply (full success or all-miss), `Err` for a
        /// protocol error, a deadline, or a teardown with the request
        /// outstanding; see [`FetchCompletion`]. If it returns `Err`, the
        /// callback is dropped without being invoked, so a callback that
        /// releases resources must do so on drop as well as on call.
        ///
        /// # Errors
        ///
        /// [`TransferEndpointError::InvalidEndpoint`] if `peer` is not a
        /// normalized `"host:port"`; [`TransferEndpointError::IncompatiblePeer`]
        /// if its version range does not intersect the local one;
        /// [`TransferEndpointError::NotInitialized`] if no landing region is
        /// registered.
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

component_macros::define_interface! {
    pub ITransferEndpointAdmin {
        /// Set the start-up configuration. Takes effect at
        /// [`initialize`](Self::initialize); later calls before it replace
        /// earlier ones.
        ///
        /// # Errors
        ///
        /// [`TransferEndpointError::InvalidEndpoint`] if `config.local` is not a
        /// `"host:port"`; [`TransferEndpointError::Transport`] if the queue
        /// sizes cannot hold what the protocol posts — `message_credits` or
        /// `inbound_quota` of zero, or `max_recv_wr` below their sum, or a
        /// `deadline_action` that is not implemented;
        /// [`TransferEndpointError::NotInitialized`] carrying
        /// "already initialized" once the endpoint has started, since a running
        /// endpoint cannot change its name or its queue sizes.
        fn configure(&self, config: EndpointConfig) -> Result<(), TransferEndpointError>;

        /// Start listening as the configured instance and start the endpoint
        /// thread. Until this succeeds, every `ITransferEndpoint` method
        /// returns [`TransferEndpointError::NotInitialized`].
        ///
        /// # Errors
        ///
        /// [`TransferEndpointError::NotInitialized`] if not configured or
        /// already initialized; [`TransferEndpointError::Transport`] if the
        /// transport cannot listen.
        fn initialize(&self) -> Result<(), TransferEndpointError>;

        /// Stop the endpoint: refuse new work, tear down and drain every
        /// connection so all held state is released, then join the thread.
        /// Idempotent.
        ///
        /// This is the **only** way to stop a running endpoint. A component
        /// holds references to itself for interface queries, so releasing
        /// every handle does not drop it; an endpoint never shut down keeps its
        /// thread, its listener and its connections until the process exits.
        ///
        /// # Errors
        ///
        /// [`TransferEndpointError::Transport`] if a connection could not be
        /// drained; the thread is still joined, but that connection's queue
        /// pair is deliberately left undestroyed rather than lose what it holds.
        fn shutdown(&self) -> Result<(), TransferEndpointError>;
    }
}
