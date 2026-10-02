//! IDispatcher interface and associated types for the dispatcher component.

use std::fmt;

use crate::idispatch_map::CacheKey;
#[cfg(feature = "spdk")]
use crate::igpu_services::GpuStream;

/// Configuration for dispatcher initialization.
///
/// # Examples
///
/// ```
/// use interfaces::DispatcherConfig;
///
/// let config = DispatcherConfig {
///     data_pci_addrs: vec![
///         "0000:02:00.0".to_string(),
///         "0000:03:00.0".to_string(),
///     ],
///     ..Default::default()
/// };
/// assert_eq!(config.data_pci_addrs.len(), 2);
/// ```
#[derive(Debug, Clone)]
pub struct DispatcherConfig {
    /// PCI addresses of N data block devices (one per extent manager).
    pub data_pci_addrs: Vec<String>,
    /// Maximum entries in the in-memory cache. Default: 10000.
    pub max_cache_entries: usize,
    /// Whether to format extent managers on initialization.
    /// Default: true. Set to false when re-initializing to preserve on-disk data.
    pub format_on_init: bool,
    /// SSD utilization fraction (0.0–1.0) at which background eviction starts.
    /// Default: 0.9 (eviction triggers at 90% full). Set to 0.0 to disable.
    pub ssd_eviction_threshold: f64,
    /// SSD utilization fraction below which eviction stops (low-water mark).
    /// Default: 0.8.
    pub ssd_eviction_low_watermark: f64,
    /// Number of extents to evaluate per eviction sweep cycle.
    /// Default: 64.
    pub ssd_eviction_batch_size: usize,
    /// Seconds between SSD utilization checks.
    /// Default: 5.
    pub ssd_eviction_interval_secs: u64,
    /// Base CPU index for NVMe poller threads.
    ///
    /// Drive `i`'s actor thread is pinned to CPU `poller_base_cpu + i`.
    /// When `None`, the dispatcher pins each drive's actor to a core on the
    /// drive's NUMA node, round-robin over that node's cores excluding its
    /// first two. A drive whose NUMA node cannot be resolved, or whose node has
    /// two or fewer cores, is left to the block device's own NUMA-local choice;
    /// if the NUMA topology cannot be discovered at all, pollers are unpinned.
    /// Set this to a dedicated core range to give each drive exclusive use of
    /// a core, which is required for SPDK busy-polling to achieve full bandwidth.
    pub poller_base_cpu: Option<usize>,
    /// Maximum eviction attempts before returning AllocationFailed.
    /// Default: 2048.
    pub max_eviction_attempts: usize,
    /// Maximum time, in milliseconds, that a store allocation
    /// (`reserve_memory`) will backpressure — retrying eviction while the
    /// background evictor demotes and in-flight write-throughs land — before
    /// surfacing `AllocationFailed`. Absorbs momentary memory-tier saturation
    /// under sustained store bursts (e.g. async closed-loop offload) instead of
    /// failing the store, which is fatal to the vLLM offloading connector.
    /// Default: 5000 (5 s). Set to 0 to disable (fail fast, original behavior).
    /// Only the store path waits; load paths keep their staging fallback.
    pub store_backpressure_ms: u64,
    /// Delay in milliseconds between DRAM backfill jobs after P2P cold reads.
    /// Throttles background NVMe→DRAM reads to avoid contending with active
    /// P2P cold reads for drive bandwidth.
    /// Default: 10. Set to 0 to disable backfill entirely.
    pub backfill_delay_ms: u64,
    /// Size of the extent-manager metadata partition in bytes.
    /// Default: 128 MiB.
    pub metadata_partition_size: u64,
    /// Size of the extended metadata partition in bytes.
    /// Default: 128 MiB.
    pub extended_metadata_partition_size: u64,
    /// Memory-tier utilization fraction (0.0–1.0) at which background demotion starts.
    /// Demotes LRU entries from DRAM to SSD proactively.
    /// Default: 0.0 (disabled). Set to e.g. 0.8 to trigger at 80% full.
    pub memory_tier_eviction_threshold: f64,
    /// Memory-tier utilization fraction below which demotion stops (low-water mark).
    /// Default: 0.70.
    pub memory_tier_eviction_low_watermark: f64,
    /// Number of entries to evaluate per memory-tier demotion sweep cycle.
    /// Default: 64.
    pub memory_tier_eviction_batch_size: usize,
    /// Seconds between memory-tier utilization checks.
    /// Default: 2.
    pub memory_tier_eviction_interval_secs: u64,
    /// Number of pre-registered staging buffers for cold loads that can't get a
    /// memory-tier slot under pressure. Bounds concurrent cold-read parallelism.
    /// Default: 64. Set to 0 to disable staging (cold loads then fail on a full tier).
    pub cold_staging_slots: usize,
    /// Byte capacity of each cold-load staging buffer. Must be >= the largest
    /// per-block transfer size. Default: 4 MiB.
    pub cold_staging_buf_bytes: usize,
}

