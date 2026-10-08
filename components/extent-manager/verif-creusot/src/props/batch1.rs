//! Batch 1 — the 23 highest-attachment in-scope properties of `extent-manager`.
//!
//! One scored module per inventory id: `verify_<id>` (proof) or `refute_<id>`
//! (machine-checked negation), each with a `__mutant` twin that must FAIL.
//! Witness chains shared by several refutations live in `witness.rs` (helpers,
//! proved by the whole-crate run, not scored).
use crate::model::bitmap::*;
use crate::model::buddy::*;
use crate::model::component::*;
use crate::model::params::*;
use crate::model::region::*;
use crate::model::slab::*;
use crate::props::witness::*;
use creusot_std::logic::FMap;
use crate::model::assume::*;
use crate::props::l3start::*;
use creusot_std::prelude::*;

// =============================================================================
// Slab / bitmap layer
// =============================================================================

/// **EM-BITMAP-NO-DOUBLE-ALLOC** — "A slot is only ever marked allocated when it is
/// currently free." `AllocationBitmap::set` carries the debug-assert of bitmap.rs:19
/// as `#[requires(!slot_bit(idx))]`; its only two callers — `Slab::alloc_slot`
/// (slab.rs:31, every `reserve_extent`) and `Slab::mark_slot_allocated` (slab.rs:79,
/// every `initialize` via `slab_from_descriptor`) — discharge it (their proofs are
/// part of the crate). This driver runs both call sites and states the property.
#[requires(slab_inv(*s))]
// LEVEL-3 D-B1: the descriptor premise is now the declared range 980f61 for a descriptor of
// the region [base, base+size) (+ the decoder fact t_dec_keys), the region satisfying the
// PROVED invariant em_rg_built (rg_built: base + size <= data_disk_size, a u64); the former
// `keys.len == slots_of` and `start + slab <= u64::MAX` are DERIVED (lemma_desc_slots).
#[requires(a_980f61_d(*desc, base@, size@) && t_dec_keys(*desc) && rg_built(base@, size@, fp))]
#[ensures(match result.0 {
    Some((i, _)) => !slot_bit(s.bitmap, i@) && slot_bit((^s).bitmap, i@),
    None => ^s == *s,
})]
#[ensures(forall<i: Int> 0 <= i && i < result.1.bitmap.num_slots@ ==> slot_bit(result.1.bitmap, i) == (desc.keys@[i] != FREE_KEY))]
pub fn verify_em_bitmap_no_double_alloc(s: &mut Slab, desc: &SlabDescriptor, base: u64, size: u64, fp: FormatParams) -> (Option<(usize, u64)>, Slab) {
    proof_assert! { lemma_desc_slots(*desc, base@, size@); desc.keys@.len() == slots_of(desc.slab_size@, desc.element_size@) };
    proof_assert! { desc.start_offset@ + desc.slab_size@ <= u64::MAX@ };
    let a = s.alloc_slot();
    let b = slab_from_descriptor(desc);
    (a, b)
}

/// Mutant: marks an already-allocated slot allocated — `set`'s precondition fails.
#[requires(slab_inv(*s) && s.bitmap.num_slots@ > 0 && slot_bit(s.bitmap, 0))]
pub fn verify_em_bitmap_no_double_alloc__mutant(s: &mut Slab) {
    s.mark_slot_allocated(0);
}

/// **EM-SLAB-FREE-CLEARS-KEY** — "Freeing a slot marks it free and resets its key to
/// the free marker." Every slot release (abort/drop, publish(FREE_KEY), checkpoint's
/// deferred frees) goes through `RegionState::free_slot` → `Slab::free_slot`
/// (slab.rs:37-40). If the slab survives, the slot is clear with key FREE_KEY; if it
/// became empty it is removed from the region altogether.
#[requires(rg_inv(*r))]
#[requires(free_ok(*r, s, i))]
#[ensures(forall<sl: Slab> r.slabs@.get(s@) == Some(sl) ==>
    ((^r).slabs@.contains(s@) ==>
        !slot_bit((^r).slabs@.lookup(s@).bitmap, i@) && (^r).slabs@.lookup(s@).keys@[i@] == FREE_KEY)
    && (!(^r).slabs@.contains(s@) ==> sl.bitmap.allocated_count@ == 1))]
pub fn verify_em_slab_free_clears_key(r: &mut RegionState, s: u64, i: usize) {
    let old = snapshot! { *r };
    r.free_slot(s, i);
    proof_assert! { forall<sl: Slab> old.slabs@.get(s@) == Some(sl) ==> free_case(*old, *r, s, i, sl) };
}

/// Mutant: claims the key survives the free.
#[requires(rg_inv(*r))]
#[requires(free_ok(*r, s, i))]
#[requires(r.slabs@.contains(s@) && r.slabs@.lookup(s@).bitmap.allocated_count@ > 1)]
#[requires(r.slabs@.lookup(s@).keys@[i@] != FREE_KEY)]
#[ensures((^r).slabs@.lookup(s@).keys@[i@] == r.slabs@.lookup(s@).keys@[i@])]
pub fn verify_em_slab_free_clears_key__mutant(r: &mut RegionState, s: u64, i: usize) {
    r.free_slot(s, i);
}

/// **EM-SLAB-SLOTS-INSIDE** — "Every slot of a slab begins at or after the slab's start
/// and ends at or before the slab's end, and different slots of the same slab do not
/// overlap." Holds for every slab satisfying `slab_inv`, which `Slab::new` establishes
/// and every slab operation preserves (proved in `model::slab`).
// LEVEL-3 D-B1: widened — any two slots i, j of the slab (incl. i == j and the 1-slot slab);
// containment for each, disjointness for every pair of DIFFERENT slots (either order).
#[requires(slab_inv(*s))]
#[requires(i@ < s.bitmap.num_slots@ && j@ < s.bitmap.num_slots@)]
#[ensures(s.start_offset@ <= result.0@ && result.0@ + s.element_size@ <= s.start_offset@ + s.slab_size@)]
#[ensures(s.start_offset@ <= result.1@ && result.1@ + s.element_size@ <= s.start_offset@ + s.slab_size@)]
#[ensures(i@ < j@ ==> result.0@ + s.element_size@ <= result.1@)]
#[ensures(i@ != j@ ==> result.0@ + s.element_size@ <= result.1@ || result.1@ + s.element_size@ <= result.0@)]
pub fn verify_em_slab_slots_inside(s: &Slab, i: usize, j: usize) -> (u64, u64) {
    proof_assert! { lemma_slot_inside(*s, i@); slot_off(*s, i@) + s.element_size@ <= s.start_offset@ + s.slab_size@ };
    proof_assert! { lemma_slot_inside(*s, j@); slot_off(*s, j@) + s.element_size@ <= s.start_offset@ + s.slab_size@ };
    proof_assert! { i@ < j@ ==> { lemma_slots_disjoint(*s, i@, j@); slot_off(*s, i@) + s.element_size@ <= slot_off(*s, j@) } };
    proof_assert! { j@ < i@ ==> { lemma_slots_disjoint(*s, j@, i@); slot_off(*s, j@) + s.element_size@ <= slot_off(*s, i@) } };
    (s.slot_offset(i), s.slot_offset(j))
}

/// Mutant: claims the later slot ends before the earlier one starts.
#[requires(slab_inv(*s))]
#[requires(i@ < j@ && j@ < s.bitmap.num_slots@)]
#[ensures(result.1@ + s.element_size@ <= result.0@)]
pub fn verify_em_slab_slots_inside__mutant(s: &Slab, i: usize, j: usize) -> (u64, u64) {
    proof_assert! { lemma_slot_inside(*s, j@); true };
    (s.slot_offset(i), s.slot_offset(j))
}

