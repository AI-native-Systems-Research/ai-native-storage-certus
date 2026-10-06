// initialize, and the component-wide invariants. Invariant harnesses are INDUCTIVE where the
// obligation is a maintained invariant: symbolic pre-state satisfying it (kani::assume), ONE
// operation chosen symbolically from all 20 IDispatchMap methods with symbolic arguments
// (mocks::run_op), assert it again. Concurrency-phrased invariants are checked as their
// sequential projection and say so.
use crate::entry::{DispatchEntry, Location};
use crate::mocks::*;
use crate::*;
use interfaces::{DispatchMapError, Extent, IDispatchMap, LookupResult};

/// DM-INITIALIZE-RECOVERS-ALL-EXTENTS (under D-RANGE-FR-012-437b1e, D-RANGE-FR-020-fd9bf8)
#[kani::proof]
#[kani::stub(std::collections::hash_map::RandomState::new, crate::mocks::concrete_state)]
#[kani::stub(std::collections::HashMap::get, crate::hashmap_model::get)]
#[kani::stub(std::collections::HashMap::get_mut, crate::hashmap_model::get_mut)]
#[kani::stub(std::collections::HashMap::insert, crate::hashmap_model::insert)]
#[kani::stub(std::collections::HashMap::remove, crate::hashmap_model::remove)]
#[kani::stub(std::collections::HashMap::contains_key, crate::hashmap_model::contains_key)]
#[kani::stub(std::collections::HashMap::len, crate::hashmap_model::len)]
#[kani::stub(std::time::Instant::now, crate::mocks::instant_now)]
#[kani::stub(std::sync::Condvar::wait_timeout, crate::mocks::condvar_wait_timeout)]
#[kani::stub(std::sync::Condvar::notify_all, crate::mocks::condvar_notify_all)]
#[kani::stub(std::fmt::format, crate::mocks::fmt_format)]
#[kani::unwind(4)]
fn verify_dm_initialize_recovers_all_extents() {
    let (c, ep) = component_with_ep(); // D-RANGE-FR-012-437b1e: an eviction policy is connected
    let ex = any_extents();            // n <= 2 extents, distinct keys 1 and 2
    let n = ex.len();
    let (e0, e1) = (ex.get(0).cloned(), ex.get(1).cloned());
    connect_em(&c, ex);                // D-RANGE-FR-020-fd9bf8: the map holds none of those keys

    let r = c.initialize();
    assert!(r.is_ok());
    kani::cover!(n == 2);
    for e in [e0, e1] {
        if let Some(e) = e {
            let s = snap(&c, e.key).unwrap();
            assert!(!s.is_mem && s.offset == e.offset);
            match c.lookup(e.key) {
                Ok(LookupResult::BlockDevice { offset }) => assert!(offset == e.offset),
                _ => assert!(false),
            }
        }
    }
    assert!(len(&c) == n);
}

#[kani::proof]
#[kani::stub(std::collections::hash_map::RandomState::new, crate::mocks::concrete_state)]
#[kani::stub(std::collections::HashMap::get, crate::hashmap_model::get)]
#[kani::stub(std::collections::HashMap::get_mut, crate::hashmap_model::get_mut)]
#[kani::stub(std::collections::HashMap::insert, crate::hashmap_model::insert)]
#[kani::stub(std::collections::HashMap::remove, crate::hashmap_model::remove)]
#[kani::stub(std::collections::HashMap::contains_key, crate::hashmap_model::contains_key)]
#[kani::stub(std::collections::HashMap::len, crate::hashmap_model::len)]
#[kani::stub(std::time::Instant::now, crate::mocks::instant_now)]
#[kani::stub(std::sync::Condvar::wait_timeout, crate::mocks::condvar_wait_timeout)]
#[kani::stub(std::sync::Condvar::notify_all, crate::mocks::condvar_notify_all)]
#[kani::stub(std::fmt::format, crate::mocks::fmt_format)]
#[kani::unwind(4)]
fn verify_dm_initialize_recovers_all_extents__mutant() {
    let (c, ep) = component_with_ep(); // D-RANGE-FR-012-437b1e: an eviction policy is connected
    let ex = any_extents();            // n <= 2 extents, distinct keys 1 and 2
    let n = ex.len();
    let (e0, e1) = (ex.get(0).cloned(), ex.get(1).cloned());
    connect_em(&c, ex);                // D-RANGE-FR-020-fd9bf8: the map holds none of those keys

    let _ = c.initialize();
    assert!(len(&c) == 0); // MUTANT
}

/// DM-INITIALIZE-RECOVERED-SIZE
#[kani::proof]
#[kani::stub(std::collections::hash_map::RandomState::new, crate::mocks::concrete_state)]
#[kani::stub(std::collections::HashMap::get, crate::hashmap_model::get)]
#[kani::stub(std::collections::HashMap::get_mut, crate::hashmap_model::get_mut)]
#[kani::stub(std::collections::HashMap::insert, crate::hashmap_model::insert)]
#[kani::stub(std::collections::HashMap::remove, crate::hashmap_model::remove)]
#[kani::stub(std::collections::HashMap::contains_key, crate::hashmap_model::contains_key)]
#[kani::stub(std::collections::HashMap::len, crate::hashmap_model::len)]
#[kani::stub(std::time::Instant::now, crate::mocks::instant_now)]
#[kani::stub(std::sync::Condvar::wait_timeout, crate::mocks::condvar_wait_timeout)]
#[kani::stub(std::sync::Condvar::notify_all, crate::mocks::condvar_notify_all)]
#[kani::stub(std::fmt::format, crate::mocks::fmt_format)]
#[kani::unwind(4)]
fn verify_dm_initialize_recovered_size() {
    let (c, ep) = component_with_ep(); // D-RANGE-FR-012-437b1e: an eviction policy is connected
    let ex = any_extents();            // n <= 2 extents, distinct keys 1 and 2
    let n = ex.len();
    let (e0, e1) = (ex.get(0).cloned(), ex.get(1).cloned());
    connect_em(&c, ex);                // D-RANGE-FR-020-fd9bf8: the map holds none of those keys

    assert!(c.initialize().is_ok());
    kani::cover!(n >= 1);
    for e in [e0, e1] {
        if let Some(e) = e {
            assert!(snap(&c, e.key).unwrap().size_blocks == e.size);
        }
    }
}

#[kani::proof]
#[kani::stub(std::collections::hash_map::RandomState::new, crate::mocks::concrete_state)]
#[kani::stub(std::collections::HashMap::get, crate::hashmap_model::get)]
#[kani::stub(std::collections::HashMap::get_mut, crate::hashmap_model::get_mut)]
#[kani::stub(std::collections::HashMap::insert, crate::hashmap_model::insert)]
#[kani::stub(std::collections::HashMap::remove, crate::hashmap_model::remove)]
#[kani::stub(std::collections::HashMap::contains_key, crate::hashmap_model::contains_key)]
#[kani::stub(std::collections::HashMap::len, crate::hashmap_model::len)]
#[kani::stub(std::time::Instant::now, crate::mocks::instant_now)]
#[kani::stub(std::sync::Condvar::wait_timeout, crate::mocks::condvar_wait_timeout)]
#[kani::stub(std::sync::Condvar::notify_all, crate::mocks::condvar_notify_all)]
#[kani::stub(std::fmt::format, crate::mocks::fmt_format)]
#[kani::unwind(4)]
fn verify_dm_initialize_recovered_size__mutant() {
    let (c, ep) = component_with_ep(); // D-RANGE-FR-012-437b1e: an eviction policy is connected
    let ex = any_extents();            // n <= 2 extents, distinct keys 1 and 2
    let n = ex.len();
    let (e0, e1) = (ex.get(0).cloned(), ex.get(1).cloned());
    connect_em(&c, ex);                // D-RANGE-FR-020-fd9bf8: the map holds none of those keys

    let _ = c.initialize();
    if let Some(e) = e0 {
        assert!(snap(&c, e.key).unwrap().size_blocks != e.size); // MUTANT
    }
}

/// DM-INITIALIZE-EMPTY-EXTENT-MANAGER
#[kani::proof]
#[kani::stub(std::collections::hash_map::RandomState::new, crate::mocks::concrete_state)]
#[kani::stub(std::collections::HashMap::get, crate::hashmap_model::get)]
#[kani::stub(std::collections::HashMap::get_mut, crate::hashmap_model::get_mut)]
#[kani::stub(std::collections::HashMap::insert, crate::hashmap_model::insert)]
#[kani::stub(std::collections::HashMap::remove, crate::hashmap_model::remove)]
#[kani::stub(std::collections::HashMap::contains_key, crate::hashmap_model::contains_key)]
#[kani::stub(std::collections::HashMap::len, crate::hashmap_model::len)]
#[kani::stub(std::time::Instant::now, crate::mocks::instant_now)]
#[kani::stub(std::sync::Condvar::wait_timeout, crate::mocks::condvar_wait_timeout)]
#[kani::stub(std::sync::Condvar::notify_all, crate::mocks::condvar_notify_all)]
#[kani::stub(std::fmt::format, crate::mocks::fmt_format)]
#[kani::unwind(4)]
fn verify_dm_initialize_empty_extent_manager() {
    let (c, ep) = component_with_ep();
    connect_em(&c, Vec::new());
    assert!(c.initialize().is_ok());
    assert!(len(&c) == 0);
}

#[kani::proof]
#[kani::stub(std::collections::hash_map::RandomState::new, crate::mocks::concrete_state)]
#[kani::stub(std::collections::HashMap::get, crate::hashmap_model::get)]
#[kani::stub(std::collections::HashMap::get_mut, crate::hashmap_model::get_mut)]
#[kani::stub(std::collections::HashMap::insert, crate::hashmap_model::insert)]
#[kani::stub(std::collections::HashMap::remove, crate::hashmap_model::remove)]
#[kani::stub(std::collections::HashMap::contains_key, crate::hashmap_model::contains_key)]
#[kani::stub(std::collections::HashMap::len, crate::hashmap_model::len)]
#[kani::stub(std::time::Instant::now, crate::mocks::instant_now)]
#[kani::stub(std::sync::Condvar::wait_timeout, crate::mocks::condvar_wait_timeout)]
#[kani::stub(std::sync::Condvar::notify_all, crate::mocks::condvar_notify_all)]
#[kani::stub(std::fmt::format, crate::mocks::fmt_format)]
#[kani::unwind(4)]
fn verify_dm_initialize_empty_extent_manager__mutant() {
    let (c, ep) = component_with_ep();
    connect_em(&c, Vec::new());
    assert!(c.initialize().is_err()); // MUTANT
}

