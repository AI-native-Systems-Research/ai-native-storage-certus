//! LRU eviction policy component.
//!
//! Provides an O(1) LRU eviction policy via index-based doubly-linked lists.
//! Supports multiple independent pools within a single component instance,
//! allowing shared use across memory-tier (16 pools) and dispatch-map (1 pool).

mod lru_list;

use std::sync::{Mutex, RwLock};

use component_framework::define_component;
use interfaces::{
    BlockSemantics, CacheKey, EvictionHandle, EvictionPolicyError, IEvictionPolicy, ILogger, PoolId,
};

use crate::lru_list::LruList;

struct Pool {
    lru: LruList,
}

#[derive(Default)]
struct EvictionState {
    pools: Vec<Mutex<Pool>>,
}

define_component! {
    pub EvictionPolicyLruComponent {
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

impl IEvictionPolicy for EvictionPolicyLruComponent {
    fn create_pool(&self) -> PoolId {
        let mut state = self.state.write().unwrap();
        let id = state.pools.len() as u32;
        state.pools.push(Mutex::new(Pool {
            lru: LruList::new(),
        }));
        if let Ok(logger) = self.logger.get() {
            logger.debug(&format!("eviction-policy-lru: created pool {id}"));
        }
        id
    }

    fn track(
        &self,
        pool: PoolId,
        key: CacheKey,
        _semantics: BlockSemantics,
    ) -> Result<EvictionHandle, EvictionPolicyError> {
        let state = self.state.read().unwrap();
        let pool_mutex = state.pools.get(pool as usize).ok_or_else(|| {
            if let Ok(logger) = self.logger.get() {
                logger.warn(&format!(
                    "eviction-policy-lru: track on invalid pool {pool}"
                ));
            }
            EvictionPolicyError::InvalidPool(pool)
        })?;
        let mut pool_guard = pool_mutex.lock().unwrap();
        let index = pool_guard.lru.push_back(key);
        Ok(EvictionHandle::new(pool, index))
    }

    fn touch(&self, handle: EvictionHandle) -> Result<(), EvictionPolicyError> {
        let state = self.state.read().unwrap();
        let pool_mutex = state.pools.get(handle.pool_id() as usize).ok_or_else(|| {
            if let Ok(logger) = self.logger.get() {
                logger.warn(&format!(
                    "eviction-policy-lru: touch on invalid pool {}",
                    handle.pool_id()
                ));
            }
            EvictionPolicyError::InvalidPool(handle.pool_id())
        })?;
        let mut pool_guard = pool_mutex.lock().unwrap();
        pool_guard.lru.move_to_back(handle.index());
        Ok(())
    }

    fn batch_touch(&self, handles: &[EvictionHandle]) -> Result<(), EvictionPolicyError> {
        if handles.is_empty() {
            return Ok(());
        }
        let state = self.state.read().unwrap();
        let mut current_pool_id = handles[0].pool_id();
        let pool_mutex = state
            .pools
            .get(current_pool_id as usize)
            .ok_or(EvictionPolicyError::InvalidPool(current_pool_id))?;
        let mut pool_guard = pool_mutex.lock().unwrap();
        pool_guard.lru.move_to_back(handles[0].index());

        for handle in &handles[1..] {
            if handle.pool_id() != current_pool_id {
                drop(pool_guard);
                current_pool_id = handle.pool_id();
                let pm = state
                    .pools
                    .get(current_pool_id as usize)
                    .ok_or(EvictionPolicyError::InvalidPool(current_pool_id))?;
                pool_guard = pm.lock().unwrap();
            }
            pool_guard.lru.move_to_back(handle.index());
        }
        Ok(())
    }

    fn remove(&self, handle: EvictionHandle) -> Result<(), EvictionPolicyError> {
        let state = self.state.read().unwrap();
        let pool_mutex = state.pools.get(handle.pool_id() as usize).ok_or_else(|| {
            if let Ok(logger) = self.logger.get() {
                logger.warn(&format!(
                    "eviction-policy-lru: remove on invalid pool {}",
                    handle.pool_id()
                ));
            }
            EvictionPolicyError::InvalidPool(handle.pool_id())
        })?;
        let mut pool_guard = pool_mutex.lock().unwrap();
        pool_guard.lru.remove(handle.index());
        Ok(())
    }

    fn identify_next_to_evict(&self, pool: PoolId) -> Option<CacheKey> {
        let state = self.state.read().unwrap();
        let pool_mutex = state.pools.get(pool as usize)?;
        let mut pool_guard = pool_mutex.lock().unwrap();
        pool_guard.lru.pop_front()
    }

    fn get_eviction_candidates(&self, pool: PoolId, n: usize) -> Vec<CacheKey> {
        let state = self.state.read().unwrap();
        match state.pools.get(pool as usize) {
            Some(pool_mutex) => {
                let pool_guard = pool_mutex.lock().unwrap();
                pool_guard.lru.peek_front_n(n)
            }
            None => Vec::new(),
        }
    }

    fn len(&self, pool: PoolId) -> usize {
        let state = self.state.read().unwrap();
        match state.pools.get(pool as usize) {
            Some(pool_mutex) => {
                let pool_guard = pool_mutex.lock().unwrap();
                pool_guard.lru.len()
            }
            None => 0,
        }
    }

    fn clear_pool(&self, pool: PoolId) {
        let state = self.state.read().unwrap();
        if let Some(pool_mutex) = state.pools.get(pool as usize) {
            let mut pool_guard = pool_mutex.lock().unwrap();
            pool_guard.lru.clear();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use component_core::query_interface;

    fn setup() -> std::sync::Arc<EvictionPolicyLruComponent> {
        EvictionPolicyLruComponent::new_default()
    }

    #[test]
    fn create_pool_returns_sequential_ids() {
        let comp = setup();
        let ep: std::sync::Arc<dyn IEvictionPolicy + Send + Sync> =
            query_interface!(comp, IEvictionPolicy).unwrap();
        assert_eq!(ep.create_pool(), 0);
        assert_eq!(ep.create_pool(), 1);
        assert_eq!(ep.create_pool(), 2);
    }

    #[test]
    fn track_and_evict_fifo_order() {
        let comp = setup();
        let ep: std::sync::Arc<dyn IEvictionPolicy + Send + Sync> =
            query_interface!(comp, IEvictionPolicy).unwrap();
        let pool = ep.create_pool();

        ep.track(pool, 100, BlockSemantics::default()).unwrap();
        ep.track(pool, 200, BlockSemantics::default()).unwrap();
        ep.track(pool, 300, BlockSemantics::default()).unwrap();

        assert_eq!(ep.identify_next_to_evict(pool), Some(100));
        assert_eq!(ep.identify_next_to_evict(pool), Some(200));
        assert_eq!(ep.identify_next_to_evict(pool), Some(300));
        assert_eq!(ep.identify_next_to_evict(pool), None);
    }

    #[test]
    fn touch_moves_to_back() {
        let comp = setup();
        let ep: std::sync::Arc<dyn IEvictionPolicy + Send + Sync> =
            query_interface!(comp, IEvictionPolicy).unwrap();
        let pool = ep.create_pool();

        let h1 = ep.track(pool, 100, BlockSemantics::default()).unwrap();
        ep.track(pool, 200, BlockSemantics::default()).unwrap();
        ep.track(pool, 300, BlockSemantics::default()).unwrap();

        ep.touch(h1).unwrap();

        assert_eq!(ep.identify_next_to_evict(pool), Some(200));
        assert_eq!(ep.identify_next_to_evict(pool), Some(300));
        assert_eq!(ep.identify_next_to_evict(pool), Some(100));
    }

    #[test]
    fn remove_invalidates_entry() {
        let comp = setup();
        let ep: std::sync::Arc<dyn IEvictionPolicy + Send + Sync> =
            query_interface!(comp, IEvictionPolicy).unwrap();
        let pool = ep.create_pool();

        ep.track(pool, 100, BlockSemantics::default()).unwrap();
        let h2 = ep.track(pool, 200, BlockSemantics::default()).unwrap();
        ep.track(pool, 300, BlockSemantics::default()).unwrap();

        ep.remove(h2).unwrap();

        assert_eq!(ep.identify_next_to_evict(pool), Some(100));
        assert_eq!(ep.identify_next_to_evict(pool), Some(300));
        assert_eq!(ep.identify_next_to_evict(pool), None);
    }

    #[test]
    fn get_eviction_candidates_does_not_remove() {
        let comp = setup();
        let ep: std::sync::Arc<dyn IEvictionPolicy + Send + Sync> =
            query_interface!(comp, IEvictionPolicy).unwrap();
        let pool = ep.create_pool();

        ep.track(pool, 10, BlockSemantics::default()).unwrap();
        ep.track(pool, 20, BlockSemantics::default()).unwrap();
        ep.track(pool, 30, BlockSemantics::default()).unwrap();

        assert_eq!(ep.get_eviction_candidates(pool, 2), vec![10, 20]);
        assert_eq!(ep.len(pool), 3);
    }

    #[test]
    fn pools_are_independent() {
        let comp = setup();
        let ep: std::sync::Arc<dyn IEvictionPolicy + Send + Sync> =
            query_interface!(comp, IEvictionPolicy).unwrap();
        let pool_a = ep.create_pool();
        let pool_b = ep.create_pool();

        ep.track(pool_a, 1, BlockSemantics::default()).unwrap();
        ep.track(pool_a, 2, BlockSemantics::default()).unwrap();
        ep.track(pool_b, 99, BlockSemantics::default()).unwrap();

        assert_eq!(ep.len(pool_a), 2);
        assert_eq!(ep.len(pool_b), 1);
        assert_eq!(ep.identify_next_to_evict(pool_b), Some(99));
        assert_eq!(ep.len(pool_a), 2);
    }

    #[test]
    fn clear_pool_resets() {
        let comp = setup();
        let ep: std::sync::Arc<dyn IEvictionPolicy + Send + Sync> =
            query_interface!(comp, IEvictionPolicy).unwrap();
        let pool = ep.create_pool();

        ep.track(pool, 1, BlockSemantics::default()).unwrap();
        ep.track(pool, 2, BlockSemantics::default()).unwrap();
        ep.track(pool, 3, BlockSemantics::default()).unwrap();

        ep.clear_pool(pool);
        assert_eq!(ep.len(pool), 0);
        assert_eq!(ep.identify_next_to_evict(pool), None);

        // Can still track after clear
        ep.track(pool, 10, BlockSemantics::default()).unwrap();
        assert_eq!(ep.identify_next_to_evict(pool), Some(10));
    }

    #[test]
    fn invalid_pool_returns_error() {
        let comp = setup();
        let ep: std::sync::Arc<dyn IEvictionPolicy + Send + Sync> =
            query_interface!(comp, IEvictionPolicy).unwrap();

        assert!(ep.track(99, 1, BlockSemantics::default()).is_err());
        assert_eq!(ep.identify_next_to_evict(99), None);
        assert_eq!(ep.get_eviction_candidates(99, 5), Vec::<CacheKey>::new());
        assert_eq!(ep.len(99), 0);
    }

    #[test]
    fn concurrent_access() {
        use std::sync::Arc;
        use std::thread;

        let comp = setup();
        let ep: Arc<dyn IEvictionPolicy + Send + Sync> =
            query_interface!(comp, IEvictionPolicy).unwrap();
        let pool = ep.create_pool();

        let handles: Vec<_> = (0..4)
            .map(|t| {
                let ep = Arc::clone(&ep);
                thread::spawn(move || {
                    for i in 0..100 {
                        let key = (t * 1000 + i) as u64;
                        let h = ep.track(pool, key, BlockSemantics::default()).unwrap();
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
