# Interface Contract: IDispatcher

**Crate**: `interfaces` | **Feature gate**: `spdk`

## Definition

```rust
define_interface! {
    pub IDispatcher {
        /// Initialize the dispatcher with the given configuration.
        ///
        /// Creates and initializes N data block devices and N extent managers
        /// based on the provided PCI addresses. If the ISPDKEnv receptacle is
        /// connected, block devices and extent managers are created internally.
        /// Otherwise, operates in staging-only mode (useful for unit tests).
        ///
        /// # Errors
        /// Returns `DispatcherError::NotInitialized` if required receptacles
        /// are not bound, or `DispatcherError::IoError` if device initialization fails.
        fn initialize(&self, config: DispatcherConfig) -> Result<(), DispatcherError>;

        /// Shut down the dispatcher, completing all in-flight background writes.
        ///
        /// Blocks until all pending staging-to-SSD writes finish, then shuts down
        /// all managed block devices in reverse order.
        fn shutdown(&self) -> Result<(), DispatcherError>;

        /// Look up a cache entry and DMA-copy data to the client's GPU memory.
        ///
        /// If the entry is in staging, copies from the staging buffer via
        /// IGpuServices::dma_copy_to_device. If the entry is on SSD, reads
        /// from the block device using MDTS-aware segmented I/O and copies.
        /// Blocks if a writer is active on the key (dispatch map semantics).
        ///
        /// # Errors
        /// Returns `DispatcherError::KeyNotFound` on cache miss,
        /// `DispatcherError::IoError` on DMA copy or block device read failure.
        fn lookup(&self, key: CacheKey, ipc_handle: IpcHandle) -> Result<(), DispatcherError>;

        /// Check whether a cache entry exists without transferring data.
        fn check(&self, key: CacheKey) -> Result<bool, DispatcherError>;

        /// Remove a cache entry, freeing all associated resources.
        ///
        /// Acquires a write reference, then removes the dispatch map entry.
        /// If the entry is on a block device, frees the SSD extent via the
        /// extent manager.
        ///
        /// # Errors
        /// Returns `DispatcherError::KeyNotFound` if the key does not exist.
        fn remove(&self, key: CacheKey) -> Result<(), DispatcherError>;

        /// Populate a new cache entry by DMA-copying from GPU memory.
        ///
        /// Allocates a staging buffer via the dispatch map, copies data from the
        /// IPC handle using IGpuServices::dma_copy_to_host, downgrades the write
        /// reference, and enqueues an asynchronous background write to SSD.
        ///
        /// # Errors
        /// Returns `DispatcherError::AlreadyExists` if the key exists,
        /// `DispatcherError::AllocationFailed` if staging buffer allocation fails,
        /// `DispatcherError::InvalidParameter` if ipc_handle.size is 0.
        fn populate(&self, key: CacheKey, ipc_handle: IpcHandle) -> Result<(), DispatcherError>;

        /// Prepare a store operation for the given cache key.
        ///
        /// Runs eviction if the cache is over capacity, allocates an extent on
        /// the target data drive, registers the key in the dispatch map, and
        /// returns a DMA buffer the caller can write into.
        ///
        /// # Errors
        /// Returns `DispatcherError::AlreadyExists` if the key exists,
        /// `DispatcherError::AllocationFailed` if extent reservation or DMA
        /// buffer allocation fails, `DispatcherError::InvalidParameter` if size is 0.
        fn prepare_store(&self, key: CacheKey, size: u32) -> Result<Arc<DmaBuffer>, DispatcherError>;

        /// Commit a previously prepared store, writing the DMA buffer to SSD.
        ///
        /// Retrieves the pending write for `key`, writes the buffer contents
        /// to the reserved extent using MDTS-aware segmented I/O, publishes
        /// the extent metadata, and registers the entry as block-device-backed.
        ///
        /// # Errors
        /// Returns `DispatcherError::KeyNotFound` if no pending write exists,
        /// `DispatcherError::IoError` on SSD write failure.
        fn commit_store(&self, key: CacheKey) -> Result<(), DispatcherError>;

        /// Cancel a previously prepared store, freeing the reserved extent.
        ///
        /// Removes the pending write (WriteHandle::drop auto-aborts the
        /// reservation) and removes the dispatch map entry.
        ///
        /// # Errors
        /// Returns `DispatcherError::KeyNotFound` if no pending write exists.
        fn cancel_store(&self, key: CacheKey) -> Result<(), DispatcherError>;

        /// Update the timestamp for a cache entry without performing any DMA.
        ///
        /// Refreshes the eviction timestamp in the dispatch map, preventing the
        /// entry from being selected as an eviction victim. Does not acquire any
        /// read or write reference.
        ///
        /// # Errors
        /// Returns `DispatcherError::KeyNotFound` if the key does not exist.
        fn touch(&self, key: CacheKey) -> Result<(), DispatcherError>;
    }
}
```

