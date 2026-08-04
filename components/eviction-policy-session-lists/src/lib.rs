//! Session-lineage eviction policy component.
//!
//! An alternative to plain recency-LRU that exploits *lineage* between cache
//! blocks. For each session id, blocks form a linear chain (a stack): the block
//! registered immediately after another becomes its child, recording that the
//! earlier block is its parent. Only *leaves* — blocks with no tracked child —
//! are eligible for eviction, and the victim is the globally oldest-accessed
//! leaf across every session in a pool. This preserves heads and interior
//! blocks that still have descendants, avoiding the LRU failure mode of
//! dropping a still-needed ancestor.
//!
//! Recency-LRU behaviour is recoverable from this same policy: assign each block
//! a distinct `session_id` (for example the cache key) so every block is its own
//! singleton chain and therefore always a leaf.
//!
//! [`EvictionPolicySessionListsComponent`] implements the [`IEvictionPolicy`]
//! interface, supports multiple independent pools within one instance (as used
//! by memory-tier and dispatch-map), and declares an [`ILogger`] receptacle.
//! Per-pool state is guarded by a `Mutex` behind a shared `RwLock`, so distinct
//! pools operate concurrently.
//!
//! # Registering lineage and selecting a lineage-preserving victim
//!
//! ```
//! use component_core::query_interface;
//! use interfaces::{BlockSemantics, IEvictionPolicy};
//!
//! let comp = eviction_policy_session_lists::EvictionPolicySessionListsComponent::new_default();
//! let ep = query_interface!(comp, IEvictionPolicy).unwrap();
//! let pool = ep.create_pool();
//!
//! // Session 7: chain A -> B -> C (C is the leaf).
//! let sem = BlockSemantics { session_id: 7 };
//! ep.track(pool, 0xA, sem).unwrap();
//! ep.track(pool, 0xB, sem).unwrap();
//! ep.track(pool, 0xC, sem).unwrap();
//!
//! // A second, independent session.
//! ep.track(pool, 0xD, BlockSemantics { session_id: 9 }).unwrap();
//!
//! // Only leaves are candidates; the head (0xA) is protected while it has
//! // descendants. The oldest-accessed leaf is evicted first.
//! assert_eq!(ep.identify_next_to_evict(pool), Some(0xC));
//! // With C gone, its parent B is promoted to leaf and becomes eligible.
//! assert_eq!(ep.identify_next_to_evict(pool), Some(0xB));
//! ```
//!
//! # Refreshing recency changes the victim
//!
//! ```
//! use component_core::query_interface;
//! use interfaces::{BlockSemantics, IEvictionPolicy};
//!
//! let comp = eviction_policy_session_lists::EvictionPolicySessionListsComponent::new_default();
//! let ep = query_interface!(comp, IEvictionPolicy).unwrap();
//! let pool = ep.create_pool();
//!
//! // Two singleton sessions: 0xA is older than 0xB.
//! let ha = ep.track(pool, 0xA, BlockSemantics { session_id: 1 }).unwrap();
//! let hb = ep.track(pool, 0xB, BlockSemantics { session_id: 2 }).unwrap();
//!
//! // Touch the older leaf; the untouched one is now the eviction victim.
//! ep.batch_touch(&[ha]).unwrap();
//! ep.touch(ha).unwrap();
//! let _ = hb;
//! assert_eq!(ep.identify_next_to_evict(pool), Some(0xB));
//! ```

mod session_list;

use std::sync::{Mutex, RwLock};

use component_framework::define_component;
use interfaces::{
    BlockSemantics, CacheKey, EvictionHandle, EvictionPolicyError, IEvictionPolicy, ILogger, PoolId,
};

use crate::session_list::Pool;

#[derive(Default)]
struct EvictionState {
    pools: Vec<Mutex<Pool>>,
    /// Set once the "selected as active policy" line has been logged, so the
    /// startup announcement fires exactly once regardless of how many pools
    /// memory-tier / dispatch-map create.
    announced: bool,
}

define_component! {
    pub EvictionPolicySessionListsComponent {
        version: "0.1.0",
        provides: [IEvictionPolicy],
        receptacles: {
            logger: ILogger,
        },
        fields: {
            state: RwLock<EvictionState>,
        },
    }
}

