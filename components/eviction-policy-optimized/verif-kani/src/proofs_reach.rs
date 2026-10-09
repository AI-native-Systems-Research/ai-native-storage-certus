// List-level harnesses over REACHABLE states (re-proved 2026-10-07).
//
// WHY A SECOND LIST FILE. The inductive harnesses in `proofs_list.rs` start from
// `inspect::valid_list(3, 5)`: an arbitrary 3-slot representation constrained by
// `kani::assume(invariant(..))`. That invariant is proved inductive only for arenas of
// exactly three slots (the 2026-10-06 assumption audit), so a proof that starts from it
// leans on an assumption that is not established for every reachable list. The harnesses
// here assume NO invariant at all. Their pre-state is built by running the REAL
// `LruList` (src/lru_list.rs, include!d) from `LruList::new()` through a SYMBOLIC history
// of real mutator calls (see `reachable_list` for its exact shape) with symbolic keys and
// symbolic in-range slot numbers. Every pre-state is therefore reachable by construction.
// Any in-range slot number may be passed to remove/move_to_back, because
// `EvictionHandle::new` is public and the component checks only the pool field.
// Out-of-range slots are excluded: those panic, which is refuted separately
// (`refute_epo_inv_stale_handle_never_crashes`). Arenas here hold at most 5 slots, so the
// declared level-2 assumption D-RANGE-FR-002-7fbb2b (fewer than 2^32 - 1 slots) holds
// trivially and is never needed as an assume.
//
// No `kani::assume` at all: an obligation's premise is written as an `if` guard around its
// conclusion (logically the same implication, and it cannot restrict the other builds in
// the same harness the way an assume would). The `__mutant` twins rule out a guard that is
// never true. Every loop is bounded inside the unwind bound with unwinding checks ON, so a pass is a
// full-strength bounded proof (fidelity real-type-bounded) over that reachable family, all
// keys, all handles.

use crate::lru_real::{inspect, LruList};
use interfaces::CacheKey;

/// Arbitrary non-insert steps in the second phase of a history.
const MID_STEPS: usize = 2;
/// Chain-walk budget: the longest chain these harnesses can build (3 + 1 inserts in the
/// history, plus the one inserted by the operation under test). Walks are asserted to have
/// covered the whole chain (chain length == `len()`), so the budget never silently
/// truncates a comparison.
const WALK: usize = 5;

fn insert_any_end(l: &mut LruList, k: CacheKey) -> u32 {
    if kani::any::<bool>() {
        l.push_front(k)
    } else {
        l.push_back(k)
    }
}

/// A list reachable from `LruList::new()` by a history of REAL calls, of this shape:
///   1. exactly `first` inserts (the harnesses call this with 1, 2 and 3), each at a
///      symbolic end with a symbolic key;
///   2. MID_STEPS steps, each a symbolic choice of remove(i), move_to_back(i) (= touch),
///      pop_front() (= evict) or nothing, with `i` ANY in-range slot (live or not);
///   3. optionally one more insert at a symbolic end, which RECYCLES a freed slot whenever
///      phase 2 freed one — so recycled-slot states are covered.
/// Nothing is assumed about the result: every pre-state is reachable by construction.
///
/// WHY THIS SHAPE and not "any N calls in any order", measured on this crate (2026-10-07):
/// a free-form symbolic history of 3 calls, or a symbolic number of first inserts, did not
/// finish inside 600 s and grew past 90 GB in CBMC (the slot-vector length differs per
/// path); with the first-phase count concrete the same family proves in about a minute.
/// No history here makes more than 4 slots, so the slot buffer is allocated once. `clear`
/// is left out: it returns the list to `new()` with only capacity retained, so it reaches no
/// list state that a shorter history does not.
fn reachable_list(first: usize) -> LruList {
    let mut l = LruList::new();
    let mut s = 0usize;
    while s < first {
        insert_any_end(&mut l, kani::any());
        s += 1;
    }
    let n = inspect::nodes_len(&l); // == first: phase 2 never allocates a slot
    let mut t = 0usize;
    while t < MID_STEPS {
        let op: u8 = kani::any();
        let i = inspect::any_idx(n);
        match op {
            0 => l.remove(i),
            1 => l.move_to_back(i),
            2 => {
                let _ = l.pop_front();
            }
            _ => {}
        }
        t += 1;
    }
    if kani::any::<bool>() {
        insert_any_end(&mut l, kani::any());
    }
    l
}

// =====================================================================================
//  EPO-TOUCH-POST-NOT-NEXT-VICTIM
// =====================================================================================