/// **EM-INV-KEY-VECTOR-LENGTH** — REFUTED.
///
/// Statement: "Every slab keeps exactly one key entry per slot, so the number of key
/// entries equals the slab size divided by the element size."
///
/// Root cause: `Slab::new` computes `(slab_size / element_size as u64) as u32`
/// (slab.rs:18), silently truncating the slot count to 32 bits, while `format()`
/// accepts any `u64` slab size (lib.rs:384-401 only require `slab_size % sector_size == 0`
/// and `max_extent_size <= slab_size`).
///
/// Witness (all FR-002 checks pass): sector 4096, slab_size = 4096·(2^32+1),
/// max_extent_size 4096, one region of 2^45 bytes (data_disk_size 2^45,
/// metadata_region_size 0). `reserve_extent(k, 4096)` carves a fresh slab at offset 0
/// whose key vector has ONE entry although `slab_size / element_size = 2^32 + 1`.
#[ensures(result.0@ == 1 && result.1@ == 4294967297)]
pub fn support_old_refute_em_inv_key_vector_length() -> (usize, u64) {
    let (r, keys_len) = w_kvl_reserve();
    let _ = r;
    (keys_len, 17592186048512u64 / 4096u64)
}

/// Mutant: claims the key vector does have `slab_size / element_size` entries.
#[ensures(result@ == 17592186048512u64@ / 4096)]
pub fn support_old_refute_em_inv_key_vector_length__mutant() -> usize {
    let (_r, keys_len) = w_kvl_reserve();
    keys_len
}

// =============================================================================
// Buddy layer — the non-power-of-two-sector recovery defect
// =============================================================================
//
// Shared root cause of the four refutations below: `BuddyAllocator::mark_allocated`
// (buddy.rs:117-157), which `initialize()` calls for every recovered slab
// (lib.rs:553-555), locates the enclosing free block with `offset & !(block_span - 1)`
// (buddy.rs:136). That mask is the aligned block start ONLY when `block_span` is a power
// of two, i.e. only for power-of-two sector sizes; `format()` accepts any non-zero
// `sector_size` dividing `slab_size` (FR-002, lib.rs:384-391). For a non-power-of-two
// sector size the lookup misses, and the "silently ignore" fall-through (buddy.rs:157)
// leaves the recovered slab's space on the free lists.
//
// Witness W1 (FR-002-valid parameters): sector_size 3, slab_size 3, max_extent_size 3,
// region_count 1, data_disk_size 6, metadata_region_size 0 → one region [0, 6).
//   reserve(K1,3) → slab A at 0;  reserve(K2,3) → slab B at 3;  abort(K1) → A freed;
//   publish(K2);  checkpoint();  initialize()  → B recovered, mark_allocated(3, 3) no-op.

/// **EM-REGION-SLABS-NOT-FREE** — REFUTED. "The space occupied by a slab is never
/// simultaneously recorded as free in the region's allocator." After W1's
/// `initialize()`, recovered slab B occupies [3, 6) while the free list records the
/// order-1 block [0, 6). Root cause: buddy.rs:136 mask is wrong for a non-power-of-two
/// sector size, so mark_allocated (lib.rs:554) silently fails to reserve the slab.
#[ensures(result.0@ == 3)]
#[ensures(result.1.free_lists@[1]@ == Seq::singleton(0u64))]
#[ensures(result.0@ - result.1.base_offset@ >= result.1.free_lists@[1]@[0]@
    && result.0@ - result.1.base_offset@ + 3 <= result.1.free_lists@[1]@[0]@ + span(3, 1))]
pub fn support_old_refute_em_region_slabs_not_free() -> (u64, BuddyAllocator) {
    w1_recovered()
}

/// Mutant: claims the recovered slab's block is no longer free.
#[ensures(result.1.free_lists@[1]@.len() == 0)]
pub fn support_old_refute_em_region_slabs_not_free__mutant() -> (u64, BuddyAllocator) {
    w1_recovered()
}

/// **EM-INV-NO-OVERLAP** — REFUTED. "No two slots in use at the same time share any
/// byte; in particular a newly reserved extent overlaps no existing extent." After W1's
/// `initialize()`, the next two slab-sized reservations are carved at 0 and then at 3 —
/// the offset of the recovered, published extent K2. Root cause as above.
#[ensures(result.0 == result.2 && result.0@ == 3)]
pub fn support_old_refute_em_inv_no_overlap() -> (u64, u64, u64) {
    let (live, mut b) = w1_recovered();
    let z = b.alloc(3);
    let w = b.alloc(3);
    match (z, w) {
        (Some(z), Some(w)) => (live, z, w),
        _ => (live, 0, 1),
    }
}

/// Mutant: claims the new reservation does not land on the live extent.
#[ensures(result.0 != result.2)]
pub fn support_old_refute_em_inv_no_overlap__mutant() -> (u64, u64, u64) {
    let (live, mut b) = w1_recovered();
    let z = b.alloc(3);
    let w = b.alloc(3);
    match (z, w) {
        (Some(z), Some(w)) => (live, z, w),
        _ => (live, 0, 1),
    }
}

/// **EM-BUDDY-FREE-DISJOINT** — REFUTED. "Free blocks recorded by a region allocator
/// never overlap each other." After W1's `initialize()`, `remove_extent(3)` +
/// `checkpoint()` empties slab B and `free_slot` returns it with `buddy.free(3, 3)`
/// (region.rs:104): order-0 block [3, 6) is pushed while order-1 block [0, 6) is still
/// free. Root cause as above (mark_allocated never removed [0, 6)).
#[ensures(result.free_lists@[0]@ == Seq::singleton(3u64))]
#[ensures(result.free_lists@[1]@ == Seq::singleton(0u64))]
#[ensures(3 < 0 + span(3, 1) && 0 < 3 + span(3, 0))]
pub fn support_old_refute_em_buddy_free_disjoint() -> BuddyAllocator {
    w1_freed()
}

/// Mutant: claims the two free blocks are disjoint.
#[ensures(result.free_lists@[1]@.len() == 0 || result.free_lists@[0]@.len() == 0)]
pub fn support_old_refute_em_buddy_free_disjoint__mutant() -> BuddyAllocator {
    w1_freed()
}

/// **EM-CKPT-POST-EMPTY-SLAB-RETURNED** — REFUTED. "When freeing a slot leaves a slab
/// with no occupied slots, that slab is removed and its whole space returned to its
/// region's free space, so used_bytes() decreases by that slab's footprint." In W1,
/// before the free `used_bytes()` is 0 (6 − 6); afterwards the free lists hold 9 bytes
/// for a 6-byte region, so `total_usable_size - total_free()` (lib.rs:704) underflows
/// (debug panic; release wrap to 2^64−3) instead of decreasing by 3. Root cause as above.
#[ensures(result.0@ == 6 && result.1@ == 6 && result.2@ == 9)]
pub fn refute_em_ckpt_post_empty_slab_returned() -> (u64, u64, u64) {
    let (_live, b) = w1_recovered();
    proof_assert! { lemma_tf2(b.free_lists@, 3); tf(b.free_lists@, 3, 2) == 6 };
    let before = b.total_free();
    let a = w1_freed();
    proof_assert! { lemma_tf2(a.free_lists@, 3); tf(a.free_lists@, 3, 2) == 9 };
    let after = a.total_free();
    (before, a.total_usable_size, after)
}

/// Mutant: claims used bytes did decrease (free space did not exceed capacity).
#[ensures(result.2@ <= result.1@)]
pub fn refute_em_ckpt_post_empty_slab_returned__mutant() -> (u64, u64, u64) {
    let (_live, b) = w1_recovered();
    proof_assert! { lemma_tf2(b.free_lists@, 3); tf(b.free_lists@, 3, 2) == 6 };
    let before = b.total_free();
    let a = w1_freed();
    proof_assert! { lemma_tf2(a.free_lists@, 3); tf(a.free_lists@, 3, 2) == 9 };
    let after = a.total_free();
    (before, a.total_usable_size, after)
}

// =============================================================================
// Region-level maintained invariants (proved over every region mutator)
// =============================================================================

/// One region mutation, as reached from the public API:
/// `Reserve` = reserve_extent → alloc_extent; `Free` = WriteHandle::abort / drop /
/// publish(FREE_KEY) → free_slot; `Publish` = WriteHandle::publish → publish_slot;
/// `Remove` = remove_extent → remove_extent_by_offset; `Flush` = checkpoint() →
/// flush_pending_frees (lib.rs:318).
pub enum ROp {
    Reserve(u32),
    Free(u64, usize),
    Publish(u64, usize, u64),
    Remove(u64),
    Flush,
}

