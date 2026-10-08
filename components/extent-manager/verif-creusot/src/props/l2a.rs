//! Level-2 batch A — the 27 properties refuted only through BUDDY-MASK (buddy.rs:136 mask,
//! buddy.rs:96 xor), re-proved IN GENERAL under the level-1 assumption
//! D-RANGE-FR-002-217ace: the sector size is a power of two (`p2(sector_size)`, the std
//! definition of `u32::is_power_of_two`: `sector_size == 2^k` for some k).
//!
//! Architecture (all proved):
//!  * `model/l2.rs` — buddy theory: free blocks pairwise disjoint (`fdisj`), interval
//!    avoidance (`faway`), free bytes (`ffree`), coverage of the fresh decomposition
//!    (`cov`), the power-of-two mask fact (`mask_fact`, buddy.rs:136); new contracts of
//!    `BuddyAllocator::new` (`new_e`), `free` (`free_e`), `mark_allocated` (`mk_spec`).
//!  * `model/l2r.rs` — the maintained region invariant `rg_good` and the exact effects
//!    alloc_extent (`alloc_any`), free_slot (`free_e`), flush (`fl_spec`) and the
//!    initialize rebuild (`rg_good_o`) now expose.
//!  * here — the history model: one region and its live write handles `hs` (reserved,
//!    not yet published/aborted slots), one call at a time (`LOp`), with the invariant
//!    `lg(r, hs)`; it is established by `format` (fresh region) and by `initialize`
//!    (rebuilt region) and preserved by every call.
use crate::model::bitmap::*;
use crate::model::buddy::*;
use crate::model::component::*;
use crate::model::b4::*;
use crate::model::l2::*;
use crate::model::l2r::*;
use crate::model::listing::*;
use crate::model::params::*;
use crate::model::region::*;
use crate::model::slab::*;
use creusot_std::logic::FMap;
use crate::model::assume::*;
use crate::props::l3start::em_cap_ok;
use creusot_std::prelude::*;

// =============================================================== buddy alloc (buddy.rs:57-82)

/// `alloc`'s split keeps the free blocks disjoint; the returned block `[r - base, + span(on))`
/// avoids every remaining free block and lies inside a former free block, so every interval
/// that avoided the old free blocks avoids the new ones and the returned block.
#[logic]
#[requires(alloc_post(a, b, on, fo, r) && bd_inv(a) && fdisj(a.free_lists@, a.sector_size@))]
#[requires(b.free_lists@.len() == a.free_lists@.len())]
#[ensures(fdisj(b.free_lists@, a.sector_size@))]
#[ensures(faway(b.free_lists@, a.sector_size@, r - a.base_offset@, span(a.sector_size@, on)))]
#[ensures(forall<y: Int, l: Int> 0 < l && faway(a.free_lists@, a.sector_size@, y, l) ==>
    faway(b.free_lists@, a.sector_size@, y, l) && disj(y, l, r - a.base_offset@, span(a.sector_size@, on)))]
pub fn lemma_alloc_disj(a: BuddyAllocator, b: BuddyAllocator, on: Int, fo: Int, r: Int) {
    pearlite! {
        proof_assert! { forall<o: Int> 0 <= o ==> { lemma_span_pos(a.sector_size@, o); span(a.sector_size@, o) > 0 } };
        proof_assert! { forall<o: Int> on <= o && o < fo ==> {
            lemma_span_succ(a.sector_size@, o); lemma_span_mono(a.sector_size@, on, o); lemma_span_mono(a.sector_size@, o + 1, fo);
            span(a.sector_size@, o + 1) == 2 * span(a.sector_size@, o) && span(a.sector_size@, on) <= span(a.sector_size@, o)
            && span(a.sector_size@, o + 1) <= span(a.sector_size@, fo) } };
        proof_assert! { forall<o1: Int, o2: Int> on <= o1 && o1 < o2 && o2 < fo ==> {
            lemma_span_mono(a.sector_size@, o1 + 1, o2); span(a.sector_size@, o1 + 1) <= span(a.sector_size@, o2) } };
        lemma_span_mono(a.sector_size@, on, fo);
        proof_assert! { fv(a.free_lists@, fo, a.free_lists@[fo]@.len() - 1) };
        proof_assert! { forall<o: Int, k: Int> fv(b.free_lists@, o, k) ==>
            (on <= o && o < fo && k == a.free_lists@[o]@.len()
                && fs(b.free_lists@, o, k) == fs(a.free_lists@, fo, a.free_lists@[fo]@.len() - 1) + span(a.sector_size@, o))
            || (fv(a.free_lists@, o, k) && fs(b.free_lists@, o, k) == fs(a.free_lists@, o, k)
                && (o != fo || k != a.free_lists@[fo]@.len() - 1)) };
        proof_assert! { forall<o: Int, k: Int> fv(a.free_lists@, o, k) && (o != fo || k != a.free_lists@[fo]@.len() - 1) ==>
            disj(fs(a.free_lists@, o, k), span(a.sector_size@, o),
                 fs(a.free_lists@, fo, a.free_lists@[fo]@.len() - 1), span(a.sector_size@, fo)) };
        proof_assert! { forall<y: Int, l2: Int> 0 < l2 && faway(a.free_lists@, a.sector_size@, y, l2) ==>
            disj(y, l2, fs(a.free_lists@, fo, a.free_lists@[fo]@.len() - 1), span(a.sector_size@, fo)) };
        proof_assert! { forall<o1: Int, o2: Int> on <= o1 && o1 < o2 && o2 < fo ==> {
            lemma_sib_disj(a.sector_size@, fs(a.free_lists@, fo, a.free_lists@[fo]@.len() - 1), o1, o2);
            disj(fs(a.free_lists@, fo, a.free_lists@[fo]@.len() - 1) + span(a.sector_size@, o1), span(a.sector_size@, o1),
                 fs(a.free_lists@, fo, a.free_lists@[fo]@.len() - 1) + span(a.sector_size@, o2), span(a.sector_size@, o2)) } };
        proof_assert! { forall<o: Int, k: Int, o2: Int> fv(a.free_lists@, o, k) && (o != fo || k != a.free_lists@[fo]@.len() - 1)
            && on <= o2 && o2 < fo ==>
            disj(fs(a.free_lists@, o, k), span(a.sector_size@, o),
                 fs(a.free_lists@, fo, a.free_lists@[fo]@.len() - 1) + span(a.sector_size@, o2), span(a.sector_size@, o2)) };
        proof_assert! { forall<o1: Int, k1: Int, o2: Int, k2: Int> fv(b.free_lists@, o1, k1) && fv(b.free_lists@, o2, k2) && (o1 != o2 || k1 != k2)
            && !(on <= o1 && o1 < fo && k1 == a.free_lists@[o1]@.len()) && !(on <= o2 && o2 < fo && k2 == a.free_lists@[o2]@.len()) ==>
            disj(fs(b.free_lists@, o1, k1), span(a.sector_size@, o1), fs(b.free_lists@, o2, k2), span(a.sector_size@, o2)) };
        proof_assert! { forall<o1: Int, k1: Int, o2: Int, k2: Int> fv(b.free_lists@, o1, k1) && fv(b.free_lists@, o2, k2)
            && (on <= o1 && o1 < fo && k1 == a.free_lists@[o1]@.len()) && !(on <= o2 && o2 < fo && k2 == a.free_lists@[o2]@.len()) ==>
            disj(fs(b.free_lists@, o1, k1), span(a.sector_size@, o1), fs(b.free_lists@, o2, k2), span(a.sector_size@, o2)) };
        proof_assert! { forall<o1: Int, k1: Int, o2: Int, k2: Int> fv(b.free_lists@, o1, k1) && fv(b.free_lists@, o2, k2) && (o1 != o2 || k1 != k2)
            && (on <= o1 && o1 < fo && k1 == a.free_lists@[o1]@.len()) && (on <= o2 && o2 < fo && k2 == a.free_lists@[o2]@.len()) ==>
            o1 != o2 && disj(fs(b.free_lists@, o1, k1), span(a.sector_size@, o1), fs(b.free_lists@, o2, k2), span(a.sector_size@, o2)) };
        proof_assert! { forall<o1: Int, k1: Int, o2: Int, k2: Int> fv(b.free_lists@, o1, k1) && fv(b.free_lists@, o2, k2) && (o1 != o2 || k1 != k2) ==>
            disj(fs(b.free_lists@, o1, k1), span(a.sector_size@, o1), fs(b.free_lists@, o2, k2), span(a.sector_size@, o2)) }
    }
}

/// Two split halves of different orders do not overlap.
#[logic]
#[requires(ss > 0 && 0 <= o1 && o1 < o2)]
#[ensures(disj(loc + span(ss, o1), span(ss, o1), loc + span(ss, o2), span(ss, o2)))]
pub fn lemma_sib_disj(ss: Int, loc: Int, o1: Int, o2: Int) {
    lemma_span_succ(ss, o1);
    lemma_span_mono(ss, o1 + 1, o2);
    lemma_span_pos(ss, o1)
}

// ============================================================ live slots and handle lists

/// The history invariant: the region invariant, the live write handles `hs` are valid
/// reserved slots (live, FREE_KEY, distinct) and none of them is a queued deferred free.
#[logic(open)]
pub fn lg(r: RegionState, hs: Seq<(u64, usize)>) -> bool {
    pearlite! {
        rg_good(r) && pv(r, hs)
        && forall<p: Int, q: Int> 0 <= p && p < r.pending_frees@.len() && 0 <= q && q < hs.len() ==> r.pending_frees@[p] != hs[q]
    }
}

/// `y` is `x` without its element `j` (Vec::remove).
#[logic(open)]
pub fn subl(y: Seq<(u64, usize)>, x: Seq<(u64, usize)>, j: Int) -> bool {
    pearlite! {
        0 <= j && j < x.len() && y.len() == x.len() - 1
        && forall<p: Int> 0 <= p && p < y.len() ==> y[p] == if p < j { x[p] } else { x[p + 1] }
    }
}

#[logic]
#[requires(0 <= j && j < x.len() && y == x.subsequence(0, j).concat(x.subsequence(j + 1, x.len())))]
#[ensures(subl(y, x, j))]
pub fn lemma_subl(y: Seq<(u64, usize)>, x: Seq<(u64, usize)>, j: Int) {}

/// Dropping a handle keeps the list valid, and the dropped slot is no longer listed.
#[logic]
#[requires(pv(r, x) && subl(y, x, j))]
#[ensures(pv(r, y))]
#[ensures(forall<p: Int> 0 <= p && p < y.len() ==> y[p] != x[j])]
#[ensures(forall<p: Int> 0 <= p && p < y.len() ==> exists<q: Int> 0 <= q && q < x.len() && y[p] == x[q])]
#[ensures(!inpf(y, 0, x[j].0@, x[j].1@))]
pub fn lemma_pv_sub(r: RegionState, x: Seq<(u64, usize)>, y: Seq<(u64, usize)>, j: Int) {
    pearlite! {
        proof_assert! { forall<p: Int> 0 <= p && p < y.len() ==> y[p] != x[j] };
        proof_assert! { forall<p: Int> 0 <= p && p < y.len() && y[p].0@ == x[j].0@ && y[p].1@ == x[j].1@ ==> y[p] == x[j] }
    }
}

/// Validity of a slot list transfers along kept slots.
#[logic]
#[requires(pv(a, ps) && forall<p: Int> 0 <= p && p < ps.len() ==> keep_slot(a, b, ps[p].0@, ps[p].1@))]
#[ensures(pv(b, ps))]
pub fn lemma_pv_keep(a: RegionState, b: RegionState, ps: Seq<(u64, usize)>) {}

// ================================================================= alloc_extent (Reserve)

/// Size-class validity transfers along a sub-listing when every slab keeps its element size.
#[logic]
#[requires(sc_ok(a) && sc_sub(a.size_classes, b.size_classes, es, -1))]
#[requires(forall<k: Int> a.slabs@.contains(k) ==> b.slabs@.contains(k) && b.slabs@.lookup(k).element_size == a.slabs@.lookup(k).element_size)]
#[ensures(sc_ok(b))]
pub fn lemma_sc_ok_sub(a: RegionState, b: RegionState, es: Int) {
    pearlite! {
        proof_assert! { forall<e: Int, p: Int> 0 <= p && p < sc_get(b.size_classes, e).len() ==> {
            lemma_sc_sub_at(a.size_classes, b.size_classes, es, -1, e, p);
            listed(sc_get(a.size_classes, e), sc_get(b.size_classes, e)[p]@) } };
        proof_assert! { forall<e: Int, v: Int> listed(sc_get(a.size_classes, e), v) ==> a.slabs@.contains(v) && a.slabs@.lookup(v).element_size@ == e }
    }
}

/// The exist path (region.rs:46-58): one slot of a listed slab becomes allocated.
#[logic]
#[requires(lg(a, hs) && rg_inv(b) && rg_frame(a, b) && b.pending_frees == a.pending_frees)]
#[requires(sc_sub(a.size_classes, b.size_classes, es, -1) && b.buddy == a.buddy)]
#[requires(listed(sc_get(a.size_classes, es), s@) && a.slabs@.contains(s@))]
#[requires(b.slabs@ == a.slabs@.insert(s@, b.slabs@.lookup(s@)))]
#[requires(slot_step(a.slabs@.lookup(s@), b.slabs@.lookup(s@), i@))]
#[ensures(lg(b, hs.push_back((s, i))) && !slot_live(a, s@, i@) && b.slabs@.lookup(s@).element_size@ == es)]
pub fn lemma_reserve_exist(a: RegionState, b: RegionState, es: Int, s: u64, i: usize, hs: Seq<(u64, usize)>) {
    pearlite! {
        proof_assert! { forall<k: Int> b.slabs@.contains(k) == a.slabs@.contains(k) };
        proof_assert! { forall<k: Int> k != s@ ==> b.slabs@.get(k) == a.slabs@.get(k) };
        proof_assert! { b.slabs@.len() == a.slabs@.len() };
        proof_assert! { a.slabs@.lookup(s@).element_size@ == es };
        proof_assert! { rg_good(a) && rg_core(a) && pv(a, a.pending_frees@) && pv(a, hs) };
        proof_assert! { fdisj(a.buddy.free_lists@, a.buddy.sector_size@) && sl_away(a) && rg_acct(a) && sc_ok(a) && rg_ka(a) };
        proof_assert! { sk(b) == sk(a) && rg_tf(b) == rg_tf(a) };
        proof_assert! { fdisj(b.buddy.free_lists@, b.buddy.sector_size@) };
        proof_assert! { sl_away(b) };
        proof_assert! { rg_acct(b) };
        proof_assert! { forall<k: Int> a.slabs@.contains(k) ==> b.slabs@.contains(k) && b.slabs@.lookup(k).element_size == a.slabs@.lookup(k).element_size };
        proof_assert! { lemma_sc_ok_sub(a, b, es); sc_ok(b) };
        proof_assert! { key_alloc(a.slabs@.lookup(s@)) && key_alloc(b.slabs@.lookup(s@)) };
        proof_assert! { rg_ka(b) && rg_core(b) };
        proof_assert! { !slot_live(a, s@, i@) && b.slabs@.lookup(s@).keys@[i@] == FREE_KEY };
        proof_assert! { forall<t: Int, j: Int> slot_live(a, t, j) ==> keep_slot(a, b, t, j) };
        lemma_pv_keep(a, b, a.pending_frees@);
        lemma_pv_keep(a, b, hs);
        proof_assert! { forall<p: Int> 0 <= p && p < hs.len() ==> hs[p] != (s, i) };
        proof_assert! { forall<p: Int> 0 <= p && p < a.pending_frees@.len() ==> a.pending_frees@[p] != (s, i) };
        proof_assert! { pv(b, hs.push_back((s, i))) }
    }
}

/// The fresh path (region.rs:63-91): a new slab is carved from a free block.
#[logic]
#[requires(lg(a, hs) && rg_inv(b) && rg_frame(a, b) && b.pending_frees == a.pending_frees)]
#[requires(sc_sub(a.size_classes, b.size_classes, es, d@))]
#[requires(b.slabs@ == a.slabs@.insert(d@, b.slabs@.lookup(d@)))]
#[requires(fresh_one(b.slabs@.lookup(d@), d@, a.format_params.slab_size@, es))]
#[requires(alloc_post(a.buddy, b.buddy, slab_k(a), fo, d@))]
#[requires(rg_tf(b) == rg_tf(a) - sk(a))]
#[requires(i@ < b.slabs@.lookup(d@).bitmap.num_slots@ && slot_bit(b.slabs@.lookup(d@).bitmap, i@))]
#[ensures(lg(b, hs.push_back((d, i))) && !slot_live(a, d@, i@) && b.slabs@.lookup(d@).element_size@ == es)]
#[ensures(!a.slabs@.contains(d@))]
pub fn lemma_reserve_fresh(a: RegionState, b: RegionState, es: Int, d: u64, i: usize, fo: Int, hs: Seq<(u64, usize)>) {
    pearlite! {
        lemma_alloc_disj(a.buddy, b.buddy, slab_k(a), fo, d@);
        lemma_span_pos(a.buddy.sector_size@, slab_k(a));
        proof_assert! { sk(b) == sk(a) && b.buddy.sector_size == a.buddy.sector_size && b.buddy.base_offset == a.buddy.base_offset };
        proof_assert! { !a.slabs@.contains(d@) };
        proof_assert! { forall<k: Int> b.slabs@.contains(k) == (a.slabs@.contains(k) || k == d@) };
        proof_assert! { forall<k: Int> k != d@ ==> b.slabs@.get(k) == a.slabs@.get(k) };
        proof_assert! { b.slabs@.len() == a.slabs@.len() + 1 };
        proof_assert! { lemma_distrib(a.slabs@.len(), 1, sk(a)); (a.slabs@.len() + 1) * sk(a) == a.slabs@.len() * sk(a) + sk(a) };
        proof_assert! { b.buddy.total_usable_size == a.buddy.total_usable_size };
        proof_assert! { rg_good(a) && rg_core(a) && rg_acct(a) };
        proof_assert! { b.slabs@.len() * sk(b) == a.slabs@.len() * sk(a) + sk(a) };
        proof_assert! { rg_tf(b) + b.slabs@.len() * sk(b) == rg_tf(a) + a.slabs@.len() * sk(a) };
        proof_assert! { fdisj(b.buddy.free_lists@, b.buddy.sector_size@) && sl_away(b) && rg_acct(b) };
        proof_assert! { forall<e: Int, p: Int> 0 <= p && p < sc_get(b.size_classes, e).len() ==> {
            lemma_sc_sub_at(a.size_classes, b.size_classes, es, d@, e, p);
            listed(sc_get(a.size_classes, e), sc_get(b.size_classes, e)[p]@) || (e == es && sc_get(b.size_classes, e)[p]@ == d@) } };
        proof_assert! { sc_ok(b) };
        proof_assert! { key_alloc(b.slabs@.lookup(d@)) && rg_ka(b) && rg_core(b) };
        proof_assert! { forall<t: Int, j: Int> slot_live(a, t, j) ==> keep_slot(a, b, t, j) };
        lemma_pv_keep(a, b, a.pending_frees@);
        lemma_pv_keep(a, b, hs);
        proof_assert! { b.slabs@.lookup(d@).keys@[i@] == FREE_KEY };
        proof_assert! { forall<p: Int> 0 <= p && p < hs.len() ==> hs[p] != (d, i) };
        proof_assert! { forall<p: Int> 0 <= p && p < a.pending_frees@.len() ==> a.pending_frees@[p] != (d, i) };
        proof_assert! { pv(b, hs.push_back((d, i))) }
    }
}

/// The zero-slot rollback (region.rs:79-83): alloc then free of the same block.
#[logic]
#[requires(rg_good(a) && b.slabs@ == a.slabs@ && rg_frame(a, b))]
#[requires(alloc_post(a.buddy, m, slab_k(a), fo, d))]
#[requires(m.sector_size == a.buddy.sector_size && m.free_lists@.len() == a.buddy.free_lists@.len())]
#[requires(free_e(m.free_lists@, b.buddy.free_lists@, m.sector_size@, d - a.buddy.base_offset@, sk(a)))]
#[ensures(fdisj(b.buddy.free_lists@, b.buddy.sector_size@) && sl_away(b))]
pub fn lemma_err_rollback(a: RegionState, b: RegionState, m: BuddyAllocator, fo: Int, d: Int) {
    pearlite! {
        lemma_ord_ge(a.format_params.slab_size@ / a.format_params.sector_size@);
        lemma_span_pos(a.buddy.sector_size@, slab_k(a));
        lemma_alloc_disj(a.buddy, m, slab_k(a), fo, d);
        lemma_free_e_elim(m.free_lists@, b.buddy.free_lists@, m.sector_size@, d - a.buddy.base_offset@, sk(a));
        proof_assert! { sk(b) == sk(a) && b.buddy.sector_size == a.buddy.sector_size && b.buddy.base_offset == a.buddy.base_offset };
        proof_assert! { span(a.buddy.sector_size@, slab_k(a)) == sk(a) };
        proof_assert! { forall<k: Int> a.slabs@.contains(k) ==> faway(m.free_lists@, m.sector_size@, k - a.buddy.base_offset@, sk(a))
            && disj(k - a.buddy.base_offset@, sk(a), d - a.buddy.base_offset@, sk(a)) };
        proof_assert! { forall<k: Int> b.slabs@.contains(k) ==> faway(b.buddy.free_lists@, b.buddy.sector_size@, k - b.buddy.base_offset@, sk(b)) }
    }
}

/// The error paths (region.rs:64-70, 79-83): no slab changes; the buddy allocator is
/// unchanged or got back the block it just handed out.
#[logic]
#[requires(lg(a, hs) && rg_inv(b) && rg_frame(a, b) && b.pending_frees == a.pending_frees)]
#[requires(b.slabs@ == a.slabs@ && sc_sub(a.size_classes, b.size_classes, es, -1) && rg_tf(b) == rg_tf(a))]
#[requires(b.buddy == a.buddy
    || exists<m: BuddyAllocator, fo: Int, d: Int> alloc_post(a.buddy, m, slab_k(a), fo, d)
        && m.sector_size == a.buddy.sector_size && m.free_lists@.len() == a.buddy.free_lists@.len()
        && free_e(m.free_lists@, b.buddy.free_lists@, m.sector_size@, d - a.buddy.base_offset@, sk(a)))]
#[ensures(lg(b, hs))]
pub fn lemma_reserve_err(a: RegionState, b: RegionState, es: Int, hs: Seq<(u64, usize)>) {
    pearlite! {
        lemma_ord_ge(a.format_params.slab_size@ / a.format_params.sector_size@);
        lemma_span_pos(a.buddy.sector_size@, slab_k(a));
        proof_assert! { sk(b) == sk(a) && b.buddy.sector_size == a.buddy.sector_size && b.buddy.base_offset == a.buddy.base_offset };
        proof_assert! { forall<m: BuddyAllocator, fo: Int, d: Int> alloc_post(a.buddy, m, slab_k(a), fo, d)
            && m.sector_size == a.buddy.sector_size && m.free_lists@.len() == a.buddy.free_lists@.len()
            && free_e(m.free_lists@, b.buddy.free_lists@, m.sector_size@, d - a.buddy.base_offset@, sk(a)) ==> {
                lemma_err_rollback(a, b, m, fo, d);
                fdisj(b.buddy.free_lists@, b.buddy.sector_size@) && sl_away(b) } };
        proof_assert! { rg_good(a) && rg_core(a) && rg_acct(a) };
        proof_assert! { b.slabs@.len() == a.slabs@.len() && b.buddy.total_usable_size == a.buddy.total_usable_size };
        proof_assert! { b.slabs@.len() * sk(b) == a.slabs@.len() * sk(a) };
        proof_assert! { rg_tf(b) + b.slabs@.len() * sk(b) == rg_tf(a) + a.slabs@.len() * sk(a) };
        proof_assert! { fdisj(b.buddy.free_lists@, b.buddy.sector_size@) && sl_away(b) && rg_acct(b) };
        proof_assert! { forall<e: Int, p: Int> 0 <= p && p < sc_get(b.size_classes, e).len() ==> {
            lemma_sc_sub_at(a.size_classes, b.size_classes, es, -1, e, p);
            listed(sc_get(a.size_classes, e), sc_get(b.size_classes, e)[p]@) } };
        proof_assert! { sc_ok(b) && rg_ka(b) && rg_core(b) };
        proof_assert! { forall<t: Int, j: Int> slot_live(a, t, j) ==> keep_slot(a, b, t, j) };
        lemma_pv_keep(a, b, a.pending_frees@);
        lemma_pv_keep(a, b, hs)
    }
}