impl Default for DispatcherConfig {
    fn default() -> Self {
        Self {
            data_pci_addrs: Vec::new(),
            max_cache_entries: 10000,
            format_on_init: true,
            ssd_eviction_threshold: 0.9,
            ssd_eviction_low_watermark: 0.8,
            ssd_eviction_batch_size: 64,
            ssd_eviction_interval_secs: 5,
            poller_base_cpu: None,
            max_eviction_attempts: 2048,
            store_backpressure_ms: 5000,
            backfill_delay_ms: 10,
            metadata_partition_size: 128 * 1024 * 1024,
            extended_metadata_partition_size: 128 * 1024 * 1024,
            memory_tier_eviction_threshold: 0.0,
            memory_tier_eviction_low_watermark: 0.70,
            memory_tier_eviction_batch_size: 64,
            memory_tier_eviction_interval_secs: 2,
            cold_staging_slots: 64,
            cold_staging_buf_bytes: 4 * 1024 * 1024,
        }
    }
}

/// Opaque handle to client GPU memory for DMA transfers.
///
/// # Examples
///
/// ```
/// use interfaces::IpcHandle;
///
/// let mut buf = vec![0u8; 4096];
/// let handle = IpcHandle {
///     address: buf.as_mut_ptr(),
///     size: 4096,
/// };
/// assert_eq!(handle.size, 4096);
/// ```
#[derive(Debug, Clone, Copy)]
pub struct IpcHandle {
    /// GPU memory base address.
    pub address: *mut u8,
    /// Size of the data in bytes.
    pub size: u32,
}

// SAFETY: GPU memory is accessible cross-thread via DMA engine.
// The caller guarantees the pointer remains valid for the duration of the operation.
unsafe impl Send for IpcHandle {}

/// Errors returned by `IDispatcher` operations.
///
/// # Examples
///
/// ```
/// use interfaces::DispatcherError;
///
/// let err = DispatcherError::NotInitialized("dispatch_map not bound".into());
/// assert!(err.to_string().contains("not initialized"));
/// ```
#[derive(Debug, Clone)]
pub enum DispatcherError {
    /// Component not initialized or missing required receptacles.
    NotInitialized(String),
    /// The specified cache key was not found.
    KeyNotFound(CacheKey),
    /// A cache entry with this key already exists.
    AlreadyExists(CacheKey),
    /// DMA buffer allocation failed (out of memory).
    AllocationFailed(String),
    /// Block device or extent manager I/O error.
    IoError(String),
    /// A blocking operation exceeded the 100ms timeout.
    Timeout(String),
    /// Invalid parameter (e.g., zero-size IPC handle, empty config).
    InvalidParameter(String),
}

impl fmt::Display for DispatcherError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NotInitialized(msg) => write!(f, "not initialized: {msg}"),
            Self::KeyNotFound(k) => write!(f, "key not found: {k}"),
            Self::AlreadyExists(k) => write!(f, "key already exists: {k}"),
            Self::AllocationFailed(msg) => write!(f, "allocation failed: {msg}"),
            Self::IoError(msg) => write!(f, "I/O error: {msg}"),
            Self::Timeout(msg) => write!(f, "timeout: {msg}"),
            Self::InvalidParameter(msg) => write!(f, "invalid parameter: {msg}"),
        }
    }
}

impl std::error::Error for DispatcherError {}

/// Why an entry left the memory tier.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum EvictionReason {
    /// Demoted to the block tier; the value is still cached, on SSD.
    Demoted,
    /// Removed outright; the value is no longer cached anywhere locally.
    Removed,
}

/// One memory-tier eviction, delivered best-effort on the eviction channel.
///
/// **Defined here rather than in a dispatcher crate**, because it crosses a component
/// boundary: both `dispatcher` and `dispatcher-p2p` produce it and the shm-queue
/// transport host consumes it. It previously existed as two structurally identical
/// types, one per dispatcher, with the transport host hardcoding `dispatcher`'s --
/// which made the shm-queue server impossible to build against `dispatcher-p2p` at all,
/// since the generated stack correctly handed it the other crate's type. Both
/// dispatchers now re-export this one, so their public paths are unchanged.
#[derive(Clone, Debug)]
pub struct EvictionEvent {
    /// The evicted key.
    pub key: CacheKey,
    /// Whether the value survives on the block tier or is gone.
    pub reason: EvictionReason,
}

