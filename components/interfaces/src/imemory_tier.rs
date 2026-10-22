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

/// Default size of the bounce reserve: four concurrent device reads (the
/// transfer endpoint's default reader count) of up to 8 MiB, the largest KV
/// block `DispatcherConfig::cold_staging_buf_bytes` already provides for (a
/// 64-token block, per worker). Taken from the pool at `initialize`, capped at
/// an eighth of it.
pub const DEFAULT_BOUNCE_RESERVE: usize = 32 * 1024 * 1024;

/// A range of the memory tier's bounce reserve, holding one block-tier entry's
/// bytes while a peer is served from it.
///
/// The reserve lies inside the memory tier's pool but outside its cache, so a
/// buffer is already DMA-able (when the pool is) and already covered by any
/// registration of the whole pool, and taking one never evicts a cached entry.
/// `len` is the length asked for; the reserve allocates it rounded up to
/// 4 KiB, and must be told the same `len` when the buffer is freed.
///
/// # Examples
///
/// ```
/// use interfaces::BounceBuffer;
///
/// let mut buf = [0u8; 8];
/// let bounce = BounceBuffer { ptr: buf.as_mut_ptr(), len: 8 };
/// assert_eq!(bounce.len, 8);
/// ```
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BounceBuffer {
    /// Start of the allocated bytes.
    pub ptr: *mut u8,
    /// Bytes allocated, as requested.
    pub len: u32,
}

// SAFETY: a buffer is an address range handed between threads; whoever holds it
// owns those bytes until it is freed, and the pool outlives every buffer.
unsafe impl Send for BounceBuffer {}

/// Why a bounce buffer could not be had, or a read into one failed.
///
/// The variants divide on whether retrying can help: [`NoSpace`] is transient,
/// [`NeverFits`] is permanent.
///
/// [`NoSpace`]: BounceError::NoSpace
/// [`NeverFits`]: BounceError::NeverFits
///
/// # Examples
///
/// ```
/// use interfaces::BounceError;
///
/// assert!(BounceError::NoSpace.is_transient());
/// assert!(!BounceError::NeverFits("8 MiB > 4 MiB reserve".into()).is_transient());
/// ```
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BounceError {
    /// No room in the reserve right now. Retry after a buffer is freed.
    NoSpace,
    /// Can never be satisfied: zero length, larger than the whole reserve, no
    /// reserve, or a pool the device cannot DMA into. Retrying cannot help.
    NeverFits(String),
    /// The device read failed. Any buffer taken for it has been freed.
    Io(String),
    /// The component is not initialized.
    NotInitialized(String),
}

impl BounceError {
    /// Whether the same request may succeed later.
    #[must_use]
    pub fn is_transient(&self) -> bool {
        matches!(self, Self::NoSpace)
    }
}

impl fmt::Display for BounceError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NoSpace => write!(f, "bounce reserve has no room now"),
            Self::NeverFits(msg) => write!(f, "bounce reserve can never fit this: {msg}"),
            Self::Io(msg) => write!(f, "read into bounce reserve failed: {msg}"),
            Self::NotInitialized(msg) => write!(f, "not initialized: {msg}"),
        }
    }
}

impl std::error::Error for BounceError {}

