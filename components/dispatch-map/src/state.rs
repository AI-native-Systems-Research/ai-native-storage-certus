//! Internal synchronization state for the dispatch map.

use std::collections::HashMap;
use std::sync::{Condvar, Mutex, MutexGuard};
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

    /// Block until `predicate` returns `true`, or until `timeout` expires, and
    /// hand back the lock still held.
    ///
    /// Returning the guard is what makes the wait-then-act sequence atomic. If
    /// this released the lock and let the caller re-acquire it, the predicate it
    /// waited for could be falsified in the gap — two callers could both observe
    /// `write_ref == 0` and both go on to claim the write reference, and a reader
    /// could take a reference on an entry a writer had just claimed. Keeping the
    /// guard means whatever the predicate established still holds when the caller
    /// acts on it.
    ///
    /// The returned `bool` is `true` if the predicate was satisfied, `false` on
    /// timeout. On `false` the guard is still valid, so the caller can inspect the
    /// entry to build its error.
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