/// The documented-use precondition of each mutation (no panic in the source).
#[logic(open)]
pub fn rop_pre(r: RegionState, op: ROp) -> bool {
    pearlite! {
        match op {
            ROp::Reserve(sz) => a_231ab0(sz@) && a_3783f8(sz@, r.format_params.sector_size@),
            ROp::Free(s, i) => free_ok(r, s, i),
            ROp::Publish(s, i, _) => match r.slabs@.get(s@) { Some(sl) => i@ < sl.keys@.len(), None => true },
            ROp::Remove(_) => true,
            ROp::Flush => pending_ok(r),
        }
    }
}

/// Apply one mutation; the region invariant and geometry frame are preserved.
#[requires(rg_inv(*r) && rop_pre(*r, op))]
#[ensures(rg_inv(^r) && rg_frame(*r, ^r))]
pub fn apply_rop(r: &mut RegionState, op: ROp) {
    match op {
        ROp::Reserve(sz) => {
            let _ = r.alloc_extent(sz);
        }
        ROp::Free(s, i) => r.free_slot(s, i),
        ROp::Publish(s, i, k) => r.publish_slot(s, i, k),
        ROp::Remove(off) => {
            let _ = r.remove_extent_by_offset(off);
        }
        ROp::Flush => r.flush_pending_frees(),
    }
}

/// Every free block and every slab's block is aligned to its own size, relative to the
/// region base.
#[logic(open)]
pub fn all_aligned(r: RegionState) -> bool {
    pearlite! {
        bd_aligned(r.buddy)
        && forall<k: Int> r.slabs@.contains(k) ==> (k - r.buddy.base_offset@) % span(r.buddy.sector_size@, slab_k(r)) == 0
    }
}

/// **EM-BUDDY-ALLOC-ALIGNED** — "Every free or allocated block of 2^k sectors starts,
/// relative to the region base, at a multiple of its own size." Established by
/// `format` (BuddyAllocator::new, buddy.rs:10-46) and `initialize` (rebuild with
/// mark_allocated, lib.rs:552-565), and preserved by every region mutation.
/// Uses the trusted arithmetic fact F of [`buddy_xor`] (the `free` merge step).
#[requires(rg_inv(*r) && rop_pre(*r, op))]
#[requires(rg_in(base@, size@, fp))]
#[requires(forall<p: Int> 0 <= p && p < descs@.len() ==> desc_ok_a(descs@[p], base@, size@, fp))]
#[ensures(all_aligned(^r))]
#[ensures(all_aligned(result.0) && all_aligned(result.1))]
pub fn verify_em_buddy_alloc_aligned(
    r: &mut RegionState,
    op: ROp,
    base: u64,
    size: u64,
    fp: FormatParams,
    descs: &Vec<SlabDescriptor>,
) -> (RegionState, RegionState) {
    proof_assert! { lemma_rg_in(base@, size@, fp); rg_hd(base@, size@, fp) };
    proof_assert! { forall<p: Int> 0 <= p && p < descs@.len() ==> { lemma_desc_ok_a(descs@[p], base@, size@, fp); desc_ok(descs@[p], base@, size@, fp) } };
    apply_rop(r, op);
    // format(): a fresh region (lib.rs:465-466)
    let fresh = RegionState::new(BuddyAllocator::new(base, size, fp.sector_size), fp);
    // initialize(): the rebuilt region (lib.rs:552-565)
    let rebuilt = rebuild_region(base, size, fp, descs);
    (fresh, rebuilt)
}

/// Mutant: claims every free block is aligned to TWICE its size.
#[requires(rg_inv(*r) && rop_pre(*r, op))]
#[ensures(forall<o: Int, k: Int> 0 <= o && o < (^r).buddy.free_lists@.len() && 0 <= k && k < (^r).buddy.free_lists@[o]@.len() ==>
    (^r).buddy.free_lists@[o]@[k]@ % span((^r).buddy.sector_size@, o + 1) == 0)]
pub fn verify_em_buddy_alloc_aligned__mutant(r: &mut RegionState, op: ROp) {
    apply_rop(r, op);
}

/// Two distinct aligned blocks of the same size `m` are disjoint.
#[logic]
#[requires(m > 0 && x >= 0 && y >= 0 && x % m == 0 && y % m == 0 && x < y)]
#[ensures(x + m <= y)]
pub fn lemma_aligned_disjoint(x: Int, y: Int, m: Int) {
    lemma_divmod(x, m);
    lemma_divmod(y, m);
    lemma_mul_le(x / m + 1, y / m, m);
    lemma_distrib(x / m, 1, m)
}

/// Slabs of a region lie inside the region and are pairwise disjoint.
#[logic(open)]
pub fn slabs_within(r: RegionState) -> bool {
    pearlite! {
        (forall<k: Int> r.slabs@.contains(k) ==>
            r.buddy.base_offset@ <= k && k + r.slabs@.lookup(k).slab_size@ <= r.buddy.base_offset@ + r.buddy.total_usable_size@)
        && (forall<k1: Int, k2: Int> r.slabs@.contains(k1) && r.slabs@.contains(k2) && k1 < k2 ==>
            k1 + r.slabs@.lookup(k1).slab_size@ <= k2)
    }
}

#[logic]
#[requires(rg_inv(r))]
#[ensures(slabs_within(r))]
pub fn lemma_slabs_within(r: RegionState) {
    pearlite! {
        lemma_slab_fits(r);
        lemma_ord_ge(r.format_params.slab_size@ / r.format_params.sector_size@);
        lemma_span_pos(r.buddy.sector_size@, slab_k(r));
        proof_assert! { forall<k1: Int, k2: Int> r.slabs@.contains(k1) && r.slabs@.contains(k2) && k1 < k2 ==> {
            lemma_aligned_disjoint(k1 - r.buddy.base_offset@, k2 - r.buddy.base_offset@, span(r.buddy.sector_size@, slab_k(r)));
            k1 + span(r.buddy.sector_size@, slab_k(r)) <= k2 } }
    }
}

/// **EM-INV-SLAB-WITHIN-REGION** — "Every slab occupies one slab size of contiguous
/// bytes that lie entirely inside its own region's byte range, and the slabs of a region
/// do not overlap one another." Follows from the region invariant (each slab sits on a
/// well-formed, aligned, in-range order-K buddy block; slab keys are distinct), which
/// every mutation and both constructors maintain.
#[requires(rg_inv(*r) && rop_pre(*r, op))]
#[requires(rg_in(base@, size@, fp))]
#[requires(forall<p: Int> 0 <= p && p < descs@.len() ==> desc_ok_a(descs@[p], base@, size@, fp))]
#[ensures(slabs_within(^r))]
#[ensures(slabs_within(result))]
pub fn verify_em_inv_slab_within_region(
    r: &mut RegionState,
    op: ROp,
    base: u64,
    size: u64,
    fp: FormatParams,
    descs: &Vec<SlabDescriptor>,
) -> RegionState {
    proof_assert! { lemma_rg_in(base@, size@, fp); rg_hd(base@, size@, fp) };
    proof_assert! { forall<p: Int> 0 <= p && p < descs@.len() ==> { lemma_desc_ok_a(descs@[p], base@, size@, fp); desc_ok(descs@[p], base@, size@, fp) } };
    apply_rop(r, op);
    proof_assert! { lemma_slabs_within(*r); slabs_within(*r) };
    let rebuilt = rebuild_region(base, size, fp, descs);
    proof_assert! { lemma_slabs_within(rebuilt); slabs_within(rebuilt) };
    rebuilt
}

/// Mutant: claims a slab may extend past the region end by one byte... i.e. that the
/// region has room for one more byte after every slab (false for a slab at the end).
#[requires(rg_inv(*r) && rop_pre(*r, op))]
#[ensures(forall<k: Int> (^r).slabs@.contains(k) ==>
    k + (^r).slabs@.lookup(k).slab_size@ < (^r).buddy.base_offset@)]
