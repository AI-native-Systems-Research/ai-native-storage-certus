//! Witness chains: concrete executions of the mirrored code, each step a call to the
//! real mirror whose exact-case contract fixes the next state. Not scored on their
//! own (they are helpers); the whole-crate run proves them.
use crate::model::bitmap::*;
use crate::model::buddy::*;
use crate::model::params::*;
use crate::model::region::*;
use crate::model::slab::*;
use crate::model::assume::*;
use creusot_std::prelude::*;

/// W1 up to `initialize()`: region [0, 6), sector 3, slab 3.
/// Returns (offset of recovered slab B, the rebuilt buddy allocator).
#[ensures(result.0@ == 3)]
#[ensures(bd_inv(result.1) && result.1.base_offset@ == 0 && result.1.total_usable_size@ == 6)]
#[ensures(result.1.sector_size@ == 3 && result.1.max_order@ == 1 && result.1.free_lists@.len() == 2)]
#[ensures(result.1.free_lists@[0]@.len() == 0 && result.1.free_lists@[1]@ == Seq::singleton(0u64))]
pub fn w1_recovered() -> (u64, BuddyAllocator) {
    // format(): region 0 = [0, 6) (lib.rs:457-468) → BuddyAllocator::new(0, 6, 3)
    let mut b = BuddyAllocator::new(0, 6, 3);
    proof_assert! { 1.pow2() == 2 && 6 / 3 == 2 };
    proof_assert! { b.max_order@ == 1 && b.free_lists@[1]@ == Seq::singleton(0u64) && b.free_lists@[0]@.len() == 0 };
    proof_assert! { lemma_ord_eq(1, 0, 0); ord((3 + 3 - 1) / 3) == 0 };
    proof_assert! { first_ne(b.free_lists@, 0, 1) == 1 };
    // reserve_extent(K1, 3): fresh slab A — alloc_extent → buddy.alloc(slab_size) (region.rs:71-74)
    let x = b.alloc(3).unwrap();
    proof_assert! { x@ == 0 && b.free_lists@[0]@.len() == 1 && b.free_lists@[0]@[0]@ == 3 && b.free_lists@[1]@.len() == 0 };
    proof_assert! { first_ne(b.free_lists@, 0, 1) == 0 };
    // reserve_extent(K2, 3): A is full (one slot), fresh slab B
    let y = b.alloc(3).unwrap();
    proof_assert! { y@ == 3 && b.free_lists@[0]@.len() == 0 && b.free_lists@[1]@.len() == 0 };
    // abort(K1): A empties → free_slot → buddy.free(0, 3) (region.rs:104)
    b.free(x, 3);
    // publish(K2); checkpoint() persists slab B = (start 3, slab 3, elem 3, [K2]);
    // initialize(): BuddyAllocator::new + mark_allocated(3, 3) (lib.rs:552-555)
    let mut b2 = BuddyAllocator::new(0, 6, 3);
    proof_assert! { b2.max_order@ == 1 && b2.free_lists@[1]@ == Seq::singleton(0u64) && b2.free_lists@[0]@.len() == 0 };
    fact_mask_3_6();
    proof_assert! { forall<sp: u64, z: u64> sp@ == span(3, 1) && z@ == 3 ==> sp == 6u64 && z == 3u64 };
    b2.mark_allocated(y, 3);
    (y, b2)
}

/// W1 continued: `remove_extent(3)` + `checkpoint()` empties B, whose space
/// `free_slot` returns with `buddy.free(3, 3)` (region.rs:104).
#[ensures(bd_inv(result) && result.total_usable_size@ == 6 && result.sector_size@ == 3)]
#[ensures(result.free_lists@.len() == 2)]
#[ensures(result.free_lists@[0]@ == Seq::singleton(3u64))]
#[ensures(result.free_lists@[1]@ == Seq::singleton(0u64))]
pub fn w1_freed() -> BuddyAllocator {
    let (y, mut b) = w1_recovered();
    proof_assert! { lemma_ord_eq(1, 0, 0); ord(3 / 3) == 0 };
    b.free(y, 3);
    proof_assert! { b.free_lists@[0]@.ext_eq(Seq::singleton(3u64)) };
    b
}

