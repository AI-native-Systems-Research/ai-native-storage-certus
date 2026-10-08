//! Batch-5 proof units (pin 2cd35bac). One `verify_<id>` / `refute_<id>` per inventory id, each
//! with a `__mutant` twin; unscored `witness_<id>_pow2` / `_sequential` modules for refutations.
use crate::model::b3::*;
use crate::model::b4::*;
use crate::model::b5::*;
use crate::model::bitmap::*;
use crate::model::blockio::*;
use crate::model::buddy::*;
use crate::model::ckpt::*;
use crate::model::component::*;
use crate::model::listing::*;
use crate::model::l2r::*;
use crate::model::params::*;
use crate::model::region::*;
use crate::model::sbbytes::*;
use crate::model::slab::*;
use crate::props::batch1::*;
use crate::props::batch2::*;
use crate::props::batch4::*;
use crate::props::witness::*;
use crate::model::assume::*;
use crate::props::l3start::*;
use creusot_std::prelude::*;

// =============================================================================
// Region-level facts: slabs are pairwise disjoint (each is an aligned order-K buddy block)
// =============================================================================

/// A positive multiple of `m` is at least `m`.
#[logic]
#[requires(m > 0 && d > 0 && d % m == 0)]
#[ensures(d >= m)]
pub fn lemma_pos_mult(d: Int, m: Int) {
    pearlite! { lemma_divmod(d, m); lemma_mul_le(1, d / m, m) }
}

/// Two distinct slabs of a region do not overlap (both are aligned order-K blocks, K the slab
/// order, and a slab fits in its block — region.rs:72-77 + buddy.rs alignment).
#[logic]
#[requires(rg_inv(r) && r.slabs@.contains(a) && r.slabs@.contains(b) && a < b)]
#[ensures(a + r.format_params.slab_size@ <= b)]
pub fn lemma_slabs_disjoint(r: RegionState, a: Int, b: Int) {
    pearlite! {
        let sp = span(r.buddy.sector_size@, slab_k(r));
        let base = r.buddy.base_offset@;
        proof_assert! { slab_ok(r, r.slabs@.lookup(a)) && slab_ok(r, r.slabs@.lookup(b)) };
        proof_assert! { r.slabs@.lookup(a).start_offset@ == a && r.slabs@.lookup(b).start_offset@ == b };
        proof_assert! { (a - base) % sp == 0 && (b - base) % sp == 0 && a - base >= 0 };
        lemma_span_pos(r.buddy.sector_size@, slab_k(r));
        lemma_mod_add(b - base, a - base, sp);
        proof_assert! { (b - a) % sp == 0 };
        lemma_pos_mult(b - a, sp);
        lemma_slab_fits(r)
    }
}

/// Two slots of a region's slabs that start at the same offset belong to the same slab.
#[logic]
#[requires(rg_inv(r) && r.slabs@.contains(a) && r.slabs@.contains(b))]
#[requires(0 <= i && i < r.slabs@.lookup(a).bitmap.num_slots@ && 0 <= j && j < r.slabs@.lookup(b).bitmap.num_slots@)]
#[requires(slot_off(r.slabs@.lookup(a), i) == slot_off(r.slabs@.lookup(b), j))]
#[ensures(a == b)]
pub fn lemma_no_shared_slot(r: RegionState, a: Int, i: Int, b: Int, j: Int) {
    pearlite! {
        proof_assert! { slab_ok(r, r.slabs@.lookup(a)) && slab_ok(r, r.slabs@.lookup(b)) };
        lemma_slot_inside(r.slabs@.lookup(a), i);
        lemma_slot_inside(r.slabs@.lookup(b), j);
        if a < b { lemma_slabs_disjoint(r, a, b) } else if b < a { lemma_slabs_disjoint(r, b, a) } else { () }
    }
}

/// Every queued deferred free names a slot whose key is the free marker (the other half of
/// EM-REGION-PENDING-FREES-VALID; `pending_ok` is the first half).
#[logic(open)]
pub fn pend_free(r: RegionState) -> bool {
    pearlite! {
        forall<p: Int> 0 <= p && p < r.pending_frees@.len() ==>
            r.slabs@.contains(r.pending_frees@[p].0@)
            && r.pending_frees@[p].1@ < r.slabs@.lookup(r.pending_frees@[p].0@).keys@.len()
            && r.slabs@.lookup(r.pending_frees@[p].0@).keys@[r.pending_frees@[p].1@] == FREE_KEY
    }
}

/// EM-REGION-PENDING-FREES-VALID as one predicate.
#[logic(open)]
pub fn pend_valid(r: RegionState) -> bool {
    pearlite! { pending_ok(r) && pend_free(r) }
}

/// LEVEL-3 (phase D-B2): the PROVED region invariant rg_good gives rg_inv and pend_valid (its
/// pv(pending_frees) half: every queued slot is live with FREE_KEY, no duplicates).
#[logic]
#[requires(rg_good(r))]
#[ensures(rg_inv(r) && pend_valid(r))]
pub fn lemma_good_pend(r: RegionState) {
    pearlite! {
        proof_assert! { forall<p: Int> 0 <= p && p < r.pending_frees@.len() ==>
            slot_live(r, r.pending_frees@[p].0@, r.pending_frees@[p].1@)
            && r.slabs@.lookup(r.pending_frees@[p].0@).keys@[r.pending_frees@[p].1@] == FREE_KEY };
        proof_assert! { forall<p: Int> 0 <= p && p < r.pending_frees@.len() ==>
            slab_ok(r, r.slabs@.lookup(r.pending_frees@[p].0@)) };
        proof_assert! { pend_free(r) };
        proof_assert! { pending_ok(r) }
    }
}

/// A removal from a state whose deferred frees are valid leaves them valid.
#[logic]
#[requires(rg_inv(a) && pend_valid(a))]
#[requires(rm_case(a, off, s, i) && rm_post(a, b, s, i))]
#[ensures(pend_valid(b))]
pub fn lemma_rm_pending(a: RegionState, b: RegionState, off: u64, s: u64, i: usize) {
    pearlite! {
        proof_assert! { slab_inv(a.slabs@.lookup(s@)) };
        proof_assert! { forall<p: Int> 0 <= p && p < a.pending_frees@.len() ==> a.pending_frees@[p] != (s, i) };
        proof_assert! { forall<k: Int> b.slabs@.contains(k) == a.slabs@.contains(k) };
        proof_assert! { forall<k: Int> k != s@ ==> b.slabs@.lookup(k) == a.slabs@.lookup(k) };
        let la = a.slabs@.lookup(s@);
        let lb = b.slabs@.lookup(s@);
        proof_assert! { lb.bitmap == la.bitmap && lb.keys@ == la.keys@.set(i@, FREE_KEY) && lb.keys@.len() == la.keys@.len() };
        proof_assert! { b.pending_frees@ == a.pending_frees@.push_back((s, i)) };
        proof_assert! { forall<t: u64, j: usize> free_ok(a, t, j) ==> free_ok(b, t, j) };
        proof_assert! { free_ok(b, s, i) };
        proof_assert! { forall<p: Int> 0 <= p && p < b.pending_frees@.len() ==>
            b.slabs@.contains(b.pending_frees@[p].0@) && free_ok(b, b.pending_frees@[p].0, b.pending_frees@[p].1) };
        proof_assert! { forall<p: Int, q: Int> 0 <= p && p < q && q < b.pending_frees@.len() ==>
            b.pending_frees@[p] != b.pending_frees@[q] };
        proof_assert! { pending_ok(b) };
        proof_assert! { forall<p: Int> 0 <= p && p < a.pending_frees@.len() ==>
            b.slabs@.lookup(a.pending_frees@[p].0@).keys@.len() == a.slabs@.lookup(a.pending_frees@[p].0@).keys@.len()
            && (b.slabs@.lookup(a.pending_frees@[p].0@).keys@[a.pending_frees@[p].1@] == FREE_KEY) };
        proof_assert! { lb.keys@[i@] == FREE_KEY && i@ < lb.keys@.len() };
        proof_assert! { pend_free(b) };
        ()
    }
}

/// A second removal right after a successful one fails (no checkpoint in between).
#[logic]
#[requires(rg_inv(a))]
#[requires(rm_case(a, off, s, i) && rm_post(a, b, s, i))]
#[requires(rm_case(b, off, s2, i2))]
#[ensures(false)]
pub fn lemma_twice_noflush(a: RegionState, b: RegionState, off: u64, s: u64, i: usize, s2: u64, i2: usize) {
    pearlite! {
        proof_assert! { forall<k: Int> b.slabs@.contains(k) == a.slabs@.contains(k) };
        proof_assert! { s2 == s };
        proof_assert! { b.slabs@.lookup(s@).element_size@ > 0 };
        lemma_slot_off_inj(b.slabs@.lookup(s@), i@, i2@);
        proof_assert! { b.slabs@.lookup(s@).keys@[i@] == FREE_KEY };
        ()
    }
}

/// A second removal after a successful one and a checkpoint flush fails.
#[logic]
#[requires(rg_inv(a))]
#[requires(rm_case(a, off, s, i) && rm_post(a, b, s, i))]
#[requires(slabs_shrunk(b, c) && slot_gone(c, s, i))]
#[requires(rm_case(c, off, s2, i2))]
#[ensures(false)]
pub fn lemma_twice_flush(a: RegionState, b: RegionState, c: RegionState, off: u64, s: u64, i: usize, s2: u64, i2: usize) {
    pearlite! {
        proof_assert! { forall<k: Int> b.slabs@.contains(k) == a.slabs@.contains(k) };
        proof_assert! { a.slabs@.contains(s2@) };
        proof_assert! { c.slabs@.lookup(s2@).start_offset == a.slabs@.lookup(s2@).start_offset
            && c.slabs@.lookup(s2@).element_size == a.slabs@.lookup(s2@).element_size
            && c.slabs@.lookup(s2@).bitmap.num_slots == a.slabs@.lookup(s2@).bitmap.num_slots };
        proof_assert! { slot_off(a.slabs@.lookup(s2@), i2@) == off@ };
        lemma_no_shared_slot(a, s@, i@, s2@, i2@);
        proof_assert! { s2 == s };
        proof_assert! { a.slabs@.lookup(s@).element_size@ > 0 };
        lemma_slot_off_inj(a.slabs@.lookup(s@), i@, i2@);
        ()
    }
}

// =============================================================================
// EM-REMOVE-ERR-TWICE
// =============================================================================

/// **EM-REMOVE-ERR-TWICE** — "Removing the same extent a second time fails with an
/// OffsetNotFound error, whether or not a checkpoint happened in between." Mirror of
/// RegionState::remove_extent_by_offset (region.rs:121-150) called twice with the same offset;
/// `ckpt` = a successful checkpoint in between (lib.rs:312-321: `dirty = false;
/// flush_pending_frees()`, region.rs:152-160). From every region state satisfying the region
/// invariant and EM-REGION-PENDING-FREES-VALID's two halves, if the first removal succeeds the
/// second returns OffsetNotFound(offset).
/// LEVEL-3 (phase D-B2): the premise is the PROVED region invariant rg_good (lg); rg_inv and both
/// halves of EM-REGION-PENDING-FREES-VALID are derived from it (lemma_good_pend).
#[requires(rg_good(*r))]
#[ensures(match result.0 {
    Ok(()) => result.1 == Err(EmError::OffsetNotFound(offset)),
    Err(e) => e == EmError::OffsetNotFound(offset),
})]
pub fn verify_em_remove_err_twice(r: &mut RegionState, offset: u64, ckpt: bool) -> (Result<(), EmError>, Result<(), EmError>) {
    proof_assert! { lemma_good_pend(*r); rg_inv(*r) && pend_valid(*r) };
    let r0 = snapshot! { *r };
    let a = r.remove_extent_by_offset(offset);
    let r1 = snapshot! { *r };
    match a {
        Ok(()) => {
            proof_assert! { forall<s: u64, i: usize> rm_case(*r0, offset, s, i) && rm_post(*r0, *r1, s, i) ==> {
                lemma_rm_pending(*r0, *r1, offset, s, i); pend_valid(*r1) } };
            proof_assert! { pend_valid(*r1) };
            if ckpt {
                r.dirty = false;
                let r1d = snapshot! { *r };
                proof_assert! { r1d.slabs == r1.slabs && r1d.pending_frees == r1.pending_frees && pending_ok(*r1d) && rg_inv(*r1d) };
                r.flush_pending_frees();
                proof_assert! { slabs_shrunk(*r1, *r) && flushed_all(*r1, *r) };
            }
            let r2 = snapshot! { *r };
            proof_assert! { !ckpt ==> *r2 == *r1 };
            let b = r.remove_extent_by_offset(offset);
            match b {
                Ok(()) => {
                    proof_assert! { rm_step(*r2, *r, offset) };
                    proof_assert! { ckpt ==> forall<s: u64, i: usize, s2: u64, i2: usize>
                        rm_case(*r0, offset, s, i) && rm_post(*r0, *r1, s, i) && rm_case(*r2, offset, s2, i2) ==> {
                            proof_assert! { r1.pending_frees@[r1.pending_frees@.len() - 1] == (s, i) };
                            proof_assert! { slot_gone(*r2, s, i) };
                            lemma_twice_flush(*r0, *r1, *r2, offset, s, i, s2, i2); false } };
                    proof_assert! { !ckpt ==> forall<s: u64, i: usize, s2: u64, i2: usize>
                        rm_case(*r0, offset, s, i) && rm_post(*r0, *r1, s, i) && rm_case(*r2, offset, s2, i2) ==> {
                            lemma_twice_noflush(*r0, *r1, offset, s, i, s2, i2); false } };
                    proof_assert! { false };
                }
                Err(_) => {}
            }
            (a, b)
        }
        Err(_) => (a, Err(EmError::OffsetNotFound(offset))),
    }
}