/// DM-INITIALIZE-NO-EXTENT-MANAGER-OK
#[kani::proof]
#[kani::stub(std::collections::hash_map::RandomState::new, crate::mocks::concrete_state)]
#[kani::stub(std::collections::HashMap::get, crate::hashmap_model::get)]
#[kani::stub(std::collections::HashMap::get_mut, crate::hashmap_model::get_mut)]
#[kani::stub(std::collections::HashMap::insert, crate::hashmap_model::insert)]
#[kani::stub(std::collections::HashMap::remove, crate::hashmap_model::remove)]
#[kani::stub(std::collections::HashMap::contains_key, crate::hashmap_model::contains_key)]
#[kani::stub(std::collections::HashMap::len, crate::hashmap_model::len)]
#[kani::stub(std::time::Instant::now, crate::mocks::instant_now)]
#[kani::stub(std::sync::Condvar::wait_timeout, crate::mocks::condvar_wait_timeout)]
#[kani::stub(std::sync::Condvar::notify_all, crate::mocks::condvar_notify_all)]
#[kani::stub(std::fmt::format, crate::mocks::fmt_format)]
#[kani::unwind(4)]
fn verify_dm_initialize_no_extent_manager_ok() {
    let (c, ep) = component_with_ep();
    assert!(c.initialize().is_ok());
    assert!(len(&c) == 0);
}

#[kani::proof]
#[kani::stub(std::collections::hash_map::RandomState::new, crate::mocks::concrete_state)]
#[kani::stub(std::collections::HashMap::get, crate::hashmap_model::get)]
#[kani::stub(std::collections::HashMap::get_mut, crate::hashmap_model::get_mut)]
#[kani::stub(std::collections::HashMap::insert, crate::hashmap_model::insert)]
#[kani::stub(std::collections::HashMap::remove, crate::hashmap_model::remove)]
#[kani::stub(std::collections::HashMap::contains_key, crate::hashmap_model::contains_key)]
#[kani::stub(std::collections::HashMap::len, crate::hashmap_model::len)]
#[kani::stub(std::time::Instant::now, crate::mocks::instant_now)]
#[kani::stub(std::sync::Condvar::wait_timeout, crate::mocks::condvar_wait_timeout)]
#[kani::stub(std::sync::Condvar::notify_all, crate::mocks::condvar_notify_all)]
#[kani::stub(std::fmt::format, crate::mocks::fmt_format)]
#[kani::unwind(4)]
fn verify_dm_initialize_no_extent_manager_ok__mutant() {
    let (c, ep) = component_with_ep();
    assert!(c.initialize().is_err()); // MUTANT
}

/// DM-INITIALIZE-RESTORED-ZERO-REFS
#[kani::proof]
#[kani::stub(std::collections::hash_map::RandomState::new, crate::mocks::concrete_state)]
#[kani::stub(std::collections::HashMap::get, crate::hashmap_model::get)]
#[kani::stub(std::collections::HashMap::get_mut, crate::hashmap_model::get_mut)]
#[kani::stub(std::collections::HashMap::insert, crate::hashmap_model::insert)]
#[kani::stub(std::collections::HashMap::remove, crate::hashmap_model::remove)]
#[kani::stub(std::collections::HashMap::contains_key, crate::hashmap_model::contains_key)]
#[kani::stub(std::collections::HashMap::len, crate::hashmap_model::len)]
#[kani::stub(std::time::Instant::now, crate::mocks::instant_now)]
#[kani::stub(std::sync::Condvar::wait_timeout, crate::mocks::condvar_wait_timeout)]
#[kani::stub(std::sync::Condvar::notify_all, crate::mocks::condvar_notify_all)]
#[kani::stub(std::fmt::format, crate::mocks::fmt_format)]
#[kani::unwind(4)]
fn verify_dm_initialize_restored_zero_refs() {
    let (c, ep) = component_with_ep(); // D-RANGE-FR-012-437b1e: an eviction policy is connected
    let ex = any_extents();            // n <= 2 extents, distinct keys 1 and 2
    let n = ex.len();
    let (e0, e1) = (ex.get(0).cloned(), ex.get(1).cloned());
    connect_em(&c, ex);                // D-RANGE-FR-020-fd9bf8: the map holds none of those keys

    assert!(c.initialize().is_ok());
    kani::cover!(n >= 1);
    for e in [e0, e1] {
        if let Some(e) = e {
            let s = snap(&c, e.key).unwrap();
            assert!(s.read_ref == 0 && s.write_ref == 0);
        }
    }
}

#[kani::proof]
#[kani::stub(std::collections::hash_map::RandomState::new, crate::mocks::concrete_state)]
#[kani::stub(std::collections::HashMap::get, crate::hashmap_model::get)]
#[kani::stub(std::collections::HashMap::get_mut, crate::hashmap_model::get_mut)]
#[kani::stub(std::collections::HashMap::insert, crate::hashmap_model::insert)]
#[kani::stub(std::collections::HashMap::remove, crate::hashmap_model::remove)]
#[kani::stub(std::collections::HashMap::contains_key, crate::hashmap_model::contains_key)]
#[kani::stub(std::collections::HashMap::len, crate::hashmap_model::len)]
#[kani::stub(std::time::Instant::now, crate::mocks::instant_now)]
#[kani::stub(std::sync::Condvar::wait_timeout, crate::mocks::condvar_wait_timeout)]
#[kani::stub(std::sync::Condvar::notify_all, crate::mocks::condvar_notify_all)]
#[kani::stub(std::fmt::format, crate::mocks::fmt_format)]
#[kani::unwind(4)]
fn verify_dm_initialize_restored_zero_refs__mutant() {
    let (c, ep) = component_with_ep(); // D-RANGE-FR-012-437b1e: an eviction policy is connected
    let ex = any_extents();            // n <= 2 extents, distinct keys 1 and 2
    let n = ex.len();
    let (e0, e1) = (ex.get(0).cloned(), ex.get(1).cloned());
    connect_em(&c, ex);                // D-RANGE-FR-020-fd9bf8: the map holds none of those keys

    let _ = c.initialize();
    if let Some(e) = e0 {
        assert!(snap(&c, e.key).unwrap().write_ref == 1); // MUTANT
    }
}

/// DM-INITIALIZE-TRACKS-EVICTION
#[kani::proof]
#[kani::stub(std::collections::hash_map::RandomState::new, crate::mocks::concrete_state)]
#[kani::stub(std::collections::HashMap::get, crate::hashmap_model::get)]
#[kani::stub(std::collections::HashMap::get_mut, crate::hashmap_model::get_mut)]
#[kani::stub(std::collections::HashMap::insert, crate::hashmap_model::insert)]
#[kani::stub(std::collections::HashMap::remove, crate::hashmap_model::remove)]
#[kani::stub(std::collections::HashMap::contains_key, crate::hashmap_model::contains_key)]
#[kani::stub(std::collections::HashMap::len, crate::hashmap_model::len)]
#[kani::stub(std::time::Instant::now, crate::mocks::instant_now)]
#[kani::stub(std::sync::Condvar::wait_timeout, crate::mocks::condvar_wait_timeout)]
#[kani::stub(std::sync::Condvar::notify_all, crate::mocks::condvar_notify_all)]
#[kani::stub(std::fmt::format, crate::mocks::fmt_format)]
#[kani::unwind(4)]
fn verify_dm_initialize_tracks_eviction() {
    let (c, ep) = component_with_ep(); // D-RANGE-FR-012-437b1e: an eviction policy is connected
    let ex = any_extents();            // n <= 2 extents, distinct keys 1 and 2
    let n = ex.len();
    let (e0, e1) = (ex.get(0).cloned(), ex.get(1).cloned());
    connect_em(&c, ex);                // D-RANGE-FR-020-fd9bf8: the map holds none of those keys

    assert!(c.initialize().is_ok());
    let pool = pool_of(&c).unwrap();
    assert!(pool == ep.pool()); // the dispatch map's own pool
    kani::cover!(n == 2);
    for e in [e0, e1] {
        if let Some(e) = e {
            assert!(ep.handle_of(pool, e.key) == Some(snap(&c, e.key).unwrap().handle));
        }
    }
}

#[kani::proof]
#[kani::stub(std::collections::hash_map::RandomState::new, crate::mocks::concrete_state)]
#[kani::stub(std::collections::HashMap::get, crate::hashmap_model::get)]
#[kani::stub(std::collections::HashMap::get_mut, crate::hashmap_model::get_mut)]
#[kani::stub(std::collections::HashMap::insert, crate::hashmap_model::insert)]
#[kani::stub(std::collections::HashMap::remove, crate::hashmap_model::remove)]
#[kani::stub(std::collections::HashMap::contains_key, crate::hashmap_model::contains_key)]
#[kani::stub(std::collections::HashMap::len, crate::hashmap_model::len)]
#[kani::stub(std::time::Instant::now, crate::mocks::instant_now)]
#[kani::stub(std::sync::Condvar::wait_timeout, crate::mocks::condvar_wait_timeout)]
#[kani::stub(std::sync::Condvar::notify_all, crate::mocks::condvar_notify_all)]
#[kani::stub(std::fmt::format, crate::mocks::fmt_format)]
#[kani::unwind(4)]
fn verify_dm_initialize_tracks_eviction__mutant() {
    let (c, ep) = component_with_ep(); // D-RANGE-FR-012-437b1e: an eviction policy is connected
    let ex = any_extents();            // n <= 2 extents, distinct keys 1 and 2
    let n = ex.len();
    let (e0, e1) = (ex.get(0).cloned(), ex.get(1).cloned());
    connect_em(&c, ex);                // D-RANGE-FR-020-fd9bf8: the map holds none of those keys

    let _ = c.initialize();
    kani::assume(n >= 1);
    assert!(ep.track_calls() == 0); // MUTANT
}

/// DM-INITIALIZE-FRAME-UNREPORTED-KEYS
#[kani::proof]
#[kani::stub(std::collections::hash_map::RandomState::new, crate::mocks::concrete_state)]
#[kani::stub(std::collections::HashMap::get, crate::hashmap_model::get)]
#[kani::stub(std::collections::HashMap::get_mut, crate::hashmap_model::get_mut)]
#[kani::stub(std::collections::HashMap::insert, crate::hashmap_model::insert)]
#[kani::stub(std::collections::HashMap::remove, crate::hashmap_model::remove)]
#[kani::stub(std::collections::HashMap::contains_key, crate::hashmap_model::contains_key)]
#[kani::stub(std::collections::HashMap::len, crate::hashmap_model::len)]
#[kani::stub(std::time::Instant::now, crate::mocks::instant_now)]
#[kani::stub(std::sync::Condvar::wait_timeout, crate::mocks::condvar_wait_timeout)]
#[kani::stub(std::sync::Condvar::notify_all, crate::mocks::condvar_notify_all)]
#[kani::stub(std::fmt::format, crate::mocks::fmt_format)]
#[kani::unwind(4)]
fn verify_dm_initialize_frame_unreported_keys() {
    let (c, ep) = component_with_ep();
    let pool = ensure_pool(&c, &ep);
    seed_tracked(&c, &ep, pool, 9, any_entry()); // key 9 is never reported (reported keys: 1, 2)
    let b = snap(&c, 9).unwrap();
    connect_em(&c, any_extents());
    assert!(c.initialize().is_ok());
    assert!(snap(&c, 9).unwrap() == b);
}

