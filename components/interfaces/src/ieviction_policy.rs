//! IEvictionPolicy interface and associated types.

use std::fmt;

use crate::idispatch_map::CacheKey;

/// Opaque handle returned by `track()`, used for O(1) touch/remove.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct EvictionHandle {
    pool_id: u32,
    index: u32,
}

impl EvictionHandle {
    pub fn new(pool_id: u32, index: u32) -> Self {
        Self { pool_id, index }
    }

    pub fn pool_id(&self) -> u32 {
        self.pool_id
    }

    pub fn index(&self) -> u32 {
        self.index
    }
}

/// Identifier for an independent eviction tracking pool.
pub type PoolId = u32;

/// Errors returned by `IEvictionPolicy` operations.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EvictionPolicyError {
    /// The specified pool does not exist.
    InvalidPool(PoolId),
    /// The specified handle is invalid or already removed.
    InvalidHandle,
}

impl fmt::Display for EvictionPolicyError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidPool(id) => write!(f, "invalid pool id: {id}"),
            Self::InvalidHandle => write!(f, "invalid or already-removed handle"),
        }
    }
}

impl std::error::Error for EvictionPolicyError {}

component_macros::define_interface! {
    pub IEvictionPolicy {
        /// Create a new independent eviction tracking pool.
        fn create_pool(&self) -> PoolId;

        /// Register a key in the pool for eviction tracking.
        /// Returns a handle for O(1) touch/remove.
        fn track(&self, pool: PoolId, key: CacheKey) -> Result<EvictionHandle, EvictionPolicyError>;

        /// Record an access to the entry, updating its eviction ranking.
        /// (For a recency policy this marks it most-recently-used; other
        /// policies may update a frequency count or score.)
        fn touch(&self, handle: EvictionHandle) -> Result<(), EvictionPolicyError>;

        /// Record accesses to multiple entries in a single lock acquisition.
        /// Amortizes lock overhead over the batch for hot-path throughput.
        fn batch_touch(&self, handles: &[EvictionHandle]) -> Result<(), EvictionPolicyError>;

        /// Stop tracking the entry (O(1) removal from the ordering).
        fn remove(&self, handle: EvictionHandle) -> Result<(), EvictionPolicyError>;

        /// Select the next key the policy would evict, remove it from tracking,
        /// and return it. Returns `None` if the pool is empty. The choice is
        /// policy-defined (e.g. least-recently-used for LRU).
        fn identify_next_to_evict(&self, pool: PoolId) -> Option<CacheKey>;

        /// Return up to `n` keys the policy would evict next, in eviction
        /// order, without removing them. The ordering is policy-defined.
        fn get_eviction_candidates(&self, pool: PoolId, n: usize) -> Vec<CacheKey>;

        /// Return the number of tracked entries in the pool.
        fn len(&self, pool: PoolId) -> usize;

        /// Remove all entries from the pool, resetting it to empty.
        fn clear_pool(&self, pool: PoolId);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn eviction_handle_accessors() {
        let h = EvictionHandle::new(3, 42);
        assert_eq!(h.pool_id(), 3);
        assert_eq!(h.index(), 42);
    }

    #[test]
    fn eviction_policy_error_display() {
        assert!(EvictionPolicyError::InvalidPool(5)
            .to_string()
            .contains("invalid pool id: 5"));
        assert!(EvictionPolicyError::InvalidHandle
            .to_string()
            .contains("invalid"));
    }
}