component_macros::define_interface! {
    pub IMemoryTier {
        /// Initialize the memory-tier pool with the given size in bytes.
        ///
        /// If `numa_node` is `Some(node)` with `node >= 0`, the pool is bound to
        /// that NUMA node via `mbind(MPOL_BIND)`. If binding fails, the pool is
        /// still usable with default memory policy (FR-019 fallback).
        /// Pass `None` to use the kernel's default placement.
        fn initialize(&self, pool_size: usize, numa_node: Option<i32>) -> Result<(), MemoryTierError>;

        /// Allocate a slot for `key` of `size` bytes and return a pointer to it.
        ///
        /// The returned pointer is valid until the slot is evicted or removed.
        /// Returns `InvalidSize` for a size of 0, `NotInitialized` before
        /// `initialize`, and `PoolFull` if insufficient contiguous space is
        /// available.
        fn insert(&self, key: CacheKey, size: u32) -> Result<*mut u8, MemoryTierError>;

        /// Get the pointer and size for an existing slot, refreshing its
        /// eviction-order position (via the bound `IEvictionPolicy`).
        ///
        /// Returns `None` if the key is not present.
        fn get(&self, key: CacheKey) -> Option<(*mut u8, u32)>;

        /// Get the pointer and size for an existing slot without updating its
        /// eviction-order position.
        ///
        /// Use this for background operations (e.g., write-through) that should
        /// not prevent eviction of the entry.
        fn peek(&self, key: CacheKey) -> Option<(*mut u8, u32)>;

        /// Evict the entry selected by the bound eviction policy, freeing its slot.
        ///
        /// Returns the evicted key, or `None` if the pool is empty.
        fn evict_next(&self) -> Option<CacheKey>;

        /// Evict the eviction policy's next victim. Alias for [`evict_next`](Self::evict_next).
        ///
        /// The pool is a single unsharded region, so the `key` argument does not
        /// constrain which victim is chosen — any freed space is globally
        /// allocatable by a subsequent `insert`. Provided for call sites that
        /// want to free space "for" a specific key without special-casing the
        /// unsharded layout.
        /// Returns the evicted key, or `None` if the pool is empty.
        fn evict_next_for_key(&self, key: CacheKey) -> Option<CacheKey>;

        /// Peek at the N oldest keys without removing them.
        fn oldest_keys(&self, n: usize) -> Vec<CacheKey>;

        /// Remove a specific entry, freeing its slot.
        fn remove(&self, key: CacheKey) -> Result<(), MemoryTierError>;

        /// Update the eviction-order position for `key` without returning data.
        fn touch(&self, key: CacheKey);

        /// Update eviction-order positions for multiple keys in a single batched operation.
        /// Amortizes lock acquisition over the batch for hot-path throughput.
        fn batch_touch(&self, keys: &[CacheKey]);

        /// Check whether a slot exists for `key`.
        fn contains(&self, key: CacheKey) -> bool;

        /// Return the bytes the cache may use: the pool size less the bounce
        /// reserve (see [`set_bounce_reserve`](Self::set_bounce_reserve)),
        /// ending on a 4 KiB boundary where the reserve begins. Zero before
        /// `initialize`.
        fn capacity(&self) -> usize;

        /// Return the number of bytes currently allocated.
        fn used(&self) -> usize;

        /// Return the base pointer and size of the whole pool, bounce reserve
        /// included, for CUDA host and RDMA registration.
        fn pool_info(&self) -> Option<(*mut u8, usize)>;

        /// Returns `true` when the pool is backed by SPDK hugepages and pointers
        /// from `insert`/`get` can be used directly for NVMe DMA without an intermediate copy.
        fn is_dma_capable(&self) -> bool;

        /// Remove all entries from the pool, freeing all slots.
        ///
        /// Returns the number of entries that were cleared.
        fn clear(&self) -> Result<usize, MemoryTierError>;

        /// Return a snapshot of telemetry counters.
        ///
        /// Returns zeros when the `telemetry` feature is not enabled on the
        /// implementing crate.
        fn telemetry_snapshot(&self) -> MemoryTierTelemetrySnapshot;

        /// Set the bounce reserve's size in bytes, overriding
        /// [`DEFAULT_BOUNCE_RESERVE`]. Only before `initialize`, which takes
        /// `min(bytes, pool_size / 8)` from the top of the pool; afterwards
        /// this is an error. Zero means no reserve.
        fn set_bounce_reserve(&self, bytes: usize) -> Result<(), MemoryTierError>;

        /// Allocate `len` bytes of the bounce reserve. `NoSpace` if they do not
        /// fit now; `NeverFits` if they never can — zero, larger than the
        /// reserve, no reserve, or a pool that is not DMA-capable;
        /// `NotInitialized` before `initialize`.
        fn bounce_alloc(&self, len: u32) -> Result<BounceBuffer, BounceError>;

        /// Free a buffer from [`bounce_alloc`](Self::bounce_alloc). Only once no
        /// device or NIC can still access its bytes.
        fn bounce_free(&self, buf: BounceBuffer);

        /// The bounce reserve's size and the bytes allocated from it now, both
        /// after rounding to 4 KiB.
        fn bounce_usage(&self) -> (usize, usize);
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
