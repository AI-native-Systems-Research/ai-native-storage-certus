//! Ownership of dispatch-map read pins across an asynchronous GPU copy.

use std::sync::Arc;

use interfaces::{CacheKey, IDispatchMap};

/// A batch of dispatch-map read pins, released together when this is dropped.
///
/// A pin must outlive the *completion* of an H2D copy, not its submission.
/// `IDispatchMap::is_evictable` and `try_evict_to_block` both refuse while
/// `read_ref > 0`, and that refusal is the only thing stopping the memory-tier
/// evictor demoting the entry and handing its DRAM slot back to the pool while a
/// copy is still reading it. Since submission is asynchronous and the sync is
/// batched, the pin has to survive from `lookup` until after `stream_synchronize`.
///
/// Hence a guard rather than hand-rolled release calls: the set of paths that must
/// release grew with the async submission (submitted, submit failure, lookup miss,
/// sync failure), and a missed release is unrecoverable — `read_ref` carries no
/// owner identity, so a leaked pin makes its entry permanently unevictable and is
/// indistinguishable from a live reader. There is no leak detector to catch it.
///
/// Mirrors the identically-named type in the `dispatcher` crate, and `PinnedBatch`
/// in `remote-lookup`'s `server.rs`, which holds pins across an RDMA completion for
/// the same reason. Kept crate-local rather than hoisted into `interfaces`: this is
/// a data-plane invariant, not part of the public ABI.
pub(crate) struct PinnedKeys {
    dispatch_map: Arc<dyn IDispatchMap + Send + Sync>,
    keys: Vec<CacheKey>,
}

impl PinnedKeys {
    /// An empty batch pinning nothing.
    pub(crate) fn new(dispatch_map: Arc<dyn IDispatchMap + Send + Sync>) -> Self {
        Self {
            dispatch_map,
            keys: Vec::new(),
        }
    }

    /// Take ownership of an already-held read pin on `key`.
    ///
    /// The caller must have obtained the pin (via `lookup` or `take_read`) and must
    /// not release it itself.
    pub(crate) fn adopt(&mut self, key: CacheKey) {
        self.keys.push(key);
    }
}

impl Drop for PinnedKeys {
    fn drop(&mut self) {
        for key in self.keys.drain(..) {
            // Errors are not actionable here: a failed release means the entry is
            // already gone, which is the outcome we wanted.
            let _ = self.dispatch_map.release_read(key);
        }
    }
}