// ============================================================== the other region calls

/// alloc_extent (any path) keeps the history invariant; a success adds a valid handle in a
/// slab of the requested aligned size.
#[logic]
#[requires(lg(a, hs) && rg_inv(b) && rg_frame(a, b) && b.pending_frees == a.pending_frees)]
#[requires(alloc_any(a, b, es, res) && match res { Err(_) => rg_tf(b) == rg_tf(a), Ok(_) => true })]
#[requires(forall<s: u64, i: usize, off: u64> res == Ok((s, i, off)) ==>
    b.slabs@.contains(s@) && i@ < b.slabs@.lookup(s@).bitmap.num_slots@ && slot_bit(b.slabs@.lookup(s@).bitmap, i@))]
#[ensures(match res {
    Ok((s, i, _)) => lg(b, hs.push_back((s, i))) && !slot_live(a, s@, i@) && b.slabs@.lookup(s@).element_size@ == es
        && forall<t: Int, j: Int> slot_live(a, t, j) ==> keep_slot(a, b, t, j),
    Err(_) => lg(b, hs) && forall<t: Int, j: Int> slot_live(a, t, j) ==> keep_slot(a, b, t, j),
})]
pub fn lemma_reserve_good(a: RegionState, b: RegionState, es: Int, res: Result<(u64, usize, u64), EmError>, hs: Seq<(u64, usize)>) {
    pearlite! {
        lemma_alloc_any_read(a, b, es, res);
        match res {
            Ok((s, i, off)) => {
                proof_assert! { (sc_sub(a.size_classes, b.size_classes, es, -1) && b.buddy == a.buddy
                    && listed(sc_get(a.size_classes, es), s@) && a.slabs@.contains(s@)
                    && b.slabs@ == a.slabs@.insert(s@, b.slabs@.lookup(s@))
                    && slot_step(a.slabs@.lookup(s@), b.slabs@.lookup(s@), i@)) ==> {
                        lemma_reserve_exist(a, b, es, s, i, hs);
                        lg(b, hs.push_back((s, i))) && !slot_live(a, s@, i@) && b.slabs@.lookup(s@).element_size@ == es
                        && forall<t: Int, j: Int> slot_live(a, t, j) ==> keep_slot(a, b, t, j) } };
                proof_assert! { forall<fo: Int> sc_sub(a.size_classes, b.size_classes, es, s@)
                    && b.slabs@ == a.slabs@.insert(s@, b.slabs@.lookup(s@))
                    && fresh_one(b.slabs@.lookup(s@), s@, a.format_params.slab_size@, es)
                    && alloc_post(a.buddy, b.buddy, slab_k(a), fo, s@) && rg_tf(b) == rg_tf(a) - sk(a) ==> {
                        lemma_reserve_fresh(a, b, es, s, i, fo, hs);
                        lg(b, hs.push_back((s, i))) && !slot_live(a, s@, i@) && b.slabs@.lookup(s@).element_size@ == es
                        && forall<t: Int, j: Int> slot_live(a, t, j) ==> keep_slot(a, b, t, j) } };
                ()
            },
            Err(_) => { lemma_reserve_err(a, b, es, hs); () },
        }
    }
}

/// publish_slot through a live handle with a real key (region.rs:113-118).
#[logic]
#[requires(lg(a, hs) && subl(hs2, hs, j) && hs[j] == (s, i) && key != FREE_KEY)]
#[requires(rg_inv(b) && rg_frame(a, b) && b.buddy == a.buddy && b.pending_frees == a.pending_frees && b.size_classes == a.size_classes)]
#[requires(a.slabs@.contains(s@) && b.slabs@ == a.slabs@.insert(s@, b.slabs@.lookup(s@)))]
#[requires(b.slabs@.lookup(s@).keys@ == a.slabs@.lookup(s@).keys@.set(i@, key))]
#[requires(b.slabs@.lookup(s@).bitmap == a.slabs@.lookup(s@).bitmap && b.slabs@.lookup(s@).element_size == a.slabs@.lookup(s@).element_size)]
#[requires(b.slabs@.lookup(s@).start_offset == a.slabs@.lookup(s@).start_offset && b.slabs@.lookup(s@).slab_size == a.slabs@.lookup(s@).slab_size)]
#[ensures(lg(b, hs2))]
#[ensures(forall<t: Int, k: Int> slot_live(a, t, k) && (t != s@ || k != i@) ==> keep_slot(a, b, t, k))]
pub fn lemma_publish_good(a: RegionState, b: RegionState, s: u64, i: usize, key: u64, hs: Seq<(u64, usize)>, hs2: Seq<(u64, usize)>, j: Int) {
    pearlite! {
        lemma_pv_sub(a, hs, hs2, j);
        proof_assert! { slot_live(a, s@, i@) };
        proof_assert! { forall<k: Int> b.slabs@.contains(k) == a.slabs@.contains(k) };
        proof_assert! { forall<k: Int> k != s@ ==> b.slabs@.get(k) == a.slabs@.get(k) };
        proof_assert! { b.slabs@.len() == a.slabs@.len() && sk(b) == sk(a) && rg_tf(b) == rg_tf(a) };
        proof_assert! { sl_away(b) && rg_acct(b) && sc_ok(b) };
        proof_assert! { key_alloc(b.slabs@.lookup(s@)) && rg_ka(b) && rg_core(b) };
        proof_assert! { forall<t: Int, k: Int> slot_live(a, t, k) && (t != s@ || k != i@) ==> keep_slot(a, b, t, k) };
        proof_assert! { forall<p: Int> 0 <= p && p < a.pending_frees@.len() ==> a.pending_frees@[p] != (s, i) };
        proof_assert! { forall<p: Int> 0 <= p && p < a.pending_frees@.len() ==> a.pending_frees@[p].0@ != s@ || a.pending_frees@[p].1@ != i@ };
        lemma_pv_keep(a, b, a.pending_frees@);
        proof_assert! { forall<p: Int> 0 <= p && p < hs2.len() ==> hs2[p].0@ != s@ || hs2[p].1@ != i@ };
        lemma_pv_keep(a, b, hs2)
    }
}

/// free_slot through a live handle (abort / drop / publish(FREE_KEY)).
#[logic]
#[requires(lg(a, hs) && subl(hs2, hs, j) && hs[j] == (s, i))]
#[requires(rg_inv(b) && rg_frame(a, b) && b.pending_frees == a.pending_frees)]
#[requires(a.slabs@.get(s@) == Some(sl) && free_case(a, b, s, i, sl))]
#[requires(sl.bitmap.allocated_count@ == 1 ==>
    free_e(a.buddy.free_lists@, b.buddy.free_lists@, a.buddy.sector_size@, s@ - a.buddy.base_offset@, sk(a)))]
#[requires(rg_tf(b) == rg_tf(a) + if sl.bitmap.allocated_count@ == 1 { sk(a) } else { 0 })]
#[ensures(lg(b, hs2) && free_ok(a, s, i))]
#[ensures(forall<t: Int, k: Int> slot_live(a, t, k) && (t != s@ || k != i@) ==> keep_slot(a, b, t, k))]
pub fn lemma_abort_good(a: RegionState, b: RegionState, s: u64, i: usize, sl: Slab, hs: Seq<(u64, usize)>, hs2: Seq<(u64, usize)>, j: Int) {
    pearlite! {
        lemma_pv_sub(a, hs, hs2, j);
        proof_assert! { slot_live(a, s@, i@) && free_ok(a, s, i) };
        proof_assert! { forall<p: Int> 0 <= p && p < a.pending_frees@.len() ==> a.pending_frees@[p] != (s, i) };
        proof_assert! { !inpf(a.pending_frees@, 0, s@, i@) };
        lemma_free_core(a, b, s, i, sl, a.pending_frees@, 0);
        lemma_free_core(a, b, s, i, sl, hs2, 0);
        lemma_free_core_keep(a, b, s, i, sl);
        proof_assert! { forall<p: Int, q: Int> 0 <= p && p < b.pending_frees@.len() && 0 <= q && q < hs2.len() ==> b.pending_frees@[p] != hs2[q] }
    }
}

/// remove_extent_by_offset's success (region.rs:120-149): the published slot is queued.
#[logic]
#[requires(lg(a, hs) && rg_inv(b) && rm_case(a, off, s, i) && rm_post(a, b, s, i))]
#[ensures(lg(b, hs))]
#[ensures(forall<t: Int, k: Int> slot_live(a, t, k) && (t != s@ || k != i@) ==> keep_slot(a, b, t, k))]
pub fn lemma_remove_one(a: RegionState, b: RegionState, off: u64, s: u64, i: usize, hs: Seq<(u64, usize)>) {
    pearlite! {
        proof_assert! { slab_ok(a, a.slabs@.lookup(s@)) && slab_ok(b, b.slabs@.lookup(s@)) };
        proof_assert! { forall<k: Int> b.slabs@.contains(k) == a.slabs@.contains(k) };
        proof_assert! { forall<k: Int> k != s@ ==> b.slabs@.get(k) == a.slabs@.get(k) };
        proof_assert! { b.slabs@.len() == a.slabs@.len() && sk(b) == sk(a) && rg_tf(b) == rg_tf(a) };
        proof_assert! { sl_away(b) && rg_acct(b) && sc_ok(b) };
        proof_assert! { key_alloc(b.slabs@.lookup(s@)) && rg_ka(b) };
        proof_assert! { rg_core(b) };
        proof_assert! { forall<t: Int, k: Int> slot_live(a, t, k) && (t != s@ || k != i@) ==> keep_slot(a, b, t, k) };
        proof_assert! { forall<p: Int> 0 <= p && p < a.pending_frees@.len() ==> a.pending_frees@[p].0@ != s@ || a.pending_frees@[p].1@ != i@ };
        lemma_pv_keep(a, b, a.pending_frees@);
        proof_assert! { forall<p: Int> 0 <= p && p < hs.len() ==> hs[p].0@ != s@ || hs[p].1@ != i@ };
        lemma_pv_keep(a, b, hs);
        proof_assert! { forall<p: Int> 0 <= p && p < b.pending_frees@.len() ==>
            b.pending_frees@[p] == if p < a.pending_frees@.len() { a.pending_frees@[p] } else { (s, i) } };
        proof_assert! { pv(b, b.pending_frees@) };
        proof_assert! { forall<p: Int> 0 <= p && p < hs.len() ==> hs[p] != (s, i) }
    }
}

/// flush_pending_frees (checkpoint, lib.rs:316-318) keeps the history invariant.
#[logic]
#[requires(lg(a, hs) && fl_spec(a, b) && b.pending_frees@.len() == 0)]
#[ensures(lg(b, hs))]
pub fn lemma_flush_good(a: RegionState, b: RegionState, hs: Seq<(u64, usize)>) {
    pearlite! {
        lemma_fl_spec_elim(a, b);
        proof_assert! { forall<p: Int> 0 <= p && p < hs.len() ==> !inpend(a.pending_frees@, hs[p].0@, hs[p].1@) };
        lemma_pv_keep(a, b, hs)
    }
}

// ======================================================================= the history

/// One call on a region, as reached from the public API: `reserve_extent` →
/// alloc_extent; `WriteHandle::publish` (handle `j`, the reservation key) → publish_slot,
/// or free_slot for the FREE_KEY sentinel (lib.rs:597-616); `WriteHandle::abort` / drop →
/// free_slot (lib.rs:618-621); `remove_extent` → remove_extent_by_offset; `checkpoint` →
/// flush_pending_frees (lib.rs:316-318). A handle is consumed by publish / abort.
#[derive(::core::clone::Clone, ::core::marker::Copy)]
pub enum LOp {
    Reserve(u32),
    Publish(usize, u64),
    Abort(usize),
    Remove(u64),
    Flush,
}

/// The documented-use precondition: a size that fits u32 after sector alignment, and a
/// handle that is live (not yet published/aborted).
#[logic(open)]
pub fn lop_pre(r: RegionState, hs: Seq<(u64, usize)>, op: LOp) -> bool {
    pearlite! {
        match op {
            LOp::Reserve(sz) => a_231ab0(sz@) && a_3783f8(sz@, r.format_params.sector_size@),
            LOp::Publish(j, _) => j@ < hs.len(),
            LOp::Abort(j) => j@ < hs.len(),
            LOp::Remove(_) => true,
            LOp::Flush => true,
        }
    }
}

/// Apply one call: the history invariant and the region geometry are preserved.
#[requires(lg(*r, hs@) && lop_pre(*r, hs@, op))]
#[ensures(lg(^r, (^hs)@) && rg_frame(*r, ^r))]
pub fn apply_lop(r: &mut RegionState, hs: &mut Vec<(u64, usize)>, op: LOp) {
    let a = snapshot! { *r };
    let h0 = snapshot! { hs@ };
    match op {
        LOp::Reserve(sz) => {
            let res = r.alloc_extent(sz);
            proof_assert! { lemma_reserve_good(*a, *r, align_l(sz@, a.format_params.sector_size@), res, *h0); true };
            match res {
                Ok((s, i, _)) => hs.push((s, i)),
                Err(_) => {}
            }
        }
        LOp::Publish(j, key) => {
            let (s, i) = hs[j];
            proof_assert! { h0[j@] == (s, i) && slot_live(*a, s@, i@) };
            let sl = snapshot! { a.slabs@.lookup(s@) };
            if key == FREE_KEY {
                r.free_slot(s, i);
                let _ = hs.remove(j);
                proof_assert! { lemma_subl(hs@, *h0, j@); lemma_abort_good(*a, *r, s, i, *sl, *h0, hs@, j@); true };
            } else {
                r.publish_slot(s, i, key);
                let _ = hs.remove(j);
                proof_assert! { lemma_subl(hs@, *h0, j@); lemma_publish_good(*a, *r, s, i, key, *h0, hs@, j@); true };
            }
        }
        LOp::Abort(j) => {
            let (s, i) = hs[j];
            proof_assert! { h0[j@] == (s, i) && slot_live(*a, s@, i@) };
            let sl = snapshot! { a.slabs@.lookup(s@) };
            r.free_slot(s, i);
            let _ = hs.remove(j);
            proof_assert! { lemma_subl(hs@, *h0, j@); lemma_abort_good(*a, *r, s, i, *sl, *h0, hs@, j@); true };
        }
        LOp::Remove(off) => {
            let res = r.remove_extent_by_offset(off);
            proof_assert! { res == Ok(()) ==> forall<s: u64, i: usize> rm_case(*a, off, s, i) && rm_post(*a, *r, s, i) ==> {
                lemma_remove_one(*a, *r, off, s, i, *h0); lg(*r, *h0) } };
        }
        LOp::Flush => {
            r.flush_pending_frees();
            proof_assert! { lemma_flush_good(*a, *r, *h0); true };
        }
    }
}

// ============================================================= the two lifecycle events

/// `format()`'s fresh region (lib.rs:465-466): `RegionState::new(BuddyAllocator::new(..))`.
#[requires(p2(fp.sector_size@))]
#[requires(rg_in(base@, size@, fp))]
#[ensures(lg(result, Seq::empty()) && result.buddy.base_offset == base && result.buddy.total_usable_size == size)]
#[ensures(result.format_params == fp && result.slabs@ == FMap::empty())]
#[ensures(rg_tf(result) == (size@ / fp.sector_size@) * fp.sector_size@)]
pub fn fresh_region(base: u64, size: u64, fp: FormatParams) -> RegionState {
    proof_assert! { lemma_rg_in(base@, size@, fp); rg_hd(base@, size@, fp) };
    let b = BuddyAllocator::new(base, size, fp.sector_size);
    proof_assert! { lemma_new_e_elim(b.free_lists@, fp.sector_size@, (size@ / fp.sector_size@) * fp.sector_size@); true };
    proof_assert! { lemma_span_pos(fp.sector_size@, b.max_order@); b.max_order@ == 0 ==> span(fp.sector_size@, 0) == fp.sector_size@ };
    let r = RegionState::new(b, fp);
    proof_assert! { rg_params(r) && rg_buddy(r) && rg_slabs(r) && rg_inv(r) };
    proof_assert! { r.slabs@.len() == 0 && rg_acct(r) && sl_away(r) && sc_ok(r) && rg_ka(r) };
    proof_assert! { rg_core(r) && pv(r, r.pending_frees@) && pv(r, Seq::empty()) };
    r
}

/// `initialize()`'s rebuilt region (lib.rs:552-565) from distinct, well-formed descriptors.
#[requires(p2(fp.sector_size@))]
#[requires(rg_in(base@, size@, fp))]
#[requires(forall<p: Int> 0 <= p && p < descs@.len() ==> desc_ok_a(descs@[p], base@, size@, fp))]
#[requires(a_980f61_list(descs@, base@, size@))]
#[ensures(lg(result, Seq::empty()) && rebuilt_all(result, descs@))]
#[ensures(result.format_params == fp && result.buddy.base_offset == base && result.buddy.total_usable_size == size)]
pub fn rebuilt_region(base: u64, size: u64, fp: FormatParams, descs: &Vec<SlabDescriptor>) -> RegionState {
    proof_assert! { lemma_rg_in(base@, size@, fp); rg_hd(base@, size@, fp) };
    proof_assert! { forall<p: Int> 0 <= p && p < descs@.len() ==> { lemma_desc_ok_a(descs@[p], base@, size@, fp); desc_ok(descs@[p], base@, size@, fp) } };
    proof_assert! { forall<p: Int> 0 <= p && p < descs@.len() ==> descs@[p].slab_size@ > 0 };
    proof_assert! { lemma_starts_distinct(descs@, base@, size@); starts_distinct(descs@) };
    let r = rebuild_region(base, size, fp, descs);
    proof_assert! { lemma_rg_good_o(r); rg_good(r) && pv(r, Seq::empty()) };
    r
}

// ===================================================================== shared drivers

/// One call on a reachable region plus the two lifecycle constructions: the history
/// invariant holds for all three resulting regions.
#[requires(lg(*r, hs@) && lop_pre(*r, hs@, op))]
#[requires(p2(fp.sector_size@))]
#[requires(rg_in(base@, size@, fp))]
#[requires(forall<p: Int> 0 <= p && p < descs@.len() ==> desc_ok_a(descs@[p], base@, size@, fp))]
#[requires(a_980f61_list(descs@, base@, size@))]
#[ensures(lg(^r, (^hs)@) && rg_frame(*r, ^r))]
#[ensures(lg(result.0, Seq::empty()) && lg(result.1, Seq::empty()) && rebuilt_all(result.1, descs@))]
#[ensures(result.0.buddy.base_offset == base && result.0.buddy.total_usable_size == size && result.0.format_params == fp)]
#[ensures(result.1.buddy.base_offset == base && result.1.buddy.total_usable_size == size && result.1.format_params == fp)]
#[ensures(result.0.slabs@ == FMap::empty())]
pub fn all3(r: &mut RegionState, hs: &mut Vec<(u64, usize)>, op: LOp, base: u64, size: u64, fp: FormatParams, descs: &Vec<SlabDescriptor>)
    -> (RegionState, RegionState) {
    proof_assert! { lemma_rg_in(base@, size@, fp); rg_hd(base@, size@, fp) };
    proof_assert! { forall<p: Int> 0 <= p && p < descs@.len() ==> { lemma_desc_ok_a(descs@[p], base@, size@, fp); desc_ok(descs@[p], base@, size@, fp) } };
    proof_assert! { forall<p: Int> 0 <= p && p < descs@.len() ==> descs@[p].slab_size@ > 0 };
    proof_assert! { lemma_starts_distinct(descs@, base@, size@); starts_distinct(descs@) };
    apply_lop(r, hs, op);
    let f = fresh_region(base, size, fp);
    let g = rebuilt_region(base, size, fp, descs);
    (f, g)
}

/// Every slab's bytes avoid every free block.
#[logic(open)]
pub fn slabs_off_free(r: RegionState) -> bool {
    pearlite! {
        forall<k: Int> r.slabs@.contains(k) ==>
            faway(r.buddy.free_lists@, r.buddy.sector_size@, k - r.buddy.base_offset@, r.slabs@.lookup(k).slab_size@)
    }
}

#[logic]
#[requires(rg_good(r))]
#[ensures(slabs_off_free(r))]
pub fn lemma_slabs_off_free(r: RegionState) {
    pearlite! {
        lemma_slab_fits(r);
        proof_assert! { forall<o: Int> 0 <= o ==> { lemma_span_pos(r.buddy.sector_size@, o); span(r.buddy.sector_size@, o) > 0 } };
        proof_assert! { forall<k: Int> r.slabs@.contains(k) ==> slab_ok(r, r.slabs@.lookup(k)) && r.slabs@.lookup(k).slab_size@ > 0 };
        proof_assert! { forall<k: Int> r.slabs@.contains(k) ==> {
            lemma_away_sub(r.buddy.free_lists@, r.buddy.sector_size@, k - r.buddy.base_offset@, sk(r), k - r.buddy.base_offset@, r.slabs@.lookup(k).slab_size@);
            faway(r.buddy.free_lists@, r.buddy.sector_size@, k - r.buddy.base_offset@, r.slabs@.lookup(k).slab_size@) } }
    }
}

/// What every `buddy.free` call of free_slot (region.rs:105) passes: an allocated slab
/// block of this region — at or after the region base, aligned, inside the region, of the
/// slab size it was allocated with, and on no free list.
#[logic(open)]
pub fn free_pre_ok(r: RegionState) -> bool {
    pearlite! {
        forall<k: Int> r.slabs@.contains(k) ==>
            k >= r.buddy.base_offset@
            && (k - r.buddy.base_offset@) % sk(r) == 0
            && k - r.buddy.base_offset@ + sk(r) <= r.buddy.total_usable_size@
            && r.slabs@.lookup(k).slab_size == r.format_params.slab_size
            && ord(r.slabs@.lookup(k).slab_size@ / r.buddy.sector_size@) == slab_k(r)
            && faway(r.buddy.free_lists@, r.buddy.sector_size@, k - r.buddy.base_offset@, sk(r))
    }
}

#[logic]
#[requires(rg_good(r))]
#[ensures(free_pre_ok(r))]
pub fn lemma_free_pre_ok(r: RegionState) {
    pearlite! { proof_assert! { forall<k: Int> r.slabs@.contains(k) ==> slab_ok(r, r.slabs@.lookup(k)) } }
}

/// Distinct live slots occupy disjoint bytes.
#[logic(open)]
pub fn no_overlap(r: RegionState) -> bool {
    pearlite! {
        forall<t1: Int, j1: Int, t2: Int, j2: Int> slot_live(r, t1, j1) && slot_live(r, t2, j2) && (t1 != t2 || j1 != j2) ==>
            disj(slot_off(r.slabs@.lookup(t1), j1), r.slabs@.lookup(t1).element_size@,
                 slot_off(r.slabs@.lookup(t2), j2), r.slabs@.lookup(t2).element_size@)
    }
}