/// The obligation on one reachable list. `touch` toggles the operation under test so the
/// twin can drop it; the base always passes `true`.
fn touch_not_next_victim(first: usize, touch: bool) {
    let mut l = reachable_list(first);
    let n = inspect::nodes_len(&l);
    let i = inspect::any_idx(n); // the touched handle names an existing slot
    // premises (a): "an entry" = a live one; "at least one other entry" = len >= 2
    if inspect::node_active(&l, i) && l.len() >= 2 {
        if touch {
            l.move_to_back(i); // touch (lib.rs:142-156 -> LruList::move_to_back)
        }
        let victim = l.pop_front(); // identify_next_to_evict (lib.rs:202-207 -> pop_front)
        assert!(victim.is_some());
        // the touched entry is identified by SLOT (keys may repeat): it is still live, so
        // the entry just evicted was a different one
        assert!(inspect::node_active(&l, i));
    }
}

/// EPO-TOUCH-POST-NOT-NEXT-VICTIM — after an entry is touched, asking for the next entry to
/// evict does not return that touched entry, as long as the pool holds at least one other
/// entry.
///
/// Real code end to end at list level: `touch` is `LruList::move_to_back` and
/// `identify_next_to_evict` is `LruList::pop_front`. Checked on every reachable list of the
/// family above with 1, 2 and 3 first-phase inserts (so arenas of 1 to 4 slots), every key,
/// every in-range handle. No assumption; no invariant.
#[kani::proof]
#[kani::unwind(6)]
fn verify_epo_touch_post_not_next_victim() {
    touch_not_next_victim(1, true);
    touch_not_next_victim(2, true);
    touch_not_next_victim(3, true);
}

/// Anti-vacuity twin: the same harness WITHOUT the touch. Must FAIL — an untouched live
/// entry is the next victim whenever it is the head — which also shows the premise guard is
/// satisfiable, i.e. the base's conclusion is really checked.
#[kani::proof]
#[kani::unwind(6)]
fn verify_epo_touch_post_not_next_victim__mutant() {
    touch_not_next_victim(1, false); // MUTANT: touch dropped
    touch_not_next_victim(2, false);
    touch_not_next_victim(3, false);
}

// =====================================================================================
//  EPO-TRACK-FRAME-EXISTING-ENTRIES
// =====================================================================================

/// The obligation on one reachable list. `mutant` replaces the frame check by its negation.
fn track_frame(first: usize, mutant: bool) {
    let mut l = reachable_list(first);
    let (b_slots, b_n) = inspect::chain_slots(&l, WALK);
    let mut b_keys = [0u64; inspect::SLOTS];
    let mut k = 0usize;
    while k < b_n {
        b_keys[k] = inspect::node_key(&l, b_slots[k]);
        k += 1;
    }
    let len_before = l.len();
    assert!(b_n == len_before); // the walk saw the whole chain

    // `track` changes the list by exactly one call: push_front OR push_back (lib.rs:129-137).
    // The sketch comparison choosing between them never touches the list, so a SYMBOLIC end
    // over-approximates it soundly and covers "whichever end".
    let key: CacheKey = kani::any();
    let at_front: bool = kani::any();
    let new_idx = if at_front { l.push_front(key) } else { l.push_back(key) };

    let (a_slots, a_n) = inspect::chain_slots_without(&l, new_idx, WALK);
    if mutant {
        // MUTANT: claim the existing entries' chain changed.
        assert!(!inspect::slots_eq(&(a_slots, a_n), &(b_slots, b_n)));
        return;
    }
    assert!(inspect::chain_slots(&l, WALK).1 == l.len()); // whole post-chain seen
    // the new slot was not one of the existing entries
    let mut k = 0usize;
    while k < b_n {
        assert!(b_slots[k] != new_idx);
        k += 1;
    }
    // nothing dropped, nothing reordered, no existing key rewritten
    assert!(a_n == b_n);
    let mut k = 0usize;
    while k < b_n {
        assert!(a_slots[k] == b_slots[k]);
        assert!(inspect::node_key(&l, a_slots[k]) == b_keys[k]);
        k += 1;
    }
    assert!(l.len() == len_before + 1);
    // and the new entry is at the end it went in at
    if at_front {
        assert!(inspect::head(&l) == Some(new_idx));
    } else {
        assert!(inspect::tail(&l) == Some(new_idx));
    }
}

/// EPO-TRACK-FRAME-EXISTING-ENTRIES — a successful track only inserts; it never drops an
/// entry the pool already held, and the entries already there keep their order relative to
/// one another, whichever end the new key was inserted at.
///
/// Asserted for BOTH ends, a symbolic key, and every reachable list of the family above
/// (1, 2 and 3 first-phase inserts; the insert under test may recycle a freed slot or grow
/// the arena): the old chain (slots AND keys, in order) is exactly the new chain with the new
/// slot deleted; the new slot was not on the old chain; the size grew by one; the new entry
/// sits at the end it was inserted at. No assumption at all — "a successful track" is every
/// track into an existing pool, and push_front / push_back cannot fail.
#[kani::proof]
#[kani::unwind(7)]
fn verify_epo_track_frame_existing_entries() {
    track_frame(1, false);
    track_frame(2, false);
    track_frame(3, false);
}

