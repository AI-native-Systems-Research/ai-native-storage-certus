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
///     metadata_pci_addr: "0000:01:00.0".to_string(),
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
    /// PCI address of the metadata block device.
    pub metadata_pci_addr: String,
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
}

impl Default for DispatcherConfig {
    fn default() -> Self {
        Self {
            metadata_pci_addr: String::new(),
            data_pci_addrs: Vec::new(),
            max_cache_entries: 10000,
            eviction_threshold: 0.8,
            format_on_init: true,
            ssd_eviction_threshold: 0.9,
            ssd_eviction_low_watermark: 0.8,
            ssd_eviction_batch_size: 64,
            ssd_eviction_interval_secs: 5,
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
        fn initialize(&self, config: DispatcherConfig) -> Result<(), DispatcherError>;

        /// Shut down the dispatcher, completing all in-flight background writes.
        ///
        /// Blocks until all pending staging-to-SSD writes finish, then shuts down
        /// all managed block devices and extent managers.
        fn shutdown(&self) -> Result<(), DispatcherError>;

        /// Look up a cache entry and DMA-copy data to the client's GPU memory.
        ///
        /// If the entry is in staging, copies from the staging buffer.
        /// If the entry is on SSD, reads from the block device and copies.
        /// Blocks if a writer is active on the key (dispatch map semantics).
        fn lookup(&self, key: CacheKey, ipc_handle: IpcHandle) -> Result<(), DispatcherError>;

        /// Async variant of [`lookup`] — issues the H2D DMA copy without blocking.
        ///
        /// Returns the CUDA stream the copy was issued on. The caller must call
        /// `stream_synchronize` on the returned stream before accessing the GPU
        /// destination memory. For non-memory-tier paths (staging, SSD) the copy
        /// completes synchronously and a null stream is returned.
        fn lookup_async(&self, key: CacheKey, ipc_handle: IpcHandle) -> Result<GpuStream, DispatcherError>;

        /// Check whether a cache entry exists without transferring data.
        fn check(&self, key: CacheKey) -> Result<bool, DispatcherError>;

        /// Remove a cache entry, freeing all associated resources.
        ///
        /// If a background write is in progress, blocks until it completes
        /// before removing. Frees staging buffer and/or SSD extent.
        fn remove(&self, key: CacheKey) -> Result<(), DispatcherError>;

        /// Populate a new cache entry by DMA-copying from GPU memory.
        ///
        /// Allocates a staging buffer, copies data from the IPC handle,
        /// and returns immediately. The staging-to-SSD write happens
        /// asynchronously in the background.
        fn populate(&self, key: CacheKey, ipc_handle: IpcHandle) -> Result<(), DispatcherError>;

        /// Prepare a store operation for the given cache key.
        ///
        /// Runs eviction if the cache is over capacity, allocates an extent
        /// on the target data drive, and returns a DMA buffer the caller can
        /// write into. The extent is committed when the caller subsequently
        /// calls `commit_store`.
        fn prepare_store(&self, key: CacheKey, size: u32) -> Result<Arc<DmaBuffer>, DispatcherError>;

        /// Commit a previously prepared store, writing the DMA buffer to SSD.
        ///
        /// Retrieves the pending write for `key`, writes the buffer contents
        /// to the reserved extent on SSD, publishes the extent metadata, and
        /// registers the entry in the dispatch map as block-device-backed.
        fn commit_store(&self, key: CacheKey) -> Result<(), DispatcherError>;

        /// Cancel a previously prepared store, freeing the reserved extent.
        ///
        /// Removes and drops the pending write for `key`. The `WriteHandle`
        /// destructor automatically aborts the extent reservation.
        fn cancel_store(&self, key: CacheKey) -> Result<(), DispatcherError>;

        /// Update the timestamp for a cache entry without performing any DMA.
        ///
        /// Used to refresh the eviction timestamp in the dispatch map,
        /// preventing the entry from being evicted without transferring data.
        fn touch(&self, key: CacheKey) -> Result<(), DispatcherError>;
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
            metadata_pci_addr: "0000:01:00.0".to_string(),
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