#[kani::proof]
#[kani::stub(std::collections::hash_map::RandomState::new, crate::mocks::concrete_state)]
#[kani::stub(std::collections::HashMap::get, crate::hashmap_model::get)]
#[kani::stub(std::collections::HashMap::get_mut, crate::hashmap_model::get_mut)]
#[kani::stub(std::collections::HashMap::insert, crate::hashmap_model::insert)]
#[kani::stub(std::collections::HashMap::remove, crate::hashmap_model::remove)]
#[kani::stub(std::collections::HashMap::contains_key, crate::hashmap_model::contains_key)]
#[kani::stub(std::collections::HashMap::len, crate::hashmap_model::len)]
#[kani::stub(std::time::Instant::now, crate::mocks::instant_now)]
#[kani::stub(std::sync::Condvar::wait_timeout, crate::mocks::condvar_wait_timeout)]
#[kani::stub(std::sync::Condvar::notify_all, crate::mocks::condvar_notify_all)]
#[kani::stub(std::fmt::format, crate::mocks::fmt_format)]
#[kani::unwind(4)]
fn verify_dm_initialize_frame_unreported_keys__mutant() {
    let (c, ep) = component_with_ep();
    let pool = ensure_pool(&c, &ep);
    seed_tracked(&c, &ep, pool, 9, any_entry());
    connect_em(&c, any_extents());
    let _ = c.initialize();
    assert!(snap(&c, 9).is_none()); // MUTANT
}

/// DM-INITIALIZE-EXPLICIT-ONLY
#[kani::proof]
#[kani::stub(std::collections::hash_map::RandomState::new, crate::mocks::concrete_state)]
#[kani::stub(std::collections::HashMap::get, crate::hashmap_model::get)]
#[kani::stub(std::collections::HashMap::get_mut, crate::hashmap_model::get_mut)]
#[kani::stub(std::collections::HashMap::insert, crate::hashmap_model::insert)]
#[kani::stub(std::collections::HashMap::remove, crate::hashmap_model::remove)]
#[kani::stub(std::collections::HashMap::contains_key, crate::hashmap_model::contains_key)]
#[kani::stub(std::collections::HashMap::len, crate::hashmap_model::len)]
#[kani::stub(std::time::Instant::now, crate::mocks::instant_now)]
#[kani::stub(std::sync::Condvar::wait_timeout, crate::mocks::condvar_wait_timeout)]
#[kani::stub(std::sync::Condvar::notify_all, crate::mocks::condvar_notify_all)]
#[kani::stub(std::fmt::format, crate::mocks::fmt_format)]
#[kani::unwind(4)]
fn verify_dm_initialize_explicit_only() {
    let (c, ep) = component_with_ep();
    connect_em(&c, vec![Extent { key: 1, size: kani::any(), offset: kani::any() }]);
    // constructed and wired, but initialize not called
    assert!(len(&c) == 0);
    assert!(matches!(c.lookup(1), Ok(LookupResult::NotExist)));
    assert!(ep.track_calls() == 0);
    // ... and calling it is what recovers the extent
    assert!(c.initialize().is_ok() && snap(&c, 1).is_some());
}

#[kani::proof]
#[kani::stub(std::collections::hash_map::RandomState::new, crate::mocks::concrete_state)]
#[kani::stub(std::collections::HashMap::get, crate::hashmap_model::get)]
#[kani::stub(std::collections::HashMap::get_mut, crate::hashmap_model::get_mut)]
#[kani::stub(std::collections::HashMap::insert, crate::hashmap_model::insert)]
#[kani::stub(std::collections::HashMap::remove, crate::hashmap_model::remove)]
#[kani::stub(std::collections::HashMap::contains_key, crate::hashmap_model::contains_key)]
#[kani::stub(std::collections::HashMap::len, crate::hashmap_model::len)]
#[kani::stub(std::time::Instant::now, crate::mocks::instant_now)]
#[kani::stub(std::sync::Condvar::wait_timeout, crate::mocks::condvar_wait_timeout)]
#[kani::stub(std::sync::Condvar::notify_all, crate::mocks::condvar_notify_all)]
#[kani::stub(std::fmt::format, crate::mocks::fmt_format)]
#[kani::unwind(4)]
fn verify_dm_initialize_explicit_only__mutant() {
    let (c, ep) = component_with_ep();
    connect_em(&c, vec![Extent { key: 1, size: kani::any(), offset: kani::any() }]);
    assert!(len(&c) == 1); // MUTANT: recovered at construction
}

/// DM-INV-RELEASE-WAKES-WAITERS: every successful release_read, release_write,
/// downgrade_reference and convert_to_storage calls Condvar::notify_all (observed through the
/// recording stub; in a single-threaded run there is nobody to wake, which is the real effect).
#[kani::proof]
#[kani::stub(std::collections::hash_map::RandomState::new, crate::mocks::concrete_state)]
#[kani::stub(std::collections::HashMap::get, crate::hashmap_model::get)]
#[kani::stub(std::collections::HashMap::get_mut, crate::hashmap_model::get_mut)]
#[kani::stub(std::collections::HashMap::insert, crate::hashmap_model::insert)]
#[kani::stub(std::collections::HashMap::remove, crate::hashmap_model::remove)]
#[kani::stub(std::collections::HashMap::contains_key, crate::hashmap_model::contains_key)]
#[kani::stub(std::collections::HashMap::len, crate::hashmap_model::len)]
#[kani::stub(std::time::Instant::now, crate::mocks::instant_now)]
#[kani::stub(std::sync::Condvar::wait_timeout, crate::mocks::condvar_wait_timeout)]
#[kani::stub(std::sync::Condvar::notify_all, crate::mocks::condvar_notify_all)]
#[kani::stub(std::fmt::format, crate::mocks::fmt_format)]
#[kani::unwind(4)]
fn verify_dm_inv_release_wakes_waiters() {
    let c = component_bare();
    seed(&c, 7, any_entry());
    let n0 = notifies();
    // candidate operations: OP_RELEASE_READ, OP_RELEASE_WRITE, OP_DOWNGRADE, OP_CONVERT_TO_STORAGE
    let (op, err) = { let s: u8 = kani::any(); kani::assume((s as usize) < 4); if s == 0 { run_op_const(&c, 7, OP_RELEASE_READ) } else if s == 1 { run_op_const(&c, 7, OP_RELEASE_WRITE) } else if s == 2 { run_op_const(&c, 7, OP_DOWNGRADE) } else { run_op_const(&c, 7, OP_CONVERT_TO_STORAGE) } };
    kani::cover!(!err && op == OP_CONVERT_TO_STORAGE);
    kani::cover!(!err && op == OP_DOWNGRADE);
    if !err {
        assert!(notifies() > n0);
    }
}

#[kani::proof]
#[kani::stub(std::collections::hash_map::RandomState::new, crate::mocks::concrete_state)]
#[kani::stub(std::collections::HashMap::get, crate::hashmap_model::get)]
#[kani::stub(std::collections::HashMap::get_mut, crate::hashmap_model::get_mut)]
#[kani::stub(std::collections::HashMap::insert, crate::hashmap_model::insert)]
#[kani::stub(std::collections::HashMap::remove, crate::hashmap_model::remove)]
#[kani::stub(std::collections::HashMap::contains_key, crate::hashmap_model::contains_key)]
#[kani::stub(std::collections::HashMap::len, crate::hashmap_model::len)]
#[kani::stub(std::time::Instant::now, crate::mocks::instant_now)]
#[kani::stub(std::sync::Condvar::wait_timeout, crate::mocks::condvar_wait_timeout)]
#[kani::stub(std::sync::Condvar::notify_all, crate::mocks::condvar_notify_all)]
#[kani::stub(std::fmt::format, crate::mocks::fmt_format)]
#[kani::unwind(4)]
fn verify_dm_inv_release_wakes_waiters__mutant() {
    let c = component_bare();
    seed(&c, 7, any_entry());
    let n0 = notifies();
    // candidate operations: OP_RELEASE_READ, OP_RELEASE_WRITE, OP_DOWNGRADE, OP_CONVERT_TO_STORAGE
    let (_op, err) = { let s: u8 = kani::any(); kani::assume((s as usize) < 4); if s == 0 { run_op_const(&c, 7, OP_RELEASE_READ) } else if s == 1 { run_op_const(&c, 7, OP_RELEASE_WRITE) } else if s == 2 { run_op_const(&c, 7, OP_DOWNGRADE) } else { run_op_const(&c, 7, OP_CONVERT_TO_STORAGE) } };
    if !err {
        assert!(notifies() == n0); // MUTANT
    }
}

/// DM-INV-WAIT-TIMES-OUT-ONLY-IF-BLOCKED: lookup / take_read / take_write report Timeout exactly
/// when their wait condition is false (single-threaded: it then stays false for the whole period).
#[kani::proof]
#[kani::stub(std::collections::hash_map::RandomState::new, crate::mocks::concrete_state)]
#[kani::stub(std::collections::HashMap::get, crate::hashmap_model::get)]
#[kani::stub(std::collections::HashMap::get_mut, crate::hashmap_model::get_mut)]
#[kani::stub(std::collections::HashMap::insert, crate::hashmap_model::insert)]
#[kani::stub(std::collections::HashMap::remove, crate::hashmap_model::remove)]
#[kani::stub(std::collections::HashMap::contains_key, crate::hashmap_model::contains_key)]
#[kani::stub(std::collections::HashMap::len, crate::hashmap_model::len)]
#[kani::stub(std::time::Instant::now, crate::mocks::instant_now)]
#[kani::stub(std::sync::Condvar::wait_timeout, crate::mocks::condvar_wait_timeout)]
#[kani::stub(std::sync::Condvar::notify_all, crate::mocks::condvar_notify_all)]
#[kani::stub(std::fmt::format, crate::mocks::fmt_format)]
#[kani::unwind(4)]
fn verify_dm_inv_wait_times_out_only_if_blocked() {
    let c = component_bare();
    let present: bool = kani::any();
    let (mut r0, mut w0) = (0u32, 0u32);
    if present {
        let e = any_entry();
        r0 = e.read_ref; w0 = e.write_ref;
        seed(&c, 7, e);
    }
    let op: u8 = kani::any();
    kani::assume(op < 3);
    let (res, cond) = if op == 0 {
        (c.lookup(7).err(), !present || w0 == 0)
    } else if op == 1 {
        (c.take_read(7).err(), !present || w0 == 0)
    } else {
        (c.take_write(7).err(), !present || (r0 == 0 && w0 == 0))
    };
    let timed_out = matches!(res, Some(DispatchMapError::Timeout(7)));
    kani::cover!(timed_out);
    assert!(timed_out == !cond);
}

