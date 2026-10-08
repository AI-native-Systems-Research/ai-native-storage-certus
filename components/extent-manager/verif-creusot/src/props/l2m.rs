//! Level-2 EM-BUDDY-MERGE, re-proved IN GENERAL under the level-1 assumption
//! D-RANGE-FR-002-217ace (`p2(sector_size)`, the std `u32::is_power_of_two`).
//!
//! "When a freed block's buddy is also free, the two are merged, so freeing all blocks of a
//! region makes the largest original blocks available again."
//!
//! History model: one `BuddyAllocator` built by `new` (buddy.rs:10-46, as format() does) and
//! ANY sequence of calls `alloc(size)` / `free(block)` (buddy.rs:57-115), where `free` is
//! only given a block that an earlier `alloc` returned and that is not yet freed (`lv` = the
//! live blocks, each `(offset, span)`). The history invariant `bh` is: the buddy invariant,
//! disjoint free blocks, COALESCED free blocks (`coal`: no two free buddies of the same
//! order below the top), live blocks disjoint from each other and from every free block,
//! and every usable byte free or live. When every block is freed again the free blocks
//! cover every aligned block exactly as after `new` (`inblk` for every aligned block that
//! fits, the `cov` that `new` guarantees), and the largest original block `[0, span(max))`
//! is again a single free block — the only one on the top list.
use crate::model::buddy::*;
use crate::model::l2::*;
use crate::model::l2m::*;
use crate::props::l2a::lemma_alloc_disj;
use crate::model::region::*;
use crate::model::params::*;
use crate::model::assume::*;
use creusot_std::prelude::*;

/// One buddy call: `alloc(size)`, or `free` of live block number `i`.
#[derive(::core::clone::Clone, ::core::marker::Copy)]
pub enum BOp {
    Alloc(u64),
    Free(usize),
}

/// Relative start of live block `i`.
#[logic(open)]
pub fn lx(b: BuddyAllocator, lv: Seq<(u64, u64)>, i: Int) -> Int {
    pearlite! { lv[i].0@ - b.base_offset@ }
}

/// Order of live block `i` (the order `free` computes from its size, buddy.rs:86-91).
#[logic(open)]
pub fn lk(b: BuddyAllocator, lv: Seq<(u64, u64)>, i: Int) -> Int {
    pearlite! { ord(lv[i].1@ / b.sector_size@) }
}

/// Usable bytes: whole sectors of the region.
#[logic(open)]
pub fn usable(b: BuddyAllocator) -> Int {
    pearlite! { (b.total_usable_size@ / b.sector_size@) * b.sector_size@ }
}

/// Geometry facts fixed by `new` (power-of-two sector size, D-RANGE-FR-002-217ace).
#[logic(open)]
pub fn bgeo(b: BuddyAllocator) -> bool {
    pearlite! {
        bd_inv(b) && p2(b.sector_size@)
        && b.total_usable_size@ + span(b.sector_size@, b.max_order@) <= u64::MAX@
        && (b.total_usable_size@ / b.sector_size@ >= 1 ==>
            b.total_usable_size@ / b.sector_size@ < (b.max_order@ + 1).pow2()
            && b.max_order@.pow2() <= b.total_usable_size@ / b.sector_size@)
    }
}

/// Every live block is a well-formed block that `alloc` returned and no free block meets.
#[logic(open)]
pub fn live_ok(b: BuddyAllocator, lv: Seq<(u64, u64)>) -> bool {
    pearlite! {
        forall<i: Int> 0 <= i && i < lv.len() ==>
            lv[i].0@ >= b.base_offset@ && lk(b, lv, i) <= b.max_order@
            && good_block(b, lx(b, lv, i), lk(b, lv, i))
            && faway(b.free_lists@, b.sector_size@, lx(b, lv, i), span(b.sector_size@, lk(b, lv, i)))
    }
}

/// Live blocks are pairwise disjoint.
#[logic(open)]
pub fn live_disj(b: BuddyAllocator, lv: Seq<(u64, u64)>) -> bool {
    pearlite! {
        forall<i: Int, j: Int> 0 <= i && i < lv.len() && 0 <= j && j < lv.len() && i != j ==>
            disj(lx(b, lv, i), span(b.sector_size@, lk(b, lv, i)), lx(b, lv, j), span(b.sector_size@, lk(b, lv, j)))
    }
}

/// Byte `z` lies in a live block.
#[logic(open)]
pub fn inlive(b: BuddyAllocator, lv: Seq<(u64, u64)>, z: Int) -> bool {
    pearlite! { exists<i: Int> 0 <= i && i < lv.len() && lx(b, lv, i) <= z && z < lx(b, lv, i) + span(b.sector_size@, lk(b, lv, i)) }
}

/// `inlive` from a witness (bool form for use under quantifiers).
#[logic]
#[requires(0 <= i && i < lv.len() && lx(b, lv, i) <= z && z < lx(b, lv, i) + span(b.sector_size@, lk(b, lv, i)))]
#[ensures(result && inlive(b, lv, z))]
pub fn lemma_inlive_intro(b: BuddyAllocator, lv: Seq<(u64, u64)>, z: Int, i: Int) -> bool { true }

/// Every usable byte is free or live.
#[logic(open)]
pub fn live_cover(b: BuddyAllocator, lv: Seq<(u64, u64)>) -> bool {
    pearlite! { forall<z: Int> 0 <= z && z < usable(b) ==> ffree(b.free_lists@, b.sector_size@, z) || inlive(b, lv, z) }
}

/// The history invariant.
#[logic(open)]
pub fn bh(b: BuddyAllocator, lv: Seq<(u64, u64)>) -> bool {
    pearlite! {
        bgeo(b) && fdisj(b.free_lists@, b.sector_size@) && coal(b.free_lists@, b.sector_size@)
        && live_ok(b, lv) && live_disj(b, lv) && live_cover(b, lv)
    }
}

/// Same geometry (every call keeps it).
#[logic(open)]
pub fn same_geo(a: BuddyAllocator, b: BuddyAllocator) -> bool {
    pearlite! {
        a.base_offset == b.base_offset && a.total_usable_size == b.total_usable_size
        && a.sector_size == b.sector_size && a.max_order == b.max_order
        && a.free_lists@.len() == b.free_lists@.len()
    }
}

// ------------------------------------------------------------------ arithmetic

