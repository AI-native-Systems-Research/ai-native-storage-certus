//! Regression test for MT-INSERT-TRACK-FAILURE-PANICS: when the eviction
//! policy refuses to track a new key, `insert` must fail cleanly (no panic,
//! no reserved space, no recorded slot) and leave the component usable.

use super::*;
use component_core::query_interface;
use std::sync::Arc;

fn setup() -> Arc<MemoryTierComponent> {
    let ep_comp = eviction_policy_lru::EvictionPolicyLruComponent::new_default();
    let ep: Arc<dyn IEvictionPolicy + Send + Sync> =
        query_interface!(ep_comp, IEvictionPolicy).unwrap();

    let c = MemoryTierComponent::new(RwLock::new(MemoryTierState::default()));
    c.eviction_policy.connect(ep).unwrap();
    let mt = query_interface!(c, IMemoryTier).unwrap();
    mt.initialize(64 * 4096, None).unwrap();
    c
}

#[test]
fn insert_fails_cleanly_when_policy_refuses_to_track() {
    let c = setup();
    let mt = query_interface!(c, IMemoryTier).unwrap();
    mt.insert(1, 4096).unwrap();
    let used_before = mt.used();

    // Point the component at a pool the bound policy never created, so
    // `track` refuses with `InvalidPool` (as it would after the receptacle
    // was re-bound to another policy instance).
    let real_pool = {
        let mut st = c.state.write().unwrap();
        let p = st.pool_id;
        st.pool_id = p + 1000;
        p
    };

    let r = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| mt.insert(2, 4096)));
    let r = r.expect("insert panicked when the eviction policy refused to track the key");
    assert!(r.is_err(), "insert must fail when the policy refuses");
    assert_eq!(mt.used(), used_before, "no pool space may stay reserved");
    assert!(
        !mt.contains(2),
        "no slot may be recorded for the refused key"
    );

    // The component is still usable: no lock was poisoned.
    c.state.write().unwrap().pool_id = real_pool;
    mt.insert(2, 4096).unwrap();
    assert!(mt.contains(2));
    assert_eq!(mt.used(), used_before + 4096);
}
