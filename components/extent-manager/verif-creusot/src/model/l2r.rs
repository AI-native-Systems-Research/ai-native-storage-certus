//! Level-2 (batch A) region theory: the maintained region invariant `rg_good` under a
//! power-of-two sector size (D-RANGE-FR-002-217ace) and the exact-effect predicates the
//! region mutators expose for it.
use crate::model::bitmap::*;
use crate::model::buddy::*;
use crate::model::l2::*;
use crate::model::params::*;
use crate::model::region::*;
use crate::model::slab::*;
use creusot_std::logic::FMap;
use crate::model::assume::*;
use creusot_std::prelude::*;

/// Bytes of one slab's buddy block: `span(ss, K)`.
#[logic(open)]
pub fn sk(r: RegionState) -> Int {
    pearlite! { span(r.buddy.sector_size@, slab_k(r)) }
}

/// `alloc_slot`'s exact effect on a slab: slot `i` becomes allocated, nothing else changes.
#[logic(open)]
pub fn slot_step(a: Slab, b: Slab, i: Int) -> bool {
    pearlite! {
        0 <= i && i < a.bitmap.num_slots@ && !slot_bit(a.bitmap, i) && slot_bit(b.bitmap, i)
        && (forall<j: Int> 0 <= j && j < a.bitmap.num_slots@ && j != i ==> slot_bit(b.bitmap, j) == slot_bit(a.bitmap, j))
        && b.keys == a.keys && b.start_offset == a.start_offset && b.slab_size == a.slab_size
        && b.element_size == a.element_size && b.bitmap.num_slots == a.bitmap.num_slots
        && b.bitmap.allocated_count@ == a.bitmap.allocated_count@ + 1
    }
}

/// Every size-class entry of `b` was already an entry of `a` (same class), or is `d` in class `es`.
/// (Body private to this module; use [`lemma_sc_sub_at`] to read it.)
#[logic]
pub fn sc_sub(a: SizeClassManager, b: SizeClassManager, es: Int, d: Int) -> bool {
    pearlite! {
        forall<e: Int, p: Int> 0 <= p && p < sc_get(b, e).len() ==>
            listed(sc_get(a, e), sc_get(b, e)[p]@) || (e == es && sc_get(b, e)[p]@ == d)
    }
}

/// alloc_extent's exact effect on the slab map, the size classes and the buddy allocator
/// (region.rs:40-91), whatever the size-class lists contain.
#[logic]
pub fn alloc_any(old: RegionState, new: RegionState, es: Int, res: Result<(u64, usize, u64), EmError>) -> bool {
    pearlite! {
        match res {
            Ok((s, i, _)) =>
                (sc_sub(old.size_classes, new.size_classes, es, -1) && new.buddy == old.buddy
                    && listed(sc_get(old.size_classes, es), s@) && old.slabs@.contains(s@)
                    && new.slabs@ == old.slabs@.insert(s@, new.slabs@.lookup(s@))
                    && slot_step(old.slabs@.lookup(s@), new.slabs@.lookup(s@), i@))
                || (sc_sub(old.size_classes, new.size_classes, es, s@)
                    && new.slabs@ == old.slabs@.insert(s@, new.slabs@.lookup(s@))
                    && fresh_one(new.slabs@.lookup(s@), s@, old.format_params.slab_size@, es)
                    && (exists<fo: Int> alloc_post(old.buddy, new.buddy, slab_k(old), fo, s@))
                    && rg_tf(new) == rg_tf(old) - sk(old)),
            Err(_) => new.slabs@ == old.slabs@ && sc_sub(old.size_classes, new.size_classes, es, -1)
                && (new.buddy == old.buddy
                    || exists<m: BuddyAllocator, fo: Int, d: Int> alloc_post(old.buddy, m, slab_k(old), fo, d)
                        && m.sector_size == old.buddy.sector_size && m.free_lists@.len() == old.buddy.free_lists@.len()
                        && free_e(m.free_lists@, new.buddy.free_lists@, m.sector_size@, d - old.buddy.base_offset@, sk(old))),
        }
    }
}

#[logic]
#[ensures(sc_sub(a, a, es, d))]
pub fn lemma_sc_sub_refl(a: SizeClassManager, es: Int, d: Int) {
    pearlite! {
        proof_assert! { forall<e: Int, p: Int> 0 <= p && p < sc_get(a, e).len() ==> listed(sc_get(a, e), sc_get(a, e)[p]@) }
    }
}

/// Every entry of `filter_ne(sq, x)` is an entry of `sq`.
#[logic]
#[variant(sq.len())]
#[ensures(forall<p: Int> 0 <= p && p < filter_ne(sq, x).len() ==> listed(sq, filter_ne(sq, x)[p]@))]
pub fn lemma_filter_sub(sq: Seq<u64>, x: u64) {
    pearlite! {
        if sq.len() == 0 { () } else {
            lemma_filter_sub(sq.subsequence(0, sq.len() - 1), x);
            proof_assert! { forall<p: Int> 0 <= p && p < sq.len() - 1 ==> sq.subsequence(0, sq.len() - 1)[p] == sq[p] };
            proof_assert! { forall<v: Int> listed(sq.subsequence(0, sq.len() - 1), v) ==> listed(sq, v) };
            ()
        }
    }
}

/// A size-class removal (`filter_ne` on class `es`) keeps `sc_sub` from `old`.
#[logic]
#[requires(sc_sub(old, it, es, -1))]
#[requires(forall<e: Int> sc_get(nw, e) == if e == es { filter_ne(sc_get(it, e), s) } else { sc_get(it, e) })]
#[ensures(sc_sub(old, nw, es, -1))]
pub fn lemma_sc_sub_remove(old: SizeClassManager, it: SizeClassManager, nw: SizeClassManager, es: Int, s: u64) {
    pearlite! {
        lemma_filter_sub(sc_get(it, es), s);
        proof_assert! { forall<e: Int, v: Int> listed(sc_get(it, e), v) && v >= 0 ==> listed(sc_get(old, e), v) };
        proof_assert! { forall<e: Int, p: Int> 0 <= p && p < sc_get(nw, e).len() ==> listed(sc_get(it, e), sc_get(nw, e)[p]@) }
    }
}

/// A push of `d` onto class `es` (or nothing) keeps `sc_sub` from `old`, with `d` new.
#[logic]
#[requires(sc_sub(old, it, es, -1))]
#[requires(forall<e: Int> sc_get(nw, e) == if e == es && c { sc_get(it, e).push_back(d) } else { sc_get(it, e) })]
#[ensures(sc_sub(old, nw, es, d@))]
pub fn lemma_sc_sub_push(old: SizeClassManager, it: SizeClassManager, nw: SizeClassManager, es: Int, d: u64, c: bool) {
    pearlite! {
        proof_assert! { forall<e: Int, v: Int> listed(sc_get(it, e), v) && v >= 0 ==> listed(sc_get(old, e), v) }
    }
}

/// Reading `sc_sub` at one entry.
#[logic]
#[requires(sc_sub(a, b, es, d) && 0 <= p && p < sc_get(b, e).len())]
#[ensures(listed(sc_get(a, e), sc_get(b, e)[p]@) || (e == es && sc_get(b, e)[p]@ == d))]
pub fn lemma_sc_sub_at(a: SizeClassManager, b: SizeClassManager, es: Int, d: Int, e: Int, p: Int) {}

