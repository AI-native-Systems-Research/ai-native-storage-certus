// Diagnostics for the stubs (named check_*, so the scorer does not score them as properties).
use crate::entry::{DispatchEntry, Location};
use crate::mocks::*;
use crate::*;
use interfaces::{DispatchMapError, Extent, IDispatchMap, LookupResult};

/// Diagnostic (not a property): the HashMap stub model honours the std HashMap contract for the
/// six methods dispatch-map uses, and its paths are REACHED and return values (the "stub can blind
/// the code" check). Must PASS.
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
fn check_hashmap_model_contract() {
    let c = component_bare();
    let mut g = c.state.inner.lock().unwrap();
    let m = &mut g.entries;
    assert!(m.len() == 0 && !m.contains_key(&7) && m.get(&7).is_none());
    let e1 = any_entry();
    let a1 = snap_entry(&e1);
    assert!(m.insert(7, e1).is_none());                       // new key -> None
    assert!(m.len() == 1 && m.contains_key(&7) && !m.contains_key(&9));
    assert!(snap_entry(m.get(&7).unwrap()) == a1);            // reached, returns the value
    let e2 = any_entry();
    let a2 = snap_entry(&e2);
    let old = m.insert(7, e2);                                 // existing key -> old value back
    assert!(snap_entry(&old.unwrap()) == a1 && m.len() == 1);
    assert!(snap_entry(m.get(&7).unwrap()) == a2);
    m.get_mut(&7).unwrap().read_ref = 5;                       // get_mut writes through
    assert!(m.get(&7).unwrap().read_ref == 5);
    assert!(m.insert(9, any_entry()).is_none() && m.len() == 2);
    let r = m.remove(&7);                                      // remove returns it, others kept
    assert!(r.is_some() && r.unwrap().read_ref == 5);
    assert!(!m.contains_key(&7) && m.get(&7).is_none() && m.get_mut(&7).is_none());
    assert!(m.remove(&7).is_none() && m.len() == 1 && m.contains_key(&9));
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
fn check_hashmap_model_contract__mutant() {
    let c = component_bare();
    let mut g = c.state.inner.lock().unwrap();
    let m = &mut g.entries;
    let _ = m.insert(7, any_entry());
    assert!(m.get(&7).is_none()); // MUTANT: a blinded model would pass this
}

/// Diagnostic: a create -> release_write -> lookup -> release_read -> remove -> lookup round
/// trip through the REAL component code returns real results (the stubbed map is not blind).
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
fn check_hashmap_model_reached_through_component() {
    let (c, ep) = component_with_ep();
    assert!(c.create_memory_tier_entry(7, any_ptr(), 4096).is_ok());
    assert!(c.release_write(7).is_ok());
    assert!(matches!(c.lookup(7), Ok(LookupResult::MemoryTier { .. })));
    assert!(c.release_read(7).is_ok());
    assert!(c.remove(7).is_ok());
    assert!(matches!(c.lookup(7), Ok(LookupResult::NotExist)));
}

/// Diagnostic, REAL hashbrown table (no HashMap stub): the release_read obligation on one concrete
/// entry — agreement evidence for the model on the real container. Expensive (minutes).
#[kani::proof]
#[kani::stub(std::collections::hash_map::RandomState::new, crate::mocks::concrete_state)]
#[kani::stub(std::time::Instant::now, crate::mocks::instant_now)]
#[kani::stub(std::sync::Condvar::wait_timeout, crate::mocks::condvar_wait_timeout)]
#[kani::stub(std::sync::Condvar::notify_all, crate::mocks::condvar_notify_all)]
#[kani::stub(std::fmt::format, crate::mocks::fmt_format)]

#[kani::unwind(6)]
fn check_real_hashmap_release_read_decrements() {
    let c = component_bare();
    let e = any_entry();
    kani::assume(e.read_ref > 0);
    let before = e.read_ref;
    c.state.inner.lock().unwrap().entries.insert(7, e);
    let r = c.release_read(7);
    assert!(r.is_ok());
    assert!(c.state.inner.lock().unwrap().entries.get(&7).unwrap().read_ref == before - 1);
}

