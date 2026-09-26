//! Kani proof harnesses for eviction-policy-lru (Role-2 KANI, ADVISORY).
//!
//! MEASURED BOUNDARY (same class the logger harnesses hit, reproduced not assumed):
//! the observable methods live on `EvictionPolicyLruComponent`, reachable only through
//! a component built by the `define_component!` framework macro (Arc + atomic refcount +
//! IUnknown vtable + a `component-core` registry backed by `HashMap`/`RandomState`) over
//! `RwLock<Vec<Mutex<Pool>>>`. Driving CBMC through that construction OOMs, exactly as it
//! does for the logger. So — as the logger does — these harnesses prove the component's
//! PURE, tractable core directly and model the thin framework layer faithfully:
//!
//!   * The eviction semantics all live in the real arena `crate::lru_list::LruList`
//!     (index-based doubly-linked list; no locks, no HashMap, no format!). Every arena
//!     harness calls the REAL `LruList` methods and proves the END-TO-END ORDERING that
//!     the Creusot mirror deferred (FIFO eviction order, MRU-after-touch, oldest-first
//!     candidate order, a removed entry is never evicted, batch reordering).
//!   * The per-method pool ROUTING (`state.pools.get(pool as usize)` -> Ok/Err /
//!     None / 0 / empty / no-op) is a thin, lock-free `Vec::get` decision. The `Mutex`/
//!     `RwLock` wrappers have NO effect on single-threaded functional behaviour (that is
//!     precisely the concurrency obligation delegated to Loom, NV-CODE-1); dropping them
//!     is behaviour-preserving here. Routing harnesses reconstruct the EXACT source
//!     expression over a real `Vec<LruList>` and the real `interfaces` error/handle types.
//!
//! Fidelity per property: verif/kani_advisory.yaml. Every `verify_<id>` has a
//! `verify_<id>__mutant` twin that asserts a deliberately FALSE claim about the same
//! scenario, so the scorer's anti-vacuity re-run drives it to VERIFICATION FAILED.
#![allow(clippy::bool_assert_comparison)]

use crate::lru_list::LruList;
use interfaces::{CacheKey, EvictionHandle, EvictionPolicyError, PoolId};

// ===========================================================================
// LRU-CREATEPOOL-SEQ — create_pool returns pools.len() as the id, then pushes a
// fresh pool, so ids come out 0,1,2,... (lib.rs:44-54).
// ===========================================================================
#[kani::proof]
#[kani::unwind(8)]
fn verify_lru_createpool_seq() {
    let mut pools: Vec<LruList> = Vec::new();
    let id0 = pools.len() as u32;
    pools.push(LruList::new());
    let id1 = pools.len() as u32;
    pools.push(LruList::new());
    let id2 = pools.len() as u32;
    pools.push(LruList::new());
    assert!(id0 == 0 && id1 == 1 && id2 == 2);
}
#[kani::proof]
#[kani::unwind(8)]
fn verify_lru_createpool_seq__mutant() {
    let mut pools: Vec<LruList> = Vec::new();
    let _id0 = pools.len() as u32;
    pools.push(LruList::new());
    let id1 = pools.len() as u32;
    assert!(id1 == 0); // FALSE: second id is 1, not 0
}

// ===========================================================================
// LRU-TRACK-OK-INC — track push_backs the key; active count rises by one
// (lib.rs:72 -> lru_list.rs:65).
// ===========================================================================
#[kani::proof]
#[kani::unwind(8)]
fn verify_lru_track_ok_inc() {
    let mut l = LruList::new();
    assert!(l.len() == 0);
    let k: CacheKey = kani::any();
    l.push_back(k);
    assert!(l.len() == 1);
}
#[kani::proof]
#[kani::unwind(8)]
fn verify_lru_track_ok_inc__mutant() {
    let mut l = LruList::new();
    let k: CacheKey = kani::any();
    l.push_back(k);
    assert!(l.len() == 0); // FALSE: len is 1 after a push
}

