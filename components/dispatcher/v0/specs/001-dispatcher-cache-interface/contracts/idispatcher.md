# Interface Contract: IDispatcher

**Crate**: `interfaces` | **Feature gate**: `spdk`

## Definition

```rust
define_interface! {
    pub IDispatcher {
        /// Initialize the dispatcher with the given configuration.
        ///
        /// Creates and initializes N data block devices and N extent managers
        /// based on the provided PCI addresses. The metadata block device
        /// uses namespace partitions for extent manager metadata.
        ///
        /// # Errors
        /// Returns `DispatcherError::NotInitialized` if required receptacles
        /// are not bound, or `DispatcherError::IoError` if device initialization fails.
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
        ///
        /// # Errors
        /// Returns `DispatcherError::KeyNotFound` on cache miss,
        /// `DispatcherError::IoError` on block device read failure.
        fn lookup(&self, key: CacheKey, ipc_handle: IpcHandle) -> Result<(), DispatcherError>;

        /// Check whether a cache entry exists without transferring data.
        fn check(&self, key: CacheKey) -> Result<bool, DispatcherError>;

        /// Remove a cache entry, freeing all associated resources.
        ///
        /// If a background write is in progress, blocks until it completes
        /// before removing. Frees staging buffer and/or SSD extent as applicable.
        ///
        /// # Errors
        /// Returns `DispatcherError::KeyNotFound` if the key does not exist.
        fn remove(&self, key: CacheKey) -> Result<(), DispatcherError>;

        /// Populate a new cache entry by DMA-copying from GPU memory.
        ///
        /// Allocates a staging buffer, copies data from the IPC handle,
        /// and returns immediately. The staging-to-SSD write happens
        /// asynchronously in the background.
        ///
        /// # Errors
        /// Returns `DispatcherError::AlreadyExists` if the key exists,
        /// `DispatcherError::AllocationFailed` if staging buffer allocation fails.
        fn populate(&self, key: CacheKey, ipc_handle: IpcHandle) -> Result<(), DispatcherError>;
    }
}
```

## Supporting Types

```rust
/// Configuration for dispatcher initialization.
pub struct DispatcherConfig {
    /// PCI address of the metadata block device.
    pub metadata_pci_addr: PciAddress,
    /// PCI addresses of N data block devices (one per extent manager).
    pub data_pci_addrs: Vec<PciAddress>,
}

/// Opaque handle to client GPU memory for DMA transfers.
pub struct IpcHandle {
    /// GPU memory base address.
    pub address: *mut u8,
    /// Size of the data in bytes.
    pub size: u32,
}

// SAFETY: GPU memory is accessible cross-thread via DMA engine.
unsafe impl Send for IpcHandle {}
```

## Preconditions

- `initialize()` must be called before any other method (except `shutdown()`).
- All required receptacles (dispatch_map) must be bound before `initialize()`.
- `DispatcherConfig::data_pci_addrs` must be non-empty.
- `IpcHandle::size` must be > 0 and <= extent manager's max extent size.

## Postconditions

- `populate()` guarantees the entry is registered in the dispatch map before returning.
- `shutdown()` guarantees no background threads are running when it returns.
- `remove()` guarantees all resources (staging + SSD) are freed when it returns.
