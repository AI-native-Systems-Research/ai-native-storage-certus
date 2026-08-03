//! Session-lists eviction policy component.
//!
//! Provides the [`IEvictionPolicy`] contract with a per-instance pool model and
//! an `ILogger` receptacle. Bootstrapped from `eviction-policy-lru` as a
//! skeleton: the LRU eviction data structure has been removed and the policy
//! methods are left as `todo!()` stubs to be implemented for the session-lists
//! eviction strategy.
//!
//! Supports multiple independent pools within a single component instance,
//! allowing shared use across memory-tier (16 pools) and dispatch-map (1 pool).

use std::sync::RwLock;

use component_framework::define_component;
use interfaces::{
    CacheKey, EvictionHandle, EvictionPolicyError, IEvictionPolicy, ILogger, PoolId,
};

/// Per-pool eviction bookkeeping.
///
/// Placeholder for the session-lists data structure that will back each pool.
#[derive(Default)]
struct Pool {}

#[derive(Default)]
struct EvictionState {
    pools: Vec<Pool>,
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
        state.pools.push(Pool::default());
        if let Ok(logger) = self.logger.get() {
            logger.debug(&format!("eviction-policy-session-lists: created pool {id}"));
        }
        id
    }

    fn track(
        &self,
        _pool: PoolId,
        _key: CacheKey,
    ) -> Result<EvictionHandle, EvictionPolicyError> {
        todo!("session-lists eviction: track")
    }

    fn touch(&self, _handle: EvictionHandle) -> Result<(), EvictionPolicyError> {
        todo!("session-lists eviction: touch")
    }

    fn batch_touch(&self, _handles: &[EvictionHandle]) -> Result<(), EvictionPolicyError> {
        todo!("session-lists eviction: batch_touch")
    }

    fn remove(&self, _handle: EvictionHandle) -> Result<(), EvictionPolicyError> {
        todo!("session-lists eviction: remove")
    }

    fn identify_next_to_evict(&self, _pool: PoolId) -> Option<CacheKey> {
        todo!("session-lists eviction: identify_next_to_evict")
    }

    fn get_eviction_candidates(&self, _pool: PoolId, _n: usize) -> Vec<CacheKey> {
        todo!("session-lists eviction: get_eviction_candidates")
    }

    fn len(&self, _pool: PoolId) -> usize {
        todo!("session-lists eviction: len")
    }

    fn clear_pool(&self, _pool: PoolId) {
        todo!("session-lists eviction: clear_pool")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use component_core::query_interface;

    #[test]
    fn provides_eviction_policy_and_creates_pools() {
        let comp = EvictionPolicySessionListsComponent::new_default();
        let ep: std::sync::Arc<dyn IEvictionPolicy + Send + Sync> =
            query_interface!(comp, IEvictionPolicy).unwrap();
        assert_eq!(ep.create_pool(), 0);
        assert_eq!(ep.create_pool(), 1);
        assert_eq!(ep.create_pool(), 2);
    }
}