/// Mutant: claims the second removal succeeds.
#[requires(rg_good(*r))]
#[ensures(match result.0 { Ok(()) => result.1 == Ok(()), Err(_) => true })]
pub fn verify_em_remove_err_twice__mutant(r: &mut RegionState, offset: u64, ckpt: bool) -> (Result<(), EmError>, Result<(), EmError>) {
    let a = r.remove_extent_by_offset(offset);
    match a {
        Ok(()) => {
            if ckpt {
                r.dirty = false;
            }
            let b = r.remove_extent_by_offset(offset);
            (a, b)
        }
        Err(_) => (a, Err(EmError::OffsetNotFound(offset))),
    }
}

// =============================================================================
// EM-REMOVE-ERR-UNPUBLISHED
// =============================================================================

/// **EM-REMOVE-ERR-UNPUBLISHED** — "remove_extent() with the offset of a free slot or of a
/// reservation that has not been published fails with an OffsetNotFound error, and an
/// outstanding reservation stays intact." For every region state with the region invariant and
/// every slot `i` of slab `s` that is free or allocated-but-unpublished (key = FREE_KEY, which
/// is exactly a live reservation, region.rs:77-89 / lib.rs:610), remove_extent_by_offset at the
/// slot's offset (region.rs:121-150) returns OffsetNotFound and leaves the region unchanged:
/// the floor slab of the offset is `s` itself (slabs are disjoint aligned blocks) and the slot
/// fails region.rs:137.
#[requires(rg_inv(*r) && r.slabs@.contains(s@) && i@ < r.slabs@.lookup(s@).bitmap.num_slots@)]
#[requires(!slot_bit(r.slabs@.lookup(s@).bitmap, i@) || r.slabs@.lookup(s@).keys@[i@] == FREE_KEY)]
#[requires(offset@ == slot_off(r.slabs@.lookup(s@), i@))]
#[ensures(result == Err(EmError::OffsetNotFound(offset)) && ^r == *r)]
pub fn verify_em_remove_err_unpublished(r: &mut RegionState, s: u64, i: usize, offset: u64) -> Result<(), EmError> {
    let r0 = snapshot! { *r };
    let res = r.remove_extent_by_offset(offset);
    match res {
        Ok(()) => {
            proof_assert! { forall<s2: u64, i2: usize> rm_case(*r0, offset, s2, i2) ==> {
                lemma_no_shared_slot(*r0, s@, i@, s2@, i2@);
                lemma_slot_off_inj(r0.slabs@.lookup(s@), i@, i2@);
                false } };
            proof_assert! { false };
        }
        Err(_) => {}
    }
    res
}

/// Mutant: claims the removal succeeds.
#[requires(rg_inv(*r) && r.slabs@.contains(s@) && i@ < r.slabs@.lookup(s@).bitmap.num_slots@)]
#[requires(!slot_bit(r.slabs@.lookup(s@).bitmap, i@) || r.slabs@.lookup(s@).keys@[i@] == FREE_KEY)]
#[requires(offset@ == slot_off(r.slabs@.lookup(s@), i@))]
#[ensures(result == Ok(()))]
pub fn verify_em_remove_err_unpublished__mutant(r: &mut RegionState, s: u64, i: usize, offset: u64) -> Result<(), EmError> {
    r.remove_extent_by_offset(offset)
}

// =============================================================================
// EM-REMOVE-FRAME-OTHER-EXTENTS
// =============================================================================

/// The slot facts of every extent at another offset are untouched by one removal step.
#[logic]
#[requires(rg_inv(a) && rm_case(a, off, s, i) && rm_post(a, b, s, i))]
#[requires(e.offset != off)]
#[ensures(from_slot(a, k, j, e) == from_slot(b, k, j, e))]
pub fn lemma_rm_other(a: RegionState, b: RegionState, off: u64, s: u64, i: usize, k: Int, j: Int, e: Extent) {
    pearlite! {
        proof_assert! { forall<t: Int> b.slabs@.contains(t) == a.slabs@.contains(t) };
        if k == s@ {
            let la = a.slabs@.lookup(s@);
            let lb = b.slabs@.lookup(s@);
            proof_assert! { lb.bitmap == la.bitmap && lb.keys@ == la.keys@.set(i@, FREE_KEY)
                && lb.start_offset == la.start_offset && lb.element_size == la.element_size };
            proof_assert! { slot_off(lb, j) == slot_off(la, j) };
            proof_assert! { slab_ok(a, la) && slab_inv(la) && i@ < la.keys@.len() && la.keys@.len() == la.bitmap.num_slots@ };
            if j == i@ {
                proof_assert! { slot_off(la, i@) == off@ && lb.keys@[i@] == FREE_KEY }; ()
            } else {
                proof_assert! { 0 <= j && j < la.keys@.len() ==> lb.keys@[j] == la.keys@[j] }; ()
            }
        } else { proof_assert! { b.slabs@.lookup(k) == a.slabs@.lookup(k) }; () }
    }
}

/// One removal step publishes nothing.
#[logic]
#[requires(rg_inv(a) && rm_case(a, off, s, i) && rm_post(a, b, s, i))]
#[requires(from_slot(b, k, j, e))]
#[ensures(from_slot(a, k, j, e))]
pub fn lemma_rm_nonew(a: RegionState, b: RegionState, off: u64, s: u64, i: usize, k: Int, j: Int, e: Extent) {
    pearlite! {
        proof_assert! { forall<t: Int> b.slabs@.contains(t) == a.slabs@.contains(t) };
        if k == s@ {
            let la = a.slabs@.lookup(s@);
            let lb = b.slabs@.lookup(s@);
            proof_assert! { lb.bitmap == la.bitmap && lb.keys@ == la.keys@.set(i@, FREE_KEY)
                && lb.start_offset == la.start_offset && lb.element_size == la.element_size };
            proof_assert! { slot_off(lb, j) == slot_off(la, j) };
            proof_assert! { slab_ok(a, la) && slab_inv(la) && i@ < la.keys@.len() && la.keys@.len() == la.bitmap.num_slots@ };
            if j == i@ {
                proof_assert! { slot_off(la, i@) == off@ && lb.keys@[i@] == FREE_KEY }; ()
            } else {
                proof_assert! { 0 <= j && j < la.keys@.len() ==> lb.keys@[j] == la.keys@[j] }; ()
            }
        } else { proof_assert! { b.slabs@.lookup(k) == a.slabs@.lookup(k) }; () }
    }
}

/// **EM-REMOVE-FRAME-OTHER-EXTENTS** — "remove_extent(offset) affects only the extent at that
/// offset; the key, offset, size and visibility of every other extent, and the reported
/// capacity, stay the same." Mirror of IExtentManager::remove_extent (lib.rs:680-684 with
/// region_for_offset lib.rs:223-255 inlined, model/component.rs) for every component state and
/// offset: in every region, an extent at any OTHER offset is built from a slot (get_extents,
/// lib.rs:632-656: key, slot offset, element size) after the call iff it was before, no
/// extent appears, the published region set is the same, and every region's capacity term
/// (buddy.total_usable_size, lib.rs:710-715) is unchanged.
#[requires(em_regions_wf(*em) && em_regions_ok(*em))]
#[ensures((^em).regions == em.regions && (^em).arena@.len() == em.arena@.len())]
#[ensures(forall<x: Int, k: Int, j: Int, e: Extent> 0 <= x && x < em.arena@.len() && e.offset != offset ==>
    from_slot(em.arena@[x], k, j, e) == from_slot((^em).arena@[x], k, j, e))]
#[ensures(forall<x: Int, k: Int, j: Int, e: Extent> 0 <= x && x < em.arena@.len() && from_slot((^em).arena@[x], k, j, e) ==>
    from_slot(em.arena@[x], k, j, e))]
#[ensures(forall<x: Int> 0 <= x && x < em.arena@.len() ==> (^em).arena@[x].buddy.total_usable_size == em.arena@[x].buddy.total_usable_size)]
#[ensures(match em.regions { Some(rv) => cap_sum((^em).arena@, rv@, rv@.len()) == cap_sum(em.arena@, rv@, rv@.len()), None => true })]
pub fn verify_em_remove_frame_other_extents(em: &mut ExtentManager, offset: u64) -> Result<(), EmError> {
    let e0 = snapshot! { *em };
    let res = em.remove_extent(offset);
    proof_assert! { forall<x: Int, s: u64, i: usize, k: Int, j: Int, e: Extent>
        0 <= x && x < e0.arena@.len() && rg_inv(e0.arena@[x]) && rm_case(e0.arena@[x], offset, s, i) && rm_post(e0.arena@[x], em.arena@[x], s, i) && e.offset != offset ==> {
            lemma_rm_other(e0.arena@[x], em.arena@[x], offset, s, i, k, j, e);
            from_slot(e0.arena@[x], k, j, e) == from_slot(em.arena@[x], k, j, e) } };
    proof_assert! { forall<x: Int, s: u64, i: usize, k: Int, j: Int, e: Extent>
        0 <= x && x < e0.arena@.len() && rg_inv(e0.arena@[x]) && rm_case(e0.arena@[x], offset, s, i) && rm_post(e0.arena@[x], em.arena@[x], s, i)
        && from_slot(em.arena@[x], k, j, e) ==> {
            lemma_rm_nonew(e0.arena@[x], em.arena@[x], offset, s, i, k, j, e);
            from_slot(e0.arena@[x], k, j, e) } };
    proof_assert! { match e0.regions { Some(rv) => { lemma_cap_frame(em.arena@, e0.arena@, rv@, rv@.len());
        cap_sum(em.arena@, rv@, rv@.len()) == cap_sum(e0.arena@, rv@, rv@.len()) }, None => true } };
    res
}

/// Mutant: claims every extent of the regions keeps its slot facts (also the removed one).
#[requires(em_regions_wf(*em) && em_regions_ok(*em))]
#[ensures(forall<x: Int, k: Int, j: Int, e: Extent> 0 <= x && x < em.arena@.len() ==>
    from_slot(em.arena@[x], k, j, e) == from_slot((^em).arena@[x], k, j, e))]
pub fn verify_em_remove_frame_other_extents__mutant(em: &mut ExtentManager, offset: u64) -> Result<(), EmError> {
    em.remove_extent(offset)
}

// =============================================================================
// EM-RESERVE-FRAME-ON-ERROR / EM-REGION-NEW-SLAB-ROLLBACK / EM-RESERVE-NO-SPURIOUS-OUT-OF-SPACE
// =============================================================================

/// **EM-RESERVE-FRAME-ON-ERROR** — "When reserve_extent() fails, used_bytes(), the listed
/// extents and the space available for later reservations are the same as before the call."
/// Mirror of RegionState::alloc_extent (region.rs:41-92; reserve_extent's only state change,
/// lib.rs:584-592) for every region state and size: on Err the slab map is identical (every
/// slot's key, offset, size, allocation bit — hence get_extents and every slab's free slots),
/// the buddy allocator's free bytes are identical (so `total_usable_size - total_free`, this
/// region's used_bytes term, lib.rs:704, is identical), the deferred frees and dirty flag are
/// untouched, and every slab with a free slot is still listed in its size class (only stale
/// entries of full/missing slabs may have been dropped, region.rs:62-63).
#[requires(rg_inv(*r) && size@ > 0 && a_3783f8(size@, r.format_params.sector_size@))]
#[ensures(match result {
    Ok(_) => true,
    Err(e) => e == EmError::OutOfSpace && (^r).slabs@ == r.slabs@ && rg_tf(^r) == rg_tf(*r)
        && rg_used(^r) == rg_used(*r) && (^r).pending_frees == r.pending_frees && (^r).dirty == r.dirty
        && (forall<k: Int, es: Int> free_at(^r, k, es) == free_at(*r, k, es))
        && (forall<k: Int, j: Int, ex: Extent> from_slot(^r, k, j, ex) == from_slot(*r, k, j, ex))
        && (nonfull_listed(*r) ==> nonfull_listed(^r)),
})]
pub fn verify_em_reserve_frame_on_error(r: &mut RegionState, size: u32) -> Result<(u64, usize, u64), EmError> {
    r.alloc_extent(size)
}

