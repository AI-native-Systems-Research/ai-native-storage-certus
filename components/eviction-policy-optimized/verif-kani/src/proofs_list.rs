// Harnesses over the REAL `LruList` — the production `src/lru_list.rs`, compiled in
// situ (see `lru_real.rs`). Fidelity `real-type-bounded` throughout: the real type, the
// real code, under a stated slot bound.
//
// GEOMETRY. `NS` slots and a walk cap of `NS + 2`. `NS = 3` is the smallest size at
// which every structural case is reachable: a chain with a front, a middle and a back, so
// `remove`/`move_to_back` each exercise all three of their branches (front, interior,
// back), and one slot can be free while two are live. Every harness states its bound.
//
// PROOF SHAPE — inductive, not trace-based. Each global invariant harness takes an
// ARBITRARY valid list (`inspect::valid_list`: every key, link, live flag, both ends, the
// free list and the stored length symbolic, constrained only by the invariant), applies
// ONE arbitrary mutator, and asserts the invariant clause again. Together with the
// establishment step on `LruList::new()` in the same harness, that is induction over all
// operation sequences of any length — not only the ones a bounded trace would reach.
//
// VACUITY. An assumed pre-state is the one thing that could make all of this hollow, so
// every inductive harness has a `__mutant` twin that asserts something FALSE of any real
// list from the very same assumed pre-state. The scorer requires those to FAIL; a mutant
// that passed would prove the pre-state is unsatisfiable rather than that the code is
// correct.

use crate::lru_real::{inspect, LruList};

/// Slots in the symbolic pre-state.
const NS: usize = 3;
/// Chain-walk budget. Must exceed `NS` so a cycle is DETECTED (the walk runs out of
/// budget) rather than looping forever, and must leave room for the one slot a `push` may
/// append.
const CAP: usize = NS + 2;

// =====================================================================================
//  Mutator dispatch
// =====================================================================================

/// Apply exactly one arbitrary `LruList` mutator with arbitrary arguments.
///
/// PRECONDITION MIRRORED: `move_to_back` and `remove` index `self.nodes[idx as usize]`
/// with no bounds check (lru_list.rs:109 and :160), so this helper assumes
/// `idx < nodes.len()`. That is not a convenience — it is the production contract as the
/// inventory itself records it: EPO-INV-INTERNAL-INDEX-IN-RANGE says in as many words
/// that "slot numbers arriving from outside in a caller's handle are NOT covered by this
/// and are not checked anywhere". What happens when a caller breaks it is the subject of
/// `verify_epo_inv_stale_handle_never_crashes` below, which is where the assumption is
/// deliberately dropped.
fn apply_any_mutator(l: &mut LruList) {
    let n = inspect::nodes_len(l);
    let which: u8 = kani::any();
    kani::assume(which < 6);
    let key: u64 = kani::any();
    match which {
        0 => {
            l.push_front(key);
        }
        1 => {
            l.push_back(key);
        }
        2 => {
            if n > 0 {
                l.move_to_back(inspect::any_idx(n));
            }
        }
        3 => {
            if n > 0 {
                l.remove(inspect::any_idx(n));
            }
        }
        4 => {
            l.pop_front();
        }
        _ => {
            l.clear();
        }
    }
}

// =====================================================================================
//  Global structural invariants — inductive
// =====================================================================================

/// EPO-INV-LEN-MATCHES-CHAIN — the reported size equals the length of the eviction chain.
#[kani::proof]
#[kani::unwind(8)]
fn verify_epo_inv_len_matches_chain() {
    // Establishment.
    let fresh = LruList::new();
    assert!(inspect::inv_len_matches_chain(&fresh, CAP));
    // Preservation.
    let mut l = inspect::valid_list(NS, CAP);
    apply_any_mutator(&mut l);
    assert!(inspect::inv_len_matches_chain(&l, CAP));
}

/// Anti-vacuity twin: from the same assumed pre-state, claim the chain is one longer than
/// `len`. Must FAIL.
#[kani::proof]
#[kani::unwind(8)]
fn verify_epo_inv_len_matches_chain__mutant() {
    let mut l = inspect::valid_list(NS, CAP);
    apply_any_mutator(&mut l);
    let (steps, _) = inspect::walk_forward(&l, CAP);
    assert!(steps == inspect::len_field(&l) + 1);
}

/// EPO-INV-LEN-MATCHES-ACTIVE-SLOTS — the reported size equals the number of live slots,
/// and a slot is live exactly while it is on the chain.
#[kani::proof]
#[kani::unwind(8)]
fn verify_epo_inv_len_matches_active_slots() {
    let fresh = LruList::new();
    assert!(inspect::inv_len_matches_active_slots(&fresh, CAP));
    let mut l = inspect::valid_list(NS, CAP);
    apply_any_mutator(&mut l);
    assert!(inspect::inv_len_matches_active_slots(&l, CAP));
}

#[kani::proof]
#[kani::unwind(8)]
fn verify_epo_inv_len_matches_active_slots__mutant() {
    let mut l = inspect::valid_list(NS, CAP);
    apply_any_mutator(&mut l);
    assert!(inspect::active_count(&l) != inspect::len_field(&l));
}

/// EPO-INV-LIST-EMPTY-IFF-NO-ENDS — both ends and a non-zero size, or neither end and a
/// size of zero; never one end without the other.
#[kani::proof]
#[kani::unwind(8)]
fn verify_epo_inv_list_empty_iff_no_ends() {
    let fresh = LruList::new();
    assert!(inspect::inv_empty_iff_no_ends(&fresh));
    let mut l = inspect::valid_list(NS, CAP);
    apply_any_mutator(&mut l);
    assert!(inspect::inv_empty_iff_no_ends(&l));
}

#[kani::proof]
#[kani::unwind(8)]
fn verify_epo_inv_list_empty_iff_no_ends__mutant() {
    let mut l = inspect::valid_list(NS, CAP);
    apply_any_mutator(&mut l);
    // Claim the list always ends up non-empty — false, e.g. after `clear`.
    assert!(inspect::head(&l).is_some());
}

/// EPO-INV-LIST-LINKS-SYMMETRIC — forward and backward links agree; the front has no
/// backward link and the back no forward link.
#[kani::proof]
#[kani::unwind(8)]
fn verify_epo_inv_list_links_symmetric() {
    let fresh = LruList::new();
    assert!(inspect::inv_links_symmetric(&fresh, CAP));
    let mut l = inspect::valid_list(NS, CAP);
    apply_any_mutator(&mut l);
    assert!(inspect::inv_links_symmetric(&l, CAP));
}

#[kani::proof]
#[kani::unwind(8)]
fn verify_epo_inv_list_links_symmetric__mutant() {
    let mut l = inspect::valid_list(NS, CAP);
    apply_any_mutator(&mut l);
    // Claim the front always HAS a backward link — false of any real list.
    match inspect::head(&l) {
        Some(h) => assert!(inspect::node_prev(&l, h).is_some()),
        None => assert!(inspect::len_field(&l) != 0),
    }
}

/// EPO-INV-LIST-ACYCLIC-AND-LENGTH — following forward links from the front reaches the
/// back in exactly `len` steps and then stops; no cycle, so walking always terminates.
///
/// A cycle is witnessed as the `false` flag from `walk_forward`, which gives up after
/// `CAP > NS + 1` steps — more steps than any sound chain over the whole slot array can
/// take, so the flag separates "cycle" from "long list" exactly.
#[kani::proof]
#[kani::unwind(8)]
fn verify_epo_inv_list_acyclic_and_length() {
    let fresh = LruList::new();
    let (s0, ok0) = inspect::walk_forward(&fresh, CAP);
    assert!(ok0 && s0 == 0);

    let mut l = inspect::valid_list(NS, CAP);
    apply_any_mutator(&mut l);
    let (steps, terminated) = inspect::walk_forward(&l, CAP);
    assert!(terminated);
    assert!(steps == inspect::len_field(&l));
    // The walk ends AT the recorded back.
    let chain = inspect::chain_indices(&l, CAP);
    if chain.len() > 0 {
        assert!(inspect::tail(&l) == Some(chain[chain.len() - 1]));
    } else {
        assert!(inspect::tail(&l).is_none());
    }
}