/// How a looked-up key was served — the **route**, not the entry's residency after
/// serving.
///
/// Exactly six values, and the value space is defined **here and only here**: any
/// other document or crate references this type rather than restating it, because a
/// taxonomy written down twice is a taxonomy that will disagree with itself.
///
/// Route, not residency, is the load-bearing distinction. In `dispatcher` an SSD hit
/// is promoted into DRAM as part of being served, so "served from SSD" and "now in
/// DRAM" are both true of one request; `Ssd` is the honest answer because it is what
/// the request cost.
///
/// # Examples
///
/// ```
/// use interfaces::ServedBy;
///
/// assert!(ServedBy::Dram.is_hit());
/// assert!(ServedBy::Remote.is_hit());
/// assert!(!ServedBy::Miss.is_hit());
/// ```
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ServedBy {
    /// Local memory tier (DRAM) hit. The block was already resident.
    Dram,
    /// A local data drive was read to serve this request — whether it was then
    /// promoted into DRAM (`dispatcher`) or delivered straight to the GPU with an
    /// asynchronous DRAM backfill (`dispatcher-p2p`).
    Ssd,
    /// A peer served it.
    ///
    /// Deliberately **not** subdivided by the peer's tier. A responder promotes
    /// disk-resident keys into its own memory tier before the RDMA read, so every
    /// remotely served byte leaves a peer's DRAM and a `RemoteSsd` value could only
    /// have meant "a peer's disk read was on this request's critical path" — an
    /// advertisement rather than an observation, decaying on repeat access, at 0.81%
    /// of hits. The peer's own disk work is counted on the peer instead, by
    /// `IRemoteLookup::serve_stats`.
    Remote,
    /// Not found in any tier, local or remote.
    Miss,
    /// Present, but at a different size than requested. Distinct from `Miss`: the key
    /// exists, so the caller's size model disagrees with what is stored rather than
    /// needing to populate from scratch.
    SizeMismatch,
    /// Attempted and failed for some other reason. Deliberately flat — it does not
    /// record which tier was attempted, because a failure's tier is not something a
    /// caller can act on.
    Error,
}

impl ServedBy {
    /// True for the three values in which data reached the caller's destination.
    ///
    /// A hit is reported if and only if the lookup succeeded, so a consumer can
    /// compute a hit rate as `hits / total` without knowing the error taxonomy.
    pub fn is_hit(&self) -> bool {
        matches!(self, Self::Dram | Self::Ssd | Self::Remote)
    }
}

/// The per-key outcome of a batched lookup: what happened, and where it came from.
///
/// `served_by` is meaningful on every path, including the failures — which is what
/// makes per-key attribution total.
///
/// **A struct, not `Result<ServedBy, DispatcherError>`.** The tier-on-`Ok` encoding
/// cannot express `Miss`, `SizeMismatch` or `Error`, because those *are* the `Err`
/// cases; it would push a third of the taxonomy into a per-server error-to-tier
/// mapping, which is precisely how two servers reporting the same cache would drift
/// apart.
///
/// # Examples
///
/// ```
/// use interfaces::{LookupOutcome, ServedBy};
///
/// let served = LookupOutcome { served_by: ServedBy::Dram, result: Ok(()) };
/// assert_eq!(served.served_by.is_hit(), served.result.is_ok());
/// ```
#[derive(Debug, Clone)]
pub struct LookupOutcome {
    /// How the key was served, or why it was not.
    pub served_by: ServedBy,
    /// The result the caller acts on. `is_ok()` if and only if `served_by.is_hit()`.
    pub result: Result<(), DispatcherError>,
}