/// Each live slot lies inside its slab's block.
#[logic]
#[requires(rg_inv(r) && slot_live(r, t, j))]
#[ensures(t <= slot_off(r.slabs@.lookup(t), j))]
#[ensures(slot_off(r.slabs@.lookup(t), j) + r.slabs@.lookup(t).element_size@ <= t + sk(r))]
#[ensures(slot_off(r.slabs@.lookup(t), j) + r.slabs@.lookup(t).element_size@ <= t + r.slabs@.lookup(t).slab_size@)]
pub fn lemma_slot_in_block(r: RegionState, t: Int, j: Int) {
    pearlite! {
        proof_assert! { slab_ok(r, r.slabs@.lookup(t)) && r.slabs@.lookup(t).start_offset@ == t };
        lemma_slot_inside(r.slabs@.lookup(t), j);
        lemma_slab_fits(r)
    }
}

#[logic]
#[requires(rg_inv(r))]
#[ensures(no_overlap(r))]
pub fn lemma_no_overlap(r: RegionState) {
    pearlite! {
        proof_assert! { forall<t1: Int, j1: Int, t2: Int, j2: Int> slot_live(r, t1, j1) && slot_live(r, t2, j2) && t1 != t2 ==> {
            lemma_slot_in_block(r, t1, j1); lemma_slot_in_block(r, t2, j2);
            lemma_slab_blocks_disj(r, t1, t2);
            disj(slot_off(r.slabs@.lookup(t1), j1), r.slabs@.lookup(t1).element_size@,
                 slot_off(r.slabs@.lookup(t2), j2), r.slabs@.lookup(t2).element_size@) } };
        proof_assert! { forall<t: Int, j1: Int, j2: Int> slot_live(r, t, j1) && slot_live(r, t, j2) && j1 < j2 ==> {
            proof_assert! { slab_inv(r.slabs@.lookup(t)) };
            lemma_slots_disjoint(r.slabs@.lookup(t), j1, j2);
            disj(slot_off(r.slabs@.lookup(t), j1), r.slabs@.lookup(t).element_size@,
                 slot_off(r.slabs@.lookup(t), j2), r.slabs@.lookup(t).element_size@) } }
    }
}

/// Free bytes never exceed the usable size (accounting).
#[logic]
#[requires(rg_good(r))]
#[ensures(0 <= rg_tf(r) && rg_tf(r) <= r.buddy.total_usable_size@)]
pub fn lemma_tf_le(r: RegionState) {
    pearlite! {
        lemma_tf_mono(r.buddy.free_lists@, r.buddy.sector_size@, r.buddy.free_lists@.len());
        proof_assert! { 0 <= tf(r.buddy.free_lists@, r.buddy.sector_size@, r.buddy.free_lists@.len()) };
        proof_assert! { rg_core(r) && rg_acct(r) };
        lemma_ord_ge(r.format_params.slab_size@ / r.format_params.sector_size@);
        lemma_span_pos(r.buddy.sector_size@, slab_k(r));
        lemma_mul_le(0, r.slabs@.len(), sk(r));
        proof_assert! { 0 <= r.slabs@.len() * sk(r) };
        lemma_divmod(r.buddy.total_usable_size@, r.buddy.sector_size@);
        proof_assert! { (r.buddy.total_usable_size@ / r.buddy.sector_size@) * r.buddy.sector_size@ <= r.buddy.total_usable_size@ }
    }
}

// ============================================================== the scored modules

/// **EM-BUDDY-FREE-DISJOINT** — "Free blocks recorded by a region allocator never overlap
/// each other." LEVEL-2 under D-RANGE-FR-002-217ace (power-of-two sector size): holds after
/// every call on a reachable region and for the regions format() and initialize() build.
#[requires(lg(*r, hs@) && lop_pre(*r, hs@, op))]
#[requires(p2(fp.sector_size@))]
#[requires(rg_in(base@, size@, fp))]
#[requires(forall<p: Int> 0 <= p && p < descs@.len() ==> desc_ok_a(descs@[p], base@, size@, fp))]
#[requires(a_980f61_list(descs@, base@, size@))]
#[ensures(fdisj((^r).buddy.free_lists@, (^r).buddy.sector_size@))]
#[ensures(fdisj(result.0.buddy.free_lists@, result.0.buddy.sector_size@))]
#[ensures(fdisj(result.1.buddy.free_lists@, result.1.buddy.sector_size@))]
pub fn verify_em_buddy_free_disjoint(r: &mut RegionState, hs: &mut Vec<(u64, usize)>, op: LOp, base: u64, size: u64,
    fp: FormatParams, descs: &Vec<SlabDescriptor>) -> (RegionState, RegionState) {
    proof_assert! { lemma_rg_in(base@, size@, fp); rg_hd(base@, size@, fp) };
    proof_assert! { forall<p: Int> 0 <= p && p < descs@.len() ==> { lemma_desc_ok_a(descs@[p], base@, size@, fp); desc_ok(descs@[p], base@, size@, fp) } };
    proof_assert! { forall<p: Int> 0 <= p && p < descs@.len() ==> descs@[p].slab_size@ > 0 };
    proof_assert! { lemma_starts_distinct(descs@, base@, size@); starts_distinct(descs@) };
    all3(r, hs, op, base, size, fp, descs)
}

/// Mutant: claims two free blocks of the region overlap.
/// LEVEL-3 (phase D-B2): mutant with EXACTLY the property's requires and body; it claims the
/// negation of the property's first postcondition.
#[requires(lg(*r, hs@) && lop_pre(*r, hs@, op))]
#[requires(p2(fp.sector_size@))]
#[requires(rg_in(base@, size@, fp))]
#[requires(forall<p: Int> 0 <= p && p < descs@.len() ==> desc_ok_a(descs@[p], base@, size@, fp))]
#[requires(a_980f61_list(descs@, base@, size@))]
#[ensures(!(fdisj((^r).buddy.free_lists@, (^r).buddy.sector_size@)))]
pub fn verify_em_buddy_free_disjoint__mutant(r: &mut RegionState, hs: &mut Vec<(u64, usize)>, op: LOp, base: u64, size: u64,
    fp: FormatParams, descs: &Vec<SlabDescriptor>) -> (RegionState, RegionState) {
    proof_assert! { lemma_rg_in(base@, size@, fp); rg_hd(base@, size@, fp) };
    proof_assert! { forall<p: Int> 0 <= p && p < descs@.len() ==> { lemma_desc_ok_a(descs@[p], base@, size@, fp); desc_ok(descs@[p], base@, size@, fp) } };
    proof_assert! { forall<p: Int> 0 <= p && p < descs@.len() ==> descs@[p].slab_size@ > 0 };
    proof_assert! { lemma_starts_distinct(descs@, base@, size@); starts_distinct(descs@) };
    all3(r, hs, op, base, size, fp, descs)
}

/// **EM-REGION-SLABS-NOT-FREE** — "The space occupied by a slab is never simultaneously
/// recorded as free in the region's allocator."
#[requires(lg(*r, hs@) && lop_pre(*r, hs@, op))]
#[requires(p2(fp.sector_size@))]
#[requires(rg_in(base@, size@, fp))]
#[requires(forall<p: Int> 0 <= p && p < descs@.len() ==> desc_ok_a(descs@[p], base@, size@, fp))]
#[requires(a_980f61_list(descs@, base@, size@))]
#[ensures(slabs_off_free(^r))]
#[ensures(slabs_off_free(result.0) && slabs_off_free(result.1))]
pub fn verify_em_region_slabs_not_free(r: &mut RegionState, hs: &mut Vec<(u64, usize)>, op: LOp, base: u64, size: u64,
    fp: FormatParams, descs: &Vec<SlabDescriptor>) -> (RegionState, RegionState) {
    proof_assert! { lemma_rg_in(base@, size@, fp); rg_hd(base@, size@, fp) };
    proof_assert! { forall<p: Int> 0 <= p && p < descs@.len() ==> { lemma_desc_ok_a(descs@[p], base@, size@, fp); desc_ok(descs@[p], base@, size@, fp) } };
    proof_assert! { forall<p: Int> 0 <= p && p < descs@.len() ==> descs@[p].slab_size@ > 0 };
    proof_assert! { lemma_starts_distinct(descs@, base@, size@); starts_distinct(descs@) };
    let res = all3(r, hs, op, base, size, fp, descs);
    proof_assert! { lemma_slabs_off_free(*r); lemma_slabs_off_free(res.0); lemma_slabs_off_free(res.1); true };
    res
}

/// Mutant: claims some slab's bytes are on a free list.
/// LEVEL-3 (phase D-B2): mutant with EXACTLY the property's requires and body; it claims the
/// negation of the property's first postcondition.
#[requires(lg(*r, hs@) && lop_pre(*r, hs@, op))]
#[requires(p2(fp.sector_size@))]
#[requires(rg_in(base@, size@, fp))]
#[requires(forall<p: Int> 0 <= p && p < descs@.len() ==> desc_ok_a(descs@[p], base@, size@, fp))]
#[requires(a_980f61_list(descs@, base@, size@))]
#[ensures(!(slabs_off_free(^r)))]
pub fn verify_em_region_slabs_not_free__mutant(r: &mut RegionState, hs: &mut Vec<(u64, usize)>, op: LOp, base: u64, size: u64,
    fp: FormatParams, descs: &Vec<SlabDescriptor>) -> (RegionState, RegionState) {
    proof_assert! { lemma_rg_in(base@, size@, fp); rg_hd(base@, size@, fp) };
    proof_assert! { forall<p: Int> 0 <= p && p < descs@.len() ==> { lemma_desc_ok_a(descs@[p], base@, size@, fp); desc_ok(descs@[p], base@, size@, fp) } };
    proof_assert! { forall<p: Int> 0 <= p && p < descs@.len() ==> descs@[p].slab_size@ > 0 };
    proof_assert! { lemma_starts_distinct(descs@, base@, size@); starts_distinct(descs@) };
    let res = all3(r, hs, op, base, size, fp, descs);
    proof_assert! { lemma_slabs_off_free(*r); lemma_slabs_off_free(res.0); lemma_slabs_off_free(res.1); true };
    res
}

/// **EM-BUDDY-FREE-PRE** — "A block is only freed if it is currently allocated from this
/// region, with the size it was allocated with and an offset at or after the region base."
/// free_slot frees only slab blocks (region.rs:104-107): in every reachable region each slab
/// is an aligned order-K block at or after the base, of the configured slab size, on no free
/// list. (The other caller, alloc_extent's zero-slot rollback, frees the block alloc just
/// returned — `alloc_any`'s rollback case.)
#[requires(lg(*r, hs@) && lop_pre(*r, hs@, op))]
#[requires(p2(fp.sector_size@))]
#[requires(rg_in(base@, size@, fp))]
#[requires(forall<p: Int> 0 <= p && p < descs@.len() ==> desc_ok_a(descs@[p], base@, size@, fp))]
#[requires(a_980f61_list(descs@, base@, size@))]
#[ensures(free_pre_ok(*r) && free_pre_ok(^r))]
#[ensures(free_pre_ok(result.0) && free_pre_ok(result.1))]
pub fn verify_em_buddy_free_pre(r: &mut RegionState, hs: &mut Vec<(u64, usize)>, op: LOp, base: u64, size: u64,
    fp: FormatParams, descs: &Vec<SlabDescriptor>) -> (RegionState, RegionState) {
    proof_assert! { lemma_rg_in(base@, size@, fp); rg_hd(base@, size@, fp) };
    proof_assert! { forall<p: Int> 0 <= p && p < descs@.len() ==> { lemma_desc_ok_a(descs@[p], base@, size@, fp); desc_ok(descs@[p], base@, size@, fp) } };
    proof_assert! { forall<p: Int> 0 <= p && p < descs@.len() ==> descs@[p].slab_size@ > 0 };
    proof_assert! { lemma_starts_distinct(descs@, base@, size@); starts_distinct(descs@) };
    proof_assert! { lemma_free_pre_ok(*r); true };
    let res = all3(r, hs, op, base, size, fp, descs);
    proof_assert! { lemma_free_pre_ok(*r); lemma_free_pre_ok(res.0); lemma_free_pre_ok(res.1); true };
    res
}

/// Mutant: claims a slab block is a free block.
/// LEVEL-3 (phase D-B2): mutant with EXACTLY the property's requires and body; it claims the
/// negation of the property's first postcondition.
#[requires(lg(*r, hs@) && lop_pre(*r, hs@, op))]
#[requires(p2(fp.sector_size@))]
#[requires(rg_in(base@, size@, fp))]
#[requires(forall<p: Int> 0 <= p && p < descs@.len() ==> desc_ok_a(descs@[p], base@, size@, fp))]
#[requires(a_980f61_list(descs@, base@, size@))]
#[ensures(!(free_pre_ok(*r) && free_pre_ok(^r)))]
pub fn verify_em_buddy_free_pre__mutant(r: &mut RegionState, hs: &mut Vec<(u64, usize)>, op: LOp, base: u64, size: u64,
    fp: FormatParams, descs: &Vec<SlabDescriptor>) -> (RegionState, RegionState) {
    proof_assert! { lemma_rg_in(base@, size@, fp); rg_hd(base@, size@, fp) };
    proof_assert! { forall<p: Int> 0 <= p && p < descs@.len() ==> { lemma_desc_ok_a(descs@[p], base@, size@, fp); desc_ok(descs@[p], base@, size@, fp) } };
    proof_assert! { forall<p: Int> 0 <= p && p < descs@.len() ==> descs@[p].slab_size@ > 0 };
    proof_assert! { lemma_starts_distinct(descs@, base@, size@); starts_distinct(descs@) };
    proof_assert! { lemma_free_pre_ok(*r); true };
    let res = all3(r, hs, op, base, size, fp, descs);
    proof_assert! { lemma_free_pre_ok(*r); lemma_free_pre_ok(res.0); lemma_free_pre_ok(res.1); true };
    res
}

/// **EM-USED-NO-UNDERFLOW** — "In every region the free space tracked by the allocator never
/// exceeds the region's usable size, so computing used bytes never underflows."
/// `total_usable_size - total_free()` (lib.rs:704) on every reachable region.
#[requires(lg(*r, hs@) && lop_pre(*r, hs@, op))]
#[requires(p2(fp.sector_size@))]
#[requires(rg_in(base@, size@, fp))]
#[requires(forall<p: Int> 0 <= p && p < descs@.len() ==> desc_ok_a(descs@[p], base@, size@, fp))]
#[requires(a_980f61_list(descs@, base@, size@))]
#[ensures(result.0@ <= (^r).buddy.total_usable_size@ && result.0@ == rg_tf(^r))]
#[ensures(result.1@ <= size@ && result.2@ <= size@)]
pub fn verify_em_used_no_underflow(r: &mut RegionState, hs: &mut Vec<(u64, usize)>, op: LOp, base: u64, size: u64,
    fp: FormatParams, descs: &Vec<SlabDescriptor>) -> (u64, u64, u64) {
    proof_assert! { lemma_rg_in(base@, size@, fp); rg_hd(base@, size@, fp) };
    proof_assert! { forall<p: Int> 0 <= p && p < descs@.len() ==> { lemma_desc_ok_a(descs@[p], base@, size@, fp); desc_ok(descs@[p], base@, size@, fp) } };
    proof_assert! { forall<p: Int> 0 <= p && p < descs@.len() ==> descs@[p].slab_size@ > 0 };
    proof_assert! { lemma_starts_distinct(descs@, base@, size@); starts_distinct(descs@) };
    let (f, g) = all3(r, hs, op, base, size, fp, descs);
    proof_assert! { lemma_tf_le(*r); rg_tf(*r) <= r.buddy.total_usable_size@ && rg_inv(*r) };
    proof_assert! { lemma_tf_le(f); rg_tf(f) <= f.buddy.total_usable_size@ && rg_inv(f) };
    proof_assert! { lemma_tf_le(g); rg_tf(g) <= g.buddy.total_usable_size@ && rg_inv(g) };
    let a = r.buddy.total_free();
    let b = f.buddy.total_free();
    let c = g.buddy.total_free();
    let _used = r.buddy.total_usable_size() - a; // lib.rs:704 — no underflow
    (a, b, c)
}

/// Mutant: claims the free space exceeds the usable size.
/// LEVEL-3 (phase D-B2): mutant with EXACTLY the property's requires and body; it claims the
/// negation of the property's first postcondition.
#[requires(lg(*r, hs@) && lop_pre(*r, hs@, op))]
#[requires(p2(fp.sector_size@))]
#[requires(rg_in(base@, size@, fp))]
#[requires(forall<p: Int> 0 <= p && p < descs@.len() ==> desc_ok_a(descs@[p], base@, size@, fp))]
#[requires(a_980f61_list(descs@, base@, size@))]
#[ensures(!(result.0@ <= (^r).buddy.total_usable_size@ && result.0@ == rg_tf(^r)))]
pub fn verify_em_used_no_underflow__mutant(r: &mut RegionState, hs: &mut Vec<(u64, usize)>, op: LOp, base: u64, size: u64,
    fp: FormatParams, descs: &Vec<SlabDescriptor>) -> (u64, u64, u64) {
    proof_assert! { lemma_rg_in(base@, size@, fp); rg_hd(base@, size@, fp) };
    proof_assert! { forall<p: Int> 0 <= p && p < descs@.len() ==> { lemma_desc_ok_a(descs@[p], base@, size@, fp); desc_ok(descs@[p], base@, size@, fp) } };
    proof_assert! { forall<p: Int> 0 <= p && p < descs@.len() ==> descs@[p].slab_size@ > 0 };
    proof_assert! { lemma_starts_distinct(descs@, base@, size@); starts_distinct(descs@) };
    let (f, g) = all3(r, hs, op, base, size, fp, descs);
    proof_assert! { lemma_tf_le(*r); rg_tf(*r) <= r.buddy.total_usable_size@ && rg_inv(*r) };
    proof_assert! { lemma_tf_le(f); rg_tf(f) <= f.buddy.total_usable_size@ && rg_inv(f) };
    proof_assert! { lemma_tf_le(g); rg_tf(g) <= g.buddy.total_usable_size@ && rg_inv(g) };
    let a = r.buddy.total_free();
    let b = f.buddy.total_free();
    let c = g.buddy.total_free();
    let _used = r.buddy.total_usable_size() - a; // lib.rs:704 — no underflow
    (a, b, c)
}

/// **EM-INV-KEY-IMPLIES-ALLOCATED** — "Any slot whose key entry is not the FREE_KEY sentinel
/// (u64::MAX) is marked allocated in its slab's allocation bitmap."
#[requires(lg(*r, hs@) && lop_pre(*r, hs@, op))]
#[requires(p2(fp.sector_size@))]
#[requires(rg_in(base@, size@, fp))]
#[requires(forall<p: Int> 0 <= p && p < descs@.len() ==> desc_ok_a(descs@[p], base@, size@, fp))]
#[requires(a_980f61_list(descs@, base@, size@))]
#[ensures(rg_ka(^r) && rg_ka(result.0) && rg_ka(result.1))]
pub fn verify_em_inv_key_implies_allocated(r: &mut RegionState, hs: &mut Vec<(u64, usize)>, op: LOp, base: u64, size: u64,
    fp: FormatParams, descs: &Vec<SlabDescriptor>) -> (RegionState, RegionState) {
    proof_assert! { lemma_rg_in(base@, size@, fp); rg_hd(base@, size@, fp) };
    proof_assert! { forall<p: Int> 0 <= p && p < descs@.len() ==> { lemma_desc_ok_a(descs@[p], base@, size@, fp); desc_ok(descs@[p], base@, size@, fp) } };
    proof_assert! { forall<p: Int> 0 <= p && p < descs@.len() ==> descs@[p].slab_size@ > 0 };
    proof_assert! { lemma_starts_distinct(descs@, base@, size@); starts_distinct(descs@) };
    all3(r, hs, op, base, size, fp, descs)
}

/// Mutant: claims some non-FREE key sits on an unallocated slot.
/// LEVEL-3 (phase D-B2): mutant with EXACTLY the property's requires and body; it claims the
/// negation of the property's first postcondition.
#[requires(lg(*r, hs@) && lop_pre(*r, hs@, op))]
#[requires(p2(fp.sector_size@))]
#[requires(rg_in(base@, size@, fp))]
#[requires(forall<p: Int> 0 <= p && p < descs@.len() ==> desc_ok_a(descs@[p], base@, size@, fp))]
#[requires(a_980f61_list(descs@, base@, size@))]
#[ensures(!(rg_ka(^r) && rg_ka(result.0) && rg_ka(result.1)))]
pub fn verify_em_inv_key_implies_allocated__mutant(r: &mut RegionState, hs: &mut Vec<(u64, usize)>, op: LOp, base: u64, size: u64,
    fp: FormatParams, descs: &Vec<SlabDescriptor>) -> (RegionState, RegionState) {
    proof_assert! { lemma_rg_in(base@, size@, fp); rg_hd(base@, size@, fp) };
    proof_assert! { forall<p: Int> 0 <= p && p < descs@.len() ==> { lemma_desc_ok_a(descs@[p], base@, size@, fp); desc_ok(descs@[p], base@, size@, fp) } };
    proof_assert! { forall<p: Int> 0 <= p && p < descs@.len() ==> descs@[p].slab_size@ > 0 };
    proof_assert! { lemma_starts_distinct(descs@, base@, size@); starts_distinct(descs@) };
    all3(r, hs, op, base, size, fp, descs)
}

/// **EM-REGION-PENDING-FREES-VALID** — "Every queued removal refers to an existing slab and a
/// slot that is still marked allocated and carries the free marker as its key, and no slot is
/// queued twice."
#[requires(lg(*r, hs@) && lop_pre(*r, hs@, op))]
#[requires(p2(fp.sector_size@))]
#[requires(rg_in(base@, size@, fp))]
#[requires(forall<p: Int> 0 <= p && p < descs@.len() ==> desc_ok_a(descs@[p], base@, size@, fp))]
#[requires(a_980f61_list(descs@, base@, size@))]
#[ensures(pv(^r, (^r).pending_frees@))]
#[ensures(pv(result.0, result.0.pending_frees@) && pv(result.1, result.1.pending_frees@))]
pub fn verify_em_region_pending_frees_valid(r: &mut RegionState, hs: &mut Vec<(u64, usize)>, op: LOp, base: u64, size: u64,
    fp: FormatParams, descs: &Vec<SlabDescriptor>) -> (RegionState, RegionState) {
    proof_assert! { lemma_rg_in(base@, size@, fp); rg_hd(base@, size@, fp) };
    proof_assert! { forall<p: Int> 0 <= p && p < descs@.len() ==> { lemma_desc_ok_a(descs@[p], base@, size@, fp); desc_ok(descs@[p], base@, size@, fp) } };
    proof_assert! { forall<p: Int> 0 <= p && p < descs@.len() ==> descs@[p].slab_size@ > 0 };
    proof_assert! { lemma_starts_distinct(descs@, base@, size@); starts_distinct(descs@) };
    all3(r, hs, op, base, size, fp, descs)
}

/// Mutant: claims a queued slot carries a published key.
/// LEVEL-3 (phase D-B2): mutant with EXACTLY the property's requires and body; it claims the
/// negation of the property's first postcondition.
#[requires(lg(*r, hs@) && lop_pre(*r, hs@, op))]
#[requires(p2(fp.sector_size@))]
#[requires(rg_in(base@, size@, fp))]
#[requires(forall<p: Int> 0 <= p && p < descs@.len() ==> desc_ok_a(descs@[p], base@, size@, fp))]
#[requires(a_980f61_list(descs@, base@, size@))]
#[ensures(!(pv(^r, (^r).pending_frees@)))]
pub fn verify_em_region_pending_frees_valid__mutant(r: &mut RegionState, hs: &mut Vec<(u64, usize)>, op: LOp, base: u64, size: u64,
    fp: FormatParams, descs: &Vec<SlabDescriptor>) -> (RegionState, RegionState) {
    proof_assert! { lemma_rg_in(base@, size@, fp); rg_hd(base@, size@, fp) };
    proof_assert! { forall<p: Int> 0 <= p && p < descs@.len() ==> { lemma_desc_ok_a(descs@[p], base@, size@, fp); desc_ok(descs@[p], base@, size@, fp) } };
    proof_assert! { forall<p: Int> 0 <= p && p < descs@.len() ==> descs@[p].slab_size@ > 0 };
    proof_assert! { lemma_starts_distinct(descs@, base@, size@); starts_distinct(descs@) };
    all3(r, hs, op, base, size, fp, descs)
}