/// `sc_sub` composes.
#[logic]
#[requires(sc_sub(a, b, es, -1) && sc_sub(b, c, es, d))]
#[ensures(sc_sub(a, c, es, d))]
pub fn lemma_sc_sub_trans(a: SizeClassManager, b: SizeClassManager, c: SizeClassManager, es: Int, d: Int) {
    pearlite! {
        proof_assert! { forall<e: Int, v: Int> listed(sc_get(b, e), v) && v >= 0 ==> listed(sc_get(a, e), v) }
    }
}

/// `sc_sub` from equality of the managers.
#[logic]
#[requires(sc_sub(a, b, es, d) && b == c)]
#[ensures(sc_sub(a, c, es, d))]
pub fn lemma_sc_sub_eq(a: SizeClassManager, b: SizeClassManager, c: SizeClassManager, es: Int, d: Int) {}

// ------------------------------------------- PROOF-ONLY ghost helpers for alloc_extent
// (ghost arguments only, empty bodies apart from proof assertions: they let the big
// alloc_extent VC see only opaque atoms).

#[ensures(sc_sub(*a, *a, *es, -1))]
pub fn g_sc_refl(a: Snapshot<SizeClassManager>, es: Snapshot<Int>) {
    proof_assert! { lemma_sc_sub_refl(*a, *es, -1); true };
}

#[requires(sc_sub(*old, *it, *es, -1) && sc_get(*it, *es).len() > 0 && sc_get(*it, *es)[0] == s)]
#[ensures(listed(sc_get(*old, *es), s@))]
pub fn g_sc_first(old: Snapshot<SizeClassManager>, it: Snapshot<SizeClassManager>, es: Snapshot<Int>, s: u64) {
    proof_assert! { lemma_sc_sub_at(*old, *it, *es, -1, *es, 0); true };
}

#[requires(sc_sub(*old, *it, *es, -1))]
#[requires(forall<e: Int> sc_get(*nw, e) == if e == *es { filter_ne(sc_get(*it, e), s) } else { sc_get(*it, e) })]
#[ensures(sc_sub(*old, *nw, *es, -1))]
pub fn g_sc_remove(old: Snapshot<SizeClassManager>, it: Snapshot<SizeClassManager>, nw: Snapshot<SizeClassManager>, es: Snapshot<Int>, s: u64) {
    proof_assert! { lemma_sc_sub_remove(*old, *it, *nw, *es, s); true };
}

#[requires(sc_sub(*old, *it, *es, -1))]
#[requires(forall<e: Int> sc_get(*nw, e) == if e == *es && *c { sc_get(*it, e).push_back(d) } else { sc_get(*it, e) })]
#[ensures(sc_sub(*old, *nw, *es, d@))]
pub fn g_sc_push(old: Snapshot<SizeClassManager>, it: Snapshot<SizeClassManager>, nw: Snapshot<SizeClassManager>, es: Snapshot<Int>, d: u64, c: Snapshot<bool>) {
    proof_assert! { lemma_sc_sub_push(*old, *it, *nw, *es, d, *c); true };
}

/// alloc_extent's Ok return from an existing slab (region.rs:52-58).
#[requires(sc_sub(old.size_classes, nw.size_classes, *es, -1) && nw.buddy == old.buddy)]
#[requires(listed(sc_get(old.size_classes, *es), s@) && old.slabs@.contains(s@))]
#[requires(nw.slabs@ == old.slabs@.insert(s@, nw.slabs@.lookup(s@)))]
#[requires(slot_step(old.slabs@.lookup(s@), nw.slabs@.lookup(s@), i@))]
#[ensures(alloc_any(*old, *nw, *es, Ok((s, i, off))))]
pub fn g_alloc_exist(old: Snapshot<RegionState>, nw: Snapshot<RegionState>, es: Snapshot<Int>, s: u64, i: usize, off: u64) {}

/// alloc_extent's Ok return from a freshly carved slab (region.rs:72-91).
#[requires(sc_sub(old.size_classes, nw.size_classes, *es, d@))]
#[requires(nw.slabs@ == old.slabs@.insert(d@, nw.slabs@.lookup(d@)))]
#[requires(fresh_one(nw.slabs@.lookup(d@), d@, old.format_params.slab_size@, *es))]
#[requires(alloc_post(old.buddy, nw.buddy, slab_k(*old), *fo, d@))]
#[requires(rg_tf(*nw) == rg_tf(*old) - sk(*old))]
#[ensures(alloc_any(*old, *nw, *es, Ok((d, i, off))))]
pub fn g_alloc_fresh(old: Snapshot<RegionState>, nw: Snapshot<RegionState>, es: Snapshot<Int>, fo: Snapshot<Int>, d: u64, i: usize, off: u64) {}

/// alloc_extent's Err return before any buddy operation.
#[requires(nw.slabs@ == old.slabs@ && sc_sub(old.size_classes, nw.size_classes, *es, -1) && nw.buddy == old.buddy)]
#[ensures(alloc_any(*old, *nw, *es, Err(e)))]
pub fn g_alloc_err(old: Snapshot<RegionState>, nw: Snapshot<RegionState>, es: Snapshot<Int>, e: EmError) {}

/// alloc_extent's Err return after the zero-slot rollback (region.rs:79-83).
#[requires(nw.slabs@ == old.slabs@ && sc_sub(old.size_classes, nw.size_classes, *es, -1))]
#[requires(alloc_post(old.buddy, *m, slab_k(*old), *fo, *d))]
#[requires(m.sector_size == old.buddy.sector_size && m.free_lists@.len() == old.buddy.free_lists@.len())]
#[requires(free_e(m.free_lists@, nw.buddy.free_lists@, m.sector_size@, *d - old.buddy.base_offset@, sk(*old)))]
#[ensures(alloc_any(*old, *nw, *es, Err(e)))]
pub fn g_alloc_rollback(old: Snapshot<RegionState>, nw: Snapshot<RegionState>, m: Snapshot<BuddyAllocator>, es: Snapshot<Int>, fo: Snapshot<Int>, d: Snapshot<Int>, e: EmError) {}

// ================================================================ the region invariant

/// Slot `j` of slab `t` exists and is allocated.
#[logic(open)]
pub fn slot_live(r: RegionState, t: Int, j: Int) -> bool {
    pearlite! {
        r.slabs@.contains(t) && 0 <= j && j < r.slabs@.lookup(t).bitmap.num_slots@
        && slot_bit(r.slabs@.lookup(t).bitmap, j)
    }
}

/// `(t, j)` occurs in the slot list `ps` at or after position `lo`.
#[logic(open)]
pub fn inpf(ps: Seq<(u64, usize)>, lo: Int, t: Int, j: Int) -> bool {
    pearlite! { exists<p: Int> lo <= p && p < ps.len() && ps[p].0@ == t && ps[p].1@ == j }
}

#[logic(open)]
pub fn inpend(ps: Seq<(u64, usize)>, t: Int, j: Int) -> bool {
    pearlite! { inpf(ps, 0, t, j) }
}