/// A cumulative snapshot of the dispatcher's KV-cache tier-movement counters.
///
/// All fields are monotonic since process start; subtract two successive
/// snapshots to obtain a per-interval delta. Returned by
/// [`IDispatcher::tier_event_stats`].
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct TierEventStats {
    /// Blocks promoted SSD -> DRAM (memory tier), across all promote paths.
    pub promotions_to_memory: u64,
    /// Lookups served from the local memory tier (`ServedBy::Dram`).
    ///
    /// With `lookup_hits_ssd` and `remote_lookup_hits` this partitions every served
    /// key by route, so the three sum to the served count. They are counted where the
    /// dispatcher derives its attribution, so they cannot disagree with the per-key
    /// `served_by` it returns for the same batch.
    pub lookup_hits_dram: u64,
    /// Lookups served by reading a local data drive (`ServedBy::Ssd`).
    ///
    /// Route, not residency: in `dispatcher` the block is promoted into DRAM as part
    /// of being served, and this still counts as an SSD hit because that is what the
    /// request cost. In `dispatcher-p2p` there is no synchronous promotion (FR-014),
    /// so a repeat read of the same key may count here again.
    pub lookup_hits_ssd: u64,
    /// Lookups served up to the GPU (one per successfully-served lookup key),
    /// whether the source was the memory tier or SSD.
    pub promotions_to_gpu: u64,
    /// Memory-tier (DRAM) entries evicted — demoted to SSD or dropped — by
    /// either the foreground clean-eviction path or the background evictor.
    pub evictions_from_memory: u64,
    /// Extents freed on SSD by the background extent evictor.
    pub evictions_from_ssd: u64,
    /// Lookups a **peer** served, one per key the dispatcher obtained through
    /// `IRemoteLookup` rather than from its own tiers.
    ///
    /// A `RemoteLookupError::TransportError` counts as **neither** a hit nor a miss:
    /// the question "did a peer hold this key" was never answered, so counting it
    /// either way would invent data. Consequently
    /// `remote_lookup_hits + remote_lookup_misses` is the number of keys that got an
    /// *answer*, not the number forwarded.
    ///
    /// A hit is recorded when the peer returns data, not after local delivery
    /// succeeds — a delivery fault on this node is not evidence about what the peer
    /// held.
    ///
    /// Requester-side: what this node got *from* peers, not what peers asked *of*
    /// it. Those are different quantities and must not share a name.
    ///
    /// This exists because `promotions_to_gpu` counts every served key without
    /// distinguishing the source, so remote lookup's *benefit* was unmeasurable
    /// while its cost was not — two cluster measurements established the cost and
    /// neither could establish the benefit.
    pub remote_lookup_hits: u64,
    /// Keys forwarded to a peer that no peer held. Absent everywhere, so a miss.
    ///
    /// With `remote_lookup_hits` this gives the remote path's own hit rate, which
    /// is the number that says whether forwarding earns what it costs.
    pub remote_lookup_misses: u64,
    /// Store-allocation retries taken while backpressuring on a momentarily
    /// full memory tier (see `DispatcherConfig::store_backpressure_ms`). A
    /// nonzero value means the tier saturated and the store path waited for the
    /// evictor to catch up rather than failing the store.
    pub store_backpressure_events: u64,
    /// Stores dropped best-effort because the memory tier was still full after
    /// the backpressure budget elapsed. The KV block is not cached (a later
    /// load misses and recomputes), but the store reports success so a full
    /// cache is never fatal to the vLLM offloading connector. A nonzero value
    /// means the tier could not absorb the store inflow even after
    /// backpressuring — consider a larger tier or more SSD write bandwidth.
    pub store_drops_on_full: u64,
    /// Eviction candidates skipped because a read pin was held on them —
    /// `IDispatchMap::try_evict_to_block` returned `ActiveReferences`.
    ///
    /// Counted per candidate examined, so one call that scans past several pinned
    /// entries bumps this several times.
    ///
    /// This exists to separate the two reasons a clean eviction can fail, which look
    /// identical from the call site and mean opposite things. A candidate may be
    /// undemotable because write-through has not landed yet — expected under write
    /// load, and self-correcting — or because something is holding a read pin, which
    /// is the only one that implicates a reader. Counting "eviction failed" without
    /// that distinction produces a number that cannot support either conclusion, so
    /// the error variant is matched rather than `is_ok()` tested.
    pub evictions_blocked_by_pin: u64,
    /// Times a clean-eviction scan examined every candidate it was given and could
    /// free none, so the caller had to surface pool-full.
    ///
    /// This is the event that becomes a declined store: `reserve_memory` retries
    /// against its `--store-backpressure-ms` budget, each retry scanning again, and
    /// when the budget elapses the store is dropped (`store_drops_on_full`).
    pub eviction_scans_exhausted: u64,
    /// Eviction candidates skipped because write-through had not landed, so there was
    /// no `ssd_offset` to demote to. Counted per candidate examined.
    ///
    /// This is the *other* reason a clean eviction fails, and it involves no reader and
    /// no peer — `reserve_memory`'s own comment names both: the oldest entries are
    /// transiently un-evictable because they are "pinned by an in-flight load, or not
    /// yet written through by the bg_writer".
    ///
    /// It is counted rather than inferred by subtraction from
    /// `evictions_blocked_by_pin`, because subtraction would silently absorb every
    /// third cause and any future variant, and would attribute them to whichever of
    /// the two was not measured. With both counted, `evictions_blocked_by_pin +
    /// evictions_blocked_unpersisted` against the scanned total is itself a check that
    /// the two explanations are exhaustive.
    pub evictions_blocked_unpersisted: u64,
    /// How many of the oldest memory-tier keys were sampled when this snapshot was
    /// taken, and how many of those were demotable. **Gauges, not counters.**
    ///
    /// Sampled over `oldest_keys`, deliberately, because that is the window
    /// `evict_one_clean` actually scans. A tier-wide persisted fraction would be the
    /// wrong number: an entry that cannot be evicted ages to the oldest end of the LRU
    /// and stays there, so the oldest keys are *where stuck entries concentrate*, and a
    /// whole-tier average would dilute exactly the effect being measured.
    ///
    /// `oldest_persisted == 0` with `oldest_sampled > 0` means every candidate eviction
    /// would be refused for want of an `ssd_offset` — the store path is about to
    /// backpressure and decline no matter how much tier capacity exists.
    pub oldest_sampled: u64,
    /// Of `oldest_sampled`, how many had an `ssd_offset` and so could be demoted.
    pub oldest_persisted: u64,
}

