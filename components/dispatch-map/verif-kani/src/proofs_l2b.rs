// Level-2 harnesses for the six properties the classifier moved into level 2 on 2026-10-06
// (class A): eviction-use reporting (create / lookup / touch / oldest_keys order), key locality,
// and touch-never-waits. The REAL IDispatchMap impl, same geometry and stubs as proofs_misc.rs.
//
// What dispatch-map itself owns in the eviction-ORDER statements: it reports every use of an
// entry to the connected eviction policy (track on create/recover, touch on lookup/touch, in the
// order the uses happen, under the entry's own handle, in the map's own pool) and oldest_keys
// returns exactly the list the policy returns. The order itself ("least recently used first") is
// the policy's guarantee and is delegated to eviction-policy in kani_advisory.yaml; the recording
// MockEp (mocks.rs) logs the calls in order (`events`) and the exact candidate list it returned.
//
// Pre-states: entries are seeded with `seed_tracked`, i.e. present <=> tracked in the map's pool
// under the entry's own handle — the invariant DM-INV-ENTRY-TRACKED-FOR-EVICTION, proved
// inductive in proofs_inv.rs.
use crate::entry::{DispatchEntry, Location};
use crate::mocks::*;
use crate::*;
use interfaces::{CacheKey, DispatchMapError, IDispatchMap, LookupResult};

/// oldest_keys(n) for a small symbolic n, checked to be the policy's exact answer.
fn oldest_keys_is_policy_answer(c: &DispatchMapComponent, ep: &MockEp) -> bool {
    let n: usize = kani::any();
    kani::assume(n <= 3);
    let before = ep.cand_calls();
    let v = c.oldest_keys(n);
    ep.cand_calls() == before + 1 && ep.same_as_last_cands(&v)
}

/// Every operation that names a key (initialize names none), chosen symbolically, on key `k`.
fn run_keyed_op(c: &DispatchMapComponent, k: CacheKey) -> u8 {
    let s: u8 = kani::any();
    kani::assume(s < 16);
    let op = match s {
        0 => OP_LOOKUP, 1 => OP_CONVERT_TO_STORAGE, 2 => OP_TAKE_READ, 3 => OP_TAKE_WRITE,
        4 => OP_RELEASE_READ, 5 => OP_RELEASE_WRITE, 6 => OP_DOWNGRADE, 7 => OP_REMOVE,
        8 => OP_TOUCH, 9 => OP_ENTRY_SIZE, 10 => OP_CREATE, 11 => OP_CONVERT_MTB,
        12 => OP_PROMOTE, 13 => OP_TRY_EVICT, 14 => OP_RECOVER, _ => OP_SET_CHECKSUM,
    };
    // literal dispatch, so symbolic execution enters only the arm that runs
    if op == OP_LOOKUP { run_op_const(c, k, OP_LOOKUP); }
    else if op == OP_CONVERT_TO_STORAGE { run_op_const(c, k, OP_CONVERT_TO_STORAGE); }
    else if op == OP_TAKE_READ { run_op_const(c, k, OP_TAKE_READ); }
    else if op == OP_TAKE_WRITE { run_op_const(c, k, OP_TAKE_WRITE); }
    else if op == OP_RELEASE_READ { run_op_const(c, k, OP_RELEASE_READ); }
    else if op == OP_RELEASE_WRITE { run_op_const(c, k, OP_RELEASE_WRITE); }
    else if op == OP_DOWNGRADE { run_op_const(c, k, OP_DOWNGRADE); }
    else if op == OP_REMOVE { run_op_const(c, k, OP_REMOVE); }
    else if op == OP_TOUCH { run_op_const(c, k, OP_TOUCH); }
    else if op == OP_ENTRY_SIZE { run_op_const(c, k, OP_ENTRY_SIZE); }
    else if op == OP_CREATE { run_op_const(c, k, OP_CREATE); }
    else if op == OP_CONVERT_MTB { run_op_const(c, k, OP_CONVERT_MTB); }
    else if op == OP_PROMOTE { run_op_const(c, k, OP_PROMOTE); }
    else if op == OP_TRY_EVICT { run_op_const(c, k, OP_TRY_EVICT); }
    else if op == OP_RECOVER { run_op_const(c, k, OP_RECOVER); }
    else { run_op_const(c, k, OP_SET_CHECKSUM); }
    op
}