/// `ord(span(ss, k) / ss) == k`: freeing a block with its own span names its order.
#[logic]
#[requires(ss > 0 && 0 <= k && k < 64)]
#[ensures(ord(span(ss, k) / ss) == k)]
pub fn lemma_ord_span(ss: Int, k: Int) {
    lemma_pow2(k);
    lemma_mul_mod0(ss, k.pow2());
    proof_assert! { span(ss, k) / ss == k.pow2() };
    if k > 0 {
        lemma_pow2(k - 1)
    }
    proof_assert! { is_order(k, k.pow2()) };
    lemma_ord_eq(k.pow2(), 0, k)
}

/// The region is shorter than two top-order blocks.
#[logic]
#[requires(ss > 0 && total >= 0 && mo >= 0 && total / ss < (mo + 1).pow2())]
#[ensures(total < span(ss, mo + 1))]
pub fn lemma_tot_lt(ss: Int, total: Int, mo: Int) {
    lemma_divmod(total, ss);
    lemma_mul_le(total / ss + 1, (mo + 1).pow2(), ss);
    lemma_distrib(total / ss, 1, ss)
}

/// An aligned block that fits in the region fits in its whole sectors.
#[logic]
#[requires(ss > 0 && 0 <= kk && t >= 0 && t % span(ss, kk) == 0 && t + span(ss, kk) <= total)]
#[ensures(t + span(ss, kk) <= (total / ss) * ss)]
pub fn lemma_fit_usable(ss: Int, total: Int, kk: Int, t: Int) {
    lemma_span_pos(ss, kk);
    lemma_pow2(0);
    proof_assert! { span(ss, 0) == ss };
    lemma_span_mult(ss, 0, kk);
    lemma_span_div(t, ss, 0, kk);
    lemma_mod_add(t, span(ss, kk), ss);
    lemma_floor_le(t + span(ss, kk), ss, total)
}

// ------------------------------------------------------------- base case: new

/// `new`'s coverage, read at one aligned block (bool form for use under quantifiers).
#[logic]
#[requires(cov(fl, ss, (total / ss) * ss) && ss > 0 && total >= 0)]
#[requires(0 <= kk && 0 <= t && t % span(ss, kk) == 0 && t + span(ss, kk) <= total)]
#[ensures(result && inblk(fl, ss, kk, t))]
pub fn lemma_cov_total_b(fl: Seq<Vec<u64>>, ss: Int, total: Int, kk: Int, t: Int) -> bool {
    lemma_cov_total(fl, ss, total, kk, t);
    true
}

/// `new`'s coverage makes every usable byte free.
#[logic]
#[requires(cov(fl, ss, (total / ss) * ss) && ss > 0 && total >= 0 && 0 <= z && z < (total / ss) * ss)]
#[ensures(result && ffree(fl, ss, z))]
pub fn lemma_cov_ffree_b(fl: Seq<Vec<u64>>, ss: Int, total: Int, z: Int) -> bool {
    pearlite! {
        lemma_divmod(z, ss);
        lemma_divmod(total, ss);
        lemma_pow2(0);
        proof_assert! { span(ss, 0) == ss };
        lemma_mul_mod0(ss, z / ss);
        proof_assert! { (z - z % ss) % ss == 0 && z - z % ss >= 0 && z - z % ss == ss * (z / ss) };
        proof_assert! { z / ss < total / ss };
        lemma_mul_le(z / ss + 1, total / ss, ss);
        lemma_distrib(z / ss, 1, ss);
        proof_assert! { z - z % ss + ss <= (total / ss) * ss };
        proof_assert! { (total / ss) * ss <= total };
        lemma_cov_total(fl, ss, total, 0, z - z % ss);
        true
    }
}

/// The history invariant holds for the allocator `new` builds, with no live block.
#[logic]
#[requires(bd_inv(b) && p2(b.sector_size@) && b.base_offset@ + b.total_usable_size@ <= u64::MAX@)]
#[requires(2 * b.total_usable_size@ + b.sector_size@ <= u64::MAX@)]
#[requires(new_e(b.free_lists@, b.sector_size@, usable(b)))]
#[requires(b.total_usable_size@ / b.sector_size@ >= 1 ==>
    b.total_usable_size@ / b.sector_size@ < (b.max_order@ + 1).pow2() && b.max_order@.pow2() <= b.total_usable_size@ / b.sector_size@)]
#[ensures(bh(b, Seq::empty()))]
pub fn lemma_bh_new(b: BuddyAllocator) {
    pearlite! {
        lemma_new_e_elim(b.free_lists@, b.sector_size@, usable(b));
        lemma_span_pos(b.sector_size@, b.max_order@);
        lemma_pow2(0);
        proof_assert! { b.max_order@ == 0 ==> span(b.sector_size@, 0) == b.sector_size@ };
        proof_assert! { b.total_usable_size@ + span(b.sector_size@, b.max_order@) <= u64::MAX@ };
        proof_assert! { forall<kk: Int, t: Int> 0 <= kk && 0 <= t && t % span(b.sector_size@, kk) == 0
            && t + span(b.sector_size@, kk) <= b.total_usable_size@ ==>
            if lemma_cov_total_b(b.free_lists@, b.sector_size@, b.total_usable_size@, kk, t) {
                inblk(b.free_lists@, b.sector_size@, kk, t) } else { false } };
        proof_assert! { fal(b.free_lists@, b.sector_size@) };
        lemma_cov_coal(b.free_lists@, b.sector_size@, b.total_usable_size@);
        proof_assert! { forall<z: Int> 0 <= z && z < usable(b) ==>
            if lemma_cov_ffree_b(b.free_lists@, b.sector_size@, b.total_usable_size@, z) {
                ffree(b.free_lists@, b.sector_size@, z) } else { false } };
        proof_assert! { live_cover(b, Seq::empty()) }
    }
}

/// PROOF-ONLY ghost wrapper of [`lemma_bh_new`].
#[requires(bd_inv(*b) && p2(b.sector_size@) && b.base_offset@ + b.total_usable_size@ <= u64::MAX@)]
#[requires(2 * b.total_usable_size@ + b.sector_size@ <= u64::MAX@)]
#[requires(new_e(b.free_lists@, b.sector_size@, usable(*b)))]
#[requires(b.total_usable_size@ / b.sector_size@ >= 1 ==>
    b.total_usable_size@ / b.sector_size@ < (b.max_order@ + 1).pow2() && b.max_order@.pow2() <= b.total_usable_size@ / b.sector_size@)]
#[ensures(bh(*b, Seq::empty()))]
pub fn g_bh_new(b: Snapshot<BuddyAllocator>) {
    proof_assert! { lemma_bh_new(*b); true };
}

