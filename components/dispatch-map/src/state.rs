//! Internal synchronization state for the dispatch map.

use std::collections::HashMap;
use std::sync::{Arc, Condvar, Mutex, MutexGuard};
use std::time::Duration;

use interfaces::{CacheKey, DmaAllocFn, DmaBuffer, PoolId};

use crate::entry::DispatchEntry;

/// Number of independent shards for concurrent access.
pub(crate) const NUM_SHARDS: usize = 16;

fn default_dma_alloc() -> DmaAllocFn {
    Arc::new(|size, align, numa| DmaBuffer::new(size, align, numa).map_err(|e| e.to_string()))
}

/// A single shard of the dispatch map, protecting a partition of the keyspace.
pub(crate) struct Shard {
    pub entries: HashMap<CacheKey, DispatchEntry>,
}

/// Compute which shard a given key belongs to.
pub(crate) fn shard_for_key(key: CacheKey) -> usize {
    key as usize % NUM_SHARDS
}

/// Thread-safe dispatch map state with blocking support, sharded for concurrency.
pub struct DispatchMapState {
    pub(crate) shards: Vec<Mutex<Shard>>,
    pub(crate) condvars: Vec<Condvar>,
    pub(crate) dma_alloc: Mutex<Option<DmaAllocFn>>,
    pub(crate) pool_id: Mutex<Option<PoolId>>,
}

impl Default for DispatchMapState {
    fn default() -> Self {
        Self::new()
    }
}

impl DispatchMapState {
    pub fn new() -> Self {
        let shards = (0..NUM_SHARDS)
            .map(|_| {
                Mutex::new(Shard {
                    entries: HashMap::new(),
                })
            })
            .collect();
        let condvars = (0..NUM_SHARDS).map(|_| Condvar::new()).collect();

        Self {
            shards,
            condvars,
            dma_alloc: Mutex::new(Some(default_dma_alloc())),
            pool_id: Mutex::new(None),
        }
    }

    /// Lock the shard for the given index.
    pub(crate) fn lock_shard(&self, shard_idx: usize) -> MutexGuard<'_, Shard> {
        self.shards[shard_idx].lock().unwrap()
    }

    /// Block until `predicate` returns `true` for the shard at `shard_idx`, or
    /// until `timeout` expires. Returns `true` if the predicate was
    /// satisfied, `false` on timeout.
    pub(crate) fn wait_for_shard<F>(
        &self,
        shard_idx: usize,
        timeout: Duration,
        mut predicate: F,
    ) -> bool
    where
        F: FnMut(&Shard) -> bool,
    {
        let mut guard = self.shards[shard_idx].lock().unwrap();
        let deadline = std::time::Instant::now() + timeout;
        loop {
            if predicate(&guard) {
                return true;
            }
            let remaining = deadline.saturating_duration_since(std::time::Instant::now());
            if remaining.is_zero() {
                return false;
            }
            let (new_guard, wait_result) = self.condvars[shard_idx]
                .wait_timeout(guard, remaining)
                .unwrap();
            guard = new_guard;
            if wait_result.timed_out() && !predicate(&guard) {
                return false;
            }
        }
    }
}