pub fn verify_em_inv_slab_within_region__mutant(r: &mut RegionState, op: ROp) {
    apply_rop(r, op);
}

// =============================================================================
// Component level (batch 1c): one public-API step, and the lifecycle
// =============================================================================

/// One public-API call between two lifecycle events (format / initialize):
/// `reserve_extent`, `WriteHandle::publish`, `WriteHandle::abort`/`drop`,
/// `remove_extent`, `checkpoint`.
pub enum COp {
    Reserve(u64, u32),
    Publish(Handle),
    Abort(Handle),
    Remove(u64),
    Checkpoint,
}

/// The component's maintained structural invariant (region indices + region invariant).
#[logic(open)]
pub fn em_ok(em: ExtentManager) -> bool {
    pearlite! { em_regions_wf(em) && em_regions_ok(em) }
}

/// The documented-use precondition of each call (what keeps the mirror panic-free).
#[logic(open)]
pub fn cop_pre(em: ExtentManager, op: COp) -> bool {
    pearlite! {
        match op {
            COp::Reserve(_, sz) => a_231ab0(sz@) && rsv_cur_ok(em, sz@),
            COp::Publish(h) => h.region@ < em.arena@.len() && rg_inv(em.arena@[h.region@])
                && (h.key == FREE_KEY ==> free_ok(em.arena@[h.region@], h.slab_start, h.slot_idx))
                && match em.arena@[h.region@].slabs@.get(h.slab_start@) { Some(sl) => h.slot_idx@ < sl.keys@.len(), None => true },
            COp::Abort(h) => h.region@ < em.arena@.len() && rg_inv(em.arena@[h.region@])
                && free_ok(em.arena@[h.region@], h.slab_start, h.slot_idx),
            COp::Remove(_) => true,
            COp::Checkpoint => em_together(em)
                && match em.regions { Some(rv) => forall<i: Int> 0 <= i && i < rv@.len() ==> pending_ok(em.arena@[rv@[i]@]), None => true },
        }
    }
}

/// The shared layout fields no non-lifecycle call changes.
#[logic(open)]
pub fn shared_frame(a: Option<SharedState>, b: Option<SharedState>) -> bool {
    pearlite! {
        match (a, b) {
            (Some(x), Some(y)) => y.superblock.instance_id == x.superblock.instance_id
                && y.superblock.data_start_offset == x.superblock.data_start_offset
                && y.superblock.data_disk_size == x.superblock.data_disk_size
                && y.format_params == x.format_params,
            (None, None) => true,
            _ => false,
        }
    }
}

/// Apply one call; the structural invariant, the region geometry, the published
/// regions and the shared layout are preserved.
#[requires(em_ok(*em) && cop_pre(*em, op))]
#[ensures(em_ok(^em))]
#[ensures((^em).regions == em.regions && (^em).arena@.len() == em.arena@.len())]
#[ensures(forall<i: Int> 0 <= i && i < em.arena@.len() ==> rg_frame(em.arena@[i], (^em).arena@[i]))]
#[ensures((^em).metadata_base_lba == em.metadata_base_lba && (^em).data_base_lba == em.data_base_lba)]
#[ensures(shared_frame(em.shared, (^em).shared))]
#[ensures(seq_agree(*em) ==> seq_agree(^em))]
#[ensures(match (em.shared, (^em).shared) { (Some(a), Some(b)) => sb_frame(a.superblock, b.superblock), _ => true })]
pub fn apply_cop(em: &mut ExtentManager, op: COp) {
    match op {
        COp::Reserve(k, sz) => {
            let _ = em.reserve_extent(k, sz);
        }
        COp::Publish(h) => {
            let _ = em.publish(h);
        }
        COp::Abort(h) => em.abort(h),
        COp::Remove(off) => {
            let _ = em.remove_extent(off);
        }
        COp::Checkpoint => {
            let _ = em.checkpoint();
        }
    }
}

/// **EM-CAP-CONSTANT** — "Between one successful format() or initialize() and the next,
/// capacity_bytes() returns the same value no matter which reservations, publishes,
/// aborts, removals or checkpoints occur." capacity_bytes (lib.rs:710-715) sums each
/// current region's `total_usable_size`, fixed at BuddyAllocator::new (buddy.rs:10-46);
/// every call in COp preserves the region set and each region's geometry (rg_frame).
#[requires(em_ok(*em) && cop_pre(*em, op))]
#[requires(crate::props::l3start::em_cap_ok(*em))] // LEVEL-3 D-B1: the PROVED invariant em_cap_ok (inv_em_cap_ok)
#[ensures(result.0 == result.1)]
pub fn verify_em_cap_constant(em: &mut ExtentManager, op: COp) -> (u64, u64) {
    let c0 = em.capacity_bytes();
    let old = snapshot! { *em };
    apply_cop(em, op);
    proof_assert! { match em.regions {
        Some(rv) => {
            lemma_cap_frame(old.arena@, em.arena@, rv@, rv@.len());
            cap_sum(old.arena@, rv@, rv@.len()) == cap_sum(em.arena@, rv@, rv@.len())
        },
        None => true,
    } };
    let c1 = em.capacity_bytes();
    (c0, c1)
}

/// Mutant: claims a call grows the capacity by one byte.
#[requires(em_ok(*em) && cop_pre(*em, op))]
#[requires(match em.regions { Some(rv) => cap_sum(em.arena@, rv@, rv@.len()) <= u64::MAX@, None => true })]
#[ensures(result.0@ + 1 == result.1@)]
pub fn verify_em_cap_constant__mutant(em: &mut ExtentManager, op: COp) -> (u64, u64) {
    let c0 = em.capacity_bytes();
    let old = snapshot! { *em };
    apply_cop(em, op);
    proof_assert! { match em.regions {
        Some(rv) => {
            lemma_cap_frame(old.arena@, em.arena@, rv@, rv@.len());
            cap_sum(old.arena@, rv@, rv@.len()) == cap_sum(em.arena@, rv@, rv@.len())
        },
        None => true,
    } };
    let c1 = em.capacity_bytes();
    (c0, c1)
}

/// **EM-INSTID-POST-FROM-SUPERBLOCK-STABLE** — "Once the component is formatted or
/// initialized, get_instance_id() returns success with the instance identifier stored in
/// the superblock, and that value does not change except through format() or
/// initialize(); reservations, publishes, removals and checkpoints leave it unchanged."
/// (a) a successful format publishes a SharedState whose superblock was written to the
/// metadata device (dev.sb), and get_instance_id returns its instance_id (= the caller's
/// id when one was given); (b) the same for initialize, returning the recovered
/// superblock's id; (c) every COp call leaves get_instance_id's result unchanged
/// (checkpoint rewrites the superblock with the same instance_id, checkpoint.rs:106-112).
#[requires(em_ok(*em) && cop_pre(*em, op))]
#[requires(em_regions_wf(*f))]
#[requires(f.dev.connected ==> a_67ea4f(f.dev.num_sectors@, f.dev.sector_size@))]
#[requires(a_2ee144(params.metadata_alignment@))]
#[requires(sane_params(params))]
#[requires(em_regions_wf(*g) && sb_sane(sb) && recovered_ok(sb, per_region@))]
#[ensures(result.0 == result.1)]
#[ensures(match result.2 {
    Ok(()) => match (^f).shared {
        Some(sh) => result.3 == Ok(sh.superblock.instance_id) && (^f).dev.sb == Some(sh.superblock)
            && match params.instance_id { Some(id) => result.3 == Ok(id), None => true },
        None => false,
    },
    Err(_) => true,
})]
#[ensures(result.4 == Ok(sb.instance_id))]
pub fn verify_em_instid_post_from_superblock_stable(
    em: &mut ExtentManager,
    op: COp,
    f: &mut ExtentManager,
    params: FormatParams,
    g: &mut ExtentManager,
    sb: Superblock,
    per_region: &Vec<Vec<SlabDescriptor>>,
) -> (Result<u64, EmError>, Result<u64, EmError>, Result<(), EmError>, Result<u64, EmError>, Result<u64, EmError>) {
    let a = em.get_instance_id();
    apply_cop(em, op);
    let b = em.get_instance_id();
    let r = f.format(params);
    let c = f.get_instance_id();
    g.initialize(sb, per_region);
    let d = g.get_instance_id();
    (a, b, r, c, d)
}