// --------------------------------------------------------------- step: alloc

/// A successful `alloc` adds its block to the live set and keeps the history invariant.
#[logic]
#[requires(bh(a, la) && alloc_post(a, b, on, fo, r@) && bd_inv(b) && same_geo(a, b))]
#[requires(sp@ == span(a.sector_size@, on) && lb == la.push_back((r, sp)))]
#[ensures(bh(b, lb))]
pub fn lemma_bh_alloc(a: BuddyAllocator, b: BuddyAllocator, la: Seq<(u64, u64)>, lb: Seq<(u64, u64)>, on: Int, fo: Int, r: u64, sp: u64) {
    pearlite! {
        lemma_alloc_disj(a, b, on, fo, r@);
        lemma_alloc_merge(a, b, on, fo, r@);
        lemma_span_pos(a.sector_size@, on);
        lemma_ord_span(a.sector_size@, on);
        proof_assert! { lk(b, lb, la.len()) == on && lx(b, lb, la.len()) == r@ - a.base_offset@ };
        proof_assert! { forall<i: Int> 0 <= i && i < la.len() ==> lb[i] == la[i] && lk(b, lb, i) == lk(a, la, i) && lx(b, lb, i) == lx(a, la, i) };
        proof_assert! { forall<i: Int> 0 <= i && i < la.len() ==> { lemma_span_pos(a.sector_size@, lk(a, la, i)); span(a.sector_size@, lk(a, la, i)) > 0 } };
        proof_assert! { live_ok(b, lb) };
        proof_assert! { live_disj(b, lb) };
        proof_assert! { forall<z: Int> inlive(a, la, z) ==> inlive(b, lb, z) };
        proof_assert! { forall<z: Int> r@ - a.base_offset@ <= z && z < r@ - a.base_offset@ + span(a.sector_size@, on) ==>
            if lemma_inlive_intro(b, lb, z, la.len()) { inlive(b, lb, z) } else { false } };
        proof_assert! { usable(b) == usable(a) };
        proof_assert! { forall<z: Int> 0 <= z && z < usable(b) ==> ffree(a.free_lists@, a.sector_size@, z) || inlive(a, la, z) };
        proof_assert! { forall<z: Int> 0 <= z && z < usable(b) && ffree(a.free_lists@, a.sector_size@, z) ==>
            ffree(b.free_lists@, b.sector_size@, z) || inlive(b, lb, z) };
        proof_assert! { live_cover(b, lb) }
    }
}

/// PROOF-ONLY ghost wrapper of [`lemma_bh_alloc`].
#[requires(bh(*a, *la) && alloc_post(*a, *b, *on, *fo, r@) && bd_inv(*b) && same_geo(*a, *b))]
#[requires(sp@ == span(a.sector_size@, *on) && *lb == la.push_back((r, sp)))]
#[ensures(bh(*b, *lb))]
pub fn g_bh_alloc(a: Snapshot<BuddyAllocator>, b: Snapshot<BuddyAllocator>, la: Snapshot<Seq<(u64, u64)>>, lb: Snapshot<Seq<(u64, u64)>>,
    on: Snapshot<Int>, fo: Snapshot<Int>, r: u64, sp: u64) {
    proof_assert! { lemma_bh_alloc(*a, *b, *la, *lb, *on, *fo, r, sp); true };
}

// ---------------------------------------------------------------- step: free

/// `lb` is `la` without element `i` (Vec::remove).
#[logic(open)]
pub fn rm_rel(lb: Seq<(u64, u64)>, la: Seq<(u64, u64)>, i: Int) -> bool {
    pearlite! {
        0 <= i && i < la.len() && lb.len() == la.len() - 1
        && forall<p: Int> 0 <= p && p < lb.len() ==> lb[p] == if p < i { la[p] } else { la[p + 1] }
    }
}

#[logic]
#[requires(0 <= i && i < la.len() && lb == la.subsequence(0, i).concat(la.subsequence(i + 1, la.len())))]
#[ensures(rm_rel(lb, la, i))]
pub fn lemma_rm_rel(lb: Seq<(u64, u64)>, la: Seq<(u64, u64)>, i: Int) {}

/// Freeing live block `i` removes it from the live set and keeps the history invariant.
#[logic]
#[requires(bh(a, la) && rm_rel(lb, la, i) && bd_inv(b) && same_geo(a, b))]
#[requires(free_e(a.free_lists@, b.free_lists@, a.sector_size@, lx(a, la, i), span(a.sector_size@, lk(a, la, i))))]
#[requires(free_m(a.free_lists@, b.free_lists@, a.sector_size@, lx(a, la, i), span(a.sector_size@, lk(a, la, i))))]
#[ensures(bh(b, lb))]
pub fn lemma_bh_free(a: BuddyAllocator, b: BuddyAllocator, la: Seq<(u64, u64)>, lb: Seq<(u64, u64)>, i: Int) {
    pearlite! {
        lemma_free_e_elim(a.free_lists@, b.free_lists@, a.sector_size@, lx(a, la, i), span(a.sector_size@, lk(a, la, i)));
        lemma_free_m_elim(a.free_lists@, b.free_lists@, a.sector_size@, lx(a, la, i), span(a.sector_size@, lk(a, la, i)));
        proof_assert! { fdisj(b.free_lists@, b.sector_size@) };
        proof_assert! { coal(b.free_lists@, b.sector_size@) };
        proof_assert! { forall<p: Int> 0 <= p && p < lb.len() ==>
            lk(b, lb, p) == lk(a, la, if p < i { p } else { p + 1 }) && lx(b, lb, p) == lx(a, la, if p < i { p } else { p + 1 }) };
        proof_assert! { forall<j: Int> 0 <= j && j < la.len() ==> { lemma_span_pos(a.sector_size@, lk(a, la, j)); span(a.sector_size@, lk(a, la, j)) > 0 } };
        proof_assert! { live_ok(b, lb) };
        proof_assert! { live_disj(b, lb) };
        proof_assert! { forall<z: Int, j: Int> 0 <= j && j < i && lx(a, la, j) <= z && z < lx(a, la, j) + span(a.sector_size@, lk(a, la, j)) ==>
            if lemma_inlive_intro(b, lb, z, j) { inlive(b, lb, z) } else { false } };
        proof_assert! { forall<j: Int> i < j && j < la.len() ==> 0 <= j - 1 && j - 1 < lb.len() && !(j - 1 < i)
            && lk(b, lb, j - 1) == lk(a, la, j) && lx(b, lb, j - 1) == lx(a, la, j) };
        proof_assert! { forall<z: Int, j: Int> i < j && j < la.len() && lx(a, la, j) <= z && z < lx(a, la, j) + span(a.sector_size@, lk(a, la, j)) ==>
            if lemma_inlive_intro(b, lb, z, j - 1) { inlive(b, lb, z) } else { false } };
        proof_assert! { forall<z: Int> lx(a, la, i) <= z && z < lx(a, la, i) + span(a.sector_size@, lk(a, la, i)) ==>
            ffree(b.free_lists@, b.sector_size@, z) };
        proof_assert! { forall<z: Int> inlive(a, la, z) ==> inlive(b, lb, z) || ffree(b.free_lists@, b.sector_size@, z) };
        proof_assert! { usable(b) == usable(a) };
        proof_assert! { forall<z: Int> 0 <= z && z < usable(b) ==> ffree(a.free_lists@, a.sector_size@, z) || inlive(a, la, z) };
        proof_assert! { forall<z: Int> ffree(a.free_lists@, a.sector_size@, z) ==> ffree(b.free_lists@, b.sector_size@, z) };
        proof_assert! { live_cover(b, lb) }
    }
}