#[kani::proof]
#[kani::stub(std::collections::hash_map::RandomState::new, crate::mocks::concrete_state)]
#[kani::stub(std::collections::HashMap::get, crate::hashmap_model::get)]
#[kani::stub(std::collections::HashMap::get_mut, crate::hashmap_model::get_mut)]
#[kani::stub(std::collections::HashMap::insert, crate::hashmap_model::insert)]
#[kani::stub(std::collections::HashMap::remove, crate::hashmap_model::remove)]
#[kani::stub(std::collections::HashMap::contains_key, crate::hashmap_model::contains_key)]
#[kani::stub(std::collections::HashMap::len, crate::hashmap_model::len)]
#[kani::stub(std::time::Instant::now, crate::mocks::instant_now)]
#[kani::stub(std::sync::Condvar::wait_timeout, crate::mocks::condvar_wait_timeout)]
#[kani::stub(std::sync::Condvar::notify_all, crate::mocks::condvar_notify_all)]
#[kani::stub(std::fmt::format, crate::mocks::fmt_format)]
#[kani::unwind(4)]
fn verify_dm_inv_wait_times_out_only_if_blocked__mutant() {
    let c = component_bare();
    let e = any_entry();
    kani::assume(e.write_ref == 0);
    seed(&c, 7, e);
    assert!(matches!(c.take_read(7), Err(DispatchMapError::Timeout(7)))); // MUTANT
}

/// DM-INV-WRITE-REF-AT-MOST-ONE, inductive: from any state whose entry holds <= 1 write
/// reference, EVERY IDispatchMap operation (all 20, symbolic arguments) leaves every entry with <= 1;
/// establishment is covered by the same harness (create / recover / initialize add entries with 1 / 0 / 0).
#[kani::proof]
#[kani::stub(std::collections::hash_map::RandomState::new, crate::mocks::concrete_state)]
#[kani::stub(std::collections::HashMap::get, crate::hashmap_model::get)]
#[kani::stub(std::collections::HashMap::get_mut, crate::hashmap_model::get_mut)]
#[kani::stub(std::collections::HashMap::insert, crate::hashmap_model::insert)]
#[kani::stub(std::collections::HashMap::remove, crate::hashmap_model::remove)]
#[kani::stub(std::collections::HashMap::contains_key, crate::hashmap_model::contains_key)]
#[kani::stub(std::collections::HashMap::len, crate::hashmap_model::len)]
#[kani::stub(std::time::Instant::now, crate::mocks::instant_now)]
#[kani::stub(std::sync::Condvar::wait_timeout, crate::mocks::condvar_wait_timeout)]
#[kani::stub(std::sync::Condvar::notify_all, crate::mocks::condvar_notify_all)]
#[kani::stub(std::fmt::format, crate::mocks::fmt_format)]
#[kani::unwind(4)]
fn verify_dm_inv_write_ref_at_most_one() {
    let (c, ep) = component_with_ep();
    let pool = ensure_pool(&c, &ep);
    connect_em(&c, vec![Extent { key: 1, size: kani::any(), offset: kani::any() }]);
    let e = any_entry();
    kani::assume(e.size_blocks <= u32::MAX / 4096); // entries inside the agreed range

    kani::assume(e.write_ref <= 1);                 // induction hypothesis
    seed_tracked(&c, &ep, pool, 7, e);
    let k: CacheKey = if kani::any() { 7 } else { 8 };   // 8: a fresh key (create/recover)
    let (op, err) = run_op(&c, k, &ALL_OPS);
    kani::cover!(!err && op == OP_TAKE_WRITE);
    kani::cover!(!err && op == OP_CREATE);
    for key in [1u64, 7, 8] {
        if let Some(s) = snap(&c, key) { assert!(s.write_ref <= 1); }
    }
}

#[kani::proof]
#[kani::stub(std::collections::hash_map::RandomState::new, crate::mocks::concrete_state)]
#[kani::stub(std::collections::HashMap::get, crate::hashmap_model::get)]
#[kani::stub(std::collections::HashMap::get_mut, crate::hashmap_model::get_mut)]
#[kani::stub(std::collections::HashMap::insert, crate::hashmap_model::insert)]
#[kani::stub(std::collections::HashMap::remove, crate::hashmap_model::remove)]
#[kani::stub(std::collections::HashMap::contains_key, crate::hashmap_model::contains_key)]
#[kani::stub(std::collections::HashMap::len, crate::hashmap_model::len)]
#[kani::stub(std::time::Instant::now, crate::mocks::instant_now)]
#[kani::stub(std::sync::Condvar::wait_timeout, crate::mocks::condvar_wait_timeout)]
#[kani::stub(std::sync::Condvar::notify_all, crate::mocks::condvar_notify_all)]
#[kani::stub(std::fmt::format, crate::mocks::fmt_format)]
#[kani::unwind(4)]
fn verify_dm_inv_write_ref_at_most_one__mutant() {
    let (c, ep) = component_with_ep();
    let pool = ensure_pool(&c, &ep);
    connect_em(&c, vec![Extent { key: 1, size: kani::any(), offset: kani::any() }]);
    let e = any_entry();
    kani::assume(e.size_blocks <= u32::MAX / 4096); // entries inside the agreed range

    kani::assume(e.write_ref <= 1);
    seed_tracked(&c, &ep, pool, 7, e);
    let _ = c.create_memory_tier_entry(8, any_ptr(), 4096); // one cheap step suffices to refute
    if let Some(s) = snap(&c, 8) { assert!(s.write_ref == 0); } // MUTANT
}

/// DM-INV-READ-WRITE-EXCLUSIVE, inductive over all 20 operations (an entry holding a write
/// reference holds no read references).
#[kani::proof]
#[kani::stub(std::collections::hash_map::RandomState::new, crate::mocks::concrete_state)]
#[kani::stub(std::collections::HashMap::get, crate::hashmap_model::get)]
#[kani::stub(std::collections::HashMap::get_mut, crate::hashmap_model::get_mut)]
#[kani::stub(std::collections::HashMap::insert, crate::hashmap_model::insert)]
#[kani::stub(std::collections::HashMap::remove, crate::hashmap_model::remove)]
#[kani::stub(std::collections::HashMap::contains_key, crate::hashmap_model::contains_key)]
#[kani::stub(std::collections::HashMap::len, crate::hashmap_model::len)]
#[kani::stub(std::time::Instant::now, crate::mocks::instant_now)]
#[kani::stub(std::sync::Condvar::wait_timeout, crate::mocks::condvar_wait_timeout)]
#[kani::stub(std::sync::Condvar::notify_all, crate::mocks::condvar_notify_all)]
#[kani::stub(std::fmt::format, crate::mocks::fmt_format)]
#[kani::unwind(4)]
fn verify_dm_inv_read_write_exclusive() {
    let (c, ep) = component_with_ep();
    let pool = ensure_pool(&c, &ep);
    connect_em(&c, vec![Extent { key: 1, size: kani::any(), offset: kani::any() }]);
    let e = any_entry();
    kani::assume(e.size_blocks <= u32::MAX / 4096); // entries inside the agreed range

    kani::assume(e.write_ref == 0 || e.read_ref == 0); // induction hypothesis
    seed_tracked(&c, &ep, pool, 7, e);
    let k: CacheKey = if kani::any() { 7 } else { 8 };
    let (op, err) = run_op(&c, k, &ALL_OPS);
    kani::cover!(!err && op == OP_DOWNGRADE);
    for key in [1u64, 7, 8] {
        if let Some(s) = snap(&c, key) { assert!(s.write_ref == 0 || s.read_ref == 0); }
    }
}

#[kani::proof]
#[kani::stub(std::collections::hash_map::RandomState::new, crate::mocks::concrete_state)]
#[kani::stub(std::collections::HashMap::get, crate::hashmap_model::get)]
#[kani::stub(std::collections::HashMap::get_mut, crate::hashmap_model::get_mut)]
#[kani::stub(std::collections::HashMap::insert, crate::hashmap_model::insert)]
#[kani::stub(std::collections::HashMap::remove, crate::hashmap_model::remove)]
#[kani::stub(std::collections::HashMap::contains_key, crate::hashmap_model::contains_key)]
#[kani::stub(std::collections::HashMap::len, crate::hashmap_model::len)]
#[kani::stub(std::time::Instant::now, crate::mocks::instant_now)]
#[kani::stub(std::sync::Condvar::wait_timeout, crate::mocks::condvar_wait_timeout)]
#[kani::stub(std::sync::Condvar::notify_all, crate::mocks::condvar_notify_all)]
#[kani::stub(std::fmt::format, crate::mocks::fmt_format)]
#[kani::unwind(4)]
fn verify_dm_inv_read_write_exclusive__mutant() {
    let (c, ep) = component_with_ep();
    let pool = ensure_pool(&c, &ep);
    connect_em(&c, vec![Extent { key: 1, size: kani::any(), offset: kani::any() }]);
    let e = any_entry();
    kani::assume(e.size_blocks <= u32::MAX / 4096); // entries inside the agreed range

    kani::assume(e.write_ref == 0 || e.read_ref == 0);
    seed_tracked(&c, &ep, pool, 7, e);
    let _ = c.downgrade_reference(7);
    let s = snap(&c, 7);
    if let Some(s) = s { assert!(s.write_ref == 0 && s.read_ref == 0); } // MUTANT
}

/// DM-INV-REFCOUNT-CONSERVATION, SEQUENTIAL projection: over any sequence of two reference
/// operations (lookup, take_read, release_read, downgrade_reference, convert_to_storage), the read count
/// equals start + successful acquisitions - successful releases/consumptions. Interleavings are not
/// modelled (Kani has no thread scheduler); route the concurrent half to Loom.
#[kani::proof]
#[kani::stub(std::collections::hash_map::RandomState::new, crate::mocks::concrete_state)]
#[kani::stub(std::collections::HashMap::get, crate::hashmap_model::get)]
#[kani::stub(std::collections::HashMap::get_mut, crate::hashmap_model::get_mut)]
#[kani::stub(std::collections::HashMap::insert, crate::hashmap_model::insert)]
#[kani::stub(std::collections::HashMap::remove, crate::hashmap_model::remove)]
#[kani::stub(std::collections::HashMap::contains_key, crate::hashmap_model::contains_key)]
#[kani::stub(std::collections::HashMap::len, crate::hashmap_model::len)]
#[kani::stub(std::time::Instant::now, crate::mocks::instant_now)]
#[kani::stub(std::sync::Condvar::wait_timeout, crate::mocks::condvar_wait_timeout)]
#[kani::stub(std::sync::Condvar::notify_all, crate::mocks::condvar_notify_all)]
#[kani::stub(std::fmt::format, crate::mocks::fmt_format)]
#[kani::unwind(4)]
fn verify_dm_inv_refcount_conservation() {
    let c = component_bare();
    let e = any_entry();
    let start = e.read_ref as u64;
    seed(&c, 7, e);
    let mut acquired: u64 = 0;
    let mut released: u64 = 0;
    // candidate operations: OP_LOOKUP, OP_TAKE_READ, OP_RELEASE_READ, OP_DOWNGRADE, OP_CONVERT_TO_STORAGE
    let mut step = 0;
    while step < 2 {
        let before = snap(&c, 7).unwrap().read_ref;
        let (op, err) = { let s: u8 = kani::any(); kani::assume((s as usize) < 5); if s == 0 { run_op_const(&c, 7, OP_LOOKUP) } else if s == 1 { run_op_const(&c, 7, OP_TAKE_READ) } else if s == 2 { run_op_const(&c, 7, OP_RELEASE_READ) } else if s == 3 { run_op_const(&c, 7, OP_DOWNGRADE) } else { run_op_const(&c, 7, OP_CONVERT_TO_STORAGE) } };
        if !err {
            if op == OP_LOOKUP || op == OP_TAKE_READ || op == OP_DOWNGRADE { acquired += 1; }
            if op == OP_RELEASE_READ { released += 1; }
            if op == OP_CONVERT_TO_STORAGE && before > 0 { released += 1; } // consumed
        }
        step += 1;
    }
    kani::cover!(acquired == 1 && released == 1);
    let now = snap(&c, 7).unwrap().read_ref as u64;
    assert!(start + acquired >= released);          // no underflow
    assert!(now == start + acquired - released);   // no lost update, no leak
}