/// Mutant: claims a failed reservation changes the free space.
#[requires(rg_inv(*r) && size@ > 0 && a_3783f8(size@, r.format_params.sector_size@))]
#[ensures(match result { Ok(_) => true, Err(_) => rg_tf(^r) != rg_tf(*r) })]
pub fn verify_em_reserve_frame_on_error__mutant(r: &mut RegionState, size: u32) -> Result<(u64, usize, u64), EmError> {
    r.alloc_extent(size)
}

/// **EM-REGION-NEW-SLAB-ROLLBACK** — "If a newly carved slab cannot provide even one slot, its
/// space is returned to the allocator before the reservation fails, so nothing is leaked."
/// Mirror of RegionState::alloc_extent (region.rs:41-92) for every region state and size whose
/// size class is empty and whose fresh slab has ZERO slots (`(slab_size / element_size) as u32
/// == 0`, slab.rs:18 — e.g. sector 1, slab 2^32, size 1): the reservation fails with
/// OutOfSpace, and afterwards the buddy allocator's free bytes equal those before the call
/// (buddy.alloc took span(K), region.rs:72-75; buddy.free gave span(K) back, region.rs:81), the
/// slab map is unchanged and this region's used_bytes term is unchanged.
/// LEVEL-3 (phase D-B2): WIDENED to every size-class list (the old premise "the size class is
/// empty" is gone): every listed slab of that class has the class's element size (sc_ok, part of
/// the PROVED invariant rg_good), so it has zero slots too and cannot serve the request; the
/// reservation reaches the carve path, which hands the block back.
#[requires(rg_good(*r) && size@ > 0 && a_3783f8(size@, r.format_params.sector_size@))]
#[requires(slots_of(r.format_params.slab_size@, align_l(size@, r.format_params.sector_size@)) == 0)]
#[ensures(result == Err(EmError::OutOfSpace))]
#[ensures((^r).slabs@ == r.slabs@ && rg_tf(^r) == rg_tf(*r) && rg_used(^r) == rg_used(*r))]
pub fn verify_em_region_new_slab_rollback(r: &mut RegionState, size: u32) -> Result<(u64, usize, u64), EmError> {
    let r0 = snapshot! { *r };
    let es = snapshot! { align_l(size@, r.format_params.sector_size@) };
    proof_assert! { rg_inv(*r0) && sc_ok(*r0) };
    proof_assert! { forall<p: Int> 0 <= p && p < sc_get(r0.size_classes, *es).len() ==>
        r0.slabs@.contains(sc_get(r0.size_classes, *es)[p]@)
        && r0.slabs@.lookup(sc_get(r0.size_classes, *es)[p]@).element_size@ == *es
        && slab_ok(*r0, r0.slabs@.lookup(sc_get(r0.size_classes, *es)[p]@))
        && r0.slabs@.lookup(sc_get(r0.size_classes, *es)[p]@).bitmap.num_slots@ == 0 };
    let res = r.alloc_extent(size);
    proof_assert! { alloc_any(*r0, *r, *es, res) };
    proof_assert! { lemma_alloc_any_read(*r0, *r, *es, res); true };
    proof_assert! { forall<x: (u64, usize, u64)> res == Ok(x) ==>
        (listed(sc_get(r0.size_classes, *es), x.0@) && slot_step(r0.slabs@.lookup(x.0@), r.slabs@.lookup(x.0@), x.1@))
        || fresh_one(r.slabs@.lookup(x.0@), x.0@, r0.format_params.slab_size@, *es) };
    proof_assert! { forall<x: (u64, usize, u64)> res == Ok(x) && listed(sc_get(r0.size_classes, *es), x.0@)
        && slot_step(r0.slabs@.lookup(x.0@), r.slabs@.lookup(x.0@), x.1@) ==> {
        lemma_listed_slot0(*r0, *es, x.0@);
        false } };
    proof_assert! { forall<x: (u64, usize, u64)> res == Ok(x)
        && fresh_one(r.slabs@.lookup(x.0@), x.0@, r0.format_params.slab_size@, *es) ==>
        r.slabs@.lookup(x.0@).bitmap.num_slots@ == 0 && cnt(r.slabs@.lookup(x.0@).bitmap, 0) == 0 && false };
    proof_assert! { forall<x: (u64, usize, u64)> res != Ok(x) };
    proof_assert! { match res { Ok(_) => false, Err(e) => e == EmError::OutOfSpace } };
    res
}

/// No slab of class `es` has a free slot when `slots_of(slab, es) == 0` (sc_ok + slab_ok).
#[logic]
#[requires(rg_inv(r) && sc_ok(r) && slots_of(r.format_params.slab_size@, es) == 0)]
#[requires(listed(sc_get(r.size_classes, es), s))]
#[ensures(r.slabs@.contains(s) && r.slabs@.lookup(s).bitmap.num_slots@ == 0)]
pub fn lemma_listed_slot0(r: RegionState, es: Int, s: Int) {
    pearlite! {
        proof_assert! { exists<p: Int> 0 <= p && p < sc_get(r.size_classes, es).len() && sc_get(r.size_classes, es)[p]@ == s };
        proof_assert! { r.slabs@.contains(s) && r.slabs@.lookup(s).element_size@ == es };
        proof_assert! { slab_ok(r, r.slabs@.lookup(s)) }
    }
}

/// Mutant: claims the carved block is leaked (free space shrinks).
#[requires(rg_good(*r) && size@ > 0 && a_3783f8(size@, r.format_params.sector_size@))]
#[requires(slots_of(r.format_params.slab_size@, align_l(size@, r.format_params.sector_size@)) == 0)]
#[ensures(rg_tf(^r) < rg_tf(*r))]
pub fn verify_em_region_new_slab_rollback__mutant(r: &mut RegionState, size: u32) -> Result<(u64, usize, u64), EmError> {
    r.alloc_extent(size)
}

/// **EM-RESERVE-NO-SPURIOUS-OUT-OF-SPACE** — "reserve_extent() does not report OutOfSpace while
/// some slab in the key's region with the required element size still has a free slot." Mirror
/// of RegionState::alloc_extent (region.rs:41-92) for every region state satisfying the region
/// invariant and the maintained size-class invariant nonfull_listed (EM-SIZECLASS-NONFULL-LISTED:
/// every slab with a free slot is listed under its element size; established by format and
/// initialize, lib.rs:563, preserved by every mutator), every size and every slab `k` of element
/// size `align(size)` with a free slot: the call succeeds (stale entries are skipped,
/// region.rs:62-63, and `k` is never one of them).
#[requires(rg_inv(*r) && nonfull_listed(*r) && size@ > 0 && a_3783f8(size@, r.format_params.sector_size@))]
#[requires(free_at(*r, k@, align_l(size@, r.format_params.sector_size@)))]
#[ensures(match result { Ok(_) => true, Err(_) => false })]
pub fn verify_em_reserve_no_spurious_out_of_space(r: &mut RegionState, size: u32, k: u64) -> Result<(u64, usize, u64), EmError> {
    r.alloc_extent(size)
}

/// Mutant: claims the call fails.
#[requires(rg_inv(*r) && nonfull_listed(*r) && size@ > 0 && a_3783f8(size@, r.format_params.sector_size@))]
#[requires(free_at(*r, k@, align_l(size@, r.format_params.sector_size@)))]
#[ensures(match result { Ok(_) => false, Err(_) => true })]
pub fn verify_em_reserve_no_spurious_out_of_space__mutant(r: &mut RegionState, size: u32, k: u64) -> Result<(u64, usize, u64), EmError> {
    r.alloc_extent(size)
}

// =============================================================================
// EM-SLAB-OFFSET-SLOT-INVERSE
// =============================================================================

/// **EM-SLAB-OFFSET-SLOT-INVERSE** — "Looking up the slot for the start offset of slot i
/// returns i, and looking up an offset that is not the start of any slot of the slab returns
/// no slot." Mirrors of Slab::slot_offset / slot_for_offset (slab.rs:58-76) for every
/// well-formed slab, slot index and byte offset.
#[requires(slab_inv(*s) && i@ < s.bitmap.num_slots@)]
#[ensures(result.0 == Some(i))]
#[ensures((forall<j: Int> 0 <= j && j < s.bitmap.num_slots@ ==> slot_off(*s, j) != off@) ==> result.1 == None)]
pub fn verify_em_slab_offset_slot_inverse(s: &Slab, i: usize, off: u64) -> (Option<usize>, Option<usize>) {
    proof_assert! { lemma_slot_inside(*s, i@); slot_off(*s, i@) <= u64::MAX@ };
    let o = s.slot_offset(i);
    let a = s.slot_for_offset(o);
    proof_assert! { match a { Some(j) => { lemma_slot_off_inj(*s, i@, j@); j == i }, None => false } };
    let b = s.slot_for_offset(off);
    (a, b)
}

/// Mutant: claims the round trip finds no slot.
#[requires(slab_inv(*s) && i@ < s.bitmap.num_slots@)]
#[ensures(result.0 == None)]
pub fn verify_em_slab_offset_slot_inverse__mutant(s: &Slab, i: usize, off: u64) -> (Option<usize>, Option<usize>) {
    proof_assert! { lemma_slot_inside(*s, i@); slot_off(*s, i@) <= u64::MAX@ };
    let o = s.slot_offset(i);
    let a = s.slot_for_offset(o);
    let b = s.slot_for_offset(off);
    (a, b)
}

// =============================================================================
// Checkpoint copy decoding: EM-CKPT-READ-OVERSIZE / -SEQ-MISMATCH / -TRUNCATED
// =============================================================================

/// **EM-CKPT-READ-OVERSIZE** — "Reading a checkpoint copy whose header claims more data than
/// fits in the copy fails with a CorruptMetadata error; data that exactly fills the copy is
/// accepted." Byte-level read_checkpoint_region mirror (model/b4.rs read_ckpt_bytes,
/// checkpoint.rs:117-173) for every copy-region content, sector size >= 16, region size and
/// expected seq.
/// LEVEL-3 (phase D-B2): the rejection half now holds for EVERY metadata sector size `ss > 0`
/// (dfd6b0); the acceptance half still needs `ss >= 16` — for 1 <= ss <= 15 the code rejects every
/// copy (checkpoint.rs:128-130), so "data that exactly fills the copy is accepted" is false there
/// and no declared range bounds ss from below (advisory: NOT CREDITED).
#[requires(ss@ > 0 && region_size@ <= area@.len() && region_size@ % ss@ == 0 && area@.len() + ss@ <= usize::MAX@)]
#[requires(region_size@ > 0 || ss@ <= area@.len())]
#[ensures(16 + dec32(area@.subsequence(8, 12))@ > region_size@ ==> result == Err(EmError::CorruptMetadata))]
#[ensures(ss@ >= 16 && 16 + dec32(area@.subsequence(8, 12))@ == region_size@ && dec64(area@.subsequence(0, 8)) == expected
    && dec32(area@.subsequence(12, 16)) == crc32(chk(area@, region_size@)) ==>
    match result { Ok(v) => v@ == area@.subsequence(16, region_size@), Err(_) => false })]
pub fn narrowed_verify_em_ckpt_read_oversize(area: &Vec<u8>, ss: usize, region_size: u64, expected: u64) -> Result<Vec<u8>, EmError> {
    read_ckpt_bytes(area, ss, region_size, expected)
}

/// Mutant: claims an exactly-filling, valid copy is rejected.
#[requires(ss@ > 0 && region_size@ <= area@.len() && region_size@ % ss@ == 0 && area@.len() + ss@ <= usize::MAX@)]
#[requires(region_size@ > 0 || ss@ <= area@.len())]
#[ensures(ss@ >= 16 && 16 + dec32(area@.subsequence(8, 12))@ == region_size@ && dec64(area@.subsequence(0, 8)) == expected
    && dec32(area@.subsequence(12, 16)) == crc32(chk(area@, region_size@)) ==> result == Err(EmError::CorruptMetadata))]
pub fn narrowed_verify_em_ckpt_read_oversize__mutant(area: &Vec<u8>, ss: usize, region_size: u64, expected: u64) -> Result<Vec<u8>, EmError> {
    read_ckpt_bytes(area, ss, region_size, expected)
}

/// **EM-CKPT-READ-SEQ-MISMATCH** — "Reading a checkpoint copy whose recorded sequence number
/// differs from the expected one fails with a CorruptMetadata error." read_ckpt_bytes
/// (checkpoint.rs:132-140) for every copy content, sector size >= 16, region size and seq.
#[requires(ss@ > 0 && region_size@ <= area@.len() && region_size@ % ss@ == 0 && area@.len() + ss@ <= usize::MAX@)]
#[requires(region_size@ > 0 || ss@ <= area@.len())]
#[ensures(dec64(area@.subsequence(0, 8)) != expected ==> result == Err(EmError::CorruptMetadata))]
pub fn verify_em_ckpt_read_seq_mismatch(area: &Vec<u8>, ss: usize, region_size: u64, expected: u64) -> Result<Vec<u8>, EmError> {
    read_ckpt_bytes(area, ss, region_size, expected)
}

