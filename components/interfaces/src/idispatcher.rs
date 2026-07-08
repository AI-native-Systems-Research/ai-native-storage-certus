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
    /// When `None`, each drive's actor falls back to the first available CPU
    /// in its NUMA node (all drives on the same node would share that core).
    /// Set this to a dedicated core range to give each drive exclusive use of
    /// a core, which is required for SPDK busy-polling to achieve full bandwidth.
    pub poller_base_cpu: Option<usize>,
    /// Maximum eviction attempts before returning AllocationFailed.
    /// Default: 2048.
    pub max_eviction_attempts: usize,
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
            backfill_delay_ms: 10,
            metadata_partition_size: 128 * 1024 * 1024,
            extended_metadata_partition_size: 128 * 1024 * 1024,
        }
    }
}

/// Cumulative cache-level hit/miss counters for the load (lookup) path.
///
/// Every load classifies each requested key against the dispatch-map and is
/// tallied here by where it resolved:
/// - `mem_tier_hits` — served from the DRAM memory tier (no SSD read).
/// - `ssd_hits` — resolved to a block-device entry, requiring an NVMe read to
///   promote the block back into DRAM before serving it.
/// - `misses` — the key was not present in the cache at all (`NotExist`).
///
/// `mem_tier_evictions` covers the write side rather than the load side: it counts
/// blocks demoted out of the DRAM memory tier to the block device under
/// capacity pressure (LRU eviction while making room for a new entry).
///
/// Counters are monotonic for the life of the dispatcher; take deltas across
/// two calls to measure a window (e.g. one benchmark round). Comparing
/// `mem_tier_hits` against `ssd_hits` shows what fraction of the working set is
/// being served from DRAM versus SSD.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct CacheStats {
    /// Load blocks served from the DRAM memory tier (no SSD read).
    pub mem_tier_hits: u64,
    /// Load blocks resolved from the NVMe block device (each implies an SSD read).
    pub ssd_hits: u64,
    /// Load blocks not present in the cache (`LookupResult::NotExist`).
    pub misses: u64,
    /// Blocks evicted from the DRAM memory tier to the block device under
    /// capacity pressure (demotions made while reserving space for new entries).
    pub mem_tier_evictions: u64,
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
#[derive(Debug)]
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