#[kani::proof]
#[kani::stub(std::collections::hash_map::RandomState::new, crate::mocks::concrete_state)]
#[kani::stub(std::collections::HashMap::get, crate::hashmap_model::get)]
#[kani::stub(std::collections::HashMap::get_mut, crate::hashmap_model::get_mut)]
#[kani::stub(std::collections::HashMap::insert, crate::hashmap_model::insert)]
#[kani::stub(std::collections::HashMap::remove, crate::hashmap_model::remove)]
#[kani::stub(std::collections::HashMap::contains_key, crate::hashmap_model::contains_key)]
#[kani::stub(std::collections::HashMap::len, crate::hashmap_model::len)]
#[kani::stub(std::time::Instant::now, crate::mocks::instant_now)]
#[kani::stub(std::sync::Condvar::wait_timeout, crate::mocks::condvar_wait_timeout)]
#[kani::stub(std::sync::Condvar::notify_all, crate::mocks::condvar_notify_all)]
#[kani::stub(std::fmt::format, crate::mocks::fmt_format)]
#[kani::unwind(4)]
fn verify_dm_inv_refcount_conservation__mutant() {
    let c = component_bare();
    let e = any_entry();
    let start = e.read_ref;
    seed(&c, 7, e);
    // candidate operations: OP_LOOKUP, OP_TAKE_READ, OP_RELEASE_READ
    let (_op, err) = { let s: u8 = kani::any(); kani::assume((s as usize) < 3); if s == 0 { run_op_const(&c, 7, OP_LOOKUP) } else if s == 1 { run_op_const(&c, 7, OP_TAKE_READ) } else { run_op_const(&c, 7, OP_RELEASE_READ) } };
    if !err { assert!(snap(&c, 7).unwrap().read_ref == start); } // MUTANT
}

/// DM-INV-MEMORY-TIER-SIZE-NONZERO, inductive over all 20 operations.
#[kani::proof]
#[kani::stub(std::collections::hash_map::RandomState::new, crate::mocks::concrete_state)]
#[kani::stub(std::collections::HashMap::get, crate::hashmap_model::get)]
#[kani::stub(std::collections::HashMap::get_mut, crate::hashmap_model::get_mut)]
#[kani::stub(std::collections::HashMap::insert, crate::hashmap_model::insert)]
#[kani::stub(std::collections::HashMap::remove, crate::hashmap_model::remove)]
#[kani::stub(std::collections::HashMap::contains_key, crate::hashmap_model::contains_key)]
#[kani::stub(std::collections::HashMap::len, crate::hashmap_model::len)]
#[kani::stub(std::time::Instant::now, crate::mocks::instant_now)]
#[kani::stub(std::sync::Condvar::wait_timeout, crate::mocks::condvar_wait_timeout)]
#[kani::stub(std::sync::Condvar::notify_all, crate::mocks::condvar_notify_all)]
#[kani::stub(std::fmt::format, crate::mocks::fmt_format)]
#[kani::unwind(4)]
fn verify_dm_inv_memory_tier_size_nonzero() {
    let (c, ep) = component_with_ep();
    let pool = ensure_pool(&c, &ep);
    connect_em(&c, vec![Extent { key: 1, size: kani::any(), offset: kani::any() }]);
    let e = any_entry();
    kani::assume(e.size_blocks <= u32::MAX / 4096); // entries inside the agreed range

    let a = snap_entry(&e);
    kani::assume(!a.is_mem || a.size > 0);          // induction hypothesis
    seed_tracked(&c, &ep, pool, 7, e);
    let k: CacheKey = if kani::any() { 7 } else { 8 };
    let (op, err) = run_op(&c, k, &ALL_OPS);
    kani::cover!(!err && (op == OP_CREATE || op == OP_PROMOTE));
    for key in [1u64, 7, 8] {
        if let Some(s) = snap(&c, key) { assert!(!s.is_mem || s.size > 0); }
    }
}

#[kani::proof]
#[kani::stub(std::collections::hash_map::RandomState::new, crate::mocks::concrete_state)]
#[kani::stub(std::collections::HashMap::get, crate::hashmap_model::get)]
#[kani::stub(std::collections::HashMap::get_mut, crate::hashmap_model::get_mut)]
#[kani::stub(std::collections::HashMap::insert, crate::hashmap_model::insert)]
#[kani::stub(std::collections::HashMap::remove, crate::hashmap_model::remove)]
#[kani::stub(std::collections::HashMap::contains_key, crate::hashmap_model::contains_key)]
#[kani::stub(std::collections::HashMap::len, crate::hashmap_model::len)]
#[kani::stub(std::time::Instant::now, crate::mocks::instant_now)]
#[kani::stub(std::sync::Condvar::wait_timeout, crate::mocks::condvar_wait_timeout)]
#[kani::stub(std::sync::Condvar::notify_all, crate::mocks::condvar_notify_all)]
#[kani::stub(std::fmt::format, crate::mocks::fmt_format)]
#[kani::unwind(4)]
fn verify_dm_inv_memory_tier_size_nonzero__mutant() {
    let (c, ep) = component_with_ep();
    let pool = ensure_pool(&c, &ep);
    connect_em(&c, vec![Extent { key: 1, size: kani::any(), offset: kani::any() }]);
    let e = any_entry();
    kani::assume(e.size_blocks <= u32::MAX / 4096); // entries inside the agreed range

    let a = snap_entry(&e);
    kani::assume(!a.is_mem || a.size > 0);
    seed_tracked(&c, &ep, pool, 7, e);
    let _ = c.create_memory_tier_entry(8, any_ptr(), kani::any());
    if let Some(s) = snap(&c, 8) { assert!(!s.is_mem); } // MUTANT
}

/// DM-INV-ENTRY-TRACKED-FOR-EVICTION, inductive: a key is tracked in the dispatch map's pool
/// (under the entry's own handle) exactly when it has an entry, across initialize, create, recover,
/// remove, oldest_keys, promote, convert_memory_tier_to_block and try_evict_to_block.
#[kani::proof]
#[kani::stub(std::collections::hash_map::RandomState::new, crate::mocks::concrete_state)]
#[kani::stub(std::collections::HashMap::get, crate::hashmap_model::get)]
#[kani::stub(std::collections::HashMap::get_mut, crate::hashmap_model::get_mut)]
#[kani::stub(std::collections::HashMap::insert, crate::hashmap_model::insert)]
#[kani::stub(std::collections::HashMap::remove, crate::hashmap_model::remove)]
#[kani::stub(std::collections::HashMap::contains_key, crate::hashmap_model::contains_key)]
#[kani::stub(std::collections::HashMap::len, crate::hashmap_model::len)]
#[kani::stub(std::time::Instant::now, crate::mocks::instant_now)]
#[kani::stub(std::sync::Condvar::wait_timeout, crate::mocks::condvar_wait_timeout)]
#[kani::stub(std::sync::Condvar::notify_all, crate::mocks::condvar_notify_all)]
#[kani::stub(std::fmt::format, crate::mocks::fmt_format)]
#[kani::unwind(4)]
fn verify_dm_inv_entry_tracked_for_eviction() {
    let (c, ep) = component_with_ep();
    let pool = ensure_pool(&c, &ep);
    connect_em(&c, vec![Extent { key: 1, size: kani::any(), offset: kani::any() }]);
    let e = any_entry();
    kani::assume(e.size_blocks <= u32::MAX / 4096); // entries inside the agreed range

    seed_tracked(&c, &ep, pool, 7, e);              // induction hypothesis: present <=> tracked
    // candidate operations: OP_INITIALIZE, OP_CREATE, OP_RECOVER, OP_REMOVE, OP_OLDEST_KEYS, OP_PROMOTE, OP_CONVERT_MTB, OP_TRY_EVICT
    let k: CacheKey = if kani::any() { 7 } else { 8 };
    let (op, err) = { let s: u8 = kani::any(); kani::assume((s as usize) < 8); if s == 0 { run_op_const(&c, k, OP_INITIALIZE) } else if s == 1 { run_op_const(&c, k, OP_CREATE) } else if s == 2 { run_op_const(&c, k, OP_RECOVER) } else if s == 3 { run_op_const(&c, k, OP_REMOVE) } else if s == 4 { run_op_const(&c, k, OP_OLDEST_KEYS) } else if s == 5 { run_op_const(&c, k, OP_PROMOTE) } else if s == 6 { run_op_const(&c, k, OP_CONVERT_MTB) } else { run_op_const(&c, k, OP_TRY_EVICT) } };
    kani::cover!(!err && op == OP_REMOVE);
    kani::cover!(!err && op == OP_INITIALIZE);
    for key in [1u64, 7, 8] {
        let present = snap(&c, key);
        assert!(present.is_some() == ep.is_tracked(pool, key));
        if let Some(s) = present { assert!(ep.handle_of(pool, key) == Some(s.handle)); }
    }
}

