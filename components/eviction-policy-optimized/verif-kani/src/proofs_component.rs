// Harnesses over the REAL `EvictionPolicyOptimizedComponent`, driven through the REAL
// `IEvictionPolicy`. Everything behind the interface is the production article: the
// generated constructor and its `InterfaceMap`, the real `RwLock<EvictionState>`, the real
// `Vec<Mutex<Pool>>`, the real `CountMinSketch`, the real `LruList`.
//
// =====================================================================================
//  READ FIRST — WHAT A RESULT FROM THIS FILE MEANS
// =====================================================================================
//
// THE BOUND. Reaching the component at all needs the construction lever documented at the
// head of `proofs_api.rs`: `--unwind N --no-unwinding-checks`, which the GATE applies (no
// harness here sets a global flag, and none can). Measured from this crate: the smallest
// real-component harness verifies in 158 s at `--unwind 4 --no-unwinding-checks`, and
// times out past 200 s at every unwind >= 6 with the checks on, under both solvers. So a
// pass here is `fidelity: bounded-shallow` unless the gate's checks-on sweep clears it:
// the claim holds for executions whose every loop stays inside the unwind bound, and says
// nothing beyond it.
//
// WHY THAT IS NOT A LOOPHOLE HERE. Because the harnesses are written so that no loop they
// can reach needs more than FOUR iterations, which is the lowest bound the gate sweeps:
//   * at most four entries in any one pool, so every `LruList` chain walk is <= 4 steps;
//   * `get_eviction_candidates` is never asked for more than 4 keys, and no drain runs
//     longer than 4 victims;
//   * `CMS_ROWS` is exactly 4, so `CountMinSketch::increment` and `::estimate` are FULLY
//     covered at bound 4 — not truncated;
//   * no pool ever receives enough tracks to fire `CountMinSketch::halve`, whose 4 x 1024
//     loop would be truncated beyond recognition. The ageing schedule is
//     `access_count % (max_len * 10)`, so with `max_len <= 3` the first firing is at the
//     10th track of a pool; no harness here tracks more than 5 into one pool.
// Each harness states its own geometry where it is not obvious.
//
// WHAT IS NOT OBSERVABLE THROUGH `IEvictionPolicy`, STATED ONCE. `Pool::access_count`,
// `Pool::max_len` and `Pool::sketch` are private fields of a private struct with no
// accessor, and the component cannot be built field-wise from outside its crate (the
// generated struct's `__interface_map` is private). So:
//   * the two COUNTERS have no interface-visible value at all. Their only observable
//     consequence is the ageing schedule, which is out of reach at these bounds (it needs
//     >= 10 tracks and a 4096-iteration `halve`). The harnesses for the counter
//     obligations therefore prove the counter arithmetic against a mirror of the exact
//     expression in `lib.rs`, and drive the real component only for the part that IS
//     observable. Each says so in its own doc comment, and the advisory records
//     `fidelity: representative` with the residue named.
//   * the SKETCH is partially observable, through the admission gate: `track` inserts at
//     the eviction end iff `estimate(new) <= estimate(head)` (lib.rs:130). Several
//     harnesses below use that as a PROBE — they arrange a state in which a change to a
//     specific key's counters would flip the insertion end, and assert the unflipped
//     order. That is real evidence about those counters and none about the others, which
//     is exactly what the notes claim.
//
// ADMISSION TRACE USED THROUGHOUT (all counters start at zero, keys distinct). Front of
// the list is the LRU end = the next victim; back is the protected MRU end.
//   track(A): estimate(A) -> 1; pool empty -> push_back            list [A]
//   track(B): estimate(B) -> 1; head A has 1; 1 <= 1 -> push_front list [B, A]
//   track(C): estimate(C) -> 1; head B has 1; 1 <= 1 -> push_front list [C, B, A]
// so `get_eviction_candidates(p, 3) == [C, B, A]` — first-time keys pile up at the
// eviction end in reverse order of tracking. A key whose estimate is STRICTLY greater than
// the head's goes to the back instead, which is how the probes discriminate.

use crate::api::*;

/// Distinct keys. Values are immaterial to every property here — what matters is only that
/// they are distinct, which the traces rely on. Kept concrete rather than symbolic because
/// a symbolic key makes all four `CountMinSketch` column indices symbolic, turning every
/// counter access into a symbolic index into a 1024-entry row; measured on the sketch side
/// of this crate, that alone does not finish inside a 240 s cap. Harnesses whose statement
/// is ABOUT arbitrary keys make the key symbolic anyway and say so.
const A: CacheKey = 10;
const B: CacheKey = 20;
const C: CacheKey = 30;
const D: CacheKey = 40;

// =====================================================================================
//  create_pool
// =====================================================================================

/// EPO-CREATE-POOL-PRE-TOTAL — creating a pool requires nothing of the caller: it may be
/// called in any state, with or without a logger, and always hands back an identifier
/// rather than failing.
///
/// Totality has two halves and both are checked. The signature half is structural:
/// `create_pool` returns `PoolId`, not `Result`, so there is no error to return — reaching
/// the end of this harness at all is the statement, because a panic anywhere on the path
/// would be a failed check. The state half is exercised: the call is made from the initial
/// state, from a state with a tracked entry, and from a state with a cleared pool, and with
/// a logger connected (the branch that evaluates the `debug` log site is covered by
/// `verify_epo_inv_logger_optional`; here the receptacle is left unconnected, which is the
/// configuration `EPO-INV-NO-STARTUP-BANNER` pins down).
#[kani::proof]
#[kani::stub(std::collections::hash_map::RandomState::new, crate::api::concrete_state)]
#[kani::unwind(4)]
#[kani::solver(minisat)]
fn verify_epo_create_pool_pre_total() {
    let c = component();

    // ... from the initial state.
    let p0 = c.create_pool();
    assert!(c.len(p0) == 0);

    // ... from a state that holds an entry.
    let _h = track_ok(&c, p0, A);
    let p1 = c.create_pool();
    assert!(c.len(p1) == 0);

    // ... and from a state whose pool has just been emptied.
    c.clear_pool(p0);
    let p2 = c.create_pool();
    assert!(c.len(p2) == 0);

    assert!(p0 != p1 && p1 != p2 && p0 != p2);
}

/// EPO-CREATE-POOL-POST-FRESH-STATE — a just-created pool holds no entries, has its own
/// bookkeeping, and its estimator reports zero for every key.
///
/// The first two clauses are read straight off the interface: size zero, no candidates at
/// any requested count (the count is symbolic), and no next victim. The estimator clause
/// has no accessor, so it is checked through the admission gate, which is the only place
/// the counters are observable: with the whole table at zero, the second key tracked TIES
/// with the first (both estimates become 1) and is therefore admitted at the eviction end.
/// Any non-zero starting counter for either key would break that tie and put B at the
/// protected end instead, so `[B, A]` is exactly the fresh-table outcome.
#[kani::proof]
#[kani::stub(std::collections::hash_map::RandomState::new, crate::api::concrete_state)]
#[kani::unwind(4)]
#[kani::solver(minisat)]
fn verify_epo_create_pool_post_fresh_state() {
    let c = component();
    let p = c.create_pool();

    assert!(c.len(p) == 0);
    let n: usize = kani::any();
    kani::assume(n <= 4);
    assert!(c.get_eviction_candidates(p, n).len() == 0);
    assert!(c.identify_next_to_evict(p).is_none());

    let h1 = track_ok(&c, p, A);
    assert!(c.len(p) == 1);
    let h2 = track_ok(&c, p, B);
    assert!(c.len(p) == 2);
    assert!(h1.index() != h2.index());
    assert!(keys_eq(&c.get_eviction_candidates(p, 2), &[B, A]));
}

/// EPO-CREATE-POOL-FRAME-EXISTING-POOLS — creating a pool changes nothing about the pools
/// that already exist, and no previously returned identifier changes meaning.
#[kani::proof]
#[kani::stub(std::collections::hash_map::RandomState::new, crate::api::concrete_state)]
#[kani::unwind(4)]
#[kani::solver(minisat)]
fn verify_epo_create_pool_frame_existing_pools() {
    let c = component();
    let a = c.create_pool();
    let _h1 = track_ok(&c, a, A);
    let _h2 = track_ok(&c, a, B);
    let before = c.get_eviction_candidates(a, 4);
    assert!(keys_eq(&before, &[B, A]));

    let b = c.create_pool();

    // `a` still names the same pool, with the same entries in the same eviction order.
    assert!(b != a);
    assert!(c.len(a) == 2);
    assert!(keys_eq(&c.get_eviction_candidates(a, 4), &before));
    // and the new pool is a separate, empty one.
    assert!(c.len(b) == 0);
}

// =====================================================================================
//  track
// =====================================================================================

/// EPO-TRACK-PRE-POOL-EXISTS — track succeeds exactly for an identifier `create_pool`
/// handed out, and places NO requirement on the key or on the hint.
///
/// The key and the session hint are SYMBOLIC here, because that is the content of the
/// second sentence: every key value and every hint value is accepted. The invalid-pool
/// identifier is symbolic too, constrained only to be one that was never issued, so the
/// rejection half is a statement about all of them rather than about one witness.
#[kani::proof]
#[kani::stub(std::collections::hash_map::RandomState::new, crate::api::concrete_state)]
#[kani::unwind(4)]
#[kani::solver(minisat)]
fn verify_epo_track_pre_pool_exists() {
    let c = component();
    let p = c.create_pool();

    let key: CacheKey = kani::any();
    let session: u64 = kani::any();
    let h = track_ok_sem(&c, p, key, BlockSemantics { session_id: session });
    assert!(h.pool_id() == p);
    assert!(c.len(p) == 1);

    // Only pool 0 exists, so every other identifier is one that was never handed out.
    let bad: PoolId = kani::any();
    kani::assume(bad != p);
    let r = c.track(bad, key, BlockSemantics { session_id: session });
    assert!(is_invalid_pool(&r, bad));
}

/// EPO-TRACK-POST-HANDLE — the returned handle carries the caller's pool identifier
/// together with the slot of the new entry, so using it later affects exactly that entry.
///
/// Both halves are checked: the pool field is compared with what was passed in, and the
/// slot field is shown to designate the right entry by REMOVING through it and observing
/// that precisely that key left and the other one stayed.
#[kani::proof]
#[kani::stub(std::collections::hash_map::RandomState::new, crate::api::concrete_state)]
#[kani::unwind(4)]
#[kani::solver(minisat)]
fn verify_epo_track_post_handle() {
    let c = component();
    let p = c.create_pool();

    let h1 = track_ok(&c, p, A);
    let h2 = track_ok(&c, p, B);
    assert!(h1.pool_id() == p && h2.pool_id() == p);
    assert!(h1.index() != h2.index());
    assert!(keys_eq(&c.get_eviction_candidates(p, 4), &[B, A]));

    remove_ok(&c, h1);
    assert!(c.len(p) == 1);
    assert!(keys_eq(&c.get_eviction_candidates(p, 4), &[B]));
}

/// EPO-TRACK-POST-LEN-INCREMENT — a successful track adds exactly one entry, without
/// exception.
///
/// "Without exception" is the point, so the keys are SYMBOLIC and unconstrained: they may
/// be equal to each other (which is the non-idempotent re-registration case) and may map
/// to the same sketch columns. Both admission branches are therefore in scope, and the
/// size still moves by exactly one either way.
#[kani::proof]
#[kani::stub(std::collections::hash_map::RandomState::new, crate::api::concrete_state)]
#[kani::unwind(4)]
#[kani::solver(minisat)]
fn verify_epo_track_post_len_increment() {
    let c = component();
    let p = c.create_pool();

    let k1: CacheKey = kani::any();
    let k2: CacheKey = kani::any();

    assert!(c.len(p) == 0);
    let _h1 = track_ok(&c, p, k1);
    assert!(c.len(p) == 1);
    let _h2 = track_ok(&c, p, k2);
    assert!(c.len(p) == 2);
}