/// Every slot of `ps` from position `lo` on is live with the FREE_KEY marker, and no slot
/// occurs twice there (EM-REGION-PENDING-FREES-VALID for `ps = pending_frees`, `lo = 0`).
#[logic(open)]
pub fn pvf(r: RegionState, ps: Seq<(u64, usize)>, lo: Int) -> bool {
    pearlite! {
        (forall<p: Int> lo <= p && p < ps.len() ==>
            slot_live(r, ps[p].0@, ps[p].1@) && r.slabs@.lookup(ps[p].0@).keys@[ps[p].1@] == FREE_KEY)
        && (forall<p: Int, q: Int> lo <= p && p < q && q < ps.len() ==> ps[p] != ps[q])
    }
}

#[logic(open)]
pub fn pv(r: RegionState, ps: Seq<(u64, usize)>) -> bool {
    pearlite! { pvf(r, ps, 0) }
}

/// EM-INV-SLAB-UNIFORM-ELEMENT: every size-class entry is a slab of that element size.
#[logic(open)]
pub fn sc_ok(r: RegionState) -> bool {
    pearlite! {
        forall<e: Int, p: Int> 0 <= p && p < sc_get(r.size_classes, e).len() ==>
            r.slabs@.contains(sc_get(r.size_classes, e)[p]@)
            && r.slabs@.lookup(sc_get(r.size_classes, e)[p]@).element_size@ == e
    }
}

/// EM-INV-KEY-IMPLIES-ALLOCATED over every slab.
#[logic(open)]
pub fn rg_ka(r: RegionState) -> bool {
    pearlite! { forall<k: Int> r.slabs@.contains(k) ==> key_alloc(r.slabs@.lookup(k)) }
}

/// EM-REGION-SLABS-NOT-FREE: no slab's block overlaps a free block.
#[logic(open)]
pub fn sl_away(r: RegionState) -> bool {
    pearlite! {
        forall<k: Int> r.slabs@.contains(k) ==>
            faway(r.buddy.free_lists@, r.buddy.sector_size@, k - r.buddy.base_offset@, sk(r))
    }
}

/// Accounting: free bytes plus one block per slab is the whole-sector usable size.
#[logic(open)]
pub fn rg_acct(r: RegionState) -> bool {
    pearlite! {
        rg_tf(r) + r.slabs@.len() * sk(r) == (r.buddy.total_usable_size@ / r.buddy.sector_size@) * r.buddy.sector_size@
    }
}

/// The region invariant without the deferred-free part.
#[logic(open)]
pub fn rg_core(r: RegionState) -> bool {
    pearlite! {
        rg_inv(r) && p2(r.buddy.sector_size@) && fdisj(r.buddy.free_lists@, r.buddy.sector_size@)
        && sl_away(r) && rg_acct(r) && sc_ok(r) && rg_ka(r)
    }
}

/// The level-2 maintained region invariant (power-of-two sector size).
#[logic(open)]
pub fn rg_good(r: RegionState) -> bool {
    pearlite! { rg_core(r) && pv(r, r.pending_frees@) }
}

/// The slot `(t, j)` live in `a` is still live in `b` with the same key and slab geometry.
#[logic(open)]
pub fn keep_slot(a: RegionState, b: RegionState, t: Int, j: Int) -> bool {
    pearlite! {
        slot_live(b, t, j) && b.slabs@.lookup(t).keys@[j] == a.slabs@.lookup(t).keys@[j]
        && b.slabs@.lookup(t).start_offset == a.slabs@.lookup(t).start_offset
        && b.slabs@.lookup(t).element_size == a.slabs@.lookup(t).element_size
        && b.slabs@.lookup(t).slab_size == a.slabs@.lookup(t).slab_size
        && b.slabs@.lookup(t).bitmap.num_slots == a.slabs@.lookup(t).bitmap.num_slots
    }
}

/// Every live slot of `a` not queued in `a`'s deferred frees is kept in `b` (flush frame).
#[logic(open)]
pub fn flush_keep(a: RegionState, b: RegionState) -> bool {
    pearlite! { forall<t: Int, j: Int> slot_live(a, t, j) && !inpend(a.pending_frees@, t, j) ==> keep_slot(a, b, t, j) }
}

// ------------------------------------------------------------------ helper lemmas

/// Two distinct slabs of a region occupy disjoint order-K blocks.
#[logic]
#[requires(rg_inv(r) && r.slabs@.contains(k1) && r.slabs@.contains(k2) && k1 != k2)]
#[ensures(disj(k1 - r.buddy.base_offset@, sk(r), k2 - r.buddy.base_offset@, sk(r)))]
pub fn lemma_slab_blocks_disj(r: RegionState, k1: Int, k2: Int) {
    pearlite! {
        lemma_span_pos(r.buddy.sector_size@, slab_k(r));
        proof_assert! { slab_ok(r, r.slabs@.lookup(k1)) && slab_ok(r, r.slabs@.lookup(k2)) };
        if k1 < k2 {
            lemma_aligned_disj2(k1 - r.buddy.base_offset@, k2 - r.buddy.base_offset@, sk(r))
        } else {
            lemma_aligned_disj2(k2 - r.buddy.base_offset@, k1 - r.buddy.base_offset@, sk(r))
        }
    }
}

/// No entry of `filter_ne(sq, x)` is `x`.
#[logic]
#[variant(sq.len())]
#[ensures(forall<p: Int> 0 <= p && p < filter_ne(sq, x).len() ==> filter_ne(sq, x)[p] != x)]
pub fn lemma_filter_ne(sq: Seq<u64>, x: u64) {
    pearlite! {
        if sq.len() == 0 { () } else { lemma_filter_ne(sq.subsequence(0, sq.len() - 1), x) }
    }
}

/// The count of a slab with exactly one allocated slot `i` has no other allocated slot.
#[logic]
#[requires(bm_inv(b) && b.allocated_count@ == 1 && 0 <= i && i < b.num_slots@ && slot_bit(b, i))]
#[ensures(forall<j: Int> 0 <= j && j < b.num_slots@ && slot_bit(b, j) ==> j == i)]
pub fn lemma_only_one(b: AllocationBitmap, i: Int) {
    pearlite! {
        proof_assert! { forall<j: Int> 0 <= j && j < b.num_slots@ && slot_bit(b, j) && j != i ==> {
            lemma_cnt_two(b, i, j, b.num_slots@); false } }
    }
}

/// `free_slot(s, i)` keeps the region invariant and every other live slot (the core step
/// of abort / drop / publish(FREE_KEY) and of each deferred free of a checkpoint).
#[logic]
#[requires(rg_inv(a) && rg_inv(b) && rg_frame(a, b))]
#[requires(a.slabs@.get(s@) == Some(sl) && free_ok(a, s, i) && free_case(a, b, s, i, sl))]
#[requires(sl.bitmap.allocated_count@ == 1 ==>
    free_e(a.buddy.free_lists@, b.buddy.free_lists@, a.buddy.sector_size@, s@ - a.buddy.base_offset@, sk(a)))]