// ======================================================= handle-level calls (lib.rs:597-621)

/// The abort closure (lib.rs:618-621) through live handle `j`: `free_slot(slab_start, slot)`.
#[requires(lg(*r, hs@) && j@ < hs@.len())]
#[ensures(lg(^r, (^hs)@) && rg_frame(*r, ^r))]
#[ensures(free_ok(*r, hs@[j@].0, hs@[j@].1) && slot_live(*r, hs@[j@].0@, hs@[j@].1@))]
#[ensures(slot_gone(^r, hs@[j@].0, hs@[j@].1))]
#[ensures((^r).slabs@.contains(hs@[j@].0@) ==> (^r).slabs@.lookup(hs@[j@].0@).keys@[hs@[j@].1@] == FREE_KEY)]
#[ensures(!inpend((^hs)@, hs@[j@].0@, hs@[j@].1@) && subl((^hs)@, hs@, j@))]
#[ensures(forall<t: Int, k: Int> slot_live(*r, t, k) && (t != hs@[j@].0@ || k != hs@[j@].1@) ==> keep_slot(*r, ^r, t, k))]
#[ensures((^r).pending_frees == r.pending_frees)]
#[ensures(forall<t: Int, k: Int> slot_live(^r, t, k) ==> keep_slot(^r, *r, t, k))]
pub fn abort_h(r: &mut RegionState, hs: &mut Vec<(u64, usize)>, j: usize) {
    let a = snapshot! { *r };
    let h0 = snapshot! { hs@ };
    let (s, i) = hs[j];
    proof_assert! { h0[j@] == (s, i) && slot_live(*a, s@, i@) };
    let sl = snapshot! { a.slabs@.lookup(s@) };
    r.free_slot(s, i);
    let _ = hs.remove(j);
    proof_assert! { lemma_subl(hs@, *h0, j@); lemma_abort_good(*a, *r, s, i, *sl, *h0, hs@, j@); true };
    proof_assert! { lemma_pv_sub(*a, *h0, hs@, j@); true };
    proof_assert! { free_case(*a, *r, s, i, *sl) };
    proof_assert! { lemma_free_back(*a, *r, s, i, *sl); true };
}

/// The publish closure (lib.rs:597-616) through live handle `j` with reservation key `key`.
#[requires(lg(*r, hs@) && j@ < hs@.len())]
#[ensures(lg(^r, (^hs)@) && rg_frame(*r, ^r))]
#[ensures(slot_live(*r, hs@[j@].0@, hs@[j@].1@) && free_ok(*r, hs@[j@].0, hs@[j@].1))]
#[ensures(!inpend((^hs)@, hs@[j@].0@, hs@[j@].1@) && subl((^hs)@, hs@, j@))]
#[ensures(key == FREE_KEY ==> slot_gone(^r, hs@[j@].0, hs@[j@].1)
    && ((^r).slabs@.contains(hs@[j@].0@) ==> (^r).slabs@.lookup(hs@[j@].0@).keys@[hs@[j@].1@] == FREE_KEY))]
#[ensures(key != FREE_KEY ==> slot_live(^r, hs@[j@].0@, hs@[j@].1@)
    && (^r).slabs@.lookup(hs@[j@].0@).keys@[hs@[j@].1@] == key
    && (^r).slabs@.lookup(hs@[j@].0@).start_offset == r.slabs@.lookup(hs@[j@].0@).start_offset
    && (^r).slabs@.lookup(hs@[j@].0@).element_size == r.slabs@.lookup(hs@[j@].0@).element_size)]
#[ensures(forall<t: Int, k: Int> slot_live(*r, t, k) && (t != hs@[j@].0@ || k != hs@[j@].1@) ==> keep_slot(*r, ^r, t, k))]
#[ensures(key != FREE_KEY ==> forall<t: Int, k: Int> (t != hs@[j@].0@ || k != hs@[j@].1@) && (^r).slabs@.contains(t) && r.slabs@.contains(t)
    && 0 <= k && k < r.slabs@.lookup(t).bitmap.num_slots@ ==>
    (^r).slabs@.lookup(t).keys@[k] == r.slabs@.lookup(t).keys@[k])]
#[ensures(key != FREE_KEY ==> forall<t: Int> (^r).slabs@.contains(t) == r.slabs@.contains(t))]
#[ensures((^r).pending_frees == r.pending_frees)]
#[ensures(key == FREE_KEY ==> forall<t: Int, k: Int> slot_live(^r, t, k) ==> keep_slot(^r, *r, t, k))]
#[ensures(key != FREE_KEY ==> forall<t: Int, k: Int> slot_live(^r, t, k) ==> slot_live(*r, t, k))]
pub fn publish_h(r: &mut RegionState, hs: &mut Vec<(u64, usize)>, j: usize, key: u64) {
    let a = snapshot! { *r };
    let h0 = snapshot! { hs@ };
    let (s, i) = hs[j];
    proof_assert! { h0[j@] == (s, i) && slot_live(*a, s@, i@) };
    let sl = snapshot! { a.slabs@.lookup(s@) };
    if key == FREE_KEY {
        // Special sentinel: silently discard — free the slot (lib.rs:600-608).
        r.free_slot(s, i);
        let _ = hs.remove(j);
        proof_assert! { lemma_subl(hs@, *h0, j@); lemma_abort_good(*a, *r, s, i, *sl, *h0, hs@, j@); true };
        proof_assert! { free_case(*a, *r, s, i, *sl) };
        proof_assert! { lemma_free_back(*a, *r, s, i, *sl); true };
    } else {
        r.publish_slot(s, i, key);
        proof_assert! { slab_inv(*sl) && i@ < sl.keys@.len() };
        proof_assert! { r.slabs@.contains(s@) && r.slabs@.lookup(s@).bitmap == sl.bitmap && r.slabs@.lookup(s@).keys@[i@] == key };
        proof_assert! { forall<t: Int> r.slabs@.contains(t) == a.slabs@.contains(t) };
        proof_assert! { forall<t: Int> t != s@ ==> r.slabs@.get(t) == a.slabs@.get(t) };
        let _ = hs.remove(j);
        proof_assert! { lemma_subl(hs@, *h0, j@); lemma_publish_good(*a, *r, s, i, key, *h0, hs@, j@); true };
    }
    proof_assert! { lemma_pv_sub(*a, *h0, hs@, j@); true };
}

/// **EM-ABORT-POST-RELEASED** — "After abort() is called on a write handle, the reserved
/// extent never appears in get_extents() or for_each_extent(), and its slot is released
/// immediately, without waiting for a checkpoint, so later reservations can reuse it."
/// Through the abort closure (lib.rs:618-621) on any reachable region and live handle: the
/// slot is no longer allocated (or its slab went back to the buddy allocator), its key is
/// FREE_KEY (get_extents lists only non-FREE keys), and the handle is consumed; every other
/// live slot is untouched; the invariant holds afterwards.
#[requires(lg(*r, hs@) && j@ < hs@.len())]
#[ensures(lg(^r, (^hs)@))]
#[ensures(slot_gone(^r, hs@[j@].0, hs@[j@].1))]
#[ensures((^r).slabs@.contains(hs@[j@].0@) ==> (^r).slabs@.lookup(hs@[j@].0@).keys@[hs@[j@].1@] == FREE_KEY)]
#[ensures(!inpend((^hs)@, hs@[j@].0@, hs@[j@].1@) && !inpend((^r).pending_frees@, hs@[j@].0@, hs@[j@].1@))]
pub fn verify_em_abort_post_released(r: &mut RegionState, hs: &mut Vec<(u64, usize)>, j: usize) {
    let a = snapshot! { *r };
    let h0 = snapshot! { hs@ };
    abort_h(r, hs, j);
    proof_assert! { r.pending_frees == a.pending_frees };
    proof_assert! { forall<p: Int> 0 <= p && p < a.pending_frees@.len() ==> a.pending_frees@[p] != h0[j@] };
    proof_assert! { forall<p: Int> 0 <= p && p < a.pending_frees@.len() ==> a.pending_frees@[p].0@ != h0[j@].0@ || a.pending_frees@[p].1@ != h0[j@].1@ };
}

/// Mutant: claims the aborted slot is still allocated afterwards.
#[requires(lg(*r, hs@) && j@ < hs@.len())]
#[ensures(slot_live(^r, hs@[j@].0@, hs@[j@].1@))]
pub fn verify_em_abort_post_released__mutant(r: &mut RegionState, hs: &mut Vec<(u64, usize)>, j: usize) {
    abort_h(r, hs, j);
}

/// **EM-HANDLE-DROP-AUTO-ABORT** — "When a write handle is discarded without publish() or
/// abort() having been called, the reservation is aborted automatically with the same
/// observable effect as abort() (the extent is not visible and its slot is released), and
/// this does not crash." WriteHandle::drop (interfaces/src/iextent_manager.rs:150) runs the
/// same abort closure (lib.rs:618-621): its free_slot never trips bitmap.rs:27-28 or the
/// slab.rs:39 index (`free_ok` — the slot is in range and allocated), and the effect is
/// abort's.
#[requires(lg(*r, hs@) && j@ < hs@.len())]
#[ensures(free_ok(*r, hs@[j@].0, hs@[j@].1))]
#[ensures(lg(^r, (^hs)@))]
#[ensures(slot_gone(^r, hs@[j@].0, hs@[j@].1))]
#[ensures((^r).slabs@.contains(hs@[j@].0@) ==> (^r).slabs@.lookup(hs@[j@].0@).keys@[hs@[j@].1@] == FREE_KEY)]
pub fn verify_em_handle_drop_auto_abort(r: &mut RegionState, hs: &mut Vec<(u64, usize)>, j: usize) {
    abort_h(r, hs, j);
}

/// Mutant: claims the dropped handle's slot is out of range (the panic case).
#[requires(lg(*r, hs@) && j@ < hs@.len())]
#[ensures(!free_ok(*r, hs@[j@].0, hs@[j@].1))]
pub fn verify_em_handle_drop_auto_abort__mutant(r: &mut RegionState, hs: &mut Vec<(u64, usize)>, j: usize) {
    abort_h(r, hs, j);
}

/// **EM-PUBLISH-POST-FREE-KEY-DISCARDED** — "When publish() is called on a handle whose key is
/// u64::MAX (the FREE_KEY sentinel), it returns success, no extent for that reservation
/// appears in get_extents() or for_each_extent(), and the reserved slot is released
/// immediately so later reservations can reuse it without waiting for a checkpoint."
/// (The success return is component publish's `result == Ok(..)`, lib.rs:600-608.)
#[requires(lg(*r, hs@) && j@ < hs@.len())]
#[ensures(lg(^r, (^hs)@))]
#[ensures(slot_gone(^r, hs@[j@].0, hs@[j@].1))]
#[ensures((^r).slabs@.contains(hs@[j@].0@) ==> (^r).slabs@.lookup(hs@[j@].0@).keys@[hs@[j@].1@] == FREE_KEY)]
#[ensures(!inpend((^r).pending_frees@, hs@[j@].0@, hs@[j@].1@))]
pub fn verify_em_publish_post_free_key_discarded(r: &mut RegionState, hs: &mut Vec<(u64, usize)>, j: usize) {
    let a = snapshot! { *r };
    let h0 = snapshot! { hs@ };
    publish_h(r, hs, j, FREE_KEY);
    proof_assert! { forall<p: Int> 0 <= p && p < a.pending_frees@.len() ==> a.pending_frees@[p] != h0[j@] };
    proof_assert! { forall<p: Int> 0 <= p && p < a.pending_frees@.len() ==> a.pending_frees@[p].0@ != h0[j@].0@ || a.pending_frees@[p].1@ != h0[j@].1@ };
}

/// Mutant: claims the discarded reservation is listed with key u64::MAX.
#[requires(lg(*r, hs@) && j@ < hs@.len())]
#[ensures((^r).slabs@.contains(hs@[j@].0@) && (^r).slabs@.lookup(hs@[j@].0@).keys@[hs@[j@].1@] != FREE_KEY)]
pub fn verify_em_publish_post_free_key_discarded__mutant(r: &mut RegionState, hs: &mut Vec<(u64, usize)>, j: usize) {
    publish_h(r, hs, j, FREE_KEY);
}

/// **EM-PUBLISH-FRAME-OTHER-EXTENTS** — "publish() changes only the published slot; the key,
/// offset, size and visibility of every other extent stay the same." Every other live slot
/// (every other extent: keys imply allocated) keeps key, slab start, element size and slot
/// count, and no other slot's key changes.
// LEVEL-3 (phase D-B2): WIDENED to every key, the FREE_KEY sentinel included (the old premise
// `key != FREE_KEY` is gone): every other live slot keeps key, offset, size and visibility
// (keep_slot), and no other slot becomes visible or changes key; the two extra facts about
// non-live slots and the slab set hold for a real key only (FREE_KEY may drop the emptied slab
// of the published slot itself).
#[requires(lg(*r, hs@) && j@ < hs@.len())]
#[ensures(forall<t: Int, k: Int> slot_live(*r, t, k) && (t != hs@[j@].0@ || k != hs@[j@].1@) ==> keep_slot(*r, ^r, t, k))]
#[ensures(lg(^r, (^hs)@))]
#[ensures(forall<t: Int, k: Int> slot_live(^r, t, k) && (t != hs@[j@].0@ || k != hs@[j@].1@) ==>
    slot_live(*r, t, k) && (^r).slabs@.lookup(t).keys@[k] == r.slabs@.lookup(t).keys@[k])]
#[ensures(key != FREE_KEY ==> forall<t: Int, k: Int> (t != hs@[j@].0@ || k != hs@[j@].1@) && (^r).slabs@.contains(t) && r.slabs@.contains(t)
    && 0 <= k && k < r.slabs@.lookup(t).bitmap.num_slots@ ==>
    (^r).slabs@.lookup(t).keys@[k] == r.slabs@.lookup(t).keys@[k])]
#[ensures(key != FREE_KEY ==> forall<t: Int> (^r).slabs@.contains(t) == r.slabs@.contains(t))]
pub fn verify_em_publish_frame_other_extents(r: &mut RegionState, hs: &mut Vec<(u64, usize)>, j: usize, key: u64) {
    let r0 = snapshot! { *r };
    let h0 = snapshot! { hs@ };
    publish_h(r, hs, j, key);
    proof_assert! { key == FREE_KEY ==> forall<t: Int, k: Int> slot_live(*r, t, k) ==> keep_slot(*r, *r0, t, k) };
    proof_assert! { key != FREE_KEY ==> forall<t: Int, k: Int> slot_live(*r, t, k) && (t != h0[j@].0@ || k != h0[j@].1@) ==>
        slot_live(*r0, t, k) && r0.slabs@.contains(t) && r.slabs@.contains(t) && 0 <= k && k < r0.slabs@.lookup(t).bitmap.num_slots@ };
}

/// Mutant: claims another live slot changed its key.
/// LEVEL-3 (phase D-B2): mutant with EXACTLY the property's requires and body; it claims the
/// negation of the property's first postcondition.
/// LEVEL-3 (phase D-B2): mutant with EXACTLY the property's requires and body; it claims the
/// negation of the property's first postcondition.
// LEVEL-3 (phase D-B2): WIDENED to every key, the FREE_KEY sentinel included (the old premise
// `key != FREE_KEY` is gone): every other live slot keeps key, offset, size and visibility
// (keep_slot), and no other slot becomes visible or changes key; the two extra facts about
// non-live slots and the slab set hold for a real key only (FREE_KEY may drop the emptied slab
// of the published slot itself).
#[requires(lg(*r, hs@) && j@ < hs@.len())]
#[ensures(!(forall<t: Int, k: Int> slot_live(*r, t, k) && (t != hs@[j@].0@ || k != hs@[j@].1@) ==> keep_slot(*r, ^r, t, k)))]
pub fn verify_em_publish_frame_other_extents__mutant(r: &mut RegionState, hs: &mut Vec<(u64, usize)>, j: usize, key: u64) {
    let r0 = snapshot! { *r };
    let h0 = snapshot! { hs@ };
    publish_h(r, hs, j, key);
    proof_assert! { key == FREE_KEY ==> forall<t: Int, k: Int> slot_live(*r, t, k) ==> keep_slot(*r, *r0, t, k) };
    proof_assert! { key != FREE_KEY ==> forall<t: Int, k: Int> slot_live(*r, t, k) && (t != h0[j@].0@ || k != h0[j@].1@) ==>
        slot_live(*r0, t, k) && r0.slabs@.contains(t) && r.slabs@.contains(t) && 0 <= k && k < r0.slabs@.lookup(t).bitmap.num_slots@ };
}

/// **EM-PUBLISH-POST-VISIBLE** — "After publish() succeeds on a handle whose key is not
/// u64::MAX, get_extents() lists exactly one extent with that handle's key, offset and size."
/// The handle's slot is live with the key, at the same offset/element size; no other live
/// slot of the region shares any byte with it (so no other listed extent has that offset);
/// get_extents lists exactly the live non-FREE slots (batch 3, listing.rs).
#[requires(lg(*r, hs@) && j@ < hs@.len() && key != FREE_KEY)]
#[ensures(lg(^r, (^hs)@))]
#[ensures(slot_live(^r, hs@[j@].0@, hs@[j@].1@) && (^r).slabs@.lookup(hs@[j@].0@).keys@[hs@[j@].1@] == key)]
#[ensures(slot_off((^r).slabs@.lookup(hs@[j@].0@), hs@[j@].1@) == slot_off(r.slabs@.lookup(hs@[j@].0@), hs@[j@].1@))]
#[ensures((^r).slabs@.lookup(hs@[j@].0@).element_size == r.slabs@.lookup(hs@[j@].0@).element_size)]
#[ensures(forall<t: Int, k: Int> slot_live(^r, t, k) && (t != hs@[j@].0@ || k != hs@[j@].1@) ==>
    slot_off((^r).slabs@.lookup(t), k) != slot_off((^r).slabs@.lookup(hs@[j@].0@), hs@[j@].1@))]
pub fn verify_em_publish_post_visible(r: &mut RegionState, hs: &mut Vec<(u64, usize)>, j: usize, key: u64) {
    publish_h(r, hs, j, key);
    proof_assert! { lemma_no_overlap(*r); no_overlap(*r) };
    proof_assert! { forall<t: Int, k: Int> slot_live(*r, t, k) ==> slab_ok(*r, r.slabs@.lookup(t)) && r.slabs@.lookup(t).element_size@ > 0 };
}

/// Mutant: claims the published key is not on the slot.
#[requires(lg(*r, hs@) && j@ < hs@.len() && key != FREE_KEY)]
#[ensures((^r).slabs@.lookup(hs@[j@].0@).keys@[hs@[j@].1@] != key)]
pub fn verify_em_publish_post_visible__mutant(r: &mut RegionState, hs: &mut Vec<(u64, usize)>, j: usize, key: u64) {
    publish_h(r, hs, j, key);
}

// ============================================================ reserve (lib.rs:584-630)

/// A slot that became live does not overlap a kept live slot.
#[logic]
#[requires(no_overlap(b) && slot_live(b, s, i) && !slot_live(a, s, i) && slot_live(a, t, k) && keep_slot(a, b, t, k))]
#[ensures(disj(slot_off(b.slabs@.lookup(s), i), b.slabs@.lookup(s).element_size@,
    slot_off(a.slabs@.lookup(t), k), a.slabs@.lookup(t).element_size@))]
pub fn lemma_new_disj(a: RegionState, b: RegionState, s: Int, i: Int, t: Int, k: Int) {
    pearlite! {
        proof_assert! { s != t || i != k };
        proof_assert! { slot_off(b.slabs@.lookup(t), k) == slot_off(a.slabs@.lookup(t), k) }
    }
}

/// The slots a call frees are allocated, on every reachable region.
#[logic]
#[requires(lg(r, hs) && lop_pre(r, hs, op))]
#[ensures(frees_ok(r, hs, op))]
pub fn lemma_frees_ok(r: RegionState, hs: Seq<(u64, usize)>, op: LOp) {}

/// `reserve_extent`'s region step (region.rs:40-91 via lib.rs:589): the invariant holds
/// after, every live slot is kept, and a success adds a new live handle slot of the
/// requested aligned size at the returned offset.
#[requires(lg(*r, hs@) && sz@ > 0 && a_3783f8(sz@, r.format_params.sector_size@))]
#[ensures(lg(^r, (^hs)@) && rg_frame(*r, ^r) && (^r).pending_frees == r.pending_frees)]
#[ensures(forall<t: Int, k: Int> slot_live(*r, t, k) ==> keep_slot(*r, ^r, t, k))]
#[ensures(no_new_keys(*r, ^r))]
#[ensures(match result {
    Ok((s, i, off)) => (^hs)@ == hs@.push_back((s, i)) && !slot_live(*r, s@, i@) && slot_live(^r, s@, i@)
        && (^r).slabs@.lookup(s@).element_size@ == align_l(sz@, r.format_params.sector_size@)
        && off@ == slot_off((^r).slabs@.lookup(s@), i@)
        && (^r).slabs@.lookup(s@).keys@[i@] == FREE_KEY,
    Err(_) => (^hs)@ == hs@,
})]
pub fn reserve_h(r: &mut RegionState, hs: &mut Vec<(u64, usize)>, sz: u32) -> Result<(u64, usize, u64), EmError> {
    let a = snapshot! { *r };
    let h0 = snapshot! { hs@ };
    let res = r.alloc_extent(sz);
    proof_assert! { lemma_reserve_good(*a, *r, align_l(sz@, a.format_params.sector_size@), res, *h0); true };
    match res {
        Ok((s, i, off)) => {
            hs.push((s, i));
            proof_assert! { pv(*r, hs@) && hs@[hs@.len() - 1] == (s, i) };
        }
        Err(_) => {}
    }
    res
}

/// **EM-INV-NO-OVERLAP** — "At all times, no two slots that are in use at the same time
/// (reserved, published, or removed but not yet released by a checkpoint) share any byte of
/// the data device; in particular a newly reserved extent overlaps no such existing extent."
/// In-use slots are the live (allocated) slots. After any call on a reachable region, then a
/// reservation: distinct live slots are byte-disjoint, every slot in use before the
/// reservation is still in use at the same place, and the new extent overlaps none of them.
#[requires(lg(*r, hs@) && lop_pre(*r, hs@, op))]
#[requires(sz@ > 0 && a_3783f8(sz@, r.format_params.sector_size@))]
#[ensures(no_overlap(^r))]
#[ensures(match result.1 {
    Ok((s, i, off)) => forall<t: Int, k: Int> slot_live(*result.0, t, k) ==>
        keep_slot(*result.0, ^r, t, k)
        && disj(off@, (^r).slabs@.lookup(s@).element_size@,
                slot_off(result.0.slabs@.lookup(t), k), result.0.slabs@.lookup(t).element_size@),
    Err(_) => true,
})]
pub fn verify_em_inv_no_overlap(r: &mut RegionState, hs: &mut Vec<(u64, usize)>, op: LOp, sz: u32)
    -> (Snapshot<RegionState>, Result<(u64, usize, u64), EmError>) {
    apply_lop(r, hs, op);
    let a = snapshot! { *r };
    let res = reserve_h(r, hs, sz);
    proof_assert! { lemma_no_overlap(*r); no_overlap(*r) };
    proof_assert! { forall<t: Int, k: Int> slot_live(*a, t, k) ==> keep_slot(*a, *r, t, k) && slot_live(*r, t, k) };
    proof_assert! { match res { Ok((s, i, off)) => forall<t: Int, k: Int> slot_live(*a, t, k) ==> {
        lemma_new_disj(*a, *r, s@, i@, t, k);
        disj(off@, r.slabs@.lookup(s@).element_size@, slot_off(a.slabs@.lookup(t), k), a.slabs@.lookup(t).element_size@) },
        Err(_) => true } };
    (a, res)
}