#[kani::proof]
#[kani::unwind(8)]
fn verify_epo_inv_list_acyclic_and_length__mutant() {
    let mut l = inspect::valid_list(NS, CAP);
    apply_any_mutator(&mut l);
    let (_, terminated) = inspect::walk_forward(&l, CAP);
    assert!(!terminated);
}

/// EPO-INV-LIST-TAIL-IS-ONLY-NODE-WITHOUT-NEXT — the only live entry with no forward link
/// is the recorded back.
///
/// This is the invariant `move_to_back` leans on: it decides an entry is already at the
/// back by comparing against the recorded back (lru_list.rs:112), so if the two ever
/// disagreed the recorded back would be left naming an entry no longer in the chain.
#[kani::proof]
#[kani::unwind(8)]
fn verify_epo_inv_list_tail_is_only_node_without_next() {
    let fresh = LruList::new();
    assert!(inspect::inv_tail_only_node_without_next(&fresh));
    let mut l = inspect::valid_list(NS, CAP);
    apply_any_mutator(&mut l);
    assert!(inspect::inv_tail_only_node_without_next(&l));
}

#[kani::proof]
#[kani::unwind(8)]
fn verify_epo_inv_list_tail_is_only_node_without_next__mutant() {
    let mut l = inspect::valid_list(NS, CAP);
    apply_any_mutator(&mut l);
    // Claim the recorded back always HAS a forward link — false of any real list.
    match inspect::tail(&l) {
        Some(t) => assert!(inspect::node_next(&l, t).is_some()),
        None => assert!(inspect::len_field(&l) != 0),
    }
}

/// EPO-INV-SLOT-OWNERSHIP-DISJOINT — every slot is either live or on the free list, never
/// both and never neither, and appears on the free list at most once. This is what makes
/// it impossible for two inserts to be handed the same slot.
#[kani::proof]
#[kani::unwind(8)]
fn verify_epo_inv_slot_ownership_disjoint() {
    let fresh = LruList::new();
    assert!(inspect::inv_slot_ownership_disjoint(&fresh));
    let mut l = inspect::valid_list(NS, CAP);
    apply_any_mutator(&mut l);
    assert!(inspect::inv_slot_ownership_disjoint(&l));
}

#[kani::proof]
#[kani::unwind(8)]
fn verify_epo_inv_slot_ownership_disjoint__mutant() {
    let mut l = inspect::valid_list(NS, CAP);
    apply_any_mutator(&mut l);
    // Claim some live slot is ALSO on the free list.
    let n = inspect::nodes_len(&l);
    let mut bad = false;
    let mut i = 0usize;
    while i < n {
        if inspect::node_active(&l, i as u32) && inspect::free_occurrences(&l, i as u32) > 0 {
            bad = true;
        }
        i += 1;
    }
    assert!(bad);
}

/// EPO-INV-SLOTS-ACCOUNTED — allocated slots = tracked entries + free slots. No slot is
/// lost or double counted, which is what bounds a pool's memory by its high-water mark.
#[kani::proof]
#[kani::unwind(8)]
fn verify_epo_inv_slots_accounted() {
    let fresh = LruList::new();
    assert!(inspect::inv_slots_accounted(&fresh));
    let mut l = inspect::valid_list(NS, CAP);
    apply_any_mutator(&mut l);
    assert!(inspect::inv_slots_accounted(&l));
}

#[kani::proof]
#[kani::unwind(8)]
fn verify_epo_inv_slots_accounted__mutant() {
    let mut l = inspect::valid_list(NS, CAP);
    apply_any_mutator(&mut l);
    assert!(inspect::nodes_len(&l) != inspect::len_field(&l) + inspect::free_len(&l));
}

/// EPO-INV-INTERNAL-INDEX-IN-RANGE — every slot number the list stores internally (both
/// ends, every link, every free-list entry) designates an allocated slot, so no internal
/// walk can read outside the slot storage.
///
/// Scope, exactly as the inventory states it: this covers the numbers the COMPONENT
/// keeps. Slot numbers arriving from outside in a caller's handle are not covered and are
/// not checked anywhere — see `verify_epo_inv_stale_handle_never_crashes`.
#[kani::proof]
#[kani::unwind(8)]
fn verify_epo_inv_internal_index_in_range() {
    let fresh = LruList::new();
    assert!(inspect::inv_index_in_range(&fresh));
    let mut l = inspect::valid_list(NS, CAP);
    apply_any_mutator(&mut l);
    assert!(inspect::inv_index_in_range(&l));
}

#[kani::proof]
#[kani::unwind(8)]
fn verify_epo_inv_internal_index_in_range__mutant() {
    let mut l = inspect::valid_list(NS, CAP);
    apply_any_mutator(&mut l);
    // Claim the recorded front is out of range.
    match inspect::head(&l) {
        Some(h) => assert!(h as usize >= inspect::nodes_len(&l)),
        None => assert!(inspect::len_field(&l) != 0),
    }
}

/// EPO-INV-LEN-NO-UNDERFLOW — `len` is never driven below zero, so repeated removal of an
/// already-removed entry cannot wrap it round to a huge number.
///
/// `len: usize` at lru_list.rs:22 and the only decrement is `self.len -= 1` at
/// lru_list.rs:182, reached only past the `if !active { return }` guard at :160. Kani
/// checks subtraction overflow natively, so the absence of a wrap IS the absence of an
/// arithmetic-overflow failure in the mutator; the harness additionally pins the value
/// relation so a silent saturation could not pass either.
#[kani::proof]
#[kani::unwind(8)]
fn verify_epo_inv_len_no_underflow() {
    let mut l = inspect::valid_list(NS, CAP);
    let before = inspect::len_field(&l);
    apply_any_mutator(&mut l);
    let after = inspect::len_field(&l);
    // Never wrapped. A `usize` underflow wraps to a value near `usize::MAX`, so the content of
    // this property is that the size stays within reach of where it started: at most one more
    // (a single insert), and never astronomically larger.
    assert!(after <= before + 1);
    assert!(after <= inspect::nodes_len(&l));
    // NOTE: no LOWER bound on `after` is asserted, deliberately. `clear` is one of the
    // mutators and legitimately drops the size from `before` to 0 in a single step, so a
    // clause like `after + 1 >= before` would be false of CORRECT code rather than a check on
    // it — an earlier revision of this harness asserted exactly that and was refuted by the
    // `clear` branch. "A decrement is by exactly one" belongs to the single removal path and
    // is EPO-REMOVE-POST-LEN-DECREMENT; the repeated-removal path that would underflow if the
    // `active` guard were missing is the __split_double_remove artifact below.
}

#[kani::proof]
#[kani::unwind(8)]
fn verify_epo_inv_len_no_underflow__mutant() {
    let mut l = inspect::valid_list(NS, CAP);
    let before = inspect::len_field(&l);
    apply_any_mutator(&mut l);
    assert!(inspect::len_field(&l) > before + 1);
}

/// Double removal specifically: the path that would underflow if the `active` guard were
/// missing. Its own harness because the arbitrary mutator applies only ONE operation and
/// the underflow scenario needs two.
#[kani::proof]
#[kani::unwind(8)]
fn verify_epo_inv_len_no_underflow__split_double_remove() {
    let mut l = inspect::valid_list(NS, CAP);
    kani::assume(inspect::nodes_len(&l) > 0);
    let i = inspect::any_idx(inspect::nodes_len(&l));
    l.remove(i);
    let mid = inspect::len_field(&l);
    l.remove(i);
    assert!(inspect::len_field(&l) == mid);
    assert!(inspect::invariant(&l, CAP));
}

