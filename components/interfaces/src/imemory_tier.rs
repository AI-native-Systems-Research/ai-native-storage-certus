//! IMemoryTier interface and associated types for the memory-tier component.

use std::fmt;

use crate::idispatch_map::CacheKey;

/// Errors returned by `IMemoryTier` operations.
#[derive(Debug, Clone)]
pub enum MemoryTierError {
    /// The memory pool is full and no space can be freed.
    PoolFull,
    /// The specified cache key was not found.
    KeyNotFound(CacheKey),
    /// A slot with this key already exists.
    AlreadyExists(CacheKey),
    /// Pool allocation failed.
    AllocationFailed(String),
    /// Invalid size parameter (e.g., zero).
    InvalidSize,
    /// Entry cannot be evicted (e.g., write-through not yet complete).
    NotEvictable(CacheKey),
    /// Component not initialized.
    NotInitialized(String),
}

impl fmt::Display for MemoryTierError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::PoolFull => write!(f, "memory-tier pool full"),
            Self::KeyNotFound(k) => write!(f, "key not found: {k}"),
            Self::AlreadyExists(k) => write!(f, "key already exists: {k}"),
            Self::AllocationFailed(msg) => write!(f, "allocation failed: {msg}"),
            Self::InvalidSize => write!(f, "invalid size: must be > 0"),
            Self::NotEvictable(k) => write!(f, "entry not evictable: {k}"),
            Self::NotInitialized(msg) => write!(f, "not initialized: {msg}"),
        }
    }
}

impl std::error::Error for MemoryTierError {}

#[cfg(feature = "spdk")]
component_macros::define_interface! {
    pub IMemoryTier {
        /// Initialize the memory-tier pool with the given size in bytes.
        fn initialize(&self, pool_size: usize) -> Result<(), MemoryTierError>;

        /// Allocate a slot for `key` of `size` bytes and return a pointer to it.
        ///
        /// The returned pointer is valid until the slot is evicted or removed.
        /// Returns `PoolFull` if insufficient contiguous space is available.
        fn insert(&self, key: CacheKey, size: u32) -> Result<*mut u8, MemoryTierError>;

        /// Get the pointer and size for an existing slot, refreshing its LRU position.
        ///
        /// Returns `None` if the key is not present.
        fn get(&self, key: CacheKey) -> Option<(*mut u8, u32)>;

        /// Get the pointer and size for an existing slot without updating LRU.
        ///
        /// Use this for background operations (e.g., write-through) that should
        /// not prevent eviction of the entry.
        fn peek(&self, key: CacheKey) -> Option<(*mut u8, u32)>;

        /// Evict the least-recently-used entry, freeing its slot.
        ///
        /// Returns the evicted key, or `None` if the pool is empty.
        fn evict_lru(&self) -> Option<CacheKey>;

        /// Evict the least-recently-used entry from the same shard as `key`.
        ///
        /// This ensures the freed space is allocatable by a subsequent `insert(key, ...)`.
        /// Returns the evicted key, or `None` if the target shard is empty.
        fn evict_lru_for_key(&self, key: CacheKey) -> Option<CacheKey>;

        /// Peek at the N oldest keys without removing them.
        fn oldest_keys(&self, n: usize) -> Vec<CacheKey>;

        /// Remove a specific entry, freeing its slot.
        fn remove(&self, key: CacheKey) -> Result<(), MemoryTierError>;

        /// Update the LRU position for `key` without returning data.
        fn touch(&self, key: CacheKey);

        /// Check whether a slot exists for `key`.
        fn contains(&self, key: CacheKey) -> bool;

        /// Return the total pool capacity in bytes.
        fn capacity(&self) -> usize;

        /// Return the number of bytes currently allocated.
        fn used(&self) -> usize;

        /// Return the base pointer and size of the pool for CUDA host registration.
        fn pool_info(&self) -> Option<(*mut u8, usize)>;

        /// Remove all entries from the pool, freeing all slots.
        ///
        /// Returns the number of entries that were cleared.
        fn clear(&self) -> Result<usize, MemoryTierError>;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn memory_tier_error_display() {
        assert!(MemoryTierError::PoolFull.to_string().contains("pool full"));
        assert!(MemoryTierError::KeyNotFound(42).to_string().contains("42"));
        assert!(MemoryTierError::AlreadyExists(7)
            .to_string()
            .contains("already exists"));
        assert!(MemoryTierError::InvalidSize
            .to_string()
            .contains("invalid size"));
        assert!(MemoryTierError::NotEvictable(3)
            .to_string()
            .contains("not evictable"));
    }
}