impl IEvictionPolicy for EvictionPolicySessionListsComponent {
    fn create_pool(&self) -> PoolId {
        let mut state = self.state.write().unwrap();
        let id = state.pools.len() as u32;
        state.pools.push(Mutex::new(Pool::default()));
        if let Ok(logger) = self.logger.get() {
            // One-time, info-level so it is visible in certus-server-yaml's
            // default (Info) output — confirms this session-lineage policy was
            // selected instead of the plain LRU policy.
            if !state.announced {
                logger.info(
                    "eviction-policy-session-lists: selected as active eviction policy \
                     (session-lineage — only leaf blocks are evictable)",
                );
                state.announced = true;
            }
            logger.debug(&format!("eviction-policy-session-lists: created pool {id}"));
        }
        id
    }

    fn track(
        &self,
        pool: PoolId,
        key: CacheKey,
        semantics: BlockSemantics,
    ) -> Result<EvictionHandle, EvictionPolicyError> {
        let state = self.state.read().unwrap();
        let pool_mutex = state.pools.get(pool as usize).ok_or_else(|| {
            if let Ok(logger) = self.logger.get() {
                logger.warn(&format!(
                    "eviction-policy-session-lists: track on invalid pool {pool}"
                ));
            }
            EvictionPolicyError::InvalidPool(pool)
        })?;
        let mut pool_guard = pool_mutex.lock().unwrap();
        let index = pool_guard.register(key, semantics.session_id);
        Ok(EvictionHandle::new(pool, index))
    }

    fn touch(&self, handle: EvictionHandle) -> Result<(), EvictionPolicyError> {
        let state = self.state.read().unwrap();
        let pool_mutex = state
            .pools
            .get(handle.pool_id() as usize)
            .ok_or(EvictionPolicyError::InvalidPool(handle.pool_id()))?;
        let mut pool_guard = pool_mutex.lock().unwrap();
        if pool_guard.touch(handle.index()) {
            Ok(())
        } else {
            Err(EvictionPolicyError::InvalidHandle)
        }
    }

    fn batch_touch(&self, handles: &[EvictionHandle]) -> Result<(), EvictionPolicyError> {
        if handles.is_empty() {
            return Ok(());
        }
        let state = self.state.read().unwrap();
        let mut current_pool_id = handles[0].pool_id();
        let mut pool_mutex = state
            .pools
            .get(current_pool_id as usize)
            .ok_or(EvictionPolicyError::InvalidPool(current_pool_id))?;
        let mut pool_guard = pool_mutex.lock().unwrap();

        for handle in handles {
            if handle.pool_id() != current_pool_id {
                drop(pool_guard);
                current_pool_id = handle.pool_id();
                pool_mutex = state
                    .pools
                    .get(current_pool_id as usize)
                    .ok_or(EvictionPolicyError::InvalidPool(current_pool_id))?;
                pool_guard = pool_mutex.lock().unwrap();
            }
            if !pool_guard.touch(handle.index()) {
                return Err(EvictionPolicyError::InvalidHandle);
            }
        }
        Ok(())
    }

    fn remove(&self, handle: EvictionHandle) -> Result<(), EvictionPolicyError> {
        let state = self.state.read().unwrap();
        let pool_mutex = state
            .pools
            .get(handle.pool_id() as usize)
            .ok_or(EvictionPolicyError::InvalidPool(handle.pool_id()))?;
        let mut pool_guard = pool_mutex.lock().unwrap();
        if pool_guard.remove(handle.index()) {
            Ok(())
        } else {
            Err(EvictionPolicyError::InvalidHandle)
        }
    }

    fn identify_next_to_evict(&self, pool: PoolId) -> Option<CacheKey> {
        let state = self.state.read().unwrap();
        let pool_mutex = state.pools.get(pool as usize)?;
        let mut pool_guard = pool_mutex.lock().unwrap();
        pool_guard.evict_oldest()
    }

    fn get_eviction_candidates(&self, pool: PoolId, n: usize) -> Vec<CacheKey> {
        let state = self.state.read().unwrap();
        match state.pools.get(pool as usize) {
            Some(pool_mutex) => pool_mutex.lock().unwrap().candidates(n),
            None => Vec::new(),
        }
    }

    fn len(&self, pool: PoolId) -> usize {
        let state = self.state.read().unwrap();
        match state.pools.get(pool as usize) {
            Some(pool_mutex) => pool_mutex.lock().unwrap().len(),
            None => 0,
        }
    }