/// Mutant: claims a mismatching sequence number is accepted.
#[requires(ss@ > 0 && region_size@ <= area@.len() && region_size@ % ss@ == 0 && area@.len() + ss@ <= usize::MAX@)]
#[requires(region_size@ > 0 || ss@ <= area@.len())]
#[ensures(dec64(area@.subsequence(0, 8)) != expected ==> result != Err(EmError::CorruptMetadata))]
pub fn verify_em_ckpt_read_seq_mismatch__mutant(area: &Vec<u8>, ss: usize, region_size: u64, expected: u64) -> Result<Vec<u8>, EmError> {
    read_ckpt_bytes(area, ss, region_size, expected)
}

/// **EM-CKPT-READ-TRUNCATED** — "Reading a checkpoint copy that yields fewer bytes than the
/// header and recorded payload require fails with a CorruptMetadata error." read_checkpoint_
/// region mirror with the two read_blocks results ARBITRARY (model/b5.rs read_ckpt_raw): a
/// header read shorter than 16 bytes (checkpoint.rs:128-130), or a payload read — the header
/// sector itself when one sector suffices, else the second read (checkpoint.rs:150-156) —
/// shorter than 16 + the recorded length (checkpoint.rs:158-160) is CorruptMetadata, and every
/// failure of the function is CorruptMetadata.
#[requires(ss@ > 0 && ss@ <= 4294967295)]
#[ensures(h@.len() < 16 ==> result == Err(EmError::CorruptMetadata))]
#[ensures(h@.len() >= 16 && raw_of(h@, full@, ss@).len() < 16 + dec32(h@.subsequence(8, 12))@ ==> result == Err(EmError::CorruptMetadata))]
#[ensures(match result { Ok(_) => true, Err(e) => e == EmError::CorruptMetadata })]
pub fn verify_em_ckpt_read_truncated(h: Vec<u8>, full: Vec<u8>, ss: usize, region_size: u64, expected: u64) -> Result<Vec<u8>, EmError> {
    read_ckpt_raw(h, full, ss, region_size, expected)
}

/// Mutant: claims a too-short header read is accepted.
#[requires(ss@ > 0 && ss@ <= 4294967295)]
#[ensures(h@.len() < 16 ==> result != Err(EmError::CorruptMetadata))]
pub fn verify_em_ckpt_read_truncated__mutant(h: Vec<u8>, full: Vec<u8>, ss: usize, region_size: u64, expected: u64) -> Result<Vec<u8>, EmError> {
    read_ckpt_raw(h, full, ss, region_size, expected)
}

// =============================================================================
// EM-BIO-READ-LAYOUT
// =============================================================================

/// **EM-BIO-READ-LAYOUT** — "A successful metadata read of n bytes returns exactly n bytes,
/// taken from consecutive blocks starting at the base LBA plus the requested LBA." Mirror of
/// BlockDeviceClient::read_blocks (block_io.rs:110-155) over the ghost device (trusted
/// dev_read_sync = one ReadSync command per block per the IBlockDevice contract) for every
/// client, LBA and length: on success the result has exactly n bytes and byte q is byte
/// q % sector_size of the block at (ns_id, base_lba + lba + q / sector_size); the command log
/// records exactly those consecutive blocks in order.
#[requires(c.sector_size@ > 0)]
#[requires(n@ + c.sector_size@ <= usize::MAX@)]
#[requires(c.base_lba@ + lba@ + (n@ + c.sector_size@ - 1) / c.sector_size@ <= u64::MAX@)]
#[ensures(match result {
    Ok(v) => v@.len() == n@ && forall<q: Int> 0 <= q && q < n@ ==>
        match (*dev.store).get((c.ns_id@, c.base_lba@ + lba@ + q / c.sector_size@)) {
            Some(blk) => v@[q] == blk[q % c.sector_size@],
            None => true,
        },
    Err(e) => e == EmError::IoError,
})]
#[ensures(forall<p: Int> dev.log.len() <= p && p < (^dev).log.len() ==>
    (*(^dev).log)[p] == (c.ns_id@, c.base_lba@ + lba@ + (p - dev.log.len())))]
pub fn verify_em_bio_read_layout(c: &Client, dev: &mut BDev, lba: u64, n: usize) -> Result<Vec<u8>, EmError> {
    c.read_blocks(dev, lba, n)
}

/// Mutant: claims a successful read returns one byte more.
#[requires(c.sector_size@ > 0)]
#[requires(n@ + c.sector_size@ <= usize::MAX@)]
#[requires(c.base_lba@ + lba@ + (n@ + c.sector_size@ - 1) / c.sector_size@ <= u64::MAX@)]
#[ensures(match result { Ok(v) => v@.len() == n@ + 1, Err(_) => true })]
pub fn verify_em_bio_read_layout__mutant(c: &Client, dev: &mut BDev, lba: u64, n: usize) -> Result<Vec<u8>, EmError> {
    c.read_blocks(dev, lba, n)
}

// =============================================================================
// EM-SB-ACTIVE-COPY-VALID (refuted: SB-ACTIVE-COPY-UNCHECKED)
// =============================================================================

/// **EM-SB-ACTIVE-COPY-VALID** — REFUTED (INDEPENDENT; new root cause SB-ACTIVE-COPY-UNCHECKED).
/// "The active checkpoint copy number is always 0 or 1; decoding a superblock that names any
/// other copy fails with a CorruptMetadata error instead of crashing later." A superblock
/// sector whose bytes are a correctly encoded superblock (magic, CRC) naming copy 2 decodes
/// with Ok (superblock.rs:99-168 never checks active_copy; byte-level mirror model/sbbytes.rs),
/// and recovery then computes `1 - sb.active_copy` on u8 (recovery.rs:26): 1 - 2 underflows
/// (a debug build panics; a release build reads copy 255 at checkpoint_region_offset + 255 *
/// checkpoint_region_size).
#[ensures(result.0 == Ok(result.1) && result.1.active_copy@ == 2 && result.1.active_copy@ > 1)]
pub fn refute_em_sb_active_copy_valid() -> (Result<Superblock, EmError>, Superblock) {
    let mut sb = Superblock::new(1048576, 4096, 65536, 65536, 1, 4096, 8192, 7, 1, 0);
    sb.checkpoint_seq = 1;
    sb.active_copy = 2;
    let bytes = sb.serialize();
    let d = Superblock::deserialize(&bytes);
    (d, sb)
}

/// Mutant: claims the decoder rejects the copy-2 superblock with CorruptMetadata.
#[ensures(result.0 == Err(EmError::CorruptMetadata))]
pub fn refute_em_sb_active_copy_valid__mutant() -> (Result<Superblock, EmError>, Superblock) {
    let mut sb = Superblock::new(1048576, 4096, 65536, 65536, 1, 4096, 8192, 7, 1, 0);
    sb.checkpoint_seq = 1;
    sb.active_copy = 2;
    let bytes = sb.serialize();
    let d = Superblock::deserialize(&bytes);
    (d, sb)
}

// =============================================================================
// EM-NSID-POST-METADATA-NAMESPACE
// =============================================================================

/// **EM-NSID-POST-METADATA-NAMESPACE** — "After set_metadata_ns_id(n) on the concrete
/// ExtentManager, initialize() reads the superblock and checkpoints from NVMe namespace n; if
/// it was never called, namespace 1 is used." initialize (lib.rs:514-523) builds ONE client
/// with `metadata_ns_id.unwrap_or(1)` (lib.rs:517, mirror init_ns) via get_metadata_client
/// (lib.rs:185-209), and recovery issues every read through it: the superblock read
/// (recovery.rs:15) and each copy's header / payload reads (checkpoint.rs:127, 155). Proved for
/// every component state, every set (`Some(n)`) or no set (`None`), sector size, and ANY
/// sequence of reads after the superblock read: every command logged carries that namespace.
#[requires(ss@ > 0 && 4096 + ss@ <= usize::MAX@ && em.metadata_base_lba@ + (4096 + ss@ - 1) / ss@ <= u64::MAX@)]
#[requires(forall<p: Int> 0 <= p && p < reqs@.len() ==> reqs@[p].1@ + ss@ <= usize::MAX@
    && em.metadata_base_lba@ + reqs@[p].0@ + (reqs@[p].1@ + ss@ - 1) / ss@ <= u64::MAX@)]
#[ensures(result == match set { Some(n) => n, None => match em.metadata_ns_id { Some(m) => m, None => 1u32 } })]
#[ensures(set == None && em.metadata_ns_id == None ==> result == 1u32)]
#[ensures(forall<p: Int> dev.log.len() <= p && p < (^dev).log.len() ==> (*(^dev).log)[p].0 == result@)]
pub fn verify_em_nsid_post_metadata_namespace(em: &mut ExtentManager, set: Option<u32>, ss: u32, dev: &mut BDev, reqs: &Vec<(u64, usize)>) -> u32 {
    match set {
        Some(n) => em.set_metadata_ns_id(n),
        None => {}
    }
    let ns = em.init_ns();
    let c = em.get_metadata_client(ns, ss);
    let d0 = snapshot! { *dev };
    match c.read_blocks(dev, 0, 4096) {
        Ok(_) => {}
        Err(_) => return ns,
    }
    let mut p: usize = 0;
    #[invariant(p@ <= reqs@.len())]
    #[invariant(d0.log.len() <= dev.log.len())]
    #[invariant(forall<q: Int> d0.log.len() <= q && q < dev.log.len() ==> (*dev.log)[q].0 == ns@)]
    while p < reqs.len() {
        let (l, n) = reqs[p];
        match c.read_blocks(dev, l, n) {
            Ok(_) => {}
            Err(_) => return ns,
        }
        p += 1;
    }
    ns
}

/// Mutant: claims the namespace is always 1 even after set_metadata_ns_id(2).
#[requires(ss@ > 0 && 4096 + ss@ <= usize::MAX@ && em.metadata_base_lba@ + (4096 + ss@ - 1) / ss@ <= u64::MAX@)]
#[ensures(set == Some(2u32) ==> forall<p: Int> dev.log.len() <= p && p < (^dev).log.len() ==> (*(^dev).log)[p].0 == 1)]
#[ensures(set == Some(2u32) ==> (^dev).log.len() > dev.log.len())]
pub fn verify_em_nsid_post_metadata_namespace__mutant(em: &mut ExtentManager, set: Option<u32>, ss: u32, dev: &mut BDev) -> u32 {
    match set {
        Some(n) => em.set_metadata_ns_id(n),
        None => {}
    }
    let ns = em.init_ns();
    let c = em.get_metadata_client(ns, ss);
    let _ = c.read_blocks(dev, 0, 4096);
    ns
}

// =============================================================================
// EM-SETDBASE-POST-STORED / EM-SETMBASE-FRAME
// =============================================================================

/// One later call on the component (any IExtentManager method other than set_data_base_lba,
/// and the concrete setters).
pub enum DStep {
    Cop(COp),
    Format(FormatParams),
    Init(Superblock, Vec<Vec<SlabDescriptor>>),
    SetMBase(u64),
    SetNs(u32),
    Read,
}

/// **EM-SETDBASE-POST-STORED** — "After set_data_base_lba(b), data_base_lba() returns b until
/// set_data_base_lba() is called again." set_data_base_lba / data_base_lba (lib.rs:721-727)
/// for every component state and b, followed by ANY one other call — reserve / publish /
/// abort-or-drop / remove / checkpoint (COp, under its documented-use precondition), format,
/// initialize, set_metadata_base_lba, set_metadata_ns_id, or nothing — each of which preserves
/// the field (by induction, any sequence of them): data_base_lba() returns b.
// LEVEL-3 (phase D-B2): the intervening format() is any format call within format's own declared
// ranges (2ee144, 67ea4f, and fmt_sane = e0fe79 + 3b58ea) — no 7cb7c3 (format accepts slab_size 0).
#[requires(match st {
    DStep::Cop(op) => em_ok(*em) && cop_pre(*em, op),
    DStep::Format(p) => em_regions_wf(*em) && (em.dev.connected ==> a_67ea4f(em.dev.num_sectors@, em.dev.sector_size@))
        && a_2ee144(p.metadata_alignment@) && fmt_sane(p),
    DStep::Init(sb, pr) => em_regions_wf(*em) && em_regions_ok(*em) && sb_sane(sb) && recovered_ok(sb, pr@),
    _ => true,
})]
#[ensures(result == b)]
pub fn verify_em_setdbase_post_stored(em: &mut ExtentManager, b: u64, st: DStep) -> u64 {
    em.set_data_base_lba(b);
    match st {
        DStep::Cop(op) => apply_cop(em, op),
        DStep::Format(p) => {
            let _ = em.format(p);
        }
        DStep::Init(sb, pr) => em.initialize(sb, &pr),
        DStep::SetMBase(m) => em.set_metadata_base_lba(m),
        DStep::SetNs(n) => em.set_metadata_ns_id(n),
        DStep::Read => {}
    }
    em.data_base_lba()
}