/// Mutant: claims the new extent starts at the start of some existing in-use slot.
/// LEVEL-3 (phase D-B2): mutant with EXACTLY the property's requires and body; it claims the
/// negation of the property's first postcondition.
#[requires(lg(*r, hs@) && lop_pre(*r, hs@, op))]
#[requires(sz@ > 0 && a_3783f8(sz@, r.format_params.sector_size@))]
#[ensures(!(no_overlap(^r)))]
pub fn verify_em_inv_no_overlap__mutant(r: &mut RegionState, hs: &mut Vec<(u64, usize)>, op: LOp, sz: u32)
    -> (Snapshot<RegionState>, Result<(u64, usize, u64), EmError>) {
    apply_lop(r, hs, op);
    let a = snapshot! { *r };
    let res = reserve_h(r, hs, sz);
    proof_assert! { lemma_no_overlap(*r); no_overlap(*r) };
    proof_assert! { forall<t: Int, k: Int> slot_live(*a, t, k) ==> keep_slot(*a, *r, t, k) && slot_live(*r, t, k) };
    proof_assert! { match res { Ok((s, i, off)) => forall<t: Int, k: Int> slot_live(*a, t, k) ==> {
        lemma_new_disj(*a, *r, s@, i@, t, k);
        disj(off@, r.slabs@.lookup(s@).element_size@, slot_off(a.slabs@.lookup(t), k), a.slabs@.lookup(t).element_size@) },
        Err(_) => true } };
    (a, res)
}

/// **EM-INV-SLAB-UNIFORM-ELEMENT** — "All slots of a given slab have the same element size and
/// every slab listed under a size class has exactly that element size, so extents whose
/// sector-aligned sizes differ are never placed in the same slab." (A slab has one
/// `element_size` field, slab.rs:9.) Every size-class entry names a slab of that element size
/// in every reachable region, and a reservation lands in a slab whose element size is its
/// own sector-aligned size.
#[requires(lg(*r, hs@) && lop_pre(*r, hs@, op))]
#[requires(sz@ > 0 && a_3783f8(sz@, r.format_params.sector_size@))]
#[requires(p2(fp.sector_size@))]
#[requires(rg_in(base@, size@, fp))]
#[requires(forall<p: Int> 0 <= p && p < descs@.len() ==> desc_ok_a(descs@[p], base@, size@, fp))]
#[requires(a_980f61_list(descs@, base@, size@))]
#[ensures(sc_ok(^r) && sc_ok(result.1) && sc_ok(result.2))]
#[ensures(match result.0 {
    Ok((s, _, _)) => (^r).slabs@.lookup(s@).element_size@ == align_l(sz@, r.format_params.sector_size@),
    Err(_) => true,
})]
pub fn verify_em_inv_slab_uniform_element(r: &mut RegionState, hs: &mut Vec<(u64, usize)>, op: LOp, sz: u32, base: u64, size: u64,
    fp: FormatParams, descs: &Vec<SlabDescriptor>) -> (Result<(u64, usize, u64), EmError>, RegionState, RegionState) {
    proof_assert! { lemma_rg_in(base@, size@, fp); rg_hd(base@, size@, fp) };
    proof_assert! { forall<p: Int> 0 <= p && p < descs@.len() ==> { lemma_desc_ok_a(descs@[p], base@, size@, fp); desc_ok(descs@[p], base@, size@, fp) } };
    proof_assert! { forall<p: Int> 0 <= p && p < descs@.len() ==> descs@[p].slab_size@ > 0 };
    proof_assert! { lemma_starts_distinct(descs@, base@, size@); starts_distinct(descs@) };
    let a0 = snapshot! { *r };
    let (f, g) = all3(r, hs, op, base, size, fp, descs);
    proof_assert! { r.format_params == a0.format_params };
    let res = reserve_h(r, hs, sz);
    (res, f, g)
}

/// Mutant: claims the reservation's slab has a different element size.
/// LEVEL-3 (phase D-B2): mutant with EXACTLY the property's requires and body; it claims the
/// negation of the property's first postcondition.
#[requires(lg(*r, hs@) && lop_pre(*r, hs@, op))]
#[requires(sz@ > 0 && a_3783f8(sz@, r.format_params.sector_size@))]
#[requires(p2(fp.sector_size@))]
#[requires(rg_in(base@, size@, fp))]
#[requires(forall<p: Int> 0 <= p && p < descs@.len() ==> desc_ok_a(descs@[p], base@, size@, fp))]
#[requires(a_980f61_list(descs@, base@, size@))]
#[ensures(!(sc_ok(^r) && sc_ok(result.1) && sc_ok(result.2)))]
pub fn verify_em_inv_slab_uniform_element__mutant(r: &mut RegionState, hs: &mut Vec<(u64, usize)>, op: LOp, sz: u32, base: u64, size: u64,
    fp: FormatParams, descs: &Vec<SlabDescriptor>) -> (Result<(u64, usize, u64), EmError>, RegionState, RegionState) {
    proof_assert! { lemma_rg_in(base@, size@, fp); rg_hd(base@, size@, fp) };
    proof_assert! { forall<p: Int> 0 <= p && p < descs@.len() ==> { lemma_desc_ok_a(descs@[p], base@, size@, fp); desc_ok(descs@[p], base@, size@, fp) } };
    proof_assert! { forall<p: Int> 0 <= p && p < descs@.len() ==> descs@[p].slab_size@ > 0 };
    proof_assert! { lemma_starts_distinct(descs@, base@, size@); starts_distinct(descs@) };
    let a0 = snapshot! { *r };
    let (f, g) = all3(r, hs, op, base, size, fp, descs);
    proof_assert! { r.format_params == a0.format_params };
    let res = reserve_h(r, hs, sz);
    (res, f, g)
}

/// The slot a call frees (bitmap clear, bitmap.rs:26): abort / publish(FREE_KEY) free the
/// handle's slot, a checkpoint frees every queued slot.
#[logic(open)]
pub fn frees_ok(r: RegionState, hs: Seq<(u64, usize)>, op: LOp) -> bool {
    pearlite! {
        match op {
            LOp::Abort(j) => slot_live(r, hs[j@].0@, hs[j@].1@),
            LOp::Publish(j, k) => k == FREE_KEY ==> slot_live(r, hs[j@].0@, hs[j@].1@),
            LOp::Flush => forall<p: Int> 0 <= p && p < r.pending_frees@.len() ==>
                slot_live(r, r.pending_frees@[p].0@, r.pending_frees@[p].1@),
            _ => true,
        }
    }
}

/// **EM-BITMAP-NO-DOUBLE-FREE** — "A slot is only ever marked free when it is currently
/// allocated." Every slot a call clears (abort / publish(FREE_KEY): the handle's slot;
/// checkpoint: every queued slot) is allocated before the call, on every reachable region;
/// the invariant holds afterwards so this holds for every later call too.
#[requires(lg(*r, hs@) && lop_pre(*r, hs@, op))]
#[ensures(frees_ok(*r, hs@, op))]
#[ensures(lg(^r, (^hs)@))]
pub fn verify_em_bitmap_no_double_free(r: &mut RegionState, hs: &mut Vec<(u64, usize)>, op: LOp) {
    proof_assert! { lemma_frees_ok(*r, hs@, op); frees_ok(*r, hs@, op) };
    apply_lop(r, hs, op)
}

/// Mutant: claims a checkpoint frees an unallocated queued slot.
/// LEVEL-3 (phase D-B2): mutant with EXACTLY the property's requires and body; it claims the
/// negation of the property's first postcondition.
#[requires(lg(*r, hs@) && lop_pre(*r, hs@, op))]
#[ensures(!(frees_ok(*r, hs@, op)))]
pub fn verify_em_bitmap_no_double_free__mutant(r: &mut RegionState, hs: &mut Vec<(u64, usize)>, op: LOp) {
    proof_assert! { lemma_frees_ok(*r, hs@, op); frees_ok(*r, hs@, op) };
    apply_lop(r, hs, op)
}

/// **EM-BITMAP-INDEX-IN-RANGE** — "Every slot index used to set, clear or test the bitmap is
/// less than the slab's number of slots." Clear (free_slot): the handle's / queued slot is in
/// range; set (alloc_slot via reserve): the returned index is in range; test (remove,
/// region.rs:137): the slot it found is in range — on every reachable region.
#[requires(lg(*r, hs@) && lop_pre(*r, hs@, op))]
#[requires(sz@ > 0 && a_3783f8(sz@, r.format_params.sector_size@))]
#[ensures(frees_ok(*r, hs@, op))]
#[ensures(match result.0 { Ok((s, i, _)) => i@ < result.1.slabs@.lookup(s@).bitmap.num_slots@, Err(_) => true })]
#[ensures(forall<s: u64, i: usize> rm_case(*result.1, off, s, i) ==> i@ < result.1.slabs@.lookup(s@).bitmap.num_slots@)]
pub fn verify_em_bitmap_index_in_range(r: &mut RegionState, hs: &mut Vec<(u64, usize)>, op: LOp, sz: u32, off: u64)
    -> (Result<(u64, usize, u64), EmError>, Snapshot<RegionState>) {
    proof_assert! { lemma_frees_ok(*r, hs@, op); frees_ok(*r, hs@, op) };
    apply_lop(r, hs, op);
    let res = reserve_h(r, hs, sz);
    let b = snapshot! { *r };
    let _ = r.remove_extent_by_offset(off);
    (res, b)
}

/// Mutant: claims the reserved index is out of range.
/// LEVEL-3 (phase D-B2): mutant with EXACTLY the property's requires and body; it claims the
/// negation of the property's first postcondition.
#[requires(lg(*r, hs@) && lop_pre(*r, hs@, op))]
#[requires(sz@ > 0 && a_3783f8(sz@, r.format_params.sector_size@))]
#[ensures(!(frees_ok(*r, hs@, op)))]
pub fn verify_em_bitmap_index_in_range__mutant(r: &mut RegionState, hs: &mut Vec<(u64, usize)>, op: LOp, sz: u32, off: u64)
    -> (Result<(u64, usize, u64), EmError>, Snapshot<RegionState>) {
    proof_assert! { lemma_frees_ok(*r, hs@, op); frees_ok(*r, hs@, op) };
    apply_lop(r, hs, op);
    let res = reserve_h(r, hs, sz);
    let b = snapshot! { *r };
    let _ = r.remove_extent_by_offset(off);
    (res, b)
}

/// **EM-INIT-POST-RECOVERED-SLOTS-OCCUPIED** — "After initialize() succeeds, every slot whose
/// recovered key is not the FREE_KEY sentinel is treated as occupied, so no later
/// reserve_extent() returns space overlapping the disk range of any recovered extent."
/// The rebuilt region (lib.rs:552-565) has every recovered non-FREE slot allocated and
/// satisfies the history invariant; the next reservation keeps every recovered slot and
/// overlaps none (and, by the invariant, so does every later one — verify_em_inv_no_overlap).
#[requires(p2(fp.sector_size@))]
#[requires(rg_in(base@, size@, fp))]
#[requires(forall<p: Int> 0 <= p && p < descs@.len() ==> desc_ok_a(descs@[p], base@, size@, fp))]
#[requires(a_980f61_list(descs@, base@, size@))]
#[requires(sz@ > 0 && a_3783f8(sz@, fp.sector_size@))]
#[ensures(forall<p: Int, k: Int> 0 <= p && p < descs@.len() && 0 <= k && k < descs@[p].keys@.len() && descs@[p].keys@[k] != FREE_KEY ==>
    slot_live(*result.0, descs@[p].start_offset@, k) && keep_slot(*result.0, result.2, descs@[p].start_offset@, k)
    && match result.1 {
        Ok((s, i, off)) => disj(off@, result.2.slabs@.lookup(s@).element_size@,
            slot_off(result.0.slabs@.lookup(descs@[p].start_offset@), k), result.0.slabs@.lookup(descs@[p].start_offset@).element_size@),
        Err(_) => true,
    })]
#[ensures(lg(*result.0, Seq::empty()))]
pub fn verify_em_init_post_recovered_slots_occupied(base: u64, size: u64, fp: FormatParams, descs: &Vec<SlabDescriptor>, sz: u32)
    -> (Snapshot<RegionState>, Result<(u64, usize, u64), EmError>, RegionState) {
    proof_assert! { lemma_rg_in(base@, size@, fp); rg_hd(base@, size@, fp) };
    proof_assert! { forall<p: Int> 0 <= p && p < descs@.len() ==> { lemma_desc_ok_a(descs@[p], base@, size@, fp); desc_ok(descs@[p], base@, size@, fp) } };
    proof_assert! { forall<p: Int> 0 <= p && p < descs@.len() ==> descs@[p].slab_size@ > 0 };
    proof_assert! { lemma_starts_distinct(descs@, base@, size@); starts_distinct(descs@) };
    let mut g = rebuilt_region(base, size, fp, descs);
    let g0 = snapshot! { g };
    let mut hs: Vec<(u64, usize)> = Vec::new();
    proof_assert! { hs@ == Seq::empty() };
    proof_assert! { forall<p: Int, k: Int> 0 <= p && p < descs@.len() && 0 <= k && k < descs@[p].keys@.len() && descs@[p].keys@[k] != FREE_KEY ==>
        slab_is_desc(g0.slabs@.lookup(descs@[p].start_offset@), descs@[p]) && slot_live(*g0, descs@[p].start_offset@, k) };
    let res = reserve_h(&mut g, &mut hs, sz);
    proof_assert! { lemma_no_overlap(g); no_overlap(g) };
    proof_assert! { match res { Ok((s, i, off)) => forall<t: Int, k: Int> slot_live(*g0, t, k) ==> {
        lemma_new_disj(*g0, g, s@, i@, t, k);
        disj(off@, g.slabs@.lookup(s@).element_size@, slot_off(g0.slabs@.lookup(t), k), g0.slabs@.lookup(t).element_size@) },
        Err(_) => true } };
    (g0, res, g)
}

/// Mutant: claims some recovered non-FREE slot is free after initialize.
/// LEVEL-3 (phase D-B2): mutant with EXACTLY the property's requires and body; it claims the
/// negation of the property's first postcondition.
#[requires(p2(fp.sector_size@))]
#[requires(rg_in(base@, size@, fp))]
#[requires(forall<p: Int> 0 <= p && p < descs@.len() ==> desc_ok_a(descs@[p], base@, size@, fp))]
#[requires(a_980f61_list(descs@, base@, size@))]
#[requires(sz@ > 0 && a_3783f8(sz@, fp.sector_size@))]
#[ensures(!(forall<p: Int, k: Int> 0 <= p && p < descs@.len() && 0 <= k && k < descs@[p].keys@.len() && descs@[p].keys@[k] != FREE_KEY ==>
    slot_live(*result.0, descs@[p].start_offset@, k) && keep_slot(*result.0, result.2, descs@[p].start_offset@, k)
    && match result.1 {
        Ok((s, i, off)) => disj(off@, result.2.slabs@.lookup(s@).element_size@,
            slot_off(result.0.slabs@.lookup(descs@[p].start_offset@), k), result.0.slabs@.lookup(descs@[p].start_offset@).element_size@),
        Err(_) => true,
    }))]
pub fn verify_em_init_post_recovered_slots_occupied__mutant(base: u64, size: u64, fp: FormatParams, descs: &Vec<SlabDescriptor>, sz: u32)
    -> (Snapshot<RegionState>, Result<(u64, usize, u64), EmError>, RegionState) {
    proof_assert! { lemma_rg_in(base@, size@, fp); rg_hd(base@, size@, fp) };
    proof_assert! { forall<p: Int> 0 <= p && p < descs@.len() ==> { lemma_desc_ok_a(descs@[p], base@, size@, fp); desc_ok(descs@[p], base@, size@, fp) } };
    proof_assert! { forall<p: Int> 0 <= p && p < descs@.len() ==> descs@[p].slab_size@ > 0 };
    proof_assert! { lemma_starts_distinct(descs@, base@, size@); starts_distinct(descs@) };
    let mut g = rebuilt_region(base, size, fp, descs);
    let g0 = snapshot! { g };
    let mut hs: Vec<(u64, usize)> = Vec::new();
    proof_assert! { hs@ == Seq::empty() };
    proof_assert! { forall<p: Int, k: Int> 0 <= p && p < descs@.len() && 0 <= k && k < descs@[p].keys@.len() && descs@[p].keys@[k] != FREE_KEY ==>
        slab_is_desc(g0.slabs@.lookup(descs@[p].start_offset@), descs@[p]) && slot_live(*g0, descs@[p].start_offset@, k) };
    let res = reserve_h(&mut g, &mut hs, sz);
    proof_assert! { lemma_no_overlap(g); no_overlap(g) };
    proof_assert! { match res { Ok((s, i, off)) => forall<t: Int, k: Int> slot_live(*g0, t, k) ==> {
        lemma_new_disj(*g0, g, s@, i@, t, k);
        disj(off@, g.slabs@.lookup(s@).element_size@, slot_off(g0.slabs@.lookup(t), k), g0.slabs@.lookup(t).element_size@) },
        Err(_) => true } };
    (g0, res, g)
}

/// **EM-RESERVE-FRAME** — "reserve_extent() does not change the key, offset, size or
/// visibility of any extent that already exists, does not change the reported capacity, and
/// does not write the persisted metadata." On every reachable region: every in-use slot
/// keeps key, slab start, element size (offset and size) and stays allocated; no slot gains a
/// non-FREE key (no_new_keys: nothing new becomes visible); the region geometry (capacity)
/// is unchanged. (reserve_extent's component contract leaves `dev`, `shared` and every other
/// region unchanged — no metadata write.)
#[requires(lg(*r, hs@) && sz@ > 0 && a_3783f8(sz@, r.format_params.sector_size@))]
#[ensures(forall<t: Int, k: Int> slot_live(*r, t, k) ==> keep_slot(*r, ^r, t, k))]
#[ensures(no_new_keys(*r, ^r))]
#[ensures((^r).buddy.total_usable_size == r.buddy.total_usable_size && rg_frame(*r, ^r))]
#[ensures(lg(^r, (^hs)@))]
pub fn verify_em_reserve_frame(r: &mut RegionState, hs: &mut Vec<(u64, usize)>, sz: u32) -> Result<(u64, usize, u64), EmError> {
    reserve_h(r, hs, sz)
}

/// Mutant: claims the reservation changed an existing extent's key.
/// LEVEL-3 (phase D-B2): mutant with EXACTLY the property's requires and body; it claims the
/// negation of the property's first postcondition.
#[requires(lg(*r, hs@) && sz@ > 0 && a_3783f8(sz@, r.format_params.sector_size@))]
#[ensures(!(forall<t: Int, k: Int> slot_live(*r, t, k) ==> keep_slot(*r, ^r, t, k)))]
pub fn verify_em_reserve_frame__mutant(r: &mut RegionState, hs: &mut Vec<(u64, usize)>, sz: u32) -> Result<(u64, usize, u64), EmError> {
    reserve_h(r, hs, sz)
}

/// A slot with a published key (what get_extents lists, listing.rs `from_slot`).
#[logic(open)]
pub fn pub_slot(r: RegionState, t: Int, k: Int) -> bool {
    pearlite! { r.slabs@.contains(t) && 0 <= k && k < r.slabs@.lookup(t).bitmap.num_slots@ && r.slabs@.lookup(t).keys@[k] != FREE_KEY }
}

/// **EM-CKPT-FRAME-ENUMERATION-UNCHANGED** — "checkpoint() does not change which extents
/// get_extents() lists, their keys, offsets or sizes, or the reported capacity." The region
/// step of a checkpoint (flush_pending_frees, lib.rs:316-318): exactly the same slots carry a
/// published key before and after, with the same key, slab start and element size; the
/// geometry (capacity) is unchanged — on every reachable region.
#[requires(lg(*r, hs@))]
#[ensures(forall<t: Int, k: Int> pub_slot(*r, t, k) == pub_slot(^r, t, k))]
#[ensures(forall<t: Int, k: Int> pub_slot(*r, t, k) ==>
    (^r).slabs@.lookup(t).keys@[k] == r.slabs@.lookup(t).keys@[k]
    && (^r).slabs@.lookup(t).start_offset == r.slabs@.lookup(t).start_offset
    && (^r).slabs@.lookup(t).element_size == r.slabs@.lookup(t).element_size)]
#[ensures(rg_frame(*r, ^r) && lg(^r, hs@))]
pub fn verify_em_ckpt_frame_enumeration_unchanged(r: &mut RegionState, hs: &Vec<(u64, usize)>) {
    let a = snapshot! { *r };
    r.flush_pending_frees();
    proof_assert! { lemma_fl_spec_elim(*a, *r); lemma_flush_good(*a, *r, hs@); true };
    proof_assert! { forall<t: Int, k: Int> pub_slot(*a, t, k) ==> slot_live(*a, t, k) && !inpend(a.pending_frees@, t, k) };
    proof_assert! { forall<t: Int, k: Int> pub_slot(*r, t, k) ==> slot_live(*r, t, k) };
}

/// Mutant: claims a published extent disappears.
/// LEVEL-3 (phase D-B2): mutant with EXACTLY the property's requires and body; it claims the
/// negation of the property's first postcondition.
#[requires(lg(*r, hs@))]
#[ensures(!(forall<t: Int, k: Int> pub_slot(*r, t, k) == pub_slot(^r, t, k)))]
pub fn verify_em_ckpt_frame_enumeration_unchanged__mutant(r: &mut RegionState, hs: &Vec<(u64, usize)>) {
    let a = snapshot! { *r };
    r.flush_pending_frees();
    proof_assert! { lemma_fl_spec_elim(*a, *r); lemma_flush_good(*a, *r, hs@); true };
    proof_assert! { forall<t: Int, k: Int> pub_slot(*a, t, k) ==> slot_live(*a, t, k) && !inpend(a.pending_frees@, t, k) };
    proof_assert! { forall<t: Int, k: Int> pub_slot(*r, t, k) ==> slot_live(*r, t, k) };
}

/// **EM-INV-WITHIN-USABLE-RANGE** — "Every reserved or published extent starts at or after the
/// data start offset and ends at or before the data disk size..." For a region laid out
/// inside `[lo, hi]` (format/initialize place every region inside
/// `[data_start_offset, data_disk_size]`, `em_layout`), every in-use slot of every reachable
/// region lies inside `[lo, hi]`, and a reservation of `sz` bytes returns
/// `[off, off + align(sz))` inside it.
#[requires(lg(*r, hs@) && lop_pre(*r, hs@, op) && rg_within(*r, lo@, hi@))]
#[requires(sz@ > 0 && a_3783f8(sz@, r.format_params.sector_size@))]
#[ensures(forall<t: Int, k: Int> slot_live(^r, t, k) ==>
    lo@ <= slot_off((^r).slabs@.lookup(t), k) && slot_off((^r).slabs@.lookup(t), k) + (^r).slabs@.lookup(t).element_size@ <= hi@)]