    fn clear_pool(&self, pool: PoolId) {
        let state = self.state.read().unwrap();
        if let Some(pool_mutex) = state.pools.get(pool as usize) {
            pool_mutex.lock().unwrap().clear();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use component_core::query_interface;
    use std::sync::Arc;

    fn ep() -> Arc<dyn IEvictionPolicy + Send + Sync> {
        let comp = EvictionPolicySessionListsComponent::new_default();
        query_interface!(comp, IEvictionPolicy).unwrap()
    }

    #[test]
    fn provides_eviction_policy_and_creates_pools() {
        let ep = ep();
        assert_eq!(ep.create_pool(), 0);
        assert_eq!(ep.create_pool(), 1);
        assert_eq!(ep.create_pool(), 2);
    }

    #[test]
    fn invalid_pool_is_reported() {
        let ep = ep();
        assert_eq!(
            ep.track(99, 1, BlockSemantics::default()),
            Err(EvictionPolicyError::InvalidPool(99))
        );
        assert_eq!(ep.identify_next_to_evict(99), None);
        assert_eq!(ep.get_eviction_candidates(99, 5), Vec::<CacheKey>::new());
        assert_eq!(ep.len(99), 0);
    }

    #[test]
    fn full_lifecycle_across_two_pools() {
        let ep = ep();
        let pa = ep.create_pool();
        let pb = ep.create_pool();

        // Pool A: one session chain 1 -> 2 -> 3.
        let sem = BlockSemantics { session_id: 42 };
        let h1 = ep.track(pa, 1, sem).unwrap();
        ep.track(pa, 2, sem).unwrap();
        ep.track(pa, 3, sem).unwrap();
        assert_eq!(ep.len(pa), 3);

        // Pool B is independent.
        ep.track(pb, 100, BlockSemantics { session_id: 1 }).unwrap();
        ep.track(pb, 200, BlockSemantics { session_id: 2 }).unwrap();
        assert_eq!(ep.len(pb), 2);

        // Touch head of A's chain (has descendants) — allowed, no leaf change.
        ep.touch(h1).unwrap();
        // Leaf of A is key 3.
        assert_eq!(ep.get_eviction_candidates(pa, 4), vec![3]);
        assert_eq!(ep.identify_next_to_evict(pa), Some(3));
        // Now key 2 is the leaf.
        assert_eq!(ep.identify_next_to_evict(pa), Some(2));

        // Remove remaining head of A explicitly.
        let h_head = ep.track(pa, 1, sem).unwrap(); // idempotent -> existing handle
        ep.remove(h_head).unwrap();
        assert_eq!(ep.len(pa), 0);
        assert_eq!(ep.identify_next_to_evict(pa), None);

        // Pool B untouched by pool A operations.
        assert_eq!(ep.len(pb), 2);

        ep.clear_pool(pb);
        assert_eq!(ep.len(pb), 0);
    }

    #[test]
    fn batch_touch_reorders_victims() {
        let ep = ep();
        let pool = ep.create_pool();
        let ha = ep
            .track(pool, 10, BlockSemantics { session_id: 1 })
            .unwrap();
        let hb = ep
            .track(pool, 20, BlockSemantics { session_id: 2 })
            .unwrap();
        let hc = ep
            .track(pool, 30, BlockSemantics { session_id: 3 })
            .unwrap();

        // Refresh 10 then 20; 30 becomes the oldest leaf.
        ep.batch_touch(&[ha, hb]).unwrap();
        assert_eq!(ep.identify_next_to_evict(pool), Some(30));
        let _ = hc;

        // Batch touch surfacing an invalid handle errors.
        ep.remove(ha).unwrap();
        assert_eq!(
            ep.batch_touch(&[ha]),
            Err(EvictionPolicyError::InvalidHandle)
        );
    }

    #[test]
    fn touch_and_remove_reject_invalid_handles() {
        let ep = ep();
        let pool = ep.create_pool();
        let h = ep.track(pool, 1, BlockSemantics::default()).unwrap();
        ep.remove(h).unwrap();
        assert_eq!(ep.touch(h), Err(EvictionPolicyError::InvalidHandle));
        assert_eq!(ep.remove(h), Err(EvictionPolicyError::InvalidHandle));
        assert_eq!(
            ep.touch(EvictionHandle::new(99, 0)),
            Err(EvictionPolicyError::InvalidPool(99))
        );
    }

    #[test]
    fn concurrent_pool_access() {
        use std::thread;

        let ep = ep();
        let pool = ep.create_pool();
        let handles: Vec<_> = (0..4)
            .map(|t| {
                let ep = Arc::clone(&ep);
                thread::spawn(move || {
                    for i in 0..100u64 {
                        let key = t * 1000 + i;
                        // Distinct session per block => recency-LRU behaviour.
                        let h = ep
                            .track(pool, key, BlockSemantics { session_id: key })
                            .unwrap();
                        ep.touch(h).unwrap();
                    }
                })
            })
            .collect();
        for h in handles {
            h.join().unwrap();
        }
        assert_eq!(ep.len(pool), 400);
    }
}