/// Mutant: claims data_base_lba() returns b + 1 (same requires as the property).
#[requires(match st {
    DStep::Cop(op) => em_ok(*em) && cop_pre(*em, op),
    DStep::Format(p) => em_regions_wf(*em) && (em.dev.connected ==> a_67ea4f(em.dev.num_sectors@, em.dev.sector_size@))
        && a_2ee144(p.metadata_alignment@) && fmt_sane(p),
    DStep::Init(sb, pr) => em_regions_wf(*em) && em_regions_ok(*em) && sb_sane(sb) && recovered_ok(sb, pr@),
    _ => true,
})]
#[ensures(result@ == b@ + 1)]
pub fn verify_em_setdbase_post_stored__mutant(em: &mut ExtentManager, b: u64, st: DStep) -> u64 {
    em.set_data_base_lba(b);
    match st {
        DStep::Cop(op) => apply_cop(em, op),
        DStep::Format(p) => {
            let _ = em.format(p);
        }
        DStep::Init(sb, pr) => em.initialize(sb, &pr),
        DStep::SetMBase(m) => em.set_metadata_base_lba(m),
        DStep::SetNs(n) => em.set_metadata_ns_id(n),
        DStep::Read => {}
    }
    em.data_base_lba()
}

/// **EM-SETMBASE-FRAME** — "set_metadata_base_lba() does not change the value returned by
/// data_base_lba(), the listed extents, or their offsets." set_metadata_base_lba (lib.rs:717-
/// 719) for every component state and base: data_base_lba() (lib.rs:725-727) returns the same
/// value before and after, and get_extents (lib.rs:632-656, sound + complete mirror) lists
/// exactly the same extents (key, offset, size) before and after.
#[requires(em_regions_wf(*em) && em_regions_ok(*em))]
#[ensures(result.0 == result.1)]
#[ensures(forall<e: Extent> in_from(result.2@, 0, e) == in_from(result.3@, 0, e))]
pub fn verify_em_setmbase_frame(em: &mut ExtentManager, b: u64) -> (u64, u64, Vec<Extent>, Vec<Extent>) {
    let d0 = em.data_base_lba();
    let l0 = em.get_extents();
    let e0 = snapshot! { *em };
    em.set_metadata_base_lba(b);
    let d1 = em.data_base_lba();
    let l1 = em.get_extents();
    proof_assert! { em.arena == e0.arena && em.regions == e0.regions };
    proof_assert! { forall<e: Extent> in_from(l0@, 0, e) ==> match em.regions { Some(rv) =>
        exists<i: Int> 0 <= i && i < rv@.len() && from_region(em.arena@[rv@[i]@], e), None => false } };
    proof_assert! { forall<e: Extent> in_from(l1@, 0, e) ==> match em.regions { Some(rv) =>
        exists<i: Int> 0 <= i && i < rv@.len() && from_region(em.arena@[rv@[i]@], e), None => false } };
    (d0, d1, l0, l1)
}

/// Mutant: claims data_base_lba() changes.
#[requires(em_regions_wf(*em) && em_regions_ok(*em))]
#[ensures(result.0 != result.1)]
pub fn verify_em_setmbase_frame__mutant(em: &mut ExtentManager, b: u64) -> (u64, u64, Vec<Extent>, Vec<Extent>) {
    let d0 = em.data_base_lba();
    let l0 = em.get_extents();
    em.set_metadata_base_lba(b);
    let d1 = em.data_base_lba();
    let l1 = em.get_extents();
    (d0, d1, l0, l1)
}

// =============================================================================
// BUDDY-MASK chains (W2: sector 3, slab 6, one region [0,12); recovered B = (6, 6, 3, [K3, FREE]))
// =============================================================================

/// W2 after initialize, H4 = reserve(K4, 3) (B's free slot 1, offset 9) and H5 = reserve(K5, 6)
/// (fresh slab D at 0; [6,12) left on order 1): the recovered K3 still sits in slot 0 of B.
#[ensures(rg_inv(result) && result.format_params.sector_size@ == 3 && result.format_params.slab_size@ == 6)]
#[ensures(result.buddy.base_offset@ == 0 && result.buddy.max_order@ == 2)]
#[ensures(result.buddy.free_lists@[1]@ == Seq::singleton(6u64))]
#[ensures(result.slabs@.contains(6) && result.slabs@.contains(0))]
#[ensures(forall<k: Int> result.slabs@.contains(k) ==> k == 6 || k == 0)]
#[ensures(forall<j: Int> 0 <= j && j < result.slabs@.lookup(0).keys@.len() ==> result.slabs@.lookup(0).keys@[j] == FREE_KEY)]
#[ensures(forall<e: Int> sc_get(result.size_classes, e) == Seq::empty())]
#[ensures(from_slot(result, 6, 0, Extent { key: W_K3, size: 3u32, offset: 6u64 }))]
#[ensures(result.pending_frees@.len() == 0)]
pub fn w5_h5() -> RegionState {
    let mut r = w2_recovered();
    proof_assert! { align_l(3, 3) == 3 };
    let b0 = snapshot! { r.slabs@.lookup(6) };
    proof_assert! { slot_off(*b0, 0) == 6 && W_K3 != FREE_KEY };
    let old = snapshot! { r };
    proof_assert! { exist_cond(*old, 3, 6u64) };
    let res = r.alloc_extent(3);
    proof_assert! { exist_case(*old, r, 3, 6u64, res) };
    proof_assert! { r.slabs@.lookup(6).keys == b0.keys && r.slabs@.lookup(6).start_offset == b0.start_offset
        && r.slabs@.lookup(6).element_size == b0.element_size && r.slabs@.lookup(6).bitmap.num_slots == b0.bitmap.num_slots };
    proof_assert! { forall<k: Int> r.slabs@.contains(k) ==> k == 6 };
    proof_assert! { filter_ne(Seq::singleton(6u64), 6u64) == Seq::empty() };
    proof_assert! { forall<e: Int> sc_get(r.size_classes, e) == Seq::empty() };
    proof_assert! { from_slot(r, 6, 0, Extent { key: W_K3, size: 3u32, offset: 6u64 }) };
    proof_assert! { align_l(6, 3) == 6 && slots_of(6, 6) == 1 };
    proof_assert! { lemma_ord_eq(2, 0, 1); slab_k(r) == 1 };
    proof_assert! { r.buddy == old.buddy };
    proof_assert! { first_ne(r.buddy.free_lists@, 1, 2) == 2 };
    proof_assert! { span(3, 1) == 6 };
    let o2 = snapshot! { r };
    let res2 = r.alloc_extent(6);
    proof_assert! { fresh_case(*o2, r, 6, res2) };
    proof_assert! { match res2 { Ok((d, _, _)) => d@ == 0, Err(_) => false } };
    proof_assert! { r.slabs@.lookup(6) == o2.slabs@.lookup(6) };
    proof_assert! { from_slot(r, 6, 0, Extent { key: W_K3, size: 3u32, offset: 6u64 }) };
    r
}

/// **EM-RESERVE-FRAME** — REFUTED. ROOT CAUSE = BUDDY-MASK (non-power-of-two sector size).
/// "reserve_extent() does not change the key, offset, size or visibility of any extent that
/// already exists, does not change the reported capacity, and does not write the persisted
/// metadata." W2 (sector 3): after initialize the recovered slab B's block [6,12) was never
/// taken off the free lists (buddy.rs:136); after H4/H5 (`w5_h5`) reserve_extent(K6, 6) carves
/// a fresh slab at 6 and inserts it at B's key (region.rs:89), so the recovered, published
/// extent (K3, offset 6, size 3) is no longer listed. (Capacity and persisted metadata: unchanged.)
#[ensures(from_region(*result.0, Extent { key: W_K3, size: 3u32, offset: 6u64 }))]
#[ensures(!from_region(*result.1, Extent { key: W_K3, size: 3u32, offset: 6u64 }))]
#[ensures(match result.2 { Ok(_) => true, Err(_) => false })]
#[ensures((*result.0).buddy.total_usable_size == (*result.1).buddy.total_usable_size)]
pub fn support_old_refute_em_reserve_frame() -> (Snapshot<RegionState>, Snapshot<RegionState>, Result<(u64, usize, u64), EmError>) {
    let mut r = w5_h5();
    let before = snapshot! { r };
    proof_assert! { from_slot(*before, 6, 0, Extent { key: W_K3, size: 3u32, offset: 6u64 }) };
    proof_assert! { align_l(6, 3) == 6 && slots_of(6, 6) == 1 };
    proof_assert! { lemma_ord_eq(2, 0, 1); slab_k(r) == 1 };
    proof_assert! { first_ne(r.buddy.free_lists@, 1, 2) == 1 };
    let res = r.alloc_extent(6);
    proof_assert! { fresh_case(*before, r, 6, res) };
    proof_assert! { match res { Ok((d, _, _)) => d@ == 6, Err(_) => false } };
    let e = snapshot! { r.slabs@.lookup(6) };
    proof_assert! { fresh_one(*e, 6, 6, 6) };
    proof_assert! { r.slabs@.lookup(0) == before.slabs@.lookup(0) };
    proof_assert! { forall<k: Int> r.slabs@.contains(k) ==> k == 6 || k == 0 };
    proof_assert! { forall<k: Int, j: Int> !from_slot(r, k, j, Extent { key: W_K3, size: 3u32, offset: 6u64 }) };
    let after = snapshot! { r };
    (before, after, res)
}

/// Mutant: claims K3 is still listed after the reservation.
#[ensures(from_region(*result.1, Extent { key: W_K3, size: 3u32, offset: 6u64 }))]
pub fn support_old_refute_em_reserve_frame__mutant() -> (Snapshot<RegionState>, Snapshot<RegionState>, Result<(u64, usize, u64), EmError>) {
    let mut r = w5_h5();
    let before = snapshot! { r };
    let res = r.alloc_extent(6);
    let after = snapshot! { r };
    (before, after, res)
}

/// **EM-REGION-PENDING-FREES-VALID** — REFUTED. ROOT CAUSE = BUDDY-MASK (non-power-of-two
/// sector size). "Every queued removal refers to an existing slab and a slot that is still
/// marked allocated and carries the free marker as its key, and no slot is queued twice." W2:
/// after initialize, remove_extent(6) queues (6, 0) (K3 -> FREE, region.rs:142-148);
/// reserve(K5, 6) carves D at 0; reserve(K6, 6) carves E = [6,12) — B's block, never taken off
/// the free lists (buddy.rs:136) — REPLACING B at key 6 (region.rs:89); publish(K6) writes K6
/// into E's slot 0. The queued free (6, 0) now names a slot whose key is K6, not the free
/// marker (the next checkpoint's flush frees the published K6, batch 4's wfr_chain).
#[ensures(result.pending_frees@ == Seq::singleton((6u64, 0usize)))]
#[ensures(result.slabs@.contains(6) && result.slabs@.lookup(6).keys@.len() >= 1 && result.slabs@.lookup(6).keys@[0] == WR_K6)]
#[ensures(WR_K6 != FREE_KEY && !pend_free(result))]
pub fn support_old_refute_em_region_pending_frees_valid() -> RegionState {
    let mut r = w2_recovered();
    let pre = snapshot! { r };
    proof_assert! { slot_off(pre.slabs@.lookup(6), 0) == 6 && W_K3 != FREE_KEY };
    proof_assert! { rm_case(*pre, 6u64, 6u64, 0usize) };
    let rm = r.remove_extent_by_offset(6);
    proof_assert! { rm == Ok(()) && rm_post(*pre, r, 6u64, 0usize) };
    proof_assert! { r.pending_frees@ == Seq::singleton((6u64, 0usize)) };
    proof_assert! { forall<k: Int> r.slabs@.contains(k) ==> k == 6 };
    proof_assert! { align_l(6, 3) == 6 && slots_of(6, 6) == 1 };
    proof_assert! { lemma_ord_eq(2, 0, 1); slab_k(r) == 1 };
    proof_assert! { r.buddy == pre.buddy && sc_get(r.size_classes, 6) == Seq::empty() };
    proof_assert! { first_ne(r.buddy.free_lists@, 1, 2) == 2 };
    proof_assert! { span(3, 1) == 6 };
    let o1 = snapshot! { r };
    let a1 = r.alloc_extent(6);
    proof_assert! { fresh_case(*o1, r, 6, a1) };
    proof_assert! { match a1 { Ok((d, _, _)) => d@ == 0, Err(_) => false } };
    proof_assert! { r.buddy.free_lists@[1]@ == Seq::singleton(6u64) };
    proof_assert! { sc_get(r.size_classes, 6) == Seq::empty() };
    proof_assert! { r.pending_frees@ == Seq::singleton((6u64, 0usize)) };
    proof_assert! { first_ne(r.buddy.free_lists@, 1, 2) == 1 };
    let o2 = snapshot! { r };
    let a2 = r.alloc_extent(6);
    proof_assert! { fresh_case(*o2, r, 6, a2) };
    proof_assert! { match a2 { Ok((d, _, _)) => d@ == 6, Err(_) => false } };
    let e = snapshot! { r.slabs@.lookup(6) };
    proof_assert! { fresh_one(*e, 6, 6, 6) };
    proof_assert! { e.keys@.len() == 1 };
    r.publish_slot(6, 0, WR_K6);
    proof_assert! { r.slabs@.lookup(6).keys@ == e.keys@.set(0, WR_K6) };
    proof_assert! { r.slabs@.lookup(6).keys@[0] == WR_K6 };
    proof_assert! { r.pending_frees@ == Seq::singleton((6u64, 0usize)) };
    r
}