/// EM-INV-KEY-VECTOR-LENGTH witness: one region of 2^45 bytes, sector 4096,
/// slab_size 4096·(2^32+1); `reserve_extent(k, 4096)` on the fresh region.
/// Returns the region and the key-vector length of the slab it created.
#[ensures(result.1@ == 1)]
pub fn w_kvl_reserve() -> (RegionState, usize) {
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
    // FR-002 validation (lib.rs:384-401) passes:
    proof_assert! { fp.slab_size@ % fp.sector_size@ == 0 && fp.max_extent_size@ <= fp.slab_size@ };
    // region 0 = the whole usable range [0, 2^45) (lib.rs:459-465, region_count 1)
    let b = BuddyAllocator::new(0, 35184372088832, 4096);
    proof_assert! { 33.pow2() == 8589934592 && 35184372088832 / 4096 == 8589934592 };
    proof_assert! { b.max_order@ == 33 && b.free_lists@[33]@ == Seq::singleton(0u64) };
    let mut r = RegionState::new(b, fp);
    proof_assert! { span(4096, 33) == 35184372088832 };
    proof_assert! { rg_inv(r) };
    proof_assert! { lemma_ord_eq(4294967297, 0, 33); ord(17592186048512 / 4096) == 33 };
    proof_assert! { first_ne(r.buddy.free_lists@, 33, 33) == 33 };
    proof_assert! { align_l(4096, 4096) == 4096 && slots_of(17592186048512, 4096) == 1 };
    let res = r.alloc_extent(4096);
    match res {
        Ok((s, _, _)) => {
            let n = match r.slabs.get(s) {
                Some(sl) => sl.keys.len(),
                None => 0,
            };
            (r, n)
        }
        Err(_) => (r, 0),
    }
}

/// Bit-vector fact used by W1: `3 & !(6 - 1) == 2` (the mask buddy.rs:136 computes for
/// the recovered slab at relative offset 3 with an order-1 span of 6 bytes).
#[bitwise_proof]
#[ensures((3u64 & !(6u64 - 1u64)) == 2u64)]
pub fn fact_mask_3_6() {}

// =============================================================================
// W2 — a recovered slab REPLACED at the same key while a live handle names it.
// FR-002-valid geometry: sector_size 3, slab_size 6, max_extent_size 6, region_count 1,
// data_disk_size 12, metadata_region_size 0 -> one region [0, 12) (order-2 block).
// Pre-crash: reserve(K1,3) -> slab A at 0 slot 0; reserve(K2,3) -> A slot 1;
//   reserve(K3,3) -> slab B at 6 slot 0; publish(K3); abort(K1); abort(K2) (A freed);
//   checkpoint() persists B = (start 6, slab 6, elem 3, keys [K3, FREE]).
// initialize(): BuddyAllocator::new(0,12,3) + mark_allocated(6,6) — the buddy.rs:136 mask
//   6 & !(12-1) = 4 misses, so [0,12) stays free while B is rebuilt.
// =============================================================================

pub const W_K3: u64 = 3;

/// The W2 format parameters (all FR-002 checks pass: 3 > 0, 6 % 3 == 0, 6 <= 6, 1 = 2^0).
#[ensures(result.data_disk_size@ == 12 && result.slab_size@ == 6 && result.max_extent_size@ == 6)]
#[ensures(result.sector_size@ == 3 && result.region_count@ == 1 && result.metadata_region_size@ == 0)]
pub fn w2_fp() -> FormatParams {
    FormatParams {
        data_disk_size: 12,
        slab_size: 6,
        max_extent_size: 6,
        sector_size: 3,
        region_count: 1,
        metadata_alignment: 0,
        instance_id: Some(1),
        metadata_disk_ns_id: 1,
        metadata_region_size: 0,
    }
}

/// The checkpointed descriptor of slab B.
#[ensures(result@.len() == 1)]
#[ensures(result@[0].start_offset@ == 6 && result@[0].slab_size@ == 6 && result@[0].element_size@ == 3)]
#[ensures(result@[0].keys@.len() == 2 && result@[0].keys@[0] == W_K3 && result@[0].keys@[1] == FREE_KEY)]
pub fn w2_descs() -> Vec<SlabDescriptor> {
    let mut keys: Vec<u64> = Vec::new();
    keys.push(W_K3);
    keys.push(FREE_KEY);
    let d = SlabDescriptor { start_offset: 6, slab_size: 6, element_size: 3, keys };
    let mut v: Vec<SlabDescriptor> = Vec::new();
    v.push(d);
    v
}

/// `6 & !(12 - 1) == 4` (buddy.rs:136 for B's relative offset 6 and the order-2 span 12).
#[bitwise_proof]
#[ensures((6u64 & !(12u64 - 1u64)) == 4u64)]
pub fn fact_mask_6_12() {}

