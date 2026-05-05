//! IDispatchMap interface and associated types for the dispatch map component.

use std::fmt;

/// Key type for identifying extents in the dispatch map.
pub type CacheKey = u64;

/// Result of looking up a key in the dispatch map.
#[cfg(feature = "spdk")]
#[derive(Debug)]
pub enum LookupResult {
    /// Key not found in the map.
    NotExist,
    /// Key found but the requested size does not match the stored size.
    MismatchSize,
    /// Data is in a DMA staging buffer.
    Staging {
        /// Shared reference to the DMA buffer.
        buffer: std::sync::Arc<crate::spdk_types::DmaBuffer>,
    },
    /// Data has been committed to a block device.
    BlockDevice {
        /// Byte offset on the block device.
        offset: u64,
    },
}

/// Errors returned by `IDispatchMap` operations.
#[derive(Debug, Clone)]
pub enum DispatchMapError {
    /// The specified key was not found in the map.
    KeyNotFound(CacheKey),
    /// An entry with this key already exists.
    AlreadyExists(CacheKey),
    /// Cannot remove: active read or write references exist.
    ActiveReferences(CacheKey),
    /// A blocking operation exceeded its timeout deadline.
    Timeout(CacheKey),
    /// DMA buffer allocation failed.
    AllocationFailed(String),
    /// Invalid size parameter (e.g., zero).
    InvalidSize,
    /// Component not initialized or missing DMA allocator.
    NotInitialized(String),
    /// Reference count underflow (release when already zero).
    RefCountUnderflow(CacheKey),
    /// Downgrade requested but no write reference is held.
    NoWriteReference(CacheKey),
    /// Operation invalid for the current entry state.
    InvalidState(String),
}

impl fmt::Display for DispatchMapError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::KeyNotFound(k) => write!(f, "key not found: {k}"),
            Self::AlreadyExists(k) => write!(f, "key already exists: {k}"),
            Self::ActiveReferences(k) => write!(f, "active references on key: {k}"),
            Self::Timeout(k) => write!(f, "timeout waiting on key: {k}"),
            Self::AllocationFailed(msg) => write!(f, "allocation failed: {msg}"),
            Self::InvalidSize => write!(f, "invalid size: must be > 0"),
            Self::NotInitialized(msg) => write!(f, "not initialized: {msg}"),
            Self::RefCountUnderflow(k) => write!(f, "ref count underflow on key: {k}"),
            Self::NoWriteReference(k) => write!(f, "no write reference held on key: {k}"),
            Self::InvalidState(msg) => write!(f, "invalid state: {msg}"),
        }
    }
}

impl std::error::Error for DispatchMapError {}

#[cfg(feature = "spdk")]
component_macros::define_interface! {
    pub IDispatchMap {
        /// Set the DMA buffer allocator used by `create_staging`.
        fn set_dma_alloc(&self, alloc: crate::spdk_types::DmaAllocFn);

        /// Recover committed extents from the bound `IExtentManager`.
        fn initialize(&self) -> Result<(), DispatchMapError>;

        /// Allocate a DMA staging buffer for `key` with `size` 4KiB blocks.
        fn create_staging(
            &self,
            key: CacheKey,
            size: u32,
        ) -> Result<std::sync::Arc<crate::spdk_types::DmaBuffer>, DispatchMapError>;

        /// Look up `key`, blocking if a writer is active.
        fn lookup(&self, key: CacheKey) -> Result<LookupResult, DispatchMapError>;

        /// Transition a staging entry to a block-device location.
        fn convert_to_storage(
            &self,
            key: CacheKey,
            offset: u64,
        ) -> Result<(), DispatchMapError>;

        /// Acquire a read reference, blocking if a writer is active.
        fn take_read(&self, key: CacheKey) -> Result<(), DispatchMapError>;

        /// Acquire a write reference, blocking if readers or writers are active.
        fn take_write(&self, key: CacheKey) -> Result<(), DispatchMapError>;

        /// Release a read reference.
        fn release_read(&self, key: CacheKey) -> Result<(), DispatchMapError>;

        /// Release a write reference.
        fn release_write(&self, key: CacheKey) -> Result<(), DispatchMapError>;

        /// Atomically downgrade a write reference to a read reference.
        fn downgrade_reference(&self, key: CacheKey) -> Result<(), DispatchMapError>;

        /// Remove an entry from the map.
        fn remove(&self, key: CacheKey) -> Result<(), DispatchMapError>;

        /// Return up to `n` keys with the oldest timestamps (lowest TSC values).
        fn oldest_keys(&self, n: usize) -> Vec<CacheKey>;
    }
}
