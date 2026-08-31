//! Internal synchronization state for the dispatch map.
//!
//! The map is split into [`N_SHARDS`] independent shards, each with its own
//! `Mutex<Inner>` and `Condvar`. Operations route to a shard via
//! `key as usize % N_SHARDS`, so threads operating on different keys rarely
//! contend on the same lock.

use std::collections::HashMap;
use std::sync::{Condvar, Mutex, MutexGuard};
use std::time::Duration;

use interfaces::{CacheKey, PoolId};

use crate::entry::DispatchEntry;

/// Number of independent shards. Must be a power of two for fast modular
/// arithmetic (the compiler turns `% N` into `& (N-1)` when N is a power of 2).
pub const N_SHARDS: usize = 64;

/// Protected inner state behind each shard's Mutex.
pub(crate) struct Inner {
    pub entries: HashMap<CacheKey, DispatchEntry>,
}

/// One independent shard of the dispatch map.
pub struct Shard {
    pub(crate) inner: Mutex<Inner>,
    pub(crate) condvar: Condvar,
}

impl Shard {
    fn new() -> Self {
        Self {
            inner: Mutex::new(Inner {
                entries: HashMap::new(),
            }),
            condvar: Condvar::new(),
        }
    }

    /// Block until `predicate` returns `true`, or until `timeout` expires, and
    /// hand back the lock still held.
    ///
    /// Returning the guard is what makes the wait-then-act sequence atomic.
    /// The returned `bool` is `true` if the predicate was satisfied, `false` on
    /// timeout. On `false` the guard is still valid.
    pub(crate) fn wait_for<F>(
        &self,
        timeout: Duration,
        mut predicate: F,
    ) -> (bool, MutexGuard<'_, Inner>)
    where
        F: FnMut(&Inner) -> bool,
    {
        let mut guard = self.inner.lock().unwrap();
        let deadline = std::time::Instant::now() + timeout;
        loop {
            if predicate(&guard) {
                return (true, guard);
            }
            let remaining = deadline.saturating_duration_since(std::time::Instant::now());
            if remaining.is_zero() {
                return (false, guard);
            }
            let (new_guard, wait_result) = self.condvar.wait_timeout(guard, remaining).unwrap();
            guard = new_guard;
            if wait_result.timed_out() && !predicate(&guard) {
                return (false, guard);
            }
        }
    }
}

/// Thread-safe sharded dispatch map state.
pub struct DispatchMapState {
    pub(crate) shards: Vec<Shard>,
    pub(crate) pool_id: Mutex<Option<PoolId>>,
}

impl Default for DispatchMapState {
    fn default() -> Self {
        Self::new()
    }
}

impl DispatchMapState {
    pub fn new() -> Self {
        let mut shards = Vec::with_capacity(N_SHARDS);
        for _ in 0..N_SHARDS {
            shards.push(Shard::new());
        }
        Self {
            shards,
            pool_id: Mutex::new(None),
        }
    }

    /// Return the shard that owns `key`.
    #[inline]
    pub(crate) fn shard_for(&self, key: CacheKey) -> &Shard {
        &self.shards[key as usize % N_SHARDS]
    }
}