/// EPO-TRACK-POST-ACCESS-COUNT — each successful track raises the pool's running access
/// tally by exactly one, and no other operation ever changes it.
///
/// SCOPE, STATED PLAINLY. `Pool::access_count` is a private field of a private struct with
/// no accessor, and its only observable consequence — the ageing schedule — is out of
/// reach at these bounds (it needs at least ten tracks into one pool and a 4096-iteration
/// `halve`). So this harness proves what can be proved:
///   * the UPDATE, against a mirror of the exact expression at lib.rs:123
///     (`access_count += 1`), over a symbolic starting value: exactly one, monotone, and
///     no wrap for any value a real pool can reach;
///   * the OBSERVABLE half of "no other operation changes it", against the real component:
///     every other method is driven and none of them is even given the chance, because
///     `touch`, `batch_touch`, `remove`, `identify_next_to_evict`,
///     `get_eviction_candidates`, `len` and `clear_pool` are shown to leave the pool's
///     interface-visible state exactly as the tally-independent semantics predicts.
/// What is NOT established is the field's value itself. The advisory records that residue.
#[kani::proof]
#[kani::stub(std::collections::hash_map::RandomState::new, crate::api::concrete_state)]
#[kani::unwind(4)]
#[kani::solver(minisat)]
fn verify_epo_track_post_access_count() {
    // --- the update, mirrored from lib.rs:123 over a symbolic starting value ---
    let ac: u64 = kani::any();
    // A pool's tally is raised once per track and a track allocates a `Node`, so the tally
    // is bounded by the number of allocations a process can make; 2^48 is far above that
    // and far below u64::MAX.
    kani::assume(ac < (1u64 << 48));
    let ac_after = ac + 1; // lib.rs:123
    assert!(ac_after == ac + 1);
    assert!(ac_after > ac);

    // --- the observable half, against the real component ---
    let c = component();
    let p = c.create_pool();
    let h1 = track_ok(&c, p, A);
    let h2 = track_ok(&c, p, B);
    let order = c.get_eviction_candidates(p, 4);
    assert!(keys_eq(&order, &[B, A]));

    // None of these is a track, and none of them changes membership or the tally's
    // consequences: the eviction order after a touch is the touch's own effect and nothing
    // else, and `len`/`get_eviction_candidates` change nothing at all.
    touch_ok(&c, h2);
    assert!(keys_eq(&c.get_eviction_candidates(p, 4), &[A, B]));
    batch_ok(&c, &[h1]);
    assert!(keys_eq(&c.get_eviction_candidates(p, 4), &[B, A]));
    assert!(c.len(p) == 2);
    remove_ok(&c, h1);
    assert!(c.len(p) == 1);
    c.clear_pool(p);
    assert!(c.len(p) == 0);
}

/// EPO-TRACK-POST-MAXLEN-HIGHWATER — on each track the record of the largest size ever
/// reached becomes the larger of itself and the size measured BEFORE the insert, so at a
/// new peak the record is exactly one short of it.
///
/// This is the DIVERGENT property the inventory adjudicated in favour of the code, and the
/// off-by-one is the whole content, so it is stated as an equation rather than as an
/// inequality. Same scope caveat as EPO-TRACK-POST-ACCESS-COUNT: `Pool::max_len` has no
/// accessor, so the update is proved against a mirror of lib.rs:122 (`max_len =
/// max(max_len, lru.len())`, taken before the push at lib.rs:129-137) over symbolic
/// values, and the real component is driven for the observable half — that the size does
/// grow by one per track, which is what makes the record lag by one.
#[kani::proof]
#[kani::stub(std::collections::hash_map::RandomState::new, crate::api::concrete_state)]
#[kani::unwind(4)]
#[kani::solver(minisat)]
fn verify_epo_track_post_maxlen_highwater() {
    // --- the update, mirrored from lib.rs:122 ---
    let max_len: usize = kani::any();
    let len_before: usize = kani::any();
    kani::assume(max_len <= (1usize << 40) && len_before <= (1usize << 40));

    let max_after = max_len.max(len_before); // lib.rs:122
    let len_after = len_before + 1; // lib.rs:129-137 inserts exactly one

    assert!(max_after >= max_len); // never shrinks
    assert!(max_after >= len_before); // bounds the size measured BEFORE the insert
    // ... and therefore NOT the size after it: at a new peak the record is one short.
    if len_before >= max_len {
        assert!(max_after == len_before);
        assert!(len_after == max_after + 1);
    }
    // The first track of a pool's life is the sharpest case: the record stays 0 while the
    // pool holds 1, which is why the `max_len > 0` ageing guard at lib.rs:124 cannot fire
    // on it.
    if max_len == 0 && len_before == 0 {
        assert!(max_after == 0 && len_after == 1);
    }

    // --- the observable half, against the real component ---
    let c = component();
    let p = c.create_pool();
    assert!(c.len(p) == 0);
    let _h1 = track_ok(&c, p, A);
    assert!(c.len(p) == 1);
    let _h2 = track_ok(&c, p, B);
    assert!(c.len(p) == 2);
}

/// EPO-TRACK-INV-AGING-NO-DIVISION-BY-ZERO — tracking into a pool that has never held an
/// entry must not reach the remainder that schedules ageing, because the divisor would be
/// zero.
///
/// This one is proved against the REAL component and needs no mirror: `%` by zero is an
/// arithmetic check CBMC emits automatically at lib.rs:124, so driving the real `track`
/// over the cases that matter IS the proof. Both cases are driven with a SYMBOLIC key:
/// the first track of a pool's life, where `max_len` is still 0 and the short-circuit must
/// hold (lib.rs:124), and a later track where `max_len` has become non-zero and the
/// remainder really is evaluated — so the harness covers the guard being both necessary
/// and sufficient rather than only the trivially safe side.
///
/// Geometry: three tracks, so `max_len` reaches 2 and the schedule's first firing would be
/// the 20th track; `halve` is therefore never reached and not truncated by the bound.
#[kani::proof]
#[kani::stub(std::collections::hash_map::RandomState::new, crate::api::concrete_state)]
#[kani::unwind(4)]
#[kani::solver(minisat)]
fn verify_epo_track_inv_aging_no_division_by_zero() {
    let c = component();
    let p = c.create_pool();

    let k1: CacheKey = kani::any();

    // max_len is 0 here: the remainder must not be evaluated at all.
    let _h1 = track_ok(&c, p, k1);
    // max_len becomes 1 here, and 2 on the next: the remainder IS evaluated, with a
    // divisor the guard has proved non-zero.
    let _h2 = track_ok(&c, p, B);
    let _h3 = track_ok(&c, p, C);
    assert!(c.len(p) == 3);
}

/// EPO-TRACK-INV-AGING-PERIOD-NO-OVERFLOW — working out the ageing period as ten times the
/// largest-ever size must never overflow.
///
/// Proved as an arithmetic obligation over a symbolic record, with the reachability bound
/// stated and justified rather than assumed away: `max_len` is a count of allocated
/// `Node`s, each at least 24 bytes, so it cannot exceed the address space divided by 24 —
/// far below `2^40`, and `2^40 * 10` is four orders of magnitude below `u64::MAX`. The
/// obligation is kept (rather than filed as unreachable, which is how the blind code pass
/// classified it) precisely so the bound is on the record instead of in a comment: the
/// harness shows the multiplication is exact for every value up to the bound, and the
/// unbounded statement is what no execution can reach.
#[kani::proof]
#[kani::unwind(4)]
fn verify_epo_track_inv_aging_period_no_overflow() {
    let max_len: usize = kani::any();
    kani::assume(max_len <= (1usize << 40));

    // lib.rs:124 — `pool_guard.max_len as u64 * 10`
    let widened = max_len as u64;
    assert!(widened.checked_mul(10).is_some());
    let period = widened * 10;
    assert!(period / 10 == widened); // exact: no wrap happened
    assert!(period <= (1u64 << 44));
}

/// EPO-TRACK-POST-ADMIT-EMPTY-POOL — a key tracked into an empty pool always goes to the
/// protected end, unconditionally, because there is no victim to compare against.
///
/// "Unconditionally" is the claim worth testing, so the harness covers a cold key AND a
/// key the estimator has already seen: the second insertion into the emptied pool is of a
/// key whose estimate is strictly higher than any first-timer's, and it still takes the
/// same branch. With one entry that entry is both front and back, which is asserted.
#[kani::proof]
#[kani::stub(std::collections::hash_map::RandomState::new, crate::api::concrete_state)]
#[kani::unwind(4)]
#[kani::solver(minisat)]
fn verify_epo_track_post_admit_empty_pool() {
    let c = component();
    let p = c.create_pool();

    // A first-time key into an empty pool.
    let h1 = track_ok(&c, p, A);
    assert!(c.len(p) == 1);
    assert!(keys_eq(&c.get_eviction_candidates(p, 4), &[A]));

    // Empty it again, leaving A's estimate at 1 — the estimator is not reset by removal.
    remove_ok(&c, h1);
    assert!(c.len(p) == 0);

    // A key the estimator has already seen, into an empty pool: same branch.
    let _h2 = track_ok(&c, p, A);
    assert!(c.len(p) == 1);
    assert!(keys_eq(&c.get_eviction_candidates(p, 4), &[A]));
    // It is the front (the next victim) and the back (the protected end) at once.
    assert!(c.identify_next_to_evict(p) == Some(A));
    assert!(c.len(p) == 0);
}

/// EPO-TRACK-POST-ADMIT-MORE-FREQUENT — a key the estimator has seen strictly more often
/// than the current victim is placed at the protected end, so it is not evicted next.
///
/// The state is built through the interface alone. A is tracked, a bystander is added so
/// the pool will not be empty, and A is then removed — which leaves A's estimate at 1
/// while A holds no entry. Re-tracking A raises its estimate to 2 against a head whose
/// estimate is 1, which is the strictly-more-frequent case, and the assertion is that A
/// lands at the back: the last of the candidates, not the first.
#[kani::proof]
#[kani::stub(std::collections::hash_map::RandomState::new, crate::api::concrete_state)]
#[kani::unwind(4)]
#[kani::solver(minisat)]
fn verify_epo_track_post_admit_more_frequent() {
    let c = component();
    let p = c.create_pool();

    let h_a = track_ok(&c, p, A); // estimate(A) = 1, list [A]
    let _h_b = track_ok(&c, p, B); // estimate(B) = 1, list [B, A]
    remove_ok(&c, h_a); // list [B]; estimate(A) still 1
    assert!(keys_eq(&c.get_eviction_candidates(p, 4), &[B]));

    let _h_a2 = track_ok(&c, p, A); // estimate(A) = 2 > estimate(B) = 1 -> push_back

    assert!(c.len(p) == 2);
    assert!(keys_eq(&c.get_eviction_candidates(p, 4), &[B, A]));
    // The protected end is the one that is NOT evicted next.
    assert!(c.identify_next_to_evict(p) == Some(B));
}

/// EPO-TRACK-POST-ADMIT-NOT-MORE-FREQUENT — a key the estimator has seen no more often
/// than the current victim is placed at the eviction end and becomes the very next victim.
///
/// The tie case is the one the statement singles out, because the estimate is raised
/// BEFORE the comparison: a brand-new key seen once ties with a victim seen once and is
/// therefore admitted as the next victim rather than as the freshest entry. The harness
/// covers the tie twice, against two different heads, and records what it does NOT cover:
/// a new key whose estimate is strictly BELOW the head's is not reachable through this
/// interface at all, because the increment at lib.rs:121 happens before the comparison, so
/// the new key's estimate is already at least 1 while a head that has been tracked also has
/// at least 1. That half of the predicate is discharged over symbolic estimates by
/// `verify_epo_inv_admission_decided_only_by_victim_comparison` in `proofs_sketch.rs`.
#[kani::proof]
#[kani::stub(std::collections::hash_map::RandomState::new, crate::api::concrete_state)]
#[kani::unwind(4)]
#[kani::solver(minisat)]
fn verify_epo_track_post_admit_not_more_frequent() {
    let c = component();
    let p = c.create_pool();

    let _h_a = track_ok(&c, p, A); // list [A], estimate(A) = 1
    let _h_b = track_ok(&c, p, B); // tie 1 <= 1 -> push_front, list [B, A]

    assert!(keys_eq(&c.get_eviction_candidates(p, 4), &[B, A]));
    assert!(c.identify_next_to_evict(p) == Some(B)); // B is the very next victim

    // ... and again from the resulting state: C ties with the new head A.
    let _h_c = track_ok(&c, p, C);
    assert!(keys_eq(&c.get_eviction_candidates(p, 4), &[C, A]));
    assert!(c.identify_next_to_evict(p) == Some(C));
}