#[kani::proof]
#[kani::stub(std::collections::hash_map::RandomState::new, crate::mocks::concrete_state)]
#[kani::stub(std::collections::HashMap::get, crate::hashmap_model::get)]
#[kani::stub(std::collections::HashMap::get_mut, crate::hashmap_model::get_mut)]
#[kani::stub(std::collections::HashMap::insert, crate::hashmap_model::insert)]
#[kani::stub(std::collections::HashMap::remove, crate::hashmap_model::remove)]
#[kani::stub(std::collections::HashMap::contains_key, crate::hashmap_model::contains_key)]
#[kani::stub(std::collections::HashMap::len, crate::hashmap_model::len)]
#[kani::stub(std::time::Instant::now, crate::mocks::instant_now)]
#[kani::stub(std::sync::Condvar::wait_timeout, crate::mocks::condvar_wait_timeout)]
#[kani::stub(std::sync::Condvar::notify_all, crate::mocks::condvar_notify_all)]
#[kani::stub(std::fmt::format, crate::mocks::fmt_format)]
#[kani::unwind(4)]
fn verify_dm_inv_entry_tracked_for_eviction__mutant() {
    let (c, ep) = component_with_ep();
    let pool = ensure_pool(&c, &ep);
    connect_em(&c, vec![Extent { key: 1, size: kani::any(), offset: kani::any() }]);
    let e = any_entry();
    kani::assume(e.size_blocks <= u32::MAX / 4096); // entries inside the agreed range

    kani::assume(e.read_ref == 0 && e.write_ref == 0);
    seed_tracked(&c, &ep, pool, 7, e);
    let _ = c.remove(7);
    assert!(ep.is_tracked(pool, 7)); // MUTANT
}

/// DM-INV-CHECKSUM-SURVIVES-TRANSITIONS
#[kani::proof]
#[kani::stub(std::collections::hash_map::RandomState::new, crate::mocks::concrete_state)]
#[kani::stub(std::collections::HashMap::get, crate::hashmap_model::get)]
#[kani::stub(std::collections::HashMap::get_mut, crate::hashmap_model::get_mut)]
#[kani::stub(std::collections::HashMap::insert, crate::hashmap_model::insert)]
#[kani::stub(std::collections::HashMap::remove, crate::hashmap_model::remove)]
#[kani::stub(std::collections::HashMap::contains_key, crate::hashmap_model::contains_key)]
#[kani::stub(std::collections::HashMap::len, crate::hashmap_model::len)]
#[kani::stub(std::time::Instant::now, crate::mocks::instant_now)]
#[kani::stub(std::sync::Condvar::wait_timeout, crate::mocks::condvar_wait_timeout)]
#[kani::stub(std::sync::Condvar::notify_all, crate::mocks::condvar_notify_all)]
#[kani::stub(std::fmt::format, crate::mocks::fmt_format)]
#[kani::unwind(4)]
fn verify_dm_inv_checksum_survives_transitions() {
    let c = component_bare();
    let mut e = any_entry();
    kani::assume(e.checksum != 0);
    let cs = e.checksum;
    seed(&c, 7, e);
    // candidate operations: OP_CONVERT_MTB, OP_PROMOTE, OP_TRY_EVICT
    let (op, err) = { let s: u8 = kani::any(); kani::assume((s as usize) < 3); if s == 0 { run_op_const(&c, 7, OP_CONVERT_MTB) } else if s == 1 { run_op_const(&c, 7, OP_PROMOTE) } else { run_op_const(&c, 7, OP_TRY_EVICT) } };
    kani::cover!(!err && op == OP_PROMOTE);
    kani::cover!(!err && op == OP_TRY_EVICT);
    assert!(c.get_checksum(7) == Some(cs));
}

#[kani::proof]
#[kani::stub(std::collections::hash_map::RandomState::new, crate::mocks::concrete_state)]
#[kani::stub(std::collections::HashMap::get, crate::hashmap_model::get)]
#[kani::stub(std::collections::HashMap::get_mut, crate::hashmap_model::get_mut)]
#[kani::stub(std::collections::HashMap::insert, crate::hashmap_model::insert)]
#[kani::stub(std::collections::HashMap::remove, crate::hashmap_model::remove)]
#[kani::stub(std::collections::HashMap::contains_key, crate::hashmap_model::contains_key)]
#[kani::stub(std::collections::HashMap::len, crate::hashmap_model::len)]
#[kani::stub(std::time::Instant::now, crate::mocks::instant_now)]
#[kani::stub(std::sync::Condvar::wait_timeout, crate::mocks::condvar_wait_timeout)]
#[kani::stub(std::sync::Condvar::notify_all, crate::mocks::condvar_notify_all)]
#[kani::stub(std::fmt::format, crate::mocks::fmt_format)]
#[kani::unwind(4)]
fn verify_dm_inv_checksum_survives_transitions__mutant() {
    let c = component_bare();
    let e = any_entry();
    kani::assume(e.checksum != 0);
    seed(&c, 7, e);
    // candidate operations: OP_CONVERT_MTB, OP_PROMOTE, OP_TRY_EVICT
    let (_op, _err) = { let s: u8 = kani::any(); kani::assume((s as usize) < 3); if s == 0 { run_op_const(&c, 7, OP_CONVERT_MTB) } else if s == 1 { run_op_const(&c, 7, OP_PROMOTE) } else { run_op_const(&c, 7, OP_TRY_EVICT) } };
    assert!(c.get_checksum(7).is_none()); // MUTANT
}

/// DM-INV-SINGLE-WRITER, SEQUENTIAL projection: a second take_write while a write reference is
/// held never succeeds; only release_write (or downgrade) frees it. Concurrent callers not modelled.
#[kani::proof]
#[kani::stub(std::collections::hash_map::RandomState::new, crate::mocks::concrete_state)]
#[kani::stub(std::collections::HashMap::get, crate::hashmap_model::get)]
#[kani::stub(std::collections::HashMap::get_mut, crate::hashmap_model::get_mut)]
#[kani::stub(std::collections::HashMap::insert, crate::hashmap_model::insert)]
#[kani::stub(std::collections::HashMap::remove, crate::hashmap_model::remove)]
#[kani::stub(std::collections::HashMap::contains_key, crate::hashmap_model::contains_key)]
#[kani::stub(std::collections::HashMap::len, crate::hashmap_model::len)]
#[kani::stub(std::time::Instant::now, crate::mocks::instant_now)]
#[kani::stub(std::sync::Condvar::wait_timeout, crate::mocks::condvar_wait_timeout)]
#[kani::stub(std::sync::Condvar::notify_all, crate::mocks::condvar_notify_all)]
#[kani::stub(std::fmt::format, crate::mocks::fmt_format)]
#[kani::unwind(4)]
fn verify_dm_inv_single_writer() {
    let (c, ep) = component_with_ep();
    let fresh: bool = kani::any();
    if fresh {
        let _ = c.create_memory_tier_entry(7, any_ptr(), 4096); // the creator holds the write ref
    } else {
        seed(&c, 7, any_entry());
    }
    let r1 = c.take_write(7);
    let r2 = c.take_write(7);
    kani::cover!(r1.is_ok());
    assert!(!(r1.is_ok() && r2.is_ok()));            // never two holders
    if fresh { assert!(r1.is_err()); }               // the creator's write ref excludes a second
    if r1.is_ok() {
        // after the holder releases, the write reference can be taken again (when no readers)
        assert!(c.release_write(7).is_ok());
        assert!(c.take_write(7).is_ok());
    }
}

#[kani::proof]
#[kani::stub(std::collections::hash_map::RandomState::new, crate::mocks::concrete_state)]
#[kani::stub(std::collections::HashMap::get, crate::hashmap_model::get)]
#[kani::stub(std::collections::HashMap::get_mut, crate::hashmap_model::get_mut)]
#[kani::stub(std::collections::HashMap::insert, crate::hashmap_model::insert)]
#[kani::stub(std::collections::HashMap::remove, crate::hashmap_model::remove)]
#[kani::stub(std::collections::HashMap::contains_key, crate::hashmap_model::contains_key)]
#[kani::stub(std::collections::HashMap::len, crate::hashmap_model::len)]
#[kani::stub(std::time::Instant::now, crate::mocks::instant_now)]
#[kani::stub(std::sync::Condvar::wait_timeout, crate::mocks::condvar_wait_timeout)]
#[kani::stub(std::sync::Condvar::notify_all, crate::mocks::condvar_notify_all)]
#[kani::stub(std::fmt::format, crate::mocks::fmt_format)]
#[kani::unwind(4)]
fn verify_dm_inv_single_writer__mutant() {
    let c = component_bare();
    let e = any_entry();
    kani::assume(e.read_ref == 0 && e.write_ref == 0);
    seed(&c, 7, e);
    let r1 = c.take_write(7);
    let r2 = c.take_write(7);
    assert!(r1.is_ok() && r2.is_ok()); // MUTANT
}

/// DM-INV-WAIT-THEN-ACT-ATOMIC: the reference is taken while the waited-for condition still
/// holds — the guard returned by wait_for is the one the increment/assignment uses (state.rs:51-75).
/// Checked sequentially: condition at the moment of acquisition; no interleaving modelled.
#[kani::proof]
#[kani::stub(std::collections::hash_map::RandomState::new, crate::mocks::concrete_state)]
#[kani::stub(std::collections::HashMap::get, crate::hashmap_model::get)]
#[kani::stub(std::collections::HashMap::get_mut, crate::hashmap_model::get_mut)]
#[kani::stub(std::collections::HashMap::insert, crate::hashmap_model::insert)]
#[kani::stub(std::collections::HashMap::remove, crate::hashmap_model::remove)]
#[kani::stub(std::collections::HashMap::contains_key, crate::hashmap_model::contains_key)]
#[kani::stub(std::collections::HashMap::len, crate::hashmap_model::len)]
#[kani::stub(std::time::Instant::now, crate::mocks::instant_now)]
#[kani::stub(std::sync::Condvar::wait_timeout, crate::mocks::condvar_wait_timeout)]
#[kani::stub(std::sync::Condvar::notify_all, crate::mocks::condvar_notify_all)]
#[kani::stub(std::fmt::format, crate::mocks::fmt_format)]
#[kani::unwind(4)]
fn verify_dm_inv_wait_then_act_atomic() {
    let c = component_bare();
    let e = any_entry();
    let (r0, w0) = (e.read_ref, e.write_ref);
    seed(&c, 7, e);
    let op: u8 = kani::any();
    kani::assume(op < 3);
    let ok = if op == 0 {
        matches!(c.lookup(7), Ok(LookupResult::BlockDevice { .. }) | Ok(LookupResult::MemoryTier { .. }))
    } else if op == 1 {
        c.take_read(7).is_ok()
    } else {
        c.take_write(7).is_ok()
    };
    kani::cover!(ok && op == 2);
    if ok {
        let s = snap(&c, 7).unwrap();
        if op < 2 {
            assert!(w0 == 0 && s.write_ref == 0);         // no writer when the read ref is taken
        } else {
            assert!(r0 == 0 && w0 == 0 && s.read_ref == 0); // no reader/writer when it is taken
        }
    }
}