#[cfg(feature = "spdk")]
component_macros::define_interface! {
    pub IDispatcher {
        /// Initialize the dispatcher with the given configuration.
        ///
        /// Creates and initializes N data block devices and N extent managers
        /// based on the provided PCI addresses. The metadata block device
        /// uses namespace partitions for extent manager metadata.
        ///
        /// # Errors
        ///
        /// Returns [`DispatcherError::NotInitialized`] if required receptacles
        /// (dispatch_map, memory_tier) are not bound.
        /// Returns [`DispatcherError::InvalidParameter`] if `data_pci_addrs` is empty.
        ///
        /// # Examples
        ///
        /// ```no_run
        /// # use std::sync::Arc;
        /// # use interfaces::{IDispatcher, DispatcherConfig, DispatcherError};
        /// # fn example(dispatcher: &dyn IDispatcher) -> Result<(), DispatcherError> {
        /// let config = DispatcherConfig {
        ///     data_pci_addrs: vec![
        ///         "0000:02:00.0".to_string(),
        ///         "0000:03:00.0".to_string(),
        ///     ],
        ///     max_cache_entries: 50000,
        ///     ssd_eviction_threshold: 0.9,
        ///     ..Default::default()
        /// };
        /// dispatcher.initialize(config)?;
        /// # Ok(())
        /// # }
        /// ```
        fn initialize(&self, config: DispatcherConfig) -> Result<(), DispatcherError>;

        /// Shut down the dispatcher, completing all in-flight background writes.
        ///
        /// Blocks until all pending memory-tier-to-SSD writes finish, then shuts down
        /// all managed block devices and extent managers.
        ///
        /// # Examples
        ///
        /// ```no_run
        /// # use interfaces::{IDispatcher, DispatcherConfig, DispatcherError};
        /// # fn example(dispatcher: &dyn IDispatcher) -> Result<(), DispatcherError> {
        /// // Shutdown completes all pending background writes before returning.
        /// dispatcher.shutdown()?;
        /// // After shutdown, all operations except shutdown will return NotInitialized.
        /// # Ok(())
        /// # }
        /// ```
        fn shutdown(&self) -> Result<(), DispatcherError>;

        /// Look up a cache entry and DMA-copy data to the client's GPU memory.
        ///
        /// If the entry is in the memory-tier, copies from the DRAM buffer.
        /// If the entry is on SSD, reads from the block device and copies.
        /// Blocks if a writer is active on the key (dispatch map semantics).
        ///
        /// # Errors
        ///
        /// Returns [`DispatcherError::KeyNotFound`] if the key does not exist.
        /// Returns [`DispatcherError::NotInitialized`] if called before [`initialize`].
        ///
        /// # Examples
        ///
        /// ```no_run
        /// # use interfaces::{IDispatcher, IpcHandle, DispatcherError, CacheKey};
        /// # fn example(dispatcher: &dyn IDispatcher) -> Result<(), DispatcherError> {
        /// let key: CacheKey = 42;
        /// let mut gpu_buffer = vec![0u8; 4096];
        /// let handle = IpcHandle {
        ///     address: gpu_buffer.as_mut_ptr(),
        ///     size: 4096,
        /// };
        /// dispatcher.lookup(key, handle)?;
        /// // gpu_buffer now contains the cached data.
        /// # Ok(())
        /// # }
        /// ```
        fn lookup(&self, key: CacheKey, ipc_handle: IpcHandle) -> Result<(), DispatcherError>;

        /// Async variant of [`lookup`] — issues the H2D DMA copy without blocking.
        ///
        /// Returns the CUDA stream the copy was issued on. The caller must call
        /// `stream_synchronize` on the returned stream before accessing the GPU
        /// destination memory. For SSD-backed entries the copy completes
        /// synchronously and a null stream is returned.
        ///
        /// # Errors
        ///
        /// Returns [`DispatcherError::KeyNotFound`] if the key does not exist.
        /// Returns [`DispatcherError::IoError`] if the DMA copy fails.
        ///
        /// # Examples
        ///
        /// ```no_run
        /// # use interfaces::{IDispatcher, IGpuServices, IpcHandle, DispatcherError, CacheKey, GpuStream};
        /// # fn example(
        /// #     dispatcher: &dyn IDispatcher,
        /// #     gpu: &dyn IGpuServices,
        /// # ) -> Result<(), DispatcherError> {
        /// let key: CacheKey = 42;
        /// let mut gpu_buffer = vec![0u8; 4096];
        /// let handle = IpcHandle {
        ///     address: gpu_buffer.as_mut_ptr(),
        ///     size: 4096,
        /// };
        /// let stream = dispatcher.lookup_async(key, handle)?;
        /// // Must synchronize before accessing the destination buffer.
        /// if !stream.0.is_null() {
        ///     gpu.stream_synchronize(stream)
        ///         .map_err(|e| DispatcherError::IoError(e))?;
        /// }
        /// # Ok(())
        /// # }
        /// ```
        fn lookup_async(&self, key: CacheKey, ipc_handle: IpcHandle) -> Result<GpuStream, DispatcherError>;

        /// Batch lookup: retrieve multiple cache entries concurrently.
        ///
        /// For entries in the memory-tier, behaves like sequential
        /// lookups. For entries on SSD (cold path), promotes them in parallel
        /// to exploit multi-drive bandwidth.
        ///
        /// Returns one `Result` per input entry, in the same order.
        ///
        /// Each entry carries one or more GPU destination regions. A block that
        /// the client exports as a single coalesced allocation (vLLM <=0.22,
        /// `populate`) has exactly one region; a block split into N per-layer
        /// allocations (vLLM 0.23+) has N. The server scatters the one resident
        /// DRAM slot back to the N regions in order (region L <- slot + sum of
        /// preceding region sizes), so index and storage stay 1:1 per key.
        fn batch_lookup(
            &self,
            entries: &[(CacheKey, Vec<IpcHandle>)],
        ) -> Vec<LookupOutcome>;

        /// Check whether a cache entry exists without transferring data.
        ///
        /// Returns `true` if the key is present in the cache (any tier),
        /// `false` otherwise.
        ///
        /// # Examples
        ///
        /// ```no_run
        /// # use interfaces::{IDispatcher, DispatcherError, CacheKey};
        /// # fn example(dispatcher: &dyn IDispatcher) -> Result<(), DispatcherError> {
        /// let key: CacheKey = 42;
        /// if dispatcher.check(key)? {
        ///     println!("key {key} is cached");
        /// } else {
        ///     println!("key {key} not found, need to populate");
        /// }
        /// # Ok(())
        /// # }
        /// ```
        fn check(&self, key: CacheKey) -> Result<bool, DispatcherError>;

        /// Remove a cache entry, freeing all associated resources.
        ///
        /// Does NOT block waiting for background write-through to complete.
        /// Proceeds immediately with removal. Frees memory-tier slot and/or SSD extent.
        ///
        /// # Errors
        ///
        /// Returns [`DispatcherError::KeyNotFound`] if the key does not exist.
        ///
        /// # Examples
        ///
        /// ```no_run
        /// # use interfaces::{IDispatcher, DispatcherError, CacheKey};
        /// # fn example(dispatcher: &dyn IDispatcher) -> Result<(), DispatcherError> {
        /// let key: CacheKey = 42;
        /// dispatcher.remove(key)?;
        /// // Entry is gone — check confirms it.
        /// assert!(!dispatcher.check(key)?);
        /// # Ok(())
        /// # }
        /// ```
        fn remove(&self, key: CacheKey) -> Result<(), DispatcherError>;

        /// Populate a new cache entry by DMA-copying from GPU memory.
        ///
        /// Allocates a memory-tier slot, copies data from the GPU via DMA,
        /// and returns immediately. The memory-tier-to-SSD write happens
        /// asynchronously in the background.
        ///
        /// # Errors
        ///
        /// Returns [`DispatcherError::InvalidParameter`] if `ipc_handle.size` is 0.
        /// Returns [`DispatcherError::AlreadyExists`] if the key is already cached.
        /// Returns [`DispatcherError::AllocationFailed`] if the memory-tier pool is full.
        ///
        /// # Examples
        ///
        /// ```no_run
        /// # use interfaces::{IDispatcher, IpcHandle, DispatcherError, CacheKey};
        /// # fn example(dispatcher: &dyn IDispatcher) -> Result<(), DispatcherError> {
        /// let key: CacheKey = 100;
        /// let mut gpu_data = vec![0xABu8; 8192];
        /// let handle = IpcHandle {
        ///     address: gpu_data.as_mut_ptr(),
        ///     size: 8192,
        /// };
        /// // Copies GPU data into the cache; SSD write-through happens in background.
        /// dispatcher.populate(key, handle)?;
        /// assert!(dispatcher.check(key)?);
        /// # Ok(())
        /// # }
        /// ```
        fn populate(&self, key: CacheKey, ipc_handle: IpcHandle) -> Result<(), DispatcherError>;

        /// Batch-populate multiple cache entries from GPU memory.
        ///
        /// Semantically equivalent to calling [`populate`] for each entry, but
        /// amortizes the GPU stream synchronization across the entire batch:
        /// all D2H copies are submitted asynchronously, a single
        /// `stream_synchronize` waits for the batch, then all entries are
        /// registered in the dispatch-map and enqueued for SSD write-through.
        ///
        /// Returns one `Result` per entry, in the same order as `entries`.
        fn batch_populate(&self, entries: &[(CacheKey, IpcHandle)]) -> Vec<Result<(), DispatcherError>>;

        /// Reserve a memory-tier slot for the given key, evicting if necessary.
        ///
        /// Allocates `size` bytes in DRAM keyed by `key`. The returned pointer
        /// is valid until `release_memory` or `copy_gpu_to_memory_completed` is called.
        /// Does NOT register in the dispatch-map or issue any DMA.
        ///
        /// # Errors
        ///
        /// Returns [`DispatcherError::AlreadyExists`] if the key already has a slot.
        /// Returns [`DispatcherError::AllocationFailed`] if the pool is full after eviction.
        ///
        /// `session_id` is an opaque per-request identifier (0 = unset) supplied
        /// by the client (e.g. a hash of vLLM's `session_id`). It carries no
        /// allocation semantics today and is used only for observability.
        ///
        /// `deadline` bounds how long a full-tier allocation may backpressure
        /// (retry LRU eviction) before surfacing [`DispatcherError::AllocationFailed`]:
        /// - `None` — use the dispatcher's own `store_backpressure_ms` budget,
        ///   measured from the start of *this* call (legacy per-call behavior).
        /// - `Some(instant)` — retry only until `instant`, a deadline the caller
        ///   shares across a batch of reserves. Once the instant has passed, make
        ///   a single best-effort attempt and fail fast. This lets a multi-key
        ///   reserve batch bound its *total* backpressure (rather than paying the
        ///   full budget per key) so the client's request deadline is not exceeded;
        ///   keys that fail fast are refused caching and recomputed by the client.
        fn reserve_memory(&self, key: CacheKey, size: u32, session_id: u64, deadline: Option<std::time::Instant>) -> Result<*mut u8, DispatcherError>;

        /// DMA-copy from GPU into a previously reserved memory-tier slot.
        ///
        /// The slot must have been allocated by a prior `reserve_memory` call.
        /// Issues one `cudaMemcpyAsync` per region on the given stream and returns
        /// immediately. The N regions are gathered contiguously into the one slot
        /// (region L lands at `slot + sum of preceding region sizes`), so a block
        /// split into N per-layer GPU allocations (vLLM 0.23+) is stored as one
        /// colocated unit; a single-region block (`regions.len() == 1`) is the
        /// legacy path. Caller must synchronize the stream before calling
        /// `copy_gpu_to_memory_completed`.
        ///
        /// # Errors
        ///
        /// Returns [`DispatcherError::KeyNotFound`] if no reserved slot exists for `key`.
        /// Returns [`DispatcherError::IoError`] if the DMA copy fails.
        fn copy_gpu_to_memory_async(&self, key: CacheKey, regions: &[IpcHandle], stream: GpuStream) -> Result<(), DispatcherError>;

        /// Finalize a populated memory-tier slot: register in the dispatch-map
        /// and enqueue background write-through to SSD.
        ///
        /// Must be called after `copy_gpu_to_memory_async` completes successfully.
        ///
        /// # Errors
        ///
        /// Returns [`DispatcherError::KeyNotFound`] if the key is not in memory-tier.
        fn copy_gpu_to_memory_completed(&self, key: CacheKey, size: u32) -> Result<(), DispatcherError>;

        /// Release a reserved memory-tier slot without populating it.
        ///
        /// Used on the cancellation path (e.g., `complete_store(success=false)`).
        /// Idempotent — returns Ok if the key has no slot.
        fn release_memory(&self, key: CacheKey) -> Result<(), DispatcherError>;

        /// Acquire an eviction-protection read reference on a cache entry.
        ///
        /// While pinned, the entry cannot be evicted by the LRU policy.
        /// Each `pin` call must be balanced by a corresponding `unpin`.
        /// Multiple pins on the same key stack (ref-count increments).
        ///
        /// # Errors
        ///
        /// Returns [`DispatcherError::KeyNotFound`] if the key does not exist.
        /// Returns [`DispatcherError::Timeout`] if a writer holds exclusive access.
        fn pin(&self, key: CacheKey) -> Result<(), DispatcherError>;

        /// Release an eviction-protection read reference on a cache entry.
        ///
        /// Decrements the read ref-count. When all pins are released, the
        /// entry becomes eligible for eviction again.
        ///
        /// # Errors
        ///
        /// Returns [`DispatcherError::KeyNotFound`] if the key does not exist
        /// or was already fully unpinned (ref-count underflow).
        fn unpin(&self, key: CacheKey) -> Result<(), DispatcherError>;

        /// Update the timestamp for a cache entry without performing any DMA.
        ///
        /// Used to refresh the eviction timestamp in the dispatch map,
        /// preventing the entry from being evicted without transferring data.
        ///
        /// # Errors
        ///
        /// Returns [`DispatcherError::KeyNotFound`] if the key does not exist.
        ///
        /// # Examples
        ///
        /// ```no_run
        /// # use interfaces::{IDispatcher, DispatcherError, CacheKey};
        /// # fn example(dispatcher: &dyn IDispatcher) -> Result<(), DispatcherError> {
        /// let key: CacheKey = 42;
        /// // Refresh the eviction timestamp to keep the entry hot.
        /// dispatcher.touch(key)?;
        /// # Ok(())
        /// # }
        /// ```
        fn touch(&self, key: CacheKey) -> Result<(), DispatcherError>;

        /// Promote SSD-resident entries to the memory-tier without GPU DMA.
        ///
        /// For each key in `BlockDevice` state, reads data from SSD into a new
        /// memory-tier slot and updates the dispatch-map. Keys in `MemoryTier`
        /// or `Staging` state get a timestamp refresh. Missing keys are skipped.
        ///
        /// This is a best-effort, fire-and-forget operation intended to be called
        /// from a background task. Errors on individual keys are logged but not
        /// propagated.
        fn promote_to_memory_tier(&self, keys: &[CacheKey]);

        /// Evict all entries from the memory-tier, demoting them to block-device-backed.
        ///
        /// Entries whose write-through has completed are converted to block-device
        /// state in the dispatch map. Entries still being written are removed entirely.
        /// Returns the number of entries cleared.
        ///
        /// # Examples
        ///
        /// ```no_run
        /// # use interfaces::{IDispatcher, DispatcherError};
        /// # fn example(dispatcher: &dyn IDispatcher) -> Result<(), DispatcherError> {
        /// let cleared = dispatcher.clear_memory_tier()?;
        /// println!("cleared {cleared} entries from memory-tier");
        /// # Ok(())
        /// # }
        /// ```
        fn clear_memory_tier(&self) -> Result<usize, DispatcherError>;

        /// Flush all pending background write-through jobs to SSD and block until complete.
        ///
        /// Guarantees that every entry populated before this call has its data
        /// persisted to the block device. After this returns, `clear_memory_tier`
        /// will convert all memory-tier entries to BlockDevice state rather than
        /// dropping them.
        ///
        /// Returns the number of entries that now have a valid SSD offset.
        fn flush_to_ssd(&self) -> Result<usize, DispatcherError>;

        /// Return cumulative per-direction SSD read/write byte, op, and latency
        /// counters aggregated across all data drives. Returns zeroed counters
        /// unless the dispatcher (and its block devices) were built with the
        /// `rw-telemetry` / `telemetry` features. Monotonic; take deltas across
        /// two calls to measure a window.
        fn read_write_stats(&self) -> crate::iblock_device::ReadWriteStats;

        /// Return the cumulative KV-cache tier-movement counters: blocks promoted
        /// SSD -> DRAM and lookups served up to the GPU, plus evictions from the
        /// DRAM memory tier and from SSD. Always populated (the counters are
        /// unconditional, unlike the telemetry-gated `read_write_stats`).
        /// Monotonic since process start; take deltas across two calls to measure
        /// a window. Implementations that do no tiering return zeroed counters.
        fn tier_event_stats(&self) -> crate::TierEventStats;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dispatcher_error_display() {
        let err = DispatcherError::NotInitialized("test".into());
        assert!(err.to_string().contains("not initialized"));
    }

    #[test]
    fn dispatcher_error_key_not_found() {
        let err = DispatcherError::KeyNotFound(42);
        assert!(err.to_string().contains("42"));
    }

    #[test]
    fn dispatcher_error_already_exists() {
        let err = DispatcherError::AlreadyExists(7);
        assert!(err.to_string().contains("already exists"));
    }

    #[test]
    fn dispatcher_error_io() {
        let err = DispatcherError::IoError("disk failure".into());
        assert!(err.to_string().contains("I/O error"));
    }

    #[test]
    fn dispatcher_error_timeout() {
        let err = DispatcherError::Timeout("100ms exceeded".into());
        assert!(err.to_string().contains("timeout"));
    }

    #[test]
    fn dispatcher_error_invalid_parameter() {
        let err = DispatcherError::InvalidParameter("zero size".into());
        assert!(err.to_string().contains("invalid parameter"));
    }

    #[test]
    fn dispatcher_error_allocation_failed() {
        let err = DispatcherError::AllocationFailed("out of DMA memory".into());
        assert!(err.to_string().contains("allocation failed"));
    }

    #[test]
    fn dispatcher_config_clone() {
        let config = DispatcherConfig {
            data_pci_addrs: vec!["0000:02:00.0".to_string()],
            ..Default::default()
        };
        let config2 = config.clone();
        assert_eq!(config2.data_pci_addrs.len(), 1);
    }

    #[test]
    fn ipc_handle_creation() {
        let mut buf = vec![0u8; 4096];
        let handle = IpcHandle {
            address: buf.as_mut_ptr(),
            size: 4096,
        };
        assert_eq!(handle.size, 4096);
    }
}