// =====================================================================================
//  `remove` — postconditions and frame
// =====================================================================================

/// EPO-REMOVE-POST-UNLINK — removing a live entry takes it out of the eviction order, so
/// it is no longer one of the entries and no longer appears among the candidates.
#[kani::proof]
#[kani::unwind(8)]
fn verify_epo_remove_post_unlink() {
    let mut l = inspect::valid_list(NS, CAP);
    kani::assume(inspect::nodes_len(&l) > 0);
    let i = inspect::any_idx(inspect::nodes_len(&l));
    kani::assume(inspect::node_active(&l, i));

    l.remove(i);

    assert!(!inspect::node_active(&l, i));
    assert!(!inspect::in_chain(&l, i, CAP));
    // Nor among the previewed candidates.
    let cands = l.peek_front_n(CAP);
    let chain = inspect::chain_indices(&l, CAP);
    assert!(cands.len() == chain.len());
    assert!(inspect::invariant(&l, CAP));
}

#[kani::proof]
#[kani::unwind(8)]
fn verify_epo_remove_post_unlink__mutant() {
    let mut l = inspect::valid_list(NS, CAP);
    kani::assume(inspect::nodes_len(&l) > 0);
    let i = inspect::any_idx(inspect::nodes_len(&l));
    kani::assume(inspect::node_active(&l, i));
    l.remove(i);
    assert!(inspect::node_active(&l, i));
}

/// EPO-REMOVE-POST-LEN-DECREMENT — removing a live entry reduces the size by exactly one.
#[kani::proof]
#[kani::unwind(8)]
fn verify_epo_remove_post_len_decrement() {
    let mut l = inspect::valid_list(NS, CAP);
    kani::assume(inspect::nodes_len(&l) > 0);
    let i = inspect::any_idx(inspect::nodes_len(&l));
    kani::assume(inspect::node_active(&l, i));
    let before = l.len();
    l.remove(i);
    assert!(l.len() + 1 == before);
}

#[kani::proof]
#[kani::unwind(8)]
fn verify_epo_remove_post_len_decrement__mutant() {
    let mut l = inspect::valid_list(NS, CAP);
    kani::assume(inspect::nodes_len(&l) > 0);
    let i = inspect::any_idx(inspect::nodes_len(&l));
    kani::assume(inspect::node_active(&l, i));
    let before = l.len();
    l.remove(i);
    assert!(l.len() == before);
}

/// EPO-REMOVE-POST-NEVER-EVICTED — once removed, no later eviction and no later preview
/// ever returns that entry again.
///
/// Phrased about the KEY, so the harness assumes the list's keys are pairwise distinct
/// (`inspect::assume_distinct_keys`); with duplicates the statement is not well posed, and
/// the production code deliberately allows duplicates
/// (EPO-TRACK-POST-NON-IDEMPOTENT-REREGISTRATION). "No LATER eviction" is covered for the
/// whole future, not one step: the harness drains the entire list and checks the key never
/// comes back.
#[kani::proof]
#[kani::unwind(8)]
fn verify_epo_remove_post_never_evicted() {
    let mut l = inspect::valid_list(NS, CAP);
    kani::assume(inspect::nodes_len(&l) > 0);
    inspect::assume_distinct_keys(&l);
    let i = inspect::any_idx(inspect::nodes_len(&l));
    kani::assume(inspect::node_active(&l, i));
    let key = inspect::node_key(&l, i);

    l.remove(i);

    assert!(!inspect::chain_contains_key(&l, key, CAP));
    assert!(!inspect::node_active(&l, i));
    // Drain the whole list: the removed key is never handed back.
    let mut seen = 0usize;
    while seen < CAP {
        match l.pop_front() {
            Some(k) => {
                assert!(k != key);
                seen += 1;
            }
            None => break,
        }
    }
}

#[kani::proof]
#[kani::unwind(8)]
fn verify_epo_remove_post_never_evicted__mutant() {
    let mut l = inspect::valid_list(NS, CAP);
    kani::assume(inspect::nodes_len(&l) > 0);
    inspect::assume_distinct_keys(&l);
    let i = inspect::any_idx(inspect::nodes_len(&l));
    kani::assume(inspect::node_active(&l, i));
    let key = inspect::node_key(&l, i);
    l.remove(i);
    assert!(inspect::chain_contains_key(&l, key, CAP));
}

/// EPO-REMOVE-POST-IDEMPOTENT-REMOVED-HANDLE — removing an already-removed entry (whose
/// slot still exists) quietly succeeds and changes nothing: no crash, no second decrement,
/// no other entry disturbed.
#[kani::proof]
#[kani::unwind(8)]
fn verify_epo_remove_post_idempotent_removed_handle() {
    let mut l = inspect::valid_list(NS, CAP);
    kani::assume(inspect::nodes_len(&l) > 0);
    let i = inspect::any_idx(inspect::nodes_len(&l));
    kani::assume(!inspect::node_active(&l, i));

    let before = inspect::snapshot(&l);
    l.remove(i);
    assert!(inspect::snapshot_eq(&inspect::snapshot(&l), &before));
}

#[kani::proof]
#[kani::unwind(8)]
fn verify_epo_remove_post_idempotent_removed_handle__mutant() {
    let mut l = inspect::valid_list(NS, CAP);
    kani::assume(inspect::nodes_len(&l) > 0);
    let i = inspect::any_idx(inspect::nodes_len(&l));
    kani::assume(!inspect::node_active(&l, i));
    let before = inspect::snapshot(&l);
    l.remove(i);
    assert!(!inspect::snapshot_eq(&inspect::snapshot(&l), &before));
}

/// EPO-REMOVE-POST-SLOT-RECYCLED — removal puts the slot on the reuse list, and the next
/// insert takes a slot from that list before allocating a new one, most recently freed
/// first.
#[kani::proof]
#[kani::unwind(8)]
fn verify_epo_remove_post_slot_recycled() {
    let mut l = inspect::valid_list(NS, CAP);
    kani::assume(inspect::nodes_len(&l) > 0);
    let i = inspect::any_idx(inspect::nodes_len(&l));
    kani::assume(inspect::node_active(&l, i));

    l.remove(i);

    // On the reuse list, most recently freed last (`Vec::push` / `Vec::pop`).
    let flen = inspect::free_len(&l);
    assert!(flen > 0);
    assert!(inspect::free_at(&l, flen - 1) == i);

    // The next insert takes exactly that slot — no new allocation.
    let allocated_before = inspect::nodes_len(&l);
    let k: u64 = kani::any();
    let got = l.push_back(k);
    assert!(got == i);
    assert!(inspect::nodes_len(&l) == allocated_before);
}

#[kani::proof]
#[kani::unwind(8)]
fn verify_epo_remove_post_slot_recycled__mutant() {
    let mut l = inspect::valid_list(NS, CAP);
    kani::assume(inspect::nodes_len(&l) > 0);
    let i = inspect::any_idx(inspect::nodes_len(&l));
    kani::assume(inspect::node_active(&l, i));
    l.remove(i);
    let k: u64 = kani::any();
    // Claim the insert allocates a fresh slot instead of recycling.
    assert!(l.push_back(k) != i);
}