/// Mutant: claims a call changes the instance id.
#[requires(em_ok(*em) && cop_pre(*em, op))]
#[ensures(result.0 != result.1)]
pub fn verify_em_instid_post_from_superblock_stable__mutant(
    em: &mut ExtentManager,
    op: COp,
) -> (Result<u64, EmError>, Result<u64, EmError>) {
    let a = em.get_instance_id();
    apply_cop(em, op);
    let b = em.get_instance_id();
    (a, b)
}

/// **EM-SETDBASE-FRAME** — "set_data_base_lba() does not change the extent offsets the
/// component reports ..., does not change the metadata base address, and does not change
/// where metadata-device I/O is addressed." set_data_base_lba (lib.rs:721-723) writes only
/// its own mutex; every other field — the regions (hence every offset reserve_extent,
/// publish, get_extents, for_each_extent report: none of them reads data_base_lba),
/// shared, metadata_base_lba, metadata_ns_id and the device — is unchanged, and
/// data_base_lba() then returns the new value.
#[ensures((^em).arena == em.arena && (^em).regions == em.regions && (^em).shared == em.shared)]
#[ensures((^em).metadata_base_lba == em.metadata_base_lba && (^em).metadata_ns_id == em.metadata_ns_id)]
#[ensures((^em).dev == em.dev)]
#[ensures(result == lba)]
pub fn verify_em_setdbase_frame(em: &mut ExtentManager, lba: u64) -> u64 {
    em.set_data_base_lba(lba);
    em.data_base_lba()
}

/// Mutant: claims set_data_base_lba also moves the metadata base address.
#[ensures((^em).metadata_base_lba == lba)]
pub fn verify_em_setdbase_frame__mutant(em: &mut ExtentManager, lba: u64) -> u64 {
    em.set_data_base_lba(lba);
    em.data_base_lba()
}

/// Release through a stale handle (WriteHandle::publish / abort / drop).
pub enum HOp {
    Publish(Handle),
    Abort(Handle),
}

#[logic(open)]
pub fn hop_pre(em: ExtentManager, op: HOp) -> bool {
    pearlite! {
        match op {
            HOp::Publish(h) => cop_pre(em, COp::Publish(h)),
            HOp::Abort(h) => cop_pre(em, COp::Abort(h)),
        }
    }
}

#[logic(open)]
pub fn hop_region(op: HOp) -> Int {
    pearlite! { match op { HOp::Publish(h) => h.region@, HOp::Abort(h) => h.region@ } }
}

/// The handle closures act on the region object they captured and on nothing else.
#[requires(hop_pre(*em, op))]
#[ensures((^em).arena@.len() == em.arena@.len())]
#[ensures(forall<i: Int> 0 <= i && i < em.arena@.len() && i != hop_region(op) ==> (^em).arena@[i] == em.arena@[i])]
#[ensures((^em).regions == em.regions && (^em).shared == em.shared && (^em).dev == em.dev)]
pub fn apply_hop(em: &mut ExtentManager, op: HOp) {
    match op {
        HOp::Publish(h) => {
            let _ = em.publish(h);
        }
        HOp::Abort(h) => em.abort(h),
    }
}

/// **EM-FORMAT-FRAME-STALE-HANDLES** — "Publishing, aborting or dropping a write handle
/// that was reserved before a successful format() (or initialize()) has no effect on the
/// extents, used bytes or free space of the new layout." The handle's closures captured
/// the OLD `Arc<RwLock<RegionState>>` (lib.rs:594-595); format/initialize publish freshly
/// built region objects (lib.rs:457-468, 538-568, 506, 576). Proved: after format (resp.
/// initialize) the new regions are arena entries created by it, every old entry is
/// untouched (so the stale handle's preconditions still hold), and the handle's
/// publish/abort leaves every new region's state — slabs, key vectors, buddy free lists,
/// i.e. extents, used and free bytes — exactly as it was.
#[requires(em_regions_wf(*em) && hop_pre(*em, op))]
#[requires(em.dev.connected ==> a_67ea4f(em.dev.num_sectors@, em.dev.sector_size@))]
#[requires(a_2ee144(params.metadata_alignment@))]
#[requires(sane_params(params))]
#[requires(sb_sane(sb) && recovered_ok(sb, per_region@))]
#[ensures(match result.0 {
    Ok(()) => match (^em).regions {
        Some(rv) => (forall<i: Int> 0 <= i && i < rv@.len() ==> em.arena@.len() <= rv@[i]@)
            && (forall<i: Int> 0 <= i && i < rv@.len() ==> (^em).arena@[rv@[i]@] == result.1.arena@[rv@[i]@])
            && (^em).regions == result.1.regions,
        None => false,
    },
    Err(_) => true,
})]
pub fn verify_em_format_frame_stale_handles(
    em: &mut ExtentManager,
    op: HOp,
    params: FormatParams,
    reinit: bool,
    sb: Superblock,
    per_region: &Vec<Vec<SlabDescriptor>>,
) -> (Result<(), EmError>, Snapshot<ExtentManager>) {
    let old_len = snapshot! { em.arena@.len() };
    if reinit {
        em.initialize(sb, per_region);
    } else {
        match em.format(params) {
            Ok(()) => {}
            Err(e) => return (Err(e), snapshot! { *em }),
        }
    }
    let mid = snapshot! { *em };
    proof_assert! { hop_pre(*em, op) };
    apply_hop(em, op);
    proof_assert! { match em.regions {
        Some(rv) => forall<i: Int> 0 <= i && i < rv@.len() ==> *old_len <= rv@[i]@ && rv@[i]@ < mid.arena@.len()
            && em.arena@[rv@[i]@] == mid.arena@[rv@[i]@],
        None => false,
    } };
    (Ok(()), mid)
}

/// Mutant: claims the stale handle's release DOES land in a new region (its region
/// index is one of the new layout's).
#[requires(em_regions_wf(*em) && hop_pre(*em, op))]
#[requires(em.dev.connected ==> a_67ea4f(em.dev.num_sectors@, em.dev.sector_size@))]
#[requires(a_2ee144(params.metadata_alignment@))]
#[requires(sane_params(params))]
#[ensures(match result {
    Ok(()) => match (^em).regions { Some(rv) => exists<i: Int> 0 <= i && i < rv@.len() && rv@[i]@ == hop_region(op), None => true },
    Err(_) => true,
})]
pub fn verify_em_format_frame_stale_handles__mutant(em: &mut ExtentManager, op: HOp, params: FormatParams) -> Result<(), EmError> {
    em.format(params)?;
    apply_hop(em, op);
    Ok(())
}

