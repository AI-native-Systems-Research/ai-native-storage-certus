//! IMemoryTier interface and associated types for the memory-tier component.

use std::fmt;

use crate::idispatch_map::CacheKey;

/// Snapshot of memory-tier telemetry counters.
///
/// Returned by `IMemoryTier::telemetry_snapshot()`. All fields are cumulative
/// since the last reset (or component creation).
#[derive(Debug, Clone, Copy, Default)]
pub struct MemoryTierTelemetrySnapshot {
    pub evictions: u64,
    pub write_lock_contentions: u64,
    pub read_lock_contentions: u64,
}

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

// # Verified Properties (see `components/memory-tier/verif/`)
//
// The following invariants are formally proved with Creusot:
//
// - P1 (size-nonzero): insert rejects size == 0
// - P2 (init-guard): insert/remove/get fail when pool not initialized
// - P3 (no-duplicates): insert rejects key that already has a slot
// - P4 (shard-bounded): shard_for_key always returns index < 16
// - P5 (shard-deterministic): same key always maps to same shard
// - P6 (capacity-accounting): insert increases used by size; remove decreases by size
// - P7 (used-within-capacity): used() never exceeds capacity()
// - P8 (pool-full): insert returns PoolFull when used + size > capacity
// - P9 (remove-key-not-found): remove on absent key returns KeyNotFound
// - P10 (evict-round-robin): evict_lru cycles through all 16 shards
//
// Total: 10 properties, 21 verification conditions discharged by SMT solvers.