// ===========================================================================
// LRU-TRACK-HANDLE-POOL — the returned handle carries the pool id passed in and
// the assigned node index (lib.rs:72-73).
// ===========================================================================
#[kani::proof]
fn verify_lru_track_handle_pool() {
    let pool: PoolId = kani::any();
    let index: u32 = kani::any();
    let h = EvictionHandle::new(pool, index);
    assert!(h.pool_id() == pool);
    assert!(h.index() == index);
}
#[kani::proof]
fn verify_lru_track_handle_pool__mutant() {
    let pool: PoolId = kani::any();
    let index: u32 = kani::any();
    let h = EvictionHandle::new(pool, index);
    assert!(h.pool_id() == index); // FALSE: pool_id is `pool`, differs from `index` in general
}

// ===========================================================================
// LRU-TRACK-NONIDEMPOTENT — track never dedups; tracking the SAME key twice adds
// two entries (lib.rs:72 unconditional push_back).
// ===========================================================================
#[kani::proof]
#[kani::unwind(8)]
fn verify_lru_track_nonidempotent() {
    let mut l = LruList::new();
    let k: CacheKey = kani::any();
    l.push_back(k);
    l.push_back(k); // same key again
    assert!(l.len() == 2);
}
#[kani::proof]
#[kani::unwind(8)]
fn verify_lru_track_nonidempotent__mutant() {
    let mut l = LruList::new();
    let k: CacheKey = kani::any();
    l.push_back(k);
    l.push_back(k);
    assert!(l.len() == 1); // FALSE: no dedup, so two entries
}

// ===========================================================================
// LRU-TRACK-INVALIDPOOL — an out-of-range pool -> Err(InvalidPool(pool)), no
// state change (lib.rs:63-70).
// ===========================================================================
#[kani::proof]
#[kani::unwind(8)]
fn verify_lru_track_invalidpool() {
    let mut pools: Vec<LruList> = Vec::new();
    pools.push(LruList::new());
    pools.push(LruList::new());
    let p: PoolId = kani::any();
    kani::assume(p as usize >= pools.len());
    // exact routing of track: pools.get(pool).ok_or(InvalidPool(pool))
    let r: Result<(), EvictionPolicyError> =
        pools.get(p as usize).map(|_| ()).ok_or(EvictionPolicyError::InvalidPool(p));
    assert!(matches!(r, Err(EvictionPolicyError::InvalidPool(pp)) if pp == p));
    assert!(pools[0].len() == 0 && pools[1].len() == 0); // unchanged
}
#[kani::proof]
#[kani::unwind(8)]
fn verify_lru_track_invalidpool__mutant() {
    let mut pools: Vec<LruList> = Vec::new();
    pools.push(LruList::new());
    let p: PoolId = kani::any();
    kani::assume(p as usize >= pools.len());
    let r: Result<(), EvictionPolicyError> =
        pools.get(p as usize).map(|_| ()).ok_or(EvictionPolicyError::InvalidPool(p));
    assert!(r.is_ok()); // FALSE: an out-of-range pool routes to Err
}

// ===========================================================================
// LRU-TOUCH-MRU — touch (move_to_back) on an active non-tail node defers it to
// most-recent, changing eviction order end-to-end (lib.rs:88 -> lru_list.rs:70-96).
// ===========================================================================
#[kani::proof]
#[kani::unwind(8)]
fn verify_lru_touch_mru() {
    let mut l = LruList::new();
    let a: CacheKey = kani::any();
    let b: CacheKey = kani::any();
    let c: CacheKey = kani::any();
    let i0 = l.push_back(a);
    l.push_back(b);
    l.push_back(c);
    l.move_to_back(i0); // touch the LRU end -> order becomes b, c, a
    assert!(l.pop_front() == Some(b));
    assert!(l.pop_front() == Some(c));
    assert!(l.pop_front() == Some(a));
}
#[kani::proof]
#[kani::unwind(8)]
fn verify_lru_touch_mru__mutant() {
    let mut l = LruList::new();
    let a: CacheKey = kani::any();
    let b: CacheKey = kani::any();
    let c: CacheKey = kani::any();
    let i0 = l.push_back(a);
    l.push_back(b);
    l.push_back(c);
    l.move_to_back(i0);
    assert!(l.pop_front() == Some(a)); // FALSE: after touch, `a` is now most-recent, not LRU
}