/// PROOF-ONLY ghost wrapper of [`lemma_bh_free`].
#[requires(bh(*a, *la) && *lb == la.subsequence(0, *i).concat(la.subsequence(*i + 1, la.len())) && 0 <= *i && *i < la.len())]
#[requires(bd_inv(*b) && same_geo(*a, *b))]
#[requires(free_e(a.free_lists@, b.free_lists@, a.sector_size@, lx(*a, *la, *i), span(a.sector_size@, lk(*a, *la, *i))))]
#[requires(free_m(a.free_lists@, b.free_lists@, a.sector_size@, lx(*a, *la, *i), span(a.sector_size@, lk(*a, *la, *i))))]
#[ensures(bh(*b, *lb))]
pub fn g_bh_free(a: Snapshot<BuddyAllocator>, b: Snapshot<BuddyAllocator>, la: Snapshot<Seq<(u64, u64)>>, lb: Snapshot<Seq<(u64, u64)>>,
    i: Snapshot<Int>) {
    proof_assert! { lemma_rm_rel(*lb, *la, *i); lemma_bh_free(*a, *b, *la, *lb, *i); true };
}

// ------------------------------------------------------- step: mark_allocated (recovery)

/// No block of order above the top fits in the region.
#[logic]
#[requires(bgeo(b) && kk > b.max_order@ && x >= 0 && x + span(b.sector_size@, kk) <= b.total_usable_size@)]
#[ensures(false)]
pub fn lemma_no_big(b: BuddyAllocator, kk: Int, x: Int) {
    pearlite! {
        lemma_span_mono(b.sector_size@, b.max_order@ + 1, kk);
        lemma_divmod(b.total_usable_size@, b.sector_size@);
        lemma_span_pos(b.sector_size@, b.max_order@ + 1);
        lemma_span_mono(b.sector_size@, 0, b.max_order@ + 1);
        lemma_pow2(0);
        proof_assert! { span(b.sector_size@, 0) == b.sector_size@ };
        if b.total_usable_size@ / b.sector_size@ >= 1 {
            lemma_tot_lt(b.sector_size@, b.total_usable_size@, b.max_order@)
        }
    }
}

/// Marking (recovery, lib.rs:553-555) a block that fits, is aligned and is disjoint from every
/// live block makes it live and keeps the history invariant. Its bytes are all free (cover),
/// so by coalescing it lies inside ONE free block — mark_allocated's own hypothesis.
#[logic]
#[requires(bh(a, la) && bd_inv(b) && same_geo(a, b) && x.0@ >= a.base_offset@)]
#[requires((x.0@ - a.base_offset@) % span(a.sector_size@, ord(x.1@ / a.sector_size@)) == 0)]
#[requires(x.0@ - a.base_offset@ + span(a.sector_size@, ord(x.1@ / a.sector_size@)) <= a.total_usable_size@)]
#[requires(forall<p: Int> 0 <= p && p < la.len() ==>
    disj(lx(a, la, p), span(a.sector_size@, lk(a, la, p)), x.0@ - a.base_offset@, span(a.sector_size@, ord(x.1@ / a.sector_size@))))]
#[requires(mk_spec(a.free_lists@, b.free_lists@, a.sector_size@, x.0@ - a.base_offset@, ord(x.1@ / a.sector_size@)))]
#[requires(mk_c(a.free_lists@, b.free_lists@, a.sector_size@, x.0@ - a.base_offset@, ord(x.1@ / a.sector_size@)))]
#[requires(lb == la.push_back(x))]
#[ensures(mk_hyp(a.free_lists@, a.sector_size@, x.0@ - a.base_offset@, ord(x.1@ / a.sector_size@)))]
#[ensures(ord(x.1@ / a.sector_size@) <= a.max_order@)]
#[ensures(bh(b, lb))]
pub fn lemma_bh_mark(a: BuddyAllocator, b: BuddyAllocator, la: Seq<(u64, u64)>, lb: Seq<(u64, u64)>, x: (u64, u64)) {
    pearlite! {
        lemma_ord_ge(x.1@ / a.sector_size@);
        if ord(x.1@ / a.sector_size@) > a.max_order@ {
            lemma_no_big(a, ord(x.1@ / a.sector_size@), x.0@ - a.base_offset@)
        };
        lemma_span_pos(a.sector_size@, ord(x.1@ / a.sector_size@));
        lemma_fit_usable(a.sector_size@, a.total_usable_size@, ord(x.1@ / a.sector_size@), x.0@ - a.base_offset@);
        proof_assert! { forall<z: Int> x.0@ - a.base_offset@ <= z && z < x.0@ - a.base_offset@ + span(a.sector_size@, ord(x.1@ / a.sector_size@)) ==>
            !inlive(a, la, z) };
        proof_assert! { forall<z: Int> x.0@ - a.base_offset@ <= z && z < x.0@ - a.base_offset@ + span(a.sector_size@, ord(x.1@ / a.sector_size@)) ==>
            ffree(a.free_lists@, a.sector_size@, z) };
        proof_assert! { fal(a.free_lists@, a.sector_size@) };
        lemma_allfree_blk(a.free_lists@, a.sector_size@, ord(x.1@ / a.sector_size@), x.0@ - a.base_offset@);
        proof_assert! { mk_hyp(a.free_lists@, a.sector_size@, x.0@ - a.base_offset@, ord(x.1@ / a.sector_size@)) };
        lemma_mk_spec_elim(a.free_lists@, b.free_lists@, a.sector_size@, x.0@ - a.base_offset@, ord(x.1@ / a.sector_size@));
        lemma_mk_c_elim(a.free_lists@, b.free_lists@, a.sector_size@, x.0@ - a.base_offset@, ord(x.1@ / a.sector_size@));
        proof_assert! { lk(b, lb, la.len()) == ord(x.1@ / a.sector_size@) && lx(b, lb, la.len()) == x.0@ - a.base_offset@ };
        proof_assert! { forall<i: Int> 0 <= i && i < la.len() ==> lb[i] == la[i] && lk(b, lb, i) == lk(a, la, i) && lx(b, lb, i) == lx(a, la, i) };
        proof_assert! { forall<i: Int> 0 <= i && i < la.len() ==> { lemma_span_pos(a.sector_size@, lk(a, la, i)); span(a.sector_size@, lk(a, la, i)) > 0 } };
        proof_assert! { live_ok(b, lb) };
        proof_assert! { live_disj(b, lb) };
        proof_assert! { forall<z: Int> inlive(a, la, z) ==> inlive(b, lb, z) };
        proof_assert! { forall<z: Int> x.0@ - a.base_offset@ <= z && z < x.0@ - a.base_offset@ + span(a.sector_size@, ord(x.1@ / a.sector_size@)) ==>
            if lemma_inlive_intro(b, lb, z, la.len()) { inlive(b, lb, z) } else { false } };
        proof_assert! { usable(b) == usable(a) };
        proof_assert! { forall<z: Int> 0 <= z && z < usable(b) ==> ffree(a.free_lists@, a.sector_size@, z) || inlive(a, la, z) };
        proof_assert! { live_cover(b, lb) }
    }
}