#[requires(rg_tf(b) == rg_tf(a) + if sl.bitmap.allocated_count@ == 1 { sk(a) } else { 0 })]
#[requires(rg_core(a) && pvf(a, ps, lo) && !inpf(ps, lo, s@, i@))]
#[ensures(rg_core(b) && pvf(b, ps, lo))]
pub fn lemma_free_core(a: RegionState, b: RegionState, s: u64, i: usize, sl: Slab, ps: Seq<(u64, usize)>, lo: Int) {
    pearlite! {
        let es = sl.element_size@;
        proof_assert! { slab_ok(a, sl) && sl.start_offset@ == s@ && slab_inv(sl) };
        proof_assert! { forall<o: Int> 0 <= o ==> { lemma_span_pos(a.buddy.sector_size@, o); span(a.buddy.sector_size@, o) > 0 } };
        proof_assert! { sk(b) == sk(a) && b.buddy.sector_size == a.buddy.sector_size && b.buddy.base_offset == a.buddy.base_offset };
        lemma_filter_sub(sc_get(a.size_classes, es), s);
        lemma_filter_ne(sc_get(a.size_classes, es), s);
        if sl.bitmap.allocated_count@ == 1 {
            lemma_only_one(sl.bitmap, i@);
            lemma_free_e_elim(a.buddy.free_lists@, b.buddy.free_lists@, a.buddy.sector_size@, s@ - a.buddy.base_offset@, sk(a));
            proof_assert! { b.slabs@ == a.slabs@.remove(s@) };
            proof_assert! { forall<k: Int> b.slabs@.contains(k) ==> a.slabs@.contains(k) && k != s@ && b.slabs@.lookup(k) == a.slabs@.lookup(k) };
            proof_assert! { fdisj(b.buddy.free_lists@, b.buddy.sector_size@) };
            proof_assert! { forall<k: Int> b.slabs@.contains(k) ==> {
                lemma_slab_blocks_disj(a, k, s@);
                faway(b.buddy.free_lists@, b.buddy.sector_size@, k - b.buddy.base_offset@, sk(b)) } };
            proof_assert! { sl_away(b) };
            proof_assert! { b.slabs@.len() == a.slabs@.len() - 1 };
            proof_assert! { rg_acct(b) };
            proof_assert! { forall<e: Int, p: Int> 0 <= p && p < sc_get(b.size_classes, e).len() ==>
                listed(sc_get(a.size_classes, e), sc_get(b.size_classes, e)[p]@) && sc_get(b.size_classes, e)[p]@ != s@ };
            proof_assert! { sc_ok(b) };
            proof_assert! { rg_ka(b) };
            proof_assert! { forall<p: Int> lo <= p && p < ps.len() ==> ps[p].0@ != s@ };
            ()
        } else {
            proof_assert! { b.buddy == a.buddy };
            proof_assert! { b.slabs@ == a.slabs@.insert(s@, b.slabs@.lookup(s@)) };
            proof_assert! { b.slabs@.contains(s@) && slab_ok(b, b.slabs@.lookup(s@)) && b.slabs@.lookup(s@).start_offset@ == s@ };
            proof_assert! { forall<k: Int> b.slabs@.contains(k) == a.slabs@.contains(k) };
            proof_assert! { b.slabs@.len() == a.slabs@.len() };
            proof_assert! { sl_away(b) && rg_acct(b) };
            proof_assert! { sc_ok(b) };
            proof_assert! { key_alloc(b.slabs@.lookup(s@)) };
            proof_assert! { rg_ka(b) };
            ()
        }
    }
}

/// PROOF-ONLY ghost helper wrapping [`lemma_free_core`].
#[requires(rg_inv(*a) && rg_inv(*b) && rg_frame(*a, *b))]
#[requires(a.slabs@.get(s@) == Some(*sl) && free_ok(*a, s, i) && free_case(*a, *b, s, i, *sl))]
#[requires(sl.bitmap.allocated_count@ == 1 ==>
    free_e(a.buddy.free_lists@, b.buddy.free_lists@, a.buddy.sector_size@, s@ - a.buddy.base_offset@, sk(*a)))]
#[requires(rg_tf(*b) == rg_tf(*a) + if sl.bitmap.allocated_count@ == 1 { sk(*a) } else { 0 })]
#[ensures(rg_core(*a) && pvf(*a, *ps, *lo) && !inpf(*ps, *lo, s@, i@) ==> rg_core(*b) && pvf(*b, *ps, *lo))]
#[ensures(forall<t: Int, j: Int> slot_live(*a, t, j) && (t != s@ || j != i@) ==> keep_slot(*a, *b, t, j))]
pub fn g_free_core(a: Snapshot<RegionState>, b: Snapshot<RegionState>, s: u64, i: usize, sl: Snapshot<Slab>, ps: Snapshot<Seq<(u64, usize)>>, lo: Snapshot<Int>) {
    proof_assert! { rg_core(*a) && pvf(*a, *ps, *lo) && !inpf(*ps, *lo, s@, i@) ==> { lemma_free_core(*a, *b, s, i, *sl, *ps, *lo); rg_core(*b) && pvf(*b, *ps, *lo) } };
    proof_assert! { lemma_free_core_keep(*a, *b, s, i, *sl); true };
}

/// The live-slot frame of `free_slot` alone (no invariant needed).
#[logic]
#[requires(rg_inv(a) && rg_inv(b) && rg_frame(a, b))]
#[requires(a.slabs@.get(s@) == Some(sl) && free_ok(a, s, i) && free_case(a, b, s, i, sl))]
#[ensures(forall<t: Int, j: Int> slot_live(a, t, j) && (t != s@ || j != i@) ==> keep_slot(a, b, t, j))]
pub fn lemma_free_core_keep(a: RegionState, b: RegionState, s: u64, i: usize, sl: Slab) {
    pearlite! {
        proof_assert! { slab_ok(a, sl) && sl.start_offset@ == s@ && slab_inv(sl) };
        if sl.bitmap.allocated_count@ == 1 {
            lemma_only_one(sl.bitmap, i@);
            proof_assert! { b.slabs@ == a.slabs@.remove(s@) };
            ()
        } else {
            proof_assert! { b.slabs@ == a.slabs@.insert(s@, b.slabs@.lookup(s@)) };
            proof_assert! { b.slabs@.contains(s@) && slab_ok(b, b.slabs@.lookup(s@)) && b.slabs@.lookup(s@).start_offset@ == s@ };
            proof_assert! { b.slabs@.lookup(s@).slab_size == sl.slab_size && b.slabs@.lookup(s@).start_offset == sl.start_offset
                && b.slabs@.lookup(s@).element_size == sl.element_size && b.slabs@.lookup(s@).bitmap.num_slots == sl.bitmap.num_slots };
            proof_assert! { forall<j: Int> 0 <= j && j < sl.bitmap.num_slots@ && j != i@ ==>
                b.slabs@.lookup(s@).keys@[j] == sl.keys@[j] && slot_bit(b.slabs@.lookup(s@).bitmap, j) == slot_bit(sl.bitmap, j) };
            proof_assert! { forall<t: Int> t != s@ ==> b.slabs@.get(t) == a.slabs@.get(t) };
            ()
        }
    }
}

// ----------------------------------------------------- flush_pending_frees (region.rs:151-160)

/// flush_pending_frees' level-2 contract: the invariant is kept and every live slot that was
/// not queued keeps its key and geometry. Opaque outside this module ([`lemma_fl_spec_elim`]).
#[logic]
pub fn fl_spec(a: RegionState, b: RegionState) -> bool {
    pearlite! { (rg_good(a) ==> rg_good(b)) && flush_keep(a, b) && flush_back(a, b) && (ne(a) ==> ne(b)) }
}

/// Every slab has an allocated slot (true of every history since format(); a recovered
/// checkpoint can hold an all-free slab — RECOVER-EMPTY-SLAB).
#[logic(open)]
pub fn ne(r: RegionState) -> bool {
    pearlite! { forall<k: Int> r.slabs@.contains(k) ==> r.slabs@.lookup(k).bitmap.allocated_count@ >= 1 }
}