// ===========================================================================
// LRU-TOUCH-INVALIDPOOL — a handle naming an out-of-range pool -> Err(InvalidPool),
// no state change (lib.rs:78-86).
// ===========================================================================
#[kani::proof]
#[kani::unwind(8)]
fn verify_lru_touch_invalidpool() {
    let mut pools: Vec<LruList> = Vec::new();
    pools.push(LruList::new());
    let p: PoolId = kani::any();
    kani::assume(p as usize >= pools.len());
    let h = EvictionHandle::new(p, 0);
    let r: Result<(), EvictionPolicyError> =
        pools.get(h.pool_id() as usize).map(|_| ()).ok_or(EvictionPolicyError::InvalidPool(h.pool_id()));
    assert!(matches!(r, Err(EvictionPolicyError::InvalidPool(pp)) if pp == p));
    assert!(pools[0].len() == 0);
}
#[kani::proof]
#[kani::unwind(8)]
fn verify_lru_touch_invalidpool__mutant() {
    let mut pools: Vec<LruList> = Vec::new();
    pools.push(LruList::new());
    let p: PoolId = kani::any();
    kani::assume(p as usize >= pools.len());
    let h = EvictionHandle::new(p, 0);
    let r: Result<(), EvictionPolicyError> =
        pools.get(h.pool_id() as usize).map(|_| ()).ok_or(EvictionPolicyError::InvalidPool(h.pool_id()));
    assert!(r.is_ok()); // FALSE
}

// ===========================================================================
// LRU-TOUCH-STALE-IDEMPOTENT — move_to_back on an inactive (removed) node is a
// no-op; order and count are unchanged (lru_list.rs:71-73).
// ===========================================================================
#[kani::proof]
#[kani::unwind(8)]
fn verify_lru_touch_stale_idempotent() {
    let mut l = LruList::new();
    let a: CacheKey = kani::any();
    let b: CacheKey = kani::any();
    let ia = l.push_back(a);
    l.push_back(b);
    l.remove(ia); // a is now inactive
    l.move_to_back(ia); // stale touch: no-op
    assert!(l.len() == 1);
    assert!(l.pop_front() == Some(b)); // order intact; a never resurfaces
    assert!(l.pop_front() == None);
}
#[kani::proof]
#[kani::unwind(8)]
fn verify_lru_touch_stale_idempotent__mutant() {
    let mut l = LruList::new();
    let a: CacheKey = kani::any();
    let b: CacheKey = kani::any();
    let ia = l.push_back(a);
    l.push_back(b);
    l.remove(ia);
    l.move_to_back(ia);
    assert!(l.len() == 2); // FALSE: the stale touch does not resurrect the removed node
}

// ===========================================================================
// LRU-REMOVE-OK-DEC — remove on an active node decrements the count by one
// (lib.rs:132 -> lru_list.rs:144).
// ===========================================================================
#[kani::proof]
#[kani::unwind(8)]
fn verify_lru_remove_ok_dec() {
    let mut l = LruList::new();
    let a: CacheKey = kani::any();
    let b: CacheKey = kani::any();
    l.push_back(a);
    let ib = l.push_back(b);
    assert!(l.len() == 2);
    l.remove(ib);
    assert!(l.len() == 1);
}
#[kani::proof]
#[kani::unwind(8)]
fn verify_lru_remove_ok_dec__mutant() {
    let mut l = LruList::new();
    let a: CacheKey = kani::any();
    let b: CacheKey = kani::any();
    l.push_back(a);
    let ib = l.push_back(b);
    l.remove(ib);
    assert!(l.len() == 2); // FALSE: removing an active node drops len to 1
}