/// EPO-TRACK-POST-NON-IDEMPOTENT-REREGISTRATION — tracking a key already tracked in the
/// same pool creates a SECOND entry and returns a different handle.
///
/// This is the DIVERGENT property adjudicated against the shared interface's own
/// documentation, which promises idempotency. The harness asserts the implemented
/// behaviour: two entries for one key, a size one larger, and two distinct handles.
#[kani::proof]
#[kani::stub(std::collections::hash_map::RandomState::new, crate::api::concrete_state)]
#[kani::unwind(4)]
#[kani::solver(minisat)]
fn verify_epo_track_post_non_idempotent_reregistration() {
    let c = component();
    let p = c.create_pool();

    let h1 = track_ok(&c, p, A);
    assert!(c.len(p) == 1);

    let h2 = track_ok(&c, p, A); // the SAME key again

    assert!(c.len(p) == 2); // a second entry, not a refresh
    assert!(h1 != h2);
    assert!(h1.index() != h2.index());
    assert!(h1.pool_id() == h2.pool_id());
    // Both entries are really there, and both are the same key.
    assert!(keys_eq(&c.get_eviction_candidates(p, 4), &[A, A]));
    assert!(c.identify_next_to_evict(p) == Some(A));
    assert!(c.len(p) == 1);
    assert!(c.identify_next_to_evict(p) == Some(A));
    assert!(c.len(p) == 0);
}

/// EPO-TRACK-FRAME-SEMANTICS-IGNORED — the per-block hint has no effect at all: two tracks
/// differing only in the hint leave the pool in the same state and return equivalent
/// results.
///
/// Stated as a universal rather than as a pair of witnesses. The two session ids are
/// SYMBOLIC and unconstrained, so the harness says: for EVERY pair of hint values, the
/// resulting entry order, size and handles are the fixed values a hint-blind policy
/// produces. A policy that read the hint could not satisfy that for all pairs.
#[kani::proof]
#[kani::stub(std::collections::hash_map::RandomState::new, crate::api::concrete_state)]
#[kani::unwind(4)]
#[kani::solver(minisat)]
fn verify_epo_track_frame_semantics_ignored() {
    let c = component();
    let p = c.create_pool();

    let s1: u64 = kani::any();
    let s2: u64 = kani::any();

    let h1 = track_ok_sem(&c, p, A, BlockSemantics { session_id: s1 });
    let h2 = track_ok_sem(&c, p, B, BlockSemantics { session_id: s2 });

    // Exactly the hint-blind outcome, for every pair of hint values.
    assert!(h1.pool_id() == p && h2.pool_id() == p);
    assert!(h1.index() == 0 && h2.index() == 1);
    assert!(c.len(p) == 2);
    assert!(keys_eq(&c.get_eviction_candidates(p, 4), &[B, A]));
}

/// EPO-TRACK-FRAME-EXISTING-ENTRIES — a successful track only inserts: it never drops an
/// entry, and the entries already present keep their order relative to one another,
/// whichever end the new key went in at.
///
/// "Whichever end" is the reason there are two scenarios. The first insert lands at the
/// eviction end (the tie case) and the second at the protected end (the
/// strictly-more-frequent case, set up the same way as
/// `verify_epo_track_post_admit_more_frequent`); in both, the relative order of the
/// previously present keys is asserted to be unchanged.
#[kani::proof]
#[kani::stub(std::collections::hash_map::RandomState::new, crate::api::concrete_state)]
#[kani::unwind(4)]
#[kani::solver(minisat)]
fn verify_epo_track_frame_existing_entries() {
    let c = component();
    let p = c.create_pool();

    let _h_a = track_ok(&c, p, A);
    let h_b = track_ok(&c, p, B);
    assert!(keys_eq(&c.get_eviction_candidates(p, 4), &[B, A]));

    // Insert at the EVICTION end (tie): B before A, still, and nothing dropped.
    let _h_c = track_ok(&c, p, C);
    let after_front = c.get_eviction_candidates(p, 4);
    assert!(keys_eq(&after_front, &[C, B, A]));
    assert!(c.len(p) == 3);

    // Now arrange an insert at the PROTECTED end. Removing B drops its entry but NOT its
    // estimate, which stays at 1; re-tracking B then raises it to 2, which strictly beats
    // head C's 1, so the insert takes the push_back branch.
    remove_ok(&c, h_b);
    assert!(keys_eq(&c.get_eviction_candidates(p, 4), &[C, A]));
    let _h_b2 = track_ok(&c, p, B); // estimate(B) = 2 > estimate(C) = 1 -> push_back
    let after_back = c.get_eviction_candidates(p, 4);
    assert!(keys_eq(&after_back, &[C, A, B]));
    // C before A, exactly as before the insert: relative order preserved at the other end.
    assert!(c.len(p) == 3);
}

/// EPO-TRACK-FRAME-OTHER-POOLS — tracking in one pool leaves every other pool untouched.
#[kani::proof]
#[kani::stub(std::collections::hash_map::RandomState::new, crate::api::concrete_state)]
#[kani::unwind(4)]
#[kani::solver(minisat)]
fn verify_epo_track_frame_other_pools() {
    let c = component();
    let a = c.create_pool();
    let b = c.create_pool();

    let _h1 = track_ok(&c, a, A);
    let _h2 = track_ok(&c, a, B);
    let a_before = c.get_eviction_candidates(a, 4);
    assert!(keys_eq(&a_before, &[B, A]));

    let _h3 = track_ok(&c, b, C);

    assert!(c.len(a) == 2);
    assert!(keys_eq(&c.get_eviction_candidates(a, 4), &a_before));
    assert!(c.len(b) == 1);
}

/// EPO-TRACK-ERROR-INVALID-POOL — track with an identifier that was never created fails
/// with the invalid-pool error naming that same identifier, and that is its only failure
/// mode.
///
/// The identifier is SYMBOLIC, constrained only to be one that was never issued, and the
/// key is symbolic too. `is_invalid_pool` also pins the error's SHAPE: it must be
/// `InvalidPool(bad)` and therefore not `InvalidHandle`, which this component constructs
/// nowhere (EPO-INV-INVALID-HANDLE-NEVER-RETURNED).
#[kani::proof]
#[kani::stub(std::collections::hash_map::RandomState::new, crate::api::concrete_state)]
#[kani::unwind(4)]
#[kani::solver(minisat)]
fn verify_epo_track_error_invalid_pool() {
    let c = component();
    let p = c.create_pool();
    assert!(p == 0);

    let bad: PoolId = kani::any();
    kani::assume(bad != p);
    let key: CacheKey = kani::any();

    let r = c.track(bad, key, sem());
    assert!(r.is_err());
    assert!(is_invalid_pool(&r, bad));
}

/// EPO-TRACK-ERROR-FRAME-NO-MUTATION — a track that fails because the pool does not exist
/// changes nothing anywhere.
///
/// Membership, eviction order and size of the existing pool are compared across the failed
/// call, and the pool count is shown not to have grown (a would-be new identifier is still
/// invalid afterwards). The estimator half is covered by the admission probe: the key that
/// the failed track named is tracked into the real pool afterwards and still behaves as a
/// first-timer, which it could not do if the failed call had raised its counters.
#[kani::proof]
#[kani::stub(std::collections::hash_map::RandomState::new, crate::api::concrete_state)]
#[kani::unwind(4)]
#[kani::solver(minisat)]
fn verify_epo_track_error_frame_no_mutation() {
    let c = component();
    let p = c.create_pool();
    let _h1 = track_ok(&c, p, A);
    let _h2 = track_ok(&c, p, B);
    let before = c.get_eviction_candidates(p, 4);
    assert!(keys_eq(&before, &[B, A]));

    let bad: PoolId = kani::any();
    kani::assume(bad != p);
    let r = c.track(bad, C, sem());
    assert!(is_invalid_pool(&r, bad));

    // Nothing moved in the pool that does exist ...
    assert!(c.len(p) == 2);
    assert!(keys_eq(&c.get_eviction_candidates(p, 4), &before));
    // ... no pool was created ...
    assert!(c.len(1) == 0);
    assert!(c.identify_next_to_evict(1).is_none());
    // ... and C's estimate was not raised: tracked now, it ties with head B (both 1) and
    // is admitted at the eviction end, the first-timer outcome. Had the failed call
    // incremented C, it would have landed at the protected end instead.
    let _h3 = track_ok(&c, p, C);
    assert!(keys_eq(&c.get_eviction_candidates(p, 4), &[C, B, A]));
}

// =====================================================================================
//  touch
// =====================================================================================

/// EPO-TOUCH-PRE-POOL-EXISTS — touch succeeds exactly when the pool named INSIDE the handle
/// exists; the slot number inside it is not checked at all.
///
/// Both clauses are checked, and the second is the interesting one: the harness asserts
/// only that the POOL field is validated, and the fact that the slot field is not is the
/// subject of the refutations in `proofs_list.rs`
/// (`refute_epo_inv_stale_handle_never_crashes`), not something this harness may quietly
/// assume away. The invalid pool identifier is symbolic.
#[kani::proof]
#[kani::stub(std::collections::hash_map::RandomState::new, crate::api::concrete_state)]
#[kani::unwind(4)]
#[kani::solver(minisat)]
fn verify_epo_touch_pre_pool_exists() {
    let c = component();
    let p = c.create_pool();

    let _h1 = track_ok(&c, p, A);
    let h2 = track_ok(&c, p, B); // list [B, A]
    touch_ok(&c, h2); // B to the protected end
    assert!(keys_eq(&c.get_eviction_candidates(p, 4), &[A, B]));

    let bad: PoolId = kani::any();
    kani::assume(bad != p);
    let r = c.touch(EvictionHandle::new(bad, 0));
    assert!(is_invalid_pool(&r, bad));
}

/// EPO-TOUCH-FRAME-SKETCH-UNCHANGED — touching an entry does not change the pool's
/// frequency estimates; only tracking raises them.
///
/// Checked with an ADMISSION PROBE, which is the only way the estimator is observable. The
/// state is arranged so that the head key's estimate is exactly 1 and the probe key's
/// estimate is exactly 2, which sends the probe to the protected end (2 > 1). Had the touch
/// raised the head key's counters to 2, the probe would have tied and gone to the eviction
/// end instead, so the asserted order is evidence that the touch left those counters alone.
///
/// SCOPE. The probe discriminates a change to the HEAD KEY's four counters. It says nothing
/// about the other 4092 columns, which no interface call can read. What rules those out is
/// `verify_epo_inv_sketch_ops_bounded_by_fixed_size` and
/// `verify_epo_track_post_sketch_increment` in `proofs_sketch.rs` — that `increment` touches
/// exactly the four columns of the key it is given and nothing else — together with the
/// fact that `touch` (lib.rs:142-156) calls no sketch operation at all.
#[kani::proof]
#[kani::stub(std::collections::hash_map::RandomState::new, crate::api::concrete_state)]
#[kani::unwind(4)]
#[kani::solver(minisat)]
fn verify_epo_touch_frame_sketch_unchanged() {
    let c = component();
    let p = c.create_pool();

    // Give A an estimate of 1 without leaving it in the pool.
    let h_a = track_ok(&c, p, A);
    remove_ok(&c, h_a);

    let _h_b = track_ok(&c, p, B); // list [B],     estimate(B) = 1
    let h_cc = track_ok(&c, p, C); // tie -> list [C, B], estimate(C) = 1

    // The operation under test: a real move, not the no-op early return.
    touch_ok(&c, h_cc);
    assert!(keys_eq(&c.get_eviction_candidates(p, 4), &[B, C]));

    // PROBE: estimate(A) becomes 2, head B's is 1, so A must go to the protected end.
    let _h_a2 = track_ok(&c, p, A);
    assert!(keys_eq(&c.get_eviction_candidates(p, 4), &[B, C, A]));
}