/// The W2 region after `initialize()`: B recovered at 6 (slot 0 holds K3, slot 1 free),
/// size class 3 lists [6], and the buddy allocator still holds the whole region [0,12)
/// as one free order-2 block.
#[ensures(rg_inv(result) && result.format_params.sector_size@ == 3 && result.format_params.slab_size@ == 6)]
#[ensures(result.buddy.base_offset@ == 0 && result.buddy.total_usable_size@ == 12 && result.buddy.max_order@ == 2)]
#[ensures(result.buddy.free_lists@[0]@.len() == 0 && result.buddy.free_lists@[1]@.len() == 0)]
#[ensures(result.buddy.free_lists@[2]@ == Seq::singleton(0u64))]
#[ensures(result.slabs@.contains(6) && forall<k: Int> result.slabs@.contains(k) ==> k == 6)]
#[ensures(result.slabs@.lookup(6).bitmap.num_slots@ == 2 && result.slabs@.lookup(6).bitmap.allocated_count@ == 1)]
#[ensures(slot_bit(result.slabs@.lookup(6).bitmap, 0) && !slot_bit(result.slabs@.lookup(6).bitmap, 1))]
#[ensures(result.slabs@.lookup(6).element_size@ == 3 && result.slabs@.lookup(6).start_offset@ == 6)]
#[ensures(result.slabs@.lookup(6).keys@[0] == W_K3 && result.slabs@.lookup(6).keys@[1] == FREE_KEY)]
#[ensures(forall<e: Int> sc_get(result.size_classes, e) == if e == 3 { Seq::singleton(6u64) } else { Seq::empty() })]
#[ensures(result.pending_frees@.len() == 0 && !result.dirty)]
pub fn w2_recovered() -> RegionState {
    let fp = w2_fp();
    let descs = w2_descs();
    fact_mask_6_12();
    proof_assert! { 2.pow2() == 4 && 1.pow2() == 2 && 12 / 3 == 4 };
    proof_assert! { lemma_ord_eq(2, 0, 1); ord(6 / 3) == 1 };
    proof_assert! { span(3, 1) == 6 && span(3, 2) == 12 };
    proof_assert! { slots_of(6, 3) == 2 };
    proof_assert! { desc_ok(descs@[0], 0, 12, fp) };
    proof_assert! { forall<sp: u64, x: u64> sp@ == span(3, 2) && x@ == 6 ==> sp == 12u64 && x == 6u64 };
    proof_assert! { mark_noop_p2(descs@[0], 0, 12, fp, 2) };
    let r = rebuild_region(0, 12, fp, &descs);
    proof_assert! { rebuilt_one(r, descs@[0], 0, 12, fp) };
    proof_assert! { one_slab(r, descs@[0]) };
    let b = snapshot! { r.slabs@.lookup(6).bitmap };
    proof_assert! { b.num_slots@ == 2 && slot_bit(*b, 0) && !slot_bit(*b, 1) };
    proof_assert! { cnt(*b, 0) == 0 && cnt(*b, 1) == 1 && cnt(*b, 2) == 1 };
    r
}

/// W2 step 1 (after initialize): reserve_extent(K4, 3) -> size class 3 lists B, which has
/// a free slot -> slot 1 of B (offset 9). B is now full and leaves size class 3.
/// Returns (region, handle slab_start, slot_idx, offset).
#[ensures(rg_inv(result.0) && result.0.format_params.sector_size@ == 3 && result.0.format_params.slab_size@ == 6)]
#[ensures(result.1@ == 6 && result.2@ == 1 && result.3@ == 9)]
#[ensures(result.0.buddy.base_offset@ == 0 && result.0.buddy.max_order@ == 2)]
#[ensures(result.0.buddy.free_lists@[0]@.len() == 0 && result.0.buddy.free_lists@[1]@.len() == 0)]
#[ensures(result.0.buddy.free_lists@[2]@ == Seq::singleton(0u64))]
#[ensures(result.0.slabs@.contains(6) && forall<k: Int> result.0.slabs@.contains(k) ==> k == 6)]
#[ensures(result.0.slabs@.lookup(6).bitmap.num_slots@ == 2 && slot_bit(result.0.slabs@.lookup(6).bitmap, 1))]
#[ensures(forall<e: Int> sc_get(result.0.size_classes, e) == Seq::empty())]
pub fn w2_h4() -> (RegionState, u64, usize, u64) {
    let mut r = w2_recovered();
    proof_assert! { align_l(3, 3) == 3 };
    let old = snapshot! { r };
    proof_assert! { exist_cond(*old, 3, 6u64) };
    let res = r.alloc_extent(3);
    proof_assert! { exist_case(*old, r, 3, 6u64, res) };
    match res {
        Ok((s, i, off)) => {
            proof_assert! { i@ < 2 && !slot_bit(old.slabs@.lookup(6).bitmap, i@) };
            proof_assert! { i@ == 1 };
            proof_assert! { off@ == slot_off(old.slabs@.lookup(6), 1) };
            proof_assert! { filter_ne(Seq::singleton(6u64), 6u64) == Seq::empty() };
            (r, s, i, off)
        }
        Err(_) => (r, 0, 0, 0),
    }
}