#[ensures(match result { Ok((_, _, off)) => lo@ <= off@ && off@ + align_l(sz@, r.format_params.sector_size@) <= hi@, Err(_) => true })]
pub fn verify_em_inv_within_usable_range(r: &mut RegionState, hs: &mut Vec<(u64, usize)>, op: LOp, sz: u32, lo: u64, hi: u64)
    -> Result<(u64, usize, u64), EmError> {
    let a0 = snapshot! { *r };
    apply_lop(r, hs, op);
    proof_assert! { r.format_params == a0.format_params && rg_within(*r, lo@, hi@) };
    let res = reserve_h(r, hs, sz);
    proof_assert! { rg_within(*r, lo@, hi@) };
    proof_assert! { forall<t: Int, k: Int> slot_live(*r, t, k) ==> {
        lemma_slot_in_block(*r, t, k);
        slab_ok(*r, r.slabs@.lookup(t)) && r.slabs@.lookup(t).start_offset@ == t
        && lo@ <= slot_off(r.slabs@.lookup(t), k) && slot_off(r.slabs@.lookup(t), k) + r.slabs@.lookup(t).element_size@ <= hi@ } };
    res
}

/// Mutant: claims a reservation may end past `hi`.
/// LEVEL-3 (phase D-B2): mutant with EXACTLY the property's requires and body; it claims the
/// negation of the property's first postcondition.
#[requires(lg(*r, hs@) && lop_pre(*r, hs@, op) && rg_within(*r, lo@, hi@))]
#[requires(sz@ > 0 && a_3783f8(sz@, r.format_params.sector_size@))]
#[ensures(!(forall<t: Int, k: Int> slot_live(^r, t, k) ==>
    lo@ <= slot_off((^r).slabs@.lookup(t), k) && slot_off((^r).slabs@.lookup(t), k) + (^r).slabs@.lookup(t).element_size@ <= hi@))]
pub fn verify_em_inv_within_usable_range__mutant(r: &mut RegionState, hs: &mut Vec<(u64, usize)>, op: LOp, sz: u32, lo: u64, hi: u64)
    -> Result<(u64, usize, u64), EmError> {
    let a0 = snapshot! { *r };
    apply_lop(r, hs, op);
    proof_assert! { r.format_params == a0.format_params && rg_within(*r, lo@, hi@) };
    let res = reserve_h(r, hs, sz);
    proof_assert! { rg_within(*r, lo@, hi@) };
    proof_assert! { forall<t: Int, k: Int> slot_live(*r, t, k) ==> {
        lemma_slot_in_block(*r, t, k);
        slab_ok(*r, r.slabs@.lookup(t)) && r.slabs@.lookup(t).start_offset@ == t
        && lo@ <= slot_off(r.slabs@.lookup(t), k) && slot_off(r.slabs@.lookup(t), k) + r.slabs@.lookup(t).element_size@ <= hi@ } };
    res
}

// ======================================================== mark_allocated (buddy.rs:117-157)

/// Nothing becomes free: a free byte of `b` was free in `a`, when every interval that avoided
/// `a`'s free blocks avoids `b`'s.
#[logic]
#[requires(ss > 0 && forall<y: Int, l: Int> 0 < l && faway(a, ss, y, l) ==> faway(b, ss, y, l))]
#[ensures(forall<z: Int> ffree(b, ss, z) ==> ffree(a, ss, z))]
pub fn lemma_free_sub(a: Seq<Vec<u64>>, b: Seq<Vec<u64>>, ss: Int) {
    pearlite! {
        proof_assert! { forall<o: Int> 0 <= o ==> { lemma_span_pos(ss, o); span(ss, o) > 0 } };
        proof_assert! { forall<z: Int> !ffree(a, ss, z) ==> faway(a, ss, z, 1) };
        proof_assert! { forall<z: Int> faway(b, ss, z, 1) ==> !ffree(b, ss, z) }
    }
}

/// **EM-BUDDY-MARK-ALLOCATED** — "Marking a free range as allocated (during recovery) removes
/// exactly that range from the free space, reducing the free total by its block size, and
/// leaves every other free byte free." For an order-K block `[x, x + span(K))` that is free —
/// it lies inside one free block, the shape of every recovered slab at initialize() (proved
/// for the rebuild: `rebuilt_region`, `lemma_rb_step`) — with disjoint free lists and a
/// power-of-two sector size: afterwards no free block meets the range, every other free byte
/// is still free, nothing else became free, the free total dropped by `span(K)`, and the free
/// blocks are still disjoint.
#[requires(bd_inv(*b) && abs@ >= b.base_offset@ && ord(size@ / b.sector_size@) <= b.max_order@)]
#[requires(mk_hyp(b.free_lists@, b.sector_size@, abs@ - b.base_offset@, ord(size@ / b.sector_size@)))]
#[ensures(faway((^b).free_lists@, b.sector_size@, abs@ - b.base_offset@, span(b.sector_size@, ord(size@ / b.sector_size@))))]
#[ensures(forall<z: Int> ffree(b.free_lists@, b.sector_size@, z)
    && !(abs@ - b.base_offset@ <= z && z < abs@ - b.base_offset@ + span(b.sector_size@, ord(size@ / b.sector_size@))) ==>
    ffree((^b).free_lists@, b.sector_size@, z))]
#[ensures(forall<z: Int> ffree((^b).free_lists@, b.sector_size@, z) ==> ffree(b.free_lists@, b.sector_size@, z)
    && !(abs@ - b.base_offset@ <= z && z < abs@ - b.base_offset@ + span(b.sector_size@, ord(size@ / b.sector_size@))))]
#[ensures(tf((^b).free_lists@, b.sector_size@, (^b).free_lists@.len())
    == tf(b.free_lists@, b.sector_size@, b.free_lists@.len()) - span(b.sector_size@, ord(size@ / b.sector_size@)))]
#[ensures(fdisj((^b).free_lists@, b.sector_size@) && bd_inv(^b))]
pub fn verify_em_buddy_mark_allocated(b: &mut BuddyAllocator, abs: u64, size: u64) {
    let old = snapshot! { *b };
    b.mark_allocated(abs, size);
    proof_assert! { lemma_mk_spec_elim(old.free_lists@, b.free_lists@, old.sector_size@, abs@ - old.base_offset@, ord(size@ / old.sector_size@));
        mk_post(old.free_lists@, b.free_lists@, old.sector_size@, abs@ - old.base_offset@, ord(size@ / old.sector_size@)) };
    proof_assert! { lemma_free_sub(old.free_lists@, b.free_lists@, old.sector_size@); true };
    proof_assert! { lemma_span_pos(old.sector_size@, ord(size@ / old.sector_size@)); true };
    proof_assert! { forall<z: Int> ffree(b.free_lists@, old.sector_size@, z) ==>
        !(abs@ - old.base_offset@ <= z && z < abs@ - old.base_offset@ + span(old.sector_size@, ord(size@ / old.sector_size@))) };
}

/// Mutant: claims marking leaves the free total unchanged.
#[requires(bd_inv(*b) && abs@ >= b.base_offset@ && ord(size@ / b.sector_size@) <= b.max_order@)]
#[requires(mk_hyp(b.free_lists@, b.sector_size@, abs@ - b.base_offset@, ord(size@ / b.sector_size@)))]
#[ensures(tf((^b).free_lists@, b.sector_size@, (^b).free_lists@.len()) == tf(b.free_lists@, b.sector_size@, b.free_lists@.len()))]
pub fn verify_em_buddy_mark_allocated__mutant(b: &mut BuddyAllocator, abs: u64, size: u64) {
    let old = snapshot! { *b };
    b.mark_allocated(abs, size);
    proof_assert! { lemma_mk_spec_elim(old.free_lists@, b.free_lists@, old.sector_size@, abs@ - old.base_offset@, ord(size@ / old.sector_size@)); true };
}

// ================================================== histories since format() (used bytes)

/// Every live slot is a live handle, a queued deferred free, or a published extent.
#[logic(open)]
pub fn cl(r: RegionState, hs: Seq<(u64, usize)>) -> bool {
    pearlite! {
        forall<t: Int, j: Int> slot_live(r, t, j) ==>
            r.slabs@.lookup(t).keys@[j] != FREE_KEY || inpend(r.pending_frees@, t, j) || inpend(hs, t, j)
    }
}

/// The history invariant of a region since format(): `lg`, no empty slab, and every live
/// slot accounted for.
#[logic(open)]
pub fn lgf(r: RegionState, hs: Seq<(u64, usize)>) -> bool {
    pearlite! { lg(r, hs) && ne(r) && cl(r, hs) }
}

/// Dropping element `j` keeps every other element.
#[logic]
#[requires(subl(y, x, j) && 0 <= q && q < x.len() && q != j)]
#[ensures(inpend(y, x[q].0@, x[q].1@))]
pub fn lemma_subl_keep(y: Seq<(u64, usize)>, x: Seq<(u64, usize)>, j: Int, q: Int) {
    pearlite! { if q < j { proof_assert! { y[q] == x[q] } } else { proof_assert! { y[q - 1] == x[q] } } }
}

#[logic]
#[requires(lgf(a, hs) && lg(b, hs2) && b.pending_frees == a.pending_frees)]
#[requires(forall<t: Int, j: Int> slot_live(b, t, j) ==> slot_live(a, t, j) && b.slabs@.lookup(t).keys@[j] == a.slabs@.lookup(t).keys@[j]
    && (t != s || j != i))]
#[requires(forall<t: Int, j: Int> inpend(hs, t, j) && (t != s || j != i) ==> inpend(hs2, t, j))]
#[requires(ne(b))]
#[ensures(lgf(b, hs2))]
pub fn lemma_cl_shrink(a: RegionState, b: RegionState, hs: Seq<(u64, usize)>, hs2: Seq<(u64, usize)>, s: Int, i: Int) {}

/// After alloc_extent's success every live slot is the new one or an old live slot with the
/// same key, and no slab is empty.
#[logic]
#[requires(lg(a, hs) && ne(a) && rg_inv(b) && rg_frame(a, b) && b.pending_frees == a.pending_frees)]
#[requires(alloc_any(a, b, es, Ok((s, i, off))))]
#[requires(b.slabs@.contains(s@) && i@ < b.slabs@.lookup(s@).bitmap.num_slots@ && slot_bit(b.slabs@.lookup(s@).bitmap, i@))]
#[ensures(forall<t: Int, j: Int> slot_live(b, t, j) ==> (t == s@ && j == i@)
    || (slot_live(a, t, j) && b.slabs@.lookup(t).keys@[j] == a.slabs@.lookup(t).keys@[j]))]
#[ensures(ne(b))]
pub fn lemma_reserve_live(a: RegionState, b: RegionState, es: Int, s: u64, i: usize, off: u64, hs: Seq<(u64, usize)>) {
    pearlite! {
        lemma_alloc_any_read(a, b, es, Ok((s, i, off)));
        if b.buddy == a.buddy && a.slabs@.contains(s@)
            && b.slabs@ == a.slabs@.insert(s@, b.slabs@.lookup(s@))
            && slot_step(a.slabs@.lookup(s@), b.slabs@.lookup(s@), i@) {
            proof_assert! { forall<k: Int> k != s@ ==> b.slabs@.get(k) == a.slabs@.get(k) };
            proof_assert! { b.slabs@.lookup(s@).bitmap.allocated_count@ >= 1 };
            proof_assert! { forall<j: Int> 0 <= j && j < a.slabs@.lookup(s@).bitmap.num_slots@ && j != i@ ==>
                slot_bit(b.slabs@.lookup(s@).bitmap, j) == slot_bit(a.slabs@.lookup(s@).bitmap, j) };
            proof_assert! { b.slabs@.lookup(s@).keys == a.slabs@.lookup(s@).keys };
            ()
        } else {
            proof_assert! { forall<fo: Int> alloc_post(a.buddy, b.buddy, slab_k(a), fo, s@) ==> {
                lemma_reserve_fresh(a, b, es, s, i, fo, hs); !a.slabs@.contains(s@) } };
            proof_assert! { !a.slabs@.contains(s@) };
            proof_assert! { forall<k: Int> k != s@ ==> b.slabs@.get(k) == a.slabs@.get(k) };
            proof_assert! { fresh_one(b.slabs@.lookup(s@), s@, a.format_params.slab_size@, es) };
            proof_assert! { i@ == 0 };
            proof_assert! { b.slabs@.lookup(s@).bitmap.allocated_count@ == 1 };
            proof_assert! { forall<j: Int> slot_live(b, s@, j) ==> j == 0 };
            proof_assert! { forall<t: Int, j: Int> slot_live(b, t, j) && t != s@ ==> slot_live(a, t, j) && b.slabs@.lookup(t) == a.slabs@.lookup(t) };
            ()
        }
    }
}

/// alloc_extent keeps the format-history invariant (the new live slot is the new handle).
#[logic]
#[requires(lgf(a, hs) && rg_inv(b) && rg_frame(a, b) && b.pending_frees == a.pending_frees)]
#[requires(alloc_any(a, b, es, res) && match res { Err(_) => rg_tf(b) == rg_tf(a), Ok(_) => true })]
#[requires(forall<s: u64, i: usize, off: u64> res == Ok((s, i, off)) ==>
    b.slabs@.contains(s@) && i@ < b.slabs@.lookup(s@).bitmap.num_slots@ && slot_bit(b.slabs@.lookup(s@).bitmap, i@))]
#[ensures(match res { Ok((s, i, _)) => lgf(b, hs.push_back((s, i))), Err(_) => lgf(b, hs) })]
pub fn lemma_reserve_f(a: RegionState, b: RegionState, es: Int, res: Result<(u64, usize, u64), EmError>, hs: Seq<(u64, usize)>) {
    pearlite! {
        lemma_reserve_good(a, b, es, res, hs);
        match res {
            Ok((s, i, off)) => {
                lemma_reserve_live(a, b, es, s, i, off, hs);
                proof_assert! { forall<p: Int> 0 <= p && p < hs.len() ==> hs.push_back((s, i))[p] == hs[p] };
                proof_assert! { hs.push_back((s, i))[hs.len()] == (s, i) };
                proof_assert! { forall<t: Int, j: Int> inpend(hs, t, j) ==> inpend(hs.push_back((s, i)), t, j) };
                proof_assert! { inpend(hs.push_back((s, i)), s@, i@) };
                proof_assert! { forall<t: Int, j: Int> slot_live(b, t, j) && (t != s@ || j != i@) ==>
                    slot_live(a, t, j) && b.slabs@.lookup(t).keys@[j] == a.slabs@.lookup(t).keys@[j] };
                proof_assert! { ne(b) && cl(b, hs.push_back((s, i))) };
                ()
            },
            Err(_) => {
                lemma_alloc_any_read(a, b, es, res);
                proof_assert! { b.slabs@ == a.slabs@ };
                proof_assert! { ne(b) && cl(b, hs) }; ()
            },
        }
    }
}

/// free_slot through a live handle keeps the format-history invariant.
#[logic]
#[requires(lgf(a, hs) && subl(hs2, hs, j) && hs[j] == (s, i))]
#[requires(rg_inv(b) && rg_frame(a, b) && b.pending_frees == a.pending_frees)]
#[requires(a.slabs@.get(s@) == Some(sl) && free_case(a, b, s, i, sl))]
#[requires(sl.bitmap.allocated_count@ == 1 ==>
    free_e(a.buddy.free_lists@, b.buddy.free_lists@, a.buddy.sector_size@, s@ - a.buddy.base_offset@, sk(a)))]
#[requires(rg_tf(b) == rg_tf(a) + if sl.bitmap.allocated_count@ == 1 { sk(a) } else { 0 })]
#[ensures(lgf(b, hs2))]
pub fn lemma_lgf_free(a: RegionState, b: RegionState, s: u64, i: usize, sl: Slab, hs: Seq<(u64, usize)>, hs2: Seq<(u64, usize)>, j: Int) {
    pearlite! {
        lemma_abort_good(a, b, s, i, sl, hs, hs2, j);
        lemma_free_back(a, b, s, i, sl);
        lemma_free_ne(a, b, s, i, sl);
        proof_assert! { forall<q: Int> 0 <= q && q < hs.len() && q != j ==> { lemma_subl_keep(hs2, hs, j, q); inpend(hs2, hs[q].0@, hs[q].1@) } };
        proof_assert! { forall<t: Int, k: Int> inpend(hs, t, k) && (t != s@ || k != i@) ==> inpend(hs2, t, k) };
        lemma_cl_shrink(a, b, hs, hs2, s@, i@)
    }
}

/// publish_slot through a live handle (key != FREE_KEY) keeps the format-history invariant.
#[logic]
#[requires(lgf(a, hs) && subl(hs2, hs, j) && hs[j] == (s, i) && key != FREE_KEY)]
#[requires(rg_inv(b) && rg_frame(a, b) && b.buddy == a.buddy && b.pending_frees == a.pending_frees && b.size_classes == a.size_classes)]
#[requires(a.slabs@.contains(s@) && b.slabs@ == a.slabs@.insert(s@, b.slabs@.lookup(s@)))]
#[requires(b.slabs@.lookup(s@).keys@ == a.slabs@.lookup(s@).keys@.set(i@, key))]
#[requires(b.slabs@.lookup(s@).bitmap == a.slabs@.lookup(s@).bitmap && b.slabs@.lookup(s@).element_size == a.slabs@.lookup(s@).element_size)]
#[requires(b.slabs@.lookup(s@).start_offset == a.slabs@.lookup(s@).start_offset && b.slabs@.lookup(s@).slab_size == a.slabs@.lookup(s@).slab_size)]
#[ensures(lgf(b, hs2))]
pub fn lemma_lgf_publish(a: RegionState, b: RegionState, s: u64, i: usize, key: u64, hs: Seq<(u64, usize)>, hs2: Seq<(u64, usize)>, j: Int) {
    pearlite! {
        lemma_publish_good(a, b, s, i, key, hs, hs2, j);
        proof_assert! { slab_inv(a.slabs@.lookup(s@)) && i@ < a.slabs@.lookup(s@).keys@.len() };
        proof_assert! { forall<k: Int> k != s@ ==> b.slabs@.get(k) == a.slabs@.get(k) };
        proof_assert! { forall<q: Int> 0 <= q && q < hs.len() && q != j ==> { lemma_subl_keep(hs2, hs, j, q); inpend(hs2, hs[q].0@, hs[q].1@) } };
        proof_assert! { forall<t: Int, k: Int> slot_live(b, t, k) ==> slot_live(a, t, k) };
        proof_assert! { b.slabs@.lookup(s@).keys@[i@] == key };
        proof_assert! { forall<t: Int, k: Int> slot_live(b, t, k) && (t != s@ || k != i@) ==> b.slabs@.lookup(t).keys@[k] == a.slabs@.lookup(t).keys@[k] };
        proof_assert! { ne(b) && cl(b, hs2) }
    }
}

/// remove_extent_by_offset's success keeps the format-history invariant.
#[logic]
#[requires(lgf(a, hs) && rg_inv(b) && rm_case(a, off, s, i) && rm_post(a, b, s, i))]
#[ensures(lgf(b, hs))]
pub fn lemma_lgf_remove(a: RegionState, b: RegionState, off: u64, s: u64, i: usize, hs: Seq<(u64, usize)>) {
    pearlite! {
        lemma_remove_one(a, b, off, s, i, hs);
        proof_assert! { b.pending_frees@ == a.pending_frees@.push_back((s, i)) };
        proof_assert! { forall<p: Int> 0 <= p && p < a.pending_frees@.len() ==> b.pending_frees@[p] == a.pending_frees@[p] };
        proof_assert! { b.pending_frees@[a.pending_frees@.len()] == (s, i) };
        proof_assert! { inpend(b.pending_frees@, s@, i@) };
        proof_assert! { forall<t: Int, k: Int> inpend(a.pending_frees@, t, k) ==> inpend(b.pending_frees@, t, k) };
        proof_assert! { forall<k: Int> k != s@ ==> b.slabs@.get(k) == a.slabs@.get(k) };
        proof_assert! { forall<t: Int, k: Int> slot_live(b, t, k) ==> slot_live(a, t, k) };
        proof_assert! { ne(b) && cl(b, hs) }
    }
}

/// flush_pending_frees keeps the format-history invariant.
#[logic]
#[requires(lgf(a, hs) && fl_spec(a, b) && b.pending_frees@.len() == 0 && flushed_all(a, b))]
#[ensures(lgf(b, hs))]
pub fn lemma_lgf_flush(a: RegionState, b: RegionState, hs: Seq<(u64, usize)>) {
    pearlite! {
        lemma_flush_good(a, b, hs);
        lemma_fl_spec_elim(a, b);
        proof_assert! { forall<t: Int, k: Int> slot_live(b, t, k) ==> keep_slot(b, a, t, k) && slot_live(a, t, k) };
        proof_assert! { forall<t: Int, k: Int> slot_live(b, t, k) ==> !inpend(a.pending_frees@, t, k) };
        proof_assert! { ne(b) && cl(b, hs) }
    }
}

/// apply_lop with the format-history invariant.
#[requires(lgf(*r, hs@) && lop_pre(*r, hs@, op))]
#[ensures(lgf(^r, (^hs)@) && rg_frame(*r, ^r))]
pub fn apply_lopf(r: &mut RegionState, hs: &mut Vec<(u64, usize)>, op: LOp) {
    let a = snapshot! { *r };
    let h0 = snapshot! { hs@ };
    match op {
        LOp::Reserve(sz) => {
            let res = r.alloc_extent(sz);
            proof_assert! { lemma_reserve_f(*a, *r, align_l(sz@, a.format_params.sector_size@), res, *h0); true };
            match res {
                Ok((s, i, _)) => hs.push((s, i)),
                Err(_) => {}
            }
        }
        LOp::Publish(j, key) => {
            let (s, i) = hs[j];
            proof_assert! { h0[j@] == (s, i) && slot_live(*a, s@, i@) };
            let sl = snapshot! { a.slabs@.lookup(s@) };
            if key == FREE_KEY {
                r.free_slot(s, i);
                let _ = hs.remove(j);
                proof_assert! { lemma_subl(hs@, *h0, j@); lemma_lgf_free(*a, *r, s, i, *sl, *h0, hs@, j@); lgf(*r, hs@) };
            } else {
                r.publish_slot(s, i, key);
                let _ = hs.remove(j);
                proof_assert! { lemma_subl(hs@, *h0, j@); lemma_lgf_publish(*a, *r, s, i, key, *h0, hs@, j@); lgf(*r, hs@) };
            }
        }
        LOp::Abort(j) => {
            let (s, i) = hs[j];
            proof_assert! { h0[j@] == (s, i) && slot_live(*a, s@, i@) };
            let sl = snapshot! { a.slabs@.lookup(s@) };
            r.free_slot(s, i);
            let _ = hs.remove(j);
            proof_assert! { lemma_subl(hs@, *h0, j@); lemma_lgf_free(*a, *r, s, i, *sl, *h0, hs@, j@); lgf(*r, hs@) };
        }
        LOp::Remove(off) => {
            let res = r.remove_extent_by_offset(off);
            proof_assert! { res == Ok(()) ==> forall<s: u64, i: usize> rm_case(*a, off, s, i) && rm_post(*a, *r, s, i) ==> {
                lemma_lgf_remove(*a, *r, off, s, i, *h0); lgf(*r, *h0) } };
        }
        LOp::Flush => {
            r.flush_pending_frees();
            proof_assert! { lemma_lgf_flush(*a, *r, *h0); lgf(*r, *h0) };
        }
    }
}

