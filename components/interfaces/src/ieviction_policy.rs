//! IEvictionPolicy interface and associated types.

use std::fmt;

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

/// Key type tracked by the eviction policy (same underlying type as CacheKey).
pub type EvictionKey = u64;

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

        /// Register a key in the pool, marking it as most-recently-used.
        /// Returns a handle for O(1) touch/remove.
        fn track(&self, pool: PoolId, key: EvictionKey) -> Result<EvictionHandle, EvictionPolicyError>;

        /// Mark the entry as most-recently-used (O(1)).
        fn touch(&self, handle: EvictionHandle) -> Result<(), EvictionPolicyError>;

        /// Stop tracking the entry (O(1) removal from the ordering).
        fn remove(&self, handle: EvictionHandle) -> Result<(), EvictionPolicyError>;

        /// Remove and return the least-recently-used key from the pool (O(1)).
        fn pop_oldest(&self, pool: PoolId) -> Option<EvictionKey>;

        /// Return up to `n` oldest keys from the pool without removing them (O(n)).
        fn peek_oldest(&self, pool: PoolId, n: usize) -> Vec<EvictionKey>;

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