/// W2 step 2: reserve_extent(K5, 6) -> size class 6 is empty -> a fresh slab D is carved:
/// buddy.alloc(6) pops [0,12) and splits it, returning 0 and pushing [6,12) on order 1.
/// Returns (region, H4's slab_start, slot_idx, offset) — H4 is still live.
#[ensures(rg_inv(result.0) && result.0.format_params.sector_size@ == 3 && result.0.format_params.slab_size@ == 6)]
#[ensures(result.1@ == 6 && result.2@ == 1 && result.3@ == 9)]
#[ensures(result.0.buddy.base_offset@ == 0 && result.0.buddy.max_order@ == 2)]
#[ensures(result.0.buddy.free_lists@[1]@ == Seq::singleton(6u64))]
#[ensures(result.0.slabs@.contains(6) && result.0.slabs@.contains(0))]
#[ensures(forall<k: Int> result.0.slabs@.contains(k) ==> k == 6 || k == 0)]
#[ensures(result.0.slabs@.lookup(0).bitmap.num_slots@ == 1 && result.0.slabs@.lookup(0).element_size@ == 6)]
#[ensures(forall<j: Int> 0 <= j && j < result.0.slabs@.lookup(0).keys@.len() ==> result.0.slabs@.lookup(0).keys@[j] == FREE_KEY)]
#[ensures(forall<e: Int> sc_get(result.0.size_classes, e) == Seq::empty())]
pub fn w2_h5() -> (RegionState, u64, usize, u64) {
    let (mut r, s4, i4, off4) = w2_h4();
    proof_assert! { align_l(6, 3) == 6 && slots_of(6, 6) == 1 };
    proof_assert! { lemma_ord_eq(2, 0, 1); slab_k(r) == 1 };
    proof_assert! { first_ne(r.buddy.free_lists@, 1, 2) == 2 };
    proof_assert! { span(3, 1) == 6 };
    let old = snapshot! { r };
    let res = r.alloc_extent(6);
    proof_assert! { fresh_case(*old, r, 6, res) };
    match res {
        Ok((d, _, _)) => {
            proof_assert! { d@ == 0 };
            (r, s4, i4, off4)
        }
        Err(_) => (r, s4, i4, off4),
    }
}

/// W2b step 3: reserve_extent(K6, 6) -> size class 6 still empty -> fresh slab E of
/// element size 6 carved at [6,12) — inserted at key 6, REPLACING the recovered slab B
/// (region.rs:89) while H4 still names (6, slot 1). E has ONE slot.
/// Returns (region, H4 slab_start, slot_idx, offset, H6 offset).
#[ensures(rg_inv(result.0))]
#[ensures(result.1@ == 6 && result.2@ == 1 && result.3@ == 9 && result.4@ == 6)]
#[ensures(result.0.slabs@.contains(6))]
#[ensures(result.0.slabs@.lookup(6).bitmap.num_slots@ == 1 && result.0.slabs@.lookup(6).element_size@ == 6)]
#[ensures(result.0.slabs@.lookup(6).start_offset@ == 6 && slot_bit(result.0.slabs@.lookup(6).bitmap, 0))]
pub fn w2b_replaced() -> (RegionState, u64, usize, u64, u64) {
    let (mut r, s4, i4, off4) = w2_h5();
    proof_assert! { align_l(6, 3) == 6 && slots_of(6, 6) == 1 };
    proof_assert! { lemma_ord_eq(2, 0, 1); slab_k(r) == 1 };
    proof_assert! { first_ne(r.buddy.free_lists@, 1, 2) == 1 };
    let old = snapshot! { r };
    let res = r.alloc_extent(6);
    proof_assert! { fresh_case(*old, r, 6, res) };
    match res {
        Ok((d, _, off6)) => {
            proof_assert! { d@ == 6 && off6@ == 6 };
            (r, s4, i4, off4, off6)
        }
        Err(_) => (r, s4, i4, off4, 0),
    }
}

