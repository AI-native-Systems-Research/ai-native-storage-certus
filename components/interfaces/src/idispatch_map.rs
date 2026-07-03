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

// # Verified Properties (see `components/dispatch-map/verif/`)
//
// The following invariants are formally proved with Creusot:
//
// - P1 (read-underflow): release_read fails when read_ref == 0
// - P2 (write-underflow): release_write fails when write_ref == 0
// - P3 (write-binary): write_ref is always 0 or 1 (take_write sets exactly 1)
// - P4 (downgrade-requires-write): downgrade_reference fails without active write ref
// - P5 (downgrade-conservation): downgrade preserves total ref count (write+read constant)
// - P6 (remove-zero-refs): remove fails if any read or write references are active
// - P7 (create-no-duplicates): create_memory_tier_entry rejects existing keys
// - P8 (size-nonzero): create_memory_tier_entry rejects size == 0
// - P9 (lookup-increments-read): successful lookup increments read_ref by exactly 1
// - P10 (convert-requires-ssd-offset): convert_memory_tier_to_block requires ssd_offset present
//
// Total: 10 properties, 24 verification conditions discharged by SMT solvers.

#[cfg(feature = "spdk")]
component_macros::define_interface! {
    pub IDispatchMap {
        /// Recover committed extents from the bound `IExtentManager`.
        fn initialize(&self) -> Result<(), DispatchMapError>;

        /// Look up `key`, blocking if a writer is active.
        ///
        /// # Verified: P9 (lookup-increments-read)
        /// On success, increments read_ref by exactly 1. Caller must call
        /// release_read when done.
        ///
        /// # Unchecked: Blocks until writer releases (timeout 2s)
        /// Uses condvar wait with 2s timeout. Concurrency correctness of the
        /// wait/notify protocol is not modeled sequentially.
        /// Suggested technique: Spin model or Loom test.
        fn lookup(&self, key: CacheKey) -> Result<LookupResult, DispatchMapError>;

        /// Transition a memory-tier entry to a block-device location.
        ///
        /// # Unchecked: Also decrements read_ref by 1
        /// Implementation atomically converts location AND decrements read_ref.
        /// The combined semantics are tested but not formally proved.
        /// Suggested technique: extend Creusot model with state-machine transitions.
        fn convert_to_storage(
            &self,
            key: CacheKey,
            offset: u64,
        ) -> Result<(), DispatchMapError>;

        /// Acquire a read reference, blocking if a writer is active.
        ///
        /// # Verified: P9 (lookup-increments-read)
        /// Increments read_ref by 1 on success. Fails with Timeout if writer
        /// holds ref beyond 2s.
        ///
        /// # Unchecked: Blocks until writer releases (timeout 2s)
        /// Suggested technique: Spin model.
        fn take_read(&self, key: CacheKey) -> Result<(), DispatchMapError>;

        /// Acquire a write reference, blocking if readers or writers are active.
        ///
        /// # Verified: P3 (write-binary)
        /// Sets write_ref to exactly 1. Only succeeds when both read_ref == 0
        /// and write_ref == 0 (exclusive access).
        ///
        /// # Unchecked: Blocks until all refs released (timeout 2s)
        /// Suggested technique: Spin model.
        fn take_write(&self, key: CacheKey) -> Result<(), DispatchMapError>;

        /// Release a read reference.
        ///
        /// # Verified: P1 (read-underflow)
        /// Returns RefCountUnderflow if read_ref is already 0.
        /// On success, decrements read_ref by exactly 1.
        fn release_read(&self, key: CacheKey) -> Result<(), DispatchMapError>;

        /// Release a write reference.
        ///
        /// # Verified: P2 (write-underflow)
        /// Returns RefCountUnderflow if write_ref is already 0.
        /// On success, sets write_ref to 0.
        fn release_write(&self, key: CacheKey) -> Result<(), DispatchMapError>;

        /// Atomically downgrade a write reference to a read reference.
        ///
        /// # Verified: P4 (downgrade-requires-write), P5 (downgrade-conservation)
        /// Fails with NoWriteReference if write_ref == 0. On success,
        /// write_ref becomes 0 and read_ref increments by 1. Total
        /// reference count (write + read) is preserved.
        fn downgrade_reference(&self, key: CacheKey) -> Result<(), DispatchMapError>;

        /// Remove an entry from the map.
        ///
        /// # Verified: P6 (remove-zero-refs)
        /// Returns ActiveReferences if read_ref > 0 or write_ref > 0.
        /// Entry can only be removed when completely unreferenced.
        fn remove(&self, key: CacheKey) -> Result<(), DispatchMapError>;

        /// Update the timestamp for `key` without taking any reference.
        fn touch(&self, key: CacheKey) -> Result<(), DispatchMapError>;

        /// Return the stored size of an entry in bytes.
        ///
        /// Does not acquire any reference. Returns `KeyNotFound` if absent.
        fn entry_size(&self, key: CacheKey) -> Result<u32, DispatchMapError>;

        /// Return up to `n` keys with the oldest timestamps (lowest TSC values).
        ///
        /// # Unchecked: Oldest-first ordering
        /// Depends on eviction policy LRU correctness.
        /// Suggested technique: property-based test with known insertion order.
        fn oldest_keys(&self, n: usize) -> Vec<CacheKey>;

        /// Create an entry for a key with a memory-tier location.
        ///
        /// Acquires a write reference.
        ///
        /// # Verified: P7 (create-no-duplicates), P8 (size-nonzero)
        /// Returns AlreadyExists if key present. Returns InvalidSize if size == 0.
        /// On success, sets write_ref=1.
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
        ///
        /// # Verified: P10 (convert-requires-ssd-offset)
        /// Rejects entries without ssd_offset (InvalidState).
        /// Rejects entries not in MemoryTier state.
        fn convert_memory_tier_to_block(&self, key: CacheKey) -> Result<(), DispatchMapError>;

        /// Check if a memory-tier entry is safe to evict.
        ///
        /// Returns `true` if the entry exists, is in MemoryTier state with
        /// `ssd_offset: Some(_)`, and has no active read/write references.
        ///
        /// # Verified: P6 (remove-zero-refs), P10 (convert-requires-ssd-offset)
        /// Combines the zero-refs check (P6) with the ssd_offset presence
        /// check (P10) into a single predicate.
        fn is_evictable(&self, key: CacheKey) -> bool;

        /// Insert a recovered extent as a BlockDevice entry.
        ///
        /// Used during recovery to rebuild the dispatch map from persisted
        /// extents without allocating DMA buffers.
        ///
        /// # Verified: P7 (create-no-duplicates)
        /// Returns AlreadyExists if key is already present.
        fn recover_extent(
            &self,
            key: CacheKey,
            offset: u64,
            size_blocks: u32,
        ) -> Result<(), DispatchMapError>;
    }
}