/// **EM-INV-INITIALIZED-TOGETHER** — REFUTED. "The component is either fully initialized
/// (regions and shared layout state both present) or not initialized at all; no operation
/// observes one without the other."
///
/// Root cause: `format` publishes the regions and the shared state under two different
/// locks in two separate statements (lib.rs:506 `*self.regions.write() = ...`, lib.rs:507
/// `*self.shared.lock().unwrap() = ...`; initialize likewise at lib.rs:576-577), so a
/// concurrent caller (FR-024: the component is safe for concurrent use) that runs between
/// them sees regions present and shared absent.
///
/// Witness: a fresh, never-formatted component (regions None, shared None) with a
/// connected metadata device; `format` with valid parameters (fmt_ok_inputs). Unless the
/// device fails the superblock write (IoError — the only failure left on these inputs),
/// format reaches lib.rs:506; at that point a concurrent `get_instance_id` returns
/// NotInitialized (lib.rs:686-692) while `reserve_extent`'s `region_for_key` succeeds
/// (lib.rs:211-221) — the same instant is "initialized" to one operation and "not
/// initialized" to another.
#[requires(em.regions == None && em.shared == None && em_regions_wf(*em))]
#[requires(fmt_ok_inputs(*em, params))]
#[requires(a_67ea4f(em.dev.num_sectors@, em.dev.sector_size@))]
#[requires(sane_params(params))]
#[requires(l3_em_fmt_inputs(*em, params))]
#[ensures(match result {
    Ok((reg, inst)) => !em_together(^em) && (^em).regions != None && (^em).shared == None
        && inst == Err(EmError::NotInitialized) && match reg { Ok(_) => true, Err(_) => false },
    Err(e) => e == EmError::IoError,
})]
pub fn refute_em_inv_initialized_together(
    em: &mut ExtentManager,
    params: FormatParams,
    key: u64,
) -> Result<(Result<usize, EmError>, Result<u64, EmError>), EmError> {
    // LEVEL-3 reachable start: the `new` state (regions / shared None, em_regions_wf — the
    // proved invariants em_layout / seq_agree / em_regions_ok hold vacuously) and every
    // format-start range on the inputs (#[requires] l3_em_fmt_inputs; the mutant below keeps
    // failing under the same requires, so it is not vacuous).
    proof_assert! { em_regions_wf(*em) && em_layout(*em) && seq_agree(*em) && em_regions_ok(*em) };
    let (rv, _sh) = em.format_build(params)?;
    em.publish_regions(rv); // lib.rs:506 — the window opens here
    let reg = em.region_for_key(key); // concurrent reserve_extent (lib.rs:585)
    let inst = em.get_instance_id(); // concurrent get_instance_id (lib.rs:686)
    Ok((reg, inst))
}

/// Mutant: claims the two halves are always published together.
#[requires(em.regions == None && em.shared == None && em_regions_wf(*em))]
#[requires(fmt_ok_inputs(*em, params))]
#[requires(a_67ea4f(em.dev.num_sectors@, em.dev.sector_size@))]
#[requires(sane_params(params))]
#[requires(l3_em_fmt_inputs(*em, params))]
#[ensures(match result { Ok(_) => em_together(^em), Err(_) => true })]
pub fn refute_em_inv_initialized_together__mutant(
    em: &mut ExtentManager,
    params: FormatParams,
    key: u64,
) -> Result<(Result<usize, EmError>, Result<u64, EmError>), EmError> {
    let (rv, _sh) = em.format_build(params)?;
    em.publish_regions(rv);
    let reg = em.region_for_key(key);
    let inst = em.get_instance_id();
    Ok((reg, inst))
}

/// **EM-BITMAP-COUNT-MATCHES** — "A slab's count of allocated slots always equals the
/// number of slots marked allocated in its bitmap and is at most the number of slots."
/// `bm_count_ok` is part of `slab_inv`, hence of `rg_inv`: established by Slab::new and
/// slab_from_descriptor (format, initialize) and preserved by every region mutation the
/// public API reaches (ROp), under debug-build semantics (the bitmap.rs:18,19,27,28
/// debug_asserts are preconditions).
#[requires(rg_inv(*r) && rop_pre(*r, op))]
#[requires(rg_in(base@, size@, fp))]
#[requires(forall<p: Int> 0 <= p && p < descs@.len() ==> desc_ok_a(descs@[p], base@, size@, fp))]
#[ensures(count_matches(^r) && count_matches(result.0) && count_matches(result.1))]
pub fn verify_em_bitmap_count_matches(
    r: &mut RegionState,
    op: ROp,
    base: u64,
    size: u64,
    fp: FormatParams,
    descs: &Vec<SlabDescriptor>,
) -> (RegionState, RegionState) {
    proof_assert! { lemma_rg_in(base@, size@, fp); rg_hd(base@, size@, fp) };
    proof_assert! { forall<p: Int> 0 <= p && p < descs@.len() ==> { lemma_desc_ok_a(descs@[p], base@, size@, fp); desc_ok(descs@[p], base@, size@, fp) } };
    apply_rop(r, op);
    proof_assert! { lemma_count_matches(*r); count_matches(*r) };
    let fresh = RegionState::new(BuddyAllocator::new(base, size, fp.sector_size), fp);
    proof_assert! { lemma_count_matches(fresh); count_matches(fresh) };
    let rebuilt = rebuild_region(base, size, fp, descs);
    proof_assert! { lemma_count_matches(rebuilt); count_matches(rebuilt) };
    (fresh, rebuilt)
}

/// Every slab of the region: counter == set bits, and counter <= slot count.
#[logic(open)]
pub fn count_matches(r: RegionState) -> bool {
    pearlite! {
        forall<k: Int> r.slabs@.contains(k) ==>
            r.slabs@.lookup(k).bitmap.allocated_count@ == cnt(r.slabs@.lookup(k).bitmap, r.slabs@.lookup(k).bitmap.num_slots@)
            && r.slabs@.lookup(k).bitmap.allocated_count@ <= r.slabs@.lookup(k).bitmap.num_slots@
    }
}

#[logic]
#[requires(rg_slabs(r))]
#[ensures(count_matches(r))]
pub fn lemma_count_matches(r: RegionState) {
    pearlite! {
        proof_assert! { forall<k: Int> r.slabs@.contains(k) ==> {
            lemma_cnt_bounds(r.slabs@.lookup(k).bitmap, r.slabs@.lookup(k).bitmap.num_slots@);
            cnt(r.slabs@.lookup(k).bitmap, r.slabs@.lookup(k).bitmap.num_slots@) <= r.slabs@.lookup(k).bitmap.num_slots@ } }
    }
}

/// Mutant: claims every slab of the region has at least one free slot (false once a
/// slab fills up).
#[requires(rg_inv(*r) && rop_pre(*r, op))]
#[ensures(forall<k: Int> (^r).slabs@.contains(k) ==>
    (^r).slabs@.lookup(k).bitmap.allocated_count@ < (^r).slabs@.lookup(k).bitmap.num_slots@)]
pub fn verify_em_bitmap_count_matches__mutant(r: &mut RegionState, op: ROp) {
    apply_rop(r, op);
}

// =============================================================================
// Recovery cascade W2 (witness.rs): a recovered slab REPLACED at its own key while a
// live write handle still names one of its slots.
// =============================================================================
//
// Shared root cause of the refutations below: `initialize` (lib.rs:553-555) relies on
// `BuddyAllocator::mark_allocated` to take each recovered slab's block off the free lists,
// but its enclosing-block lookup `offset & !(block_span - 1)` (buddy.rs:136) is the aligned
// block start only for a power-of-two `block_span`, i.e. only for power-of-two sector
// sizes, while FR-002 / lib.rs:384-401 accept any `sector_size > 0` dividing `slab_size`.
// The miss is silent (buddy.rs:157), so the buddy allocator later hands the recovered
// slab's block out again and `alloc_extent` inserts the fresh slab at the SAME map key
// (region.rs:89), replacing the recovered one. A handle reserved in the recovered slab
// before that still carries (slab_start, slot_idx) of a slab that no longer exists, and
// its publish/abort closures (lib.rs:597-621) act on the replacement.
//
// W2 (FR-002-valid): sector 3, slab 6, max_extent 6, region_count 1, data_disk_size 12,
// metadata_region_size 0. After initialize (B recovered at 6 with slot 1 free):
//   H4 = reserve(K4, 3) -> B slot 1 (offset 9);  H5 = reserve(K5, 6) -> fresh slab D at 0;
//   W2a: H6 = reserve(K6, 3) -> fresh slab E (element 3, 2 slots) at 6 replaces B;
//   W2b: H6 = reserve(K6, 6) -> fresh slab E (element 6, 1 slot)  at 6 replaces B.

/// **EM-BITMAP-NO-DOUBLE-FREE** — REFUTED. "A slot is only ever marked free when it is
/// currently allocated." In W2a, H4 names slot 1 of the slab now at key 6 (E), and that
/// slot is FREE: H4.abort() / drop / publish(FREE_KEY) runs free_slot(6, 1)
/// (lib.rs:620 / 602 -> region.rs:98 -> slab.rs:38) and clears a free slot — the
/// bitmap.rs:28 `debug_assert!(self.is_set(idx), "double-clear")` fires (debug build);
/// a release build decrements allocated_count without clearing a bit (bitmap.rs:31-32).
/// The ensures is exactly `!free_ok` = the negation of that call's precondition.
#[ensures(result.0.slabs@.contains(result.1@))]
#[ensures(result.2@ < result.0.slabs@.lookup(result.1@).bitmap.num_slots@)]
#[ensures(!slot_bit(result.0.slabs@.lookup(result.1@).bitmap, result.2@))]
#[ensures(!free_ok(result.0, result.1, result.2))]
pub fn support_old_refute_em_bitmap_no_double_free() -> (RegionState, u64, usize) {
    let (r, s4, i4, _off4) = w2a_replaced();
    (r, s4, i4)
}

