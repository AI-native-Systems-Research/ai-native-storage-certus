//! IRemoteLookup interface and associated types.

use std::fmt;
use std::time::Duration;

use crate::idispatch_map::CacheKey;
use crate::izyre::GossipConfig;

/// Configuration for `remote-lookup` supplied to
/// [`IRemoteLookup::initialize`].
///
/// A public value type (mirrors `DispatcherConfig`): it derives no serde but
/// implements [`Default`] so integrating mainlines can build it with
/// `..Default::default()` and stay robust to config growth. Every field has a
/// sensible default.
///
/// # Examples
///
/// ```
/// use interfaces::LookupConfig;
///
/// let cfg = LookupConfig { quorum_pct: 90, ..Default::default() };
/// assert_eq!(cfg.quorum_pct, 90);
/// assert_eq!(LookupConfig::default().max_keys_per_query, 256);
/// // By default the caller is coupled to `op_deadline` (no separate wait).
/// assert_eq!(LookupConfig::default().caller_wait, None);
/// ```
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LookupConfig {
    /// zyre group joined on activation.
    pub group: String,
    /// Percentage of group peers that must reply to trigger the Phase-1→Phase-2
    /// transition (0–100).
    pub quorum_pct: u8,
    /// Phase-1 cap before falling through to Phase-2 regardless of quorum.
    pub phase1_timeout: Duration,
    /// Overall operation deadline: how long the actor keeps an operation alive —
    /// fetching, retrying, and publishing landed keys to the local tier — before
    /// finalizing remaining keys as `NotFound`. Decoupled from how long the caller
    /// blocks (see [`caller_wait`](Self::caller_wait)): after the caller returns,
    /// the operation keeps running to this deadline so a slow/recovering fetch
    /// still populates the cache for the next lookup.
    pub op_deadline: Duration,
    /// How long `batch_lookup` blocks the calling thread before returning
    /// `NotFound` for unsatisfied keys. `None` couples the caller to
    /// [`op_deadline`](Self::op_deadline) (the historical behavior — block until
    /// the operation finalizes). `Some(w)` lets the caller give up after `w` while
    /// the operation continues in the background (publish-on-success fills the
    /// cache for the next lookup). Should be `<= op_deadline`.
    pub caller_wait: Option<Duration>,
    /// Grace period, measured from an operation's finalize, before an orphaned
    /// landing slot (one exposed to a still-live peer whose one-sided write never
    /// resolved) is force-reclaimed: the peer's QP is torn down (Disconnect →
    /// DisconnectAck) and only then is the buffer freed. Bounds the lifetime of a
    /// buffer that a peer could still DMA into, without ever freeing one under a
    /// possibly-pending write.
    pub connection_teardown_timeout: Duration,
    /// Maximum number of retry rounds re-targeting alternate peers.
    pub max_retry_rounds: u32,
    /// Maximum keys per KEY_QUERY message before splitting into multiple SHOUTs.
    pub max_keys_per_query: usize,
    /// RoCE IPv4 the responder binds to (handed to the responder admin).
    pub bind_ip: String,
    /// Optional NUMA/CPU pin for the actor + responder loop.
    pub actor_cpu: Option<usize>,
    /// Peer-discovery mode for the zyre node. `None` (the default) uses zyre's
    /// UDP-beacon discovery, appropriate for a single broadcast domain. `Some`
    /// selects gossip-based discovery over an explicit hub endpoint — required
    /// for clusters that span subnets (where UDP broadcast does not reach) and
    /// used by the in-process multi-node test mesh over TCP loopback. Maps to
    /// [`NodeConfig::gossip`](crate::NodeConfig).
    pub discovery: Option<GossipConfig>,
    /// This node's own ZRE data-mailbox endpoint (e.g. `"tcp://127.0.0.1:0"`).
    /// Required by zyre when [`discovery`](Self::discovery) is `Some` (gossip
    /// mode disables beaconing, so the node must publish its endpoint
    /// explicitly); ignored in beacon mode. Maps to
    /// [`NodeConfig::endpoint`](crate::NodeConfig).
    pub node_endpoint: Option<String>,
}

impl Default for LookupConfig {
    fn default() -> Self {
        Self {
            group: "remote_lookup".to_string(),
            quorum_pct: 80,
            phase1_timeout: Duration::from_millis(20),
            op_deadline: Duration::from_millis(50),
            caller_wait: None,
            connection_teardown_timeout: Duration::from_millis(1000),
            max_retry_rounds: 2,
            max_keys_per_query: 256,
            bind_ip: String::new(),
            actor_cpu: None,
            discovery: None,
            node_endpoint: None,
        }
    }
}

/// Errors returned by `IRemoteLookup` operations.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RemoteLookupError {
    /// The requested key was not found on the remote node.
    NotFound,
    /// A transport or network error occurred.
    TransportError(String),
}

impl fmt::Display for RemoteLookupError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NotFound => write!(f, "key not found"),
            Self::TransportError(msg) => write!(f, "transport error: {msg}"),
        }
    }
}

impl std::error::Error for RemoteLookupError {}