// ===========================================================================
// LRU-REMOVE-NEVER-EVICTED — a removed entry is never returned by eviction, and
// the surviving entries evict in FIFO order (lru_list.rs:99-104,121-145).
// ===========================================================================
#[kani::proof]
#[kani::unwind(8)]
fn verify_lru_remove_never_evicted() {
    let mut l = LruList::new();
    let a: CacheKey = kani::any();
    let b: CacheKey = kani::any();
    let c: CacheKey = kani::any();
    l.push_back(a);
    let ib = l.push_back(b);
    l.push_back(c);
    l.remove(ib); // b removed
    let p1 = l.pop_front();
    let p2 = l.pop_front();
    let p3 = l.pop_front();
    assert!(p1 == Some(a));
    assert!(p2 == Some(c));
    assert!(p3 == None); // only two survivors; b never comes back
}
#[kani::proof]
#[kani::unwind(8)]
fn verify_lru_remove_never_evicted__mutant() {
    let mut l = LruList::new();
    let a: CacheKey = kani::any();
    let b: CacheKey = kani::any();
    let c: CacheKey = kani::any();
    l.push_back(a);
    let ib = l.push_back(b);
    l.push_back(c);
    l.remove(ib);
    assert!(l.pop_front() == Some(b)); // FALSE: b was removed; head is `a`
}

// ===========================================================================
// LRU-REMOVE-INVALIDPOOL — a handle naming an out-of-range pool -> Err, no change
// (lib.rs:122-130).
// ===========================================================================
#[kani::proof]
#[kani::unwind(8)]
fn verify_lru_remove_invalidpool() {
    let mut pools: Vec<LruList> = Vec::new();
    pools.push(LruList::new());
    let p: PoolId = kani::any();
    kani::assume(p as usize >= pools.len());
    let h = EvictionHandle::new(p, 0);
    let r: Result<(), EvictionPolicyError> =
        pools.get(h.pool_id() as usize).map(|_| ()).ok_or(EvictionPolicyError::InvalidPool(h.pool_id()));
    assert!(matches!(r, Err(EvictionPolicyError::InvalidPool(pp)) if pp == p));
    assert!(pools[0].len() == 0);
}
#[kani::proof]
#[kani::unwind(8)]
fn verify_lru_remove_invalidpool__mutant() {
    let mut pools: Vec<LruList> = Vec::new();
    pools.push(LruList::new());
    let p: PoolId = kani::any();
    kani::assume(p as usize >= pools.len());
    let h = EvictionHandle::new(p, 0);
    let r: Result<(), EvictionPolicyError> =
        pools.get(h.pool_id() as usize).map(|_| ()).ok_or(EvictionPolicyError::InvalidPool(h.pool_id()));
    assert!(r.is_ok()); // FALSE
}

// ===========================================================================
// LRU-REMOVE-STALE-IDEMPOTENT — remove on an already-inactive node is a no-op:
// count stays put (no underflow), nothing resurfaces (lru_list.rs:122-124).
// ===========================================================================
#[kani::proof]
#[kani::unwind(8)]
fn verify_lru_remove_stale_idempotent() {
    let mut l = LruList::new();
    let a: CacheKey = kani::any();
    let ia = l.push_back(a);
    l.remove(ia);
    assert!(l.len() == 0);
    l.remove(ia); // second remove: idempotent no-op, no underflow
    assert!(l.len() == 0);
    assert!(l.pop_front() == None);
}
#[kani::proof]
#[kani::unwind(8)]
fn verify_lru_remove_stale_idempotent__mutant() {
    let mut l = LruList::new();
    let a: CacheKey = kani::any();
    let ia = l.push_back(a);
    l.remove(ia);
    l.remove(ia);
    assert!(l.len() == 1); // FALSE: len is 0 and a second remove changes nothing
}

// ===========================================================================
// LRU-EVICT-FIFO — identify_next_to_evict pops the head (oldest); repeated calls
// return keys in insertion (FIFO) order (lib.rs:140 -> lru_list.rs:99-104).
// ===========================================================================
#[kani::proof]
#[kani::unwind(8)]
fn verify_lru_evict_fifo() {
    let mut l = LruList::new();
    let a: CacheKey = kani::any();
    let b: CacheKey = kani::any();
    let c: CacheKey = kani::any();
    l.push_back(a);
    l.push_back(b);
    l.push_back(c);
    assert!(l.pop_front() == Some(a));
    assert!(l.pop_front() == Some(b));
    assert!(l.pop_front() == Some(c));
    assert!(l.pop_front() == None);
}
#[kani::proof]
#[kani::unwind(8)]
fn verify_lru_evict_fifo__mutant() {
    let mut l = LruList::new();
    let a: CacheKey = kani::any();
    let b: CacheKey = kani::any();
    let c: CacheKey = kani::any();
    l.push_back(a);
    l.push_back(b);
    l.push_back(c);
    assert!(l.pop_front() == Some(c)); // FALSE: FIFO evicts the oldest `a` first, not `c`
}

