// Reference-count operations: take_read, take_write, release_read, release_write,
// downgrade_reference — driven through the REAL IDispatchMap impl on the REAL
// Mutex<HashMap<CacheKey, DispatchEntry>>.
//
// Geometry: key 7 is the entry under test, key 9 a bystander for frame obligations (both
// concrete — symbolic keys drive SipHash over symbolic bytes, see the skill). Every other field
// of each entry is SYMBOLIC (`any_entry`): any location, any pointer/offset/size, any reference
// counts, any handle, any checksum. A harness narrows that only by what its obligation states.
//
// Waiting: lookup/take_read/take_write wait on a Condvar. Under the single-threaded model in
// mocks.rs the wait can only end at its deadline (no other thread exists to release), so a
// blocked call returns Timeout, which is exactly the obligation's "remains held for the whole
// waiting period" premise. Interleavings are not modelled here.
use crate::entry::{DispatchEntry, Location};
use crate::mocks::*;
use crate::*;
use interfaces::{DispatchMapError, IDispatchMap};

// ===================================================================================== take_read

/// DM-TAKE-READ-INCREMENTS: on success read_ref is exactly one higher.
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
fn verify_dm_take_read_increments() {
    let c = component_bare();
    let e = any_entry();
    let before = e.read_ref;
    seed(&c, 7, e);
    let r = c.take_read(7);
    kani::cover!(r.is_ok());
    if r.is_ok() {
        assert!(snap(&c, 7).unwrap().read_ref == before + 1);
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
fn verify_dm_take_read_increments__mutant() {
    let c = component_bare();
    let e = any_entry();
    let before = e.read_ref;
    seed(&c, 7, e);
    let r = c.take_read(7);
    if r.is_ok() {
        assert!(snap(&c, 7).unwrap().read_ref == before); // MUTANT: claims no increment
    }
}

/// DM-TAKE-READ-WAITS-FOR-WRITER: a read reference is acquired only when no write reference
/// is held; while a writer holds the entry the call does not acquire.
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
fn verify_dm_take_read_waits_for_writer() {
    let c = component_bare();
    let e = any_entry();
    let (w, rr) = (e.write_ref, e.read_ref);
    seed(&c, 7, e);
    let r = c.take_read(7);
    kani::cover!(w != 0);
    if r.is_ok() {
        assert!(w == 0);
    }
    if w != 0 {
        assert!(r.is_err());
        assert!(snap(&c, 7).unwrap().read_ref == rr);
        assert!(waits() >= 1); // it waited rather than returning at once
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
fn verify_dm_take_read_waits_for_writer__mutant() {
    let c = component_bare();
    let e = any_entry();
    let w = e.write_ref;
    seed(&c, 7, e);
    let r = c.take_read(7);
    if w != 0 {
        assert!(r.is_ok()); // MUTANT: claims it acquires despite a writer
    }
}

/// DM-TAKE-READ-TIMEOUT: writer held for the whole wait -> Timeout, read_ref unchanged.
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
fn verify_dm_take_read_timeout() {
    let c = component_bare();
    let e = any_entry();
    kani::assume(e.write_ref != 0);
    let rr = e.read_ref;
    seed(&c, 7, e);
    let r = c.take_read(7);
    assert!(matches!(r, Err(DispatchMapError::Timeout(7))));
    assert!(snap(&c, 7).unwrap().read_ref == rr);
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
fn verify_dm_take_read_timeout__mutant() {
    let c = component_bare();
    let e = any_entry();
    kani::assume(e.write_ref != 0);
    let rr = e.read_ref;
    seed(&c, 7, e);
    let _ = c.take_read(7);
    assert!(snap(&c, 7).unwrap().read_ref != rr); // MUTANT
}

/// DM-TAKE-READ-NOT-FOUND: absent key -> KeyNotFound, no entry created.
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
fn verify_dm_take_read_not_found() {
    let c = component_bare();
    seed(&c, 9, any_entry());
    let r = c.take_read(7);
    assert!(matches!(r, Err(DispatchMapError::KeyNotFound(7))));
    assert!(snap(&c, 7).is_none());
    assert!(len(&c) == 1);
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
fn verify_dm_take_read_not_found__mutant() {
    let c = component_bare();
    seed(&c, 9, any_entry());
    let r = c.take_read(7);
    assert!(r.is_ok()); // MUTANT
}

/// DM-TAKE-READ-REFCOUNT-OVERFLOW: read_ref at u32::MAX -> RefCountOverflow, unchanged.
/// Pre-state satisfies the proved invariants (so no writer is held at read_ref == u32::MAX).
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
fn verify_dm_take_read_refcount_overflow() {
    let c = component_bare();
    let e = any_entry();
    kani::assume(e.read_ref == u32::MAX);
    kani::assume(e.write_ref <= 1 && (e.write_ref == 0 || e.read_ref == 0)); // proved invariants DM-INV-WRITE-REF-AT-MOST-ONE + DM-INV-READ-WRITE-EXCLUSIVE
    seed(&c, 7, e);
    let r = c.take_read(7);
    assert!(matches!(r, Err(DispatchMapError::RefCountOverflow(7))));
    assert!(snap(&c, 7).unwrap().read_ref == u32::MAX);
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
fn verify_dm_take_read_refcount_overflow__mutant() {
    let c = component_bare();
    let e = any_entry();
    kani::assume(e.read_ref == u32::MAX);
    kani::assume(e.write_ref == 0);
    seed(&c, 7, e);
    let _ = c.take_read(7);
    assert!(snap(&c, 7).unwrap().read_ref == 0); // MUTANT: wrapped
}

/// DM-TAKE-READ-FRAME: location, size, write_ref, handle unchanged; eviction ranking not
/// touched (no IEvictionPolicy call); other entries unchanged.
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
fn verify_dm_take_read_frame() {
    let (c, ep) = component_with_ep();
    seed(&c, 7, any_entry());
    seed(&c, 9, any_entry());
    let a = snap(&c, 7).unwrap();
    let b = snap(&c, 9).unwrap();
    let _ = c.take_read(7);
    let a2 = snap(&c, 7).unwrap();
    assert!(a2.is_mem == a.is_mem && a2.offset == a.offset && a2.pointer == a.pointer);
    assert!(a2.size == a.size && a2.ssd == a.ssd && a2.size_blocks == a.size_blocks);
    assert!(a2.write_ref == a.write_ref && a2.handle == a.handle && a2.checksum == a.checksum);
    assert!(snap(&c, 9).unwrap() == b);
    assert!(ep.touch_calls() == 0 && ep.track_calls() == 0 && ep.remove_calls() == 0);
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
fn verify_dm_take_read_frame__mutant() {
    let (c, ep) = component_with_ep();
    seed(&c, 7, any_entry());
    let a = snap(&c, 7).unwrap();
    let _ = c.take_read(7);
    assert!(snap(&c, 7).unwrap() == a); // MUTANT: read_ref is not in the frame
}

// ==================================================================================== take_write

/// DM-TAKE-WRITE-SETS-WRITE-REF
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
fn verify_dm_take_write_sets_write_ref() {
    let c = component_bare();
    seed(&c, 7, any_entry());
    let r = c.take_write(7);
    kani::cover!(r.is_ok());
    if r.is_ok() {
        assert!(snap(&c, 7).unwrap().write_ref == 1);
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
fn verify_dm_take_write_sets_write_ref__mutant() {
    let c = component_bare();
    seed(&c, 7, any_entry());
    let r = c.take_write(7);
    if r.is_ok() {
        assert!(snap(&c, 7).unwrap().write_ref == 0); // MUTANT
    }
}

/// DM-TAKE-WRITE-REQUIRES-NO-REFS: acquires only when r == 0 && w == 0; otherwise waits.
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
fn verify_dm_take_write_requires_no_refs() {
    let c = component_bare();
    let e = any_entry();
    let (w, rr) = (e.write_ref, e.read_ref);
    seed(&c, 7, e);
    let r = c.take_write(7);
    kani::cover!(rr != 0 || w != 0);
    if r.is_ok() {
        assert!(rr == 0 && w == 0);
    }
    if rr != 0 || w != 0 {
        assert!(r.is_err());
        assert!(waits() >= 1);
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
fn verify_dm_take_write_requires_no_refs__mutant() {
    let c = component_bare();
    let e = any_entry();
    let rr = e.read_ref;
    seed(&c, 7, e);
    let r = c.take_write(7);
    if rr != 0 {
        assert!(r.is_ok()); // MUTANT: writer admitted alongside readers
    }
}

/// DM-TAKE-WRITE-TIMEOUT: refs held for the whole wait -> Timeout, counts unchanged.
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
fn verify_dm_take_write_timeout() {
    let c = component_bare();
    let e = any_entry();
    kani::assume(e.read_ref != 0 || e.write_ref != 0);
    let (w, rr) = (e.write_ref, e.read_ref);
    seed(&c, 7, e);
    let r = c.take_write(7);
    assert!(matches!(r, Err(DispatchMapError::Timeout(7))));
    let s = snap(&c, 7).unwrap();
    assert!(s.read_ref == rr && s.write_ref == w);
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
fn verify_dm_take_write_timeout__mutant() {
    let c = component_bare();
    let e = any_entry();
    kani::assume(e.read_ref != 0 || e.write_ref != 0);
    seed(&c, 7, e);
    let _ = c.take_write(7);
    assert!(snap(&c, 7).unwrap().write_ref == 1); // MUTANT: write ref taken anyway
}

/// DM-TAKE-WRITE-NOT-FOUND
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
fn verify_dm_take_write_not_found() {
    let c = component_bare();
    seed(&c, 9, any_entry());
    let r = c.take_write(7);
    assert!(matches!(r, Err(DispatchMapError::KeyNotFound(7))));
    assert!(snap(&c, 7).is_none());
    assert!(len(&c) == 1);
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
fn verify_dm_take_write_not_found__mutant() {
    let c = component_bare();
    seed(&c, 9, any_entry());
    let _ = c.take_write(7);
    assert!(snap(&c, 7).is_some()); // MUTANT
}

/// DM-TAKE-WRITE-FRAME: location, size, read_ref, handle unchanged; other entries unchanged.
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
fn verify_dm_take_write_frame() {
    let c = component_bare();
    seed(&c, 7, any_entry());
    seed(&c, 9, any_entry());
    let a = snap(&c, 7).unwrap();
    let b = snap(&c, 9).unwrap();
    let _ = c.take_write(7);
    let a2 = snap(&c, 7).unwrap();
    assert!(a2.is_mem == a.is_mem && a2.offset == a.offset && a2.pointer == a.pointer);
    assert!(a2.size == a.size && a2.ssd == a.ssd && a2.size_blocks == a.size_blocks);
    assert!(a2.read_ref == a.read_ref && a2.handle == a.handle && a2.checksum == a.checksum);
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
fn verify_dm_take_write_frame__mutant() {
    let c = component_bare();
    seed(&c, 7, any_entry());
    let a = snap(&c, 7).unwrap();
    let _ = c.take_write(7);
    assert!(snap(&c, 7).unwrap() == a); // MUTANT: write_ref is not in the frame
}

// ================================================================================== release_read

/// DM-RELEASE-READ-DECREMENTS
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
fn verify_dm_release_read_decrements() {
    let c = component_bare();
    let e = any_entry();
    let before = e.read_ref;
    seed(&c, 7, e);
    let r = c.release_read(7);
    kani::cover!(r.is_ok());
    if r.is_ok() {
        assert!(snap(&c, 7).unwrap().read_ref == before - 1);
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
fn verify_dm_release_read_decrements__mutant() {
    let c = component_bare();
    let e = any_entry();
    let before = e.read_ref;
    seed(&c, 7, e);
    let r = c.release_read(7);
    if r.is_ok() {
        assert!(snap(&c, 7).unwrap().read_ref == before); // MUTANT
    }
}

/// DM-RELEASE-READ-UNDERFLOW
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
fn verify_dm_release_read_underflow() {
    let c = component_bare();
    let e = any_entry();
    kani::assume(e.read_ref == 0);
    seed(&c, 7, e);
    let r = c.release_read(7);
    assert!(matches!(r, Err(DispatchMapError::RefCountUnderflow(7))));
    assert!(snap(&c, 7).unwrap().read_ref == 0);
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
fn verify_dm_release_read_underflow__mutant() {
    let c = component_bare();
    let e = any_entry();
    kani::assume(e.read_ref == 0);
    seed(&c, 7, e);
    let r = c.release_read(7);
    assert!(r.is_ok()); // MUTANT
}

/// DM-RELEASE-READ-NOT-FOUND
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
fn verify_dm_release_read_not_found() {
    let c = component_bare();
    seed(&c, 9, any_entry());
    let b = snap(&c, 9).unwrap();
    let r = c.release_read(7);
    assert!(matches!(r, Err(DispatchMapError::KeyNotFound(7))));
    assert!(snap(&c, 7).is_none() && len(&c) == 1 && snap(&c, 9).unwrap() == b);
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
fn verify_dm_release_read_not_found__mutant() {
    let c = component_bare();
    seed(&c, 9, any_entry());
    let r = c.release_read(7);
    assert!(r.is_ok()); // MUTANT
}

/// DM-RELEASE-READ-FRAME
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
fn verify_dm_release_read_frame() {
    let c = component_bare();
    seed(&c, 7, any_entry());
    seed(&c, 9, any_entry());
    let a = snap(&c, 7).unwrap();
    let b = snap(&c, 9).unwrap();
    let _ = c.release_read(7);
    let a2 = snap(&c, 7).unwrap();
    assert!(a2.is_mem == a.is_mem && a2.offset == a.offset && a2.pointer == a.pointer);
    assert!(a2.size == a.size && a2.ssd == a.ssd && a2.size_blocks == a.size_blocks);
    assert!(a2.write_ref == a.write_ref && a2.handle == a.handle && a2.checksum == a.checksum);
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
fn verify_dm_release_read_frame__mutant() {
    let c = component_bare();
    seed(&c, 7, any_entry());
    let a = snap(&c, 7).unwrap();
    let _ = c.release_read(7);
    assert!(snap(&c, 7).unwrap() == a); // MUTANT: read_ref is not in the frame
}

// ================================================================================= release_write

/// DM-RELEASE-WRITE-CLEARS
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
fn verify_dm_release_write_clears() {
    let c = component_bare();
    seed(&c, 7, any_entry());
    let r = c.release_write(7);
    kani::cover!(r.is_ok());
    if r.is_ok() {
        assert!(snap(&c, 7).unwrap().write_ref == 0);
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
fn verify_dm_release_write_clears__mutant() {
    let c = component_bare();
    seed(&c, 7, any_entry());
    let r = c.release_write(7);
    if r.is_ok() {
        assert!(snap(&c, 7).unwrap().write_ref != 0); // MUTANT
    }
}

/// DM-RELEASE-WRITE-UNDERFLOW
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
fn verify_dm_release_write_underflow() {
    let c = component_bare();
    let e = any_entry();
    kani::assume(e.write_ref == 0);
    seed(&c, 7, e);
    let r = c.release_write(7);
    assert!(matches!(r, Err(DispatchMapError::RefCountUnderflow(7))));
    assert!(snap(&c, 7).unwrap().write_ref == 0);
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
fn verify_dm_release_write_underflow__mutant() {
    let c = component_bare();
    let e = any_entry();
    kani::assume(e.write_ref == 0);
    seed(&c, 7, e);
    let r = c.release_write(7);
    assert!(r.is_ok()); // MUTANT
}

/// DM-RELEASE-WRITE-NOT-FOUND
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
fn verify_dm_release_write_not_found() {
    let c = component_bare();
    seed(&c, 9, any_entry());
    let b = snap(&c, 9).unwrap();
    let r = c.release_write(7);
    assert!(matches!(r, Err(DispatchMapError::KeyNotFound(7))));
    assert!(snap(&c, 7).is_none() && len(&c) == 1 && snap(&c, 9).unwrap() == b);
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
fn verify_dm_release_write_not_found__mutant() {
    let c = component_bare();
    seed(&c, 9, any_entry());
    let r = c.release_write(7);
    assert!(r.is_ok()); // MUTANT
}

/// DM-RELEASE-WRITE-FRAME
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
fn verify_dm_release_write_frame() {
    let c = component_bare();
    seed(&c, 7, any_entry());
    seed(&c, 9, any_entry());
    let a = snap(&c, 7).unwrap();
    let b = snap(&c, 9).unwrap();
    let _ = c.release_write(7);
    let a2 = snap(&c, 7).unwrap();
    assert!(a2.is_mem == a.is_mem && a2.offset == a.offset && a2.pointer == a.pointer);
    assert!(a2.size == a.size && a2.ssd == a.ssd && a2.size_blocks == a.size_blocks);
    assert!(a2.read_ref == a.read_ref && a2.handle == a.handle && a2.checksum == a.checksum);
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
fn verify_dm_release_write_frame__mutant() {
    let c = component_bare();
    seed(&c, 7, any_entry());
    let a = snap(&c, 7).unwrap();
    let _ = c.release_write(7);
    assert!(snap(&c, 7).unwrap() == a); // MUTANT: write_ref is not in the frame
}

// =========================================================================== downgrade_reference

/// DM-DOWNGRADE-REFERENCE-SWAPS: on success w == 0 and r == before + 1, in one critical
/// section (the whole update happens under one lock acquisition; interleaving not modelled).
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
fn verify_dm_downgrade_reference_swaps() {
    let c = component_bare();
    let e = any_entry();
    let before = e.read_ref;
    seed(&c, 7, e);
    let r = c.downgrade_reference(7);
    kani::cover!(r.is_ok());
    if r.is_ok() {
        let s = snap(&c, 7).unwrap();
        assert!(s.write_ref == 0 && s.read_ref == before + 1);
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
fn verify_dm_downgrade_reference_swaps__mutant() {
    let c = component_bare();
    let e = any_entry();
    let before = e.read_ref;
    seed(&c, 7, e);
    let r = c.downgrade_reference(7);
    if r.is_ok() {
        assert!(snap(&c, 7).unwrap().read_ref == before); // MUTANT
    }
}

/// DM-DOWNGRADE-REFERENCE-NO-WRITE-REF
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
fn verify_dm_downgrade_reference_no_write_ref() {
    let c = component_bare();
    let e = any_entry();
    kani::assume(e.write_ref == 0);
    let rr = e.read_ref;
    seed(&c, 7, e);
    let r = c.downgrade_reference(7);
    assert!(matches!(r, Err(DispatchMapError::NoWriteReference(7))));
    let s = snap(&c, 7).unwrap();
    assert!(s.write_ref == 0 && s.read_ref == rr);
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
fn verify_dm_downgrade_reference_no_write_ref__mutant() {
    let c = component_bare();
    let e = any_entry();
    kani::assume(e.write_ref == 0);
    seed(&c, 7, e);
    let r = c.downgrade_reference(7);
    assert!(r.is_ok()); // MUTANT
}

/// DM-DOWNGRADE-REFERENCE-NOT-FOUND
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
fn verify_dm_downgrade_reference_not_found() {
    let c = component_bare();
    seed(&c, 9, any_entry());
    let b = snap(&c, 9).unwrap();
    let r = c.downgrade_reference(7);
    assert!(matches!(r, Err(DispatchMapError::KeyNotFound(7))));
    assert!(snap(&c, 7).is_none() && len(&c) == 1 && snap(&c, 9).unwrap() == b);
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
fn verify_dm_downgrade_reference_not_found__mutant() {
    let c = component_bare();
    seed(&c, 9, any_entry());
    let r = c.downgrade_reference(7);
    assert!(r.is_ok()); // MUTANT
}

/// DM-DOWNGRADE-REFERENCE-FRAME: location, size, handle, other entries unchanged.
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
fn verify_dm_downgrade_reference_frame() {
    let c = component_bare();
    seed(&c, 7, any_entry());
    seed(&c, 9, any_entry());
    let a = snap(&c, 7).unwrap();
    let b = snap(&c, 9).unwrap();
    let _ = c.downgrade_reference(7);
    let a2 = snap(&c, 7).unwrap();
    assert!(a2.is_mem == a.is_mem && a2.offset == a.offset && a2.pointer == a.pointer);
    assert!(a2.size == a.size && a2.ssd == a.ssd && a2.size_blocks == a.size_blocks);
    assert!(a2.handle == a.handle && a2.checksum == a.checksum);
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
fn verify_dm_downgrade_reference_frame__mutant() {
    let c = component_bare();
    seed(&c, 7, any_entry());
    let a = snap(&c, 7).unwrap();
    let _ = c.downgrade_reference(7);
    assert!(snap(&c, 7).unwrap() == a); // MUTANT: refs are not in the frame
}