/// W2a step 3: reserve_extent(K6, 3) -> size class 3 is empty (B left it when full) ->
/// fresh slab E of element size 3 carved at [6,12), REPLACING B at key 6 while H4 still
/// names (6, slot 1). E's slot 0 belongs to H6; slot 1 is FREE.
/// Returns (region, H4 slab_start, slot_idx, offset).
#[ensures(rg_inv(result.0) && result.0.format_params.sector_size@ == 3)]
#[ensures(result.1@ == 6 && result.2@ == 1 && result.3@ == 9)]
#[ensures(result.0.slabs@.contains(6) && result.0.slabs@.contains(0))]
#[ensures(forall<k: Int> result.0.slabs@.contains(k) ==> k == 6 || k == 0)]
#[ensures(result.0.slabs@.lookup(6).bitmap.num_slots@ == 2 && result.0.slabs@.lookup(6).element_size@ == 3)]
#[ensures(result.0.slabs@.lookup(6).bitmap.allocated_count@ == 1)]
#[ensures(slot_bit(result.0.slabs@.lookup(6).bitmap, 0) && !slot_bit(result.0.slabs@.lookup(6).bitmap, 1))]
#[ensures(forall<j: Int> 0 <= j && j < result.0.slabs@.lookup(6).keys@.len() ==> result.0.slabs@.lookup(6).keys@[j] == FREE_KEY)]
#[ensures(forall<j: Int> 0 <= j && j < result.0.slabs@.lookup(0).keys@.len() ==> result.0.slabs@.lookup(0).keys@[j] == FREE_KEY)]
#[ensures(forall<e: Int> sc_get(result.0.size_classes, e) == if e == 3 { Seq::singleton(6u64) } else { Seq::empty() })]
pub fn w2a_replaced() -> (RegionState, u64, usize, u64) {
    let (mut r, s4, i4, off4) = w2_h5();
    proof_assert! { align_l(3, 3) == 3 && slots_of(6, 3) == 2 };
    proof_assert! { lemma_ord_eq(2, 0, 1); slab_k(r) == 1 };
    proof_assert! { first_ne(r.buddy.free_lists@, 1, 2) == 1 };
    let old = snapshot! { r };
    let res = r.alloc_extent(3);
    proof_assert! { fresh_case(*old, r, 3, res) };
    match res {
        Ok((d, _, _)) => {
            proof_assert! { d@ == 6 };
            proof_assert! { 1 % 2 == 1 };
            proof_assert! { Seq::<u64>::empty().push_back(6u64).ext_eq(Seq::singleton(6u64)) };
            (r, s4, i4, off4)
        }
        Err(_) => (r, s4, i4, off4),
    }
}

// =============================================================================
// W3 — a STALE size-class entry: the recovered slab is replaced by one of a SMALLER
// element size, but its size class still lists the key.
// FR-002-valid geometry: sector_size 3, slab_size 12, max_extent_size 12, region_count 1,
// data_disk_size 24, metadata_region_size 0 -> one region [0, 24) (order-3 block).
// Pre-crash: reserve(K1,12) -> slab A at 0; reserve(K2,6) -> slab Y at 12 slot 0;
//   publish(K2); abort(K1) (A freed); checkpoint() persists Y = (12, 12, 6, [K2, FREE]).
// initialize(): mark_allocated(12,12) — mask 12 & !(24-1) = 8 misses; Y rebuilt and
//   listed in size class 6 (lib.rs:563).
// =============================================================================

pub const W_K2: u64 = 2;

#[ensures(result.data_disk_size@ == 24 && result.slab_size@ == 12 && result.max_extent_size@ == 12)]
#[ensures(result.sector_size@ == 3 && result.region_count@ == 1 && result.metadata_region_size@ == 0)]
pub fn w3_fp() -> FormatParams {
    FormatParams {
        data_disk_size: 24,
        slab_size: 12,
        max_extent_size: 12,
        sector_size: 3,
        region_count: 1,
        metadata_alignment: 0,
        instance_id: Some(1),
        metadata_disk_ns_id: 1,
        metadata_region_size: 0,
    }
}

#[ensures(result@.len() == 1)]
#[ensures(result@[0].start_offset@ == 12 && result@[0].slab_size@ == 12 && result@[0].element_size@ == 6)]
#[ensures(result@[0].keys@.len() == 2 && result@[0].keys@[0] == W_K2 && result@[0].keys@[1] == FREE_KEY)]
pub fn w3_descs() -> Vec<SlabDescriptor> {
    let mut keys: Vec<u64> = Vec::new();
    keys.push(W_K2);
    keys.push(FREE_KEY);
    let d = SlabDescriptor { start_offset: 12, slab_size: 12, element_size: 6, keys };
    let mut v: Vec<SlabDescriptor> = Vec::new();
    v.push(d);
    v
}

/// `12 & !(24 - 1) == 8` (buddy.rs:136 for Y's relative offset 12 and the order-3 span 24).
#[bitwise_proof]
#[ensures((12u64 & !(24u64 - 1u64)) == 8u64)]
pub fn fact_mask_12_24() {}