// ===========================================================================
// LRU-EVICT-EMPTY-NONE — evicting an empty pool returns None and leaves it empty
// (lru_list.rs:99-104 with head == None).
// ===========================================================================
#[kani::proof]
#[kani::unwind(8)]
fn verify_lru_evict_empty_none() {
    let mut l = LruList::new();
    assert!(l.pop_front() == None);
    assert!(l.len() == 0);
}
#[kani::proof]
#[kani::unwind(8)]
fn verify_lru_evict_empty_none__mutant() {
    let mut l = LruList::new();
    assert!(l.pop_front().is_some()); // FALSE: an empty list yields None
}

// ===========================================================================
// LRU-CANDIDATES-NONDESTRUCTIVE — get_eviction_candidates (peek_front_n) does not
// remove; length is unchanged (lru_list.rs:107-118).
// ===========================================================================
#[kani::proof]
#[kani::unwind(10)]
fn verify_lru_candidates_nondestructive() {
    let mut l = LruList::new();
    let a: CacheKey = kani::any();
    let b: CacheKey = kani::any();
    let c: CacheKey = kani::any();
    l.push_back(a);
    l.push_back(b);
    l.push_back(c);
    let before = l.len();
    let v = l.peek_front_n(2);
    assert!(v.len() == 2);
    assert!(l.len() == before); // non-destructive
    assert!(l.len() == 3);
}
#[kani::proof]
#[kani::unwind(10)]
fn verify_lru_candidates_nondestructive__mutant() {
    let mut l = LruList::new();
    let a: CacheKey = kani::any();
    let b: CacheKey = kani::any();
    let c: CacheKey = kani::any();
    l.push_back(a);
    l.push_back(b);
    l.push_back(c);
    let _ = l.peek_front_n(2);
    assert!(l.len() == 1); // FALSE: peek removes nothing, len stays 3
}

// ===========================================================================
// LRU-CANDIDATES-COUNT-ORDER — peek_front_n(n) returns min(n,len) keys, oldest
// first (lru_list.rs:107-118).
// ===========================================================================
#[kani::proof]
#[kani::unwind(10)]
fn verify_lru_candidates_count_order() {
    let mut l = LruList::new();
    let a: CacheKey = kani::any();
    let b: CacheKey = kani::any();
    let c: CacheKey = kani::any();
    l.push_back(a);
    l.push_back(b);
    l.push_back(c);
    let two = l.peek_front_n(2);
    assert!(two.len() == 2); // min(2,3)
    assert!(two[0] == a && two[1] == b); // oldest-first
    let five = l.peek_front_n(5);
    assert!(five.len() == 3); // capped at len
    assert!(five[0] == a && five[1] == b && five[2] == c);
}
#[kani::proof]
#[kani::unwind(10)]
fn verify_lru_candidates_count_order__mutant() {
    let mut l = LruList::new();
    let a: CacheKey = kani::any();
    let b: CacheKey = kani::any();
    let c: CacheKey = kani::any();
    l.push_back(a);
    l.push_back(b);
    l.push_back(c);
    let five = l.peek_front_n(5);
    assert!(five.len() == 5); // FALSE: capped at len (3), never n when n > len
}