// # Verified Properties (see `components/dispatcher/verif/`)
//
// The following invariants are formally proved with Creusot:
//
// - P1 (drive-index-bounded): drive_index(key, N) always returns a value < N
// - P2 (eviction-terminates): evict_for_space loop exits after at most max_attempts iterations
// - P3 (size-validation): populate rejects size == 0
// - P4 (init-guard): all operations return NotInitialized before initialize() succeeds
// - P5 (populate-lifecycle): successful populate yields MemoryTier entry with read_ref=1, no write_ref
// - P6 (drive-index-deterministic): same key always maps to same drive
// - P7 (eviction-progress): each successful eviction strictly decreases memory used
// - P8 (reserve-complete-lifecycle): reserve→copy→complete yields MemoryTier entry with read_ref=1
//
// Total: 10 properties, 24 verification conditions discharged by SMT solvers.

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
        /// # Verified: P3 (size-validation), P4 (init-guard)
        /// Rejects empty `data_pci_addrs` (InvalidParameter). After success,
        /// sets initialized=true enabling all other operations.
        ///
        /// # Unchecked: Concurrent initialization safety
        /// Two threads calling initialize() simultaneously could race on the
        /// AtomicBool store. Suggested technique: Spin model or Loom testing.
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
        /// # Verified: P4 (init-guard)
        /// After shutdown completes, initialized=false and all subsequent
        /// operations return NotInitialized.
        ///
        /// # Unchecked: Background writer drain completeness
        /// Claims all pending writes complete before returning. Cannot be
        /// modeled sequentially. Suggested technique: Spin model or integration test.
        ///
        /// # Unchecked: Two-phase block device shutdown ordering
        /// Signal-all-then-join ordering prevents use-after-free on SPDK
        /// transport memory. Suggested technique: Loom concurrency testing.
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
        /// # Verified: P1 (drive-index-bounded), P4 (init-guard)
        /// Drive selection for cold-path reads is always within [0, num_drives).
        /// Returns NotInitialized if called before initialize().
        ///
        /// # Unchecked: Blocks until writer completes
        /// Doc claims lookup blocks if a writer is active. This is a concurrency
        /// property delegated to dispatch-map's reference protocol.
        /// Suggested technique: Spin model of reader/writer interaction.
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
        /// # Verified: P1 (drive-index-bounded), P4 (init-guard)
        /// Cold-path drive selection bounded. Rejects uninitialized.
        ///
        /// # Unchecked: Caller must synchronize returned stream before memory access
        /// Caller protocol — if violated, GPU destination contains partial data.
        /// Suggested technique: debug assertion in wrapper layer.
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
        /// # Verified: P1 (drive-index-bounded), P2 (eviction-terminates), P4 (init-guard)
        /// Cold-path drive selection is bounded. Eviction during promotion
        /// terminates. Rejects calls before initialization.
        ///
        /// # Unchecked: Result ordering matches input ordering
        /// The implementation uses thread::scope with per-drive parallelism;
        /// results are assembled by index. Suggested technique: property-based testing.
        fn batch_lookup(
            &self,
            entries: &[(CacheKey, IpcHandle)],
        ) -> Vec<Result<(), DispatcherError>>;

        /// Check whether a cache entry exists without transferring data.
        ///
        /// Returns `true` if the key is present in the cache (any tier),
        /// `false` otherwise.
        ///
        /// # Verified: P4 (init-guard)
        /// Returns NotInitialized before initialize().
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
        /// If a background write is in progress, blocks until it completes
        /// before removing. Frees memory-tier slot and/or SSD extent.
        ///
        /// # Errors
        ///
        /// Returns [`DispatcherError::KeyNotFound`] if the key does not exist.
        ///
        /// # Verified: P1 (drive-index-bounded), P4 (init-guard)
        /// Drive index for extent removal is bounded. Rejects uninitialized.
        ///
        /// # Unchecked: Blocks until background write completes
        /// Doc claims remove blocks if a writer is active. This depends on
        /// dispatch-map reference semantics (lookup acquires read ref).
        /// Suggested technique: Spin model or Loom test.
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
        /// # Verified: P3 (size-validation), P4 (init-guard), P5 (populate-lifecycle), P2 (eviction-terminates)
        /// Rejects zero-size. Rejects uninitialized. On success, entry is
        /// registered in MemoryTier with read_ref=1 (held by background writer)
        /// and no write_ref. Eviction terminates within max_attempts.
        ///
        /// # Unchecked: Background write-through eventually persists to SSD
        /// The enqueued write job executes asynchronously; completion is not
        /// guaranteed before `flush_to_ssd` is called.
        /// Suggested technique: integration test with flush barrier.
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
        /// # Verified: P2 (eviction-terminates), P3 (size-validation), P4 (init-guard), P10 (reserve-complete-lifecycle)
        /// Eviction terminates. Rejects zero-size. Rejects uninitialized.
        /// Part of the reserve→copy→complete lifecycle.
        ///
        /// # Unchecked: Returned pointer validity
        /// The pointer is valid pinned DRAM co-registered with CUDA and SPDK.
        /// Correctness depends on memory-tier pool lifetime.
        /// Suggested technique: Miri or ASAN integration test.
        fn reserve_memory(&self, key: CacheKey, size: u32) -> Result<*mut u8, DispatcherError>;

        /// DMA-copy from GPU into a previously reserved memory-tier slot.
        ///
        /// The slot must have been allocated by a prior `reserve_memory` call.
        /// Issues `cudaMemcpyAsync` on the given stream and returns immediately.
        /// Caller must synchronize the stream before calling `copy_gpu_to_memory_completed`.
        ///
        /// # Errors
        ///
        /// Returns [`DispatcherError::KeyNotFound`] if no reserved slot exists for `key`.
        /// Returns [`DispatcherError::IoError`] if the DMA copy fails.
        ///
        /// # Verified: P4 (init-guard), P10 (reserve-complete-lifecycle)
        /// Rejects uninitialized. Part of the reserve→copy→complete lifecycle.
        ///
        /// # Unchecked: Stream must be synchronized before copy_gpu_to_memory_completed
        /// Caller protocol — no in-process enforcement. If violated, GPU DMA
        /// may not have completed and memory-tier slot contains partial data.
        /// Suggested technique: debug-mode runtime assertion via stream query.
        fn copy_gpu_to_memory_async(&self, key: CacheKey, ipc_handle: IpcHandle, stream: GpuStream) -> Result<(), DispatcherError>;

        /// Finalize a populated memory-tier slot: register in the dispatch-map
        /// and enqueue background write-through to SSD.
        ///
        /// Must be called after `copy_gpu_to_memory_async` completes successfully.
        ///
        /// # Errors
        ///
        /// Returns [`DispatcherError::KeyNotFound`] if the key is not in memory-tier.
        ///
        /// # Verified: P4 (init-guard), P5 (populate-lifecycle), P10 (reserve-complete-lifecycle)
        /// Rejects uninitialized. Produces entry with read_ref=1 (for background
        /// writer) via downgrade_reference. Enqueues write job.
        fn copy_gpu_to_memory_completed(&self, key: CacheKey, size: u32) -> Result<(), DispatcherError>;

        /// Release a reserved memory-tier slot without populating it.
        ///
        /// Used on the cancellation path (e.g., `complete_store(success=false)`).
        /// Idempotent — returns Ok if the key has no slot.
        ///
        /// # Verified: P4 (init-guard)
        /// Rejects uninitialized.
        fn release_memory(&self, key: CacheKey) -> Result<(), DispatcherError>;

        /// Update the timestamp for a cache entry without performing any DMA.
        ///
        /// Used to refresh the eviction timestamp in the dispatch map,
        /// preventing the entry from being evicted without transferring data.
        ///
        /// # Errors
        ///
        /// Returns [`DispatcherError::KeyNotFound`] if the key does not exist.
        ///
        /// # Verified: P4 (init-guard)
        /// Rejects uninitialized.
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
        ///
        /// # Verified: P1 (drive-index-bounded), P2 (eviction-terminates), P4 (init-guard)
        /// Drive selection bounded. Eviction terminates. Rejects uninitialized.
        ///
        /// # Unchecked: Per-drive parallelism correctness
        /// Multiple threads read from the same physical drive concurrently.
        /// Correctness depends on SPDK queue-pair isolation.
        /// Suggested technique: stress test with concurrent promote + lookup.
        fn promote_to_memory_tier(&self, keys: &[CacheKey]);

        /// Evict all entries from the memory-tier, demoting them to block-device-backed.
        ///
        /// Entries whose write-through has completed are converted to block-device
        /// state in the dispatch map. Entries still being written are removed entirely.
        /// Returns the number of entries cleared.
        ///
        /// # Verified: P4 (init-guard), P9 (eviction-progress)
        /// Rejects uninitialized. Each eviction decreases memory used.
        ///
        /// # Unchecked: Entries still being written are removed without data loss
        /// Entries without ssd_offset are fully removed (data lost). This is
        /// intentional but callers must call flush_to_ssd first to avoid loss.
        /// Suggested technique: integration test verifying flush→clear sequence.
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
        ///
        /// # Verified: P4 (init-guard)
        /// Rejects uninitialized.
        ///
        /// # Unchecked: All populated entries persisted after return
        /// Guarantees total persistence of all entries populated before the call.
        /// This is a liveness property on the background writer channel drain.
        /// Suggested technique: integration test with populate→flush→verify sequence.
        fn flush_to_ssd(&self) -> Result<usize, DispatcherError>;

        /// Return cumulative SSD read/write byte and op counters, aggregated
        /// across all data drives. Counters are monotonic for the life of the
        /// dispatcher; take deltas across two calls to measure a window. Values
        /// are all-zero unless the underlying block devices were built with
        /// their `telemetry` feature enabled.
        fn read_write_stats(&self) -> crate::iblock_device::ReadWriteStats;

        /// Return cumulative cache-level hit/miss counters for the load path,
        /// broken down by the tier that served each requested block (DRAM
        /// memory tier vs NVMe block device) plus outright misses. Counters are
        /// monotonic for the life of the dispatcher; take deltas across two
        /// calls to measure a window.
        fn cache_stats(&self) -> CacheStats;
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