/// Mutant: claims the queued free still names a FREE-keyed slot.
#[ensures(pend_free(result))]
pub fn support_old_refute_em_region_pending_frees_valid__mutant() -> RegionState {
    support_old_refute_em_region_pending_frees_valid()
}

/// **EM-USED-COVERS-EXTENTS** — REFUTED. ROOT CAUSE = BUDDY-MASK (non-power-of-two sector
/// size). "used_bytes() is at least the total size of all extents that are currently reserved,
/// published, or removed but not yet released by a checkpoint." W2 (sector 3, one region
/// [0,12), so used_bytes() is this region's `total_usable_size - total_free`, lib.rs:700-708):
/// right after initialize the recovered slab B at 6 holds the published extent (K3, 6, 3), but
/// mark_allocated missed its block (buddy.rs:136), so the whole region is still one free
/// order-2 block: used_bytes() = 12 - 12 = 0 < 3.
#[ensures(from_slot(result.0, 6, 0, Extent { key: W_K3, size: 3u32, offset: 6u64 }))]
#[ensures(result.1@ == 0 && result.1@ < 3)]
pub fn support_old_refute_em_used_covers_extents() -> (RegionState, u64) {
    let r = w2_recovered();
    proof_assert! { bd_inv(r.buddy) && r.buddy.free_lists@.len() == 3 };
    proof_assert! { lemma_tf3(r.buddy.free_lists@, 3); span(3, 2) == 12 && tf(r.buddy.free_lists@, 3, 3) == 12 };
    proof_assert! { slot_off(r.slabs@.lookup(6), 0) == 6 && W_K3 != FREE_KEY };
    let u = r.buddy.total_usable_size();
    let f = r.buddy.total_free();
    let used = u - f;
    (r, used)
}

/// Mutant: claims used_bytes() covers K3.
#[ensures(result.1@ >= 3)]
pub fn support_old_refute_em_used_covers_extents__mutant() -> (RegionState, u64) {
    let r = w2_recovered();
    proof_assert! { bd_inv(r.buddy) && r.buddy.free_lists@.len() == 3 };
    proof_assert! { lemma_tf3(r.buddy.free_lists@, 3); span(3, 2) == 12 && tf(r.buddy.free_lists@, 3, 3) == 12 };
    let u = r.buddy.total_usable_size();
    let f = r.buddy.total_free();
    let used = u - f;
    (r, used)
}

/// A component holding one region (index 0, published) and nothing else.
#[ensures(result.arena@ == Seq::singleton(r) && result.shared == None)]
#[ensures(match result.regions { Some(rv) => rv@ == Seq::singleton(0usize), None => false })]
pub fn em_one(r: RegionState) -> ExtentManager {
    let mut arena: Vec<RegionState> = Vec::new();
    arena.push(r);
    let mut rv: Vec<usize> = Vec::new();
    rv.push(0);
    proof_assert! { rv@ == Seq::singleton(0usize) };
    ExtentManager {
        arena,
        regions: Some(rv),
        shared: None,
        metadata_ns_id: None,
        metadata_base_lba: 0,
        data_base_lba: 0,
        dev: MetaDevice { connected: true, sector_size: 4096, num_sectors: 4, sb: None, ckpt0: None, ckpt1: None },
    }
}

/// **EM-HANDLE-DROP-AUTO-ABORT** — REFUTED. ROOT CAUSE = BUDDY-MASK (non-power-of-two sector
/// size). "When a write handle is discarded without publish() or abort() having been called,
/// the reservation is aborted automatically with the same observable effect as abort() (the
/// extent is not visible and its slot is released), and this does not crash." W2b (sector 3):
/// H4 = reserve(K4, 3) names slot 1 of the recovered slab B at 6; reserve(K5, 6) and
/// reserve(K6, 6) then carve a fresh ONE-slot slab E at 6 that replaces B (buddy.rs:136 left
/// B's block free; region.rs:89). Dropping the live H4 (both closures present) runs the abort
/// closure (iextent_manager.rs:149-155 -> lib.rs:618-621 -> region.rs:94-99) = free_slot(6, 1)
/// on E: the WH::drop_wh precondition fails — slot 1 does not exist in E, and slab.rs:39
/// `self.keys[1]` on a 1-entry key vector panics (debug and release); H4's bytes [9,12) are
/// never released and stay inside H6's allocated slot. (drop runs the same closure as abort(),
/// so the two do have the same effect — here, the same crash.)
#[ensures(result.0.abort_fn && result.0.publish_fn)]
#[ensures(result.0.h.region@ == 0 && result.1.arena@.len() == 1)]
#[ensures(!free_ok(result.1.arena@[0], result.0.h.slab_start, result.0.h.slot_idx))]
#[ensures(result.1.arena@[0].slabs@.contains(result.0.h.slab_start@)
    && result.0.h.slot_idx@ >= result.1.arena@[0].slabs@.lookup(result.0.h.slab_start@).keys@.len())]
pub fn support_old_refute_em_handle_drop_auto_abort() -> (WH, ExtentManager) {
    let (r, s4, i4, off4, _off6) = w2b_replaced();
    proof_assert! { slab_inv(r.slabs@.lookup(6)) };
    let em = em_one(r);
    let wh = WH::new(Handle { region: 0, key: 4, offset: off4, size: 3, slab_start: s4, slot_idx: i4 });
    (wh, em)
}

/// Mutant: claims dropping H4 is a valid release.
#[ensures(free_ok(result.1.arena@[0], result.0.h.slab_start, result.0.h.slot_idx))]
pub fn support_old_refute_em_handle_drop_auto_abort__mutant() -> (WH, ExtentManager) {
    let (r, s4, i4, off4, _off6) = w2b_replaced();
    let em = em_one(r);
    let wh = WH::new(Handle { region: 0, key: 4, offset: off4, size: 3, slab_start: s4, slot_idx: i4 });
    (wh, em)
}

// =============================================================================
// pow2 witnesses: W2p (the W2 scenario at sector 4: slab 8, one region [0,16); recovered
// B = (8, 8, 4, [K3, FREE]); mark_allocated hits, buddy keeps [0,8) free)
// =============================================================================

/// W2p right after initialize (power-of-two sector: mark_allocated splits [0,16) and removes B).
#[ensures(rg_inv(result) && nonfull_listed(result) && result.pending_frees@.len() == 0 && !result.dirty)]
#[ensures(result.format_params.sector_size@ == 4 && result.format_params.slab_size@ == 8)]
#[ensures(result.buddy.base_offset@ == 0 && result.buddy.total_usable_size@ == 16 && result.buddy.sector_size@ == 4)]
#[ensures(result.buddy.max_order@ == 2 && result.buddy.free_lists@.len() == 3)]
#[ensures(result.buddy.free_lists@[0]@.len() == 0 && result.buddy.free_lists@[2]@.len() == 0)]
#[ensures(result.buddy.free_lists@[1]@.len() == 1 && result.buddy.free_lists@[1]@[0]@ == 0)]
#[ensures(result.slabs@.contains(8) && forall<k: Int> result.slabs@.contains(k) ==> k == 8)]
#[ensures(result.slabs@.lookup(8).bitmap.num_slots@ == 2 && result.slabs@.lookup(8).bitmap.allocated_count@ == 1)]
#[ensures(slot_bit(result.slabs@.lookup(8).bitmap, 0) && !slot_bit(result.slabs@.lookup(8).bitmap, 1))]
#[ensures(result.slabs@.lookup(8).keys@.len() == 2 && result.slabs@.lookup(8).keys@[0] == WP_K3 && result.slabs@.lookup(8).keys@[1] == FREE_KEY)]
#[ensures(result.slabs@.lookup(8).element_size@ == 4 && result.slabs@.lookup(8).start_offset@ == 8 && slab_inv(result.slabs@.lookup(8)))]
#[ensures(forall<e: Int> sc_get(result.size_classes, e) == if e == 4 { Seq::singleton(8u64) } else { Seq::empty() })]
#[ensures(slab_k(result) == 1)]
pub fn w5p_rec() -> RegionState {
    let fp = w2p_fp();
    let descs = w2p_descs();
    fact_mask_pow2();
    proof_assert! { 2.pow2() == 4 && 1.pow2() == 2 && 16 / 4 == 4 };
    proof_assert! { lemma_ord_eq(2, 0, 1); ord(8 / 4) == 1 };
    proof_assert! { span(4, 1) == 8 && span(4, 2) == 16 && slots_of(8, 4) == 2 && slots_of(8, 8) == 1 };
    proof_assert! { desc_ok(descs@[0], 0, 16, fp) };
    proof_assert! { forall<sp: u64, x: u64> sp@ == span(4, 2) && x@ == 8 ==> sp == 16u64 && x == 8u64 };
    proof_assert! { mark_hit_p2(descs@[0], 0, 16, fp, 2) };
    let r = rebuild_region(0, 16, fp, &descs);
    proof_assert! { rebuilt_hit(r, descs@[0], 0, 16, fp) && hit_lists(r.buddy, descs@[0], 0, fp, 2) };
    proof_assert! { rebuilt_one(r, descs@[0], 0, 16, fp) && one_slab(r, descs@[0]) };
    let bb = snapshot! { r.slabs@.lookup(8) };
    proof_assert! { bb.bitmap.num_slots@ == 2 && slot_bit(bb.bitmap, 0) && !slot_bit(bb.bitmap, 1) };
    proof_assert! { cnt(bb.bitmap, 0) == 0 && cnt(bb.bitmap, 1) == 1 && cnt(bb.bitmap, 2) == 1 };
    proof_assert! { bb.keys@[0] == WP_K3 && bb.keys@[1] == FREE_KEY && bb.keys@.len() == 2 };
    proof_assert! { slab_k(r) == 1 };
    r
}

/// pow2 witness of EM-RESERVE-FRAME: W2p — reserve(K4,4) (B's slot 1), reserve(K5,8) (fresh slab
/// at 0) and reserve(K6,4) (B full, no free block: OutOfSpace) each keep the recovered
/// (K3, 8, 4) listed; capacity unchanged.
#[ensures(from_slot(*result.0, 8, 0, Extent { key: WP_K3, size: 4u32, offset: 8u64 }))]
#[ensures(from_slot(*result.1, 8, 0, Extent { key: WP_K3, size: 4u32, offset: 8u64 }))]
#[ensures(from_slot(*result.2, 8, 0, Extent { key: WP_K3, size: 4u32, offset: 8u64 }))]
#[ensures(from_slot(*result.3, 8, 0, Extent { key: WP_K3, size: 4u32, offset: 8u64 }))]
#[ensures((*result.0).buddy.total_usable_size == (*result.3).buddy.total_usable_size)]
pub fn witness_em_reserve_frame_pow2() -> (Snapshot<RegionState>, Snapshot<RegionState>, Snapshot<RegionState>, Snapshot<RegionState>) {
    let mut r = w5p_rec();
    let s0 = snapshot! { r };
    proof_assert! { slot_off(s0.slabs@.lookup(8), 0) == 8 && WP_K3 != FREE_KEY };
    proof_assert! { align_l(4, 4) == 4 && align_l(8, 4) == 8 && slots_of(8, 8) == 1 };
    proof_assert! { exist_cond(*s0, 4, 8u64) };
    let h4 = r.alloc_extent(4);
    proof_assert! { exist_case(*s0, r, 4, 8u64, h4) };
    proof_assert! { forall<k: Int> r.slabs@.contains(k) ==> k == 8 };
    let s1 = snapshot! { r };
    proof_assert! { filter_ne(Seq::singleton(8u64), 8u64) == Seq::empty() };
    proof_assert! { sc_get(r.size_classes, 4) == Seq::empty() && sc_get(r.size_classes, 8) == Seq::empty() };
    proof_assert! { r.buddy == s0.buddy && first_ne(r.buddy.free_lists@, 1, 2) == 1 };
    let h5 = r.alloc_extent(8);
    proof_assert! { fresh_case(*s1, r, 8, h5) };
    proof_assert! { match h5 { Ok((d, _, _)) => d@ == 0, Err(_) => false } };
    proof_assert! { r.slabs@.lookup(8) == s1.slabs@.lookup(8) };
    let s2 = snapshot! { r };
    proof_assert! { r.buddy.free_lists@[1]@.len() == 0 && r.buddy.free_lists@[2]@.len() == 0 };
    proof_assert! { first_ne(r.buddy.free_lists@, 1, 2) == -1 };
    proof_assert! { sc_get(r.size_classes, 4) == Seq::empty() };
    let h6 = r.alloc_extent(4);
    proof_assert! { fresh_case(*s2, r, 4, h6) && r == *s2 };
    let s3 = snapshot! { r };
    (s0, s1, s2, s3)
}