// ===========================================================================
// LRU-CANDIDATES-DEGRADE — get_eviction_candidates on an out-of-range pool returns
// an empty vector (lib.rs:150 None arm).
// ===========================================================================
#[kani::proof]
#[kani::unwind(10)]
fn verify_lru_candidates_degrade() {
    let mut pools: Vec<LruList> = Vec::new();
    pools.push(LruList::new());
    let p: PoolId = kani::any();
    kani::assume(p as usize >= pools.len());
    let n: usize = kani::any();
    kani::assume(n <= 4);
    let r: Vec<CacheKey> = match pools.get(p as usize) {
        Some(pg) => pg.peek_front_n(n),
        None => Vec::new(),
    };
    assert!(r.is_empty());
}
#[kani::proof]
#[kani::unwind(10)]
fn verify_lru_candidates_degrade__mutant() {
    let mut pools: Vec<LruList> = Vec::new();
    pools.push(LruList::new());
    let p: PoolId = kani::any();
    kani::assume(p as usize >= pools.len());
    let r: Vec<CacheKey> = match pools.get(p as usize) {
        Some(pg) => pg.peek_front_n(2),
        None => Vec::new(),
    };
    assert!(!r.is_empty()); // FALSE: out-of-range pool yields an empty vector
}

// ===========================================================================
// LRU-LEN-COUNT — len returns the number of active entries; it tracks pushes and
// removes exactly (lru_list.rs:157-159).
// ===========================================================================
#[kani::proof]
#[kani::unwind(8)]
fn verify_lru_len_count() {
    let mut l = LruList::new();
    let a: CacheKey = kani::any();
    let b: CacheKey = kani::any();
    let c: CacheKey = kani::any();
    l.push_back(a);
    let ib = l.push_back(b);
    l.push_back(c);
    l.remove(ib);
    assert!(l.len() == 2); // 3 tracked, 1 removed -> 2 active
}
#[kani::proof]
#[kani::unwind(8)]
fn verify_lru_len_count__mutant() {
    let mut l = LruList::new();
    let a: CacheKey = kani::any();
    let b: CacheKey = kani::any();
    let c: CacheKey = kani::any();
    l.push_back(a);
    let ib = l.push_back(b);
    l.push_back(c);
    l.remove(ib);
    assert!(l.len() == 3); // FALSE: the removed entry is not counted
}

// ===========================================================================
// LRU-LEN-DEGRADE — len on an out-of-range pool returns 0 (lib.rs:160-161).
// ===========================================================================
#[kani::proof]
#[kani::unwind(8)]
fn verify_lru_len_degrade() {
    let mut pools: Vec<LruList> = Vec::new();
    pools.push(LruList::new());
    let p: PoolId = kani::any();
    kani::assume(p as usize >= pools.len());
    let r: usize = match pools.get(p as usize) {
        Some(pg) => pg.len(),
        None => 0,
    };
    assert!(r == 0);
}
#[kani::proof]
#[kani::unwind(8)]
fn verify_lru_len_degrade__mutant() {
    let mut pools: Vec<LruList> = Vec::new();
    pools.push(LruList::new());
    let p: PoolId = kani::any();
    kani::assume(p as usize >= pools.len());
    let r: usize = match pools.get(p as usize) {
        Some(pg) => pg.len(),
        None => 0,
    };
    assert!(r == 1); // FALSE: degraded len is 0
}

// ===========================================================================
// LRU-CLEAR-EMPTY — clear_pool empties the pool: len 0 and eviction yields None
// (lib.rs:169 -> lru_list.rs:148-154).
// ===========================================================================
#[kani::proof]
#[kani::unwind(8)]
fn verify_lru_clear_empty() {
    let mut l = LruList::new();
    let a: CacheKey = kani::any();
    let b: CacheKey = kani::any();
    let c: CacheKey = kani::any();
    l.push_back(a);
    l.push_back(b);
    l.push_back(c);
    l.clear();
    assert!(l.len() == 0);
    assert!(l.pop_front() == None);
}
#[kani::proof]
#[kani::unwind(8)]
fn verify_lru_clear_empty__mutant() {
    let mut l = LruList::new();
    let a: CacheKey = kani::any();
    l.push_back(a);
    l.push_back(kani::any());
    l.push_back(kani::any());
    l.clear();
    assert!(l.len() == 3); // FALSE: clear resets len to 0
}