/// PROOF-ONLY ghost wrapper of [`lemma_bh_mark`].
#[requires(bh(*a, *la) && bd_inv(*b) && same_geo(*a, *b) && x.0@ >= a.base_offset@)]
#[requires((x.0@ - a.base_offset@) % span(a.sector_size@, ord(x.1@ / a.sector_size@)) == 0)]
#[requires(x.0@ - a.base_offset@ + span(a.sector_size@, ord(x.1@ / a.sector_size@)) <= a.total_usable_size@)]
#[requires(forall<p: Int> 0 <= p && p < la.len() ==>
    disj(lx(*a, *la, p), span(a.sector_size@, lk(*a, *la, p)), x.0@ - a.base_offset@, span(a.sector_size@, ord(x.1@ / a.sector_size@))))]
#[requires(mk_spec(a.free_lists@, b.free_lists@, a.sector_size@, x.0@ - a.base_offset@, ord(x.1@ / a.sector_size@)))]
#[requires(mk_c(a.free_lists@, b.free_lists@, a.sector_size@, x.0@ - a.base_offset@, ord(x.1@ / a.sector_size@)))]
#[requires(*lb == la.push_back(x))]
#[ensures(bh(*b, *lb))]
pub fn g_bh_mark(a: Snapshot<BuddyAllocator>, b: Snapshot<BuddyAllocator>, la: Snapshot<Seq<(u64, u64)>>, lb: Snapshot<Seq<(u64, u64)>>, x: (u64, u64)) {
    proof_assert! { lemma_bh_mark(*a, *b, *la, *lb, x); true };
}

/// mark_allocated's own precondition at a recovered descriptor: its order is at most the top.
#[logic]
#[requires(bh(a, la) && x0 >= a.base_offset@ && 0 <= x1 && x1 <= u64::MAX@)]
#[requires(x0 - a.base_offset@ + span(a.sector_size@, ord(x1 / a.sector_size@)) <= a.total_usable_size@)]
#[ensures(result && ord(x1 / a.sector_size@) <= a.max_order@)]
pub fn lemma_desc_ord_b(a: BuddyAllocator, la: Seq<(u64, u64)>, x0: Int, x1: Int) -> bool {
    pearlite! {
        lemma_ord_ge(x1 / a.sector_size@);
        if ord(x1 / a.sector_size@) > a.max_order@ {
            lemma_no_big(a, ord(x1 / a.sector_size@), x0 - a.base_offset@)
        };
        true
    }
}

/// Distinct descriptor starts, aligned to the same block size, name disjoint blocks.
#[logic]
#[requires(m > 0 && x >= 0 && y >= 0 && x % m == 0 && y % m == 0 && x != y)]
#[ensures(result && disj(x, m, y, m))]
pub fn lemma_al_disj_b(x: Int, y: Int, m: Int) -> bool {
    if x < y {
        lemma_aligned_disj2(x, y, m);
        true
    } else {
        lemma_aligned_disj2(y, x, m);
        true
    }
}

// ------------------------------------------------------------------ one call

/// Apply one call to a reachable allocator: the history invariant and the geometry are kept.
/// `alloc` is called only within its own precondition (`size + sector_size <= u64::MAX`,
/// buddy.rs:58), and `free` only on a live block, with that block's span.
#[requires(bh(*b, lv@))]
#[ensures(bh(^b, (^lv)@) && same_geo(*b, ^b))]
pub fn apply_bop(b: &mut BuddyAllocator, lv: &mut Vec<(u64, u64)>, op: BOp) {
    let a = snapshot! { *b };
    let l0 = snapshot! { lv@ };
    let ss = b.sector_size as u64;
    match op {
        BOp::Alloc(sz) => {
            if sz <= u64::MAX - ss {
                let res = b.alloc(sz);
                match res {
                    Some(r) => {
                        let k = order_for_blocks((sz + ss - 1) / ss);
                        let on = snapshot! { ord((sz@ + a.sector_size@ - 1) / a.sector_size@) };
                        let fo = snapshot! { first_ne(a.free_lists@, *on, a.max_order@) };
                        proof_assert! { alloc_post(*a, *b, *on, *fo, r@) };
                        proof_assert! { k@ == *on && *on <= a.max_order@ && a.max_order@ < 64 };
                        proof_assert! { lemma_span_mono(ss@, k@, a.max_order@); span(ss@, k@) <= span(ss@, a.max_order@) };
                        proof_assert! { span(ss@, k@) <= u64::MAX@ };
                        let p = pow2_u64(k);
                        proof_assert! { p@ * ss@ == span(ss@, k@) };
                        let sp = p * ss;
                        lv.push((r, sp));
                        g_bh_alloc(a, snapshot! { *b }, l0, snapshot! { lv@ }, on, fo, r, sp); // PROOF-ONLY (ghost)
                    }
                    None => {}
                }
            }
        }
        BOp::Free(i) => {
            if i < lv.len() {
                let (x, s) = lv.remove(i);
                proof_assert! { (x, s) == l0[i@] };
                b.free(x, s);
                g_bh_free(a, snapshot! { *b }, l0, snapshot! { lv@ }, snapshot! { i@ }); // PROOF-ONLY (ghost)
            }
        }
    }
}