/// W3 after initialize, then reserve(K3, 12) (fresh slab Z0 at 0, buddy pushes [12,24) on
/// order 2) and reserve(K4, 3) (fresh slab Z of element 3 at 12, REPLACING Y). Size class
/// 6 still lists 12 (Y's entry, lib.rs:563) although the slab at 12 is now Z.
#[ensures(rg_inv(result) && result.format_params.sector_size@ == 3)]
#[ensures(result.buddy.base_offset@ == 0 && result.buddy.total_usable_size@ == 24)]
#[ensures(result.slabs@.contains(12))]
#[ensures(result.slabs@.lookup(12).start_offset@ == 12 && result.slabs@.lookup(12).element_size@ == 3)]
#[ensures(result.slabs@.lookup(12).bitmap.num_slots@ == 4 && result.slabs@.lookup(12).bitmap.allocated_count@ == 1)]
#[ensures(slot_bit(result.slabs@.lookup(12).bitmap, 0))]
#[ensures(forall<j: Int> 0 < j && j < 4 ==> !slot_bit(result.slabs@.lookup(12).bitmap, j))]
#[ensures(sc_get(result.size_classes, 6) == Seq::singleton(12u64))]
pub fn w3_stale() -> RegionState {
    let fp = w3_fp();
    let descs = w3_descs();
    fact_mask_12_24();
    proof_assert! { 3.pow2() == 8 && 2.pow2() == 4 && 1.pow2() == 2 && 24 / 3 == 8 };
    proof_assert! { lemma_ord_eq(4, 0, 2); ord(12 / 3) == 2 };
    proof_assert! { span(3, 2) == 12 && span(3, 3) == 24 };
    proof_assert! { slots_of(12, 6) == 2 && slots_of(12, 12) == 1 && slots_of(12, 3) == 4 };
    proof_assert! { desc_ok(descs@[0], 0, 24, fp) };
    proof_assert! { forall<sp: u64, x: u64> sp@ == span(3, 3) && x@ == 12 ==> sp == 24u64 && x == 12u64 };
    proof_assert! { mark_noop_p2(descs@[0], 0, 24, fp, 3) };
    let mut r = rebuild_region(0, 24, fp, &descs);
    proof_assert! { rebuilt_one(r, descs@[0], 0, 24, fp) };
    proof_assert! { one_slab(r, descs@[0]) };
    proof_assert! { r.buddy.max_order@ == 3 && r.buddy.free_lists@[3]@ == Seq::singleton(0u64) };
    proof_assert! { r.buddy.free_lists@[2]@.len() == 0 };
    proof_assert! { slab_k(r) == 2 };
    proof_assert! { first_ne(r.buddy.free_lists@, 2, 3) == 3 };
    proof_assert! { align_l(12, 3) == 12 && align_l(3, 3) == 3 };
    // reserve(K3, 12): fresh slab Z0 at 0
    let old = snapshot! { r };
    let res = r.alloc_extent(12);
    proof_assert! { fresh_case(*old, r, 12, res) };
    match res {
        Ok((d, _, _)) => {
            proof_assert! { d@ == 0 };
        }
        Err(_) => {}
    }
    proof_assert! { r.buddy.free_lists@[2]@ == Seq::singleton(12u64) };
    proof_assert! { first_ne(r.buddy.free_lists@, 2, 3) == 2 };
    // reserve(K4, 3): fresh slab Z (element 3) at 12, replacing Y
    let old2 = snapshot! { r };
    let res2 = r.alloc_extent(3);
    proof_assert! { fresh_case(*old2, r, 3, res2) };
    match res2 {
        Ok((d, _, _)) => {
            proof_assert! { d@ == 12 };
        }
        Err(_) => {}
    }
    r
}