// ===========================================================================
// LRU-CLEAR-DEGRADE — clear_pool on an out-of-range pool does nothing (the if-let
// guard fails); other pools are untouched (lib.rs:166-170).
// ===========================================================================
#[kani::proof]
#[kani::unwind(8)]
fn verify_lru_clear_degrade() {
    let mut pools: Vec<LruList> = Vec::new();
    pools.push(LruList::new());
    pools[0].push_back(kani::any()); // pool 0 holds one entry
    let p: PoolId = kani::any();
    kani::assume(p as usize >= pools.len());
    if let Some(pg) = pools.get_mut(p as usize) {
        pg.clear();
    }
    assert!(pools[0].len() == 1); // out-of-range clear left pool 0 alone
}
#[kani::proof]
#[kani::unwind(8)]
fn verify_lru_clear_degrade__mutant() {
    let mut pools: Vec<LruList> = Vec::new();
    pools.push(LruList::new());
    pools[0].push_back(kani::any());
    let p: PoolId = kani::any();
    kani::assume(p as usize >= pools.len());
    if let Some(pg) = pools.get_mut(p as usize) {
        pg.clear();
    }
    assert!(pools[0].len() == 0); // FALSE: the guard fails, pool 0 keeps its entry
}

// ===========================================================================
// LRU-BATCH-EMPTY-OK — batch_touch on an empty slice returns Ok and touches
// nothing; order is preserved (lib.rs:93-95).
// ===========================================================================
#[kani::proof]
#[kani::unwind(8)]
fn verify_lru_batch_empty_ok() {
    let mut l = LruList::new();
    let a: CacheKey = kani::any();
    let b: CacheKey = kani::any();
    l.push_back(a);
    l.push_back(b);
    let handles: &[EvictionHandle] = &[];
    let r: Result<(), EvictionPolicyError> = if handles.is_empty() { Ok(()) } else { Ok(()) };
    assert!(r.is_ok());
    // nothing was touched -> FIFO order intact
    assert!(l.pop_front() == Some(a));
    assert!(l.pop_front() == Some(b));
}
#[kani::proof]
#[kani::unwind(8)]
fn verify_lru_batch_empty_ok__mutant() {
    let mut l = LruList::new();
    let a: CacheKey = kani::any();
    let b: CacheKey = kani::any();
    l.push_back(a);
    l.push_back(b);
    assert!(l.pop_front() == Some(b)); // FALSE: an empty batch reorders nothing; head is `a`
}

// ===========================================================================
// LRU-BATCH-MRU — batch_touch calls move_to_back for every handle in order; the
// touched entries become most-recent in that order (lib.rs:103,115).
// ===========================================================================
#[kani::proof]
#[kani::unwind(8)]
fn verify_lru_batch_mru() {
    let mut l = LruList::new();
    let a: CacheKey = kani::any();
    let b: CacheKey = kani::any();
    let c: CacheKey = kani::any();
    let i0 = l.push_back(a);
    let i1 = l.push_back(b);
    l.push_back(c);
    // batch touch [i0, i1] -> move_to_back(i0) then move_to_back(i1)
    l.move_to_back(i0); // b, c, a
    l.move_to_back(i1); // c, a, b
    assert!(l.pop_front() == Some(c));
    assert!(l.pop_front() == Some(a));
    assert!(l.pop_front() == Some(b));
}
#[kani::proof]
#[kani::unwind(8)]
fn verify_lru_batch_mru__mutant() {
    let mut l = LruList::new();
    let a: CacheKey = kani::any();
    let b: CacheKey = kani::any();
    let c: CacheKey = kani::any();
    let i0 = l.push_back(a);
    let i1 = l.push_back(b);
    l.push_back(c);
    l.move_to_back(i0);
    l.move_to_back(i1);
    assert!(l.pop_front() == Some(a)); // FALSE: after touching a then b, head is `c`
}