// ------------------------------------------------------------------ final state

/// With every block freed, an aligned block that fits lies inside one free block (bool form).
#[logic]
#[requires(bh(b, Seq::empty()))]
#[requires(0 <= kk && 0 <= t && t % span(b.sector_size@, kk) == 0 && t + span(b.sector_size@, kk) <= b.total_usable_size@)]
#[ensures(result && inblk(b.free_lists@, b.sector_size@, kk, t))]
pub fn lemma_fin_blk_b(b: BuddyAllocator, kk: Int, t: Int) -> bool {
    pearlite! {
        lemma_fit_usable(b.sector_size@, b.total_usable_size@, kk, t);
        lemma_span_pos(b.sector_size@, kk);
        proof_assert! { forall<z: Int> t <= z && z < t + span(b.sector_size@, kk) ==> ffree(b.free_lists@, b.sector_size@, z) };
        proof_assert! { fal(b.free_lists@, b.sector_size@) };
        if kk <= b.max_order@ {
            lemma_allfree_blk(b.free_lists@, b.sector_size@, kk, t);
            true
        } else {
            lemma_span_mono(b.sector_size@, b.max_order@ + 1, kk);
            lemma_divmod(b.total_usable_size@, b.sector_size@);
            lemma_pow2(b.max_order@);
            lemma_span_mono(b.sector_size@, 0, b.max_order@ + 1);
            proof_assert! { span(b.sector_size@, 0) == b.sector_size@ };
            if b.total_usable_size@ / b.sector_size@ >= 1 {
                lemma_tot_lt(b.sector_size@, b.total_usable_size@, b.max_order@);
                true
            } else {
                true
            }
        }
    }
}

/// With every block freed: every aligned block that fits lies inside one free block, and the
/// top list is exactly the largest original block `[0, span(max))`.
#[logic]
#[requires(bh(b, Seq::empty()))]
#[ensures(forall<kk: Int, t: Int> 0 <= kk && 0 <= t && t % span(b.sector_size@, kk) == 0
    && t + span(b.sector_size@, kk) <= b.total_usable_size@ ==> inblk(b.free_lists@, b.sector_size@, kk, t))]
#[ensures(b.total_usable_size@ / b.sector_size@ >= 1 ==>
    b.free_lists@[b.max_order@]@.len() == 1 && b.free_lists@[b.max_order@]@[0]@ == 0)]
pub fn lemma_bh_final(b: BuddyAllocator) {
    pearlite! {
        proof_assert! { forall<kk: Int, t: Int> 0 <= kk && 0 <= t && t % span(b.sector_size@, kk) == 0
            && t + span(b.sector_size@, kk) <= b.total_usable_size@ ==>
            if lemma_fin_blk_b(b, kk, t) { inblk(b.free_lists@, b.sector_size@, kk, t) } else { false } };
        if b.total_usable_size@ / b.sector_size@ >= 1 {
            lemma_span_pos(b.sector_size@, b.max_order@);
            lemma_tot_lt(b.sector_size@, b.total_usable_size@, b.max_order@);
            lemma_span_succ(b.sector_size@, b.max_order@);
            lemma_mul_le(b.max_order@.pow2(), b.total_usable_size@ / b.sector_size@, b.sector_size@);
            lemma_divmod(b.total_usable_size@, b.sector_size@);
            proof_assert! { span(b.sector_size@, b.max_order@) <= b.total_usable_size@ };
            lemma_mod_self(span(b.sector_size@, b.max_order@));
            proof_assert! { 0 % span(b.sector_size@, b.max_order@) == 0 };
            proof_assert! { inblk(b.free_lists@, b.sector_size@, b.max_order@, 0) };
            proof_assert! { forall<o: Int, k: Int> fv(b.free_lists@, o, k) && o >= b.max_order@ && fs(b.free_lists@, o, k) <= 0 ==>
                o == b.max_order@ && fs(b.free_lists@, o, k) == 0 };
            proof_assert! { b.free_lists@[b.max_order@]@.len() >= 1 };
            proof_assert! { forall<q: Int> 0 <= q && q < b.free_lists@[b.max_order@]@.len() ==> {
                if fs(b.free_lists@, b.max_order@, q) > 0 {
                    lemma_aligned_disj2(0, fs(b.free_lists@, b.max_order@, q), span(b.sector_size@, b.max_order@))
                };
                fs(b.free_lists@, b.max_order@, q) == 0 } };
            proof_assert! { b.free_lists@[b.max_order@]@.len() >= 2 ==>
                fv(b.free_lists@, b.max_order@, 0) && fv(b.free_lists@, b.max_order@, 1)
                && !disj(fs(b.free_lists@, b.max_order@, 0), span(b.sector_size@, b.max_order@),
                         fs(b.free_lists@, b.max_order@, 1), span(b.sector_size@, b.max_order@)) };
            proof_assert! { b.free_lists@[b.max_order@]@.len() == 1 }
        }
    }
}

/// PROOF-ONLY ghost wrapper of [`lemma_bh_final`].
#[requires(bh(*b, Seq::empty()))]
#[ensures(forall<kk: Int, t: Int> 0 <= kk && 0 <= t && t % span(b.sector_size@, kk) == 0
    && t + span(b.sector_size@, kk) <= b.total_usable_size@ ==> inblk(b.free_lists@, b.sector_size@, kk, t))]
#[ensures(b.total_usable_size@ / b.sector_size@ >= 1 ==>
    b.free_lists@[b.max_order@]@.len() == 1 && b.free_lists@[b.max_order@]@[0]@ == 0)]
pub fn g_bh_final(b: Snapshot<BuddyAllocator>) {
    proof_assert! { lemma_bh_final(*b); true };
}

