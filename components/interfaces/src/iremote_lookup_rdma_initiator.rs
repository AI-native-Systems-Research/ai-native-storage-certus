//! IRemoteLookupRdmaInitiator interface for pushing local cache values to remote nodes.
//!
//! This interface is an **outbound initiator**: given a target host endpoint and
//! a batch of `(key, remote-region)` pairs, it connects to the host (reusing an
//! established connection when possible), looks each key up in the local memory
//! tier, and — when the key is present and its size matches the remote region —
//! RDMA-writes the value directly into the remote host's memory.
//!
//! The component maintains a table of connections keyed by endpoint. A host that
//! is absent from the table is "disconnected"; entries are otherwise
//! "connecting", "connected", or "recovering". Connections are reused across
//! calls and repaired automatically if their queue pair enters an error state.
//!
//! # Submission is asynchronous
//!
//! The underlying verbs interface is asynchronous, and so is this one.
//! [`IRemoteLookupRdmaInitiator::push_async`] *enqueues* a batch and returns
//! immediately; a per-connection thread posts the work requests, reaps their
//! completions, and then invokes the caller's completion callback. This lets a
//! caller keep many batches in flight per peer, so control-plane latency
//! (discovery, status round trips, local lookups) overlaps data transfer instead
//! of serializing against it.
//!
//! [`IRemoteLookupRdmaInitiator::push`] remains as a blocking convenience wrapper
//! that submits and waits.
//!
//! ## The callback owns whatever keeps the source buffers alive
//!
//! The NIC reads the local value buffers asynchronously, so they must stay valid
//! and must not be evicted or relocated until the completion fires — *not* merely
//! until `push_async` returns. Whatever the caller uses to guarantee that (a
//! dispatch-map read pin, for instance) must therefore be owned by the callback,
//! not released at the submission site.
//!
//! The callback is invoked exactly once for every accepted batch, on every
//! outcome — success, per-item failure, connection loss, or teardown. If the
//! initiator is torn down with batches still outstanding, their callbacks are
//! invoked (or dropped, which must be equally safe) rather than forgotten, so a
//! caller can rely on the callback firing to release what it holds.

use std::fmt;

use crate::idispatch_map::CacheKey;
use crate::izyre::PeerId;

/// A remote memory descriptor supplied by the requesting node.
///
/// Identifies a region in the *remote* host's address space that a matching
/// local cache value may be RDMA-written into. The `length` is the size the
/// remote expects; a local value whose size differs yields
/// [`PushStatus::SizeMismatch`] rather than a partial write.
///
/// # Examples
///
/// ```
/// use interfaces::RemoteRegion;
///
/// let region = RemoteRegion { addr: 0x7f00_1000, rkey: 0x42, length: 4096 };
/// assert_eq!(region.length, 4096);
/// ```
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RemoteRegion {
    /// Remote virtual address to write into (the caller's registered buffer).
    pub addr: u64,
    /// Remote key authorizing the RDMA write into that region.
    pub rkey: u32,
    /// Expected length in bytes; must equal the local value's size.
    pub length: u32,
}

/// Per-item outcome of a push.
///
/// Reported once per input item, in the same order as the request — whether
/// delivered as the return value of [`IRemoteLookupRdmaInitiator::push`] or as the
/// argument to a [`IRemoteLookupRdmaInitiator::push_async`] completion callback.
///
/// # Examples
///
/// ```
/// use interfaces::PushStatus;
///
/// assert_ne!(PushStatus::Success, PushStatus::KeyNotFound);
/// ```
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PushStatus {
    /// The value was found locally, sizes matched, and the RDMA write completed.
    Success,
    /// No connection to the host could be established (or an in-flight write
    /// failed and the connection could not be repaired). All items in a batch
    /// share this status when the connection itself cannot be made.
    UnableToConnect,
    /// The key was not present in the local memory tier.
    KeyNotFound,
    /// The key was present but its size did not match the remote region length.
    SizeMismatch,
}