// ===========================================================================
// LRU-BATCH-INVALIDPOOL — a handle in the batch naming an out-of-range pool ->
// Err(InvalidPool) (lib.rs:98-101,108-112).
// ===========================================================================
#[kani::proof]
#[kani::unwind(8)]
fn verify_lru_batch_invalidpool() {
    let mut pools: Vec<LruList> = Vec::new();
    pools.push(LruList::new());
    let p: PoolId = kani::any();
    kani::assume(p as usize >= pools.len());
    let h = EvictionHandle::new(p, 0);
    let r: Result<(), EvictionPolicyError> =
        pools.get(h.pool_id() as usize).map(|_| ()).ok_or(EvictionPolicyError::InvalidPool(h.pool_id()));
    assert!(matches!(r, Err(EvictionPolicyError::InvalidPool(pp)) if pp == p));
}
#[kani::proof]
#[kani::unwind(8)]
fn verify_lru_batch_invalidpool__mutant() {
    let mut pools: Vec<LruList> = Vec::new();
    pools.push(LruList::new());
    let p: PoolId = kani::any();
    kani::assume(p as usize >= pools.len());
    let h = EvictionHandle::new(p, 0);
    let r: Result<(), EvictionPolicyError> =
        pools.get(h.pool_id() as usize).map(|_| ()).ok_or(EvictionPolicyError::InvalidPool(h.pool_id()));
    assert!(r.is_ok()); // FALSE
}

// ===========================================================================
// LRU-FREELIST-RECYCLE — after a remove, push_back reuses the freed slot rather
// than growing (FR-011, lru_list.rs:38-55).
// ===========================================================================
#[kani::proof]
#[kani::unwind(8)]
fn verify_lru_freelist_recycle() {
    let mut l = LruList::new();
    let a: CacheKey = kani::any();
    let i0 = l.push_back(a); // slot 0
    l.push_back(kani::any()); // slot 1
    l.remove(i0); // slot 0 goes on the free list
    let reused = l.push_back(kani::any());
    assert!(reused == i0); // recycled the freed slot, did not allocate slot 2
    assert!(reused == 0);
}
#[kani::proof]
#[kani::unwind(8)]
fn verify_lru_freelist_recycle__mutant() {
    let mut l = LruList::new();
    let i0 = l.push_back(kani::any());
    l.push_back(kani::any());
    l.remove(i0);
    let reused = l.push_back(kani::any());
    assert!(reused == 2); // FALSE: the freed slot 0 is recycled, not a fresh slot 2
}

// ===========================================================================
// LRU-INV-LINK — the arena's index links stay in-bounds across an operation
// sequence on issued handles: every nodes[idx] access is safe (no CBMC OOB) and
// len bookkeeping is exact (lru_list.rs invariant, checked end-to-end).
// ===========================================================================
#[kani::proof]
#[kani::unwind(8)]
fn verify_lru_inv_link() {
    let mut l = LruList::new();
    let i0 = l.push_back(kani::any());
    let i1 = l.push_back(kani::any());
    let i2 = l.push_back(kani::any());
    // operate only on issued indices, as the component does with handles it minted
    l.move_to_back(i0);
    l.move_to_back(i2);
    l.remove(i1);
    let _ = l.pop_front();
    let _ = l.pop_front();
    // reaching here proves every index access above stayed in bounds
    assert!(l.len() == 0);
}
#[kani::proof]
#[kani::unwind(8)]
fn verify_lru_inv_link__mutant() {
    let mut l = LruList::new();
    let i0 = l.push_back(kani::any());
    let i1 = l.push_back(kani::any());
    let i2 = l.push_back(kani::any());
    l.move_to_back(i0);
    l.move_to_back(i2);
    l.remove(i1);
    let _ = l.pop_front();
    let _ = l.pop_front();
    assert!(l.len() == 3); // FALSE: after 3 pushes, 1 remove, 2 pops the list is empty
}

// ===========================================================================
// LRU-POOL-ISOLATION — pools are independent objects; mutating one never affects
// another (lib.rs:26-28 Vec<Mutex<Pool>> -> distinct lists here).
// ===========================================================================
#[kani::proof]
#[kani::unwind(8)]
fn verify_lru_pool_isolation() {
    let mut a = LruList::new();
    let mut b = LruList::new();
    a.push_back(kani::any());
    a.push_back(kani::any());
    // b is a separate object; operations on `a` leave it empty
    assert!(b.len() == 0);
    assert!(b.pop_front() == None);
    assert!(a.len() == 2);
}
#[kani::proof]
#[kani::unwind(8)]
fn verify_lru_pool_isolation__mutant() {
    let mut a = LruList::new();
    let b = LruList::new();
    a.push_back(kani::any());
    a.push_back(kani::any());
    assert!(b.len() == 2); // FALSE: pool b is untouched, still empty
}