## Supporting Types

```rust
/// Configuration for dispatcher initialization.
#[derive(Debug, Clone)]
pub struct DispatcherConfig {
    /// PCI BDF address string of the metadata block device (e.g. "0000:d8:00.0").
    pub metadata_pci_addr: String,
    /// PCI BDF address strings of N data block devices (one per extent manager).
    pub data_pci_addrs: Vec<String>,
    /// Maximum number of cache entries before eviction begins (default: 10000).
    /// Set to 0 to disable eviction.
    pub max_cache_entries: usize,
    /// Fraction of max_cache_entries at which eviction triggers (0.0–1.0).
    /// Default: 0.8 (eviction starts at 80% capacity).
    pub eviction_threshold: f64,
    /// Whether to format extent managers on initialization (default: true).
    /// Set to false when re-initializing to preserve on-disk data.
    pub format_on_init: bool,
}

/// Opaque handle to client GPU memory for DMA transfers.
pub struct IpcHandle {
    /// GPU memory base address.
    pub address: *mut u8,
    /// Size of the data in bytes.
    pub size: u32,
}

// SAFETY: GPU memory is accessible cross-thread via DMA engine.
// Caller guarantees the pointer stays valid for the duration of the operation.
unsafe impl Send for IpcHandle {}
```

## Component Wiring

```
DispatcherComponent --> [IDispatcher provider]
                      <-- [ILogger receptacle]
                      <-- [IDispatchMap receptacle]
                      <-- [IGpuServices receptacle]
                      <-- [ISPDKEnv receptacle]
```

Block devices and extent managers are created internally during `initialize()`
based on the `DispatcherConfig` PCI addresses. The `ISPDKEnv` receptacle provides
the SPDK environment for device initialization and DMA buffer allocation.

## Preconditions

- `initialize()` must be called before any other method (except `shutdown()`).
- `dispatch_map` and `gpu_services` receptacles must be bound before `initialize()`.
- `spdk_env` receptacle must be bound for hardware mode (optional for staging-only).
- `DispatcherConfig::data_pci_addrs` must be non-empty.
- `IpcHandle::size` must be > 0.
- `prepare_store` size must be > 0.

## Postconditions

- `populate()` guarantees the entry is registered in the dispatch map before returning.
- `shutdown()` guarantees no background threads are running when it returns.
- `remove()` guarantees the dispatch map entry is removed when it returns.
- `prepare_store()` guarantees the key is visible via `check()` after return.
- `commit_store()` guarantees data is persisted to SSD when it returns.
- `cancel_store()` guarantees no resources remain allocated for the key.
- `touch()` guarantees the TSC is updated atomically; does not block on references.

## Eviction

When `max_cache_entries > 0` and the cache size exceeds `eviction_threshold × max_cache_entries`, `prepare_store` triggers a synchronous eviction cycle that removes the oldest entries (by TSC) until the count reaches the watermark. Eviction skips entries with active write references.