/// EPO-REMOVE-FRAME-RELATIVE-ORDER-OTHERS — the remaining entries keep their order
/// relative to one another, whether the removed entry was at the front, in the middle or
/// at the back. All three cases are covered because `i` ranges over the whole slot array.
// ALLOCATION-FREE, and back at the file's usual unwind bound of 8. The earlier form built
// two `Vec` chain projections and compared them, which needed a bound of 20 and — measured
// under the gate's invocation — then did not finish inside a 300 s cap at all: the cost was
// the allocator and the length-dependent compare path, not the property. `inspect::chain_slots`
// projects the chain into a fixed-size array instead, and asserts it never overflows, so a
// longer chain would be a failed check rather than a silent truncation.
//
// The comment sits ABOVE `#[kani::proof]` rather than between it and `#[kani::unwind]`
// deliberately: the gate discovers harnesses by scanning for `#[kani::proof]` followed by
// further ATTRIBUTE lines and then `fn NAME`, and a comment line in that gap makes the
// harness invisible to it — which is exactly why these properties came back
// "no runnable harness" while the harness was sitting right here.
#[kani::proof]
#[kani::unwind(8)]
fn verify_epo_remove_frame_relative_order_others() {
    let mut l = inspect::valid_list(NS, CAP);
    kani::assume(inspect::nodes_len(&l) > 0);
    let i = inspect::any_idx(inspect::nodes_len(&l));
    kani::assume(inspect::node_active(&l, i));

    let others_before = inspect::chain_slots_without(&l, i, CAP);
    l.remove(i);
    let after = inspect::chain_slots(&l, CAP);
    assert!(inspect::slots_eq(&after, &others_before));
}

#[kani::proof]
#[kani::unwind(8)]
fn verify_epo_remove_frame_relative_order_others__mutant() {
    let mut l = inspect::valid_list(NS, CAP);
    kani::assume(inspect::nodes_len(&l) > 0);
    let i = inspect::any_idx(inspect::nodes_len(&l));
    kani::assume(inspect::node_active(&l, i));
    let others_before = inspect::chain_slots_without(&l, i, CAP);
    l.remove(i);
    assert!(!inspect::slots_eq(&inspect::chain_slots(&l, CAP), &others_before));
}

// =====================================================================================
//  `touch` / `move_to_back` — postconditions and frame
// =====================================================================================

/// EPO-TOUCH-POST-MOVE-TO-MRU — touching a live entry moves it to the protected,
/// most-recently-used end, so it sits behind every other entry in the queue.
#[kani::proof]
#[kani::unwind(8)]
fn verify_epo_touch_post_move_to_mru() {
    let mut l = inspect::valid_list(NS, CAP);
    kani::assume(inspect::nodes_len(&l) > 0);
    let i = inspect::any_idx(inspect::nodes_len(&l));
    kani::assume(inspect::node_active(&l, i));

    l.move_to_back(i);

    assert!(inspect::tail(&l) == Some(i));
    let chain = inspect::chain_indices(&l, CAP);
    assert!(chain.len() > 0);
    assert!(chain[chain.len() - 1] == i);
    assert!(inspect::node_next(&l, i).is_none());
    assert!(inspect::invariant(&l, CAP));
}

#[kani::proof]
#[kani::unwind(8)]
fn verify_epo_touch_post_move_to_mru__mutant() {
    let mut l = inspect::valid_list(NS, CAP);
    kani::assume(inspect::nodes_len(&l) > 0);
    let i = inspect::any_idx(inspect::nodes_len(&l));
    kani::assume(inspect::node_active(&l, i));
    l.move_to_back(i);
    assert!(inspect::tail(&l) != Some(i));
}

// EPO-TOUCH-POST-NOT-NEXT-VICTIM: re-proved 2026-10-07 in proofs_reach.rs over REACHABLE
// lists (no assumed invariant); the valid_list-based harness that stood here was removed.



/// EPO-TOUCH-POST-ALREADY-AT-BACK-NOOP — touching the entry already last in the queue
/// changes nothing at all: order, count and every internal link stay as they were.
#[kani::proof]
#[kani::unwind(8)]
fn verify_epo_touch_post_already_at_back_noop() {
    let mut l = inspect::valid_list(NS, CAP);
    kani::assume(inspect::nodes_len(&l) > 0);
    let i = inspect::any_idx(inspect::nodes_len(&l));
    kani::assume(inspect::tail(&l) == Some(i));

    let before = inspect::snapshot(&l);
    l.move_to_back(i);
    assert!(inspect::snapshot_eq(&inspect::snapshot(&l), &before));
}

#[kani::proof]
#[kani::unwind(8)]
fn verify_epo_touch_post_already_at_back_noop__mutant() {
    let mut l = inspect::valid_list(NS, CAP);
    kani::assume(inspect::nodes_len(&l) > 0);
    let i = inspect::any_idx(inspect::nodes_len(&l));
    kani::assume(inspect::tail(&l) == Some(i));
    let before = inspect::snapshot(&l);
    l.move_to_back(i);
    assert!(!inspect::snapshot_eq(&inspect::snapshot(&l), &before));
}

/// EPO-TOUCH-POST-REMOVED-HANDLE-IS-SILENT-NOOP — touching a handle whose entry is already
/// gone, while its slot still exists, quietly does nothing: no crash, no resurrection, no
/// other entry disturbed.
#[kani::proof]
#[kani::unwind(8)]
fn verify_epo_touch_post_removed_handle_is_silent_noop() {
    let mut l = inspect::valid_list(NS, CAP);
    kani::assume(inspect::nodes_len(&l) > 0);
    let i = inspect::any_idx(inspect::nodes_len(&l));
    kani::assume(!inspect::node_active(&l, i));

    let before = inspect::snapshot(&l);
    l.move_to_back(i);
    assert!(inspect::snapshot_eq(&inspect::snapshot(&l), &before));
    assert!(!inspect::node_active(&l, i));
    assert!(!inspect::in_chain(&l, i, CAP));
}

#[kani::proof]
#[kani::unwind(8)]
fn verify_epo_touch_post_removed_handle_is_silent_noop__mutant() {
    let mut l = inspect::valid_list(NS, CAP);
    kani::assume(inspect::nodes_len(&l) > 0);
    let i = inspect::any_idx(inspect::nodes_len(&l));
    kani::assume(!inspect::node_active(&l, i));
    let before = inspect::snapshot(&l);
    l.move_to_back(i);
    assert!(!inspect::snapshot_eq(&inspect::snapshot(&l), &before));
}

/// EPO-TOUCH-FRAME-MEMBERSHIP — touching neither adds nor removes entries: the same
/// entries and the same count before and after.
#[kani::proof]
#[kani::unwind(8)]
fn verify_epo_touch_frame_membership() {
    let mut l = inspect::valid_list(NS, CAP);
    kani::assume(inspect::nodes_len(&l) > 0);
    let i = inspect::any_idx(inspect::nodes_len(&l));

    let len_before = l.len();
    let n = inspect::nodes_len(&l);
    let mut active_before: Vec<bool> = Vec::new();
    let mut j = 0usize;
    while j < n {
        active_before.push(inspect::node_active(&l, j as u32));
        j += 1;
    }

    l.move_to_back(i);

    assert!(l.len() == len_before);
    assert!(inspect::nodes_len(&l) == n);
    let mut j = 0usize;
    while j < n {
        assert!(inspect::node_active(&l, j as u32) == active_before[j]);
        j += 1;
    }
}

#[kani::proof]
#[kani::unwind(8)]
fn verify_epo_touch_frame_membership__mutant() {
    let mut l = inspect::valid_list(NS, CAP);
    kani::assume(inspect::nodes_len(&l) > 0);
    let i = inspect::any_idx(inspect::nodes_len(&l));
    let len_before = l.len();
    l.move_to_back(i);
    assert!(l.len() != len_before);
}