/// free_slot keeps every slab non-empty (an emptied slab is removed, region.rs:104-107).
#[logic]
#[requires(rg_inv(a) && a.slabs@.get(s@) == Some(sl) && free_case(a, b, s, i, sl) && ne(a))]
#[ensures(ne(b))]
pub fn lemma_free_ne(a: RegionState, b: RegionState, s: u64, i: usize, sl: Slab) {
    pearlite! {
        if sl.bitmap.allocated_count@ == 1 {
            proof_assert! { b.slabs@ == a.slabs@.remove(s@) }; ()
        } else {
            proof_assert! { b.slabs@ == a.slabs@.insert(s@, b.slabs@.lookup(s@)) };
            proof_assert! { sl.bitmap.allocated_count@ >= 2 }; ()
        }
    }
}

/// Every slot live after the flush was live before with the same key and geometry.
#[logic(open)]
pub fn flush_back(a: RegionState, b: RegionState) -> bool {
    pearlite! { forall<t: Int, j: Int> slot_live(b, t, j) ==> keep_slot(b, a, t, j) }
}

#[logic]
#[requires(fl_spec(a, b))]
#[ensures(rg_good(a) ==> rg_good(b))]
#[ensures(flush_keep(a, b))]
#[ensures(flush_back(a, b))]
#[ensures(ne(a) ==> ne(b))]
pub fn lemma_fl_spec_elim(a: RegionState, b: RegionState) {}

/// free_slot never makes a slot live: every slot live after was live before, same key.
#[logic]
#[requires(rg_inv(a) && rg_inv(b) && rg_frame(a, b))]
#[requires(a.slabs@.get(s@) == Some(sl) && free_ok(a, s, i) && free_case(a, b, s, i, sl))]
#[ensures(forall<t: Int, j: Int> slot_live(b, t, j) ==> keep_slot(b, a, t, j) && (t != s@ || j != i@))]
pub fn lemma_free_back(a: RegionState, b: RegionState, s: u64, i: usize, sl: Slab) {
    pearlite! {
        proof_assert! { slab_ok(a, sl) && sl.start_offset@ == s@ && slab_inv(sl) };
        if sl.bitmap.allocated_count@ == 1 {
            proof_assert! { b.slabs@ == a.slabs@.remove(s@) };
            ()
        } else {
            proof_assert! { b.slabs@ == a.slabs@.insert(s@, b.slabs@.lookup(s@)) };
            proof_assert! { b.slabs@.contains(s@) && slab_ok(b, b.slabs@.lookup(s@)) && b.slabs@.lookup(s@).start_offset@ == s@ };
            proof_assert! { b.slabs@.lookup(s@).slab_size == sl.slab_size && b.slabs@.lookup(s@).start_offset == sl.start_offset
                && b.slabs@.lookup(s@).element_size == sl.element_size && b.slabs@.lookup(s@).bitmap.num_slots == sl.bitmap.num_slots };
            proof_assert! { forall<j: Int> 0 <= j && j < sl.bitmap.num_slots@ && j != i@ ==>
                b.slabs@.lookup(s@).keys@[j] == sl.keys@[j] && slot_bit(b.slabs@.lookup(s@).bitmap, j) == slot_bit(sl.bitmap, j) };
            proof_assert! { forall<t: Int> t != s@ ==> b.slabs@.get(t) == a.slabs@.get(t) };
            ()
        }
    }
}

/// flush's loop invariant after `i` deferred frees of `fs` (= `pre.pending_frees`).
#[logic]
pub fn fl_inv(pre: RegionState, cur: RegionState, fs: Seq<(u64, usize)>, i: Int) -> bool {
    pearlite! {
        (rg_good(pre) ==> rg_core(cur) && pvf(cur, fs, i))
        && (forall<t: Int, j: Int> slot_live(pre, t, j) && !inpend(fs, t, j) ==> keep_slot(pre, cur, t, j))
        && (forall<t: Int, j: Int> slot_live(cur, t, j) ==> keep_slot(cur, pre, t, j))
        && (ne(pre) ==> ne(cur))
    }
}

#[requires(*fs == pre.pending_frees@)]
#[requires(old.slabs == pre.slabs && old.size_classes == pre.size_classes && old.buddy == pre.buddy && old.format_params == pre.format_params)]
#[ensures(fl_spec(*pre, *pre))]
#[ensures(fl_inv(*pre, *old, *fs, 0))]
pub fn g_fl_init(pre: Snapshot<RegionState>, old: Snapshot<RegionState>, fs: Snapshot<Seq<(u64, usize)>>) {
    proof_assert! { rg_core(*pre) ==> { lemma_core_frame(*pre, *old); rg_core(*old) } };
    proof_assert! { forall<t: Int, j: Int> slot_live(*old, t, j) == slot_live(*pre, t, j) };
    proof_assert! { forall<t: Int, j: Int> slot_live(*pre, t, j) ==> keep_slot(*pre, *pre, t, j) && keep_slot(*old, *pre, t, j) && keep_slot(*pre, *old, t, j) };
    proof_assert! { flush_keep(*pre, *pre) && flush_back(*pre, *pre) };
    proof_assert! { ne(*pre) ==> ne(*old) };
}

/// `rg_core` reads only the slabs, the size classes, the buddy allocator and the parameters.
#[logic]
#[requires(a.slabs == b.slabs && a.size_classes == b.size_classes && a.buddy == b.buddy && a.format_params == b.format_params)]
#[requires(rg_core(a))]
#[ensures(rg_core(b))]
pub fn lemma_core_frame(a: RegionState, b: RegionState) {
    pearlite! {
        proof_assert! { rg_params(b) && rg_buddy(b) };
        proof_assert! { forall<x: Slab> slab_ok(a, x) == slab_ok(b, x) };
        proof_assert! { rg_slabs(b) && rg_inv(b) };
        proof_assert! { sk(a) == sk(b) && rg_tf(a) == rg_tf(b) };
        proof_assert! { sl_away(b) && rg_acct(b) && sc_ok(b) && rg_ka(b) }
    }
}

#[requires(fl_inv(*pre, *a, *fs, *i) && 0 <= *i && *i < fs.len() && fs[*i] == (s, j))]
#[requires(rg_inv(*a) && rg_inv(*b) && rg_frame(*a, *b))]
#[requires(a.slabs@.get(s@) == Some(*sl) && free_ok(*a, s, j) && free_case(*a, *b, s, j, *sl))]
#[requires(sl.bitmap.allocated_count@ == 1 ==>
    free_e(a.buddy.free_lists@, b.buddy.free_lists@, a.buddy.sector_size@, s@ - a.buddy.base_offset@, sk(*a)))]