#[kani::proof]
#[kani::stub(std::collections::hash_map::RandomState::new, crate::mocks::concrete_state)]
#[kani::stub(std::collections::HashMap::get, crate::hashmap_model::get)]
#[kani::stub(std::collections::HashMap::get_mut, crate::hashmap_model::get_mut)]
#[kani::stub(std::collections::HashMap::insert, crate::hashmap_model::insert)]
#[kani::stub(std::collections::HashMap::remove, crate::hashmap_model::remove)]
#[kani::stub(std::collections::HashMap::contains_key, crate::hashmap_model::contains_key)]
#[kani::stub(std::collections::HashMap::len, crate::hashmap_model::len)]
#[kani::stub(std::time::Instant::now, crate::mocks::instant_now)]
#[kani::stub(std::sync::Condvar::wait_timeout, crate::mocks::condvar_wait_timeout)]
#[kani::stub(std::sync::Condvar::notify_all, crate::mocks::condvar_notify_all)]
#[kani::stub(std::fmt::format, crate::mocks::fmt_format)]
#[kani::unwind(4)]
fn verify_dm_inv_wait_then_act_atomic__mutant() {
    let c = component_bare();
    let e = any_entry();
    let w0 = e.write_ref;
    seed(&c, 7, e);
    if c.take_read(7).is_ok() { assert!(w0 != 0); } // MUTANT
}

/// DM-INV-ERROR-LEAVES-STATE-UNCHANGED over all 20 operations, from states satisfying the proved
/// invariants DM-INV-WRITE-REF-AT-MOST-ONE and DM-INV-READ-WRITE-EXCLUSIVE (outside them
/// downgrade_reference's overflow path at lib.rs:308-312 would clear the write reference, but that state
/// is unreachable).
#[kani::proof]
#[kani::stub(std::collections::hash_map::RandomState::new, crate::mocks::concrete_state)]
#[kani::stub(std::collections::HashMap::get, crate::hashmap_model::get)]
#[kani::stub(std::collections::HashMap::get_mut, crate::hashmap_model::get_mut)]
#[kani::stub(std::collections::HashMap::insert, crate::hashmap_model::insert)]
#[kani::stub(std::collections::HashMap::remove, crate::hashmap_model::remove)]
#[kani::stub(std::collections::HashMap::contains_key, crate::hashmap_model::contains_key)]
#[kani::stub(std::collections::HashMap::len, crate::hashmap_model::len)]
#[kani::stub(std::time::Instant::now, crate::mocks::instant_now)]
#[kani::stub(std::sync::Condvar::wait_timeout, crate::mocks::condvar_wait_timeout)]
#[kani::stub(std::sync::Condvar::notify_all, crate::mocks::condvar_notify_all)]
#[kani::stub(std::fmt::format, crate::mocks::fmt_format)]
#[kani::unwind(4)]
fn verify_dm_inv_error_leaves_state_unchanged() {
    let (c, ep) = component_with_ep();
    let pool = ensure_pool(&c, &ep);
    // no extent manager bound: with an eviction policy connected, initialize has no error
    // return in either configuration, so binding one only adds Ok paths this obligation is not about
    let e = any_entry();
    kani::assume(e.size_blocks <= u32::MAX / 4096);
    // pre-state satisfies the two proved inductive invariants (WRITE-REF-AT-MOST-ONE,
    // READ-WRITE-EXCLUSIVE): the states the map can actually be in
    kani::assume(e.write_ref <= 1 && (e.write_ref == 0 || e.read_ref == 0));
    seed_tracked(&c, &ep, pool, 7, e);
    let a = snap(&c, 7).unwrap();
    let k: CacheKey = if kani::any() { 7 } else { 8 };
    let (op, err) = run_op(&c, k, &ALL_OPS);
    kani::cover!(err && op == OP_DOWNGRADE);
    kani::cover!(err && op == OP_CREATE);
    if err {
        assert!(snap(&c, 7).unwrap() == a);
        assert!(snap(&c, 8).is_none() && snap(&c, 1).is_none() && len(&c) == 1);
    }
}

#[kani::proof]
#[kani::stub(std::collections::hash_map::RandomState::new, crate::mocks::concrete_state)]
#[kani::stub(std::collections::HashMap::get, crate::hashmap_model::get)]
#[kani::stub(std::collections::HashMap::get_mut, crate::hashmap_model::get_mut)]
#[kani::stub(std::collections::HashMap::insert, crate::hashmap_model::insert)]
#[kani::stub(std::collections::HashMap::remove, crate::hashmap_model::remove)]
#[kani::stub(std::collections::HashMap::contains_key, crate::hashmap_model::contains_key)]
#[kani::stub(std::collections::HashMap::len, crate::hashmap_model::len)]
#[kani::stub(std::time::Instant::now, crate::mocks::instant_now)]
#[kani::stub(std::sync::Condvar::wait_timeout, crate::mocks::condvar_wait_timeout)]
#[kani::stub(std::sync::Condvar::notify_all, crate::mocks::condvar_notify_all)]
#[kani::stub(std::fmt::format, crate::mocks::fmt_format)]
#[kani::unwind(4)]
fn verify_dm_inv_error_leaves_state_unchanged__mutant() {
    let (c, ep) = component_with_ep();
    let pool = ensure_pool(&c, &ep);
    let e = any_entry();
    kani::assume(e.size_blocks <= u32::MAX / 4096);
    kani::assume(e.write_ref <= 1 && (e.write_ref == 0 || e.read_ref == 0));
    seed_tracked(&c, &ep, pool, 7, e);
    let a = snap(&c, 7).unwrap();
    let err = c.create_memory_tier_entry(7, any_ptr(), 4096).is_err(); // AlreadyExists
    if err { assert!(snap(&c, 7).unwrap() != a); } // MUTANT
}

/// DM-INV-SINGLE-POOL: any two of initialize / oldest_keys / create_memory_tier_entry /
/// recover_extent create at most one pool between them and every track/candidates call names it.
#[kani::proof]
#[kani::stub(std::collections::hash_map::RandomState::new, crate::mocks::concrete_state)]
#[kani::stub(std::collections::HashMap::get, crate::hashmap_model::get)]
#[kani::stub(std::collections::HashMap::get_mut, crate::hashmap_model::get_mut)]
#[kani::stub(std::collections::HashMap::insert, crate::hashmap_model::insert)]
#[kani::stub(std::collections::HashMap::remove, crate::hashmap_model::remove)]
#[kani::stub(std::collections::HashMap::contains_key, crate::hashmap_model::contains_key)]
#[kani::stub(std::collections::HashMap::len, crate::hashmap_model::len)]
#[kani::stub(std::time::Instant::now, crate::mocks::instant_now)]
#[kani::stub(std::sync::Condvar::wait_timeout, crate::mocks::condvar_wait_timeout)]
#[kani::stub(std::sync::Condvar::notify_all, crate::mocks::condvar_notify_all)]
#[kani::stub(std::fmt::format, crate::mocks::fmt_format)]
#[kani::unwind(4)]
fn verify_dm_inv_single_pool() {
    let (c, ep) = component_with_ep();
    // candidate operations: OP_INITIALIZE, OP_OLDEST_KEYS, OP_CREATE, OP_RECOVER
    let (o1, _) = { let s: u8 = kani::any(); kani::assume((s as usize) < 4); if s == 0 { run_op_const(&c, 1, OP_INITIALIZE) } else if s == 1 { run_op_const(&c, 1, OP_OLDEST_KEYS) } else if s == 2 { run_op_const(&c, 1, OP_CREATE) } else { run_op_const(&c, 1, OP_RECOVER) } };
    let (o2, _) = { let s: u8 = kani::any(); kani::assume((s as usize) < 4); if s == 0 { run_op_const(&c, 2, OP_INITIALIZE) } else if s == 1 { run_op_const(&c, 2, OP_OLDEST_KEYS) } else if s == 2 { run_op_const(&c, 2, OP_CREATE) } else { run_op_const(&c, 2, OP_RECOVER) } };
    kani::cover!(o1 == OP_CREATE && o2 == OP_RECOVER);
    assert!(ep.pools_created() <= 1);      // (0 when both calls were zero-size creates)
    assert!(ep.other_pool_calls() == 0);
    if ep.pools_created() == 1 { assert!(pool_of(&c) == Some(ep.pool())); }
}

#[kani::proof]
#[kani::stub(std::collections::hash_map::RandomState::new, crate::mocks::concrete_state)]
#[kani::stub(std::collections::HashMap::get, crate::hashmap_model::get)]
#[kani::stub(std::collections::HashMap::get_mut, crate::hashmap_model::get_mut)]
#[kani::stub(std::collections::HashMap::insert, crate::hashmap_model::insert)]
#[kani::stub(std::collections::HashMap::remove, crate::hashmap_model::remove)]
#[kani::stub(std::collections::HashMap::contains_key, crate::hashmap_model::contains_key)]
#[kani::stub(std::collections::HashMap::len, crate::hashmap_model::len)]
#[kani::stub(std::time::Instant::now, crate::mocks::instant_now)]
#[kani::stub(std::sync::Condvar::wait_timeout, crate::mocks::condvar_wait_timeout)]
#[kani::stub(std::sync::Condvar::notify_all, crate::mocks::condvar_notify_all)]
#[kani::stub(std::fmt::format, crate::mocks::fmt_format)]
#[kani::unwind(4)]
fn verify_dm_inv_single_pool__mutant() {
    let (c, ep) = component_with_ep();
    // candidate operations: OP_INITIALIZE, OP_OLDEST_KEYS, OP_CREATE, OP_RECOVER
    let _ = { let s: u8 = kani::any(); kani::assume((s as usize) < 4); if s == 0 { run_op_const(&c, 1, OP_INITIALIZE) } else if s == 1 { run_op_const(&c, 1, OP_OLDEST_KEYS) } else if s == 2 { run_op_const(&c, 1, OP_CREATE) } else { run_op_const(&c, 1, OP_RECOVER) } };
    let _ = { let s: u8 = kani::any(); kani::assume((s as usize) < 4); if s == 0 { run_op_const(&c, 2, OP_INITIALIZE) } else if s == 1 { run_op_const(&c, 2, OP_OLDEST_KEYS) } else if s == 2 { run_op_const(&c, 2, OP_CREATE) } else { run_op_const(&c, 2, OP_RECOVER) } };
    assert!(ep.pools_created() == 2); // MUTANT
}