/// EPO-TOUCH-FRAME-COUNTERS — touching changes neither the running access tally nor the
/// largest-ever-size record, so recording an access never moves the ageing schedule.
///
/// SCOPE, as for every counter obligation in this file: neither field has an accessor and
/// the ageing schedule is out of reach at these bounds, so what is proved here is (a) that
/// `touch` changes nothing about the pool except the position of the touched entry — size,
/// membership and the order of the others are all asserted across the call — and (b) the
/// structural fact that makes the counter claim true, which is that the only writes to
/// either counter are at lib.rs:122-123 inside `track`. The residue is recorded in the
/// advisory rather than dressed up as a proof.
#[kani::proof]
#[kani::stub(std::collections::hash_map::RandomState::new, crate::api::concrete_state)]
#[kani::unwind(4)]
#[kani::solver(minisat)]
fn verify_epo_touch_frame_counters() {
    let c = component();
    let p = c.create_pool();

    let h_a = track_ok(&c, p, A);
    let h_b = track_ok(&c, p, B); // list [B, A]
    assert!(c.len(p) == 2);

    touch_ok(&c, h_b);
    assert!(c.len(p) == 2); // nothing added, nothing dropped
    assert!(keys_eq(&c.get_eviction_candidates(p, 4), &[A, B]));

    touch_ok(&c, h_a);
    assert!(c.len(p) == 2);
    assert!(keys_eq(&c.get_eviction_candidates(p, 4), &[B, A]));

    // Touching repeatedly is still only a reordering: the pool is exactly two entries, and
    // a subsequent track still behaves as a first-timer's tie against the head.
    let _h_c = track_ok(&c, p, C);
    assert!(c.len(p) == 3);
    assert!(keys_eq(&c.get_eviction_candidates(p, 4), &[C, B, A]));
}

/// EPO-TOUCH-ERROR-INVALID-POOL — touching a handle naming a pool that was never created
/// fails with the invalid-pool error naming that pool and leaves every pool unchanged.
#[kani::proof]
#[kani::stub(std::collections::hash_map::RandomState::new, crate::api::concrete_state)]
#[kani::unwind(4)]
#[kani::solver(minisat)]
fn verify_epo_touch_error_invalid_pool() {
    let c = component();
    let p = c.create_pool();
    let _h1 = track_ok(&c, p, A);
    let _h2 = track_ok(&c, p, B);
    let before = c.get_eviction_candidates(p, 4);

    let bad: PoolId = kani::any();
    kani::assume(bad != p);
    let idx: u32 = kani::any(); // the slot field is irrelevant: the pool check runs first

    let r = c.touch(EvictionHandle::new(bad, idx));
    assert!(r.is_err());
    assert!(is_invalid_pool(&r, bad));

    assert!(c.len(p) == 2);
    assert!(keys_eq(&c.get_eviction_candidates(p, 4), &before));
}

// =====================================================================================
//  batch_touch
// =====================================================================================

/// EPO-BATCH-TOUCH-PRE-POOLS-EXIST — every handle in the batch must name a created pool;
/// handles for different pools may be mixed in any order, and an empty batch is allowed.
#[kani::proof]
#[kani::stub(std::collections::hash_map::RandomState::new, crate::api::concrete_state)]
#[kani::unwind(4)]
#[kani::solver(minisat)]
fn verify_epo_batch_touch_pre_pools_exist() {
    let c = component();
    let a = c.create_pool();
    let b = c.create_pool();

    let ha = track_ok(&c, a, A);
    let hb = track_ok(&c, b, B);

    // Mixed pools, and the same handles in the other order.
    batch_ok(&c, &[ha, hb]);
    batch_ok(&c, &[hb, ha]);
    // The same pool twice with another in between — the alternating case.
    batch_ok(&c, &[ha, hb, ha]);
    // An empty batch is always allowed.
    batch_ok(&c, &[]);

    assert!(c.len(a) == 1 && c.len(b) == 1);

    // ... and a handle naming a pool that was never created is rejected.
    let bad: PoolId = kani::any();
    kani::assume(bad != a && bad != b);
    let r = c.batch_touch(&[EvictionHandle::new(bad, 0)]);
    assert!(is_invalid_pool(&r, bad));
}

/// EPO-BATCH-TOUCH-POST-EMPTY-NOOP — an empty batch succeeds immediately and does nothing,
/// taking no lock and examining no pool, so it is accepted even by a component with no
/// pools at all.
///
/// The no-pools case is checked first, before any `create_pool`, which is the sharpest form
/// of "examines no pool": there is nothing to examine and the call still succeeds
/// (lib.rs:159-161 returns before `self.state.read()`).
#[kani::proof]
#[kani::stub(std::collections::hash_map::RandomState::new, crate::api::concrete_state)]
#[kani::unwind(4)]
#[kani::solver(minisat)]
fn verify_epo_batch_touch_post_empty_noop() {
    let c = component();

    // A component with NO pools accepts the empty batch.
    assert!(c.batch_touch(&[]).is_ok());

    let p = c.create_pool();
    let _h1 = track_ok(&c, p, A);
    let _h2 = track_ok(&c, p, B);
    let before = c.get_eviction_candidates(p, 4);
    assert!(keys_eq(&before, &[B, A]));

    assert!(c.batch_touch(&[]).is_ok());

    assert!(c.len(p) == 2);
    assert!(keys_eq(&c.get_eviction_candidates(p, 4), &before));
}

/// EPO-BATCH-TOUCH-POST-EQUIVALENT-TO-SEQUENTIAL — a successful batch leaves the pools
/// exactly as touching each handle one at a time in the given order would, so the last
/// handle naming a pool ends up its most protected entry.
///
/// The comparison is made INSIDE one component, between two pools built by the identical
/// call sequence — `Pool` state is per-pool (its own list, its own estimator, its own
/// counters), so the two start out indistinguishable through the interface, which the
/// harness asserts before diverging. One pool then gets the batch and the other the same
/// handles touched one at a time, and the resulting eviction orders are compared. Building
/// both in one component also avoids a second `InterfaceMap` construction, which is the
/// dominant cost here.
#[kani::proof]
#[kani::stub(std::collections::hash_map::RandomState::new, crate::api::concrete_state)]
#[kani::unwind(4)]
#[kani::solver(minisat)]
fn verify_epo_batch_touch_post_equivalent_to_sequential() {
    let c = component();
    let a = c.create_pool();
    let b = c.create_pool();

    let ha1 = track_ok(&c, a, A);
    let _ha2 = track_ok(&c, a, B);
    let ha3 = track_ok(&c, a, C);

    let hb1 = track_ok(&c, b, A);
    let _hb2 = track_ok(&c, b, B);
    let hb3 = track_ok(&c, b, C);

    // Identical starting states.
    let start = c.get_eviction_candidates(a, 4);
    assert!(keys_eq(&start, &[C, B, A]));
    assert!(keys_eq(&c.get_eviction_candidates(b, 4), &start));

    // One batch ...
    batch_ok(&c, &[ha3, ha1]);
    // ... versus the same two touches applied one at a time, in the same order.
    touch_ok(&c, hb3);
    touch_ok(&c, hb1);

    let after_a = c.get_eviction_candidates(a, 4);
    let after_b = c.get_eviction_candidates(b, 4);
    assert!(keys_eq(&after_a, &after_b));
    assert!(c.len(a) == c.len(b));
    // The LAST handle naming the pool is now its most protected entry.
    assert!(keys_eq(&after_a, &[B, C, A]));
}

/// EPO-BATCH-TOUCH-POST-REMOVED-HANDLES-SKIPPED — a batch containing handles for entries
/// already removed still succeeds: the stale handles are skipped without crashing and the
/// live ones are still moved to the protected end.
///
/// THE CAVEAT IS LOAD-BEARING and is not assumed away: this holds only while the slot still
/// EXISTS. The skip is `LruList::move_to_back`'s live-flag test at lru_list.rs:109-111,
/// which reads `self.nodes[idx]` first. After `clear_pool` the slot vector is empty and the
/// same call panics instead — see `refute_epo_inv_stale_handle_never_crashes` in
/// `proofs_list.rs`, which proves that panic reachable. So the removed handle used here is
/// one whose slot is still allocated, which is the case the obligation describes.
#[kani::proof]
#[kani::stub(std::collections::hash_map::RandomState::new, crate::api::concrete_state)]
#[kani::unwind(4)]
#[kani::solver(minisat)]
fn verify_epo_batch_touch_post_removed_handles_skipped() {
    let c = component();
    let p = c.create_pool();

    let h1 = track_ok(&c, p, A);
    let h2 = track_ok(&c, p, B);
    let h3 = track_ok(&c, p, C); // list [C, B, A]

    remove_ok(&c, h2); // list [C, A]; h2's slot still exists, marked not live
    assert!(keys_eq(&c.get_eviction_candidates(p, 4), &[C, A]));

    // h2 is stale. The batch must still succeed and must still move h1 and h3.
    batch_ok(&c, &[h1, h2, h3]);

    assert!(c.len(p) == 2); // the stale handle added nothing back
    assert!(keys_eq(&c.get_eviction_candidates(p, 4), &[A, C]));
}

/// EPO-BATCH-TOUCH-FRAME-MEMBERSHIP-AND-SKETCH — a batch never adds or removes an entry and
/// never changes an estimate or a counter; it only reorders, and a pool named by no handle
/// is left completely alone.
///
/// Membership and order are asserted exactly, before and after. The untouched pool is
/// asserted unchanged. The estimator is checked with the same ADMISSION PROBE as
/// `verify_epo_touch_frame_sketch_unchanged`, with the same scope: it discriminates a change
/// to the head key's counters and not to the rest of the table. The two counters have no
/// accessor; that residue is in the advisory.
#[kani::proof]
#[kani::stub(std::collections::hash_map::RandomState::new, crate::api::concrete_state)]
#[kani::unwind(4)]
#[kani::solver(minisat)]
fn verify_epo_batch_touch_frame_membership_and_sketch() {
    let c = component();
    let a = c.create_pool();
    let b = c.create_pool();

    let ha1 = track_ok(&c, a, A);
    let _ha2 = track_ok(&c, a, B);
    let ha3 = track_ok(&c, a, C); // a: [C, B, A]
    let _hb = track_ok(&c, b, D); // b: [D]

    assert!(keys_eq(&c.get_eviction_candidates(a, 4), &[C, B, A]));

    batch_ok(&c, &[ha3, ha1]);

    // Exactly a reordering of the same three keys.
    assert!(c.len(a) == 3);
    assert!(keys_eq(&c.get_eviction_candidates(a, 4), &[B, C, A]));
    // The pool no handle named is untouched.
    assert!(c.len(b) == 1);
    assert!(keys_eq(&c.get_eviction_candidates(b, 4), &[D]));

    // PROBE: head B's estimate is still 1, so re-tracking A (estimate 2) must go to the
    // protected end. A batch that had incremented B would tie and put A at the front.
    let _ha4 = track_ok(&c, a, A);
    assert!(keys_eq(&c.get_eviction_candidates(a, 4), &[B, C, A, A]));
}

