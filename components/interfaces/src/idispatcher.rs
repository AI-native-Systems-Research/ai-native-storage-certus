//! IDispatcher interface and associated types for the dispatcher component.

use std::fmt;
#[cfg(feature = "spdk")]
use std::sync::Arc;

use crate::idispatch_map::CacheKey;
#[cfg(feature = "spdk")]
use crate::igpu_services::GpuStream;
#[cfg(feature = "spdk")]
use crate::spdk_types::DmaBuffer;

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
    /// Eviction threshold (unused, retained for config compatibility). Default: 0.8.
    pub eviction_threshold: f64,
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
}

impl Default for DispatcherConfig {
    fn default() -> Self {
        Self {
            data_pci_addrs: Vec::new(),
            max_cache_entries: 10000,
            eviction_threshold: 0.8,
            format_on_init: true,
            ssd_eviction_threshold: 0.9,
            ssd_eviction_low_watermark: 0.8,
            ssd_eviction_batch_size: 64,
            ssd_eviction_interval_secs: 5,
            poller_base_cpu: None,
            max_eviction_attempts: 2048,
            backfill_delay_ms: 10,
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
        /// Blocks until all pending staging-to-SSD writes finish, then shuts down
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
        /// If the entry is in staging, copies from the staging buffer.
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
        /// destination memory. For non-memory-tier paths (staging, SSD) the copy
        /// completes synchronously and a null stream is returned.
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
        /// For entries in the memory-tier or staging, behaves like sequential
        /// lookups. For entries on SSD (cold path), promotes them in parallel
        /// to exploit multi-drive bandwidth.
        ///
        /// Returns one `Result` per input entry, in the same order.
        fn batch_lookup(
            &self,
            entries: &[(CacheKey, IpcHandle)],
        ) -> Vec<Result<(), DispatcherError>>;

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
        /// If a background write is in progress, blocks until it completes
        /// before removing. Frees staging buffer and/or SSD extent.
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
        /// Allocates a staging buffer, copies data from the IPC handle,
        /// and returns immediately. The staging-to-SSD write happens
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

        /// Async variant of [`populate`] — issues the D2H DMA copy without blocking.
        ///
        /// Evicts if needed, allocates a memory-tier slot, and issues
        /// `dma_copy_to_host_async` on the warm stream. Returns the CUDA stream
        /// the copy was issued on. The caller must poll `stream_query` on the
        /// returned stream and call [`populate_finalize`] once it reports complete.
        ///
        /// Between this call and `populate_finalize`, the memory-tier slot is
        /// allocated but not registered in the dispatch-map — no reader can see it.
        ///
        /// # Errors
        ///
        /// Returns [`DispatcherError::InvalidParameter`] if `ipc_handle.size` is 0.
        /// Returns [`DispatcherError::AlreadyExists`] if the key is already cached.
        /// Returns [`DispatcherError::AllocationFailed`] if the memory-tier pool is full.
        fn populate_async(&self, key: CacheKey, ipc_handle: IpcHandle) -> Result<GpuStream, DispatcherError>;

        /// Finalize a previously started async populate after DMA completion.
        ///
        /// Registers the memory-tier entry in the dispatch-map, downgrades the
        /// write reference to a read reference, and enqueues the background SSD
        /// write-through. Must only be called after `stream_query` confirms the
        /// DMA issued by [`populate_async`] has completed.
        ///
        /// # Errors
        ///
        /// Returns [`DispatcherError::KeyNotFound`] if no pending async populate
        /// exists for this key.
        fn populate_finalize(&self, key: CacheKey) -> Result<(), DispatcherError>;

        /// Prepare a store operation for the given cache key.
        ///
        /// Runs eviction if the cache is over capacity, allocates an extent
        /// on the target data drive, and returns a DMA buffer the caller can
        /// write into. The extent is committed when the caller subsequently
        /// calls `commit_store`.
        ///
        /// # Errors
        ///
        /// Returns [`DispatcherError::InvalidParameter`] if `size` is 0.
        /// Returns [`DispatcherError::AlreadyExists`] if the key is already cached.
        /// Returns [`DispatcherError::AllocationFailed`] if extent allocation fails.
        ///
        /// # Examples
        ///
        /// ```no_run
        /// # use interfaces::{IDispatcher, DispatcherError, CacheKey};
        /// # fn example(dispatcher: &dyn IDispatcher) -> Result<(), DispatcherError> {
        /// let key: CacheKey = 200;
        /// let size: u32 = 4096;
        ///
        /// // Phase 1: prepare — returns a DMA buffer to fill.
        /// let dma_buf = dispatcher.prepare_store(key, size)?;
        ///
        /// // Phase 2: write data into the buffer.
        /// unsafe {
        ///     std::ptr::write_bytes(dma_buf.as_ptr() as *mut u8, 0xCD, size as usize);
        /// }
        ///
        /// // Phase 3: commit — writes buffer to SSD and publishes the extent.
        /// dispatcher.commit_store(key)?;
        /// # Ok(())
        /// # }
        /// ```
        fn prepare_store(&self, key: CacheKey, size: u32) -> Result<Arc<DmaBuffer>, DispatcherError>;

        /// Commit a previously prepared store, writing the DMA buffer to SSD.
        ///
        /// Retrieves the pending write for `key`, writes the buffer contents
        /// to the reserved extent on SSD, publishes the extent metadata, and
        /// registers the entry in the dispatch map as block-device-backed.
        ///
        /// # Errors
        ///
        /// Returns [`DispatcherError::KeyNotFound`] if no pending write exists for `key`.
        /// Returns [`DispatcherError::IoError`] if the SSD write fails.
        ///
        /// # Examples
        ///
        /// ```no_run
        /// # use interfaces::{IDispatcher, DispatcherError, CacheKey};
        /// # fn example(dispatcher: &dyn IDispatcher) -> Result<(), DispatcherError> {
        /// let key: CacheKey = 200;
        /// // Assumes prepare_store(key, size) was called previously.
        /// dispatcher.commit_store(key)?;
        /// // Entry is now persisted on SSD and visible via check/lookup.
        /// assert!(dispatcher.check(key)?);
        /// # Ok(())
        /// # }
        /// ```
        fn commit_store(&self, key: CacheKey) -> Result<(), DispatcherError>;

        /// Cancel a previously prepared store, freeing the reserved extent.
        ///
        /// Removes and drops the pending write for `key`. The `WriteHandle`
        /// destructor automatically aborts the extent reservation.
        ///
        /// # Errors
        ///
        /// Returns [`DispatcherError::KeyNotFound`] if no pending write exists for `key`.
        ///
        /// # Examples
        ///
        /// ```no_run
        /// # use interfaces::{IDispatcher, DispatcherError, CacheKey};
        /// # fn example(dispatcher: &dyn IDispatcher) -> Result<(), DispatcherError> {
        /// let key: CacheKey = 300;
        /// let dma_buf = dispatcher.prepare_store(key, 4096)?;
        /// // Decide not to commit — cancel releases the reserved extent.
        /// dispatcher.cancel_store(key)?;
        /// // Key is no longer visible.
        /// assert!(!dispatcher.check(key)?);
        /// # Ok(())
        /// # }
        /// ```
        fn cancel_store(&self, key: CacheKey) -> Result<(), DispatcherError>;

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