#[requires(rg_tf(*b) == rg_tf(*a) + if sl.bitmap.allocated_count@ == 1 { sk(*a) } else { 0 })]
#[ensures(fl_inv(*pre, *b, *fs, *i + 1))]
pub fn g_fl_step(pre: Snapshot<RegionState>, a: Snapshot<RegionState>, b: Snapshot<RegionState>, fs: Snapshot<Seq<(u64, usize)>>,
    i: Snapshot<Int>, s: u64, j: usize, sl: Snapshot<Slab>) {
    proof_assert! { rg_good(*pre) ==> !inpf(*fs, *i + 1, s@, j@) && pvf(*a, *fs, *i + 1) };
    proof_assert! { rg_good(*pre) ==> { lemma_free_core(*a, *b, s, j, *sl, *fs, *i + 1); rg_core(*b) && pvf(*b, *fs, *i + 1) } };
    proof_assert! { lemma_free_core_keep(*a, *b, s, j, *sl); true };
    proof_assert! { lemma_free_back(*a, *b, s, j, *sl); true };
    proof_assert! { ne(*pre) ==> ne(*a) };
    proof_assert! { ne(*a) ==> { lemma_free_ne(*a, *b, s, j, *sl); ne(*b) } };
    proof_assert! { forall<t: Int, k: Int> slot_live(*b, t, k) ==> keep_slot(*b, *a, t, k) && slot_live(*a, t, k) };
    proof_assert! { forall<t: Int, k: Int> slot_live(*a, t, k) ==> keep_slot(*a, *pre, t, k) };
    proof_assert! { forall<t: Int, k: Int> slot_live(*b, t, k) ==> keep_slot(*b, *pre, t, k) };
    proof_assert! { forall<t: Int, k: Int> slot_live(*pre, t, k) && !inpend(*fs, t, k) ==> (t != s@ || k != j@) };
}

#[requires(fl_inv(*pre, *b, *fs, fs.len()) && b.pending_frees@.len() == 0 && *fs == pre.pending_frees@)]
#[ensures(fl_spec(*pre, *b))]
pub fn g_fl_end(pre: Snapshot<RegionState>, b: Snapshot<RegionState>, fs: Snapshot<Seq<(u64, usize)>>) {}

// ---------------------------------------------------------- rebuild (lib.rs:552-565)

/// Opaque alias of [`rg_good`] for contracts of large mirrors (no unfolding at the caller).
#[logic]
pub fn rg_good_o(r: RegionState) -> bool {
    pearlite! { rg_good(r) }
}

#[logic]
#[requires(rg_good_o(r))]
#[ensures(rg_good(r))]
pub fn lemma_rg_good_o(r: RegionState) {}

#[logic]
#[requires(rg_good(r))]
#[ensures(rg_good_o(r))]
pub fn lemma_rg_good_o_intro(r: RegionState) {}

/// Block size of a descriptor geometry.
#[logic(open)]
pub fn dsk(fp: FormatParams) -> Int {
    pearlite! { span(fp.sector_size@, ord(fp.slab_size@ / fp.sector_size@)) }
}

/// The aligned order-K block at `t` avoids the first `i` descriptors' blocks.
#[logic(open)]
pub fn off_descs(ds: Seq<SlabDescriptor>, i: Int, base: Int, fp: FormatParams, t: Int) -> bool {
    pearlite! { forall<p: Int> 0 <= p && p < i ==> disj(t, dsk(fp), ds[p].start_offset@ - base, dsk(fp)) }
}

/// rebuild's first loop after marking the first `i` descriptors (under a power-of-two
/// sector size and distinct descriptor starts).
#[logic]
pub fn rb1(b: BuddyAllocator, b0: BuddyAllocator, ds: Seq<SlabDescriptor>, i: Int, base: Int, size: Int, fp: FormatParams) -> bool {
    pearlite! {
        fdisj(b.free_lists@, fp.sector_size@)
        && (forall<p: Int> 0 <= p && p < i ==> faway(b.free_lists@, fp.sector_size@, ds[p].start_offset@ - base, dsk(fp)))
        && (forall<t: Int> 0 <= t && t % dsk(fp) == 0 && t + dsk(fp) <= size && off_descs(ds, i, base, fp, t) ==>
            inblk(b.free_lists@, fp.sector_size@, ord(fp.slab_size@ / fp.sector_size@), t))
        && tf(b.free_lists@, fp.sector_size@, b.free_lists@.len()) == tf(b0.free_lists@, fp.sector_size@, b0.free_lists@.len()) - i * dsk(fp)
    }
}

/// [`rb1`] read back outside this module (level-2 batch B; new lemma, no contract changed).
#[logic]
#[requires(rb1(b, b0, ds, i, base, size, fp))]
#[ensures(fdisj(b.free_lists@, fp.sector_size@))]
#[ensures(forall<p: Int> 0 <= p && p < i ==> faway(b.free_lists@, fp.sector_size@, ds[p].start_offset@ - base, dsk(fp)))]
#[ensures(forall<t: Int> 0 <= t && t % dsk(fp) == 0 && t + dsk(fp) <= size && off_descs(ds, i, base, fp, t) ==>
    inblk(b.free_lists@, fp.sector_size@, ord(fp.slab_size@ / fp.sector_size@), t))]
#[ensures(tf(b.free_lists@, fp.sector_size@, b.free_lists@.len()) == tf(b0.free_lists@, fp.sector_size@, b0.free_lists@.len()) - i * dsk(fp))]
pub fn lemma_rb1_elim(b: BuddyAllocator, b0: BuddyAllocator, ds: Seq<SlabDescriptor>, i: Int, base: Int, size: Int, fp: FormatParams) {}

/// [`rb1`] established outside this module (level-2 batch B; new lemma, no contract changed).
#[logic]
#[requires(fdisj(b.free_lists@, fp.sector_size@))]
#[requires(forall<p: Int> 0 <= p && p < i ==> faway(b.free_lists@, fp.sector_size@, ds[p].start_offset@ - base, dsk(fp)))]
#[requires(forall<t: Int> 0 <= t && t % dsk(fp) == 0 && t + dsk(fp) <= size && off_descs(ds, i, base, fp, t) ==>
    inblk(b.free_lists@, fp.sector_size@, ord(fp.slab_size@ / fp.sector_size@), t))]
#[requires(tf(b.free_lists@, fp.sector_size@, b.free_lists@.len()) == tf(b0.free_lists@, fp.sector_size@, b0.free_lists@.len()) - i * dsk(fp))]
#[ensures(rb1(b, b0, ds, i, base, size, fp))]
pub fn lemma_rb1_intro(b: BuddyAllocator, b0: BuddyAllocator, ds: Seq<SlabDescriptor>, i: Int, base: Int, size: Int, fp: FormatParams) {}