/// Anti-vacuity twin: claim the existing entries' order changed (the frame's central clause,
/// negated). Must FAIL.
#[kani::proof]
#[kani::unwind(7)]
fn verify_epo_track_frame_existing_entries__mutant() {
    track_frame(1, true);
    track_frame(2, true);
    track_frame(3, true);
}

// =====================================================================================
//  EPO-TRACK-POST-NON-IDEMPOTENT-REREGISTRATION
// =====================================================================================

/// Live entries on the chain whose key is `key`, with the chain walk asserted complete.
fn count_key(l: &LruList, key: CacheKey) -> usize {
    let (slots, n) = inspect::chain_slots(l, WALK);
    assert!(n == l.len()); // the walk saw the whole chain
    let mut c = 0usize;
    let mut k = 0usize;
    while k < n {
        if inspect::node_key(l, slots[k]) == key {
            c += 1;
        }
        k += 1;
    }
    c
}

/// The obligation on one reachable list. `mutant` asserts refresh semantics instead.
fn reregister(first: usize, mutant: bool) {
    let mut l = reachable_list(first);
    let h1 = inspect::any_idx(inspect::nodes_len(&l)); // the first handle's slot
    // premise (a): "a key that is already being tracked" = h1 names a live entry
    if inspect::node_active(&l, h1) {
        let key = inspect::node_key(&l, h1);
        let len_before = l.len();
        let n_before = count_key(&l, key);
        assert!(n_before >= 1);

        // re-track the SAME key: track's only list effect is one push at either end
        let at_front: bool = kani::any();
        let h2 = if at_front { l.push_front(key) } else { l.push_back(key) };

        let n_after = count_key(&l, key);
        if mutant {
            assert!(n_after == n_before); // MUTANT: "re-registering refreshes, adds nothing"
            return;
        }
        // a NEW handle: same pool (track builds it as EvictionHandle::new(pool, index),
        // lib.rs:139), so it differs iff the slot differs
        assert!(h2 != h1);
        // the existing entry was not refreshed or replaced: still live, still this key
        assert!(inspect::node_active(&l, h1));
        assert!(inspect::node_key(&l, h1) == key);
        // a second, separate entry for the key, and the size grew by exactly one
        assert!(inspect::node_active(&l, h2));
        assert!(inspect::node_key(&l, h2) == key);
        assert!(n_after == n_before + 1);
        assert!(n_after >= 2);
        assert!(l.len() == len_before + 1);
    }
}

/// EPO-TRACK-POST-NON-IDEMPOTENT-REREGISTRATION — tracking a key already tracked in the same
/// pool does not refresh the existing entry: it creates a second, separate entry and returns
/// a handle different from the first, so the pool then holds two entries for that key and
/// its size grows by one. (Re-proved 2026-10-07; was one concrete trace on a 1-entry pool.)
///
/// `track` (lib.rs:106-140) keeps no key-to-entry lookup: after the sketch bookkeeping its
/// only effect on the pool's entries is ONE call to `LruList::push_front(key)` or
/// `push_back(key)`, and the handle it returns is `EvictionHandle::new(pool, index)` of that
/// call. So the obligation is checked on the real list code, with the end chosen
/// SYMBOLICALLY (a sound over-approximation of the admission comparison, which never touches
/// the list) — on every reachable list of the family in this file, every already-tracked
/// entry as the first handle (any live slot, any key, including keys already present more
/// than once), both ends.
///
/// WHY NOT THROUGH `IEvictionPolicy`. Measured 2026-10-07: building the real component is
/// out of reach — at `--unwind 4 --no-unwinding-checks` the construction path is pruned away
/// entirely (`component(); assert!(false)` VERIFIES, so every real-component harness built on
/// it is vacuous), at `--unwind 8 --no-unwinding-checks` construction alone exceeds 1500 s.
#[kani::proof]
#[kani::unwind(7)]
fn verify_epo_track_post_non_idempotent_reregistration() {
    reregister(1, false);
    reregister(2, false);
    reregister(3, false);
}

/// Anti-vacuity twin: assert refresh semantics (no new entry for the key). Must FAIL — which
/// also shows the "already tracked" guard is satisfiable.
#[kani::proof]
#[kani::unwind(7)]
fn verify_epo_track_post_non_idempotent_reregistration__mutant() {
    reregister(1, true);
    reregister(2, true);
    reregister(3, true);
}