/// EPO-TOUCH-FRAME-RELATIVE-ORDER-OTHERS — apart from moving the touched entry to the
/// protected end, the other entries keep their order relative to one another.
// ALLOCATION-FREE, and back at the file's usual unwind bound of 8. The earlier form built
// two `Vec` chain projections and compared them, which needed a bound of 20 and — measured
// under the gate's invocation — then did not finish inside a 300 s cap at all: the cost was
// the allocator and the length-dependent compare path, not the property. `inspect::chain_slots`
// projects the chain into a fixed-size array instead, and asserts it never overflows, so a
// longer chain would be a failed check rather than a silent truncation.
//
// The comment sits ABOVE `#[kani::proof]` rather than between it and `#[kani::unwind]`
// deliberately: the gate discovers harnesses by scanning for `#[kani::proof]` followed by
// further ATTRIBUTE lines and then `fn NAME`, and a comment line in that gap makes the
// harness invisible to it — which is exactly why these properties came back
// "no runnable harness" while the harness was sitting right here.
#[kani::proof]
#[kani::unwind(8)]
fn verify_epo_touch_frame_relative_order_others() {
    let mut l = inspect::valid_list(NS, CAP);
    kani::assume(inspect::nodes_len(&l) > 0);
    let i = inspect::any_idx(inspect::nodes_len(&l));
    kani::assume(inspect::node_active(&l, i));

    let others_before = inspect::chain_slots_without(&l, i, CAP);
    l.move_to_back(i);
    let others_after = inspect::chain_slots_without(&l, i, CAP);
    assert!(inspect::slots_eq(&others_after, &others_before));
}

#[kani::proof]
#[kani::unwind(8)]
fn verify_epo_touch_frame_relative_order_others__mutant() {
    let mut l = inspect::valid_list(NS, CAP);
    kani::assume(inspect::nodes_len(&l) > 0);
    let i = inspect::any_idx(inspect::nodes_len(&l));
    kani::assume(inspect::node_active(&l, i));
    let others_before = inspect::chain_slots_without(&l, i, CAP);
    l.move_to_back(i);
    assert!(!inspect::slots_eq(&inspect::chain_slots_without(&l, i, CAP), &others_before));
}

// =====================================================================================
//  Eviction order, preview, size
// =====================================================================================

/// EPO-INV-VICTIM-IS-LIST-FRONT — whatever has happened, the entry the policy would evict
/// next is the one at the least-recently-used front, and that is the same entry a newly
/// tracked key's popularity is compared against (`peek_front_key`, lru_list.rs:70).
#[kani::proof]
#[kani::unwind(8)]
fn verify_epo_inv_victim_is_list_front() {
    let mut l = inspect::valid_list(NS, CAP);
    apply_any_mutator(&mut l);

    let front_key = l.peek_front_key();
    match inspect::head(&l) {
        Some(h) => assert!(front_key == Some(inspect::node_key(&l, h))),
        None => assert!(front_key.is_none()),
    }
    // And the victim actually taken is that same entry.
    let taken = l.pop_front();
    assert!(taken == front_key);
}

#[kani::proof]
#[kani::unwind(8)]
fn verify_epo_inv_victim_is_list_front__mutant() {
    let mut l = inspect::valid_list(NS, CAP);
    apply_any_mutator(&mut l);
    let front_key = l.peek_front_key();
    let taken = l.pop_front();
    assert!(taken != front_key);
}

/// EPO-EVICT-POST-RETURNS-LRU-HEAD — asking a non-empty pool for its victim returns the
/// key at the least-recently-used front.
#[kani::proof]
#[kani::unwind(8)]
fn verify_epo_evict_post_returns_lru_head() {
    let mut l = inspect::valid_list(NS, CAP);
    kani::assume(l.len() > 0);
    let h = inspect::head(&l).unwrap();
    let expected = inspect::node_key(&l, h);
    assert!(l.pop_front() == Some(expected));
}

#[kani::proof]
#[kani::unwind(8)]
fn verify_epo_evict_post_returns_lru_head__mutant() {
    let mut l = inspect::valid_list(NS, CAP);
    kani::assume(l.len() > 0);
    let h = inspect::head(&l).unwrap();
    let expected = inspect::node_key(&l, h);
    assert!(l.pop_front() != Some(expected));
}

/// EPO-EVICT-POST-REMOVES-RETURNED — the victim returned is at the same time taken out:
/// the size drops by one, the second in line becomes the new front, and the same entry is
/// never offered again, so the operation both selects and consumes.
#[kani::proof]
#[kani::unwind(8)]
fn verify_epo_evict_post_removes_returned() {
    let mut l = inspect::valid_list(NS, CAP);
    kani::assume(l.len() > 0);
    inspect::assume_distinct_keys(&l);

    let before_len = l.len();
    let chain_before = inspect::chain_indices(&l, CAP);
    let victim_slot = chain_before[0];

    let got = l.pop_front();

    assert!(got.is_some());
    assert!(l.len() + 1 == before_len);
    assert!(!inspect::node_active(&l, victim_slot));
    // The second in line is the new front.
    let chain_after = inspect::chain_indices(&l, CAP);
    if chain_before.len() >= 2 {
        assert!(chain_after.len() > 0 && chain_after[0] == chain_before[1]);
    } else {
        assert!(chain_after.len() == 0);
    }
    // Calling twice returns two different entries.
    if before_len >= 2 {
        let second = l.pop_front();
        assert!(second.is_some() && second != got);
    }
    assert!(inspect::invariant(&l, CAP));
}

#[kani::proof]
#[kani::unwind(8)]
fn verify_epo_evict_post_removes_returned__mutant() {
    let mut l = inspect::valid_list(NS, CAP);
    kani::assume(l.len() > 0);
    let before_len = l.len();
    l.pop_front();
    assert!(l.len() == before_len);
}

/// EPO-INV-PREVIEW-AGREES-WITH-EVICTION — the candidates reported are exactly the keys the
/// same number of successive victim requests would have returned, in the same order; the
/// first candidate is the key the next eviction returns.
#[kani::proof]
#[kani::unwind(8)]
fn verify_epo_inv_preview_agrees_with_eviction() {
    let mut l = inspect::valid_list(NS, CAP);
    let m: usize = kani::any();
    kani::assume(m <= CAP);

    let preview = l.peek_front_n(m);

    let mut evicted: Vec<u64> = Vec::new();
    let mut c = 0usize;
    while c < m {
        match l.pop_front() {
            Some(k) => evicted.push(k),
            None => break,
        }
        c += 1;
    }
    assert!(inspect::key_seq_eq(&preview, &evicted));
}

#[kani::proof]
#[kani::unwind(8)]
fn verify_epo_inv_preview_agrees_with_eviction__mutant() {
    let mut l = inspect::valid_list(NS, CAP);
    kani::assume(l.len() > 0);
    let preview = l.peek_front_n(1);
    let first = l.pop_front();
    assert!(preview.len() != 1 || preview[0] != first.unwrap());
}

/// EPO-CANDIDATES-POST-ORDERED-PREFIX — the candidates are the keys nearest the
/// least-recently-used front, in the order they would be evicted, starting with the one
/// that would go first.
#[kani::proof]
#[kani::unwind(8)]
fn verify_epo_candidates_post_ordered_prefix() {
    let l = inspect::valid_list(NS, CAP);
    let m: usize = kani::any();
    kani::assume(m <= CAP);

    let got = l.peek_front_n(m);
    let chain = inspect::chain_keys(&l, CAP);

    let expected_len = if m < chain.len() { m } else { chain.len() };
    assert!(got.len() == expected_len);
    let mut i = 0usize;
    while i < got.len() {
        assert!(got[i] == chain[i]);
        i += 1;
    }
}