/// Operations that are NOT a use of the entry (not create/recover/lookup/touch, and not remove,
/// which ends tracking), chosen symbolically, on key `k`.
fn run_non_use_op(c: &DispatchMapComponent, k: CacheKey) {
    let s: u8 = kani::any();
    kani::assume(s < 12);
    if s == 0 { run_op_const(c, k, OP_TAKE_READ); }
    else if s == 1 { run_op_const(c, k, OP_TAKE_WRITE); }
    else if s == 2 { run_op_const(c, k, OP_RELEASE_READ); }
    else if s == 3 { run_op_const(c, k, OP_RELEASE_WRITE); }
    else if s == 4 { run_op_const(c, k, OP_DOWNGRADE); }
    else if s == 5 { run_op_const(c, k, OP_CONVERT_TO_STORAGE); }
    else if s == 6 { run_op_const(c, k, OP_ENTRY_SIZE); }
    else if s == 7 { run_op_const(c, k, OP_IS_EVICTABLE); }
    else if s == 8 { run_op_const(c, k, OP_CONVERT_MTB); }
    else if s == 9 { run_op_const(c, k, OP_PROMOTE); }
    else if s == 10 { run_op_const(c, k, OP_TRY_EVICT); }
    else { run_op_const(c, k, OP_SET_CHECKSUM); }
}

