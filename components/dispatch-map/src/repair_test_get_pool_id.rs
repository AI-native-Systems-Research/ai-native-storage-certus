//! Repair test for finding `get-pool-id-panic`: with no eviction policy connected,
//! `oldest_keys`, `create_memory_tier_entry` and `recover_extent` must not panic.

use super::*;
use std::sync::Arc;

/// A dispatch map with no receptacle connected at all.
fn unbound_component() -> Arc<DispatchMapComponent> {
    DispatchMapComponent::new(DispatchMapState::new())
}

#[test]
fn oldest_keys_without_eviction_policy_returns_empty() {
    let c = unbound_component();
    assert!(c.oldest_keys(4).is_empty());
}

#[test]
fn create_memory_tier_entry_without_eviction_policy_errors() {
    let c = unbound_component();
    let mut buf = vec![0u8; 4096];
    let r = c.create_memory_tier_entry(7, buf.as_mut_ptr(), 4096);
    assert!(matches!(r, Err(DispatchMapError::NotInitialized(_))));
    assert!(matches!(
        c.entry_size(7),
        Err(DispatchMapError::KeyNotFound(7))
    ));
}

#[test]
fn recover_extent_without_eviction_policy_errors() {
    let c = unbound_component();
    let r = c.recover_extent(9, 4096, 1);
    assert!(matches!(r, Err(DispatchMapError::NotInitialized(_))));
    assert!(matches!(
        c.entry_size(9),
        Err(DispatchMapError::KeyNotFound(9))
    ));
}

/// Same root cause with a pool id already cached: the policy was connected when the pool was
/// created and then disconnected, so the direct receptacle unwrap is the one that would panic.
#[test]
fn create_and_recover_after_disconnect_error() {
    let ep_comp = eviction_policy_lru::EvictionPolicyLruComponent::new_default();
    let ep: Arc<dyn IEvictionPolicy + Send + Sync> =
        component_core::query_interface!(ep_comp, IEvictionPolicy).unwrap();
    let c = unbound_component();
    c.eviction_policy.connect(ep).unwrap();
    assert!(c.oldest_keys(1).is_empty()); // creates and caches the pool id
    c.eviction_policy.disconnect().unwrap();

    let mut buf = vec![0u8; 4096];
    let r = c.create_memory_tier_entry(7, buf.as_mut_ptr(), 4096);
    assert!(matches!(r, Err(DispatchMapError::NotInitialized(_))));
    let r = c.recover_extent(9, 4096, 1);
    assert!(matches!(r, Err(DispatchMapError::NotInitialized(_))));
    assert!(c.oldest_keys(1).is_empty());
    assert!(matches!(
        c.entry_size(7),
        Err(DispatchMapError::KeyNotFound(7))
    ));
    assert!(matches!(
        c.entry_size(9),
        Err(DispatchMapError::KeyNotFound(9))
    ));
}