/// Mutant: claims H4's slot is still allocated when H4 releases it.
#[ensures(free_ok(result.0, result.1, result.2))]
pub fn support_old_refute_em_bitmap_no_double_free__mutant() -> (RegionState, u64, usize) {
    let (r, s4, i4, _off4) = w2a_replaced();
    (r, s4, i4)
}

/// **EM-BITMAP-INDEX-IN-RANGE** — REFUTED. "Every slot index used to set, clear or test
/// the bitmap is less than the slab's number of slots." In W2b the slab at key 6 is E with
/// ONE slot, while H4 carries slot index 1: H4.abort() / drop / publish(FREE_KEY) runs
/// free_slot(6, 1) -> bitmap.clear(1) with num_slots 1 (bitmap.rs:27 debug_assert) and
/// `self.keys[1] = FREE_KEY` on a 1-entry key vector (slab.rs:39) — an index-out-of-bounds
/// panic in debug AND release builds.
#[ensures(result.0.slabs@.contains(result.1@))]
#[ensures(result.2@ >= result.0.slabs@.lookup(result.1@).bitmap.num_slots@)]
#[ensures(result.2@ >= result.0.slabs@.lookup(result.1@).keys@.len())]
pub fn support_old_refute_em_bitmap_index_in_range() -> (RegionState, u64, usize) {
    let (r, s4, i4, _off4, _off6) = w2b_replaced();
    proof_assert! { slab_inv(r.slabs@.lookup(6)) };
    (r, s4, i4)
}

/// Mutant: claims H4's index is in range of the slab it now names.
#[ensures(result.0.slabs@.contains(result.1@) ==> result.2@ < result.0.slabs@.lookup(result.1@).bitmap.num_slots@)]
pub fn support_old_refute_em_bitmap_index_in_range__mutant() -> (RegionState, u64, usize) {
    let (r, s4, i4, _off4, _off6) = w2b_replaced();
    (r, s4, i4)
}

/// **EM-ABORT-POST-RELEASED** — REFUTED. "After abort() is called on a write handle, ...
/// its slot is released immediately, without waiting for a checkpoint, so later
/// reservations can reuse it." In W2b H4 reserved [9, 12); those bytes now lie inside
/// slot 0 of the replacement slab E ([6, 12)), which is ALLOCATED to the live handle H6.
/// H4.abort() runs free_slot(6, 1) on E, whose slot index 1 does not exist: slab.rs:39
/// panics (debug and release). H4's bytes are never released and stay owned by H6.
#[ensures(result.0.slabs@.contains(result.1@))]
#[ensures(result.0.slabs@.lookup(result.1@).start_offset@ <= result.3@
    && result.3@ + 3 <= result.0.slabs@.lookup(result.1@).start_offset@ + result.0.slabs@.lookup(result.1@).element_size@)]
#[ensures(slot_bit(result.0.slabs@.lookup(result.1@).bitmap, 0))]
#[ensures(!free_ok(result.0, result.1, result.2))]
pub fn support_old_refute_em_abort_post_released() -> (RegionState, u64, usize, u64) {
    let (r, s4, i4, off4, _off6) = w2b_replaced();
    (r, s4, i4, off4)
}

/// Mutant: claims H4's abort would be a valid release of an allocated in-range slot.
#[ensures(free_ok(result.0, result.1, result.2))]
pub fn support_old_refute_em_abort_post_released__mutant() -> (RegionState, u64, usize, u64) {
    let (r, s4, i4, off4, _off6) = w2b_replaced();
    (r, s4, i4, off4)
}

/// **EM-PUBLISH-POST-FREE-KEY-DISCARDED** — REFUTED. "When publish() is called on a handle
/// whose key is u64::MAX (the FREE_KEY sentinel), it returns success, ... and the reserved
/// slot is released immediately." Take H4 reserved with key FREE_KEY (region_for_key:
/// FREE_KEY & 0 = region 0, lib.rs:219). In W2b H4.publish() takes the sentinel branch
/// (lib.rs:600-602) -> free_slot(6, 1) on the 1-slot replacement slab E -> slab.rs:39
/// index out of bounds: publish panics instead of returning success, and H4's bytes
/// [9, 12) stay inside H6's allocated slot.
#[ensures(result.0.slabs@.contains(result.1@))]
#[ensures(result.2@ >= result.0.slabs@.lookup(result.1@).keys@.len())]
#[ensures(slot_bit(result.0.slabs@.lookup(result.1@).bitmap, 0))]
#[ensures(result.0.slabs@.lookup(result.1@).start_offset@ <= result.3@
    && result.3@ + 3 <= result.0.slabs@.lookup(result.1@).start_offset@ + result.0.slabs@.lookup(result.1@).element_size@)]
pub fn support_old_refute_em_publish_post_free_key_discarded() -> (RegionState, u64, usize, u64) {
    let (r, s4, i4, off4, _off6) = w2b_replaced();
    proof_assert! { slab_inv(r.slabs@.lookup(6)) };
    (r, s4, i4, off4)
}

/// Mutant: claims the sentinel publish's free_slot call is valid.
#[ensures(free_ok(result.0, result.1, result.2))]
pub fn support_old_refute_em_publish_post_free_key_discarded__mutant() -> (RegionState, u64, usize, u64) {
    let (r, s4, i4, off4, _off6) = w2b_replaced();
    (r, s4, i4, off4)
}

pub const W_K4: u64 = 4;
pub const W_K7: u64 = 7;

/// **EM-INV-KEY-IMPLIES-ALLOCATED** — REFUTED. "Any slot whose key entry is not the
/// FREE_KEY sentinel is marked allocated in its slab's allocation bitmap." In W2a,
/// H4.publish() with key K4 runs publish_slot(6, 1, K4) (lib.rs:610 -> region.rs:114-119
/// -> slab.rs:42) on the replacement slab E: E.keys[1] = K4 while E's slot 1 is FREE.
/// get_extents then lists K4 at offset 9 in a slot the allocator still considers free.
#[ensures(result.slabs@.contains(6))]
#[ensures(result.slabs@.lookup(6).keys@[1] == W_K4 && W_K4 != FREE_KEY)]
#[ensures(1 < result.slabs@.lookup(6).bitmap.num_slots@ && !slot_bit(result.slabs@.lookup(6).bitmap, 1))]
pub fn support_old_refute_em_inv_key_implies_allocated() -> RegionState {
    let (mut r, s4, i4, _off4) = w2a_replaced();
    r.publish_slot(s4, i4, W_K4);
    r
}

/// Mutant: claims every non-free key sits in an allocated slot.
#[ensures(forall<k: Int> result.slabs@.contains(k) ==> key_alloc(result.slabs@.lookup(k)))]
pub fn support_old_refute_em_inv_key_implies_allocated__mutant() -> RegionState {
    let (mut r, s4, i4, _off4) = w2a_replaced();
    r.publish_slot(s4, i4, W_K4);
    r
}