/// No live handle, no queued free, no published extent: the region has no slab.
#[logic]
#[requires(lgf(r, hs) && hs.len() == 0 && r.pending_frees@.len() == 0 && forall<t: Int, k: Int> !pub_slot(r, t, k))]
#[ensures(r.slabs@.len() == 0 && rg_tf(r) == (r.buddy.total_usable_size@ / r.buddy.sector_size@) * r.buddy.sector_size@)]
pub fn lemma_all_released(r: RegionState, hs: Seq<(u64, usize)>) {
    pearlite! {
        proof_assert! { forall<t: Int, k: Int> !slot_live(r, t, k) };
        proof_assert! { forall<t: Int> r.slabs@.contains(t) ==> {
            lemma_cnt_zero_all(r.slabs@.lookup(t).bitmap, r.slabs@.lookup(t).bitmap.num_slots@);
            proof_assert! { slab_ok(r, r.slabs@.lookup(t)) };
            false } };
        proof_assert! { r.slabs@.ext_eq(FMap::empty()) };
        proof_assert! { r.slabs@ == FMap::empty() };
        proof_assert! { rg_acct(r) }
    }
}

/// **EM-USED-RETURNS-AFTER-FREE** — "When every reservation has been aborted or published and
/// later removed, and a checkpoint has completed after the last removal, used_bytes() returns
/// to the value it had right after format()." For every region history since format() (the
/// fresh region satisfies `lgf`; every call preserves it): once no handle is live, nothing is
/// published and no removal is queued, the region holds no slab and its used bytes
/// (`total_usable_size - total_free`, lib.rs:704) equal those of the freshly formatted region.
#[requires(lgf(*r, hs@) && lop_pre(*r, hs@, op))]
#[requires(p2(fp.sector_size@))]
#[requires(rg_in(base@, size@, fp))]
#[requires(r.buddy.base_offset == base && r.buddy.total_usable_size == size && r.format_params == fp)]
#[ensures(lgf(^r, (^hs)@))]
#[ensures(lgf(result, Seq::empty()) && result.buddy.total_usable_size == size && result.format_params == fp)]
#[ensures((^hs)@.len() == 0 && (^r).pending_frees@.len() == 0 && (forall<t: Int, k: Int> !pub_slot(^r, t, k)) ==>
    rg_used(^r) == rg_used(result))]
pub fn narrowed_verify_em_used_returns_after_free(r: &mut RegionState, hs: &mut Vec<(u64, usize)>, op: LOp, base: u64, size: u64, fp: FormatParams)
    -> RegionState {
    proof_assert! { lemma_rg_in(base@, size@, fp); rg_hd(base@, size@, fp) };
    let f = fresh_region(base, size, fp);
    proof_assert! { ne(f) && cl(f, Seq::empty()) };
    apply_lopf(r, hs, op);
    proof_assert! { (hs@.len() == 0 && r.pending_frees@.len() == 0 && (forall<t: Int, k: Int> !pub_slot(*r, t, k))) ==> {
        lemma_all_released(*r, hs@);
        rg_tf(*r) == rg_tf(f) } };
    f
}

/// Mutant: claims used bytes stay above the post-format value.
/// LEVEL-3 (phase D-B2): mutant with EXACTLY the property's requires and body; it claims the
/// negation of the property's first postcondition.
#[requires(lgf(*r, hs@) && lop_pre(*r, hs@, op))]
#[requires(p2(fp.sector_size@))]
#[requires(rg_in(base@, size@, fp))]
#[requires(r.buddy.base_offset == base && r.buddy.total_usable_size == size && r.format_params == fp)]
#[ensures(!(lgf(^r, (^hs)@)))]
pub fn narrowed_verify_em_used_returns_after_free__mutant(r: &mut RegionState, hs: &mut Vec<(u64, usize)>, op: LOp, base: u64, size: u64, fp: FormatParams)
    -> RegionState {
    proof_assert! { lemma_rg_in(base@, size@, fp); rg_hd(base@, size@, fp) };
    let f = fresh_region(base, size, fp);
    proof_assert! { ne(f) && cl(f, Seq::empty()) };
    apply_lopf(r, hs, op);
    proof_assert! { (hs@.len() == 0 && r.pending_frees@.len() == 0 && (forall<t: Int, k: Int> !pub_slot(*r, t, k))) ==> {
        lemma_all_released(*r, hs@);
        rg_tf(*r) == rg_tf(f) } };
    f
}

// ======================================================================== used bytes

/// `0 <= used <= capacity` for each region: `rg_used = total_usable_size - total_free`.
#[logic]
#[requires(rg_good(r))]
#[ensures(0 <= rg_used(r) && rg_used(r) <= r.buddy.total_usable_size@)]
#[ensures(rg_used(r) >= r.slabs@.len() * sk(r))]
pub fn lemma_used_bounds(r: RegionState) {
    pearlite! {
        lemma_tf_le(r);
        proof_assert! { rg_core(r) && rg_acct(r) };
        lemma_divmod(r.buddy.total_usable_size@, r.buddy.sector_size@);
        proof_assert! { (r.buddy.total_usable_size@ / r.buddy.sector_size@) * r.buddy.sector_size@ <= r.buddy.total_usable_size@ }
    }
}

/// Summing per-region bounds: used bytes never exceed the capacity.
#[logic]
#[variant(n)]
#[requires(0 <= n && n <= rv.len())]
#[requires(forall<i: Int> 0 <= i && i < rv.len() ==> 0 <= rg_used(arena[rv[i]@]) && rg_used(arena[rv[i]@]) <= arena[rv[i]@].buddy.total_usable_size@)]
#[ensures(0 <= used_sum(arena, rv, n) && used_sum(arena, rv, n) <= cap_sum(arena, rv, n))]
pub fn lemma_used_le_cap(arena: Seq<RegionState>, rv: Seq<usize>, n: Int) {
    if n > 0 {
        lemma_used_le_cap(arena, rv, n - 1)
    }
}

/// **EM-USED-AT-MOST-CAPACITY** — "used_bytes() is always less than or equal to
/// capacity_bytes() (equality is allowed when the device is completely full)." (a) Every
/// reachable region (after any call; format's and initialize's regions) has
/// `0 <= total_usable_size - total_free <= total_usable_size`; (b) for a component whose
/// current regions are reachable regions (`rg_good`), used_bytes() (lib.rs:698-708) <=
/// capacity_bytes() (lib.rs:710-715).
// LEVEL-3 (phase D-B2): the component part's capacity bound is the PROVED invariant em_cap_ok
// (D-B1, props/l3start.rs inv_em_cap_ok) instead of a bare `cap_sum <= u64::MAX`; each current
// region is a reachable region (rg_good, part of the PROVED lg — 217ace).
#[requires(lg(*r, hs@) && lop_pre(*r, hs@, op))]
#[requires(p2(fp.sector_size@))]
#[requires(rg_in(base@, size@, fp))]
#[requires(forall<p: Int> 0 <= p && p < descs@.len() ==> desc_ok_a(descs@[p], base@, size@, fp))]
#[requires(a_980f61_list(descs@, base@, size@))]
#[requires(em_regions_wf(*em) && em_cap_ok(*em))]
#[requires(match em.regions { Some(rv) => forall<i: Int> 0 <= i && i < rv@.len() ==> rg_good(em.arena@[rv@[i]@]), None => true })]
#[ensures(0 <= rg_used(^r) && rg_used(^r) <= (^r).buddy.total_usable_size@)]
#[ensures(rg_used(result.0) <= size@ && rg_used(result.1) <= size@)]
#[ensures(result.2@ <= result.3@)]
pub fn verify_em_used_at_most_capacity(r: &mut RegionState, hs: &mut Vec<(u64, usize)>, op: LOp, base: u64, size: u64,
    fp: FormatParams, descs: &Vec<SlabDescriptor>, em: &ExtentManager) -> (RegionState, RegionState, u64, u64) {
    proof_assert! { lemma_rg_in(base@, size@, fp); rg_hd(base@, size@, fp) };
    proof_assert! { forall<p: Int> 0 <= p && p < descs@.len() ==> { lemma_desc_ok_a(descs@[p], base@, size@, fp); desc_ok(descs@[p], base@, size@, fp) } };
    proof_assert! { forall<p: Int> 0 <= p && p < descs@.len() ==> descs@[p].slab_size@ > 0 };
    proof_assert! { lemma_starts_distinct(descs@, base@, size@); starts_distinct(descs@) };
    let (f, g) = all3(r, hs, op, base, size, fp, descs);
    proof_assert! { lemma_used_bounds(*r); lemma_used_bounds(f); lemma_used_bounds(g); true };
    proof_assert! { match em.regions { Some(rv) => {
        proof_assert! { forall<i: Int> 0 <= i && i < rv@.len() ==> {
            lemma_used_bounds(em.arena@[rv@[i]@]); lemma_tf_le(em.arena@[rv@[i]@]);
            rg_inv(em.arena@[rv@[i]@]) && 0 <= rg_used(em.arena@[rv@[i]@])
            && rg_used(em.arena@[rv@[i]@]) <= em.arena@[rv@[i]@].buddy.total_usable_size@ } };
        lemma_used_le_cap(em.arena@, rv@, rv@.len());
        used_terms_ok(em.arena@, rv@) && used_sum(em.arena@, rv@, rv@.len()) <= cap_sum(em.arena@, rv@, rv@.len()) },
        None => true } };
    let u = em.used_bytes();
    let c = em.capacity_bytes();
    (f, g, u, c)
}

/// Mutant: claims used bytes exceed the capacity.
/// LEVEL-3 (phase D-B2): mutant with EXACTLY the property's requires and body; it claims the
/// negation of the property's first postcondition.
/// LEVEL-3 (phase D-B2): mutant with EXACTLY the property's requires and body; it claims the
/// negation of the property's first postcondition.
// LEVEL-3 (phase D-B2): the component part's capacity bound is the PROVED invariant em_cap_ok
// (D-B1, props/l3start.rs inv_em_cap_ok) instead of a bare `cap_sum <= u64::MAX`; each current
// region is a reachable region (rg_good, part of the PROVED lg — 217ace).
#[requires(lg(*r, hs@) && lop_pre(*r, hs@, op))]
#[requires(p2(fp.sector_size@))]
#[requires(rg_in(base@, size@, fp))]
#[requires(forall<p: Int> 0 <= p && p < descs@.len() ==> desc_ok_a(descs@[p], base@, size@, fp))]
#[requires(a_980f61_list(descs@, base@, size@))]
#[requires(em_regions_wf(*em) && em_cap_ok(*em))]
#[requires(match em.regions { Some(rv) => forall<i: Int> 0 <= i && i < rv@.len() ==> rg_good(em.arena@[rv@[i]@]), None => true })]
#[ensures(!(0 <= rg_used(^r) && rg_used(^r) <= (^r).buddy.total_usable_size@))]
pub fn verify_em_used_at_most_capacity__mutant(r: &mut RegionState, hs: &mut Vec<(u64, usize)>, op: LOp, base: u64, size: u64,
    fp: FormatParams, descs: &Vec<SlabDescriptor>, em: &ExtentManager) -> (RegionState, RegionState, u64, u64) {
    proof_assert! { lemma_rg_in(base@, size@, fp); rg_hd(base@, size@, fp) };
    proof_assert! { forall<p: Int> 0 <= p && p < descs@.len() ==> { lemma_desc_ok_a(descs@[p], base@, size@, fp); desc_ok(descs@[p], base@, size@, fp) } };
    proof_assert! { forall<p: Int> 0 <= p && p < descs@.len() ==> descs@[p].slab_size@ > 0 };
    proof_assert! { lemma_starts_distinct(descs@, base@, size@); starts_distinct(descs@) };
    let (f, g) = all3(r, hs, op, base, size, fp, descs);
    proof_assert! { lemma_used_bounds(*r); lemma_used_bounds(f); lemma_used_bounds(g); true };
    proof_assert! { match em.regions { Some(rv) => {
        proof_assert! { forall<i: Int> 0 <= i && i < rv@.len() ==> {
            lemma_used_bounds(em.arena@[rv@[i]@]); lemma_tf_le(em.arena@[rv@[i]@]);
            rg_inv(em.arena@[rv@[i]@]) && 0 <= rg_used(em.arena@[rv@[i]@])
            && rg_used(em.arena@[rv@[i]@]) <= em.arena@[rv@[i]@].buddy.total_usable_size@ } };
        lemma_used_le_cap(em.arena@, rv@, rv@.len());
        used_terms_ok(em.arena@, rv@) && used_sum(em.arena@, rv@, rv@.len()) <= cap_sum(em.arena@, rv@, rv@.len()) },
        None => true } };
    let u = em.used_bytes();
    let c = em.capacity_bytes();
    (f, g, u, c)
}

/// `ks` lists every slab start of `r` exactly once.
#[logic(open)]
pub fn enum_slabs(r: RegionState, ks: Seq<Int>) -> bool {
    pearlite! {
        (forall<p: Int> 0 <= p && p < ks.len() ==> r.slabs@.contains(ks[p]))
        && (forall<k: Int> r.slabs@.contains(k) ==> exists<p: Int> 0 <= p && p < ks.len() && ks[p] == k)
        && (forall<p: Int, q: Int> 0 <= p && p < q && q < ks.len() ==> ks[p] != ks[q])
    }
}

/// Bytes of the in-use (live: reserved, published, or removed-not-yet-released) slots of the
/// slabs `ks[0..n]`: each slab contributes allocated_count * element_size.
#[logic(open)]
#[variant(n)]
pub fn live_sum(r: RegionState, ks: Seq<Int>, n: Int) -> Int {
    pearlite! {
        if n <= 0 { 0 } else {
            live_sum(r, ks, n - 1) + r.slabs@.lookup(ks[n - 1]).bitmap.allocated_count@ * r.slabs@.lookup(ks[n - 1]).element_size@
        }
    }
}

/// An enumeration of a map's keys has the map's length.
#[logic]
#[variant(ks.len())]
#[requires(forall<p: Int> 0 <= p && p < ks.len() ==> m.contains(ks[p]))]
#[requires(forall<k: Int> m.contains(k) ==> exists<p: Int> 0 <= p && p < ks.len() && ks[p] == k)]
#[requires(forall<p: Int, q: Int> 0 <= p && p < q && q < ks.len() ==> ks[p] != ks[q])]
#[ensures(ks.len() == m.len())]
pub fn lemma_enum_len(m: FMap<Int, Slab>, ks: Seq<Int>) {
    pearlite! {
        if ks.len() == 0 {
            proof_assert! { m.ext_eq(FMap::empty()) };
            ()
        } else {
            let last = ks[ks.len() - 1];
            let m2 = m.remove(last);
            let ks2 = ks.subsequence(0, ks.len() - 1);
            proof_assert! { forall<p: Int> 0 <= p && p < ks2.len() ==> ks2[p] == ks[p] && ks2[p] != last };
            proof_assert! { forall<k: Int> m2.contains(k) ==> m.contains(k) && k != last };
            proof_assert! { forall<k: Int> m2.contains(k) ==> exists<p: Int> 0 <= p && p < ks2.len() && ks2[p] == k };
            lemma_enum_len(m2, ks2)
        }
    }
}

/// Each slab's in-use bytes fit in its block: the live sum is at most one block per slab.
#[logic]
#[variant(n)]
#[requires(rg_inv(r) && 0 <= n && n <= ks.len() && forall<p: Int> 0 <= p && p < ks.len() ==> r.slabs@.contains(ks[p]))]
#[ensures(0 <= live_sum(r, ks, n) && live_sum(r, ks, n) <= n * sk(r))]
pub fn lemma_live_sum_le(r: RegionState, ks: Seq<Int>, n: Int) {
    pearlite! {
        if n > 0 {
            lemma_live_sum_le(r, ks, n - 1);
            let s = r.slabs@.lookup(ks[n - 1]);
            proof_assert! { slab_ok(r, s) && slab_inv(s) };
            lemma_cnt_bounds(s.bitmap, s.bitmap.num_slots@);
            lemma_slots_fit(s);
            lemma_slab_fits(r);
            lemma_mul_le(s.bitmap.allocated_count@, s.bitmap.num_slots@, s.element_size@);
            lemma_mul_le(0, s.bitmap.allocated_count@, s.element_size@);
            proof_assert! { s.bitmap.allocated_count@ * s.element_size@ <= sk(r) };
            lemma_distrib(n - 1, 1, sk(r))
        }
    }
}

/// The in-use bytes of a reachable region, over any slab enumeration, are at most its used bytes.
#[logic]
#[requires(rg_good(r))]
#[ensures(forall<ks: Seq<Int>> enum_slabs(r, ks) ==> live_sum(r, ks, ks.len()) <= rg_used(r))]
pub fn lemma_covers(r: RegionState) {
    pearlite! {
        lemma_used_bounds(r);
        lemma_span_pos(r.buddy.sector_size@, slab_k(r));
        proof_assert! { forall<ks: Seq<Int>> enum_slabs(r, ks) ==> {
            lemma_enum_len(r.slabs@, ks); lemma_live_sum_le(r, ks, ks.len());
            live_sum(r, ks, ks.len()) <= ks.len() * sk(r) && ks.len() == r.slabs@.len() } }
    }
}

/// **EM-USED-COVERS-EXTENTS** — "used_bytes() is at least the total size of all extents that
/// are currently reserved, published, or removed but not yet released by a checkpoint."
/// Those are exactly the live (allocated) slots; their total size, summed over any
/// enumeration of the slabs, is at most the region's used bytes — on every reachable region
/// (after any call; format's and initialize's regions). Summed over regions this is
/// used_bytes() (lib.rs:698-708, `used_sum`).
#[requires(lg(*r, hs@) && lop_pre(*r, hs@, op))]
#[requires(p2(fp.sector_size@))]
#[requires(rg_in(base@, size@, fp))]
#[requires(forall<p: Int> 0 <= p && p < descs@.len() ==> desc_ok_a(descs@[p], base@, size@, fp))]
#[requires(a_980f61_list(descs@, base@, size@))]
#[ensures(forall<ks: Seq<Int>> enum_slabs(^r, ks) ==> live_sum(^r, ks, ks.len()) <= rg_used(^r))]
#[ensures(forall<ks: Seq<Int>> enum_slabs(result.0, ks) ==> live_sum(result.0, ks, ks.len()) <= rg_used(result.0))]
#[ensures(forall<ks: Seq<Int>> enum_slabs(result.1, ks) ==> live_sum(result.1, ks, ks.len()) <= rg_used(result.1))]
pub fn verify_em_used_covers_extents(r: &mut RegionState, hs: &mut Vec<(u64, usize)>, op: LOp, base: u64, size: u64,
    fp: FormatParams, descs: &Vec<SlabDescriptor>) -> (RegionState, RegionState) {
    proof_assert! { lemma_rg_in(base@, size@, fp); rg_hd(base@, size@, fp) };
    proof_assert! { forall<p: Int> 0 <= p && p < descs@.len() ==> { lemma_desc_ok_a(descs@[p], base@, size@, fp); desc_ok(descs@[p], base@, size@, fp) } };
    proof_assert! { forall<p: Int> 0 <= p && p < descs@.len() ==> descs@[p].slab_size@ > 0 };
    proof_assert! { lemma_starts_distinct(descs@, base@, size@); starts_distinct(descs@) };
    let (f, g) = all3(r, hs, op, base, size, fp, descs);
    proof_assert! { lemma_covers(*r); lemma_covers(f); lemma_covers(g); true };
    (f, g)
}

/// Mutant: claims the in-use bytes exceed the used bytes.
/// LEVEL-3 (phase D-B2): mutant with EXACTLY the property's requires and body; it claims the
/// negation of the property's first postcondition.
#[requires(lg(*r, hs@) && lop_pre(*r, hs@, op))]
#[requires(p2(fp.sector_size@))]
#[requires(rg_in(base@, size@, fp))]
#[requires(forall<p: Int> 0 <= p && p < descs@.len() ==> desc_ok_a(descs@[p], base@, size@, fp))]
#[requires(a_980f61_list(descs@, base@, size@))]
#[ensures(!(forall<ks: Seq<Int>> enum_slabs(^r, ks) ==> live_sum(^r, ks, ks.len()) <= rg_used(^r)))]
pub fn verify_em_used_covers_extents__mutant(r: &mut RegionState, hs: &mut Vec<(u64, usize)>, op: LOp, base: u64, size: u64,
    fp: FormatParams, descs: &Vec<SlabDescriptor>) -> (RegionState, RegionState) {
    proof_assert! { lemma_rg_in(base@, size@, fp); rg_hd(base@, size@, fp) };
    proof_assert! { forall<p: Int> 0 <= p && p < descs@.len() ==> { lemma_desc_ok_a(descs@[p], base@, size@, fp); desc_ok(descs@[p], base@, size@, fp) } };
    proof_assert! { forall<p: Int> 0 <= p && p < descs@.len() ==> descs@[p].slab_size@ > 0 };
    proof_assert! { lemma_starts_distinct(descs@, base@, size@); starts_distinct(descs@) };
    let (f, g) = all3(r, hs, op, base, size, fp, descs);
    proof_assert! { lemma_covers(*r); lemma_covers(f); lemma_covers(g); true };
    (f, g)
}

// ============================================================ get_extents (lib.rs:632-648)

/// The published slots change exactly as the abstract operation says.
#[logic(open)]
pub fn getx_step(a: RegionState, b: RegionState, hs: Seq<(u64, usize)>, op: LOp) -> bool {
    pearlite! {
        (match op {
            LOp::Publish(j, k) =>
                (k != FREE_KEY ==> (forall<t: Int, x: Int> pub_slot(b, t, x) == (pub_slot(a, t, x) || (t == hs[j@].0@ && x == hs[j@].1@)))
                    && b.slabs@.lookup(hs[j@].0@).keys@[hs[j@].1@] == k
                    && slot_off(b.slabs@.lookup(hs[j@].0@), hs[j@].1@) == slot_off(a.slabs@.lookup(hs[j@].0@), hs[j@].1@)
                    && b.slabs@.lookup(hs[j@].0@).element_size == a.slabs@.lookup(hs[j@].0@).element_size)
                && (k == FREE_KEY ==> forall<t: Int, x: Int> pub_slot(b, t, x) == pub_slot(a, t, x)),
            LOp::Remove(off) => (forall<t: Int, x: Int> pub_slot(b, t, x) == pub_slot(a, t, x))
                || (exists<s: u64, i: usize> rm_case(a, off, s, i)
                    && forall<t: Int, x: Int> pub_slot(b, t, x) == (pub_slot(a, t, x) && slot_off(a.slabs@.lookup(t), x) != off@)),
            _ => forall<t: Int, x: Int> pub_slot(b, t, x) == pub_slot(a, t, x),
        })
        && forall<t: Int, x: Int> pub_slot(a, t, x) && pub_slot(b, t, x) ==>
            b.slabs@.lookup(t).keys@[x] == a.slabs@.lookup(t).keys@[x]
            && b.slabs@.lookup(t).start_offset == a.slabs@.lookup(t).start_offset
            && b.slabs@.lookup(t).element_size == a.slabs@.lookup(t).element_size
    }
}

/// A published slot at `off` is what remove_extent_by_offset finds (the floor slab).
#[logic]
#[requires(rg_inv(a) && pub_slot(a, s@, i@) && slot_off(a.slabs@.lookup(s@), i@) == off@)]
#[requires(rg_ka(a))]
#[ensures(rm_case(a, off, s, i))]
pub fn lemma_pub_rm_case(a: RegionState, off: u64, s: u64, i: usize) {
    pearlite! {
        proof_assert! { slab_ok(a, a.slabs@.lookup(s@)) && a.slabs@.lookup(s@).start_offset@ == s@ };
        lemma_slot_inside(a.slabs@.lookup(s@), i@);
        proof_assert! { slab_inv(a.slabs@.lookup(s@)) && a.slabs@.lookup(s@).element_size@ > 0 };
        lemma_slab_fits(a);
        proof_assert! { forall<t2: Int> a.slabs@.contains(t2) && t2 <= off@ && t2 > s@ ==> {
            lemma_slab_blocks_disj(a, s@, t2); false } };
        proof_assert! { key_alloc(a.slabs@.lookup(s@)) && slot_bit(a.slabs@.lookup(s@).bitmap, i@) }
    }
}