/// Marking keeps an aligned block that avoids the marked one inside a free block.
#[logic]
#[requires(ss > 0 && 0 <= kk && fdisj(a, ss) && inblk(a, ss, kk, t) && 0 <= t && t % span(ss, kk) == 0)]
#[requires(0 <= x && x % span(ss, kk) == 0 && disj(t, span(ss, kk), x, span(ss, kk)))]
#[requires(forall<z: Int> ffree(a, ss, z) && !(x <= z && z < x + span(ss, kk)) ==> ffree(b, ss, z))]
#[requires(fold_k(b, a, kk))]
#[requires(forall<o: Int, k: Int> fv(b, o, k) ==> fs(b, o, k) % span(ss, o) == 0 && fs(b, o, k) >= 0)]
#[ensures(inblk(b, ss, kk, t))]
pub fn lemma_cov_mark(a: Seq<Vec<u64>>, b: Seq<Vec<u64>>, ss: Int, kk: Int, x: Int, t: Int) {
    pearlite! {
        lemma_span_pos(ss, kk);
        proof_assert! { forall<o: Int, k: Int> fv(a, o, k) && o >= kk && fs(a, o, k) <= t && t + span(ss, kk) <= fs(a, o, k) + span(ss, o) ==> ffree(a, ss, t) };
        proof_assert! { ffree(a, ss, t) && ffree(b, ss, t) };
        proof_assert! { forall<o2: Int, k2: Int> fv(b, o2, k2) && o2 >= kk && fs(b, o2, k2) <= t && t < fs(b, o2, k2) + span(ss, o2) ==> {
            lemma_span_mult(ss, kk, o2);
            lemma_span_div(fs(b, o2, k2), ss, kk, o2);
            lemma_span_pos(ss, o2);
            lemma_lam(t, fs(b, o2, k2), span(ss, kk), span(ss, o2));
            inblk(b, ss, kk, t) } };
        proof_assert! { forall<o2: Int, k2: Int> fv(b, o2, k2) && o2 < kk && 0 <= o2 && fs(b, o2, k2) <= t && t < fs(b, o2, k2) + span(ss, o2) ==>
            forall<j: Int, oF: Int, kF: Int> fv(a, o2, j) && fs(a, o2, j) == fs(b, o2, k2)
                && fv(a, oF, kF) && oF >= kk && fs(a, oF, kF) <= t && t + span(ss, kk) <= fs(a, oF, kF) + span(ss, oF) ==> {
                lemma_same_blk(a, ss, o2, j, oF, kF, t); false } };
        proof_assert! { forall<o2: Int, k2: Int> fv(b, o2, k2) && fs(b, o2, k2) <= t && t < fs(b, o2, k2) + span(ss, o2) ==> inblk(b, ss, kk, t) }
    }
}

#[requires(new_e(b0.free_lists@, fp.sector_size@, (*size / fp.sector_size@) * fp.sector_size@))]
#[requires(b0.sector_size == fp.sector_size && fp.sector_size@ > 0 && *size >= 0 && b0.total_usable_size@ == *size)]
#[ensures(rb1(*b0, *b0, *ds, 0, *base, *size, *fp))]
pub fn g_rb_init(b0: Snapshot<BuddyAllocator>, ds: Snapshot<Seq<SlabDescriptor>>, base: Snapshot<Int>, size: Snapshot<Int>, fp: Snapshot<FormatParams>) {
    proof_assert! { lemma_new_e_elim(b0.free_lists@, fp.sector_size@, (*size / fp.sector_size@) * fp.sector_size@); true };
    proof_assert! { forall<t: Int> 0 <= t && t % dsk(*fp) == 0 && t + dsk(*fp) <= *size ==> {
        lemma_ord_ge(fp.slab_size@ / fp.sector_size@);
        lemma_cov_total(b0.free_lists@, fp.sector_size@, *size, ord(fp.slab_size@ / fp.sector_size@), t);
        inblk(b0.free_lists@, fp.sector_size@, ord(fp.slab_size@ / fp.sector_size@), t) } };
}

#[logic]
#[requires(p2(fp.sector_size@) && starts_distinct(ds) && rb1(bpre, b0, ds, i, base, size, fp))]
#[requires(0 <= i && i < ds.len() && desc_ok(ds[i], base, size, fp))]
#[requires(forall<p: Int> 0 <= p && p < ds.len() ==> desc_ok(ds[p], base, size, fp))]
#[requires(bpre.sector_size == fp.sector_size && bpost.sector_size == fp.sector_size && fp.sector_size@ > 0 && fp.slab_size@ > 0)]
#[requires(mk_spec(bpre.free_lists@, bpost.free_lists@, fp.sector_size@, ds[i].start_offset@ - base, ord(ds[i].slab_size@ / fp.sector_size@)))]
#[requires(bd_aligned(bpost))]
#[ensures(rb1(bpost, b0, ds, i + 1, base, size, fp))]
pub fn lemma_rb_step(bpre: BuddyAllocator, bpost: BuddyAllocator, b0: BuddyAllocator, ds: Seq<SlabDescriptor>, i: Int, base: Int, size: Int, fp: FormatParams) {
    pearlite! {
        lemma_ord_ge(fp.slab_size@ / fp.sector_size@);
        lemma_span_pos(fp.sector_size@, ord(fp.slab_size@ / fp.sector_size@));
        proof_assert! { ds[i].slab_size == fp.slab_size };
        proof_assert! { forall<p: Int> 0 <= p && p < i ==> ds[p].start_offset != ds[i].start_offset };
        proof_assert! { forall<p: Int> 0 <= p && p < i ==>
            (ds[i].start_offset@ - base < ds[p].start_offset@ - base ==> {
                lemma_aligned_disj2(ds[i].start_offset@ - base, ds[p].start_offset@ - base, dsk(fp));
                disj(ds[i].start_offset@ - base, dsk(fp), ds[p].start_offset@ - base, dsk(fp)) })
            && (ds[p].start_offset@ - base < ds[i].start_offset@ - base ==> {
                lemma_aligned_disj2(ds[p].start_offset@ - base, ds[i].start_offset@ - base, dsk(fp));
                disj(ds[i].start_offset@ - base, dsk(fp), ds[p].start_offset@ - base, dsk(fp)) }) };
        proof_assert! { off_descs(ds, i, base, fp, ds[i].start_offset@ - base) };
        proof_assert! { inblk(bpre.free_lists@, fp.sector_size@, ord(fp.slab_size@ / fp.sector_size@), ds[i].start_offset@ - base) };
        proof_assert! { mk_hyp(bpre.free_lists@, fp.sector_size@, ds[i].start_offset@ - base, ord(fp.slab_size@ / fp.sector_size@)) };
        lemma_mk_spec_elim(bpre.free_lists@, bpost.free_lists@, fp.sector_size@, ds[i].start_offset@ - base, ord(fp.slab_size@ / fp.sector_size@));
        proof_assert! { forall<o: Int, k: Int> fv(bpost.free_lists@, o, k) ==>
            fs(bpost.free_lists@, o, k) % span(fp.sector_size@, o) == 0 && fs(bpost.free_lists@, o, k) >= 0 };
        proof_assert! { forall<t: Int> 0 <= t && t % dsk(fp) == 0 && t + dsk(fp) <= size && off_descs(ds, i + 1, base, fp, t) ==> {
            lemma_cov_mark(bpre.free_lists@, bpost.free_lists@, fp.sector_size@, ord(fp.slab_size@ / fp.sector_size@), ds[i].start_offset@ - base, t);
            inblk(bpost.free_lists@, fp.sector_size@, ord(fp.slab_size@ / fp.sector_size@), t) } }
    }
}

#[requires(*g ==> p2(fp.sector_size@) && starts_distinct(*ds))]
#[requires(*g ==> rb1(*bpre, *b0, *ds, *i, *base, *size, *fp))]
#[requires(0 <= *i && *i < ds.len() && desc_ok(ds[*i], *base, *size, *fp))]
#[requires(forall<p: Int> 0 <= p && p < ds.len() ==> desc_ok(ds[p], *base, *size, *fp))]
#[requires(bpre.sector_size == fp.sector_size && bpost.sector_size == fp.sector_size && fp.sector_size@ > 0 && fp.slab_size@ > 0)]
#[requires(mk_spec(bpre.free_lists@, bpost.free_lists@, fp.sector_size@, ds[*i].start_offset@ - *base, ord(ds[*i].slab_size@ / fp.sector_size@)))]
#[requires(bd_aligned(*bpost))]
#[ensures(*g ==> rb1(*bpost, *b0, *ds, *i + 1, *base, *size, *fp))]
pub fn g_rb_step(bpre: Snapshot<BuddyAllocator>, bpost: Snapshot<BuddyAllocator>, b0: Snapshot<BuddyAllocator>, ds: Snapshot<Seq<SlabDescriptor>>,
    i: Snapshot<Int>, base: Snapshot<Int>, size: Snapshot<Int>, fp: Snapshot<FormatParams>, g: Snapshot<bool>) {
    proof_assert! { *g ==> { lemma_rb_step(*bpre, *bpost, *b0, *ds, *i, *base, *size, *fp); rb1(*bpost, *b0, *ds, *i + 1, *base, *size, *fp) } };
}

