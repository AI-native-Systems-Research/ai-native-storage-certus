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
    /// Data has been committed to a block device.
    BlockDevice {
        /// Byte offset on the block device.
        offset: u64,
    },
    /// Data is in the DRAM memory-tier.
    MemoryTier {
        /// Pointer to the data in the memory-tier pool.
        pointer: *mut u8,
        /// Size of the data in bytes.
        size: u32,
    },
}

// SAFETY: The pointer in MemoryTier refers to memory in the memory-tier pool,
// which is thread-safe (mmap'd region protected by dispatch-map mutex).
#[cfg(feature = "spdk")]
unsafe impl Send for LookupResult {}
#[cfg(feature = "spdk")]
unsafe impl Sync for LookupResult {}

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
    /// Reference count overflow (acquire when already at u32::MAX).
    RefCountOverflow(CacheKey),
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
            Self::RefCountOverflow(k) => write!(f, "ref count overflow on key: {k}"),
            Self::NoWriteReference(k) => write!(f, "no write reference held on key: {k}"),
            Self::InvalidState(msg) => write!(f, "invalid state: {msg}"),
        }
    }
}

impl std::error::Error for DispatchMapError {}

#[cfg(feature = "spdk")]
component_macros::define_interface! {
    pub IDispatchMap {
        /// Recover committed extents from the bound `IExtentManager`.
        fn initialize(&self) -> Result<(), DispatchMapError>;

        /// Look up `key`, blocking if a writer is active.
        fn lookup(&self, key: CacheKey) -> Result<LookupResult, DispatchMapError>;

        /// Transition a memory-tier entry to a block-device location.
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

        /// Update the timestamp for `key` without taking any reference.
        fn touch(&self, key: CacheKey) -> Result<(), DispatchMapError>;

        /// Return the stored size of an entry in bytes.
        ///
        /// Does not acquire any reference. Returns `KeyNotFound` if absent.
        fn entry_size(&self, key: CacheKey) -> Result<u32, DispatchMapError>;

        /// Return up to `n` keys with the oldest timestamps (lowest TSC values).
        fn oldest_keys(&self, n: usize) -> Vec<CacheKey>;

        /// Create an entry for a key with a memory-tier location.
        ///
        /// Acquires a write reference.
        fn create_memory_tier_entry(
            &self,
            key: CacheKey,
            pointer: *mut u8,
            size: u32,
        ) -> Result<(), DispatchMapError>;

        /// Convert a memory-tier entry to a block-device location.
        ///
        /// Transitions `MemoryTier { ssd_offset: Some(off) }` to
        /// `BlockDevice { offset: off }`. Fails if the entry has no
        /// recorded SSD offset (write-through not yet complete).
        fn convert_memory_tier_to_block(&self, key: CacheKey) -> Result<(), DispatchMapError>;

        /// Promote a block-device entry to a memory-tier location **in place**.
        ///
        /// Transitions `BlockDevice { offset }` to
        /// `MemoryTier { pointer, size, ssd_offset: Some(offset) }`, preserving
        /// the entry's eviction handle and ALL reference counts. This is the
        /// inverse of `convert_memory_tier_to_block` and, crucially, does NOT
        /// remove/recreate the entry — so it works while the entry is pinned
        /// (`read_ref > 0`), which is exactly the case during a load
        /// (prepare_load pins, then the lookup promotes the cold block).
        /// `ssd_offset` retains the original block offset so the promoted entry
        /// remains demotable/evictable without data loss.
        ///
        /// Returns `KeyNotFound` if absent, `InvalidSize` if size == 0, and
        /// `InvalidState` if the entry is not in block-device state.
        fn promote_block_to_memory_tier(
            &self,
            key: CacheKey,
            pointer: *mut u8,
            size: u32,
        ) -> Result<(), DispatchMapError>;

        /// Check if a memory-tier entry is safe to evict.
        ///
        /// Returns `true` if the entry exists, is in MemoryTier state with
        /// `ssd_offset: Some(_)`, and has no active read/write references.
        fn is_evictable(&self, key: CacheKey) -> bool;

        /// Atomically check evictability and transition to BlockDevice.
        ///
        /// Under a single lock hold: verifies the entry is in MemoryTier with
        /// `ssd_offset: Some(_)` and `read_ref == 0 && write_ref == 0`, then
        /// transitions it to `BlockDevice { offset }`. Returns `Ok(())` on
        /// success. After this returns Ok, no new reader can obtain the
        /// memory-tier pointer, so the caller may safely free the DRAM slot.
        ///
        /// Returns `Err(KeyNotFound)` if the key doesn't exist, or
        /// `Err(InvalidState)` if the entry is not evictable (refs held,
        /// no ssd_offset, or not in MemoryTier state).
        fn try_evict_to_block(&self, key: CacheKey) -> Result<(), DispatchMapError>;

        /// Insert a recovered extent as a BlockDevice entry.
        ///
        /// Used during recovery to rebuild the dispatch map from persisted
        /// extents without allocating DMA buffers.
        fn recover_extent(
            &self,
            key: CacheKey,
            offset: u64,
            size_blocks: u32,
        ) -> Result<(), DispatchMapError>;

        /// Record the CRC-32 integrity checksum for a stored block's data.
        ///
        /// Called on the store path once the block has landed in its slot, so
        /// the checksum travels with the index entry (and survives
        /// demote/promote) rather than living in caller-local state. Returns
        /// `KeyNotFound` if the entry is absent.
        ///
        /// Only present under the `integrity-check` feature.
        #[cfg(feature = "integrity-check")]
        fn set_checksum(&self, key: CacheKey, checksum: u32) -> Result<(), DispatchMapError>;

        /// Fetch the recorded CRC-32 for `key`, or `None` if the entry is
        /// absent or has no checksum recorded (e.g. never stored under this
        /// feature). Callers treat `None` as "skip verification".
        ///
        /// Only present under the `integrity-check` feature.
        #[cfg(feature = "integrity-check")]
        fn get_checksum(&self, key: CacheKey) -> Option<u32>;
    }
}