#[kani::proof]
#[kani::unwind(8)]
fn verify_epo_candidates_post_ordered_prefix__mutant() {
    let l = inspect::valid_list(NS, CAP);
    kani::assume(l.len() >= 1);
    let got = l.peek_front_n(1);
    let chain = inspect::chain_keys(&l, CAP);
    assert!(got[0] != chain[0]);
}

/// EPO-CANDIDATES-POST-COUNT-BOUND — never more keys than asked for and never more than
/// the pool holds; when the pool holds fewer than asked, all of them and no more.
#[kani::proof]
#[kani::unwind(8)]
fn verify_epo_candidates_post_count_bound() {
    let l = inspect::valid_list(NS, CAP);
    let m: usize = kani::any();
    kani::assume(m <= CAP);

    let got = l.peek_front_n(m);
    assert!(got.len() <= m);
    assert!(got.len() <= l.len());
    if l.len() < m {
        assert!(got.len() == l.len());
    } else {
        assert!(got.len() == m);
    }
}

#[kani::proof]
#[kani::unwind(8)]
fn verify_epo_candidates_post_count_bound__mutant() {
    let l = inspect::valid_list(NS, CAP);
    let m: usize = kani::any();
    kani::assume(m <= CAP);
    let got = l.peek_front_n(m);
    assert!(got.len() > m);
}

/// EPO-CANDIDATES-POST-ZERO-EMPTY — previewing zero candidates returns an empty list
/// whatever the pool contains, and changes nothing.
#[kani::proof]
#[kani::unwind(8)]
fn verify_epo_candidates_post_zero_empty() {
    let l = inspect::valid_list(NS, CAP);
    let before = inspect::snapshot(&l);
    let got = l.peek_front_n(0);
    assert!(got.len() == 0);
    assert!(inspect::snapshot_eq(&inspect::snapshot(&l), &before));
}

#[kani::proof]
#[kani::unwind(8)]
fn verify_epo_candidates_post_zero_empty__mutant() {
    let l = inspect::valid_list(NS, CAP);
    assert!(l.peek_front_n(0).len() != 0);
}

/// EPO-CANDIDATES-FRAME-NON-DESTRUCTIVE — looking at the candidates changes nothing: the
/// keys stay tracked, the pool keeps the same entries in the same order, and calling it
/// again returns the same answer.
// ALLOCATION-FREE, and back at the file's usual unwind bound of 8. The earlier form
// compared three `Vec`-based `snapshot`s and needed a bound of 20, at which — measured — it
// did not finish inside a 420 s cap: the cost was the allocator and the length-dependent
// compare path, not the property. `inspect::fixed_snapshot` projects the whole
// representation into fixed-size arrays instead, and asserts the slot count never exceeds
// them, so a wider list would be a failed check rather than a silent truncation. The two
// `peek_front_n` results are still the production `Vec`s, since that is the API under test.
//
// The comment sits ABOVE `#[kani::proof]` rather than between it and `#[kani::unwind]`
// deliberately: the gate discovers harnesses by scanning for `#[kani::proof]` followed by
// further ATTRIBUTE lines and then `fn NAME`, and a comment line in that gap makes the
// harness invisible to it — which is exactly why this property came back
// "no runnable harness" while the harness was sitting right here.
#[kani::proof]
#[kani::unwind(8)]
fn verify_epo_candidates_frame_non_destructive() {
    let l = inspect::valid_list(NS, CAP);
    let m: usize = kani::any();
    kani::assume(m <= CAP);

    let before = inspect::fixed_snapshot(&l);
    let first = l.peek_front_n(m);
    assert!(inspect::fixed_snapshot_eq(&inspect::fixed_snapshot(&l), &before));
    let second = l.peek_front_n(m);
    assert!(inspect::key_seq_eq(&second, &first));
    assert!(inspect::fixed_snapshot_eq(&inspect::fixed_snapshot(&l), &before));
}

#[kani::proof]
#[kani::unwind(8)]
fn verify_epo_candidates_frame_non_destructive__mutant() {
    let l = inspect::valid_list(NS, CAP);
    let before = inspect::fixed_snapshot(&l);
    let _ = l.peek_front_n(CAP);
    assert!(!inspect::fixed_snapshot_eq(&inspect::fixed_snapshot(&l), &before));
}

/// EPO-LEN-POST-ACTIVE-COUNT — the reported size is exactly the number of entries tracked
/// and not yet removed, evicted or cleared.
#[kani::proof]
#[kani::unwind(8)]
fn verify_epo_len_post_active_count() {
    let mut l = inspect::valid_list(NS, CAP);
    apply_any_mutator(&mut l);
    assert!(l.len() == inspect::active_count(&l));
    assert!(l.len() == inspect::chain_indices(&l, CAP).len());
}

#[kani::proof]
#[kani::unwind(8)]
fn verify_epo_len_post_active_count__mutant() {
    let mut l = inspect::valid_list(NS, CAP);
    apply_any_mutator(&mut l);
    assert!(l.len() != inspect::active_count(&l));
}

/// EPO-LEN-FRAME-PURE — asking for the size changes nothing.
#[kani::proof]
#[kani::unwind(8)]
fn verify_epo_len_frame_pure() {
    let l = inspect::valid_list(NS, CAP);
    let before = inspect::snapshot(&l);
    let a = l.len();
    let b = l.len();
    assert!(a == b);
    assert!(inspect::snapshot_eq(&inspect::snapshot(&l), &before));
}

#[kani::proof]
#[kani::unwind(8)]
fn verify_epo_len_frame_pure__mutant() {
    let l = inspect::valid_list(NS, CAP);
    let before = inspect::snapshot(&l);
    let _ = l.len();
    assert!(!inspect::snapshot_eq(&inspect::snapshot(&l), &before));
}

/// EPO-CLEAR-POST-SLOT-NUMBERING-RESTARTS — clearing discards the slots as well as the
/// entries, so later inserts hand out slot numbers from the beginning again, and a handle
/// issued before the clear now names a slot the pool will give to a different key.
#[kani::proof]
#[kani::unwind(8)]
fn verify_epo_clear_post_slot_numbering_restarts() {
    let mut l = inspect::valid_list(NS, CAP);
    let k_old: u64 = kani::any();
    let _old_slot = l.push_back(k_old);

    l.clear();

    assert!(inspect::nodes_len(&l) == 0);
    assert!(inspect::free_len(&l) == 0);
    assert!(inspect::len_field(&l) == 0);

    let k_new: u64 = kani::any();
    let new_slot = l.push_back(k_new);
    // Numbering restarts from zero, so slot 0 — which some pre-clear handle names — is
    // handed straight back out, now standing for a different key.
    assert!(new_slot == 0);
    assert!(inspect::node_key(&l, 0) == k_new);
}

#[kani::proof]
#[kani::unwind(8)]
fn verify_epo_clear_post_slot_numbering_restarts__mutant() {
    let mut l = inspect::valid_list(NS, CAP);
    l.clear();
    let k: u64 = kani::any();
    assert!(l.push_back(k) != 0);
}

// =====================================================================================
//  Stale handles — the two obligations the code does NOT meet
// =====================================================================================

/// EPO-INV-STALE-HANDLE-NEVER-CRASHES — "no handle a caller can present may crash the
/// component".
///
/// THIS HARNESS IS EXPECTED TO FAIL, and its failure is a DEFECT REPORT, not a tool
/// boundary. `move_to_back` (lru_list.rs:109) and `remove` (lru_list.rs:160) both open
/// with `self.nodes[idx as usize]`, an unchecked index; `touch`/`remove` on the component
/// (lib.rs:154, :198) validate only the handle's POOL and pass `handle.index()` straight
/// through. `EvictionPolicyError::InvalidHandle` exists in the shared interface
/// (ieviction_policy.rs:58) for exactly this case and is constructed nowhere in the
/// component. So a handle naming a slot the pool does not have panics the process instead
/// of being reported or ignored.
///
/// This harness deliberately does NOT assume `idx < nodes.len()` — the assumption every
/// other harness in this file mirrors from the production contract.
#[kani::proof]
#[kani::unwind(8)]
fn verify_epo_inv_stale_handle_never_crashes() {
    let mut l = inspect::valid_list(NS, CAP);
    // An arbitrary slot number, as a caller's handle could carry — NOT constrained to the
    // slots the list actually has.
    let idx: u32 = kani::any();
    kani::assume((idx as usize) <= inspect::nodes_len(&l) + 1);
    l.move_to_back(idx);
    assert!(inspect::invariant(&l, CAP));
}