/// pow2 witness of EM-REGION-PENDING-FREES-VALID: W2p — remove_extent(8) queues (8, 0);
/// reserve(K5,8) carves D at 0; reserve(K6,8) finds no space (no replacement): the queued free
/// still names an existing slab's allocated slot whose key is the free marker, once.
#[ensures(result.pending_frees@ == Seq::singleton((8u64, 0usize)))]
#[ensures(pend_valid(result))]
pub fn witness_em_region_pending_frees_valid_pow2() -> RegionState {
    let mut r = w5p_rec();
    let pre = snapshot! { r };
    proof_assert! { slot_off(pre.slabs@.lookup(8), 0) == 8 && WP_K3 != FREE_KEY };
    proof_assert! { rm_case(*pre, 8u64, 8u64, 0usize) };
    let rm = r.remove_extent_by_offset(8);
    proof_assert! { rm == Ok(()) && rm_post(*pre, r, 8u64, 0usize) };
    proof_assert! { r.pending_frees@ == Seq::singleton((8u64, 0usize)) };
    proof_assert! { align_l(8, 4) == 8 && slots_of(8, 8) == 1 };
    proof_assert! { sc_get(r.size_classes, 8) == Seq::empty() };
    proof_assert! { r.buddy == pre.buddy && first_ne(r.buddy.free_lists@, 1, 2) == 1 };
    let o2 = snapshot! { r };
    let h5 = r.alloc_extent(8);
    proof_assert! { fresh_case(*o2, r, 8, h5) };
    proof_assert! { match h5 { Ok((d, _, _)) => d@ == 0, Err(_) => false } };
    proof_assert! { r.slabs@.lookup(8) == o2.slabs@.lookup(8) };
    proof_assert! { r.buddy.free_lists@[1]@.len() == 0 && r.buddy.free_lists@[2]@.len() == 0 };
    proof_assert! { first_ne(r.buddy.free_lists@, 1, 2) == -1 };
    proof_assert! { sc_get(r.size_classes, 8) == Seq::empty() };
    let o3 = snapshot! { r };
    let h6 = r.alloc_extent(8);
    proof_assert! { fresh_case(*o3, r, 8, h6) && r == *o3 };
    proof_assert! { r.slabs@.lookup(8).keys@[0] == FREE_KEY && slot_bit(r.slabs@.lookup(8).bitmap, 0) };
    r
}

/// pow2 witness of EM-USED-COVERS-EXTENTS: W2p right after initialize, used_bytes() = 16 - 8 = 8
/// covers the recovered (K3, 8, 4).
#[ensures(from_slot(result.0, 8, 0, Extent { key: WP_K3, size: 4u32, offset: 8u64 }))]
#[ensures(result.1@ == 8 && result.1@ >= 4)]
pub fn witness_em_used_covers_extents_pow2() -> (RegionState, u64) {
    let r = w5p_rec();
    proof_assert! { lemma_tf3(r.buddy.free_lists@, 4); span(4, 1) == 8 && tf(r.buddy.free_lists@, 4, 3) == 8 };
    proof_assert! { slot_off(r.slabs@.lookup(8), 0) == 8 && WP_K3 != FREE_KEY };
    let u = r.buddy.total_usable_size();
    let f = r.buddy.total_free();
    let used = u - f;
    (r, used)
}

/// pow2 witness of EM-HANDLE-DROP-AUTO-ABORT: W2p — H4 = reserve(K4, 4) takes B's slot 1
/// (offset 12); dropping the live H4 runs the abort closure: slot 1 is released (bit clear),
/// its key is the free marker (not listed), and the recovered K3 is untouched.
#[ensures(!slot_bit(result.arena@[0].slabs@.lookup(8).bitmap, 1) && result.arena@[0].slabs@.lookup(8).keys@[1] == FREE_KEY)]
#[ensures(from_slot(result.arena@[0], 8, 0, Extent { key: WP_K3, size: 4u32, offset: 8u64 }))]
pub fn witness_em_handle_drop_auto_abort_pow2() -> ExtentManager {
    let mut r = w5p_rec();
    let s0 = snapshot! { r };
    proof_assert! { align_l(4, 4) == 4 };
    proof_assert! { exist_cond(*s0, 4, 8u64) };
    let h4 = r.alloc_extent(4);
    proof_assert! { exist_case(*s0, r, 4, 8u64, h4) };
    let (s, i, off) = match h4 {
        Ok(t) => t,
        Err(_) => (0, 0, 0),
    };
    proof_assert! { s@ == 8 && i@ == 1 };
    let b1 = snapshot! { r.slabs@.lookup(8) };
    proof_assert! { b1.bitmap.allocated_count@ == 2 && b1.keys == s0.slabs@.lookup(8).keys };
    proof_assert! { free_ok(r, 8u64, 1usize) && rg_inv(r) };
    let mut em = em_one(r);
    let wh = WH::new(Handle { region: 0, key: 4, offset: off, size: 4, slab_start: s, slot_idx: i });
    let e0 = snapshot! { em };
    wh.drop_wh(&mut em);
    proof_assert! { free_case(e0.arena@[0], em.arena@[0], 8u64, 1usize, *b1) };
    proof_assert! { rg_inv(em.arena@[0]) && em.arena@[0].slabs@.contains(8) };
    proof_assert! { em.arena@[0].slabs@.lookup(8).start_offset@ == 8 };
    proof_assert! { slot_off(em.arena@[0].slabs@.lookup(8), 0) == 8 && WP_K3 != FREE_KEY };
    em
}

// =============================================================================
// EM-REMOVE-CRASH-BEFORE-CHECKPOINT (refuted: CKPT-SB-ORDER)
// =============================================================================

pub const W5_K1: u64 = 61;

/// C8 (sector 4, slab 8, one region [0,8), batch 2's wc8_two): publish K1 into slot 0
/// (offset 0); checkpoint #1 succeeds (copy 1 = seq 1 = image with K1; superblock seq 1 /
/// copy 1); remove_extent(0) (K1 -> FREE, slot queued); [a crash HERE recovers image #1 with
/// K1]; checkpoint #2: copy 0 = seq 2 written, superblock write FAILS (IoError) — the in-memory
/// superblock already says seq 2 / copy 0 (checkpoint.rs:105-112); checkpoint #3 (still dirty)
/// writes `1 - 0` = copy 1 — the copy the ON-DISK superblock names — with seq 3, and its
/// superblock write fails too / the system crashes before it. Returns (#1, #2, #3, image #1,
/// recovery right after the removal, recovery after #3).
#[ensures(result.0 == Ok(()) && result.1 == Err(EmError::IoError) && result.2 == Err(EmError::IoError))]
#[ensures(img_lists(*result.3, Extent { key: W5_K1, size: 4u32, offset: 0u64 }))]
#[ensures(result.4 == Ok(result.3))]
#[ensures(match result.5 { Ok(_) => false, Err(_) => true })]
pub fn wrc_chain() -> (Result<(), EmError>, Result<(), EmError>, Result<(), EmError>, Snapshot<Seq<RegionState>>,
    Result<Snapshot<Seq<RegionState>>, EmError>, Result<Snapshot<Seq<RegionState>>, EmError>) {
    let r = wc8_two();
    let r0 = snapshot! { r };
    let mut rs: Vec<RegionState> = Vec::new();
    rs.push(r);
    let mut em = CkEm { regions: rs, active: 0, seq: 0, dev: wc_fresh_dev() };
    proof_assert! { em.regions@.len() == 1 && em.regions@[0] == *r0 };
    // LEVEL-3 reachable start: format's superblock (all format-start ranges), proved region
    // invariants with the live handles K1/K2, the reserve ranges, CkEm = that superblock.
    let sb0 = l3_sb_c8();
    proof_assert! { l3_fmt_ranges(wc_fp_l(8u64, 8u64), 3, 4096, sb0) && l3_rsv_ranges(4, wc_fp_l(8u64, 8u64)) };
    proof_assert! { rg_l3(em.regions@[0], hs_w2()) && regions_ok(em.regions@) };
    proof_assert! { em.active@ == sb0.active_copy@ && em.seq@ == sb0.checkpoint_seq@ && em.dev.sb_seq@ == sb0.checkpoint_seq@ && em.dev.sb_active@ == sb0.active_copy@ };
    em.regions[0].publish_slot(0, 0, W5_K1);
    proof_assert! { regions_ok(em.regions@) && !all_clean(em.regions@) };
    let img1 = snapshot! { em.regions@ };
    proof_assert! { slot_off(img1[0].slabs@.lookup(0), 0) == 0 && W5_K1 != FREE_KEY };
    proof_assert! { from_slot(img1[0], 0, 0, Extent { key: W5_K1, size: 4u32, offset: 0u64 }) };
    // checkpoint #1: both writes succeed
    let c1 = em.run_checkpoint(true, true);
    proof_assert! { c1 == Ok(()) && em.seq@ == 1 && em.active@ == 1 && em.dev.sb_seq@ == 1 && em.dev.sb_active@ == 1 };
    proof_assert! { match em.dev.copy1 { Some(c) => c.seq@ == 1 && *c.img == *img1, None => false } };
    proof_assert! { fin_rel(img1[0], em.regions@[0]) && em.regions@[0].slabs == img1[0].slabs };
    // remove_extent(0): K1's slot queued, key FREE
    let pre = snapshot! { em.regions@[0] };
    proof_assert! { forall<k: Int> pre.slabs@.contains(k) ==> k == 0 };
    proof_assert! { rm_case(*pre, 0u64, 0u64, 0usize) };
    let rm = em.regions[0].remove_extent_by_offset(0);
    proof_assert! { rm == Ok(()) && rm_post(*pre, em.regions@[0], 0u64, 0usize) };
    proof_assert! { em.regions@[0].pending_frees@ == Seq::singleton((0u64, 0usize)) };
    proof_assert! { em.regions@[0].slabs@.lookup(0).bitmap == pre.slabs@.lookup(0).bitmap };
    proof_assert! { pending_ok(em.regions@[0]) && regions_ok(em.regions@) && !all_clean(em.regions@) };
    let rec_mid = recover(&em.dev);
    proof_assert! { rec_mid == Ok(img1) };
    // checkpoint #2: copy write succeeds, superblock write fails
    let c2 = em.run_checkpoint(true, false);
    proof_assert! { c2 != Ok(()) && em.seq@ == 2 && em.active@ == 0 && em.dev.sb_seq@ == 1 && em.dev.sb_active@ == 1 };
    proof_assert! { !all_clean(em.regions@) && regions_ok(em.regions@) };
    // checkpoint #3: copy write (to copy 1) succeeds; the superblock write fails / crash
    let c3 = em.run_checkpoint(true, false);
    proof_assert! { c3 != Ok(()) && em.seq@ == 3 && em.active@ == 1 && em.dev.sb_seq@ == 1 && em.dev.sb_active@ == 1 };
    proof_assert! { match em.dev.copy1 { Some(c) => c.seq@ == 3, None => false } };
    let rec_end = recover(&em.dev);
    (c1, c2, c3, img1, rec_mid, rec_end)
}

/// **EM-REMOVE-CRASH-BEFORE-CHECKPOINT** — REFUTED (INDEPENDENT; ROOT CAUSE = CKPT-SB-ORDER).
/// "If an extent was removed after the last successful checkpoint and the system then restarts,
/// initialize() restores that extent at its original offset with its original key and size,
/// and no other recovered extent overlaps it." In `wrc_chain` K1 is removed after the last
/// successful checkpoint #1; two later checkpoints fail at their superblock write (IoError,
/// allowed by the IBlockDevice contract); after a restart recovery (recovery.rs:11-71) finds
/// the superblock's copy 1 overwritten with seq 3 and no valid fallback, so initialize() fails
/// with CorruptMetadata instead of restoring K1.
#[ensures(result.0 == Ok(()) && result.1 == Err(EmError::IoError) && result.2 == Err(EmError::IoError))]
#[ensures(img_lists(*result.3, Extent { key: W5_K1, size: 4u32, offset: 0u64 }))]
#[ensures(match result.4 { Ok(_) => false, Err(_) => true })]
pub fn refute_em_remove_crash_before_checkpoint() -> (Result<(), EmError>, Result<(), EmError>, Result<(), EmError>,
    Snapshot<Seq<RegionState>>, Result<Snapshot<Seq<RegionState>>, EmError>) {
    let (c1, c2, c3, img1, _mid, rec_end) = wrc_chain();
    (c1, c2, c3, img1, rec_end)
}

