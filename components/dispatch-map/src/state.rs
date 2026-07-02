//! Internal synchronization state for the dispatch map.

use std::collections::HashMap;
use std::sync::{Condvar, Mutex};
use std::time::Duration;

use interfaces::{CacheKey, PoolId};

use crate::entry::DispatchEntry;

/// Protected inner state behind the Mutex.
pub(crate) struct Inner {
    pub entries: HashMap<CacheKey, DispatchEntry>,
}

/// Thread-safe dispatch map state with blocking support.
pub struct DispatchMapState {
    pub(crate) inner: Mutex<Inner>,
    pub(crate) condvar: Condvar,
    pub(crate) pool_id: Mutex<Option<PoolId>>,
}

impl Default for DispatchMapState {
    fn default() -> Self {
        Self::new()
    }
}

impl DispatchMapState {
    pub fn new() -> Self {
        Self {
            inner: Mutex::new(Inner {
                entries: HashMap::new(),
            }),
            condvar: Condvar::new(),
            pool_id: Mutex::new(None),
        }
    }

    /// Block until `predicate` returns `true` for the entry at `key`, or
    /// until `timeout` expires. Returns `true` if the predicate was
    /// satisfied, `false` on timeout.
    pub(crate) fn wait_for<F>(&self, timeout: Duration, mut predicate: F) -> bool
    where
        F: FnMut(&Inner) -> bool,
    {
        let mut guard = self.inner.lock().unwrap();
        let deadline = std::time::Instant::now() + timeout;
        loop {
            if predicate(&guard) {
                return true;
            }
            let remaining = deadline.saturating_duration_since(std::time::Instant::now());
            if remaining.is_zero() {
                return false;
            }
            let (new_guard, wait_result) = self.condvar.wait_timeout(guard, remaining).unwrap();
            guard = new_guard;
            if wait_result.timed_out() && !predicate(&guard) {
                return false;
            }
        }
    }
}