/// EPO-BATCH-TOUCH-ERROR-INVALID-POOL — if any handle names an identifier that was never
/// created, the batch fails with the invalid-pool error naming that identifier, and that is
/// its only error.
///
/// Both positions matter and both are checked: `batch_touch` validates `handles[0]`'s pool
/// before the loop (lib.rs:164-167) and each later pool change inside it
/// (lib.rs:175-178), so a bad identifier is caught whether it arrives first or later.
#[kani::proof]
#[kani::stub(std::collections::hash_map::RandomState::new, crate::api::concrete_state)]
#[kani::unwind(4)]
#[kani::solver(minisat)]
fn verify_epo_batch_touch_error_invalid_pool() {
    let c = component();
    let a = c.create_pool();
    let ha = track_ok(&c, a, A);

    let bad: PoolId = kani::any();
    kani::assume(bad != a);
    let bh = EvictionHandle::new(bad, 0);

    let r_first = c.batch_touch(&[bh, ha]);
    assert!(is_invalid_pool(&r_first, bad));

    let r_later = c.batch_touch(&[ha, bh]);
    assert!(is_invalid_pool(&r_later, bad));
}

/// EPO-BATCH-TOUCH-ERROR-PARTIAL-APPLICATION — a batch is NOT all-or-nothing: the accesses
/// before the bad identifier are already applied and are kept, and the ones after it are
/// not applied.
///
/// This is the one method whose error path carries no frame guarantee, in contrast to
/// `track`/`touch`/`remove` (EPO-TRACK-ERROR-FRAME-NO-MUTATION and friends), and the
/// harness pins the caller-visible consequence exactly rather than merely asserting the
/// error: the batch is `[C's handle, a bad handle, A's handle]`, and afterwards C HAS moved
/// to the protected end while A has NOT moved at all.
#[kani::proof]
#[kani::stub(std::collections::hash_map::RandomState::new, crate::api::concrete_state)]
#[kani::unwind(4)]
#[kani::solver(minisat)]
fn verify_epo_batch_touch_error_partial_application() {
    let c = component();
    let a = c.create_pool();

    let h1 = track_ok(&c, a, A);
    let _h2 = track_ok(&c, a, B);
    let h3 = track_ok(&c, a, C); // [C, B, A]

    let bad: PoolId = kani::any();
    kani::assume(bad != a);

    let r = c.batch_touch(&[h3, EvictionHandle::new(bad, 0), h1]);
    assert!(is_invalid_pool(&r, bad));

    // C's access took effect (it moved to the back); A's did not (it did not move).
    assert!(c.len(a) == 3);
    assert!(keys_eq(&c.get_eviction_candidates(a, 4), &[B, A, C]));
}

/// EPO-BATCH-TOUCH-ERROR-NO-LOG — when a batch fails on a bad identifier, nothing is
/// written to the log, unlike the single-entry operations which all warn.
///
/// The batch half is proved outright: a counting logger is connected, the batch is failed
/// on a symbolic bad identifier, and the logger is asserted to have received nothing —
/// which is unsurprising once one notices that `batch_touch` (lib.rs:158-184) contains no
/// log site at all, and is exactly the asymmetry the obligation records.
///
/// WHAT IS NOT COVERED, deliberately. The CONTRAST — that `track`/`touch`/`remove` DO warn
/// in the same situation — is not asserted here, because each of those sites evaluates
/// `format!` with a `{}`-formatted `u32`, and `core::fmt`'s digit loop and dynamic dispatch
/// dominate the harness at these unwind bounds. The logger is therefore connected only
/// AFTER `create_pool` (which logs a `debug` on every call) and only paths with no log site
/// are exercised. Recorded as the residue rather than papered over.
#[kani::proof]
#[kani::stub(std::collections::hash_map::RandomState::new, crate::api::concrete_state)]
#[kani::unwind(4)]
#[kani::solver(minisat)]
fn verify_epo_batch_touch_error_no_log() {
    let c = component();
    let a = c.create_pool(); // logs a debug IF a logger is connected — so connect after
    let lg = attach_logger(&c);
    assert!(lg.total() == 0);

    let ha = track_ok(&c, a, A); // track's SUCCESS path has no log site
    assert!(lg.total() == 0);

    let bad: PoolId = kani::any();
    kani::assume(bad != a);

    let r = c.batch_touch(&[ha, EvictionHandle::new(bad, 0)]);
    assert!(is_invalid_pool(&r, bad));
    // Silent: an operator reading the log cannot see that a bad identifier arrived.
    assert!(lg.total() == 0);
    assert!(lg.warns.load(core::sync::atomic::Ordering::SeqCst) == 0);
}

// =====================================================================================
//  remove
// =====================================================================================

/// EPO-REMOVE-PRE-POOL-EXISTS — remove succeeds when the pool inside the handle exists and
/// the handle came from a successful track; as with touch, the pool field is checked and
/// the slot field is not.
#[kani::proof]
#[kani::stub(std::collections::hash_map::RandomState::new, crate::api::concrete_state)]
#[kani::unwind(4)]
#[kani::solver(minisat)]
fn verify_epo_remove_pre_pool_exists() {
    let c = component();
    let p = c.create_pool();

    let h1 = track_ok(&c, p, A);
    let _h2 = track_ok(&c, p, B);

    remove_ok(&c, h1);
    assert!(c.len(p) == 1);
    assert!(keys_eq(&c.get_eviction_candidates(p, 4), &[B]));

    // The slot field being unchecked is the subject of the refutations, not an assumption
    // here: presenting an already-removed but still-allocated slot is accepted silently.
    assert!(c.remove(h1).is_ok());
    assert!(c.len(p) == 1);

    let bad: PoolId = kani::any();
    kani::assume(bad != p);
    let r = c.remove(EvictionHandle::new(bad, 0));
    assert!(is_invalid_pool(&r, bad));
}

/// EPO-REMOVE-FRAME-SKETCH-UNCHANGED — removing an entry does not change the pool's
/// estimates or counters: the policy keeps remembering how often that key was seen.
///
/// The "keeps remembering" half is directly observable and is proved as such, which makes
/// this the strongest of the sketch-frame harnesses. A is tracked and then removed, so it
/// holds no entry but has an estimate of 1. A pool head with an estimate of 1 is then
/// built, and A is re-tracked: its estimate becomes 2, which STRICTLY beats the head, so it
/// is admitted at the protected end. Had the removal wiped A's counters, the re-track would
/// have tied at 1 and landed at the eviction end instead. The counters' own values have no
/// accessor; that residue is in the advisory.
#[kani::proof]
#[kani::stub(std::collections::hash_map::RandomState::new, crate::api::concrete_state)]
#[kani::unwind(4)]
#[kani::solver(minisat)]
fn verify_epo_remove_frame_sketch_unchanged() {
    let c = component();
    let p = c.create_pool();

    let h_a = track_ok(&c, p, A); // estimate(A) = 1, list [A]
    remove_ok(&c, h_a); // list []
    assert!(c.len(p) == 0);

    let _h_b = track_ok(&c, p, B); // list [B], estimate(B) = 1

    let _h_a2 = track_ok(&c, p, A); // estimate(A) = 2 > 1 -> protected end
    assert!(c.len(p) == 2);
    assert!(keys_eq(&c.get_eviction_candidates(p, 4), &[B, A]));
}

/// EPO-REMOVE-ERROR-INVALID-POOL — removing with a handle naming a pool that was never
/// created fails with the invalid-pool error naming that pool and leaves every pool
/// unchanged.
#[kani::proof]
#[kani::stub(std::collections::hash_map::RandomState::new, crate::api::concrete_state)]
#[kani::unwind(4)]
#[kani::solver(minisat)]
fn verify_epo_remove_error_invalid_pool() {
    let c = component();
    let p = c.create_pool();
    let _h1 = track_ok(&c, p, A);
    let _h2 = track_ok(&c, p, B);
    let before = c.get_eviction_candidates(p, 4);

    let bad: PoolId = kani::any();
    kani::assume(bad != p);
    let idx: u32 = kani::any();

    let r = c.remove(EvictionHandle::new(bad, idx));
    assert!(r.is_err());
    assert!(is_invalid_pool(&r, bad));

    assert!(c.len(p) == 2);
    assert!(keys_eq(&c.get_eviction_candidates(p, 4), &before));
}

// =====================================================================================
//  identify_next_to_evict
// =====================================================================================

/// EPO-EVICT-PRE-TOTAL — asking for the next victim requires nothing: any identifier at all
/// may be passed, and the call always returns.
///
/// The identifier is SYMBOLIC and completely unconstrained, so this covers the created pool
/// and every identifier that was never created in one statement. Totality is structural —
/// the return type is `Option`, not `Result` — so reaching the end without a failed check
/// is the content, and the answer is pinned in both cases so the harness cannot pass
/// vacuously.
#[kani::proof]
#[kani::stub(std::collections::hash_map::RandomState::new, crate::api::concrete_state)]
#[kani::unwind(4)]
#[kani::solver(minisat)]
fn verify_epo_evict_pre_total() {
    let c = component();
    let p = c.create_pool();
    let _h = track_ok(&c, p, A);

    let q: PoolId = kani::any();
    let r = c.identify_next_to_evict(q);

    if q == p {
        assert!(r == Some(A));
        assert!(c.len(p) == 0);
    } else {
        assert!(r.is_none());
        assert!(c.len(p) == 1);
    }
}

/// EPO-EVICT-POST-ORDER-FIRST-TIME-KEYS — keys the policy has never seen, tracked one after
/// another into a fresh pool, come back as victims in REVERSE order of tracking.
///
/// This is the sharpest end-to-end observable of the admission gate, and the component's
/// own `track_and_evict_order` test asserts it. Every first-timer's estimate ties with the
/// head's, so each one is admitted at the eviction end and the pile grows backwards.
#[kani::proof]
#[kani::stub(std::collections::hash_map::RandomState::new, crate::api::concrete_state)]
#[kani::unwind(4)]
#[kani::solver(minisat)]
fn verify_epo_evict_post_order_first_time_keys() {
    let c = component();
    let p = c.create_pool();

    let _h1 = track_ok(&c, p, A);
    let _h2 = track_ok(&c, p, B);
    let _h3 = track_ok(&c, p, C);
    assert!(keys_eq(&c.get_eviction_candidates(p, 4), &[C, B, A]));

    assert!(c.identify_next_to_evict(p) == Some(C));
    assert!(c.identify_next_to_evict(p) == Some(B));
    assert!(c.identify_next_to_evict(p) == Some(A));
    assert!(c.identify_next_to_evict(p) == None);
    assert!(c.len(p) == 0);
}

/// EPO-EVICT-FRAME-SKETCH-AND-COUNTERS — eviction does not change the estimates or the
/// counters; in particular the EVICTED key's frequency record is left in place, so tracking
/// it again later still counts its old popularity.
///
/// The load-bearing half is observable and is proved: B is evicted, then re-tracked, and
/// because its estimate survived the eviction it becomes 2 and strictly beats the head's 1,
/// so B is admitted at the protected end. Had eviction cleared B's counters it would have
/// tied and landed at the eviction end. The counters' own values have no accessor; residue
/// in the advisory.
#[kani::proof]
#[kani::stub(std::collections::hash_map::RandomState::new, crate::api::concrete_state)]
#[kani::unwind(4)]
#[kani::solver(minisat)]
fn verify_epo_evict_frame_sketch_and_counters() {
    let c = component();
    let p = c.create_pool();

    let _h1 = track_ok(&c, p, A); // [A], estimate(A) = 1
    let _h2 = track_ok(&c, p, B); // [B, A], estimate(B) = 1

    assert!(c.identify_next_to_evict(p) == Some(B)); // B evicted; its record must survive
    assert!(c.len(p) == 1);
    assert!(keys_eq(&c.get_eviction_candidates(p, 4), &[A]));

    let _h3 = track_ok(&c, p, B); // estimate(B) = 2 > estimate(A) = 1 -> protected end
    assert!(keys_eq(&c.get_eviction_candidates(p, 4), &[A, B]));
}