/// W2 pre-crash history, from `format` (fresh region [0,12)): reserve(K1,3) -> slab A at 0
/// slot 0; reserve(K2,3) -> A slot 1 (A full); reserve(K3,3) -> fresh slab B at 6 slot 0;
/// publish(K3); abort(K1); abort(K2) -> A empty, freed. The region then holds exactly B =
/// (start 6, slab 6, element 3, keys [K3, FREE]) — what checkpoint() serializes
/// (checkpoint.rs:15-33) and initialize() recovers as `w2_descs()`.
#[ensures(rg_inv(result))]
#[ensures(result.slabs@.contains(6) && forall<k: Int> result.slabs@.contains(k) ==> k == 6)]
#[ensures(result.slabs@.lookup(6).start_offset@ == 6 && result.slabs@.lookup(6).slab_size@ == 6)]
#[ensures(result.slabs@.lookup(6).element_size@ == 3 && result.slabs@.lookup(6).keys@.len() == 2)]
#[ensures(result.slabs@.lookup(6).keys@[0] == W_K3 && result.slabs@.lookup(6).keys@[1] == FREE_KEY)]
pub fn w2_precrash() -> RegionState {
    let fp = w2_fp();
    proof_assert! { 2.pow2() == 4 && 1.pow2() == 2 && 12 / 3 == 4 };
    let b = BuddyAllocator::new(0, 12, 3);
    proof_assert! { b.max_order@ == 2 && b.free_lists@[2]@ == Seq::singleton(0u64) };
    proof_assert! { b.free_lists@[0]@.len() == 0 && b.free_lists@[1]@.len() == 0 };
    let mut r = RegionState::new(b, fp);
    proof_assert! { span(3, 1) == 6 && span(3, 2) == 12 };
    proof_assert! { rg_inv(r) };
    proof_assert! { lemma_ord_eq(2, 0, 1); slab_k(r) == 1 };
    proof_assert! { align_l(3, 3) == 3 && slots_of(6, 3) == 2 };
    proof_assert! { first_ne(r.buddy.free_lists@, 1, 2) == 2 };
    // reserve(K1, 3): fresh slab A at 0
    let o1 = snapshot! { r };
    let a1 = r.alloc_extent(3);
    proof_assert! { fresh_case(*o1, r, 3, a1) };
    proof_assert! { Seq::<u64>::empty().push_back(0u64).ext_eq(Seq::singleton(0u64)) };
    let (s1, i1) = match a1 {
        Ok((s, i, _)) => (s, i),
        Err(_) => (0, 0),
    };
    proof_assert! { s1@ == 0 && i1@ == 0 && sc_get(r.size_classes, 3) == Seq::singleton(0u64) };
    proof_assert! { r.buddy.free_lists@[1]@ == Seq::singleton(6u64) && r.buddy.free_lists@[2]@.len() == 0 };
    // reserve(K2, 3): A slot 1, A becomes full and leaves size class 3
    let o2 = snapshot! { r };
    proof_assert! { exist_cond(*o2, 3, 0u64) };
    let a2 = r.alloc_extent(3);
    proof_assert! { exist_case(*o2, r, 3, 0u64, a2) };
    let (s2, i2) = match a2 {
        Ok((s, i, _)) => (s, i),
        Err(_) => (0, 0),
    };
    proof_assert! { s2@ == 0 && i2@ == 1 };
    proof_assert! { filter_ne(Seq::singleton(0u64), 0u64) == Seq::empty() };
    proof_assert! { sc_get(r.size_classes, 3).len() == 0 };
    proof_assert! { first_ne(r.buddy.free_lists@, 1, 2) == 1 };
    // reserve(K3, 3): fresh slab B at 6
    let o3 = snapshot! { r };
    let a3 = r.alloc_extent(3);
    proof_assert! { fresh_case(*o3, r, 3, a3) };
    let (s3, i3) = match a3 {
        Ok((s, i, _)) => (s, i),
        Err(_) => (0, 0),
    };
    proof_assert! { s3@ == 6 && i3@ == 0 };
    proof_assert! { r.buddy.free_lists@[1]@.len() == 0 };
    // publish(K3)
    let o35 = snapshot! { r };
    r.publish_slot(s3, i3, W_K3);
    proof_assert! { r.slabs@.lookup(6).element_size == o35.slabs@.lookup(6).element_size };
    proof_assert! { r.slabs@.lookup(6).element_size@ == 3 && r.slabs@.lookup(6).start_offset@ == 6 && r.slabs@.lookup(6).slab_size@ == 6 };
    proof_assert! { r.slabs@.lookup(6).keys@[0] == W_K3 && r.slabs@.lookup(6).keys@[1] == FREE_KEY };
    // abort(K1): A 2 -> 1 allocated
    let o4 = snapshot! { r };
    proof_assert! { free_ok(r, 0u64, 0usize) };
    r.free_slot(0, 0);
    proof_assert! { free_case(*o4, r, 0u64, 0usize, o4.slabs@.lookup(0)) };
    proof_assert! { o4.slabs@.lookup(0).bitmap.allocated_count@ == 2 };
    proof_assert! { r.slabs@ == o4.slabs@.insert(0, r.slabs@.lookup(0)) };
    proof_assert! { r.slabs@.lookup(6) == o4.slabs@.lookup(6) };
    proof_assert! { r.slabs@.lookup(0).bitmap.allocated_count@ == 1 && slot_bit(r.slabs@.lookup(0).bitmap, 1) };
    // abort(K2): A empty -> freed back to the buddy allocator, removed from the map
    let o5 = snapshot! { r };
    proof_assert! { free_ok(r, 0u64, 1usize) };
    r.free_slot(0, 1);
    proof_assert! { free_case(*o5, r, 0u64, 1usize, o5.slabs@.lookup(0)) };
    proof_assert! { r.slabs@ == o5.slabs@.remove(0) };
    proof_assert! { r.slabs@.lookup(6) == o5.slabs@.lookup(6) };
    r
}