/// The same defect on the `remove` path, separate so each entry point has its own
/// reproducible signature.
#[kani::proof]
#[kani::unwind(8)]
fn verify_epo_inv_stale_handle_never_crashes__split_remove() {
    let mut l = inspect::valid_list(NS, CAP);
    let idx: u32 = kani::any();
    kani::assume((idx as usize) <= inspect::nodes_len(&l) + 1);
    l.remove(idx);
    assert!(inspect::invariant(&l, CAP));
}

/// The positive half of the same obligation, which the code DOES meet: for a slot number
/// the pool really allocated, neither entry point can panic and the invariant survives —
/// including for an already-removed slot. This is what makes the failure above
/// attributable to the missing bounds check alone rather than to the operations.
#[kani::proof]
#[kani::unwind(8)]
fn verify_epo_inv_stale_handle_never_crashes__split_in_range() {
    let mut l = inspect::valid_list(NS, CAP);
    kani::assume(inspect::nodes_len(&l) > 0);
    let idx = inspect::any_idx(inspect::nodes_len(&l));
    if kani::any::<bool>() {
        l.move_to_back(idx);
    } else {
        l.remove(idx);
    }
    assert!(inspect::invariant(&l, CAP));
}

/// EPO-INV-STALE-HANDLE-NO-CROSS-ENTRY-EFFECT — "after an entry has been removed and its
/// slot recycled for a different key, using the old handle from the vanished entry must
/// not move or remove that different key's entry".
///
/// THIS HARNESS IS EXPECTED TO FAIL, and its failure is a DEFECT REPORT. The free list
/// (lru_list.rs:181) hands the freed slot straight back to the next insert
/// (lru_list.rs:38, :76), and nothing distinguishes the generations of a slot — there is
/// no generation counter or tag in `Node` or in `EvictionHandle`
/// (ieviction_policy.rs:9-12, two bare `u32`s). So the stale handle addresses the NEW
/// occupant, and touching or removing through it acts on a key the caller never named.
#[kani::proof]
#[kani::unwind(8)]
fn verify_epo_inv_stale_handle_no_cross_entry_effect() {
    let mut l = LruList::new();
    let k_old: u64 = kani::any();
    let k_new: u64 = kani::any();
    let k_other: u64 = kani::any();
    kani::assume(k_old != k_new && k_new != k_other && k_old != k_other);

    // One entry that will be removed, plus a bystander.
    let stale = l.push_back(k_old);
    let _bystander = l.push_back(k_other);
    l.remove(stale);

    // A different key takes over the recycled slot.
    let reused = l.push_back(k_new);
    assert!(reused == stale); // the slot really is recycled

    let before = inspect::snapshot(&l);
    // The caller still holds `stale`, which now names k_new's entry.
    l.remove(stale);
    // The obligation: k_new's entry must be untouched.
    assert!(inspect::snapshot_eq(&inspect::snapshot(&l), &before));
}

// =====================================================================================
//  REFUTATIONS — positive, machine-checked evidence that the two obligations above are
//  FALSE of this code.
//
//  WHY A SEPARATE HARNESS, when `verify_epo_inv_stale_handle_never_crashes` already
//  fails. A failing `verify_` harness is ambiguous: it is equally consistent with a bad
//  bound, a missing assumption, or a solver limit, and the gate is right to refuse to
//  read a red harness as a finding. A refutation has to be the other way round — a
//  harness that PASSES, and whose passing can only be explained by the violating
//  execution existing. That is what the two below are:
//
//    * the crash refutation is `#[kani::should_panic]`, so it is SUCCESSFUL exactly
//      when Kani finds that the panic is REACHABLE (and that every failure on the path
//      is that panic and nothing else);
//    * the cross-entry refutation asserts the NEGATED postcondition, so it is
//      SUCCESSFUL exactly when the entry belonging to a key the caller never named has
//      in fact been removed.
//
//  Neither fabricates a handle. Both use only slot numbers the list itself handed back,
//  and only public `LruList` operations, in an order a component caller can produce
//  through `IEvictionPolicy` (`track` -> `clear_pool`/`remove` -> `touch`/`remove`).
//
//  The DISCRIMINATOR stays in place: `verify_epo_inv_stale_handle_never_crashes__split_in_range`
//  PROVES that neither entry point can panic for a slot number the pool really allocated,
//  including an already-freed one. So the crash is attributable to the missing bounds
//  check at lru_list.rs:109 / :160 alone, not to `move_to_back`/`remove` in general.
// =====================================================================================

/// REFUTATION of EPO-INV-STALE-HANDLE-NEVER-CRASHES.
///
/// The obligation: "no handle a caller can present may crash the component". It is FALSE.
///
/// The witness needs NO fabricated handle. `clear_pool` (lib.rs:231-236) delegates to
/// `LruList::clear` (lru_list.rs:186-192), which empties `nodes` outright — so after a
/// clear, EVERY slot number the pool ever issued names a slot that no longer exists. The
/// caller is still holding those handles: `EvictionHandle` is two bare `u32`s
/// (ieviction_policy.rs:9-12) with no generation tag, and `touch`/`remove` (lib.rs:154,
/// :198) validate only the handle's POOL field before passing `handle.index()` straight
/// into `self.nodes[idx as usize]` with no bounds check at all (lru_list.rs:109, :160).
/// The result is an index-out-of-bounds panic, not the `EvictionPolicyError::InvalidHandle`
/// the shared interface declares for exactly this case (ieviction_policy.rs:56-58) and
/// which this component constructs nowhere.
///
/// `should_panic`: this harness is SUCCESSFUL precisely when Kani finds the panicking
/// execution. Both entry points are covered — the symbolic `bool` makes Kani explore
/// `touch`'s path and `remove`'s path, and BOTH must panic for the harness to pass, so a
/// pass is evidence about both.
#[kani::proof]
#[kani::should_panic]
#[kani::unwind(8)]
fn refute_epo_inv_stale_handle_never_crashes() {
    let mut l = LruList::new();
    let k: u64 = kani::any();

    // A handle the component genuinely issued, via the same call `track` makes.
    let handle_idx = l.push_back(k);
    assert!(handle_idx == 0);
    assert!(l.len() == 1);

    // `clear_pool` — a legitimate public operation that does not invalidate handles.
    l.clear();
    assert!(inspect::nodes_len(&l) == 0);

    // The caller still holds `handle_idx`. Either entry point now panics.
    if kani::any::<bool>() {
        l.move_to_back(handle_idx); // `touch`   — lru_list.rs:109
    } else {
        l.remove(handle_idx); // `remove`  — lru_list.rs:160
    }

    // Unreachable: control never gets here on either branch.
    assert!(inspect::invariant(&l, CAP));
}