/// Errors returned by [`IRemoteLookupRdmaInitiator`] operations.
///
/// These are *method-level* failures. Per-item outcomes (key-not-found,
/// size-mismatch, unable-to-connect) are reported via [`PushStatus`], not here.
///
/// # Examples
///
/// ```
/// use interfaces::RemoteLookupRdmaInitiatorError;
///
/// let err = RemoteLookupRdmaInitiatorError::InvalidEndpoint("no port".into());
/// assert!(err.to_string().contains("invalid endpoint"));
/// ```
#[derive(Debug, Clone)]
pub enum RemoteLookupRdmaInitiatorError {
    /// The handler is missing a required receptacle, or the memory-tier pool
    /// has not been initialized.
    NotInitialized(String),
    /// The endpoint string could not be parsed as `"ip:port"`.
    InvalidEndpoint(String),
}

impl fmt::Display for RemoteLookupRdmaInitiatorError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NotInitialized(msg) => write!(f, "not initialized: {msg}"),
            Self::InvalidEndpoint(msg) => write!(f, "invalid endpoint: {msg}"),
        }
    }
}

impl std::error::Error for RemoteLookupRdmaInitiatorError {}

/// Completion callback for [`IRemoteLookupRdmaInitiator::push_async`].
///
/// Receives one [`PushStatus`] per submitted item, in request order. Invoked
/// exactly once per accepted batch (see the trait method for the precise
/// guarantee), and boxed rather than generic so the interface stays object-safe.
///
/// The callback runs on the initiator's per-connection thread, so it must not
/// block: that thread is what posts work requests and reaps completions for every
/// batch queued to that peer. In particular it must not perform an operation that
/// can wait on a lock held across I/O.
///
/// Whatever keeps the pushed values alive for the NIC — typically a read pin on
/// each key — must be *owned* by this callback, so that running it (or dropping
/// it) is what releases them. See the module documentation.
///
/// # Examples
///
/// ```
/// use interfaces::{PushCompletion, PushStatus};
///
/// let seen: std::sync::Arc<std::sync::Mutex<Vec<PushStatus>>> = Default::default();
/// let sink = std::sync::Arc::clone(&seen);
/// let cb: PushCompletion = Box::new(move |statuses| *sink.lock().unwrap() = statuses);
///
/// cb(vec![PushStatus::Success]);
/// assert_eq!(*seen.lock().unwrap(), vec![PushStatus::Success]);
/// ```
pub type PushCompletion = Box<dyn FnOnce(Vec<PushStatus>) + Send>;