/// **EM-INV-CONCURRENT-LINEARIZABLE** — REFUTED (on the explicit clause "after a
/// concurrent mix of publish and abort exactly the published extents are listed").
/// In W2a: H4.publish() (K4, returns Ok, lib.rs:610-615) writes K4 into the FREE slot 1 of
/// the replacement slab E; the next reserve_extent(K7, 3) is handed that same slot
/// (size class 3 lists E, region.rs:47-59) and H7.publish() overwrites it with K7.
/// K4 was published and never removed, yet no key vector of the region holds it, so
/// get_extents()/for_each_extent() (which list every non-FREE key of every slab,
/// lib.rs:632-678) do not list it: a lost update. The schedule is a single-thread
/// one (a degenerate concurrent run); the cause is the recovery cascade, not a race —
/// the per-region locking itself (each call is one critical section) is not refuted.
#[ensures(forall<k: Int> result.slabs@.contains(k) ==> k == 0 || k == 6)]
#[ensures(forall<k: Int, j: Int> result.slabs@.contains(k) && 0 <= j && j < result.slabs@.lookup(k).keys@.len() ==>
    result.slabs@.lookup(k).keys@[j] != W_K4)]
#[ensures(result.slabs@.lookup(6).keys@[1] == W_K7)]
pub fn support_old_refute_em_inv_concurrent_linearizable() -> RegionState {
    let (mut r, s4, i4, _off4) = w2a_replaced();
    r.publish_slot(s4, i4, W_K4); // H4.publish() -> Ok
    let old = snapshot! { r };
    proof_assert! { align_l(3, 3) == 3 };
    proof_assert! { sc_get(r.size_classes, 3) == Seq::singleton(6u64) };
    proof_assert! { exist_cond(*old, 3, 6u64) };
    let res = r.alloc_extent(3); // H7 = reserve(K7, 3)
    proof_assert! { exist_case(*old, r, 3, 6u64, res) };
    match res {
        Ok((s7, i7, _off7)) => {
            proof_assert! { i7@ == 1 };
            r.publish_slot(s7, i7, W_K7); // H7.publish() -> Ok
        }
        Err(_) => {}
    }
    r
}

/// Mutant: claims the published K4 is still listed in slab 6.
#[ensures(result.slabs@.lookup(6).keys@[1] == W_K4)]
pub fn support_old_refute_em_inv_concurrent_linearizable__mutant() -> RegionState {
    let (mut r, s4, i4, _off4) = w2a_replaced();
    r.publish_slot(s4, i4, W_K4);
    let old = snapshot! { r };
    proof_assert! { align_l(3, 3) == 3 };
    proof_assert! { exist_cond(*old, 3, 6u64) };
    let res = r.alloc_extent(3);
    match res {
        Ok((s7, i7, _off7)) => r.publish_slot(s7, i7, W_K7),
        Err(_) => {}
    }
    r
}

/// One reserve_extent(_, 6) served from the stale size-class-6 entry 12 (region.rs:47-59).
#[requires(rg_inv(*r) && r.format_params.sector_size@ == 3)]
#[requires(sc_get(r.size_classes, 6) == Seq::singleton(12u64))]
#[requires(r.slabs@.contains(12) && r.slabs@.lookup(12).bitmap.allocated_count@ < r.slabs@.lookup(12).bitmap.num_slots@)]
#[ensures(rg_inv(^r) && (^r).format_params.sector_size@ == 3)]
#[ensures((^r).slabs@.contains(12))]
#[ensures(result@ < r.slabs@.lookup(12).bitmap.num_slots@ && !slot_bit(r.slabs@.lookup(12).bitmap, result@))]
#[ensures(slot_bit((^r).slabs@.lookup(12).bitmap, result@))]
#[ensures(forall<j: Int> 0 <= j && j < r.slabs@.lookup(12).bitmap.num_slots@ && j != result@ ==>
    slot_bit((^r).slabs@.lookup(12).bitmap, j) == slot_bit(r.slabs@.lookup(12).bitmap, j))]
#[ensures((^r).slabs@.lookup(12).bitmap.num_slots == r.slabs@.lookup(12).bitmap.num_slots)]
#[ensures((^r).slabs@.lookup(12).start_offset == r.slabs@.lookup(12).start_offset)]
#[ensures((^r).slabs@.lookup(12).element_size == r.slabs@.lookup(12).element_size)]
#[ensures((^r).slabs@.lookup(12).bitmap.allocated_count@ == r.slabs@.lookup(12).bitmap.allocated_count@ + 1)]
#[ensures((^r).slabs@.lookup(12).bitmap.allocated_count@ < (^r).slabs@.lookup(12).bitmap.num_slots@ ==>
    sc_get((^r).size_classes, 6) == Seq::singleton(12u64))]
pub fn w3_reserve6(r: &mut RegionState) -> usize {
    proof_assert! { align_l(6, 3) == 6 };
    let old = snapshot! { *r };
    proof_assert! { exist_cond(*old, 6, 12u64) };
    let res = r.alloc_extent(6);
    proof_assert! { exist_case(*old, *r, 6, 12u64, res) };
    match res {
        Ok((_s, i, _off)) => i,
        Err(_) => 0,
    }
}

/// **EM-INV-WITHIN-USABLE-RANGE** — REFUTED. "Every reserved or published extent starts at
/// or after the data start offset and ends at or before the data disk size ... so it never
/// ... runs past the end of the data device."
///
/// Root cause: after a non-power-of-two-sector recovery the buddy.rs:136 mask misses, the
/// recovered slab's block is handed out again, and the fresh slab REPLACES the recovered one
/// at its key (region.rs:89) without removing the recovered slab's size-class entry
/// (initialize lists every recovered slab, lib.rs:563); alloc_extent trusts the entry
/// (region.rs:48-59 never checks the slab's element size), so reserve_extent(_, 6) is served
/// from a 3-byte slot while reporting the requested aligned size 6 (lib.rs:591).
///
/// Witness W3 (FR-002-valid; sector 3 is accepted by format but unrealistic for NVMe):
/// data range [0, 24); after w3_stale, three reserve_extent(_, 6) calls take slots 1, 2, 3
/// of the 4-slot element-3 slab Z at 12 (in some order); the one at slot 3 reports the
/// extent [21, 27), which ends past data_disk_size 24.
#[ensures(result.3@ == 6)]
#[ensures(result.0@ + result.3@ > 24 || result.1@ + result.3@ > 24 || result.2@ + result.3@ > 24)]
pub fn support_old_refute_em_inv_within_usable_range() -> (u64, u64, u64, u32) {
    let mut r = w3_stale();
    let z0 = snapshot! { r.slabs@.lookup(12) };
    let i1 = w3_reserve6(&mut r);
    let z1 = snapshot! { r.slabs@.lookup(12) };
    let i2 = w3_reserve6(&mut r);
    let z2 = snapshot! { r.slabs@.lookup(12) };
    let i3 = w3_reserve6(&mut r);
    proof_assert! { 1 <= i1@ && i1@ < 4 && 1 <= i2@ && i2@ < 4 && 1 <= i3@ && i3@ < 4 };
    proof_assert! { i1 != i2 && i1 != i3 && i2 != i3 };
    proof_assert! { i1@ == 3 || i2@ == 3 || i3@ == 3 };
    // offsets the handles report: slot_offset = 12 + i * 3 (slab.rs:58-60, region.rs:53)
    let o1 = 12 + i1 as u64 * 3;
    let o2 = 12 + i2 as u64 * 3;
    let o3 = 12 + i3 as u64 * 3;
    // aligned_size = (size + bs - 1) / bs * bs (lib.rs:591)
    let aligned: u32 = (6 + 3 - 1) / 3 * 3;
    (o1, o2, o3, aligned)
}

/// Mutant: claims every reported extent ends within the data device.
#[ensures(result.0@ + result.3@ <= 24 && result.1@ + result.3@ <= 24 && result.2@ + result.3@ <= 24)]
pub fn support_old_refute_em_inv_within_usable_range__mutant() -> (u64, u64, u64, u32) {
    let mut r = w3_stale();
    let i1 = w3_reserve6(&mut r);
    let i2 = w3_reserve6(&mut r);
    let i3 = w3_reserve6(&mut r);
    proof_assert! { 1 <= i1@ && i1@ < 4 && 1 <= i2@ && i2@ < 4 && 1 <= i3@ && i3@ < 4 };
    let o1 = 12 + i1 as u64 * 3;
    let o2 = 12 + i2 as u64 * 3;
    let o3 = 12 + i3 as u64 * 3;
    let aligned: u32 = (6 + 3 - 1) / 3 * 3;
    (o1, o2, o3, aligned)
}