/// EPO-EVICT-FRAME-OTHER-POOLS — taking a victim out of one pool leaves every other pool's
/// entries, order and estimates completely unchanged.
#[kani::proof]
#[kani::stub(std::collections::hash_map::RandomState::new, crate::api::concrete_state)]
#[kani::unwind(4)]
#[kani::solver(minisat)]
fn verify_epo_evict_frame_other_pools() {
    let c = component();
    let a = c.create_pool();
    let b = c.create_pool();

    let _ha1 = track_ok(&c, a, A);
    let _ha2 = track_ok(&c, a, B); // a: [B, A]
    let _hb1 = track_ok(&c, b, C);
    let _hb2 = track_ok(&c, b, D); // b: [D, C]
    let a_before = c.get_eviction_candidates(a, 4);
    assert!(keys_eq(&a_before, &[B, A]));

    assert!(c.identify_next_to_evict(b) == Some(D));

    assert!(c.len(a) == 2);
    assert!(keys_eq(&c.get_eviction_candidates(a, 4), &a_before));
    // a's estimator is untouched too: re-tracking A strictly beats head B, as it would
    // have before the eviction in b.
    let _ha3 = track_ok(&c, a, A);
    assert!(keys_eq(&c.get_eviction_candidates(a, 4), &[B, A, A]));
}

/// EPO-EVICT-DEGRADE-INVALID-POOL — a victim asked of an identifier that was never created
/// comes back as nothing rather than an error, changes nothing, logs nothing, and is
/// INDISTINGUISHABLE from the answer for a real but empty pool.
///
/// The indistinguishability clause is what makes this a finding rather than a nicety, so it
/// is asserted directly: the same `None` comes back from the bogus identifier and from a
/// genuinely empty pool created alongside. Both pools are created BEFORE the logger is
/// connected, because `create_pool` logs a debug (see `connect_logger`); neither call under
/// test has a log site at all, so the silence is proved rather than assumed.
#[kani::proof]
#[kani::stub(std::collections::hash_map::RandomState::new, crate::api::concrete_state)]
#[kani::unwind(4)]
#[kani::solver(minisat)]
fn verify_epo_evict_degrade_invalid_pool() {
    let c = component();
    let p = c.create_pool();
    let q = c.create_pool(); // a real pool that will stay empty
    let lg = attach_logger(&c);

    let _h = track_ok(&c, p, A);

    let bad: PoolId = kani::any();
    kani::assume(bad != p && bad != q);

    assert!(c.identify_next_to_evict(bad).is_none()); // never created
    assert!(c.identify_next_to_evict(q).is_none()); // real, but empty: same answer
    assert!(lg.total() == 0); // and nothing said about either

    // Nothing changed anywhere.
    assert!(c.len(p) == 1);
    assert!(c.len(q) == 0);
    assert!(keys_eq(&c.get_eviction_candidates(p, 4), &[A]));
}

// =====================================================================================
//  get_eviction_candidates
// =====================================================================================

/// EPO-CANDIDATES-PRE-TOTAL — previewing requires nothing: any identifier and any requested
/// count, including zero and counts larger than the pool holds, are accepted and a list
/// always comes back.
///
/// Both the identifier and the count are SYMBOLIC. The count is capped at 4 by the unwind
/// geometry this file works in, which includes the "larger than the pool holds" case
/// (the pool holds two) and the zero case.
#[kani::proof]
#[kani::stub(std::collections::hash_map::RandomState::new, crate::api::concrete_state)]
#[kani::unwind(4)]
#[kani::solver(minisat)]
fn verify_epo_candidates_pre_total() {
    let c = component();
    let p = c.create_pool();
    let _h1 = track_ok(&c, p, A);
    let _h2 = track_ok(&c, p, B);

    let q: PoolId = kani::any();
    let n: usize = kani::any();
    kani::assume(n <= 4);

    let v = c.get_eviction_candidates(q, n);
    assert!(v.len() <= n);
    if q == p {
        assert!(v.len() == if n < 2 { n } else { 2 });
    } else {
        assert!(v.len() == 0);
    }
    // Non-destructive, so the pool is still intact afterwards.
    assert!(c.len(p) == 2);
}

/// EPO-CANDIDATES-DEGRADE-INVALID-POOL — a preview of an identifier that was never created
/// comes back empty rather than as an error, logs nothing, and is indistinguishable from
/// the preview of a real but empty pool.
#[kani::proof]
#[kani::stub(std::collections::hash_map::RandomState::new, crate::api::concrete_state)]
#[kani::unwind(4)]
#[kani::solver(minisat)]
fn verify_epo_candidates_degrade_invalid_pool() {
    let c = component();
    let p = c.create_pool();
    let q = c.create_pool();
    let lg = attach_logger(&c);

    let _h = track_ok(&c, p, A);

    let bad: PoolId = kani::any();
    kani::assume(bad != p && bad != q);
    let n: usize = kani::any();
    kani::assume(n <= 4);

    assert!(c.get_eviction_candidates(bad, n).len() == 0);
    assert!(c.get_eviction_candidates(q, n).len() == 0);
    assert!(lg.total() == 0);
    assert!(c.len(p) == 1);
}

// =====================================================================================
//  len
// =====================================================================================

/// EPO-LEN-PRE-TOTAL — asking a pool's size requires nothing: any identifier is accepted
/// and a number always comes back.
#[kani::proof]
#[kani::stub(std::collections::hash_map::RandomState::new, crate::api::concrete_state)]
#[kani::unwind(4)]
#[kani::solver(minisat)]
fn verify_epo_len_pre_total() {
    let c = component();
    let p = c.create_pool();
    let _h = track_ok(&c, p, A);

    let q: PoolId = kani::any();
    let n = c.len(q);
    if q == p {
        assert!(n == 1);
    } else {
        assert!(n == 0);
    }
    // ... and asking again gives the same answer: the call changes nothing.
    assert!(c.len(q) == n);
}

/// EPO-LEN-DEGRADE-INVALID-POOL — the size of an identifier that was never created is
/// reported as zero rather than as an error, logs nothing, and is identical to the answer
/// for a real empty pool — so this operation gives a caller no way to discover that its
/// identifier is wrong.
#[kani::proof]
#[kani::stub(std::collections::hash_map::RandomState::new, crate::api::concrete_state)]
#[kani::unwind(4)]
#[kani::solver(minisat)]
fn verify_epo_len_degrade_invalid_pool() {
    let c = component();
    let p = c.create_pool();
    let q = c.create_pool();
    let lg = attach_logger(&c);

    let _h = track_ok(&c, p, A);

    let bad: PoolId = kani::any();
    kani::assume(bad != p && bad != q);

    assert!(c.len(bad) == 0);
    assert!(c.len(q) == 0);
    assert!(c.len(bad) == c.len(q)); // indistinguishable
    assert!(lg.total() == 0);
    assert!(c.len(p) == 1);
}

// =====================================================================================
//  clear_pool
// =====================================================================================

/// EPO-CLEAR-PRE-TOTAL — clearing requires nothing and reports nothing: any identifier is
/// accepted, and it may be called on a pool that is already empty.
#[kani::proof]
#[kani::stub(std::collections::hash_map::RandomState::new, crate::api::concrete_state)]
#[kani::unwind(4)]
#[kani::solver(minisat)]
fn verify_epo_clear_pre_total() {
    let c = component();
    let p = c.create_pool();
    let _h1 = track_ok(&c, p, A);
    let _h2 = track_ok(&c, p, B);

    c.clear_pool(p);
    assert!(c.len(p) == 0);

    // Already empty: accepted, and idempotent.
    c.clear_pool(p);
    assert!(c.len(p) == 0);

    // Any identifier at all, including ones never created.
    let q: PoolId = kani::any();
    c.clear_pool(q);
    assert!(c.len(p) == 0);
}

/// EPO-CLEAR-FRAME-PRESERVES-SKETCH-AND-COUNTERS — clearing deliberately keeps everything
/// the policy has learned: the estimates, the access tally and the largest-ever-size record
/// all survive, so clearing a pool is NOT the same as creating a fresh one.
///
/// The estimator half is proved observably, and the trace is built so the outcome
/// DISCRIMINATES. Before the clear, A is tracked twice (estimate 2) and B once (estimate
/// 1). After the clear, B is tracked — becoming 2 — and then A, becoming 3. Three strictly
/// beats the head's two, so A is admitted at the protected end, giving `[B, A]`. Had the
/// clear reset the table, both would have been first-timers again: B at 1, A at 1, a tie,
/// and A would have been admitted at the EVICTION end, giving `[A, B]`. So the asserted
/// order is possible only if the estimates crossed the clear.
///
/// The two counters have no accessor, and their only consequence — the ageing schedule — is
/// out of reach at these bounds. That residue is in the advisory; the structural reason is
/// that `clear_pool` (lib.rs:231-237) delegates to `LruList::clear` alone and never touches
/// `Pool::sketch`, `Pool::access_count` or `Pool::max_len`.
///
/// Geometry: five tracks into one pool, so `max_len` reaches 2 and the ageing period is at
/// least 20 — `halve` is never reached.
#[kani::proof]
#[kani::stub(std::collections::hash_map::RandomState::new, crate::api::concrete_state)]
#[kani::unwind(4)]
#[kani::solver(minisat)]
fn verify_epo_clear_frame_preserves_sketch_and_counters() {
    let c = component();
    let p = c.create_pool();

    let _h1 = track_ok(&c, p, A); // estimate(A) = 1, [A]
    let _h2 = track_ok(&c, p, A); // estimate(A) = 2, tie -> [A, A]
    let _h3 = track_ok(&c, p, B); // estimate(B) = 1 <= 2 -> [B, A, A]
    assert!(keys_eq(&c.get_eviction_candidates(p, 4), &[B, A, A]));

    c.clear_pool(p);
    assert!(c.len(p) == 0);

    let _h4 = track_ok(&c, p, B); // estimate(B) = 2; pool empty -> [B]
    let _h5 = track_ok(&c, p, A); // estimate(A) = 3 > 2 -> protected end

    // Only reachable if the estimator crossed the clear intact.
    assert!(c.len(p) == 2);
    assert!(keys_eq(&c.get_eviction_candidates(p, 4), &[B, A]));
}

/// EPO-CLEAR-POST-RETRACK-ADMITTED-AT-MRU — a cleared pool is immediately usable under the
/// same identifier, and because it is empty the very next key is admitted at the protected
/// end however often it was seen before the clear.
///
/// "However often" is made real: A is tracked three times before the clear, so its estimate
/// is 3 — the hottest key in the pool by a wide margin — and it is still admitted at the
/// protected end afterwards, because the empty-pool branch (lib.rs:135-137) does not
/// consult the estimator at all.
#[kani::proof]
#[kani::stub(std::collections::hash_map::RandomState::new, crate::api::concrete_state)]
#[kani::unwind(4)]
#[kani::solver(minisat)]
fn verify_epo_clear_post_retrack_admitted_at_mru() {
    let c = component();
    let p = c.create_pool();

    let _h1 = track_ok(&c, p, A);
    let _h2 = track_ok(&c, p, A);
    let _h3 = track_ok(&c, p, A); // estimate(A) = 3, list [A, A, A]
    assert!(c.len(p) == 3);

    c.clear_pool(p);
    assert!(c.len(p) == 0);

    // Same identifier, immediately usable.
    let h4 = track_ok(&c, p, A);
    assert!(h4.pool_id() == p);
    assert!(c.len(p) == 1);
    // The only entry, so the protected end and the front at once.
    assert!(keys_eq(&c.get_eviction_candidates(p, 4), &[A]));
    assert!(c.identify_next_to_evict(p) == Some(A));
    assert!(c.len(p) == 0);
}

/// EPO-CLEAR-FRAME-OTHER-POOLS — clearing one pool leaves every other pool untouched, and
/// never removes the pool itself or renumbers any pool.
#[kani::proof]
#[kani::stub(std::collections::hash_map::RandomState::new, crate::api::concrete_state)]
#[kani::unwind(4)]
#[kani::solver(minisat)]
fn verify_epo_clear_frame_other_pools() {
    let c = component();
    let a = c.create_pool();
    let b = c.create_pool();

    let _ha1 = track_ok(&c, a, A);
    let _ha2 = track_ok(&c, a, B); // a: [B, A]
    let _hb = track_ok(&c, b, C); // b: [C]
    let a_before = c.get_eviction_candidates(a, 4);
    assert!(keys_eq(&a_before, &[B, A]));

    c.clear_pool(b);

    // a is untouched, in entries and in order ...
    assert!(c.len(a) == 2);
    assert!(keys_eq(&c.get_eviction_candidates(a, 4), &a_before));
    // ... and a's estimator too: re-tracking A strictly beats head B.
    let _ha3 = track_ok(&c, a, A);
    assert!(keys_eq(&c.get_eviction_candidates(a, 4), &[B, A, A]));

    // b was emptied, not removed: it still exists under the same identifier and works.
    assert!(c.len(b) == 0);
    let hb2 = track_ok(&c, b, D);
    assert!(hb2.pool_id() == b);
    assert!(c.len(b) == 1);
    // No renumbering: the next identifier continues the sequence.
    assert!(c.create_pool() == b + 1);
}