/// rebuild's second loop after inserting the first `i` recovered slabs.
#[logic]
pub fn rb2(r: RegionState, ds: Seq<SlabDescriptor>, i: Int) -> bool {
    pearlite! {
        r.slabs@.len() == i && sc_ok(r) && rg_ka(r)
        && forall<k: Int> r.slabs@.contains(k) ==> exists<p: Int> 0 <= p && p < i && ds[p].start_offset@ == k
    }
}

#[requires(r.slabs@ == FMap::empty() && forall<e: Int> sc_get(r.size_classes, e) == Seq::empty())]
#[ensures(rb2(*r, *ds, 0))]
pub fn g_rb2_init(r: Snapshot<RegionState>, ds: Snapshot<Seq<SlabDescriptor>>) {}

#[requires(*g ==> starts_distinct(*ds))]
#[requires(*g ==> rb2(*pre, *ds, *i))]
#[requires(0 <= *i && *i < ds.len() && sl.start_offset == ds[*i].start_offset && sl.element_size == ds[*i].element_size && key_alloc(*sl))]
#[requires(forall<e: Int> sc_get(mid.size_classes, e) == if e == sl.element_size@ { sc_get(pre.size_classes, e).push_back(sl.start_offset) } else { sc_get(pre.size_classes, e) })]
#[requires(mid.slabs == pre.slabs && post.size_classes == mid.size_classes && post.slabs@ == mid.slabs@.insert(sl.start_offset@, *sl))]
#[ensures(*g ==> rb2(*post, *ds, *i + 1))]
pub fn g_rb2_step(pre: Snapshot<RegionState>, mid: Snapshot<RegionState>, post: Snapshot<RegionState>, ds: Snapshot<Seq<SlabDescriptor>>,
    i: Snapshot<Int>, sl: Snapshot<Slab>, g: Snapshot<bool>) {
    proof_assert! { *g ==> !pre.slabs@.contains(sl.start_offset@) };
    proof_assert! { *g ==> post.slabs@.len() == *i + 1 };
    proof_assert! { *g ==> forall<k: Int> pre.slabs@.contains(k) ==> post.slabs@.lookup(k) == pre.slabs@.lookup(k) };
    proof_assert! { *g ==> sc_ok(*post) };
}

#[requires(*g ==> p2(fp.sector_size@) && starts_distinct(*ds))]
#[requires(*g ==> rb1(r.buddy, *b0, *ds, ds.len(), *base, *size, *fp) && rb2(*r, *ds, ds.len()))]
#[requires(rg_inv(*r) && r.format_params == *fp && r.pending_frees@.len() == 0)]
#[requires(r.buddy.base_offset@ == *base && r.buddy.total_usable_size@ == *size && r.buddy.sector_size == fp.sector_size)]
#[requires(tf(b0.free_lists@, fp.sector_size@, b0.free_lists@.len()) == (*size / fp.sector_size@) * fp.sector_size@)]
#[ensures(*g ==> rg_good_o(*r))]
pub fn g_rb_final(r: Snapshot<RegionState>, b0: Snapshot<BuddyAllocator>, ds: Snapshot<Seq<SlabDescriptor>>, base: Snapshot<Int>,
    size: Snapshot<Int>, fp: Snapshot<FormatParams>, g: Snapshot<bool>) {
    proof_assert! { *g ==> { lemma_rb_final(*r, *b0, *ds, *base, *size, *fp); rg_good_o(*r) } };
}

#[logic]
#[requires(p2(fp.sector_size@) && starts_distinct(ds))]
#[requires(rb1(r.buddy, b0, ds, ds.len(), base, size, fp) && rb2(r, ds, ds.len()))]
#[requires(rg_inv(r) && r.format_params == fp && r.pending_frees@.len() == 0)]
#[requires(r.buddy.base_offset@ == base && r.buddy.total_usable_size@ == size && r.buddy.sector_size == fp.sector_size)]
#[requires(tf(b0.free_lists@, fp.sector_size@, b0.free_lists@.len()) == (size / fp.sector_size@) * fp.sector_size@)]
#[ensures(rg_good_o(r))]
pub fn lemma_rb_final(r: RegionState, b0: BuddyAllocator, ds: Seq<SlabDescriptor>, base: Int, size: Int, fp: FormatParams) {
    pearlite! {
        proof_assert! { sk(r) == dsk(fp) };
        proof_assert! { forall<k: Int> r.slabs@.contains(k) ==> exists<p: Int> 0 <= p && p < ds.len() && ds[p].start_offset@ == k };
        proof_assert! { sl_away(r) };
        proof_assert! { rg_tf(r) == tf(r.buddy.free_lists@, fp.sector_size@, r.buddy.free_lists@.len()) };
        proof_assert! { rg_acct(r) };
        proof_assert! { rg_core(r) && pv(r, r.pending_frees@) };
        lemma_rg_good_o_intro(r)
    }
}

/// `alloc_any` read back outside this module.
#[logic]
#[requires(alloc_any(a, b, es, res))]
#[ensures(match res {
    Ok((s, i, _)) =>
        (sc_sub(a.size_classes, b.size_classes, es, -1) && b.buddy == a.buddy
            && listed(sc_get(a.size_classes, es), s@) && a.slabs@.contains(s@)
            && b.slabs@ == a.slabs@.insert(s@, b.slabs@.lookup(s@))
            && slot_step(a.slabs@.lookup(s@), b.slabs@.lookup(s@), i@))
        || (sc_sub(a.size_classes, b.size_classes, es, s@)
            && b.slabs@ == a.slabs@.insert(s@, b.slabs@.lookup(s@))
            && fresh_one(b.slabs@.lookup(s@), s@, a.format_params.slab_size@, es)
            && (exists<fo: Int> alloc_post(a.buddy, b.buddy, slab_k(a), fo, s@))
            && rg_tf(b) == rg_tf(a) - sk(a)),
    Err(_) => b.slabs@ == a.slabs@ && sc_sub(a.size_classes, b.size_classes, es, -1)
        && (b.buddy == a.buddy
            || exists<m: BuddyAllocator, fo: Int, d: Int> alloc_post(a.buddy, m, slab_k(a), fo, d)
                && m.sector_size == a.buddy.sector_size && m.free_lists@.len() == a.buddy.free_lists@.len()
                && free_e(m.free_lists@, b.buddy.free_lists@, m.sector_size@, d - a.buddy.base_offset@, sk(a))),
})]
pub fn lemma_alloc_any_read(a: RegionState, b: RegionState, es: Int, res: Result<(u64, usize, u64), EmError>) {}
