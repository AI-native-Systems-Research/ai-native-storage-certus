// Tier transitions: convert_to_storage, convert_memory_tier_to_block, promote_block_to_memory_tier,
// is_evictable, try_evict_to_block — the REAL IDispatchMap impl on the REAL entry representation
// (geometry: see proofs_refs.rs).
use crate::entry::{DispatchEntry, Location};
use crate::mocks::*;
use crate::*;
use interfaces::{DispatchMapError, Extent, IDispatchMap, LookupResult};

/// DM-CONVERT-TO-STORAGE-SETS-SSD-OFFSET
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
fn verify_dm_convert_to_storage_sets_ssd_offset() {
    let c = component_bare();
    let e = any_entry();
    kani::assume(snap_entry(&e).is_mem);
    seed(&c, 7, e);
    let off: u64 = kani::any();
    let r = c.convert_to_storage(7, off);
    kani::cover!(r.is_ok());
    if r.is_ok() {
        assert!(snap(&c, 7).unwrap().ssd == Some(off));
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
fn verify_dm_convert_to_storage_sets_ssd_offset__mutant() {
    let c = component_bare();
    let e = any_entry();
    kani::assume(snap_entry(&e).is_mem);
    seed(&c, 7, e);
    let off: u64 = kani::any();
    if c.convert_to_storage(7, off).is_ok() {
        assert!(snap(&c, 7).unwrap().ssd.is_none()); // MUTANT
    }
}

/// DM-CONVERT-TO-STORAGE-STAYS-MEMORY-TIER
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
fn verify_dm_convert_to_storage_stays_memory_tier() {
    let c = component_bare();
    let e = any_entry();
    let a = snap_entry(&e);
    kani::assume(a.is_mem);
    seed(&c, 7, e);
    let r = c.convert_to_storage(7, kani::any());
    kani::cover!(r.is_ok());
    if r.is_ok() {
        let s = snap(&c, 7).unwrap();
        assert!(s.is_mem && s.pointer == a.pointer && s.size == a.size);
        // and lookup reports the same memory-tier location, whenever lookup can return one
        if s.write_ref == 0 && s.read_ref < u32::MAX {
            match c.lookup(7) {
                Ok(LookupResult::MemoryTier { pointer, size }) => {
                    assert!(pointer as usize == a.pointer && size == a.size)
                }
                _ => assert!(false),
            }
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
fn verify_dm_convert_to_storage_stays_memory_tier__mutant() {
    let c = component_bare();
    let e = any_entry();
    kani::assume(snap_entry(&e).is_mem);
    seed(&c, 7, e);
    if c.convert_to_storage(7, kani::any()).is_ok() {
        assert!(!snap(&c, 7).unwrap().is_mem); // MUTANT
    }
}

/// DM-CONVERT-TO-STORAGE-RELEASES-READ-REF
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
fn verify_dm_convert_to_storage_releases_read_ref() {
    let c = component_bare();
    let e = any_entry();
    let before = e.read_ref;
    seed(&c, 7, e);
    let r = c.convert_to_storage(7, kani::any());
    kani::cover!(r.is_ok() && before > 0);
    kani::cover!(r.is_ok() && before == 0);
    if r.is_ok() {
        let after = snap(&c, 7).unwrap().read_ref;
        if before > 0 { assert!(after == before - 1); } else { assert!(after == 0); }
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
fn verify_dm_convert_to_storage_releases_read_ref__mutant() {
    let c = component_bare();
    let e = any_entry();
    let before = e.read_ref;
    seed(&c, 7, e);
    if c.convert_to_storage(7, kani::any()).is_ok() {
        assert!(snap(&c, 7).unwrap().read_ref == before); // MUTANT
    }
}

/// DM-CONVERT-TO-STORAGE-FRAME
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
fn verify_dm_convert_to_storage_frame() {
    let c = component_bare();
    seed(&c, 7, any_entry());
    seed(&c, 9, any_entry());
    let a = snap(&c, 7).unwrap();
    let b = snap(&c, 9).unwrap();
    let _ = c.convert_to_storage(7, kani::any());
    let a2 = snap(&c, 7).unwrap();
    assert!(a2.write_ref == a.write_ref && a2.size_blocks == a.size_blocks && a2.handle == a.handle);
    assert!(a2.checksum == a.checksum);
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
fn verify_dm_convert_to_storage_frame__mutant() {
    let c = component_bare();
    seed(&c, 7, any_entry());
    let a = snap(&c, 7).unwrap();
    let _ = c.convert_to_storage(7, kani::any());
    assert!(snap(&c, 7).unwrap() == a); // MUTANT: ssd/read_ref are not in the frame
}

/// DM-CONVERT-TO-STORAGE-NOT-FOUND
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
fn verify_dm_convert_to_storage_not_found() {
    let c = component_bare();
    seed(&c, 9, any_entry());
    let b = snap(&c, 9).unwrap();
    let r = c.convert_to_storage(7, kani::any());
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
fn verify_dm_convert_to_storage_not_found__mutant() {
    let c = component_bare();
    seed(&c, 9, any_entry());
    assert!(c.convert_to_storage(7, kani::any()).is_ok()); // MUTANT
}

/// DM-CONVERT-TO-STORAGE-BLOCK-DEVICE
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
fn verify_dm_convert_to_storage_block_device() {
    let c = component_bare();
    let e = any_entry();
    let a = snap_entry(&e);
    kani::assume(!a.is_mem);
    seed(&c, 7, e);
    let r = c.convert_to_storage(7, kani::any());
    assert!(matches!(r, Err(DispatchMapError::InvalidState(_))));
    assert!(snap(&c, 7).unwrap() == a);
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
fn verify_dm_convert_to_storage_block_device__mutant() {
    let c = component_bare();
    let e = any_entry();
    kani::assume(!snap_entry(&e).is_mem);
    seed(&c, 7, e);
    assert!(c.convert_to_storage(7, kani::any()).is_ok()); // MUTANT
}

/// DM-CONVERT-MEMORY-TIER-TO-BLOCK-TRANSITION
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
fn verify_dm_convert_memory_tier_to_block_transition() {
    let c = component_bare();
    let e = any_entry();
    let a = snap_entry(&e);
    kani::assume(a.is_mem && a.ssd.is_some());
    let x = a.ssd.unwrap();
    seed(&c, 7, e);
    let r = c.convert_memory_tier_to_block(7);
    assert!(r.is_ok());
    let s = snap(&c, 7).unwrap();
    assert!(!s.is_mem && s.offset == x);
    if s.write_ref == 0 && s.read_ref < u32::MAX {
        match c.lookup(7) {
            Ok(LookupResult::BlockDevice { offset }) => assert!(offset == x),
            _ => assert!(false),
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
fn verify_dm_convert_memory_tier_to_block_transition__mutant() {
    let c = component_bare();
    let e = any_entry();
    let a = snap_entry(&e);
    kani::assume(a.is_mem && a.ssd.is_some());
    seed(&c, 7, e);
    let _ = c.convert_memory_tier_to_block(7);
    assert!(snap(&c, 7).unwrap().is_mem); // MUTANT
}

/// DM-CONVERT-MEMORY-TIER-TO-BLOCK-NO-SSD-OFFSET
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
fn verify_dm_convert_memory_tier_to_block_no_ssd_offset() {
    let c = component_bare();
    let e = any_entry();
    let a = snap_entry(&e);
    kani::assume(a.is_mem && a.ssd.is_none());
    seed(&c, 7, e);
    let r = c.convert_memory_tier_to_block(7);
    assert!(matches!(r, Err(DispatchMapError::InvalidState(_))));
    assert!(snap(&c, 7).unwrap() == a);
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
fn verify_dm_convert_memory_tier_to_block_no_ssd_offset__mutant() {
    let c = component_bare();
    let e = any_entry();
    let a = snap_entry(&e);
    kani::assume(a.is_mem && a.ssd.is_none());
    seed(&c, 7, e);
    assert!(c.convert_memory_tier_to_block(7).is_ok()); // MUTANT
}

/// DM-CONVERT-MEMORY-TIER-TO-BLOCK-NOT-MEMORY-TIER
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
fn verify_dm_convert_memory_tier_to_block_not_memory_tier() {
    let c = component_bare();
    let e = any_entry();
    let a = snap_entry(&e);
    kani::assume(!a.is_mem);
    seed(&c, 7, e);
    let r = c.convert_memory_tier_to_block(7);
    assert!(matches!(r, Err(DispatchMapError::InvalidState(_))));
    assert!(snap(&c, 7).unwrap() == a);
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
fn verify_dm_convert_memory_tier_to_block_not_memory_tier__mutant() {
    let c = component_bare();
    let e = any_entry();
    kani::assume(!snap_entry(&e).is_mem);
    seed(&c, 7, e);
    assert!(c.convert_memory_tier_to_block(7).is_ok()); // MUTANT
}

/// DM-CONVERT-MEMORY-TIER-TO-BLOCK-NOT-FOUND
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
fn verify_dm_convert_memory_tier_to_block_not_found() {
    let c = component_bare();
    seed(&c, 9, any_entry());
    let b = snap(&c, 9).unwrap();
    let r = c.convert_memory_tier_to_block(7);
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
fn verify_dm_convert_memory_tier_to_block_not_found__mutant() {
    let c = component_bare();
    seed(&c, 9, any_entry());
    assert!(c.convert_memory_tier_to_block(7).is_ok()); // MUTANT
}

/// DM-CONVERT-MEMORY-TIER-TO-BLOCK-FRAME
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
fn verify_dm_convert_memory_tier_to_block_frame() {
    let c = component_bare();
    seed(&c, 7, any_entry());
    seed(&c, 9, any_entry());
    let a = snap(&c, 7).unwrap();
    let b = snap(&c, 9).unwrap();
    let r = c.convert_memory_tier_to_block(7);
    kani::cover!(r.is_ok());
    let a2 = snap(&c, 7).unwrap();
    assert!(a2.read_ref == a.read_ref && a2.write_ref == a.write_ref);
    assert!(a2.size_blocks == a.size_blocks && a2.handle == a.handle && a2.checksum == a.checksum);
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
fn verify_dm_convert_memory_tier_to_block_frame__mutant() {
    let c = component_bare();
    seed(&c, 7, any_entry());
    let a = snap(&c, 7).unwrap();
    let _ = c.convert_memory_tier_to_block(7);
    assert!(snap(&c, 7).unwrap() == a); // MUTANT: the location is not in the frame
}

/// DM-PROMOTE-TRANSITION
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
fn verify_dm_promote_transition() {
    let c = component_bare();
    let e = any_entry();
    let a = snap_entry(&e);
    kani::assume(!a.is_mem);
    seed(&c, 7, e);
    let p = any_ptr();
    let size: u32 = kani::any();
    let r = c.promote_block_to_memory_tier(7, p, size);
    kani::cover!(r.is_ok());
    if r.is_ok() {
        let s = snap(&c, 7).unwrap();
        assert!(s.is_mem && s.pointer == p as usize && s.size == size && s.ssd == Some(a.offset));
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
fn verify_dm_promote_transition__mutant() {
    let c = component_bare();
    let e = any_entry();
    kani::assume(!snap_entry(&e).is_mem);
    seed(&c, 7, e);
    if c.promote_block_to_memory_tier(7, any_ptr(), kani::any()).is_ok() {
        assert!(snap(&c, 7).unwrap().ssd.is_none()); // MUTANT
    }
}

/// DM-PROMOTE-RECOMPUTES-SIZE (under D-RANGE-FR-003-8e9f6a)
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
fn verify_dm_promote_recomputes_size() {
    let c = component_bare();
    let e = any_entry();
    seed(&c, 7, e);
    let size: u32 = kani::any();
    kani::assume(size <= u32::MAX - 4095); // D-RANGE-FR-003-8e9f6a
    let r = c.promote_block_to_memory_tier(7, any_ptr(), size);
    kani::cover!(r.is_ok());
    if r.is_ok() {
        let v = c.entry_size(7);
        assert!(matches!(v, Ok(_)));
        assert!(v.unwrap() as u64 == ceil_blocks(size) * 4096);
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
fn verify_dm_promote_recomputes_size__mutant() {
    let c = component_bare();
    let e = any_entry();
    kani::assume(!snap_entry(&e).is_mem);
    let old = e.size_blocks;
    seed(&c, 7, e);
    let size: u32 = kani::any();
    kani::assume(size <= u32::MAX - 4095);
    if c.promote_block_to_memory_tier(7, any_ptr(), size).is_ok() {
        assert!(snap(&c, 7).unwrap().size_blocks == old); // MUTANT: size not recomputed
    }
}

/// DM-PROMOTE-PRESERVES-REFS
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
fn verify_dm_promote_preserves_refs() {
    let c = component_bare();
    let e = any_entry();
    let a = snap_entry(&e);
    seed(&c, 7, e);
    seed(&c, 9, any_entry());
    let b = snap(&c, 9).unwrap();
    let size: u32 = kani::any();
    let r = c.promote_block_to_memory_tier(7, any_ptr(), size);
    kani::cover!(r.is_ok() && a.read_ref > 0);
    // the reference counts never block it: a block-device entry with a valid size is promoted
    if !a.is_mem && size != 0 { assert!(r.is_ok()); }
    let s = snap(&c, 7).unwrap();       // kept in the map
    assert!(s.handle == a.handle && s.read_ref == a.read_ref && s.write_ref == a.write_ref);
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
fn verify_dm_promote_preserves_refs__mutant() {
    let c = component_bare();
    let e = any_entry();
    let a = snap_entry(&e);
    kani::assume(!a.is_mem && a.read_ref > 0);
    seed(&c, 7, e);
    let size: u32 = kani::any();
    kani::assume(size != 0);
    assert!(c.promote_block_to_memory_tier(7, any_ptr(), size).is_err()); // MUTANT: refuses pinned
}

/// DM-PROMOTE-ALREADY-MEMORY-TIER (expected to FAIL: see the refutation)
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
fn verify_dm_promote_already_memory_tier() {
    let c = component_bare();
    let e = any_entry();
    let a = snap_entry(&e);
    kani::assume(a.is_mem);
    seed(&c, 7, e);
    let size: u32 = kani::any();   // any size, as the obligation states (expected to FAIL at size 0)
    let r = c.promote_block_to_memory_tier(7, any_ptr(), size);
    assert!(matches!(r, Err(DispatchMapError::InvalidState(_))));
    assert!(snap(&c, 7).unwrap() == a);
}

/// REFUTATION of DM-PROMOTE-ALREADY-MEMORY-TIER as worded: on an entry already in the memory
/// tier with size 0, promote returns InvalidSize (size check first, lib.rs:473), not InvalidState.
/// An inventory statement conflict with DM-PROMOTE-ZERO-SIZE, not a code defect.
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
fn refute_dm_promote_already_memory_tier() {
    let c = component_bare();
    let e = any_entry();
    let a = snap_entry(&e);
    kani::assume(a.is_mem);
    kani::assume(e.write_ref <= 1 && (e.write_ref == 0 || e.read_ref == 0)); // proved invariants
    kani::assume(e.size_blocks as u64 == ceil_blocks(a.size) && a.size > 0); // proved invariants
    seed(&c, 7, e);
    let r = c.promote_block_to_memory_tier(7, any_ptr(), 0);
    let violated = matches!(r, Err(DispatchMapError::InvalidSize));   // not InvalidState
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
fn verify_dm_promote_already_memory_tier__mutant() {
    let c = component_bare();
    let e = any_entry();
    kani::assume(snap_entry(&e).is_mem);
    seed(&c, 7, e);
    let size: u32 = kani::any();
    kani::assume(size != 0);
    assert!(c.promote_block_to_memory_tier(7, any_ptr(), size).is_ok()); // MUTANT
}

/// DM-PROMOTE-NOT-FOUND
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
fn verify_dm_promote_not_found() {
    let c = component_bare();
    seed(&c, 9, any_entry());
    let b = snap(&c, 9).unwrap();
    let size: u32 = kani::any();
    kani::assume(size != 0);
    let r = c.promote_block_to_memory_tier(7, any_ptr(), size);
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
fn verify_dm_promote_not_found__mutant() {
    let c = component_bare();
    seed(&c, 9, any_entry());
    let size: u32 = kani::any();
    kani::assume(size != 0);
    assert!(c.promote_block_to_memory_tier(7, any_ptr(), size).is_ok()); // MUTANT
}

/// DM-PROMOTE-ZERO-SIZE
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
fn verify_dm_promote_zero_size() {
    let c = component_bare();
    let present: bool = kani::any();
    if present {
        seed(&c, 7, any_entry());
    }
    let a = snap(&c, 7);
    let r = c.promote_block_to_memory_tier(7, any_ptr(), 0);
    kani::cover!(present);
    assert!(matches!(r, Err(DispatchMapError::InvalidSize)));
    assert!(snap(&c, 7) == a);
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
fn verify_dm_promote_zero_size__mutant() {
    let c = component_bare();
    let e = any_entry();
    kani::assume(!snap_entry(&e).is_mem);
    seed(&c, 7, e);
    assert!(c.promote_block_to_memory_tier(7, any_ptr(), 0).is_ok()); // MUTANT
}

/// DM-INV-HANDLE-STABLE: across every operation that keeps an entry in the map — demotion
/// (convert_memory_tier_to_block, try_evict_to_block), promotion, and every reference/size/checksum
/// operation — the entry's eviction handle is unchanged. Pre-state: any entry, tracked in the pool.
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
fn verify_dm_inv_handle_stable() {
    let (c, ep) = component_with_ep();
    let pool = ensure_pool(&c, &ep);
    let e = any_entry();
    kani::assume(e.size_blocks <= u32::MAX / 4096); // entries inside the agreed range (D-RANGE-FR-003/FR-023)
    let h = e.eviction_handle;
    seed_tracked(&c, &ep, pool, 7, e);
    // candidate operations: OP_PROMOTE, OP_CONVERT_MTB, OP_TRY_EVICT, OP_CONVERT_TO_STORAGE, OP_LOOKUP, OP_TAKE_READ, OP_TAKE_WRITE, OP_RELEASE_READ, OP_RELEASE_WRITE, OP_DOWNGRADE, OP_TOUCH, OP_SET_CHECKSUM, OP_ENTRY_SIZE, OP_IS_EVICTABLE
    let (op, _err) = { let s: u8 = kani::any(); kani::assume((s as usize) < 14); if s == 0 { run_op_const(&c, 7, OP_PROMOTE) } else if s == 1 { run_op_const(&c, 7, OP_CONVERT_MTB) } else if s == 2 { run_op_const(&c, 7, OP_TRY_EVICT) } else if s == 3 { run_op_const(&c, 7, OP_CONVERT_TO_STORAGE) } else if s == 4 { run_op_const(&c, 7, OP_LOOKUP) } else if s == 5 { run_op_const(&c, 7, OP_TAKE_READ) } else if s == 6 { run_op_const(&c, 7, OP_TAKE_WRITE) } else if s == 7 { run_op_const(&c, 7, OP_RELEASE_READ) } else if s == 8 { run_op_const(&c, 7, OP_RELEASE_WRITE) } else if s == 9 { run_op_const(&c, 7, OP_DOWNGRADE) } else if s == 10 { run_op_const(&c, 7, OP_TOUCH) } else if s == 11 { run_op_const(&c, 7, OP_SET_CHECKSUM) } else if s == 12 { run_op_const(&c, 7, OP_ENTRY_SIZE) } else { run_op_const(&c, 7, OP_IS_EVICTABLE) } };
    kani::cover!(op == OP_PROMOTE);
    let s = snap(&c, 7).unwrap(); // none of these operations removes the entry
    assert!(s.handle == h);
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
fn verify_dm_inv_handle_stable__mutant() {
    let (c, ep) = component_with_ep();
    let pool = ensure_pool(&c, &ep);
    let e = any_entry();
    kani::assume(snap_entry(&e).is_mem == false);
    let h = e.eviction_handle;
    seed_tracked(&c, &ep, pool, 7, e);
    let size: u32 = kani::any();
    kani::assume(size != 0);
    let _ = c.promote_block_to_memory_tier(7, any_ptr(), size);
    assert!(snap(&c, 7).unwrap().handle != h); // MUTANT
}

/// DM-IS-EVICTABLE-EXACT-CONDITION
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
fn verify_dm_is_evictable_exact_condition() {
    let c = component_bare();
    let present: bool = kani::any();
    let mut want = false;
    if present {
        let e = any_entry();
        let a = snap_entry(&e);
        want = a.is_mem && a.ssd.is_some() && a.read_ref == 0 && a.write_ref == 0;
        seed(&c, 7, e);
    }
    let got = c.is_evictable(7);
    kani::cover!(got);
    assert!(got == want);
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
fn verify_dm_is_evictable_exact_condition__mutant() {
    let c = component_bare();
    let e = any_entry();
    let a = snap_entry(&e);
    let want = a.is_mem && a.ssd.is_some() && a.read_ref == 0;  // MUTANT: write_ref ignored
    seed(&c, 7, e);
    assert!(c.is_evictable(7) == want);
}

/// DM-IS-EVICTABLE-NOT-FOUND
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
fn verify_dm_is_evictable_not_found() {
    let c = component_bare();
    seed(&c, 9, any_entry());
    assert!(!c.is_evictable(7));
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
fn verify_dm_is_evictable_not_found__mutant() {
    let c = component_bare();
    seed(&c, 9, any_entry());
    assert!(c.is_evictable(7)); // MUTANT
}

/// DM-IS-EVICTABLE-FRAME (no entry, count, or eviction-ordering change)
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
fn verify_dm_is_evictable_frame() {
    let (c, ep) = component_with_ep();
    seed(&c, 7, any_entry());
    seed(&c, 9, any_entry());
    let a = snap(&c, 7).unwrap();
    let b = snap(&c, 9).unwrap();
    let _ = c.is_evictable(7);
    assert!(snap(&c, 7).unwrap() == a && snap(&c, 9).unwrap() == b && len(&c) == 2);
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
fn verify_dm_is_evictable_frame__mutant() {
    let (c, ep) = component_with_ep();
    seed(&c, 7, any_entry());
    let a = snap(&c, 7).unwrap();
    let _ = c.is_evictable(7);
    assert!(snap(&c, 7).unwrap() != a); // MUTANT
}

/// DM-IS-EVICTABLE-AGREES-WITH-TRY-EVICT (same state: is_evictable does not change it)
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
fn verify_dm_is_evictable_agrees_with_try_evict() {
    let c = component_bare();
    if kani::any() {
        seed(&c, 7, any_entry());
    }
    let b = c.is_evictable(7);
    let r = c.try_evict_to_block(7);
    kani::cover!(b);
    assert!(b == r.is_ok());
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
fn verify_dm_is_evictable_agrees_with_try_evict__mutant() {
    let c = component_bare();
    seed(&c, 7, any_entry());
    let b = c.is_evictable(7);
    let r = c.try_evict_to_block(7);
    assert!(b == r.is_err()); // MUTANT
}

/// DM-TRY-EVICT-TO-BLOCK-SUCCESS
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
fn verify_dm_try_evict_to_block_success() {
    let c = component_bare();
    let e = any_entry();
    let a = snap_entry(&e);
    seed(&c, 7, e);
    let r = c.try_evict_to_block(7);
    kani::cover!(r.is_ok());
    if r.is_ok() {
        assert!(a.is_mem && a.ssd.is_some());
        let x = a.ssd.unwrap();
        let s = snap(&c, 7).unwrap();
        assert!(!s.is_mem && s.offset == x);
        match c.lookup(7) {
            Ok(LookupResult::BlockDevice { offset }) => assert!(offset == x),
            _ => assert!(false, "lookup after eviction must report the block-device offset"),
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
fn verify_dm_try_evict_to_block_success__mutant() {
    let c = component_bare();
    seed(&c, 7, any_entry());
    if c.try_evict_to_block(7).is_ok() {
        assert!(snap(&c, 7).unwrap().is_mem); // MUTANT
    }
}

/// DM-TRY-EVICT-TO-BLOCK-ACTIVE-REFS
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
fn verify_dm_try_evict_to_block_active_refs() {
    let c = component_bare();
    let e = any_entry();
    let a = snap_entry(&e);
    kani::assume(a.is_mem && (a.read_ref != 0 || a.write_ref != 0));
    seed(&c, 7, e);
    let r = c.try_evict_to_block(7);
    assert!(matches!(r, Err(DispatchMapError::InvalidState(_))));
    assert!(snap(&c, 7).unwrap() == a);
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
fn verify_dm_try_evict_to_block_active_refs__mutant() {
    let c = component_bare();
    let e = any_entry();
    let a = snap_entry(&e);
    kani::assume(a.is_mem && a.ssd.is_some() && a.read_ref != 0);
    seed(&c, 7, e);
    assert!(c.try_evict_to_block(7).is_ok()); // MUTANT
}

/// DM-TRY-EVICT-TO-BLOCK-NO-SSD-OFFSET
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
fn verify_dm_try_evict_to_block_no_ssd_offset() {
    let c = component_bare();
    let e = any_entry();
    let a = snap_entry(&e);
    kani::assume(a.is_mem && a.ssd.is_none());
    seed(&c, 7, e);
    let r = c.try_evict_to_block(7);
    assert!(matches!(r, Err(DispatchMapError::InvalidState(_))));
    assert!(snap(&c, 7).unwrap() == a);
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
fn verify_dm_try_evict_to_block_no_ssd_offset__mutant() {
    let c = component_bare();
    let e = any_entry();
    let a = snap_entry(&e);
    kani::assume(a.is_mem && a.ssd.is_none() && a.read_ref == 0 && a.write_ref == 0);
    seed(&c, 7, e);
    assert!(c.try_evict_to_block(7).is_ok()); // MUTANT
}

/// DM-TRY-EVICT-TO-BLOCK-NOT-MEMORY-TIER
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
fn verify_dm_try_evict_to_block_not_memory_tier() {
    let c = component_bare();
    let e = any_entry();
    let a = snap_entry(&e);
    kani::assume(!a.is_mem);
    seed(&c, 7, e);
    let r = c.try_evict_to_block(7);
    assert!(matches!(r, Err(DispatchMapError::InvalidState(_))));
    assert!(snap(&c, 7).unwrap() == a);
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
fn verify_dm_try_evict_to_block_not_memory_tier__mutant() {
    let c = component_bare();
    let e = any_entry();
    let a = snap_entry(&e);
    kani::assume(!a.is_mem && a.read_ref == 0 && a.write_ref == 0);
    seed(&c, 7, e);
    assert!(c.try_evict_to_block(7).is_ok()); // MUTANT
}

/// DM-TRY-EVICT-TO-BLOCK-NOT-FOUND
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
fn verify_dm_try_evict_to_block_not_found() {
    let c = component_bare();
    seed(&c, 9, any_entry());
    let b = snap(&c, 9).unwrap();
    let r = c.try_evict_to_block(7);
    assert!(matches!(r, Err(DispatchMapError::KeyNotFound(7))));
    assert!(snap(&c, 7).is_none() && snap(&c, 9).unwrap() == b);
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
fn verify_dm_try_evict_to_block_not_found__mutant() {
    let c = component_bare();
    seed(&c, 9, any_entry());
    assert!(c.try_evict_to_block(7).is_ok()); // MUTANT
}

/// DM-TRY-EVICT-TO-BLOCK-FRAME
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
fn verify_dm_try_evict_to_block_frame() {
    let (c, ep) = component_with_ep();
    let pool = ensure_pool(&c, &ep);
    seed_tracked(&c, &ep, pool, 7, any_entry());
    seed(&c, 9, any_entry());
    let a = snap(&c, 7).unwrap();
    let b = snap(&c, 9).unwrap();
    let r = c.try_evict_to_block(7);
    kani::cover!(r.is_ok());
    let a2 = snap(&c, 7).unwrap();
    assert!(a2.read_ref == a.read_ref && a2.write_ref == a.write_ref);
    assert!(a2.size_blocks == a.size_blocks && a2.handle == a.handle && a2.checksum == a.checksum);
    assert!(ep.remove_calls() == 0 && ep.is_tracked(pool, 7)); // still tracked
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
fn verify_dm_try_evict_to_block_frame__mutant() {
    let (c, ep) = component_with_ep();
    let pool = ensure_pool(&c, &ep);
    seed_tracked(&c, &ep, pool, 7, any_entry());
    let a = snap(&c, 7).unwrap();
    let _ = c.try_evict_to_block(7);
    assert!(snap(&c, 7).unwrap() == a); // MUTANT: the location is not in the frame
}