/// A cumulative snapshot of what this node has served **to its peers**.
///
/// Responder-side, and deliberately so: these count work other nodes caused here,
/// which is the opposite direction from `TierEventStats::remote_lookup_hits`
/// (what this node obtained *from* peers). Conflating the two is easy and the
/// names are chosen to make it hard.
///
/// Both fields are monotonic since process start; subtract two successive
/// snapshots for a per-interval rate.
///
/// # Examples
///
/// ```
/// use interfaces::RemoteServeStats;
///
/// let s = RemoteServeStats::default();
/// assert_eq!((s.peer_served_keys, s.peer_triggered_promotions), (0, 0));
/// ```
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct RemoteServeStats {
    /// Keys this node successfully served to a peer's RDMA request.
    pub peer_served_keys: u64,
    /// Keys this node had to read from its **own disk** in order to serve a peer —
    /// promoted into the local memory tier before the RDMA read, because there is
    /// no RDMA-from-SSD path.
    ///
    /// This is the cold-serve signal: the pressure peers place on this node's
    /// storage tier. It is a **first-touch** quantity, since the promotion leaves
    /// the entry in DRAM and a second request for the same key is served warm.
    /// That makes it meaningful as a rate and misleading as a running total
    /// compared against `peer_served_keys` over a long window.
    pub peer_triggered_promotions: u64,
}

component_macros::define_interface! {
    pub IRemoteLookup {
        /// Configure and bring up the component.
        ///
        /// Supplies [`LookupConfig`] and starts the actor: joins the zyre group
        /// and drives RDMA-responder bring-up (bind IP + pool registration) via
        /// the component's responder-admin receptacle. Mirrors
        /// `IDispatcher::initialize(DispatcherConfig)`. Must be called before
        /// `batch_lookup`; calling it more than once returns a transport error.
        ///
        /// # Examples
        ///
        /// ```
        /// use interfaces::{IRemoteLookup, LookupConfig, RemoteLookupError};
        ///
        /// # fn example(rl: &dyn IRemoteLookup) -> Result<(), RemoteLookupError> {
        /// rl.initialize(LookupConfig::default())?;
        /// # Ok(())
        /// # }
        /// ```
        fn initialize(&self, config: LookupConfig) -> Result<(), RemoteLookupError>;

        /// Batch lookup: fetch multiple cache entries from remote nodes into the
        /// local DRAM memory tier.
        ///
        /// Each entry is a `(key, size)` pair — `size` is the expected value
        /// length in bytes, not an address. remote-lookup works exclusively in
        /// CPU/DRAM: on success it makes the key resident in the local memory
        /// tier and returns `Ok(())` for it; the caller (the dispatcher) performs
        /// any subsequent DRAM→GPU delivery. Returns one `Result` per input
        /// entry, preserving positional order.
        ///
        /// # Examples
        ///
        /// ```
        /// use interfaces::{CacheKey, IRemoteLookup, RemoteLookupError};
        ///
        /// # fn example(rl: &dyn IRemoteLookup) {
        /// let entries: Vec<(CacheKey, u32)> = vec![(1, 4096), (2, 8192)];
        /// let results = rl.batch_lookup(&entries);
        /// assert_eq!(results.len(), entries.len());
        /// # }
        /// ```
        fn batch_lookup(
            &self,
            entries: &[(CacheKey, u32)],
        ) -> Vec<Result<(), RemoteLookupError>>;

        /// Join a cluster of Certus nodes at the given endpoint.
        ///
        /// # Examples
        ///
        /// ```
        /// use interfaces::{IRemoteLookup, RemoteLookupError};
        ///
        /// # fn example(rl: &dyn IRemoteLookup) -> Result<(), RemoteLookupError> {
        /// rl.join_cluster("192.168.1.10:9090")?;
        /// # Ok(())
        /// # }
        /// ```
        fn join_cluster(&self, endpoint: &str) -> Result<(), RemoteLookupError>;

        /// Leave the cluster, disconnecting from remote nodes.
        ///
        /// # Examples
        ///
        /// ```
        /// use interfaces::{IRemoteLookup, RemoteLookupError};
        ///
        /// # fn example(rl: &dyn IRemoteLookup) -> Result<(), RemoteLookupError> {
        /// rl.leave_cluster()?;
        /// # Ok(())
        /// # }
        /// ```
        fn leave_cluster(&self) -> Result<(), RemoteLookupError>;

        /// A cumulative snapshot of what this node has served to its peers.
        ///
        /// Mirrors `IDispatcher::tier_event_stats()`: always populated, never reset
        /// by reading, and zero on an implementation that serves no peers. Polled by
        /// the server for `/metrics` rather than pushed, so nothing is lost if no one
        /// asks.
        ///
        /// # Examples
        ///
        /// ```
        /// use interfaces::IRemoteLookup;
        ///
        /// # fn example(rl: &dyn IRemoteLookup) {
        /// let before = rl.serve_stats();
        /// // ... peers issue requests ...
        /// let after = rl.serve_stats();
        /// let cold = after.peer_triggered_promotions - before.peer_triggered_promotions;
        /// let _ = cold;
        /// # }
        /// ```
        fn serve_stats(&self) -> RemoteServeStats;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn remote_lookup_error_display() {
        assert!(RemoteLookupError::NotFound
            .to_string()
            .contains("not found"));
        assert!(RemoteLookupError::TransportError("timeout".into())
            .to_string()
            .contains("timeout"));
    }
}