component_macros::define_interface! {
    pub IMemoryTier {
        /// Initialize the memory-tier pool with the given size in bytes.
        ///
        /// If `numa_node` is `Some(node)` with `node >= 0`, the pool is bound to
        /// that NUMA node via `mbind(MPOL_BIND)`. If binding fails, the pool is
        /// still usable with default memory policy (FR-019 fallback).
        /// Pass `None` to use the kernel's default placement.
        ///
        /// # Verified: P2 (init-guard)
        /// All operations check initialized flag; this sets it.
        ///
        /// # Unchecked: Double-initialization returns error
        /// Calling initialize() twice returns AllocationFailed("already initialized").
        /// Sequential model doesn't test re-initialization race.
        /// Suggested technique: Loom test for concurrent init.
        fn initialize(&self, pool_size: usize, numa_node: Option<i32>) -> Result<(), MemoryTierError>;

        /// Allocate a slot for `key` of `size` bytes and return a pointer to it.
        ///
        /// The returned pointer is valid until the slot is evicted or removed.
        /// Returns `PoolFull` if insufficient contiguous space is available.
        ///
        /// # Verified: P1 (size-nonzero), P2 (init-guard), P3 (no-duplicates), P6 (capacity-accounting), P7 (used-within-capacity), P8 (pool-full)
        /// Rejects zero size. Rejects uninitialized. Rejects existing key.
        /// Increases used. Maintains used <= capacity. Returns PoolFull
        /// when allocator cannot satisfy request.
        ///
        /// # Unchecked: Returned pointer validity and lifetime
        /// Pointer is into mmap/SPDK-allocated pool. Valid until evict or remove.
        /// Suggested technique: ASAN integration test.
        fn insert(&self, key: CacheKey, size: u32) -> Result<*mut u8, MemoryTierError>;

        /// Get the pointer and size for an existing slot, refreshing its LRU position.
        ///
        /// Returns `None` if the key is not present.
        ///
        /// # Verified: P2 (init-guard), P4 (shard-bounded)
        /// Returns None when uninitialized. Shard lookup is in-bounds.
        fn get(&self, key: CacheKey) -> Option<(*mut u8, u32)>;

        /// Get the pointer and size for an existing slot without updating LRU.
        ///
        /// Use this for background operations (e.g., write-through) that should
        /// not prevent eviction of the entry.
        ///
        /// # Verified: P2 (init-guard), P4 (shard-bounded)
        /// Returns None when uninitialized. Shard lookup is in-bounds.
        fn peek(&self, key: CacheKey) -> Option<(*mut u8, u32)>;

        /// Evict the least-recently-used entry, freeing its slot.
        ///
        /// Returns the evicted key, or `None` if the pool is empty.
        ///
        /// # Verified: P6 (capacity-accounting), P10 (evict-round-robin)
        /// Frees the evicted slot (used decreases). Cycles through shards
        /// starting from evict_counter % 16.
        ///
        /// # Unchecked: Eviction selects truly oldest entry
        /// Depends on IEvictionPolicy::pop_oldest correctness.
        /// Suggested technique: property-based test with known insertion order.
        fn evict_lru(&self) -> Option<CacheKey>;

        /// Evict the least-recently-used entry from the same shard as `key`.
        ///
        /// This ensures the freed space is allocatable by a subsequent `insert(key, ...)`.
        /// Returns the evicted key, or `None` if the target shard is empty.
        ///
        /// # Verified: P4 (shard-bounded), P5 (shard-deterministic), P6 (capacity-accounting)
        /// Targets the correct shard deterministically. Frees evicted slot.
        fn evict_lru_for_key(&self, key: CacheKey) -> Option<CacheKey>;

        /// Peek at the N oldest keys without removing them.
        ///
        /// # Unchecked: Returns keys in oldest-first order
        /// Ordering depends on eviction policy implementation.
        /// Suggested technique: property-based test.
        fn oldest_keys(&self, n: usize) -> Vec<CacheKey>;

        /// Remove a specific entry, freeing its slot.
        ///
        /// # Verified: P2 (init-guard), P6 (capacity-accounting), P9 (remove-key-not-found)
        /// Rejects uninitialized. Frees the slot (used decreases).
        /// Returns KeyNotFound if key absent.
        fn remove(&self, key: CacheKey) -> Result<(), MemoryTierError>;

        /// Update the LRU position for `key` without returning data.
        ///
        /// # Verified: P4 (shard-bounded)
        /// Shard lookup is in-bounds.
        fn touch(&self, key: CacheKey);

        /// Update LRU positions for multiple keys in a single batched operation.
        /// Amortizes lock acquisition over the batch for hot-path throughput.
        ///
        /// # Verified: P4 (shard-bounded)
        /// Each key's shard lookup is in-bounds.
        fn batch_touch(&self, keys: &[CacheKey]);

        /// Check whether a slot exists for `key`.
        ///
        /// # Verified: P2 (init-guard), P4 (shard-bounded)
        /// Returns false when uninitialized. Shard lookup in-bounds.
        fn contains(&self, key: CacheKey) -> bool;

        /// Return the total pool capacity in bytes.
        ///
        /// # Verified: P7 (used-within-capacity)
        /// Capacity is the upper bound for used().
        fn capacity(&self) -> usize;

        /// Return the number of bytes currently allocated.
        ///
        /// # Verified: P6 (capacity-accounting), P7 (used-within-capacity)
        /// Monotonically tracks allocations. Never exceeds capacity.
        fn used(&self) -> usize;

        /// Return the base pointer and size of the pool for CUDA host registration.
        fn pool_info(&self) -> Option<(*mut u8, usize)>;

        /// Returns `true` when the pool is backed by SPDK hugepages and pointers
        /// from `insert`/`get` can be used directly for NVMe DMA without an intermediate copy.
        fn is_dma_capable(&self) -> bool;

        /// Remove all entries from the pool, freeing all slots.
        ///
        /// Returns the number of entries that were cleared.
        ///
        /// # Verified: P2 (init-guard)
        /// Rejects uninitialized.
        fn clear(&self) -> Result<usize, MemoryTierError>;

        /// Return a snapshot of telemetry counters.
        ///
        /// Returns zeros when the `telemetry` feature is not enabled on the
        /// implementing crate.
        fn telemetry_snapshot(&self) -> MemoryTierTelemetrySnapshot;
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