/// DM-CREATE-MEMORY-TIER-ENTRY-REGISTERS-EVICTION. dispatch-map's part: a successful create
/// registers the new key with the policy (one track, in the map's own pool, nothing else reported),
/// stores the handle the policy returned, and oldest_keys hands back the policy's list unchanged.
/// "As the most recently used / creation order" is the policy's track semantics (delegated).
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
fn verify_dm_create_memory_tier_entry_registers_eviction() {
    let (c, ep) = component_with_ep();
    let pool = ensure_pool(&c, &ep);
    seed_tracked(&c, &ep, pool, 9, any_entry()); // a bystander already tracked
    let size: u32 = kani::any();
    let r = c.create_memory_tier_entry(7, any_ptr(), size);
    kani::cover!(r.is_ok());
    if r.is_ok() {
        let s = snap(&c, 7).unwrap();
        assert!(ep.track_calls() == 1 && ep.other_pool_calls() == 0);
        assert!(ep.nevents() == 1 && ep.event(0) == Some(Ev::Track(7, s.handle)));
        assert!(ep.handle_of(pool, 7) == Some(s.handle));
        assert!(ep.handle_of(pool, 9).is_some());
        assert!(oldest_keys_is_policy_answer(&c, &ep));
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
fn verify_dm_create_memory_tier_entry_registers_eviction__mutant() {
    let (c, ep) = component_with_ep();
    let pool = ensure_pool(&c, &ep);
    seed_tracked(&c, &ep, pool, 9, any_entry());
    let size: u32 = kani::any();
    let r = c.create_memory_tier_entry(7, any_ptr(), size);
    if r.is_ok() {
        assert!(ep.track_calls() == 0); // MUTANT: the new key is in fact registered
    }
}

/// DM-LOOKUP-REFRESHES-EVICTION-PRIORITY. dispatch-map's part: a successful lookup of an existing
/// entry reports exactly one use — touch with that entry's own handle — and nothing else; the
/// "listed after all others" ordering is the policy's touch semantics (delegated).
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
fn verify_dm_lookup_refreshes_eviction_priority() {
    let (c, ep) = component_with_ep();
    let pool = ensure_pool(&c, &ep);
    seed_tracked(&c, &ep, pool, 7, any_entry());
    seed_tracked(&c, &ep, pool, 9, any_entry());
    let h7 = ep.handle_of(pool, 7).unwrap();
    let r = c.lookup(7);
    let found = matches!(r, Ok(LookupResult::BlockDevice { .. }) | Ok(LookupResult::MemoryTier { .. }));
    kani::cover!(found);
    if found {
        assert!(ep.nevents() == 1 && ep.event(0) == Some(Ev::Touch(h7)));
        assert!(ep.touch_calls() == 1 && ep.track_calls() == 0 && ep.remove_calls() == 0);
        assert!(oldest_keys_is_policy_answer(&c, &ep));
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
fn verify_dm_lookup_refreshes_eviction_priority__mutant() {
    let (c, ep) = component_with_ep();
    let pool = ensure_pool(&c, &ep);
    seed_tracked(&c, &ep, pool, 7, any_entry());
    seed_tracked(&c, &ep, pool, 9, any_entry());
    let h9 = ep.handle_of(pool, 9).unwrap();
    let r = c.lookup(7);
    if r.is_ok() {
        assert!(ep.event(0) == Some(Ev::Touch(h9))); // MUTANT: it is 7's handle that is touched
    }
}

/// DM-TOUCH-REFRESHES-PRIORITY. dispatch-map's part: a successful touch reports exactly one use —
/// touch with the entry's own handle — and nothing else; the resulting position is the policy's
/// touch semantics (delegated).
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
fn verify_dm_touch_refreshes_priority() {
    let (c, ep) = component_with_ep();
    let pool = ensure_pool(&c, &ep);
    seed_tracked(&c, &ep, pool, 7, any_entry());
    seed_tracked(&c, &ep, pool, 9, any_entry());
    let h7 = ep.handle_of(pool, 7).unwrap();
    let r = c.touch(7);
    kani::cover!(r.is_ok());
    if r.is_ok() {
        assert!(ep.nevents() == 1 && ep.event(0) == Some(Ev::Touch(h7)));
        assert!(ep.touch_calls() == 1 && ep.track_calls() == 0 && ep.remove_calls() == 0);
        assert!(oldest_keys_is_policy_answer(&c, &ep));
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
fn verify_dm_touch_refreshes_priority__mutant() {
    let (c, ep) = component_with_ep();
    let pool = ensure_pool(&c, &ep);
    seed_tracked(&c, &ep, pool, 7, any_entry());
    let r = c.touch(7);
    if r.is_ok() {
        assert!(ep.nevents() == 0); // MUTANT: the touch is reported
    }
}

/// DM-OLDEST-KEYS-ORDER. dispatch-map's part, on a state built only through real operations:
/// every use (create, recover, lookup, touch) is reported to the policy, in the order it happened,
/// under the right key/handle; operations that are not a use report nothing; and oldest_keys returns
/// exactly the policy's list. "Least recently used first" is the policy's ordering (delegated).
/// (Recovery during initialize is covered by DM-INITIALIZE-TRACKS-EVICTION.)
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
fn verify_dm_oldest_keys_order() {
    let (c, ep) = component_with_ep();
    assert!(c.create_memory_tier_entry(1, any_ptr(), 4096).is_ok()); // use of 1
    assert!(c.release_write(1).is_ok());                              // writer done (not a use)
    assert!(c.recover_extent(2, kani::any(), 1).is_ok());             // use of 2
    let pool = pool_of(&c).unwrap();
    let h1 = ep.handle_of(pool, 1).unwrap();
    let h2 = ep.handle_of(pool, 2).unwrap();
    let k: CacheKey = if kani::any() { 1 } else { 2 };
    let hk = if k == 1 { h1 } else { h2 };
    let looked_up = kani::any::<bool>();
    let used = if looked_up { c.lookup(k).is_ok() } else { c.touch(k).is_ok() }; // use of k
    assert!(used);
    kani::cover!(looked_up && k == 1);
    assert!(!ep.events_overflow() && ep.nevents() == 3);
    assert!(ep.event(0) == Some(Ev::Track(1, h1)));
    assert!(ep.event(1) == Some(Ev::Track(2, h2)));
    assert!(ep.event(2) == Some(Ev::Touch(hk)));
    let other: CacheKey = if kani::any() { 1 } else { 2 };
    run_non_use_op(&c, other); // not a use: nothing reported
    assert!(!ep.events_overflow() && ep.nevents() == 3);
    assert!(ep.other_pool_calls() == 0);
    assert!(oldest_keys_is_policy_answer(&c, &ep));
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
fn verify_dm_oldest_keys_order__mutant() {
    let (c, ep) = component_with_ep();
    assert!(c.create_memory_tier_entry(1, any_ptr(), 4096).is_ok());
    assert!(c.recover_extent(2, kani::any(), 1).is_ok());
    let pool = pool_of(&c).unwrap();
    let h2 = ep.handle_of(pool, 2).unwrap();
    assert!(ep.event(0) == Some(Ev::Track(2, h2))); // MUTANT: 1 was used first
}

/// DM-INV-OPERATIONS-ARE-KEY-LOCAL. Any keyed operation (all 16 that name a key; initialize names
/// none) on key k in {7 present, 8 absent} leaves every OTHER key's entry — location, pointer, size,
/// SSD copy, block count, reference counts, handle, checksum — bit-identical, adds no entry for it,
/// and leaves the bystander's eviction tracking alone. Pre-state: two arbitrary tracked entries
/// (DM-INV-ENTRY-TRACKED-FOR-EVICTION); the entry under test is kept inside the range every create /
/// recover produces under D-RANGE-FR-003-8e9f6a / D-RANGE-FR-023-388782.
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
fn verify_dm_inv_operations_are_key_local() {
    let (c, ep) = component_with_ep();
    let pool = ensure_pool(&c, &ep);
    let e7 = any_entry();
    kani::assume(e7.size_blocks <= u32::MAX / 4096); // entries inside the agreed range
    seed_tracked(&c, &ep, pool, 7, e7);
    seed_tracked(&c, &ep, pool, 9, any_entry());
    let s7 = snap(&c, 7).unwrap();
    let s9 = snap(&c, 9).unwrap();
    let h9 = ep.handle_of(pool, 9);
    let k: CacheKey = if kani::any() { 7 } else { 8 };
    let op = run_keyed_op(&c, k);
    kani::cover!(k == 8 && op == OP_CREATE && snap(&c, 8).is_some());
    kani::cover!(k == 7 && op == OP_REMOVE && snap(&c, 7).is_none());
    assert!(snap(&c, 9) == Some(s9));
    assert!(ep.handle_of(pool, 9) == h9);
    if k == 7 { assert!(snap(&c, 8).is_none()); }
    if k == 8 { assert!(snap(&c, 7) == Some(s7)); }
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
fn verify_dm_inv_operations_are_key_local__mutant() {
    let (c, ep) = component_with_ep();
    let pool = ensure_pool(&c, &ep);
    seed_tracked(&c, &ep, pool, 9, any_entry());
    let s9 = snap(&c, 9).unwrap();
    let _ = run_op_const(&c, 9, OP_SET_CHECKSUM); // MUTANT: the operation names 9 itself
    assert!(snap(&c, 9) == Some(s9));
}

/// DM-TOUCH-NO-WAIT, SEQUENTIAL HALF. On an entry holding ANY read/write reference counts, touch
/// returns Ok without ever entering the reference-count wait (the Condvar::wait_timeout stub counts
/// every entry into it), and touch never returns Timeout (absent key included). The concurrent half
/// — completing while ANOTHER THREAD holds a reference, i.e. the map lock is never held across a
/// wait — needs an interleaving checker (Loom); single-threaded Kani does not model it.
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
fn verify_dm_touch_no_wait() {
    let (c, ep) = component_with_ep();
    let pool = ensure_pool(&c, &ep);
    let e = any_entry();
    let held_w = e.write_ref;
    let held_r = e.read_ref;
    seed_tracked(&c, &ep, pool, 7, e);
    let k: CacheKey = if kani::any() { 7 } else { 8 };
    let r = c.touch(k);
    kani::cover!(k == 7 && held_w > 0);
    kani::cover!(k == 7 && held_r > 0);
    assert!(waits() == 0);
    assert!(!matches!(r, Err(DispatchMapError::Timeout(_))));
    if k == 7 { assert!(r.is_ok()); }
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
fn verify_dm_touch_no_wait__mutant() {
    // The wait counter is live: an operation that DOES wait for references (lookup while a write
    // reference is held) is caught by the same assertion.
    let (c, ep) = component_with_ep();
    let pool = ensure_pool(&c, &ep);
    let e = any_entry();
    kani::assume(e.write_ref > 0);
    seed_tracked(&c, &ep, pool, 7, e);
    let _ = c.lookup(7); // MUTANT: lookup in place of touch
    assert!(waits() == 0);
}