/// A successful removal unpublishes exactly the extent at its offset.
#[logic]
#[requires(lg(a, hs) && rg_inv(b) && rm_case(a, off, s, i) && rm_post(a, b, s, i))]
#[ensures(forall<t: Int, x: Int> pub_slot(b, t, x) == (pub_slot(a, t, x) && slot_off(a.slabs@.lookup(t), x) != off@))]
#[ensures(forall<t: Int, x: Int> pub_slot(a, t, x) && pub_slot(b, t, x) ==>
    b.slabs@.lookup(t).keys@[x] == a.slabs@.lookup(t).keys@[x]
    && b.slabs@.lookup(t).start_offset == a.slabs@.lookup(t).start_offset
    && b.slabs@.lookup(t).element_size == a.slabs@.lookup(t).element_size)]
pub fn lemma_remove_getx(a: RegionState, b: RegionState, off: u64, s: u64, i: usize, hs: Seq<(u64, usize)>) {
    pearlite! {
        lemma_no_overlap(a);
        proof_assert! { forall<t: Int, x: Int> pub_slot(a, t, x) ==> slot_live(a, t, x) && slab_ok(a, a.slabs@.lookup(t)) && a.slabs@.lookup(t).element_size@ > 0 };
        proof_assert! { slot_live(a, s@, i@) && slot_off(a.slabs@.lookup(s@), i@) == off@ };
        proof_assert! { forall<t: Int, x: Int> pub_slot(a, t, x) && (t != s@ || x != i@) ==> slot_off(a.slabs@.lookup(t), x) != off@ };
        proof_assert! { forall<k: Int> k != s@ ==> b.slabs@.get(k) == a.slabs@.get(k) };
        proof_assert! { slab_inv(a.slabs@.lookup(s@)) && i@ < a.slabs@.lookup(s@).keys@.len() };
        proof_assert! { b.slabs@.lookup(s@).keys@[i@] == FREE_KEY };
        proof_assert! { forall<x: Int> 0 <= x && x < a.slabs@.lookup(s@).bitmap.num_slots@ && x != i@ ==> b.slabs@.lookup(s@).keys@[x] == a.slabs@.lookup(s@).keys@[x] }
    }
}

/// One call with its exact effect on the published slots.
#[requires(lg(*r, hs@) && lop_pre(*r, hs@, op))]
#[ensures(lg(^r, (^hs)@))]
#[ensures(getx_step(*r, ^r, hs@, op))]
pub fn apply_lopx(r: &mut RegionState, hs: &mut Vec<(u64, usize)>, op: LOp) {
    let a = snapshot! { *r };
    let h0 = snapshot! { hs@ };
    proof_assert! { forall<t: Int, x: Int> pub_slot(*a, t, x) ==> slot_live(*a, t, x) };
    match op {
        LOp::Reserve(sz) => {
            let _ = reserve_h(r, hs, sz);
            proof_assert! { forall<t: Int, x: Int> pub_slot(*r, t, x) ==> pub_slot(*a, t, x) && r.slabs@.lookup(t).keys@[x] == a.slabs@.lookup(t).keys@[x] };
            proof_assert! { forall<t: Int, x: Int> pub_slot(*a, t, x) ==> keep_slot(*a, *r, t, x) && pub_slot(*r, t, x) };
            proof_assert! { getx_step(*a, *r, *h0, op) };
        }
        LOp::Publish(j, key) => {
            publish_h(r, hs, j, key);
            proof_assert! { forall<t: Int, x: Int> pub_slot(*r, t, x) ==> slot_live(*r, t, x) };
            proof_assert! { a.slabs@.lookup(h0[j@].0@).keys@[h0[j@].1@] == FREE_KEY && !pub_slot(*a, h0[j@].0@, h0[j@].1@) };
            proof_assert! { forall<t: Int, x: Int> pub_slot(*a, t, x) ==> (t != h0[j@].0@ || x != h0[j@].1@) && keep_slot(*a, *r, t, x) && pub_slot(*r, t, x) };
            proof_assert! { key == FREE_KEY ==> forall<t: Int, x: Int> pub_slot(*r, t, x) ==> keep_slot(*r, *a, t, x) && pub_slot(*a, t, x) };
            proof_assert! { key != FREE_KEY ==> forall<t: Int, x: Int> pub_slot(*r, t, x) && (t != h0[j@].0@ || x != h0[j@].1@) ==>
                slot_live(*a, t, x) && pub_slot(*a, t, x) };
            proof_assert! { getx_step(*a, *r, *h0, op) };
        }
        LOp::Abort(j) => {
            abort_h(r, hs, j);
            proof_assert! { forall<t: Int, x: Int> pub_slot(*r, t, x) ==> slot_live(*r, t, x) };
            proof_assert! { a.slabs@.lookup(h0[j@].0@).keys@[h0[j@].1@] == FREE_KEY };
            proof_assert! { forall<t: Int, x: Int> pub_slot(*a, t, x) ==> (t != h0[j@].0@ || x != h0[j@].1@) && keep_slot(*a, *r, t, x) && pub_slot(*r, t, x) };
            proof_assert! { forall<t: Int, x: Int> pub_slot(*r, t, x) ==> keep_slot(*r, *a, t, x) && pub_slot(*a, t, x) };
            proof_assert! { getx_step(*a, *r, *h0, op) };
        }
        LOp::Remove(off) => {
            let res = r.remove_extent_by_offset(off);
            proof_assert! { res != Ok(()) ==> *r == *a };
            proof_assert! { res == Ok(()) ==> forall<s: u64, i: usize> rm_case(*a, off, s, i) && rm_post(*a, *r, s, i) ==> {
                lemma_remove_one(*a, *r, off, s, i, *h0);
                lemma_remove_getx(*a, *r, off, s, i, *h0);
                getx_step(*a, *r, *h0, op) } };
        }
        LOp::Flush => {
            r.flush_pending_frees();
            proof_assert! { lemma_fl_spec_elim(*a, *r); lemma_flush_good(*a, *r, *h0); true };
            proof_assert! { forall<t: Int, x: Int> pub_slot(*a, t, x) ==> slot_live(*a, t, x) && !inpend(a.pending_frees@, t, x) };
            proof_assert! { forall<t: Int, x: Int> pub_slot(*a, t, x) ==> keep_slot(*a, *r, t, x) && pub_slot(*r, t, x) };
            proof_assert! { forall<t: Int, x: Int> pub_slot(*r, t, x) ==> slot_live(*r, t, x) && keep_slot(*r, *a, t, x) && pub_slot(*a, t, x) };
            proof_assert! { getx_step(*a, *r, *h0, op) };
        }
    }
}

/// **EM-GETEXT-POST-EXACT** — "get_extents() returns exactly the extents that have been published
/// (or recovered) with a key other than u64::MAX and not since removed, each exactly once and
/// with the key, offset and size that publish returned; it includes no reservation that has not
/// been published, and no removed extent even if its disk slot has not yet been freed by a
/// checkpoint." By induction over a region history: format() starts with no published slot,
/// initialize() with exactly the recovered non-FREE slots (rebuilt_all); each call changes the
/// published slots exactly as the abstract operation says (publish adds the handle's slot with
/// its key at its offset and size; remove takes out the one at that offset — even before the
/// checkpoint frees it; reserve / abort / publish(FREE_KEY) / checkpoint change nothing); and
/// the listing of a region (region_extents, lib.rs:641-648) is exactly its published slots
/// (sound + complete), whose offsets are pairwise distinct (no_overlap), so none is listed for
/// two extents.
#[requires(lg(*r, hs@) && lop_pre(*r, hs@, op))]
#[ensures(getx_step(*r, ^r, hs@, op))]
#[ensures(all_from(result@, 0, ^r) && all_listed(result@, 0, ^r))]
#[ensures(forall<t1: Int, x1: Int, t2: Int, x2: Int> pub_slot(^r, t1, x1) && pub_slot(^r, t2, x2) && (t1 != t2 || x1 != x2) ==>
    slot_off((^r).slabs@.lookup(t1), x1) != slot_off((^r).slabs@.lookup(t2), x2))]
pub fn verify_em_getext_post_exact(r: &mut RegionState, hs: &mut Vec<(u64, usize)>, op: LOp) -> Vec<Extent> {
    apply_lopx(r, hs, op);
    proof_assert! { lemma_no_overlap(*r); no_overlap(*r) };
    proof_assert! { forall<t: Int, x: Int> pub_slot(*r, t, x) ==> slot_live(*r, t, x) && slab_ok(*r, r.slabs@.lookup(t)) && r.slabs@.lookup(t).element_size@ > 0 };
    let mut out: Vec<Extent> = Vec::new();
    region_extents(r, &mut out);
    out
}

/// Mutant: claims a removal leaves the removed extent published.
/// LEVEL-3 (phase D-B2): mutant with EXACTLY the property's requires and body; it claims the
/// negation of the property's first postcondition.
#[requires(lg(*r, hs@) && lop_pre(*r, hs@, op))]
#[ensures(!(getx_step(*r, ^r, hs@, op)))]
pub fn verify_em_getext_post_exact__mutant(r: &mut RegionState, hs: &mut Vec<(u64, usize)>, op: LOp) -> Vec<Extent> {
    apply_lopx(r, hs, op);
    proof_assert! { lemma_no_overlap(*r); no_overlap(*r) };
    proof_assert! { forall<t: Int, x: Int> pub_slot(*r, t, x) ==> slot_live(*r, t, x) && slab_ok(*r, r.slabs@.lookup(t)) && r.slabs@.lookup(t).element_size@ > 0 };
    let mut out: Vec<Extent> = Vec::new();
    region_extents(r, &mut out);
    out
}

// ================================================================ key sharding (lib.rs:211-221)

#[bitwise_proof]
#[requires(m < 64usize && s == 1usize << m)]
#[ensures(result@ == x@ / s@)]
#[ensures(result == x >> m)]
pub fn bf_div_us(x: usize, s: usize, m: usize) -> usize {
    x / s
}

#[bitwise_proof]
#[requires(m < 64usize && s == 1usize << m && q@ * s@ <= usize::MAX@)]
#[ensures(result@ == q@ * s@)]
#[ensures(result == q << m)]
pub fn bf_mul_us(q: usize, s: usize, m: usize) -> usize {
    q * s
}

#[bitwise_proof]
#[requires(m < 64usize)]
#[ensures((x & ((1usize << m) - 1usize)) == x - ((x >> m) << m))]
pub fn bf_low_us(x: usize, m: usize) {}

#[bitwise_proof]
#[requires(m@ < 64 && s@ == m@.pow2())]
#[ensures(s == 1usize << m)]
pub fn bf_shl_us(s: usize, m: usize) {}

/// PROOF-ONLY: for a power of two `n`, `x & (n - 1)` (lib.rs:219) is `x mod n`.
#[requires(n@ > 0)]
#[ensures(p2(n@) ==> (x & (n - 1usize))@ == x@ % n@)]
pub fn and_mod_fact(x: usize, n: usize) {
    let mut k: usize = 0;
    #[invariant(k@ <= 63)]
    #[invariant(forall<e: Int> 0 <= e && e.pow2() == n@ ==> k@ <= e)]
    while k < 63 && pow2_u64(k) as usize != n {
        proof_assert! { forall<e: Int> 0 <= e && e.pow2() == n@ ==> k@ != e };
        k += 1;
    }
    proof_assert! { forall<e: Int> 0 <= e && e.pow2() == n@ && k@ < e ==> { lemma_pow2_strict(k@, e); k@.pow2() < e.pow2() } };
    proof_assert! { 64.pow2() == 18446744073709551616 };
    proof_assert! { forall<e: Int> 0 <= e && e.pow2() == n@ ==> (e >= 64 ==> { lemma_pow2_mono(64, e); false }) };
    let pk = pow2_u64(k) as usize;
    if pk == n {
        bf_shl_us(n, k);
        let q = bf_div_us(x, n, k);
        proof_assert! { lemma_divmod(x@, n@); q@ * n@ <= x@ };
        let y = bf_mul_us(q, n, k);
        bf_low_us(x, k);
        proof_assert! { (x & (n - 1usize)) == x - y };
        proof_assert! { lemma_divmod(x@, n@); (x & (n - 1usize))@ == x@ % n@ };
    } else {
        proof_assert! { forall<e: Int> 0 <= e && e.pow2() == n@ ==> e <= 63 && k@ == e && false };
    }
}

/// **EM-INV-KEY-SHARDING** — "An extent reserved for key k lies inside the contiguous
/// data-device byte range of region number (k AND (region_count minus 1)), which, because
/// region_count is a power of two, is k modulo the region count." reserve_extent's routing
/// (lib.rs:589 region_for_key, lib.rs:219 `key as usize & (regions.len() - 1)` = `and_mask`)
/// then the region's alloc_extent, over the current regions `rs` (each a reachable region,
/// `lg`): the index is `k mod region_count` and the reserved extent `[off, off + align(size))`
/// lies inside that region's byte range `[base, base + total_usable_size)`.
#[requires(rs@.len() > 0 && p2(rs@.len()) && rs@.len() <= usize::MAX@)]
#[requires(forall<i: Int> 0 <= i && i < rs@.len() ==> lg(rs@[i], hs@))]
#[requires(forall<i: Int> 0 <= i && i < rs@.len() ==> sz@ > 0 && a_3783f8(sz@, rs@[i].format_params.sector_size@))]
#[ensures(result.0@ == key@ % rs@.len() && result.0@ < rs@.len())]
#[ensures(match result.1 {
    Ok((_, _, off)) => rs@[result.0@].buddy.base_offset@ <= off@
        && off@ + align_l(sz@, rs@[result.0@].format_params.sector_size@) <= rs@[result.0@].buddy.base_offset@ + rs@[result.0@].buddy.total_usable_size@,
    Err(_) => true,
})]
pub fn verify_em_inv_key_sharding(rs: &mut Vec<RegionState>, hs: &mut Vec<(u64, usize)>, key: u64, sz: u32)
    -> (usize, Result<(u64, usize, u64), EmError>) {
    let n = rs.len();
    let idx = and_mask(key as usize, n - 1);              // region_for_key (lib.rs:219)
    and_mod_fact(key as usize, n);
    proof_assert! { (key as usize)@ == key@ };
    let r = &mut rs[idx];
    let a = snapshot! { *r };
    proof_assert! { rg_within(*a, a.buddy.base_offset@, a.buddy.base_offset@ + a.buddy.total_usable_size@) };
    let res = reserve_h(r, hs, sz);                         // alloc_extent (lib.rs:589)
    proof_assert! { forall<t: Int, k: Int> slot_live(*r, t, k) ==> {
        lemma_slot_in_block(*r, t, k);
        slab_ok(*r, r.slabs@.lookup(t)) && r.slabs@.lookup(t).start_offset@ == t } };
    (idx, res)
}

/// Mutant: claims the routed index differs from key mod region_count.
/// LEVEL-3 (phase D-B2): mutant with EXACTLY the property's requires and body; it claims the
/// negation of the property's first postcondition.
#[requires(rs@.len() > 0 && p2(rs@.len()) && rs@.len() <= usize::MAX@)]
#[requires(forall<i: Int> 0 <= i && i < rs@.len() ==> lg(rs@[i], hs@))]
#[requires(forall<i: Int> 0 <= i && i < rs@.len() ==> sz@ > 0 && a_3783f8(sz@, rs@[i].format_params.sector_size@))]
#[ensures(!(result.0@ == key@ % rs@.len() && result.0@ < rs@.len()))]
pub fn verify_em_inv_key_sharding__mutant(rs: &mut Vec<RegionState>, hs: &mut Vec<(u64, usize)>, key: u64, sz: u32)
    -> (usize, Result<(u64, usize, u64), EmError>) {
    let n = rs.len();
    let idx = and_mask(key as usize, n - 1);              // region_for_key (lib.rs:219)
    and_mod_fact(key as usize, n);
    proof_assert! { (key as usize)@ == key@ };
    let r = &mut rs[idx];
    let a = snapshot! { *r };
    proof_assert! { rg_within(*a, a.buddy.base_offset@, a.buddy.base_offset@ + a.buddy.total_usable_size@) };
    let res = reserve_h(r, hs, sz);                         // alloc_extent (lib.rs:589)
    proof_assert! { forall<t: Int, k: Int> slot_live(*r, t, k) ==> {
        lemma_slot_in_block(*r, t, k);
        slab_ok(*r, r.slabs@.lookup(t)) && r.slabs@.lookup(t).start_offset@ == t } };
    (idx, res)
}

// ============================================== schedules of calls (EM-INV-CONCURRENT-LINEARIZABLE)

/// LEVEL-3 (phase D-B2): only the handle half of the documented use — a publish / abort names a
/// handle the caller holds (an index into the live-handle list; an out-of-range index is no
/// handle, so no such call exists). Reserve sizes are NOT filtered any more (the driver requires
/// the declared reserve ranges of every Reserve in the schedule).
#[ensures(result == match op { LOp::Publish(j, _) => j@ < hs@.len(), LOp::Abort(j) => j@ < hs@.len(), _ => true })]
pub fn lop_live(hs: &Vec<(u64, usize)>, op: LOp) -> bool {
    match op {
        LOp::Publish(j, _) => j < hs.len(),
        LOp::Abort(j) => j < hs.len(),
        _ => true,
    }
}

/// Every Reserve of the schedule is within the declared reserve ranges (231ab0, 3783f8).
#[logic(open)]
pub fn sched_rsv_ok(ops: Seq<LOp>, ss: Int) -> bool {
    pearlite! { forall<p: Int> 0 <= p && p < ops.len() ==> match ops[p] { LOp::Reserve(sz) => a_231ab0(sz@) && a_3783f8(sz@, ss), _ => true } }
}

/// The documented-use precondition checked at run time (a caller can only use a live handle;
/// a size whose sector alignment fits u32).
#[ensures(result ==> lop_pre(*r, hs@, op))]
pub fn lop_ok(r: &RegionState, hs: &Vec<(u64, usize)>, op: LOp) -> bool {
    match op {
        LOp::Reserve(sz) => sz > 0 && (sz as u64) + (r.format_params.sector_size as u64) <= u32::MAX as u64,
        LOp::Publish(j, _) => j < hs.len(),
        LOp::Abort(j) => j < hs.len(),
        LOp::Remove(_) => true,
        LOp::Flush => true,
    }
}

/// **EM-INV-CONCURRENT-LINEARIZABLE** — "When reserve, publish, abort and remove run concurrently
/// from many threads, every operation completes without panicking and the resulting extents and
/// used bytes are the same as for some one-at-a-time ordering of those calls; in particular, after
/// concurrent publishes with distinct keys get_extents() lists exactly the total number published,
/// after a concurrent mix of publish and abort exactly the published extents are listed, and
/// concurrent removals of non-overlapping sets of existing extents all succeed and none of the
/// removed extents is listed." Each call is one critical section of its region's RwLock
/// (lib.rs:589/610/620/680), so a concurrent run is the one-at-a-time schedule of its lock
/// acquisitions. For EVERY schedule `ops` of calls on a reachable region: every call's mirror
/// precondition holds (no panic) and the invariant is kept; every call changes the published
/// extents exactly as its one-at-a-time effect (getx_step: a publish adds exactly its handle's
/// slot, an abort / publish(FREE_KEY) none, a remove exactly the one at its offset); and a removal
/// of a published extent's offset succeeds and unpublishes it.
// LEVEL-3 (phase D-B2): the run-time `lop_ok` filter is gone for reserves: EVERY Reserve of the
// schedule runs, under the declared reserve ranges (sched_rsv_ok: 231ab0, 3783f8); a publish /
// abort runs whenever its index names a live handle (lop_live — an out-of-range index is no handle).
#[requires(lg(*r, hs@) && sched_rsv_ok(ops@, r.format_params.sector_size@))]
#[ensures(lg(^r, (^hs)@))]
#[ensures(pub_slot(*result.0, t@, x@) && slot_off(result.0.slabs@.lookup(t@), x@) == off@ ==>
    result.1 == Ok(()) && !pub_slot(^r, t@, x@))]
pub fn verify_em_inv_concurrent_linearizable(r: &mut RegionState, hs: &mut Vec<(u64, usize)>, ops: &Vec<LOp>, off: u64, t: u64, x: usize)
    -> (Snapshot<RegionState>, Result<(), EmError>) {
    let r_in = snapshot! { *r };
    let mut i: usize = 0;
    #[invariant(lg(*r, hs@))]
    #[invariant(r.format_params == r_in.format_params)]
    while i < ops.len() {
        let op = ops[i];
        if lop_live(hs, op) {
            proof_assert! { match op { LOp::Reserve(sz) => a_231ab0(sz@) && a_3783f8(sz@, r.format_params.sector_size@), _ => true } };
            proof_assert! { lop_pre(*r, hs@, op) };
            apply_lop(r, hs, op);
        }
        i += 1;
    }
    let b = snapshot! { *r };
    proof_assert! { pub_slot(*b, t@, x@) && slot_off(b.slabs@.lookup(t@), x@) == off@ ==> { lemma_pub_rm_case(*b, off, t, x); rm_case(*b, off, t, x) } };
    let res = r.remove_extent_by_offset(off);
    proof_assert! { res == Ok(()) ==> forall<s: u64, k: usize> rm_case(*b, off, s, k) && rm_post(*b, *r, s, k) ==> {
        lemma_remove_one(*b, *r, off, s, k, hs@);
        lemma_remove_getx(*b, *r, off, s, k, hs@);
        lg(*r, hs@) } };
    proof_assert! { res != Ok(()) ==> *r == *b };
    (b, res)
}

/// Mutant: claims the removal of a published extent's offset fails.
/// (same requires as the property)
// LEVEL-3 (phase D-B2): the run-time `lop_ok` filter is gone for reserves: EVERY Reserve of the
// schedule runs, under the declared reserve ranges (sched_rsv_ok: 231ab0, 3783f8); a publish /
// abort runs whenever its index names a live handle (lop_live — an out-of-range index is no handle).
#[requires(lg(*r, hs@) && sched_rsv_ok(ops@, r.format_params.sector_size@))]
#[ensures(pub_slot(*result.0, t@, x@) && slot_off(result.0.slabs@.lookup(t@), x@) == off@ ==> result.1 != Ok(()))]
pub fn verify_em_inv_concurrent_linearizable__mutant(r: &mut RegionState, hs: &mut Vec<(u64, usize)>, ops: &Vec<LOp>, off: u64, t: u64, x: usize)
    -> (Snapshot<RegionState>, Result<(), EmError>) {
    let r_in = snapshot! { *r };
    let mut i: usize = 0;
    #[invariant(lg(*r, hs@))]
    #[invariant(r.format_params == r_in.format_params)]
    while i < ops.len() {
        let op = ops[i];
        if lop_live(hs, op) {
            proof_assert! { match op { LOp::Reserve(sz) => a_231ab0(sz@) && a_3783f8(sz@, r.format_params.sector_size@), _ => true } };
            proof_assert! { lop_pre(*r, hs@, op) };
            apply_lop(r, hs, op);
        }
        i += 1;
    }
    let b = snapshot! { *r };
    proof_assert! { pub_slot(*b, t@, x@) && slot_off(b.slabs@.lookup(t@), x@) == off@ ==> { lemma_pub_rm_case(*b, off, t, x); rm_case(*b, off, t, x) } };
    let res = r.remove_extent_by_offset(off);
    proof_assert! { res == Ok(()) ==> forall<s: u64, k: usize> rm_case(*b, off, s, k) && rm_post(*b, *r, s, k) ==> {
        lemma_remove_one(*b, *r, off, s, k, hs@);
        lemma_remove_getx(*b, *r, off, s, k, hs@);
        lg(*r, hs@) } };
    proof_assert! { res != Ok(()) ==> *r == *b };
    (b, res)
}