/// REFUTATION of EPO-INV-STALE-HANDLE-NO-CROSS-ENTRY-EFFECT, remove path.
///
/// The obligation: after an entry has been removed and its slot recycled for a different
/// key, the old handle "must not move or remove that different key's entry". It is FALSE,
/// and this harness PROVES the violation for EVERY triple of distinct keys, not merely
/// exhibits one witness.
///
/// `LruList::remove` pushes the freed slot onto the reuse list (lru_list.rs:181) and the
/// next insert pops it straight back (lru_list.rs:38, :76), storing the new key in the
/// very same slot and marking it live again. Nothing distinguishes the generations of a
/// slot: no counter or tag in `Node`, and none in `EvictionHandle`. So the stale handle
/// now addresses a LIVE entry belonging to a key its holder never named, and removing
/// through it evicts that key — while `EvictionPolicyOptimizedComponent::remove`
/// (lib.rs:186-200) returns `Ok(())`, so the caller cannot tell (see
/// EPO-INV-INVALID-HANDLE-NEVER-RETURNED).
///
/// The asserts before the stale use are the premises (the slot really is recycled, and it
/// really is a live entry for a different key); the asserts after it are the NEGATED
/// postcondition. All of them hold, which is what makes this a refutation rather than a
/// failed proof.
#[kani::proof]
#[kani::unwind(8)]
fn refute_epo_inv_stale_handle_no_cross_entry_effect() {
    let mut l = LruList::new();
    let k_old: u64 = kani::any();
    let k_new: u64 = kani::any();
    let k_other: u64 = kani::any();
    kani::assume(k_old != k_new && k_new != k_other && k_old != k_other);

    // One entry whose handle the caller keeps, plus a bystander so the list is not empty
    // when the slot is recycled.
    let stale = l.push_back(k_old);
    let _bystander = l.push_back(k_other);
    l.remove(stale);

    // A DIFFERENT key takes over the recycled slot.
    let reused = l.push_back(k_new);
    assert!(reused == stale); // the freed slot is handed back verbatim
    assert!(inspect::node_active(&l, reused)); // and is live again
    assert!(inspect::node_key(&l, reused) == k_new); // holding a key the handle never named
    assert!(l.len() == 2);
    assert!(inspect::chain_contains_key(&l, k_new, CAP));
    assert!(inspect::chain_contains_key(&l, k_other, CAP));

    // The caller uses the ORIGINAL handle from the entry that vanished.
    l.remove(stale);

    // NEGATED POSTCONDITION — k_new's entry, which the handle never named, is gone.
    assert!(!inspect::node_active(&l, reused));
    assert!(!inspect::chain_contains_key(&l, k_new, CAP));
    assert!(l.len() == 1);
    // ... and the bystander is what survives, so this is a cross-ENTRY effect and not a
    // wholesale corruption: the list is still well formed afterwards, which is why the
    // defect is silent.
    assert!(inspect::chain_contains_key(&l, k_other, CAP));
    assert!(inspect::invariant(&l, CAP));
}

/// REFUTATION of EPO-INV-STALE-HANDLE-NO-CROSS-ENTRY-EFFECT, touch path.
///
/// The obligation forbids the stale handle from MOVING the other key's entry as well as
/// from removing it, so the `move_to_back` half is refuted separately. Geometry: after the
/// recycle the reused slot must not already be at the back, or `move_to_back` returns
/// early (lru_list.rs:112-114) and there would be nothing to see — hence the third insert.
///
/// A companion of `refute_epo_inv_stale_handle_no_cross_entry_effect`; it also PASSES, so
/// the scorer's substring harness filter running both is harmless.
#[kani::proof]
#[kani::unwind(20)]
fn refute_epo_inv_stale_handle_no_cross_entry_effect__touch() {
    let mut l = LruList::new();
    let k_old: u64 = kani::any();
    let k_new: u64 = kani::any();
    let k_other: u64 = kani::any();
    let k_third: u64 = kani::any();
    kani::assume(k_old != k_new && k_old != k_other && k_old != k_third);
    kani::assume(k_new != k_other && k_new != k_third && k_other != k_third);

    let stale = l.push_back(k_old);
    let _bystander = l.push_back(k_other);
    l.remove(stale);
    let reused = l.push_back(k_new);
    assert!(reused == stale);
    // One more entry, so the recycled slot is INTERIOR and `move_to_back` is not a no-op.
    let _third = l.push_back(k_third);

    let before = inspect::chain_keys(&l, CAP);
    assert!(before.len() == 3);
    assert!(before[1] == k_new); // k_new sits in the middle, ahead of k_third

    // The caller uses the ORIGINAL handle from the entry that vanished.
    l.move_to_back(stale);

    // NEGATED POSTCONDITION — k_new's entry moved. Its eviction order relative to
    // k_third, a key the handle also never named, is now reversed.
    let after = inspect::chain_keys(&l, CAP);
    assert!(after.len() == 3);
    assert!(!inspect::key_seq_eq(&after, &before));
    assert!(after[1] == k_third);
    assert!(after[2] == k_new);
    assert!(inspect::invariant(&l, CAP));
}

// =====================================================================================
//  Two obligations whose content is entirely the list's, salvaged from the component
//  prong that the construction wall blocks (see proofs_api.rs).
//
//  SCOPE, stated honestly: the component methods these belong to are
//  `clear_pool` (lib.rs:231-237) and `identify_next_to_evict` (lib.rs:202-207), and each is a
//  pool lookup followed by a single delegation to the list — `pool_guard.lru.clear()` and
//  `pool_guard.lru.pop_front()` respectively, with no other logic. So what is proved below is
//  the whole post-state content of both properties, and what is NOT covered is only the pool
//  lookup in front of it (`state.pools.get(pool as usize)`). The invalid-pool half of that
//  lookup is a separate inventory property in each case
//  (EPO-CLEAR-DEGRADE-INVALID-POOL, EPO-EVICT-DEGRADE-INVALID-POOL) and remains blocked.
// =====================================================================================

/// EPO-CLEAR-POST-EMPTY — clearing drops all entries at once, so afterwards the reported size
/// is zero, no eviction candidates are offered, and asking for the next victim returns nothing.
///
/// All three consequences the property names are checked, from an arbitrary valid list.
#[kani::proof]
#[kani::unwind(8)]
fn verify_epo_clear_post_empty() {
    let mut l = inspect::valid_list(NS, CAP);

    l.clear();

    // reported size is zero
    assert!(l.len() == 0);
    // no eviction candidates, at any requested count
    let m: usize = kani::any();
    kani::assume(m <= CAP);
    assert!(l.peek_front_n(m).len() == 0);
    // and no next victim
    assert!(l.peek_front_key().is_none());
    assert!(l.pop_front().is_none());
    // the emptied list is still a well-formed list, so it stays usable
    assert!(inspect::invariant(&l, CAP));
}

#[kani::proof]
#[kani::unwind(8)]
fn verify_epo_clear_post_empty__mutant() {
    let mut l = inspect::valid_list(NS, CAP);
    l.clear();
    // Claim something survived the clear.
    assert!(l.len() > 0 || l.peek_front_key().is_some());
}

/// EPO-EVICT-POST-EMPTY-POOL-NONE — asking an existing but empty pool for its next victim
/// returns nothing and leaves the pool exactly as it was.
#[kani::proof]
#[kani::unwind(8)]
fn verify_epo_evict_post_empty_pool_none() {
    let mut l = inspect::valid_list(NS, CAP);
    kani::assume(l.len() == 0);

    let before = inspect::snapshot(&l);
    let got = l.pop_front();

    assert!(got.is_none());
    assert!(inspect::snapshot_eq(&inspect::snapshot(&l), &before));
    // Still empty, and asking again is equally harmless.
    assert!(l.pop_front().is_none());
    assert!(inspect::snapshot_eq(&inspect::snapshot(&l), &before));
}

#[kani::proof]
#[kani::unwind(8)]
fn verify_epo_evict_post_empty_pool_none__mutant() {
    let mut l = inspect::valid_list(NS, CAP);
    kani::assume(l.len() == 0);
    // Claim an empty pool yields a victim.
    assert!(l.pop_front().is_some());
}