component_macros::define_interface! {
    pub IRemoteLookupRdmaInitiator {
        /// Submit a batch of local cache values to be pushed to a remote host over
        /// RDMA, and return without waiting for the transfer.
        ///
        /// The batch is queued against `endpoint` (an `"ip:port"` string), whose
        /// connection is established, reused, or repaired as needed. For each
        /// `(key, region)` item the key is looked up in the local memory tier; if
        /// present and its size equals `region.length`, the value is RDMA-written
        /// into the remote region. When every write in the batch has completed,
        /// `on_complete` is invoked with one [`PushStatus`] per input item, in
        /// request order.
        ///
        /// Because submission returns before the NIC has read the local buffers,
        /// the caller must keep those buffers valid until `on_complete` runs —
        /// which is why whatever guarantees that should be owned by the callback.
        /// See the module documentation.
        ///
        /// # Completion guarantee
        ///
        /// If this returns `Ok(())`, `on_complete` is invoked **exactly once**, on
        /// every outcome: full success, per-item failure, connection loss, or
        /// teardown with the batch still outstanding. If it returns `Err`, the
        /// callback is dropped without being invoked, so a callback that releases
        /// resources must do so on drop as well as on call.
        ///
        /// A batch rejected because the peer's submit queue is full is *not* an
        /// error: it reports [`PushStatus::UnableToConnect`] for every item, and
        /// may do so by invoking `on_complete` synchronously on the calling
        /// thread before returning `Ok(())`. Callers must tolerate that
        /// reentrancy.
        ///
        /// # Errors
        ///
        /// Returns [`RemoteLookupRdmaInitiatorError::NotInitialized`] if the memory-tier
        /// receptacle is unbound or its pool is not initialized, or
        /// [`RemoteLookupRdmaInitiatorError::InvalidEndpoint`] if `endpoint` is not a
        /// valid `"ip:port"`.
        fn push_async(
            &self,
            endpoint: &str,
            items: &[(CacheKey, RemoteRegion)],
            on_complete: PushCompletion,
        ) -> Result<(), RemoteLookupRdmaInitiatorError>;

        /// Push local cache values to a remote host over RDMA, blocking until every
        /// write in the batch has completed.
        ///
        /// A convenience wrapper over [`push_async`](Self::push_async) with
        /// identical semantics, for callers that have nothing to overlap and would
        /// only wait anyway. It holds the calling thread for the whole transfer, so
        /// it must not be used from a thread that also needs to make progress on
        /// other work — notably not from a completion callback.
        ///
        /// Returns one [`PushStatus`] per input item, in order. If no connection
        /// to the host can be established, every item is reported as
        /// [`PushStatus::UnableToConnect`].
        ///
        /// # Errors
        ///
        /// As [`push_async`](Self::push_async).
        fn push(
            &self,
            endpoint: &str,
            items: &[(CacheKey, RemoteRegion)],
        ) -> Result<Vec<PushStatus>, RemoteLookupRdmaInitiatorError>;

        /// Proactively establish (warm) a connection to `endpoint` without
        /// writing anything.
        ///
        /// Idempotent and connection-caching like [`push`](Self::push): if a
        /// healthy connection already exists this is a no-op; otherwise it runs
        /// the full `rdma_cm` connect so that a later `push` to the same endpoint
        /// hits the established-connection fast path. Intended to be called off
        /// the caller's hot path (e.g. when a peer is discovered), since a cold
        /// connect can block for a long time.
        ///
        /// # Errors
        ///
        /// Returns [`RemoteLookupRdmaInitiatorError::NotInitialized`] if the
        /// memory-tier receptacle is unbound or its pool is not initialized (the
        /// pool must be registered before a connection can be built), or
        /// [`RemoteLookupRdmaInitiatorError::InvalidEndpoint`] if `endpoint` is
        /// not a valid `"ip:port"`. A connection that cannot be established is
        /// reported as `Ok(())` with no connection cached — the next `connect`
        /// or `push` will retry — so warming never surfaces a transient network
        /// failure as an error.
        fn connect(&self, endpoint: &str) -> Result<(), RemoteLookupRdmaInitiatorError>;

        /// Tear down the connection to a single host, if one exists.
        ///
        /// Idempotent: disconnecting an unknown endpoint is a no-op. Note that a
        /// host may back multiple discovery-layer peers, so callers should only
        /// disconnect once the host (not merely one peer) is known to be gone.
        fn disconnect(&self, endpoint: &str);

        /// Tear down all connections in the table.
        fn disconnect_all(&self);

        /// Supply this node's own zyre `PeerId`, stamped into the `rdma_cm`
        /// connect `private_data` on every outbound connection so the remote
        /// responder can correlate the inbound queue pair to this peer (required
        /// for teardown-before-reclaim). Should be called once, before the first
        /// `push`. Connections opened before it is set are unidentified
        /// (`node: None` on the responder), reclaimable only via the responder's
        /// backstop shutdown.
        fn set_local_peer_id(&self, peer: PeerId);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn error_display_not_initialized() {
        let err = RemoteLookupRdmaInitiatorError::NotInitialized("no memory tier".into());
        assert!(err.to_string().contains("not initialized"));
    }

    #[test]
    fn error_display_invalid_endpoint() {
        let err = RemoteLookupRdmaInitiatorError::InvalidEndpoint("missing port".into());
        assert!(err.to_string().contains("invalid endpoint"));
    }

    #[test]
    fn push_status_distinct() {
        assert_ne!(PushStatus::Success, PushStatus::UnableToConnect);
        assert_ne!(PushStatus::KeyNotFound, PushStatus::SizeMismatch);
    }

    #[test]
    fn remote_region_is_copy() {
        let a = RemoteRegion {
            addr: 0x1000,
            rkey: 7,
            length: 256,
        };
        let b = a;
        assert_eq!(a, b);
    }
}