/// The allocator `new` builds (format()) and then any sequence of calls `ops`.
#[requires(p2(sector_size@))]
#[requires(base@ + total@ <= u64::MAX@ && 2 * total@ + sector_size@ <= u64::MAX@)]
#[ensures(bh(result.0, result.1@))]
#[ensures(result.0.base_offset == base && result.0.total_usable_size == total && result.0.sector_size == sector_size)]
pub fn run_bops(base: u64, total: u64, sector_size: u32, ops: &Vec<BOp>) -> (BuddyAllocator, Vec<(u64, u64)>) {
    proof_assert! { forall<k: Int> 0 <= k && k.pow2() == sector_size@ ==> { lemma_pow2(k); sector_size@ > 0 } };
    let mut b = BuddyAllocator::new(base, total, sector_size);
    let mut lv: Vec<(u64, u64)> = Vec::new();
    g_bh_new(snapshot! { b }); // PROOF-ONLY (ghost)
    let mut i: usize = 0;
    #[invariant(bh(b, lv@))]
    #[invariant(b.base_offset == base && b.total_usable_size == total && b.sector_size == sector_size)]
    while i < ops.len() {
        apply_bop(&mut b, &mut lv, ops[i]);
        i += 1;
    }
    (b, lv)
}

/// initialize()'s buddy (lib.rs:552-555): `new`, then `mark_allocated(desc.start_offset,
/// desc.slab_size)` for every recovered descriptor; then any sequence of calls `ops`. The
/// recovered slabs are live blocks (freeing one is the region's slab release).
#[requires(p2(fp.sector_size@))]
#[requires(rg_in(base@, size@, fp))]
#[requires(forall<p: Int> 0 <= p && p < descs@.len() ==> desc_ok_a(descs@[p], base@, size@, fp))]
#[requires(a_980f61_list(descs@, base@, size@))]
#[ensures(bh(result.0, result.1@))]
#[ensures(result.0.base_offset == base && result.0.total_usable_size == size && result.0.sector_size == fp.sector_size)]
pub fn rebuilt_bops(base: u64, size: u64, fp: FormatParams, descs: &Vec<SlabDescriptor>, ops: &Vec<BOp>) -> (BuddyAllocator, Vec<(u64, u64)>) {
    proof_assert! { lemma_rg_in(base@, size@, fp); rg_hd(base@, size@, fp) };
    proof_assert! { forall<p: Int> 0 <= p && p < descs@.len() ==> { lemma_desc_ok_a(descs@[p], base@, size@, fp); desc_ok(descs@[p], base@, size@, fp) } };
    proof_assert! { forall<p: Int> 0 <= p && p < descs@.len() ==> descs@[p].slab_size@ > 0 };
    proof_assert! { lemma_starts_distinct(descs@, base@, size@); starts_distinct(descs@) };
    proof_assert! { forall<k: Int> 0 <= k && k.pow2() == fp.sector_size@ ==> { lemma_pow2(k); fp.sector_size@ > 0 } };
    let mut b = BuddyAllocator::new(base, size, fp.sector_size);
    let mut lv: Vec<(u64, u64)> = Vec::new();
    g_bh_new(snapshot! { b }); // PROOF-ONLY (ghost)
    let mut i: usize = 0;
    #[invariant(i@ <= descs@.len())]
    #[invariant(bh(b, lv@))]
    #[invariant(b.base_offset == base && b.total_usable_size == size && b.sector_size == fp.sector_size)]
    #[invariant(lv@.len() == i@ && forall<p: Int> 0 <= p && p < i@ ==> lv@[p] == (descs@[p].start_offset, descs@[p].slab_size))]
    while i < descs.len() {
        let x = (descs[i].start_offset, descs[i].slab_size);
        let a = snapshot! { b };
        let la = snapshot! { lv@ };
        proof_assert! { desc_ok(descs@[i@], base@, size@, fp) && x.1 == fp.slab_size };
        proof_assert! { if lemma_desc_ord_b(*a, *la, x.0@, x.1@) { ord(x.1@ / a.sector_size@) <= a.max_order@ } else { false } };
        proof_assert! { forall<p: Int> 0 <= p && p < i@ ==> descs@[p].start_offset != descs@[i@].start_offset && lk(*a, *la, p) == ord(x.1@ / a.sector_size@) };
        proof_assert! { forall<p: Int> 0 <= p && p < i@ ==>
            if lemma_al_disj_b(lx(*a, *la, p), x.0@ - a.base_offset@, span(a.sector_size@, ord(x.1@ / a.sector_size@))) {
                disj(lx(*a, *la, p), span(a.sector_size@, lk(*a, *la, p)), x.0@ - a.base_offset@, span(a.sector_size@, ord(x.1@ / a.sector_size@)))
            } else { false } };
        b.mark_allocated(x.0, x.1);
        lv.push(x);
        g_bh_mark(a, snapshot! { b }, la, snapshot! { lv@ }, x); // PROOF-ONLY (ghost)
        i += 1;
    }
    let mut j: usize = 0;
    #[invariant(bh(b, lv@))]
    #[invariant(b.base_offset == base && b.total_usable_size == size && b.sector_size == fp.sector_size)]
    while j < ops.len() {
        apply_bop(&mut b, &mut lv, ops[j]);
        j += 1;
    }
    (b, lv)
}

/// The three EM-BUDDY-MERGE facts for one reachable allocator.
#[logic(open)]
pub fn merge_facts(b: BuddyAllocator, lv: Seq<(u64, u64)>, ss: Int, total: Int) -> bool {
    pearlite! {
        coal(b.free_lists@, ss)
        && (lv.len() == 0 ==> forall<kk: Int, t: Int> 0 <= kk && 0 <= t && t % span(ss, kk) == 0
            && t + span(ss, kk) <= total ==> inblk(b.free_lists@, ss, kk, t))
        && (lv.len() == 0 && total / ss >= 1 ==>
            b.free_lists@[b.max_order@]@.len() == 1 && b.free_lists@[b.max_order@]@[0]@ == 0)
    }
}

/// PROOF-ONLY ghost helper: the history invariant gives the three facts.
#[requires(bh(*b, *lv))]
#[ensures(merge_facts(*b, *lv, b.sector_size@, b.total_usable_size@))]
pub fn g_merge_facts(b: Snapshot<BuddyAllocator>, lv: Snapshot<Seq<(u64, u64)>>) {
    proof_assert! { lv.len() == 0 ==> { lemma_bh_final(*b); true } };
}

// ============================================================== the scored module