/// Mutant: claims the restart recovers image #1 (with K1).
#[ensures(result.4 == Ok(result.3))]
pub fn refute_em_remove_crash_before_checkpoint__mutant() -> (Result<(), EmError>, Result<(), EmError>, Result<(), EmError>,
    Snapshot<Seq<RegionState>>, Result<Snapshot<Seq<RegionState>>, EmError>) {
    let (c1, c2, c3, img1, _mid, rec_end) = wrc_chain();
    (c1, c2, c3, img1, rec_end)
}

/// Sequential half of EM-REMOVE-CRASH-BEFORE-CHECKPOINT: a crash right after the removal (no
/// later checkpoint attempt) recovers image #1, which lists K1 at offset 0, size 4.
#[ensures(result.0 == Ok(result.1))]
#[ensures(img_lists(*result.1, Extent { key: W5_K1, size: 4u32, offset: 0u64 }))]
pub fn witness_em_remove_crash_before_checkpoint_sequential() -> (Result<Snapshot<Seq<RegionState>>, EmError>, Snapshot<Seq<RegionState>>) {
    let (_c1, _c2, _c3, img1, mid, _end) = wrc_chain();
    (mid, img1)
}

// =============================================================================
// EM-INV-CHECKPOINT-ROUNDTRIP (refuted: SLAB-SLOTS-U32)
// =============================================================================

/// Batch 1's EM-INV-KEY-VECTOR-LENGTH region (sector 4096, slab_size 4096·(2^32+1), one region
/// of 2^45 bytes) after reserve_extent(k, 4096): ONE slab at 0 whose slot count is
/// (2^32+1) as u32 = 1 (slab.rs:18).
#[ensures(rg_inv(result) && one_key(result, 0))]
#[ensures(result.slabs@.lookup(0).bitmap.num_slots@ == 1)]
#[ensures(result.slabs@.lookup(0).slab_size@ == 17592186048512 && result.slabs@.lookup(0).element_size@ == 4096)]
pub fn w5_kvl() -> RegionState {
    let fp = FormatParams {
        data_disk_size: 35184372088832,
        slab_size: 17592186048512,
        max_extent_size: 4096,
        sector_size: 4096,
        region_count: 1,
        metadata_alignment: 0,
        instance_id: Some(1),
        metadata_disk_ns_id: 1,
        metadata_region_size: 0,
    };
    proof_assert! { fp.slab_size@ % fp.sector_size@ == 0 && fp.max_extent_size@ <= fp.slab_size@ };
    let b = BuddyAllocator::new(0, 35184372088832, 4096);
    proof_assert! { 33.pow2() == 8589934592 && 35184372088832 / 4096 == 8589934592 };
    proof_assert! { b.max_order@ == 33 && b.free_lists@[33]@ == Seq::singleton(0u64) };
    let mut r = RegionState::new(b, fp);
    proof_assert! { span(4096, 33) == 35184372088832 };
    proof_assert! { rg_inv(r) };
    proof_assert! { lemma_ord_eq(4294967297, 0, 33); ord(17592186048512 / 4096) == 33 };
    proof_assert! { first_ne(r.buddy.free_lists@, 33, 33) == 33 };
    proof_assert! { align_l(4096, 4096) == 4096 && slots_of(17592186048512, 4096) == 1 };
    let old = snapshot! { r };
    let res = r.alloc_extent(4096);
    proof_assert! { fresh_case(*old, r, 4096, res) };
    proof_assert! { match res { Ok((d, _, _)) => d@ == 0, Err(_) => false } };
    proof_assert! { r.slabs@.ext_eq(old.slabs@.insert(0, r.slabs@.lookup(0))) };
    r
}

/// **EM-INV-CHECKPOINT-ROUNDTRIP** — REFUTED. ROOT CAUSE = SLAB-SLOTS-U32 (batch 1's root cause
/// of EM-INV-KEY-VECTOR-LENGTH: Slab::new truncates the slot count to u32, slab.rs:18, while
/// format accepts any u64 slab size, lib.rs:383-397; not the buddy mask). "The checkpoint data
/// written contains, for every region, a descriptor for every slab (start offset, slab size,
/// element size, and slot count equal to slab size divided by element size) together with that
/// slab's complete list of slot keys, ..." serialize_region (checkpoint.rs:15-33, mirror
/// model/b5.rs) of the FR-002-valid region `w5_kvl` writes the slab's descriptor with slot count
/// 1 (checkpoint.rs:22/26, a u32 field) and ONE key, while slab size / element size is 2^32+1
/// (the field cannot even hold it). Reachable via format + one reserve_extent, but needs a
/// 16 TiB slab.
#[ensures(result.1.slabs@.lookup(0).slab_size@ / result.1.slabs@.lookup(0).element_size@ == 4294967297)]
#[ensures(result.1.slabs@.lookup(0).bitmap.num_slots == 1u32)]
#[ensures(forall<j: Int> 0 <= j && j < 4 ==> result.0@[24 + j] == le32(1u32, j))]
#[ensures(result.0@.len() == 36)]
pub fn support_old_refute_em_inv_checkpoint_roundtrip() -> (Vec<u8>, RegionState) {
    let r = w5_kvl();
    proof_assert! { exists<k: Int> one_key(r, k) };
    let bytes = serialize_region(&r);
    (bytes, r)
}

/// Mutant: claims the descriptor carries all slab_size / element_size keys.
#[ensures(result.0@.len() == 28 + 8 * 4294967297)]
pub fn support_old_refute_em_inv_checkpoint_roundtrip__mutant() -> (Vec<u8>, RegionState) {
    let r = w5_kvl();
    proof_assert! { exists<k: Int> one_key(r, k) };
    let bytes = serialize_region(&r);
    (bytes, r)
}

// =============================================================================
// Checkpoint timer: EM-SETINT-POST-NONE-DISABLES / EM-SETINT-SOME-FIRES
// =============================================================================

/// **EM-SETINT-POST-NONE-DISABLES** — REFUTED (INDEPENDENT; new root cause
/// TIMER-TIMEOUT-STALE). "After set_checkpoint_interval(None), the background thread performs no
/// automatic checkpoint until an interval is set again." Timer model (model/b5.rs TSys): the
/// thread waits with the 30-tick default (lib.rs:110-112, 128-134); the wait reaches its
/// deadline (timeout) while the caller's set_checkpoint_interval(None) holds the interval
/// mutex (lib.rs:70-73: the notify_one is lost — no thread is blocked in the wait); the thread
/// re-acquires the mutex, `do_checkpoint = result.timed_out()` is true (lib.rs:134-135) and the
/// interval is never re-read: a checkpoint runs AFTER set_checkpoint_interval(None) returned,
/// with no interval set again.
#[ensures(result.0@ == 0 && result.1.ckpts@ == result.0@ + 1 && result.1.interval == None)]
pub fn refute_em_setint_post_none_disables() -> (u64, TSys) {
    let mut t = TSys { interval: Some(30), now: snapshot! { 0 }, ph: TPh::Top, shutdown: false, ckpts: 0 };
    // LEVEL-3 reachable start: the timer state `new` creates (lib.rs:110-116: default interval
    // 30, thread at its loop head, no shutdown, no checkpoint yet). No PROVED invariant of
    // invariants.yaml reads timer state, and no declared range constrains it.
    proof_assert! { t.interval == Some(30u64) && *t.now == 0 && t.ph == TPh::Top && !t.shutdown && t.ckpts@ == 0 };
    t.th_enter();
    t.tick(30);
    t.timeout();
    t.set_interval(None);
    let at_set = t.ckpts;
    t.relock();
    t.work();
    (at_set, t)
}

/// Mutant: claims no checkpoint ran after set_checkpoint_interval(None).
#[ensures(result.1.ckpts@ == result.0@)]
pub fn refute_em_setint_post_none_disables__mutant() -> (u64, TSys) {
    let mut t = TSys { interval: Some(30), now: snapshot! { 0 }, ph: TPh::Top, shutdown: false, ckpts: 0 };
    t.th_enter();
    t.tick(30);
    t.timeout();
    t.set_interval(None);
    let at_set = t.ckpts;
    t.relock();
    t.work();
    (at_set, t)
}

/// **EM-SETINT-SOME-FIRES** — REFUTED (INDEPENDENT; new root cause TIMER-DEADLINE-RESET).
/// "With an interval set, whenever a full interval elapses without the interval being changed,
/// the background thread runs a checkpoint." Timer model (model/b5.rs TSys), interval 30 ticks,
/// the thread starts waiting at 0: (a) a spurious wake-up at 29 (permitted by std Condvar) or
/// (b) set_checkpoint_interval(Some(30)) — the SAME interval — at 29 ends the wait without a
/// timeout; the thread restarts a FULL wait_timeout(30) (lib.rs:128-149 keep no deadline), so
/// at 30 a full unchanged interval has elapsed with no checkpoint and none can start before 59
/// (repeating the wake-up postpones it indefinitely).
#[ensures(*result.0.now == 30 && result.0.interval == Some(30u64) && result.0.ckpts@ == 0)]
#[ensures(match result.0.ph { TPh::Wait(dl) => *dl == 59, _ => false })]
#[ensures(*result.1.now == 30 && result.1.interval == Some(30u64) && result.1.ckpts@ == 0)]
#[ensures(match result.1.ph { TPh::Wait(dl) => *dl == 59, _ => false })]
pub fn refute_em_setint_some_fires() -> (TSys, TSys) {
    let mut a = TSys { interval: Some(30), now: snapshot! { 0 }, ph: TPh::Top, shutdown: false, ckpts: 0 };
    // LEVEL-3 reachable start: both runs start from the timer state `new` creates (lib.rs:110-116).
    proof_assert! { a.interval == Some(30u64) && *a.now == 0 && a.ph == TPh::Top && !a.shutdown && a.ckpts@ == 0 };
    a.th_enter();
    a.tick(29);
    a.spurious();
    a.relock();
    a.th_enter();
    a.tick(1);
    let mut b = TSys { interval: Some(30), now: snapshot! { 0 }, ph: TPh::Top, shutdown: false, ckpts: 0 };
    b.th_enter();
    b.tick(29);
    b.set_interval(Some(30));
    b.relock();
    b.th_enter();
    b.tick(1);
    (a, b)
}

/// Mutant: claims a checkpoint has run by tick 30.
#[ensures(result.0.ckpts@ >= 1)]
pub fn refute_em_setint_some_fires__mutant() -> (TSys, TSys) {
    let mut a = TSys { interval: Some(30), now: snapshot! { 0 }, ph: TPh::Top, shutdown: false, ckpts: 0 };
    a.th_enter();
    a.tick(29);
    a.spurious();
    a.relock();
    a.th_enter();
    a.tick(1);
    let b = TSys { interval: Some(30), now: snapshot! { 0 }, ph: TPh::Top, shutdown: false, ckpts: 0 };
    (a, b)
}

/// The timer thread has seen interval None: it is at the loop head, blocked in `wait(guard)`,
/// woken without a timeout, or gone — never about to run a checkpoint.
#[logic(open)]
pub fn timer_quiet(t: TSys) -> bool {
    pearlite! {
        t.interval == None && match t.ph {
            TPh::Top => true,
            TPh::WaitInf => true,
            TPh::Woken(b) => !b,
            TPh::Exited => true,
            _ => false,
        }
    }
}

/// Positive half of EM-SETINT-POST-NONE-DISABLES: once the thread has re-read the interval
/// after set_checkpoint_interval(None) (any quiet state), NO schedule of thread steps, clock
/// ticks, spurious wake-ups and further set_checkpoint_interval(None) calls (encoded 0-6; a
/// step that is not enabled is skipped) ever runs a checkpoint.
#[requires(timer_quiet(*t))]
#[ensures(timer_quiet(^t) && (^t).ckpts == t.ckpts)]
pub fn witness_em_setint_post_none_disables_after_reread(t: &mut TSys, sched: &Vec<u8>) {
    let t0 = snapshot! { *t };
    let mut p: usize = 0;
    #[invariant(timer_quiet(*t) && t.ckpts == t0.ckpts)]
    while p < sched.len() {
        let code = sched[p];
        if code == 0 {
            match t.ph {
                TPh::Top => t.th_enter(),
                _ => {}
            }
        } else if code == 1 {
            t.tick(1);
        } else if code == 2 {
            // `wait_timeout` reaching its deadline: never enabled while quiet (no timed wait).
            match t.ph {
                TPh::Wait(_) => t.timeout(),
                _ => {}
            }
        } else if code == 3 {
            match t.ph {
                TPh::Wait(_) => t.spurious(),
                TPh::WaitInf => t.spurious(),
                _ => {}
            }
        } else if code == 4 {
            match t.ph {
                TPh::Woken(_) => t.relock(),
                _ => {}
            }
        } else if code == 5 {
            match t.ph {
                TPh::Work => {
                    if t.ckpts < u64::MAX {
                        t.work();
                    }
                }
                _ => {}
            }
        } else if code == 6 {
            t.set_interval(None);
        }
        p += 1;
    }
}