/// DM-INV-MEMORY-TIER-SIZE-BLOCKS-CONSISTENT, inductive over all 20 operations.
#[kani::proof]
#[kani::stub(std::collections::hash_map::RandomState::new, crate::mocks::concrete_state)]
#[kani::stub(std::collections::HashMap::get, crate::hashmap_model::get)]
#[kani::stub(std::collections::HashMap::get_mut, crate::hashmap_model::get_mut)]
#[kani::stub(std::collections::HashMap::insert, crate::hashmap_model::insert)]
#[kani::stub(std::collections::HashMap::remove, crate::hashmap_model::remove)]
#[kani::stub(std::collections::HashMap::contains_key, crate::hashmap_model::contains_key)]
#[kani::stub(std::collections::HashMap::len, crate::hashmap_model::len)]
#[kani::stub(std::time::Instant::now, crate::mocks::instant_now)]
#[kani::stub(std::sync::Condvar::wait_timeout, crate::mocks::condvar_wait_timeout)]
#[kani::stub(std::sync::Condvar::notify_all, crate::mocks::condvar_notify_all)]
#[kani::stub(std::fmt::format, crate::mocks::fmt_format)]
#[kani::unwind(4)]
fn verify_dm_inv_memory_tier_size_blocks_consistent() {
    let (c, ep) = component_with_ep();
    let pool = ensure_pool(&c, &ep);
    connect_em(&c, vec![Extent { key: 1, size: kani::any(), offset: kani::any() }]);
    let e = any_entry();
    kani::assume(e.size_blocks <= u32::MAX / 4096); // entries inside the agreed range

    let a = snap_entry(&e);
    kani::assume(!a.is_mem || a.size_blocks as u64 == ceil_blocks(a.size)); // induction hypothesis
    seed_tracked(&c, &ep, pool, 7, e);
    let k: CacheKey = if kani::any() { 7 } else { 8 };
    let (op, err) = run_op(&c, k, &ALL_OPS);
    kani::cover!(!err && op == OP_PROMOTE);
    for key in [1u64, 7, 8] {
        if let Some(s) = snap(&c, key) {
            if s.is_mem { assert!(s.size_blocks as u64 == ceil_blocks(s.size)); }
        }
    }
}

#[kani::proof]
#[kani::stub(std::collections::hash_map::RandomState::new, crate::mocks::concrete_state)]
#[kani::stub(std::collections::HashMap::get, crate::hashmap_model::get)]
#[kani::stub(std::collections::HashMap::get_mut, crate::hashmap_model::get_mut)]
#[kani::stub(std::collections::HashMap::insert, crate::hashmap_model::insert)]
#[kani::stub(std::collections::HashMap::remove, crate::hashmap_model::remove)]
#[kani::stub(std::collections::HashMap::contains_key, crate::hashmap_model::contains_key)]
#[kani::stub(std::collections::HashMap::len, crate::hashmap_model::len)]
#[kani::stub(std::time::Instant::now, crate::mocks::instant_now)]
#[kani::stub(std::sync::Condvar::wait_timeout, crate::mocks::condvar_wait_timeout)]
#[kani::stub(std::sync::Condvar::notify_all, crate::mocks::condvar_notify_all)]
#[kani::stub(std::fmt::format, crate::mocks::fmt_format)]
#[kani::unwind(4)]
fn verify_dm_inv_memory_tier_size_blocks_consistent__mutant() {
    let (c, ep) = component_with_ep();
    let pool = ensure_pool(&c, &ep);
    connect_em(&c, vec![Extent { key: 1, size: kani::any(), offset: kani::any() }]);
    let e = any_entry();
    kani::assume(e.size_blocks <= u32::MAX / 4096); // entries inside the agreed range

    let a = snap_entry(&e);
    kani::assume(!a.is_mem || a.size_blocks as u64 == ceil_blocks(a.size));
    seed_tracked(&c, &ep, pool, 7, e);
    let _ = c.create_memory_tier_entry(8, any_ptr(), kani::any());
    if let Some(s) = snap(&c, 8) {
        if s.is_mem { assert!(s.size_blocks as u64 == s.size as u64 / 4096); } // MUTANT: floor
    }
}

/// DM-INV-SSD-OFFSET-STICKY over all 20 operations.
#[kani::proof]
#[kani::stub(std::collections::hash_map::RandomState::new, crate::mocks::concrete_state)]
#[kani::stub(std::collections::HashMap::get, crate::hashmap_model::get)]
#[kani::stub(std::collections::HashMap::get_mut, crate::hashmap_model::get_mut)]
#[kani::stub(std::collections::HashMap::insert, crate::hashmap_model::insert)]
#[kani::stub(std::collections::HashMap::remove, crate::hashmap_model::remove)]
#[kani::stub(std::collections::HashMap::contains_key, crate::hashmap_model::contains_key)]
#[kani::stub(std::collections::HashMap::len, crate::hashmap_model::len)]
#[kani::stub(std::time::Instant::now, crate::mocks::instant_now)]
#[kani::stub(std::sync::Condvar::wait_timeout, crate::mocks::condvar_wait_timeout)]
#[kani::stub(std::sync::Condvar::notify_all, crate::mocks::condvar_notify_all)]
#[kani::stub(std::fmt::format, crate::mocks::fmt_format)]
#[kani::unwind(4)]
fn verify_dm_inv_ssd_offset_sticky() {
    let (c, ep) = component_with_ep();
    let pool = ensure_pool(&c, &ep);
    connect_em(&c, vec![Extent { key: 1, size: kani::any(), offset: kani::any() }]);
    let e = any_entry();
    kani::assume(e.size_blocks <= u32::MAX / 4096); // entries inside the agreed range

    let a = snap_entry(&e);
    kani::assume(a.is_mem && a.ssd.is_some());
    seed_tracked(&c, &ep, pool, 7, e);
    let (op, err) = run_op(&c, 7, &ALL_OPS);
    kani::cover!(!err && op == OP_CONVERT_TO_STORAGE);
    if let Some(s) = snap(&c, 7) {
        if s.is_mem { assert!(s.ssd.is_some()); }
    }
}

#[kani::proof]
#[kani::stub(std::collections::hash_map::RandomState::new, crate::mocks::concrete_state)]
#[kani::stub(std::collections::HashMap::get, crate::hashmap_model::get)]
#[kani::stub(std::collections::HashMap::get_mut, crate::hashmap_model::get_mut)]
#[kani::stub(std::collections::HashMap::insert, crate::hashmap_model::insert)]
#[kani::stub(std::collections::HashMap::remove, crate::hashmap_model::remove)]
#[kani::stub(std::collections::HashMap::contains_key, crate::hashmap_model::contains_key)]
#[kani::stub(std::collections::HashMap::len, crate::hashmap_model::len)]
#[kani::stub(std::time::Instant::now, crate::mocks::instant_now)]
#[kani::stub(std::sync::Condvar::wait_timeout, crate::mocks::condvar_wait_timeout)]
#[kani::stub(std::sync::Condvar::notify_all, crate::mocks::condvar_notify_all)]
#[kani::stub(std::fmt::format, crate::mocks::fmt_format)]
#[kani::unwind(4)]
fn verify_dm_inv_ssd_offset_sticky__mutant() {
    let (c, ep) = component_with_ep();
    let pool = ensure_pool(&c, &ep);
    connect_em(&c, vec![Extent { key: 1, size: kani::any(), offset: kani::any() }]);
    let e = any_entry();
    kani::assume(e.size_blocks <= u32::MAX / 4096); // entries inside the agreed range

    let a = snap_entry(&e);
    kani::assume(a.is_mem && a.ssd.is_some());
    seed_tracked(&c, &ep, pool, 7, e);
    let _ = c.convert_to_storage(7, kani::any());
    assert!(snap(&c, 7).unwrap().ssd.is_none()); // MUTANT
}

/// DM-INV-LEGAL-LOCATION-TRANSITIONS over all 20 operations: memory tier -> block device only by
/// convert_memory_tier_to_block / try_evict_to_block, only with an SSD copy, landing at that offset;
/// block device -> memory tier only by promote_block_to_memory_tier.
#[kani::proof]
#[kani::stub(std::collections::hash_map::RandomState::new, crate::mocks::concrete_state)]
#[kani::stub(std::collections::HashMap::get, crate::hashmap_model::get)]
#[kani::stub(std::collections::HashMap::get_mut, crate::hashmap_model::get_mut)]
#[kani::stub(std::collections::HashMap::insert, crate::hashmap_model::insert)]
#[kani::stub(std::collections::HashMap::remove, crate::hashmap_model::remove)]
#[kani::stub(std::collections::HashMap::contains_key, crate::hashmap_model::contains_key)]
#[kani::stub(std::collections::HashMap::len, crate::hashmap_model::len)]
#[kani::stub(std::time::Instant::now, crate::mocks::instant_now)]
#[kani::stub(std::sync::Condvar::wait_timeout, crate::mocks::condvar_wait_timeout)]
#[kani::stub(std::sync::Condvar::notify_all, crate::mocks::condvar_notify_all)]
#[kani::stub(std::fmt::format, crate::mocks::fmt_format)]
#[kani::unwind(4)]
fn verify_dm_inv_legal_location_transitions() {
    let (c, ep) = component_with_ep();
    let pool = ensure_pool(&c, &ep);
    connect_em(&c, vec![Extent { key: 1, size: kani::any(), offset: kani::any() }]);
    let e = any_entry();
    kani::assume(e.size_blocks <= u32::MAX / 4096); // entries inside the agreed range

    let a = snap_entry(&e);
    seed_tracked(&c, &ep, pool, 7, e);
    let (op, err) = run_op(&c, 7, &ALL_OPS);
    kani::cover!(!err && op == OP_TRY_EVICT);
    if let Some(s) = snap(&c, 7) {
        if a.is_mem && !s.is_mem {
            assert!(op == OP_CONVERT_MTB || op == OP_TRY_EVICT);
            assert!(a.ssd == Some(s.offset));
        }
        if !a.is_mem && s.is_mem {
            assert!(op == OP_PROMOTE);
        }
    }
}

#[kani::proof]
#[kani::stub(std::collections::hash_map::RandomState::new, crate::mocks::concrete_state)]
#[kani::stub(std::collections::HashMap::get, crate::hashmap_model::get)]
#[kani::stub(std::collections::HashMap::get_mut, crate::hashmap_model::get_mut)]
#[kani::stub(std::collections::HashMap::insert, crate::hashmap_model::insert)]
#[kani::stub(std::collections::HashMap::remove, crate::hashmap_model::remove)]
#[kani::stub(std::collections::HashMap::contains_key, crate::hashmap_model::contains_key)]
#[kani::stub(std::collections::HashMap::len, crate::hashmap_model::len)]
#[kani::stub(std::time::Instant::now, crate::mocks::instant_now)]
#[kani::stub(std::sync::Condvar::wait_timeout, crate::mocks::condvar_wait_timeout)]
#[kani::stub(std::sync::Condvar::notify_all, crate::mocks::condvar_notify_all)]
#[kani::stub(std::fmt::format, crate::mocks::fmt_format)]
#[kani::unwind(4)]
fn verify_dm_inv_legal_location_transitions__mutant() {
    let (c, ep) = component_with_ep();
    let pool = ensure_pool(&c, &ep);
    connect_em(&c, vec![Extent { key: 1, size: kani::any(), offset: kani::any() }]);
    let e = any_entry();
    kani::assume(e.size_blocks <= u32::MAX / 4096); // entries inside the agreed range

    let a = snap_entry(&e);
    kani::assume(a.is_mem && a.ssd.is_some() && a.read_ref == 0 && a.write_ref == 0);
    seed_tracked(&c, &ep, pool, 7, e);
    let _ = c.try_evict_to_block(7);
    assert!(snap(&c, 7).unwrap().is_mem); // MUTANT
}