/// **EM-BUDDY-MERGE** — "When a freed block's buddy is also free, the two are merged, so
/// freeing all blocks of a region makes the largest original blocks available again."
/// LEVEL-2 under D-RANGE-FR-002-217ace (power-of-two sector size). For a region's buddy as
/// format() builds it (`result.0`: `new`) and as initialize() rebuilds it (`result.1`: `new`
/// + `mark_allocated` of every recovered slab), followed by ANY sequence of `alloc` / `free`
/// calls (free only of a live block: one alloc returned or a recovered slab, not yet freed):
/// (1) no two free buddies of the same order are left unmerged (`coal`); (2) once every
/// block is freed, every aligned block that fits in the region lies inside ONE free block
/// again (the coverage `new` guarantees); (3) the largest original block
/// `[0, span(max_order))` is again a single free block, the only one on the top-order list.
#[requires(p2(fp.sector_size@))]
#[requires(rg_in(base@, size@, fp))]
#[requires(forall<p: Int> 0 <= p && p < descs@.len() ==> desc_ok_a(descs@[p], base@, size@, fp))]
#[requires(a_980f61_list(descs@, base@, size@))]
#[ensures(coal(result.0.0.free_lists@, fp.sector_size@))]
#[ensures(result.0.1@.len() == 0 ==> forall<kk: Int, t: Int> 0 <= kk && 0 <= t && t % span(fp.sector_size@, kk) == 0
    && t + span(fp.sector_size@, kk) <= size@ ==> inblk(result.0.0.free_lists@, fp.sector_size@, kk, t))]
#[ensures(result.0.1@.len() == 0 && size@ / fp.sector_size@ >= 1 ==>
    result.0.0.free_lists@[result.0.0.max_order@]@.len() == 1 && result.0.0.free_lists@[result.0.0.max_order@]@[0]@ == 0)]
#[ensures(coal(result.1.0.free_lists@, fp.sector_size@))]
#[ensures(result.1.1@.len() == 0 ==> forall<kk: Int, t: Int> 0 <= kk && 0 <= t && t % span(fp.sector_size@, kk) == 0
    && t + span(fp.sector_size@, kk) <= size@ ==> inblk(result.1.0.free_lists@, fp.sector_size@, kk, t))]
#[ensures(result.1.1@.len() == 0 && size@ / fp.sector_size@ >= 1 ==>
    result.1.0.free_lists@[result.1.0.max_order@]@.len() == 1 && result.1.0.free_lists@[result.1.0.max_order@]@[0]@ == 0)]
pub fn verify_em_buddy_merge(base: u64, size: u64, fp: FormatParams, descs: &Vec<SlabDescriptor>, ops: &Vec<BOp>)
    -> ((BuddyAllocator, Vec<(u64, u64)>), (BuddyAllocator, Vec<(u64, u64)>)) {
    proof_assert! { lemma_rg_in(base@, size@, fp); rg_hd(base@, size@, fp) };
    proof_assert! { forall<p: Int> 0 <= p && p < descs@.len() ==> { lemma_desc_ok_a(descs@[p], base@, size@, fp); desc_ok(descs@[p], base@, size@, fp) } };
    proof_assert! { forall<p: Int> 0 <= p && p < descs@.len() ==> descs@[p].slab_size@ > 0 };
    proof_assert! { lemma_starts_distinct(descs@, base@, size@); starts_distinct(descs@) };
    let f = run_bops(base, size, fp.sector_size, ops);
    g_merge_facts(snapshot! { f.0 }, snapshot! { f.1@ }); // PROOF-ONLY (ghost)
    let g = rebuilt_bops(base, size, fp, descs, ops);
    g_merge_facts(snapshot! { g.0 }, snapshot! { g.1@ }); // PROOF-ONLY (ghost)
    (f, g)
}

/// Mutant: the same proof, with ensures (3) of the format() path flipped: claims that once
/// every block is freed the top-order list is EMPTY (the largest original block is not
/// available again).
#[requires(p2(fp.sector_size@))]
#[requires(rg_in(base@, size@, fp))]
#[requires(forall<p: Int> 0 <= p && p < descs@.len() ==> desc_ok_a(descs@[p], base@, size@, fp))]
#[requires(a_980f61_list(descs@, base@, size@))]
#[ensures(coal(result.0.0.free_lists@, fp.sector_size@))]
#[ensures(result.0.1@.len() == 0 ==> forall<kk: Int, t: Int> 0 <= kk && 0 <= t && t % span(fp.sector_size@, kk) == 0
    && t + span(fp.sector_size@, kk) <= size@ ==> inblk(result.0.0.free_lists@, fp.sector_size@, kk, t))]
#[ensures(result.0.1@.len() == 0 && size@ / fp.sector_size@ >= 1 ==>
    result.0.0.free_lists@[result.0.0.max_order@]@.len() == 0)]
#[ensures(coal(result.1.0.free_lists@, fp.sector_size@))]
#[ensures(result.1.1@.len() == 0 ==> forall<kk: Int, t: Int> 0 <= kk && 0 <= t && t % span(fp.sector_size@, kk) == 0
    && t + span(fp.sector_size@, kk) <= size@ ==> inblk(result.1.0.free_lists@, fp.sector_size@, kk, t))]
#[ensures(result.1.1@.len() == 0 && size@ / fp.sector_size@ >= 1 ==>
    result.1.0.free_lists@[result.1.0.max_order@]@.len() == 1 && result.1.0.free_lists@[result.1.0.max_order@]@[0]@ == 0)]
pub fn verify_em_buddy_merge__mutant(base: u64, size: u64, fp: FormatParams, descs: &Vec<SlabDescriptor>, ops: &Vec<BOp>)
    -> ((BuddyAllocator, Vec<(u64, u64)>), (BuddyAllocator, Vec<(u64, u64)>)) {
    proof_assert! { lemma_rg_in(base@, size@, fp); rg_hd(base@, size@, fp) };
    proof_assert! { forall<p: Int> 0 <= p && p < descs@.len() ==> { lemma_desc_ok_a(descs@[p], base@, size@, fp); desc_ok(descs@[p], base@, size@, fp) } };
    proof_assert! { forall<p: Int> 0 <= p && p < descs@.len() ==> descs@[p].slab_size@ > 0 };
    proof_assert! { lemma_starts_distinct(descs@, base@, size@); starts_distinct(descs@) };
    let f = run_bops(base, size, fp.sector_size, ops);
    g_merge_facts(snapshot! { f.0 }, snapshot! { f.1@ }); // PROOF-ONLY (ghost)
    let g = rebuilt_bops(base, size, fp, descs, ops);
    g_merge_facts(snapshot! { g.0 }, snapshot! { g.1@ }); // PROOF-ONLY (ghost)
    (f, g)
}