/// W3 pre-crash history, from `format` (fresh region [0,24)): reserve(K1,12) -> slab A at
/// 0 (one slot); reserve(K2,6) -> fresh slab Y at 12 slot 0; publish(K2); abort(K1) -> A
/// freed. The region then holds exactly Y = (12, 12, 6, [K2, FREE]) — what checkpoint()
/// serializes and initialize() recovers as `w3_descs()`.
#[ensures(rg_inv(result))]
#[ensures(result.slabs@.contains(12) && forall<k: Int> result.slabs@.contains(k) ==> k == 12)]
#[ensures(result.slabs@.lookup(12).start_offset@ == 12 && result.slabs@.lookup(12).slab_size@ == 12)]
#[ensures(result.slabs@.lookup(12).element_size@ == 6 && result.slabs@.lookup(12).keys@.len() == 2)]
#[ensures(result.slabs@.lookup(12).keys@[0] == W_K2 && result.slabs@.lookup(12).keys@[1] == FREE_KEY)]
pub fn w3_precrash() -> RegionState {
    let fp = w3_fp();
    proof_assert! { 3.pow2() == 8 && 2.pow2() == 4 && 1.pow2() == 2 && 24 / 3 == 8 };
    let b = BuddyAllocator::new(0, 24, 3);
    proof_assert! { b.max_order@ == 3 && b.free_lists@[3]@ == Seq::singleton(0u64) };
    proof_assert! { b.free_lists@[2]@.len() == 0 };
    let mut r = RegionState::new(b, fp);
    proof_assert! { span(3, 2) == 12 && span(3, 3) == 24 };
    proof_assert! { rg_inv(r) };
    proof_assert! { lemma_ord_eq(4, 0, 2); slab_k(r) == 2 };
    proof_assert! { align_l(12, 3) == 12 && align_l(6, 3) == 6 && slots_of(12, 12) == 1 && slots_of(12, 6) == 2 };
    proof_assert! { first_ne(r.buddy.free_lists@, 2, 3) == 3 };
    // reserve(K1, 12): fresh slab A at 0
    let o1 = snapshot! { r };
    let a1 = r.alloc_extent(12);
    proof_assert! { fresh_case(*o1, r, 12, a1) };
    let s1 = match a1 {
        Ok((s, _, _)) => s,
        Err(_) => 0,
    };
    proof_assert! { s1@ == 0 && r.buddy.free_lists@[2]@ == Seq::singleton(12u64) };
    proof_assert! { first_ne(r.buddy.free_lists@, 2, 3) == 2 };
    // reserve(K2, 6): fresh slab Y at 12
    let o2 = snapshot! { r };
    let a2 = r.alloc_extent(6);
    proof_assert! { fresh_case(*o2, r, 6, a2) };
    let (s2, i2) = match a2 {
        Ok((s, i, _)) => (s, i),
        Err(_) => (0, 0),
    };
    proof_assert! { s2@ == 12 && i2@ == 0 && r.buddy.free_lists@[2]@.len() == 0 };
    // publish(K2)
    r.publish_slot(s2, i2, W_K2);
    proof_assert! { r.slabs@.lookup(12).element_size@ == 6 && r.slabs@.lookup(12).start_offset@ == 12 && r.slabs@.lookup(12).slab_size@ == 12 };
    proof_assert! { r.slabs@.lookup(12).keys@[0] == W_K2 && r.slabs@.lookup(12).keys@[1] == FREE_KEY };
    // abort(K1): A empty -> freed, removed from the map
    let o3 = snapshot! { r };
    proof_assert! { o3.slabs@.lookup(0).bitmap.allocated_count@ == 1 };
    proof_assert! { free_ok(r, 0u64, 0usize) };
    r.free_slot(0, 0);
    proof_assert! { free_case(*o3, r, 0u64, 0usize, o3.slabs@.lookup(0)) };
    proof_assert! { r.slabs@ == o3.slabs@.remove(0) };
    proof_assert! { r.slabs@.lookup(12) == o3.slabs@.lookup(12) };
    r
}