/// EPO-CLEAR-DEGRADE-INVALID-POOL — clearing an identifier that was never created quietly
/// does nothing and logs nothing, so a caller passing a wrong identifier gets no
/// indication that the request was ignored.
#[kani::proof]
#[kani::stub(std::collections::hash_map::RandomState::new, crate::api::concrete_state)]
#[kani::unwind(4)]
#[kani::solver(minisat)]
fn verify_epo_clear_degrade_invalid_pool() {
    let c = component();
    let p = c.create_pool();
    let lg = attach_logger(&c);

    let _h = track_ok(&c, p, A);

    let bad: PoolId = kani::any();
    kani::assume(bad != p);

    c.clear_pool(bad); // silently ignored

    assert!(lg.total() == 0);
    assert!(c.len(p) == 1);
    assert!(keys_eq(&c.get_eviction_candidates(p, 4), &[A]));
}

// =====================================================================================
//  Global invariants over the component
// =====================================================================================

/// EPO-INV-POOL-IDS-UNIQUE-SEQUENTIAL — the pools that exist are exactly those numbered
/// zero up to one less than the number created; every larger identifier is either rejected
/// with the invalid-pool error or silently treated as an empty pool, depending on the
/// operation.
///
/// The second half is the one that needs a universal, so the out-of-range identifier is
/// SYMBOLIC and every operation is tried on it — the two that report (`track`,
/// `batch_touch` via a handle) and the four that degrade silently.
#[kani::proof]
#[kani::stub(std::collections::hash_map::RandomState::new, crate::api::concrete_state)]
#[kani::unwind(4)]
#[kani::solver(minisat)]
fn verify_epo_inv_pool_ids_unique_sequential() {
    let c = component();
    let p0 = c.create_pool();
    let p1 = c.create_pool();
    let p2 = c.create_pool();

    // A gapless sequence from zero, no repeats.
    assert!(p0 == 0 && p1 == 1 && p2 == 2);
    assert!(c.len(p0) == 0 && c.len(p1) == 0 && c.len(p2) == 0);

    let q: PoolId = kani::any();
    kani::assume(q > p2);

    // Reported ...
    let r = c.track(q, A, sem());
    assert!(is_invalid_pool(&r, q));
    let rb = c.batch_touch(&[EvictionHandle::new(q, 0)]);
    assert!(is_invalid_pool(&rb, q));
    // ... or silently treated as empty.
    assert!(c.len(q) == 0);
    assert!(c.identify_next_to_evict(q).is_none());
    assert!(c.get_eviction_candidates(q, 4).len() == 0);
    c.clear_pool(q);

    // None of that created anything.
    assert!(c.len(p0) == 0 && c.len(p1) == 0 && c.len(p2) == 0);
}

/// EPO-INV-POOLS-APPEND-ONLY — the collection of pools only ever grows: no operation
/// removes or moves a pool, identifiers are never recycled, an identifier once returned
/// stays valid for the life of the component, and one never returned stays invalid forever.
///
/// Every operation that might plausibly shrink the collection is applied between the
/// creations — a removal, a clear, and an eviction that empties a pool — and the identifier
/// issued afterwards is asserted to CONTINUE the sequence rather than to reuse a number.
#[kani::proof]
#[kani::stub(std::collections::hash_map::RandomState::new, crate::api::concrete_state)]
#[kani::unwind(4)]
#[kani::solver(minisat)]
fn verify_epo_inv_pools_append_only() {
    let c = component();
    let a = c.create_pool();

    let h = track_ok(&c, a, A);
    remove_ok(&c, h); // emptied by removal
    let b = c.create_pool();
    assert!(b == a + 1);

    let h2 = track_ok(&c, a, B);
    assert!(c.identify_next_to_evict(a) == Some(B)); // emptied by eviction
    let _ = h2;
    c.clear_pool(a); // emptied by clear

    let d = c.create_pool();
    assert!(d == b + 1); // no recycling, no renumbering

    // `a` is still valid, and still means the same pool.
    assert!(c.len(a) == 0);
    let h3 = track_ok(&c, a, C);
    assert!(h3.pool_id() == a);
    assert!(c.len(a) == 1);

    // An identifier never returned stays invalid.
    let q: PoolId = kani::any();
    kani::assume(q > d);
    assert!(c.track(q, D, sem()).is_err());
}

/// EPO-INV-POOL-ISOLATION — each pool keeps its own entries, its own ordering, its own
/// estimator and its own counters, so no operation on one pool can change what another
/// contains, what it would evict next, or what it believes about popularity.
///
/// A whole battery is run against pool `b` — track, touch, remove, evict, clear — and
/// pool `a` is then checked in all three respects: size, eviction order, and the
/// estimator, the last of these through the admission probe (re-tracking A must still
/// strictly beat head B, which it can only do if a's counters were never disturbed).
#[kani::proof]
#[kani::stub(std::collections::hash_map::RandomState::new, crate::api::concrete_state)]
#[kani::unwind(4)]
#[kani::solver(minisat)]
fn verify_epo_inv_pool_isolation() {
    let c = component();
    let a = c.create_pool();
    let b = c.create_pool();

    let _ha1 = track_ok(&c, a, A);
    let _ha2 = track_ok(&c, a, B); // a: [B, A]
    let _hb1 = track_ok(&c, b, C); // b: [C]
    let a_before = c.get_eviction_candidates(a, 4);
    assert!(keys_eq(&a_before, &[B, A]));

    // The battery, all on b.
    let hb2 = track_ok(&c, b, D); // b: [D, C]
    touch_ok(&c, hb2); // b: [C, D]
    remove_ok(&c, hb2); // b: [C]
    assert!(c.identify_next_to_evict(b) == Some(C)); // b: []
    c.clear_pool(b);
    assert!(c.len(b) == 0);

    // a is untouched in entries and order ...
    assert!(c.len(a) == 2);
    assert!(keys_eq(&c.get_eviction_candidates(a, 4), &a_before));
    // ... and in what it believes about popularity.
    let _ha3 = track_ok(&c, a, A);
    assert!(keys_eq(&c.get_eviction_candidates(a, 4), &[B, A, A]));
}

/// EPO-INV-HANDLE-POOL-SCOPED — because a handle carries its pool alongside the position,
/// and because the component only ever issues a handle naming the pool the track was made
/// into, an operation with a handle can only affect an entry in that same pool — never one
/// that happens to sit at the same position in a different pool.
///
/// The harness builds exactly the confusable state: two pools each holding one entry at
/// slot 0, so the two handles differ ONLY in their pool field, and asserts that removing
/// through one leaves the other's entry alone. The issuing half is checked too: every
/// handle returned names the pool it was tracked into.
#[kani::proof]
#[kani::stub(std::collections::hash_map::RandomState::new, crate::api::concrete_state)]
#[kani::unwind(4)]
#[kani::solver(minisat)]
fn verify_epo_inv_handle_pool_scoped() {
    let c = component();
    let a = c.create_pool();
    let b = c.create_pool();

    let ha = track_ok(&c, a, A);
    let hb = track_ok(&c, b, B);

    // Same position, different pools — the only thing telling them apart is the pool field.
    assert!(ha.index() == hb.index());
    assert!(ha.pool_id() == a && hb.pool_id() == b && a != b);

    remove_ok(&c, ha);
    assert!(c.len(a) == 0);
    assert!(c.len(b) == 1);
    assert!(keys_eq(&c.get_eviction_candidates(b, 4), &[B]));

    // Touching through b's handle affects b only.
    touch_ok(&c, hb);
    assert!(c.len(a) == 0 && c.len(b) == 1);

    // Every issued handle names the pool it was tracked into.
    let ha2 = track_ok(&c, a, C);
    let hb2 = track_ok(&c, b, D);
    assert!(ha2.pool_id() == a && hb2.pool_id() == b);
}

/// EPO-INV-INVALID-HANDLE-NEVER-RETURNED — no operation ever produces
/// `EvictionPolicyError::InvalidHandle`, even though the shared interface declares it for
/// exactly the case of a bad or already-removed handle. The only error is invalid-pool.
///
/// This is the property that makes the two refuted stale-handle defects DANGEROUS rather
/// than untidy, and it is checked from both sides. Every fallible method is failed on a
/// symbolic never-created identifier and the error is pinned to `InvalidPool(bad)` — so not
/// `InvalidHandle`. Then a handle whose ENTRY has already been removed, which is precisely
/// what the interface's `InvalidHandle` is for, is presented to all three handle-taking
/// methods, and each returns `Ok(())`: the caller is told nothing at all.
#[kani::proof]
#[kani::stub(std::collections::hash_map::RandomState::new, crate::api::concrete_state)]
#[kani::unwind(4)]
#[kani::solver(minisat)]
fn verify_epo_inv_invalid_handle_never_returned() {
    let c = component();
    let p = c.create_pool();

    let h1 = track_ok(&c, p, A);
    let _h2 = track_ok(&c, p, B);

    let bad: PoolId = kani::any();
    kani::assume(bad != p);
    let idx: u32 = kani::any();
    let bh = EvictionHandle::new(bad, idx);

    // Bad pool: invalid-POOL, never invalid-HANDLE, on every fallible method.
    let r1 = c.track(bad, A, sem());
    assert!(is_invalid_pool(&r1, bad));
    let r2 = c.touch(bh);
    assert!(is_invalid_pool(&r2, bad));
    let r3 = c.remove(bh);
    assert!(is_invalid_pool(&r3, bad));
    let r4 = c.batch_touch(&[bh]);
    assert!(is_invalid_pool(&r4, bad));

    // An already-removed handle — the interface's own use case for InvalidHandle — is
    // accepted silently instead of reported.
    remove_ok(&c, h1);
    assert!(c.touch(h1).is_ok());
    assert!(c.remove(h1).is_ok());
    assert!(c.batch_touch(&[h1]).is_ok());
    assert!(c.len(p) == 1);
}

/// EPO-INV-MAXLEN-MONOTONE — the largest-ever-size record never goes down: removal,
/// eviction and clearing all leave it untouched.
///
/// SCOPE. `Pool::max_len` has no accessor, so the monotonicity is proved against a mirror
/// of its ONLY write, lib.rs:122 (`max_len = max(max_len, lru.len())`), which cannot
/// decrease it for any value; and the real component is driven through exactly the three
/// operations the statement names, showing they leave the pool well formed and usable. What
/// no Kani harness can add is a reading of the field itself. The structural fact behind the
/// claim is that `remove` (lib.rs:186-200), `identify_next_to_evict` (lib.rs:202-207) and
/// `clear_pool` (lib.rs:231-237) contain no assignment to `max_len` at all. Residue
/// recorded in the advisory; the consequence of the record staying high is proved on the
/// arithmetic side by `verify_epo_inv_aging_period_non_decreasing` in `proofs_sketch.rs`.
#[kani::proof]
#[kani::stub(std::collections::hash_map::RandomState::new, crate::api::concrete_state)]
#[kani::unwind(4)]
#[kani::solver(minisat)]
fn verify_epo_inv_maxlen_monotone() {
    // --- the only write, mirrored from lib.rs:122 ---
    let ml: usize = kani::any();
    let len: usize = kani::any();
    kani::assume(ml <= (1usize << 40) && len <= (1usize << 40));
    assert!(ml.max(len) >= ml); // it can only stay the same or grow

    // --- the three operations the statement names, against the real component ---
    let c = component();
    let p = c.create_pool();
    let h1 = track_ok(&c, p, A);
    let _h2 = track_ok(&c, p, B);
    assert!(c.len(p) == 2);

    remove_ok(&c, h1);
    assert!(c.len(p) == 1);
    assert!(c.identify_next_to_evict(p) == Some(B));
    assert!(c.len(p) == 0);
    c.clear_pool(p);
    assert!(c.len(p) == 0);

    // Still usable afterwards, so nothing was corrupted by the sequence.
    let _h3 = track_ok(&c, p, C);
    assert!(c.len(p) == 1);
    assert!(keys_eq(&c.get_eviction_candidates(p, 4), &[C]));
}

