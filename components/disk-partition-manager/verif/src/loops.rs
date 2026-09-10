//! Whole-`Vec` loop proofs that close the two obligations the baseline run left
//! as "authorable but not done" (⧗):
//!
//! 1. `DPM-FORMAT-ERR-MULTI-REST` (FR-005) — counting `size_bytes == 0` specs
//!    over `config.partitions: Vec<PartitionSpec>` and rejecting more than one.
//!    The baseline noted the *count* needs a `Seq`-fold model; this module
//!    authors it: a recursive `#[logic]` count function plus an index-free
//!    `for`-loop whose invariant ties the running counter to that logic count,
//!    then the trivial guard on top.
//!    Source: `compute_partition_layout` (gpt.rs:241-250).
//!
//! 2. Full N-partition induction for `DPM-FORMAT-PART-NONOVERLAP` (US1-AS1) and
//!    `DPM-FORMAT-PART-WITHIN-USABLE` (FR-006). The baseline proved only the
//!    inductive *step* (`place_step`) + a two-partition instance. This module
//!    proves the invariants over the **whole** partition sequence, using the
//!    source's own `remaining` budget (gpt.rs:238, :276-281, :295-296): across
//!    all placed partitions the cursor strictly increases (⇒ starts strictly
//!    increasing ⇒ pairwise disjoint) and every partition lies within
//!    `[first_usable, last_usable]`.
//!
//! `u64` stands for each spec's `size_bytes` / a partition's sector count — the
//! only field the layout arithmetic reasons about.

use creusot_std::prelude::*;
use creusot_std::logic::Seq;

// ---------------------------------------------------------------------------
// (1) DPM-FORMAT-ERR-MULTI-REST — count size_bytes==0 specs, reject > 1.
// ---------------------------------------------------------------------------

/// Number of `size_bytes == 0` ("rest of disk") entries in `seq[from..to]`.
/// Right-recursive on `to` so that extending the range by one at the right is a
/// single definitional unfold — exactly the shape the counting loop needs.
#[logic(open)]
#[variant(to - from)]
#[requires(0 <= from && from <= to && to <= seq.len())]
#[ensures(result >= 0)]
#[ensures(result <= to - from)]
pub fn count_zeros(seq: Seq<u64>, from: Int, to: Int) -> Int {
    pearlite! {
        if to - from > 0 {
            count_zeros(seq, from, to - 1) + (if seq[to - 1]@ == 0 { 1int } else { 0int })
        } else {
            0
        }
    }
}

/// Mirror of the rest-partition tally (gpt.rs:241-245):
/// `config.partitions.iter().filter(|p| p.size_bytes == 0).count()`.
/// Proves the returned count equals the logic-level `count_zeros` over the whole
/// sequence — i.e. the `filter…count` is faithful.
#[requires(specs@.len() <= u64::MAX@)]
#[ensures(result@ == count_zeros(specs@, 0, specs@.len()))]
pub fn count_rest(specs: &[u64]) -> u64 {
    let mut count: u64 = 0;
    #[invariant(count@ == count_zeros(specs@, 0, produced.len()))]
    #[invariant(count@ <= produced.len())]
    for &x in specs {
        if x == 0 {
            count += 1;
        }
    }
    count
}

/// Mirror of the multi-rest guard (gpt.rs:246-250):
/// `if rest_count > 1 { return LayoutError }`. Returns `true` on the `Ok` path
/// (at most one rest partition). `DPM-FORMAT-ERR-MULTI-REST`: composed with
/// `count_rest`, `false` ⟺ two-or-more `size_bytes == 0` specs ⟺ `LayoutError`.
#[ensures(result == (rest_count@ <= 1))]
pub fn multi_rest_ok(rest_count: u64) -> bool {
    !(rest_count > 1)
}

/// End-to-end composition: given the spec list, decide whether the layout may
/// proceed. `true` ⟺ at most one rest partition (`count_zeros <= 1`). This is the
/// observable `DPM-FORMAT-ERR-MULTI-REST` decision, proved over the whole `Vec`.
#[requires(specs@.len() <= u64::MAX@)]
#[ensures(result == (count_zeros(specs@, 0, specs@.len()) <= 1))]
pub fn multi_rest_layout_ok(specs: &[u64]) -> bool {
    let rest_count = count_rest(specs);
    multi_rest_ok(rest_count)
}

// ---------------------------------------------------------------------------
// (2) Whole-Vec placement: NONOVERLAP + WITHIN-USABLE across all N partitions.
// ---------------------------------------------------------------------------

/// Faithful mirror of the placement loop over the *whole* partition sequence
/// (gpt.rs:269-297): starting at `first_usable` with a `remaining` budget equal
/// to the usable window, each partition of sector count `sizes[k]` is placed at
/// `current_lba`, ending at `current_lba + n - 1`, after which
/// `current_lba += n` and `remaining -= n`; a partition that does not fit the
/// remaining budget returns `LayoutError` (`Err(())`) — the per-placement
/// oversubscription guard (gpt.rs:276-281).
///
/// Proves, as loop invariants maintained across **all** iterations:
/// - the cursor never falls below `first_usable` and advances by **at least one
///   per placed partition** (`current >= first_usable + placed`) ⇒ partition
///   starts are **strictly increasing** ⇒ the placed partitions are pairwise
///   **disjoint** (`DPM-FORMAT-PART-NONOVERLAP`, whole sequence);
/// - `current + remaining == first_usable + total` and `remaining >= 0`, so the
///   cursor stays `<= first_usable + total == last_usable + 1`; hence every
///   placed partition ends at `current-1 <= last_usable`, i.e. lies within
///   `[first_usable, last_usable]` (`DPM-FORMAT-PART-WITHIN-USABLE`, whole
///   sequence).
///
/// The precondition `sizes[k] >= 1` is the "every placed partition covers at
/// least one sector" fact (fixed partitions get `ceil(size/sector) >= 1`, the
/// rest partition gets what remains, > 0 on the `Ok` path).
#[requires(forall<k> 0 <= k && k < sizes@.len() ==> sizes@[k]@ >= 1)]
#[requires(last_usable@ < u64::MAX@)]
#[requires(first_usable@ <= last_usable@ + 1)]
#[requires(total@ == last_usable@ - first_usable@ + 1)] // remaining budget = usable window
#[ensures(forall<r: u64> result == Ok(r) ==> r@ >= first_usable@ + sizes@.len())] // >=1 per partition (strict increase)
#[ensures(forall<r: u64> result == Ok(r) ==> r@ <= last_usable@ + 1)] // stays within the window
pub fn place_all(sizes: &[u64], first_usable: u64, last_usable: u64, total: u64) -> Result<u64, ()> {
    let mut current = first_usable;
    let mut remaining = total;
    #[invariant(current@ >= first_usable@ + produced.len())]
    #[invariant(current@ + remaining@ == first_usable@ + total@)]
    #[invariant(current@ <= first_usable@ + total@)]
    for &n in sizes {
        if n > remaining {
            return Err(()); // oversubscribed: this partition does not fit
        }
        // Place [current, current + n - 1]; successor starts at current + n.
        current = current + n;
        remaining = remaining - n;
    }
    Ok(current)
}
