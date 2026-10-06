// remove, touch, entry_size, oldest_keys, create_memory_tier_entry, recover_extent,
// set_checksum, get_checksum — the REAL IDispatchMap impl (geometry: see proofs_refs.rs).
// The eviction-policy receptacle is connected to the recording MockEp wherever the method uses
// it; `refute_*` harnesses run with it DISCONNECTED and are #[kani::should_panic].
use crate::entry::{DispatchEntry, Location};
use crate::mocks::*;
use crate::*;
use interfaces::{DispatchMapError, Extent, IDispatchMap, LookupResult};

/// DM-REMOVE-DELETES
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
fn verify_dm_remove_deletes() {
    let (c, ep) = component_with_ep();
    let pool = ensure_pool(&c, &ep);
    let e = any_entry();
    kani::assume(e.read_ref == 0 && e.write_ref == 0);
    seed_tracked(&c, &ep, pool, 7, e);
    let r = c.remove(7);
    assert!(r.is_ok());
    assert!(snap(&c, 7).is_none());
    assert!(matches!(c.lookup(7), Ok(LookupResult::NotExist)));
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
fn verify_dm_remove_deletes__mutant() {
    let (c, ep) = component_with_ep();
    let pool = ensure_pool(&c, &ep);
    let e = any_entry();
    kani::assume(e.read_ref == 0 && e.write_ref == 0);
    seed_tracked(&c, &ep, pool, 7, e);
    let _ = c.remove(7);
    assert!(snap(&c, 7).is_some()); // MUTANT
}

/// DM-REMOVE-ACTIVE-REFERENCES
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
fn verify_dm_remove_active_references() {
    let (c, ep) = component_with_ep();
    let pool = ensure_pool(&c, &ep);
    let e = any_entry();
    kani::assume(e.read_ref != 0 || e.write_ref != 0);
    let a = snap_entry(&e);
    seed_tracked(&c, &ep, pool, 7, e);
    let r = c.remove(7);
    assert!(matches!(r, Err(DispatchMapError::ActiveReferences(7))));
    assert!(snap(&c, 7).unwrap() == a && ep.is_tracked(pool, 7));
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
fn verify_dm_remove_active_references__mutant() {
    let (c, ep) = component_with_ep();
    let pool = ensure_pool(&c, &ep);
    let e = any_entry();
    kani::assume(e.read_ref != 0 || e.write_ref != 0);
    seed_tracked(&c, &ep, pool, 7, e);
    assert!(c.remove(7).is_ok()); // MUTANT
}

/// DM-REMOVE-NOT-FOUND
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
fn verify_dm_remove_not_found() {
    let (c, ep) = component_with_ep();
    seed(&c, 9, any_entry());
    let b = snap(&c, 9).unwrap();
    let r = c.remove(7);
    assert!(matches!(r, Err(DispatchMapError::KeyNotFound(7))));
    assert!(len(&c) == 1 && snap(&c, 9).unwrap() == b && ep.remove_calls() == 0);
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
fn verify_dm_remove_not_found__mutant() {
    let (c, ep) = component_with_ep();
    seed(&c, 9, any_entry());
    assert!(c.remove(7).is_ok()); // MUTANT
}

/// DM-REMOVE-UNTRACKS (IEvictionPolicy mock honours its contract: candidates are tracked keys)
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
fn verify_dm_remove_untracks() {
    let (c, ep) = component_with_ep();
    let pool = ensure_pool(&c, &ep);
    let e = any_entry();
    let h = e.eviction_handle;
    seed_tracked(&c, &ep, pool, 7, e);
    let r = c.remove(7);
    kani::cover!(r.is_ok());
    if r.is_ok() {
        assert!(!ep.is_tracked(pool, 7));
        assert!(ep.last_remove() == Some(h));
        // and it can never again be offered as an eviction candidate
        let v = c.oldest_keys(3);
        let mut i = 0;
        while i < v.len() { assert!(v[i] != 7); i += 1; }
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
fn verify_dm_remove_untracks__mutant() {
    let (c, ep) = component_with_ep();
    let pool = ensure_pool(&c, &ep);
    seed_tracked(&c, &ep, pool, 7, any_entry());
    if c.remove(7).is_ok() {
        assert!(ep.is_tracked(pool, 7)); // MUTANT
    }
}

/// DM-REMOVE-FRAME
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
fn verify_dm_remove_frame() {
    let (c, ep) = component_with_ep();
    let pool = ensure_pool(&c, &ep);
    seed_tracked(&c, &ep, pool, 7, any_entry());
    seed_tracked(&c, &ep, pool, 9, any_entry());
    let b = snap(&c, 9).unwrap();
    let r = c.remove(7);
    kani::cover!(r.is_ok());
    assert!(snap(&c, 9).unwrap() == b && ep.is_tracked(pool, 9));
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
fn verify_dm_remove_frame__mutant() {
    let (c, ep) = component_with_ep();
    let pool = ensure_pool(&c, &ep);
    let e = any_entry();
    kani::assume(e.read_ref == 0 && e.write_ref == 0);
    seed_tracked(&c, &ep, pool, 7, e);
    seed(&c, 9, any_entry());
    let _ = c.remove(7);
    assert!(len(&c) == 2); // MUTANT
}

/// DM-TOUCH-FRAME (refs, location, size unchanged; only the eviction ranking is touched)
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
fn verify_dm_touch_frame() {
    let (c, ep) = component_with_ep();
    seed(&c, 7, any_entry());
    seed(&c, 9, any_entry());
    let a = snap(&c, 7).unwrap();
    let b = snap(&c, 9).unwrap();
    let r = c.touch(7);
    assert!(r.is_ok());
    assert!(snap(&c, 7).unwrap() == a && snap(&c, 9).unwrap() == b);
    assert!(ep.touch_calls() == 1 && ep.last_touch() == Some(a.handle));
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
fn verify_dm_touch_frame__mutant() {
    let (c, ep) = component_with_ep();
    seed(&c, 7, any_entry());
    let a = snap(&c, 7).unwrap();
    let _ = c.touch(7);
    assert!(snap(&c, 7).unwrap().read_ref != a.read_ref); // MUTANT
}

/// DM-TOUCH-NOT-FOUND
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
fn verify_dm_touch_not_found() {
    let (c, ep) = component_with_ep();
    seed(&c, 9, any_entry());
    let r = c.touch(7);
    assert!(matches!(r, Err(DispatchMapError::KeyNotFound(7))));
    assert!(ep.touch_calls() == 0);
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
fn verify_dm_touch_not_found__mutant() {
    let (c, ep) = component_with_ep();
    seed(&c, 9, any_entry());
    assert!(c.touch(7).is_ok()); // MUTANT
}

/// DM-ENTRY-SIZE-BLOCK-MULTIPLE (under D-RANGE-FR-023-388782 / D-RANGE-FR-003-8e9f6a)
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
fn verify_dm_entry_size_block_multiple() {
    let c = component_bare();
    let e = any_entry();
    // the entry's block count is inside the range spec and code agree on:
    // D-RANGE-FR-023-388782 (recovered) / D-RANGE-FR-003-8e9f6a (memory tier, ceil(size/4096))
    kani::assume(e.size_blocks <= u32::MAX / 4096);
    let sb = e.size_blocks;
    seed(&c, 7, e);
    let r = c.entry_size(7);
    assert!(matches!(r, Ok(_)));
    let v = r.unwrap();
    assert!(v as u64 == sb as u64 * 4096 && v % 4096 == 0);
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
fn verify_dm_entry_size_block_multiple__mutant() {
    let c = component_bare();
    let e = any_entry();
    kani::assume(e.size_blocks <= u32::MAX / 4096);
    let sb = e.size_blocks;
    seed(&c, 7, e);
    let v = c.entry_size(7).unwrap();
    assert!(v as u64 == sb as u64 * 512); // MUTANT: wrong block size
}

/// DM-ENTRY-SIZE-ROUNDS-UP-MEMORY-TIER (created or promoted; under D-RANGE-FR-003-8e9f6a)
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
fn verify_dm_entry_size_rounds_up_memory_tier() {
    let (c, ep) = component_with_ep();
    let s: u32 = kani::any();
    kani::assume(s <= u32::MAX - 4095); // D-RANGE-FR-003-8e9f6a
    let promote: bool = kani::any();
    let ok = if promote {
        let e = any_entry();
        seed(&c, 7, e);
        c.promote_block_to_memory_tier(7, any_ptr(), s).is_ok()
    } else {
        c.create_memory_tier_entry(7, any_ptr(), s).is_ok()
    };
    kani::cover!(ok && promote);
    kani::cover!(ok && !promote);
    if ok {
        let v = c.entry_size(7).unwrap() as u64;
        assert!(v == ceil_blocks(s) * 4096);
        assert!(v >= s as u64 && v < s as u64 + 4096 && v % 4096 == 0);
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
fn verify_dm_entry_size_rounds_up_memory_tier__mutant() {
    let (c, ep) = component_with_ep();
    let s: u32 = kani::any();
    kani::assume(s <= u32::MAX - 4095);
    if c.create_memory_tier_entry(7, any_ptr(), s).is_ok() {
        let v = c.entry_size(7).unwrap() as u64;
        assert!(v == s as u64); // MUTANT: no rounding
    }
}

/// DM-ENTRY-SIZE-FRAME
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
fn verify_dm_entry_size_frame() {
    let c = component_bare();
    let e = any_entry();
    kani::assume(e.size_blocks <= u32::MAX / 4096);
    seed(&c, 7, e);
    seed(&c, 9, any_entry());
    let a = snap(&c, 7).unwrap();
    let b = snap(&c, 9).unwrap();
    let _ = c.entry_size(7);
    assert!(snap(&c, 7).unwrap() == a && snap(&c, 9).unwrap() == b);
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
fn verify_dm_entry_size_frame__mutant() {
    let c = component_bare();
    let e = any_entry();
    kani::assume(e.size_blocks <= u32::MAX / 4096);
    seed(&c, 7, e);
    let a = snap(&c, 7).unwrap();
    let _ = c.entry_size(7);
    assert!(snap(&c, 7).unwrap().read_ref == a.read_ref + 1); // MUTANT: takes a reference
}

/// DM-ENTRY-SIZE-NOT-FOUND
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
fn verify_dm_entry_size_not_found() {
    let c = component_bare();
    seed(&c, 9, any_entry());
    assert!(matches!(c.entry_size(7), Err(DispatchMapError::KeyNotFound(7))));
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
fn verify_dm_entry_size_not_found__mutant() {
    let c = component_bare();
    seed(&c, 9, any_entry());
    assert!(c.entry_size(7).is_ok()); // MUTANT
}

/// DM-ENTRY-SIZE-NO-ARITHMETIC-OVERFLOW: for every entry create_memory_tier_entry,
/// promote_block_to_memory_tier, recover_extent or initialize can put in the map (inputs under the
/// level-1 range assumptions D-RANGE-FR-003-8e9f6a / D-RANGE-FR-023-388782), entry_size does not
/// overflow and reports the exact size.
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
fn verify_dm_entry_size_no_arithmetic_overflow() {
    let (c, ep) = component_with_ep();
    let way: u8 = kani::any();
    kani::assume(way < 4);
    let ok = if way == 0 {
        let s: u32 = kani::any();
        kani::assume(s <= u32::MAX - 4095); // D-RANGE-FR-003-8e9f6a
        c.create_memory_tier_entry(7, any_ptr(), s).is_ok()
    } else if way == 1 {
        let e = any_entry();
        kani::assume(e.size_blocks <= u32::MAX / 4096); // pre-existing entry inside the agreed range (D-RANGE-FR-023/FR-003)
        seed(&c, 7, e);
        let s: u32 = kani::any();
        kani::assume(s <= u32::MAX - 4095); // D-RANGE-FR-003-8e9f6a
        c.promote_block_to_memory_tier(7, any_ptr(), s).is_ok()
    } else if way == 2 {
        let sb: u32 = kani::any();
        kani::assume(sb <= u32::MAX / 4096); // D-RANGE-FR-023-388782
        c.recover_extent(7, kani::any(), sb).is_ok()
    } else {
        let sb: u32 = kani::any();
        kani::assume(sb <= u32::MAX / 4096); // D-RANGE-FR-023-388782
        connect_em(&c, vec![Extent { key: 7, size: sb, offset: kani::any() }]);
        c.initialize().is_ok()
    };
    kani::cover!(ok);
    if ok {
        let sb = snap(&c, 7).unwrap().size_blocks;
        // Kani checks the multiplication at lib.rs:369 for overflow; it must not fire, and the
        // reported size must be the exact (unwrapped) product.
        let v = c.entry_size(7).unwrap();
        assert!(v as u64 == sb as u64 * 4096);
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
fn verify_dm_entry_size_no_arithmetic_overflow__mutant() {
    let c = component_bare();
    let sb: u32 = kani::any();
    kani::assume(sb > u32::MAX / 4096); // MUTANT: outside the agreed range -> the product overflows
    seed(&c, 7, DispatchEntry { location: Location::BlockDevice { offset: 0 }, size_blocks: sb,
        read_ref: 0, write_ref: 0, eviction_handle: interfaces::EvictionHandle::new(0, 0),
        reuse_count: std::sync::atomic::AtomicU32::new(0), checksum: 0 });
    let _ = c.entry_size(7);
}

/// DM-OLDEST-KEYS-AT-MOST-N: dispatch-map hands back what IEvictionPolicy::get_eviction_candidates
/// returns; the mock follows that method's documented contract ("up to n keys"), so this proves
/// dispatch-map neither adds to nor pads the list. n <= 4 with 3 tracked keys.
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
fn verify_dm_oldest_keys_at_most_n() {
    let (c, ep) = component_with_ep();
    let _ = c.create_memory_tier_entry(1, any_ptr(), 4096);
    let _ = c.recover_extent(2, kani::any(), 1);
    let _ = c.create_memory_tier_entry(3, any_ptr(), 4096);
    let n: usize = kani::any();
    kani::assume(n <= 4);
    let v = c.oldest_keys(n);
    kani::cover!(v.len() == n && n > 0);
    assert!(v.len() <= n);
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
fn verify_dm_oldest_keys_at_most_n__mutant() {
    let (c, ep) = component_with_ep();
    let _ = c.create_memory_tier_entry(1, any_ptr(), 4096);
    let n: usize = kani::any();
    kani::assume(n <= 4);
    let v = c.oldest_keys(n);
    assert!(v.len() > n); // MUTANT
}

/// DM-OLDEST-KEYS-FRAME
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
fn verify_dm_oldest_keys_frame() {
    let (c, ep) = component_with_ep();
    let pool = ensure_pool(&c, &ep);
    seed_tracked(&c, &ep, pool, 7, any_entry());
    seed_tracked(&c, &ep, pool, 9, any_entry());
    let a = snap(&c, 7).unwrap();
    let b = snap(&c, 9).unwrap();
    let (t0, r0) = (ep.track_calls(), ep.remove_calls());
    let n: usize = kani::any();
    kani::assume(n <= 4);
    let _ = c.oldest_keys(n);
    assert!(snap(&c, 7).unwrap() == a && snap(&c, 9).unwrap() == b && len(&c) == 2);
    assert!(ep.track_calls() == t0 && ep.remove_calls() == r0);
    assert!(ep.is_tracked(pool, 7) && ep.is_tracked(pool, 9));
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
fn verify_dm_oldest_keys_frame__mutant() {
    let (c, ep) = component_with_ep();
    let pool = ensure_pool(&c, &ep);
    seed_tracked(&c, &ep, pool, 7, any_entry());
    let _ = c.oldest_keys(1);
    assert!(!ep.is_tracked(pool, 7)); // MUTANT: candidate removed from tracking
}

/// DM-OLDEST-KEYS-ONLY-PRESENT-KEYS: state built by create/recover/release_write/remove;
/// every key returned is an entry of the map (relies on IEvictionPolicy returning tracked keys).
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
fn verify_dm_oldest_keys_only_present_keys() {
    let (c, ep) = component_with_ep();
    // build the state through the real operations only
    let _ = c.create_memory_tier_entry(1, any_ptr(), 4096);
    let _ = c.recover_extent(2, kani::any(), 1);
    let _ = c.create_memory_tier_entry(3, any_ptr(), 4096);
    if kani::any() { let _ = c.release_write(1); let _ = c.remove(1); }
    if kani::any() { let _ = c.remove(2); }
    let n: usize = kani::any();
    kani::assume(n <= 4);
    let v = c.oldest_keys(n);
    kani::cover!(v.len() >= 1);
    let mut i = 0;
    while i < v.len() {
        assert!(snap(&c, v[i]).is_some());
        i += 1;
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
fn verify_dm_oldest_keys_only_present_keys__mutant() {
    let (c, ep) = component_with_ep();
    let _ = c.recover_extent(2, kani::any(), 1);
    let _ = c.remove(2);
    let _ = c.recover_extent(3, kani::any(), 1);
    let v = c.oldest_keys(4);
    assert!(v.is_empty()); // MUTANT: candidates do come back
}

/// REFUTATION of DM-OLDEST-KEYS-NO-EVICTION-POLICY-EMPTY. oldest_keys first calls get_pool_id
/// (lib.rs:373), which unwraps the eviction-policy receptacle (lib.rs:55) whenever no pool has been
/// created yet, so with no eviction policy connected the call panics instead of returning an empty
/// list. The empty-list branch (lib.rs:374-378) is reachable only if a pool was created earlier and
/// the policy was disconnected afterwards.
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
#[kani::should_panic]
#[kani::unwind(4)]
fn refute_dm_oldest_keys_no_eviction_policy_empty() {
    let c = component_bare(); // no eviction policy connected, pool not yet created
    let n: usize = kani::any();
    let _ = c.oldest_keys(n);
}

/// DM-OLDEST-KEYS-NO-EVICTION-POLICY-EMPTY (expected to FAIL: see the refutation)
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
fn verify_dm_oldest_keys_no_eviction_policy_empty() {
    let c = component_bare();
    let n: usize = kani::any();
    let v = c.oldest_keys(n);
    assert!(v.is_empty());
}

/// DM-CREATE-MEMORY-TIER-ENTRY-LOCATION
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
fn verify_dm_create_memory_tier_entry_location() {
    let (c, ep) = component_with_ep();
    let p = any_ptr();
    let size: u32 = kani::any();
    let r = c.create_memory_tier_entry(7, p, size);
    kani::cover!(r.is_ok());
    if r.is_ok() {
        let s = snap(&c, 7).unwrap();
        assert!(s.is_mem && s.pointer == p as usize && s.size == size && s.ssd.is_none());
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
fn verify_dm_create_memory_tier_entry_location__mutant() {
    let (c, ep) = component_with_ep();
    if c.create_memory_tier_entry(7, any_ptr(), kani::any()).is_ok() {
        assert!(snap(&c, 7).unwrap().ssd.is_some()); // MUTANT
    }
}

/// DM-CREATE-MEMORY-TIER-ENTRY-WRITE-REF
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
fn verify_dm_create_memory_tier_entry_write_ref() {
    let (c, ep) = component_with_ep();
    let r = c.create_memory_tier_entry(7, any_ptr(), kani::any());
    kani::cover!(r.is_ok());
    if r.is_ok() {
        let s = snap(&c, 7).unwrap();
        assert!(s.write_ref == 1 && s.read_ref == 0);
        // readers wait until the creator releases
        assert!(matches!(c.lookup(7), Err(DispatchMapError::Timeout(7))));
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
fn verify_dm_create_memory_tier_entry_write_ref__mutant() {
    let (c, ep) = component_with_ep();
    if c.create_memory_tier_entry(7, any_ptr(), kani::any()).is_ok() {
        assert!(snap(&c, 7).unwrap().write_ref == 0); // MUTANT
    }
}

/// DM-CREATE-MEMORY-TIER-ENTRY-ALREADY-EXISTS (expected to FAIL: see the refutation)
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
fn verify_dm_create_memory_tier_entry_already_exists() {
    let (c, ep) = component_with_ep();
    let e = any_entry();
    let a = snap_entry(&e);
    seed(&c, 7, e);
    let size: u32 = kani::any();   // any size, as the obligation states (expected to FAIL at size 0)
    let r = c.create_memory_tier_entry(7, any_ptr(), size);
    assert!(matches!(r, Err(DispatchMapError::AlreadyExists(7))));
    assert!(snap(&c, 7).unwrap() == a && len(&c) == 1);
}

/// REFUTATION of DM-CREATE-MEMORY-TIER-ENTRY-ALREADY-EXISTS as worded: for an existing key and
/// size 0 the call returns InvalidSize (lib.rs:387, checked first), not AlreadyExists. An inventory
/// statement conflict with DM-CREATE-MEMORY-TIER-ENTRY-ZERO-SIZE, not a code defect.
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
fn refute_dm_create_memory_tier_entry_already_exists() {
    let (c, ep) = component_with_ep();
    let pool = ensure_pool(&c, &ep);
    let e = any_entry();
    kani::assume(e.write_ref <= 1 && (e.write_ref == 0 || e.read_ref == 0)); // proved invariants
    kani::assume(e.size_blocks <= u32::MAX / 4096);
    seed_tracked(&c, &ep, pool, 7, e);   // a reachable state: invariants hold, entry tracked
    let r = c.create_memory_tier_entry(7, any_ptr(), 0);
    let violated = matches!(r, Err(DispatchMapError::InvalidSize));   // not AlreadyExists
    kani::cover!(violated);
    assert!(violated);
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
fn verify_dm_create_memory_tier_entry_already_exists__mutant() {
    let (c, ep) = component_with_ep();
    seed(&c, 7, any_entry());
    let size: u32 = kani::any();
    kani::assume(size != 0);
    assert!(c.create_memory_tier_entry(7, any_ptr(), size).is_ok()); // MUTANT
}

/// DM-CREATE-MEMORY-TIER-ENTRY-ZERO-SIZE
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
fn verify_dm_create_memory_tier_entry_zero_size() {
    let c = component_bare(); // returns before any eviction-policy use, so none is needed
    let present: bool = kani::any();
    if present {
        seed(&c, 7, any_entry());
    }
    let before = snap(&c, 7);
    let r = c.create_memory_tier_entry(7, any_ptr(), 0);
    assert!(matches!(r, Err(DispatchMapError::InvalidSize)));
    assert!(snap(&c, 7) == before);
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
fn verify_dm_create_memory_tier_entry_zero_size__mutant() {
    let c = component_bare();
    let _ = c.create_memory_tier_entry(7, any_ptr(), 0);
    assert!(snap(&c, 7).is_some()); // MUTANT
}

/// DM-CREATE-MEMORY-TIER-ENTRY-SIZE-BLOCKS
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
fn verify_dm_create_memory_tier_entry_size_blocks() {
    let (c, ep) = component_with_ep();
    let size: u32 = kani::any();
    let r = c.create_memory_tier_entry(7, any_ptr(), size);
    kani::cover!(r.is_ok() && size % 4096 != 0);
    if r.is_ok() {
        assert!(snap(&c, 7).unwrap().size_blocks as u64 == ceil_blocks(size));
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
fn verify_dm_create_memory_tier_entry_size_blocks__mutant() {
    let (c, ep) = component_with_ep();
    let size: u32 = kani::any();
    if c.create_memory_tier_entry(7, any_ptr(), size).is_ok() {
        assert!(snap(&c, 7).unwrap().size_blocks as u64 == size as u64 / 4096); // MUTANT: floor
    }
}

/// REFUTATION of DM-CREATE-MEMORY-TIER-ENTRY-NO-EVICTION-POLICY-ERROR: with a non-zero size and
/// no eviction policy connected, create_memory_tier_entry panics — get_pool_id unwraps the receptacle
/// (lib.rs:391 -> lib.rs:55), and lib.rs:392 unwraps it again — instead of returning an error.
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
#[kani::should_panic]
#[kani::unwind(4)]
fn refute_dm_create_memory_tier_entry_no_eviction_policy_error() {
    let c = component_bare(); // no eviction policy connected
    let size: u32 = kani::any();
    kani::assume(size != 0);
    let _ = c.create_memory_tier_entry(7, any_ptr(), size);
}

/// DM-CREATE-MEMORY-TIER-ENTRY-NO-EVICTION-POLICY-ERROR (expected to FAIL: see the refutation)
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
fn verify_dm_create_memory_tier_entry_no_eviction_policy_error() {
    let c = component_bare();
    let size: u32 = kani::any();
    kani::assume(size != 0);
    let r = c.create_memory_tier_entry(7, any_ptr(), size);
    assert!(r.is_err() && snap(&c, 7).is_none());
}

/// DM-CREATE-MEMORY-TIER-ENTRY-FRAME
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
fn verify_dm_create_memory_tier_entry_frame() {
    let (c, ep) = component_with_ep();
    seed(&c, 9, any_entry());
    let b = snap(&c, 9).unwrap();
    let r = c.create_memory_tier_entry(7, any_ptr(), kani::any());
    kani::cover!(r.is_ok());
    assert!(snap(&c, 9).unwrap() == b);
    assert!(len(&c) == if r.is_ok() { 2 } else { 1 });
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
fn verify_dm_create_memory_tier_entry_frame__mutant() {
    let (c, ep) = component_with_ep();
    seed(&c, 9, any_entry());
    let _ = c.create_memory_tier_entry(7, any_ptr(), 4096);
    assert!(len(&c) == 1); // MUTANT
}

/// DM-RECOVER-EXTENT-INSERTS (new key; under D-RANGE-FR-023-388782)
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
fn verify_dm_recover_extent_inserts() {
    let (c, ep) = component_with_ep();
    let off: u64 = kani::any();
    let sb: u32 = kani::any();
    kani::assume(sb <= u32::MAX / 4096); // D-RANGE-FR-023-388782 (for the entry_size half)
    let r = c.recover_extent(7, off, sb);
    assert!(r.is_ok());
    let s = snap(&c, 7).unwrap();
    assert!(!s.is_mem && s.offset == off && s.size_blocks == sb);
    assert!(c.entry_size(7).unwrap() as u64 == sb as u64 * 4096);
    match c.lookup(7) {
        Ok(LookupResult::BlockDevice { offset }) => assert!(offset == off),
        _ => assert!(false),
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
fn verify_dm_recover_extent_inserts__mutant() {
    let (c, ep) = component_with_ep();
    let off: u64 = kani::any();
    let _ = c.recover_extent(7, off, 1);
    assert!(snap(&c, 7).unwrap().offset != off); // MUTANT
}

/// DM-RECOVER-EXTENT-REGISTERS-EVICTION
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
fn verify_dm_recover_extent_registers_eviction() {
    let (c, ep) = component_with_ep();
    let r = c.recover_extent(7, kani::any(), kani::any());
    assert!(r.is_ok());
    let pool = pool_of(&c).unwrap();
    assert!(pool == ep.pool());
    assert!(ep.handle_of(pool, 7) == Some(snap(&c, 7).unwrap().handle));
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
fn verify_dm_recover_extent_registers_eviction__mutant() {
    let (c, ep) = component_with_ep();
    let _ = c.recover_extent(7, kani::any(), kani::any());
    assert!(ep.track_calls() == 0); // MUTANT
}

/// DM-RECOVER-EXTENT-ALREADY-EXISTS
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
fn verify_dm_recover_extent_already_exists() {
    let (c, ep) = component_with_ep();
    let e = any_entry();
    let a = snap_entry(&e);
    seed(&c, 7, e);
    let r = c.recover_extent(7, kani::any(), kani::any());
    assert!(matches!(r, Err(DispatchMapError::AlreadyExists(7))));
    assert!(snap(&c, 7).unwrap() == a && len(&c) == 1 && ep.track_calls() == 0);
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
fn verify_dm_recover_extent_already_exists__mutant() {
    let (c, ep) = component_with_ep();
    seed(&c, 7, any_entry());
    assert!(c.recover_extent(7, kani::any(), kani::any()).is_ok()); // MUTANT
}

/// DM-RECOVER-EXTENT-ZERO-REFS
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
fn verify_dm_recover_extent_zero_refs() {
    let (c, ep) = component_with_ep();
    assert!(c.recover_extent(7, kani::any(), kani::any()).is_ok());
    let s = snap(&c, 7).unwrap();
    assert!(s.read_ref == 0 && s.write_ref == 0);
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
fn verify_dm_recover_extent_zero_refs__mutant() {
    let (c, ep) = component_with_ep();
    let _ = c.recover_extent(7, kani::any(), kani::any());
    assert!(snap(&c, 7).unwrap().write_ref == 1); // MUTANT
}

/// REFUTATION of DM-RECOVER-EXTENT-NO-EVICTION-POLICY-ERROR: with no eviction policy connected,
/// recover_extent panics (get_pool_id unwraps the receptacle, lib.rs:572 -> lib.rs:55; lib.rs:573
/// unwraps again) instead of returning an error.
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
#[kani::should_panic]
#[kani::unwind(4)]
fn refute_dm_recover_extent_no_eviction_policy_error() {
    let c = component_bare(); // no eviction policy connected
    let _ = c.recover_extent(7, kani::any(), kani::any());
}

/// DM-RECOVER-EXTENT-NO-EVICTION-POLICY-ERROR (expected to FAIL: see the refutation)
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
fn verify_dm_recover_extent_no_eviction_policy_error() {
    let c = component_bare();
    let r = c.recover_extent(7, kani::any(), kani::any());
    assert!(r.is_err() && snap(&c, 7).is_none());
}

/// DM-RECOVER-EXTENT-FRAME
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
fn verify_dm_recover_extent_frame() {
    let (c, ep) = component_with_ep();
    seed(&c, 9, any_entry());
    let b = snap(&c, 9).unwrap();
    let r = c.recover_extent(7, kani::any(), kani::any());
    kani::cover!(r.is_ok());
    assert!(snap(&c, 9).unwrap() == b && len(&c) == 2);
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
fn verify_dm_recover_extent_frame__mutant() {
    let (c, ep) = component_with_ep();
    seed(&c, 9, any_entry());
    let _ = c.recover_extent(7, kani::any(), kani::any());
    assert!(len(&c) == 1); // MUTANT
}

/// DM-SET-CHECKSUM-RECORDS
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
fn verify_dm_set_checksum_records() {
    let c = component_bare();
    seed(&c, 7, any_entry());
    let cs: u32 = kani::any();
    kani::assume(cs != 0);
    assert!(c.set_checksum(7, cs).is_ok());
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
fn verify_dm_set_checksum_records__mutant() {
    let c = component_bare();
    seed(&c, 7, any_entry());
    let cs: u32 = kani::any();
    kani::assume(cs != 0);
    let _ = c.set_checksum(7, cs);
    assert!(c.get_checksum(7).is_none()); // MUTANT
}

/// DM-SET-CHECKSUM-NOT-FOUND
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
fn verify_dm_set_checksum_not_found() {
    let c = component_bare();
    seed(&c, 9, any_entry());
    let r = c.set_checksum(7, kani::any());
    assert!(matches!(r, Err(DispatchMapError::KeyNotFound(7))));
    assert!(snap(&c, 7).is_none() && len(&c) == 1);
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
fn verify_dm_set_checksum_not_found__mutant() {
    let c = component_bare();
    seed(&c, 9, any_entry());
    assert!(c.set_checksum(7, kani::any()).is_ok()); // MUTANT
}

/// DM-SET-CHECKSUM-FRAME
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
fn verify_dm_set_checksum_frame() {
    let c = component_bare();
    seed(&c, 7, any_entry());
    seed(&c, 9, any_entry());
    let a = snap(&c, 7).unwrap();
    let b = snap(&c, 9).unwrap();
    let cs: u32 = kani::any();
    assert!(c.set_checksum(7, cs).is_ok());
    let mut want = a;
    want.checksum = cs;
    assert!(snap(&c, 7).unwrap() == want && snap(&c, 9).unwrap() == b);
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
fn verify_dm_set_checksum_frame__mutant() {
    let c = component_bare();
    seed(&c, 7, any_entry());
    let a = snap(&c, 7).unwrap();
    let cs: u32 = kani::any();
    kani::assume(cs != a.checksum);
    let _ = c.set_checksum(7, cs);
    assert!(snap(&c, 7).unwrap() == a); // MUTANT: the checksum does change
}

/// DM-GET-CHECKSUM-NONE-UNSET (entries added by create_memory_tier_entry / recover_extent / initialize)
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
fn verify_dm_get_checksum_none_unset() {
    let (c, ep) = component_with_ep();
    let way: u8 = kani::any();
    kani::assume(way < 3);
    let ok = if way == 0 {
        c.create_memory_tier_entry(7, any_ptr(), kani::any()).is_ok()
    } else if way == 1 {
        c.recover_extent(7, kani::any(), kani::any()).is_ok()
    } else {
        connect_em(&c, vec![Extent { key: 7, size: kani::any(), offset: kani::any() }]);
        c.initialize().is_ok()
    };
    kani::cover!(ok);
    if ok {
        assert!(c.get_checksum(7).is_none());
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
fn verify_dm_get_checksum_none_unset__mutant() {
    let (c, ep) = component_with_ep();
    if c.recover_extent(7, kani::any(), kani::any()).is_ok() {
        assert!(c.get_checksum(7).is_some()); // MUTANT
    }
}

/// DM-GET-CHECKSUM-ZERO-IS-NONE
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
fn verify_dm_get_checksum_zero_is_none() {
    let c = component_bare();
    let e = any_entry();
    let cs = e.checksum;
    seed(&c, 7, e);
    let g = c.get_checksum(7);
    if cs == 0 { assert!(g.is_none()); } else { assert!(g == Some(cs)); }
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
fn verify_dm_get_checksum_zero_is_none__mutant() {
    let c = component_bare();
    let e = any_entry();
    let cs = e.checksum;
    seed(&c, 7, e);
    assert!(c.get_checksum(7) == Some(cs)); // MUTANT: zero reported as a value
}

/// DM-GET-CHECKSUM-ABSENT-KEY
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
fn verify_dm_get_checksum_absent_key() {
    let c = component_bare();
    seed(&c, 9, any_entry());
    assert!(c.get_checksum(7).is_none());
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
fn verify_dm_get_checksum_absent_key__mutant() {
    let c = component_bare();
    seed(&c, 9, any_entry());
    assert!(c.get_checksum(7).is_some()); // MUTANT
}

/// DM-GET-CHECKSUM-FRAME
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
fn verify_dm_get_checksum_frame() {
    let c = component_bare();
    seed(&c, 7, any_entry());
    seed(&c, 9, any_entry());
    let a = snap(&c, 7).unwrap();
    let b = snap(&c, 9).unwrap();
    let _ = c.get_checksum(7);
    let _ = c.get_checksum(8);
    assert!(snap(&c, 7).unwrap() == a && snap(&c, 9).unwrap() == b && len(&c) == 2);
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
fn verify_dm_get_checksum_frame__mutant() {
    let c = component_bare();
    seed(&c, 7, any_entry());
    let a = snap(&c, 7).unwrap();
    let _ = c.get_checksum(7);
    assert!(snap(&c, 7).unwrap() != a); // MUTANT
}