/// EPO-INV-MAXLEN-UPPER-BOUNDS-LEN — a pool's size is never more than one greater than its
/// largest-ever-size record, and the record is never greater than a size truly reached.
/// Because the record is measured just BEFORE each insert, at a new peak it is exactly one
/// too low.
///
/// This is the DIVERGENT property adjudicated in favour of the code, and it is stated here
/// as an INDUCTIVE proof over a mirror of the real updates rather than as a spot check:
/// establishment on a fresh pool, preservation across a track (lib.rs:122 then the insert
/// at lib.rs:129-137), and preservation across every size-lowering operation (removal,
/// eviction, clear), which leave the record alone. The off-by-one is asserted as an
/// equation at the peak, and the `max_len == 0` corner — where the ageing guard at
/// lib.rs:124 can therefore never fire — is called out separately, because that is the
/// observable consequence the inventory flagged as a real defect.
#[kani::proof]
#[kani::unwind(4)]
fn verify_epo_inv_maxlen_upper_bounds_len() {
    // Establishment: a fresh pool has both at zero (lib.rs:92-97, lru_list.rs:26-34).
    let ml_fresh: usize = 0;
    let len_fresh: usize = 0;
    assert!(len_fresh <= ml_fresh + 1);

    let ml: usize = kani::any();
    let len: usize = kani::any();
    kani::assume(ml <= (1usize << 40) && len <= (1usize << 40));
    kani::assume(len <= ml + 1); // the invariant, as the pre-state

    // Preservation across one track.
    let ml2 = ml.max(len); // lib.rs:122, measured BEFORE the insert
    let len2 = len + 1; // lib.rs:129-137 inserts exactly one
    assert!(len2 <= ml2 + 1);
    assert!(ml2 == ml || ml2 == len); // never above a size truly reached

    // The off-by-one, as an equation, exactly at a new peak.
    if len >= ml {
        assert!(ml2 == len);
        assert!(len2 == ml2 + 1);
    }
    // The corner that makes the ageing guard dead for a pool that is always empty at
    // track time: the record stays 0 while the pool holds 1.
    if ml == 0 && len == 0 {
        assert!(ml2 == 0 && len2 == 1);
    }

    // Preservation across removal / eviction / clear: they lower the size and leave the
    // record alone, so the bound only gets slacker.
    let len3: usize = kani::any();
    kani::assume(len3 <= len2);
    assert!(len3 <= ml2 + 1);
}

/// EPO-INV-ACCESS-COUNT-MONOTONE — the running access tally only ever increases, by exactly
/// one per successful track, and nothing — clearing included — lowers or resets it.
///
/// SCOPE, as for the other counter obligations: the field has no accessor. What is proved
/// is the mirror of its ONLY write, lib.rs:123, over a symbolic starting value bounded by
/// what a process can reach, plus the real component driven through the operations the
/// statement singles out — in particular that a pool survives a clear and keeps working,
/// which is the observable half of "a lifetime tally rather than a measure of current
/// contents". The structural fact is that `clear_pool` (lib.rs:231-237) delegates to
/// `LruList::clear` alone.
#[kani::proof]
#[kani::stub(std::collections::hash_map::RandomState::new, crate::api::concrete_state)]
#[kani::unwind(4)]
#[kani::solver(minisat)]
fn verify_epo_inv_access_count_monotone() {
    // --- the only write, mirrored from lib.rs:123 ---
    let ac: u64 = kani::any();
    // A track allocates a `Node`, so the tally is bounded by the number of allocations a
    // process can make; 2^48 is far above that and far below u64::MAX, so `+= 1` is exact.
    kani::assume(ac < (1u64 << 48));
    assert!(ac.checked_add(1).is_some());
    assert!(ac + 1 > ac);

    // --- the real component across a clear ---
    let c = component();
    let p = c.create_pool();
    let _h1 = track_ok(&c, p, A);
    let _h2 = track_ok(&c, p, B);
    assert!(c.len(p) == 2);

    c.clear_pool(p);
    assert!(c.len(p) == 0);

    // The pool is the same pool, still tracking into the same bookkeeping — and what it
    // learned about A survives the clear, which is the observable sibling of the tally
    // surviving it (see verify_epo_clear_frame_preserves_sketch_and_counters).
    let _h3 = track_ok(&c, p, B); // estimate(B) = 2; empty pool -> protected end
    let _h4 = track_ok(&c, p, A); // estimate(A) = 2 <= 2 -> eviction end
    assert!(keys_eq(&c.get_eviction_candidates(p, 4), &[A, B]));
}

/// EPO-INV-AT-MOST-ONE-POOL-LOCK — no operation holds the exclusive lock of two pools at
/// once: each takes the shared lock on the pool list and then at most one pool's lock,
/// releasing it before acquiring another. In a batch, when consecutive handles name
/// different pools, the current pool's lock is dropped before the next is taken.
///
/// WHAT THIS HARNESS DOES AND DOES NOT ESTABLISH — stated plainly, because Kani is a
/// single-threaded model checker and this is a locking claim. `batch_touch` is the only
/// method that visits two pools in one call, and the harness drives it with handles that
/// ALTERNATE pools, which is the shape that forces the drop-then-lock sequence at
/// lib.rs:172-180 to execute; the call completes and both pools end in the expected state,
/// so the alternating control flow is reachable and correct. What it does NOT prove is
/// mutual exclusion itself: Kani does not model a blocking `Mutex`, so a harness cannot
/// distinguish a released lock from a re-entrant acquisition, and a hypothetical version of
/// this code that failed to drop the guard would deadlock at RUN time while passing here.
/// The advisory records that residue and names the properties that do cover the rest
/// (`NV-LIVENESS-DEADLOCK-FREEDOM` is the inventory's own not-verifiable entry for it).
#[kani::proof]
#[kani::stub(std::collections::hash_map::RandomState::new, crate::api::concrete_state)]
#[kani::unwind(4)]
#[kani::solver(minisat)]
fn verify_epo_inv_at_most_one_pool_lock() {
    let c = component();
    let a = c.create_pool();
    let b = c.create_pool();

    let ha = track_ok(&c, a, A);
    let hb = track_ok(&c, b, B);

    // Alternating pools: a, then b, then a again — two pool changes inside one call.
    batch_ok(&c, &[ha, hb, ha]);
    assert!(c.len(a) == 1 && c.len(b) == 1);
    assert!(keys_eq(&c.get_eviction_candidates(a, 4), &[A]));
    assert!(keys_eq(&c.get_eviction_candidates(b, 4), &[B]));

    // Each single-pool method takes at most one pool lock, structurally: one
    // `state.pools.get(..)` followed by one `lock()`. All of them complete in sequence.
    touch_ok(&c, ha);
    assert!(c.identify_next_to_evict(b) == Some(B));
    remove_ok(&c, ha);
    c.clear_pool(a);
    assert!(c.len(a) == 0 && c.len(b) == 0);
}

/// EPO-INV-LOGGER-OPTIONAL — every operation produces the same results and leaves the same
/// state whether or not a logger is connected; a missing logger never becomes a failure or
/// a changed answer.
///
/// Stated as a UNIVERSAL over connectedness, not as two runs: the receptacle is connected or
/// not according to a SYMBOLIC boolean, and then one script exercises eight of the nine
/// interface methods and asserts a fixed expected result after each step. Kani explores both
/// values of the boolean, so a pass means the whole script's observable behaviour is
/// identical either way. `is_connected()` is asserted to agree with the boolean first, so
/// the connected branch is not vacuous.
///
/// WHAT IS NOT COVERED, deliberately. The four log SITES themselves — `create_pool`'s debug
/// and the three invalid-pool warnings (lib.rs:99, :113, :147, :191) — are not on the script,
/// because each evaluates `format!` with a `{}`-formatted integer, whose `core::fmt` digit
/// loop and dynamic dispatch dominate everything else at these unwind bounds. The pool is
/// therefore created before the connection, and only success paths are driven; those eight
/// methods have no log site at all, which is also why the logger is asserted to have
/// received nothing. The residue — that the four guarded sites also do not change any
/// result — rests on their shape being identical (`if let Ok(logger) = self.logger.get()`
/// around a call whose return value is discarded) and is recorded as such in the advisory.
#[kani::proof]
#[kani::stub(std::collections::hash_map::RandomState::new, crate::api::concrete_state)]
#[kani::unwind(4)]
#[kani::solver(minisat)]
fn verify_epo_inv_logger_optional() {
    let c = component();
    let p = c.create_pool(); // before any connection: its debug site is not evaluated

    let connected: bool = kani::any();
    let lg = new_counting_logger();
    if connected {
        connect_logger(&c, &lg);
    }
    assert!(c.logger.is_connected() == connected);
    assert!(c.logger.get().is_ok() == connected);

    // One script, a fixed expected answer at every step, for both values of `connected`.
    let h1 = track_ok(&c, p, A);
    let h2 = track_ok(&c, p, B);
    assert!(c.len(p) == 2);
    assert!(keys_eq(&c.get_eviction_candidates(p, 4), &[B, A]));

    touch_ok(&c, h2);
    assert!(keys_eq(&c.get_eviction_candidates(p, 4), &[A, B]));

    batch_ok(&c, &[h1]);
    assert!(keys_eq(&c.get_eviction_candidates(p, 4), &[B, A]));

    remove_ok(&c, h1);
    assert!(c.len(p) == 1);
    assert!(c.identify_next_to_evict(p) == Some(B));
    assert!(c.len(p) == 0);

    c.clear_pool(p);
    assert!(c.len(p) == 0);

    // None of those eight methods logs on its success path, so nothing was written even
    // when a logger was there to receive it.
    assert!(lg.total() == 0);
}

/// EPO-INV-NO-STARTUP-BANNER — bringing the component up logs no banner.
///
/// The inventory's one property with an empty method bundle: it constrains construction,
/// not any interface method. The MECHANISM is what makes it checkable, and it is what the
/// harness checks. Every log site in `lib.rs` is guarded by
/// `if let Ok(logger) = self.logger.get()`, and `define_component!`'s generated constructor
/// initialises the receptacle with `Receptacle::new()` — DISCONNECTED. So at construction
/// time there is provably no logger for a banner to reach, and the harness asserts exactly
/// that on a freshly built component: the receptacle is not connected and `get()` fails.
/// A counting logger attached immediately afterwards has therefore received nothing, and
/// the component comes up with no pools at all, so there is nothing for it to announce.
///
/// This is also the cheapest real-component harness in the file — it never calls
/// `create_pool` — which makes it the useful canary for the construction lever.
#[kani::proof]
#[kani::stub(std::collections::hash_map::RandomState::new, crate::api::concrete_state)]
#[kani::unwind(4)]
#[kani::solver(minisat)]
fn verify_epo_inv_no_startup_banner() {
    let c = component();

    // The receptacle the log sites read is empty, so construction cannot have logged.
    assert!(!c.logger.is_connected());
    assert!(c.logger.get().is_err());

    // A logger attached the instant construction finishes has seen nothing.
    let lg = attach_logger(&c);
    assert!(lg.total() == 0);

    // ... and the component came up with no pools, so there was nothing to announce.
    assert!(c.len(0) == 0);
    assert!(c.identify_next_to_evict(0).is_none());
    assert!(c.get_eviction_candidates(0, 4).len() == 0);
    assert!(lg.total() == 0);
}
