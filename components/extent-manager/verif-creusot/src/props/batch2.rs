//! Batch 2 — 23 further in-scope properties of `extent-manager`.
//!
//! One scored module per inventory id: `verify_<id>` (proof) or `refute_<id>`
//! (machine-checked negation), each with a `__mutant` twin that must FAIL. Unscored
//! `witness_<id>_pow2` modules show that a BUDDY-MASK refutation does not happen when the
//! sector size is a power of two (CREUSOT.md, ADDED section).
use crate::model::bitmap::*;
use crate::model::blockio::*;
use crate::model::buddy::*;
use crate::model::ckpt::*;
use crate::model::component::*;
use crate::model::listing::*;
use crate::model::params::*;
use crate::model::region::*;
use crate::model::sbbytes::*;
use crate::model::slab::*;
use crate::props::batch1::*;
use crate::props::witness::*;
use creusot_std::logic::FMap;
use crate::model::assume::*;
use crate::model::l2::{fdisj, fv, p2};
use crate::model::l2m::{coal, lemma_coal_single};
use crate::model::l2r::{pv, rg_core, rg_good, sk, rg_acct, rg_ka, sc_ok, sl_away};
use crate::props::l2a::lg;
use crate::props::l3start::*;
use creusot_std::prelude::*;

// =============================================================================
// Region / slab layer
// =============================================================================

/// **EM-REGION-FREE-MISSING-SLAB-NOOP** — "Releasing a slot of a slab that does not exist
/// in the region changes nothing." `RegionState::free_slot` (region.rs:93-102) returns at
/// once when `slabs.get_mut(&slab_start)` is `None`; every release (abort/drop,
/// publish(FREE_KEY), checkpoint's deferred frees) goes through it.
#[requires(rg_inv(*r))]
#[requires(r.slabs@.get(s@) == None)]
#[ensures(^r == *r)]
pub fn verify_em_region_free_missing_slab_noop(r: &mut RegionState, s: u64, i: usize) {
    r.free_slot(s, i)
}

/// Mutant: claims the call changed the region.
#[requires(rg_inv(*r))]
#[requires(r.slabs@.get(s@) == None)]
#[ensures(^r != *r)]
pub fn verify_em_region_free_missing_slab_noop__mutant(r: &mut RegionState, s: u64, i: usize) {
    r.free_slot(s, i)
}

/// **EM-INV-SLAB-UNIFORM-ELEMENT** — REFUTED (second clause: "every slab listed under a
/// size class has exactly that element size"). ROOT CAUSE = BUDDY-MASK: after the W3
/// non-power-of-two-sector recovery (sector 3), the recovered slab Y (element 6) at key 12
/// is replaced by a fresh element-3 slab Z (region.rs:89) while size class 6 still lists
/// 12 (lib.rs:563 added Y's entry; nothing removes it). `w3_stale` (witness.rs).
#[ensures(sc_get(result.size_classes, 6) == Seq::singleton(12u64))]
#[ensures(result.slabs@.contains(12) && result.slabs@.lookup(12).element_size@ == 3)]
#[ensures(result.slabs@.lookup(12).element_size@ != 6)]
pub fn support_old_refute_em_inv_slab_uniform_element() -> RegionState {
    w3_stale()
}

/// Mutant: claims the slab listed under size class 6 has element size 6.
#[ensures(result.slabs@.contains(12) && result.slabs@.lookup(12).element_size@ == 6)]
pub fn support_old_refute_em_inv_slab_uniform_element__mutant() -> RegionState {
    w3_stale()
}

/// **EM-BUDDY-FREE-PRE** — REFUTED. "A block is only freed if it is currently allocated
/// from this region, with the size it was allocated with ..." ROOT CAUSE = BUDDY-MASK.
/// W1 (sector 3, region [0,6), slab 3): after initialize the recovered slab B at 3 was
/// never taken off the free lists (mark_allocated no-op, buddy.rs:136), so when
/// remove_extent(3) + checkpoint empty B, free_slot calls `buddy.free(3, 3)`
/// (region.rs:104) on a block that lies INSIDE the free order-1 block [0,6): it is not
/// currently allocated. The ensures states exactly that at the call.
#[ensures(result.0@ == 3)]
#[ensures(result.1.free_lists@[1]@ == Seq::singleton(0u64))]
#[ensures(result.1.free_lists@[1]@[0]@ <= result.0@ - result.1.base_offset@
    && result.0@ - result.1.base_offset@ + span(3, 0) <= result.1.free_lists@[1]@[0]@ + span(3, 1))]
pub fn support_old_refute_em_buddy_free_pre() -> (u64, BuddyAllocator) {
    w1_recovered()
}

/// Mutant: claims no free block covers the block about to be freed.
#[ensures(result.1.free_lists@[1]@.len() == 0)]
pub fn support_old_refute_em_buddy_free_pre__mutant() -> (u64, BuddyAllocator) {
    w1_recovered()
}

/// **EM-USED-RETURNS-AFTER-FREE** — REFUTED. "When every reservation has been aborted or
/// published and later removed, and a checkpoint has completed after the last removal,
/// used_bytes() returns to the value it had right after format()." ROOT CAUSE =
/// BUDDY-MASK. W1: after format used_bytes() = 6 - total_free() = 6 - 6 = 0. History:
/// reserve K1, reserve K2, abort K1, publish K2, checkpoint, initialize, remove_extent(3),
/// checkpoint — every reservation aborted or published-then-removed. The recovered slab's
/// block was never deducted (buddy.rs:136 no-op), so its free adds a second copy: free
/// space 9 > capacity 6 and `total_usable_size - total_free()` (lib.rs:704) underflows —
/// a debug-build panic (release: 2^64 - 3), never 0.
#[ensures(result.0@ == 6 && result.1@ == 6)]
#[ensures(result.2@ == 9 && result.2@ > result.1@)]
pub fn support_old_refute_em_used_returns_after_free() -> (u64, u64, u64) {
    let fresh = BuddyAllocator::new(0, 6, 3);
    proof_assert! { 1.pow2() == 2 && 6 / 3 == 2 };
    proof_assert! { fresh.max_order@ == 1 && fresh.free_lists@[1]@ == Seq::singleton(0u64) && fresh.free_lists@[0]@.len() == 0 };
    proof_assert! { lemma_tf2(fresh.free_lists@, 3); tf(fresh.free_lists@, 3, 2) == 6 };
    let at_format = fresh.total_free();
    let a = w1_freed();
    proof_assert! { lemma_tf2(a.free_lists@, 3); tf(a.free_lists@, 3, 2) == 9 };
    let after = a.total_free();
    (at_format, a.total_usable_size, after)
}

/// Mutant: claims used bytes returned to 0 (free space equals capacity again).
#[ensures(result.2@ == result.1@)]
pub fn support_old_refute_em_used_returns_after_free__mutant() -> (u64, u64, u64) {
    let fresh = BuddyAllocator::new(0, 6, 3);
    proof_assert! { 1.pow2() == 2 && 6 / 3 == 2 };
    proof_assert! { fresh.max_order@ == 1 && fresh.free_lists@[1]@ == Seq::singleton(0u64) && fresh.free_lists@[0]@.len() == 0 };
    proof_assert! { lemma_tf2(fresh.free_lists@, 3); tf(fresh.free_lists@, 3, 2) == 6 };
    let at_format = fresh.total_free();
    let a = w1_freed();
    proof_assert! { lemma_tf2(a.free_lists@, 3); tf(a.free_lists@, 3, 2) == 9 };
    let after = a.total_free();
    (at_format, a.total_usable_size, after)
}

/// The format parameters of the extent-size-unit witness: sector 4096, slab 4096, one
/// region of 8192 bytes (all FR-002 checks pass).
#[ensures(result.sector_size@ == 4096 && result.slab_size@ == 4096 && result.max_extent_size@ == 4096)]
#[ensures(result.data_disk_size@ == 8192 && result.region_count@ == 1)]
#[ensures(result.metadata_alignment@ == 0 && result.metadata_region_size@ == 0)]
pub fn wu_fp() -> FormatParams {
    FormatParams {
        data_disk_size: 8192,
        slab_size: 4096,
        max_extent_size: 4096,
        sector_size: 4096,
        region_count: 1,
        metadata_alignment: 0,
        instance_id: Some(1),
        metadata_disk_ns_id: 1,
        metadata_region_size: 0,
    }
}

/// **EM-INV-EXTENT-SIZE-UNIT** — REFUTED (INDEPENDENT of the buddy mask). "The size
/// reported for an extent is expressed in the unit the Extent type declares (blocks)."
/// `Extent.size` is documented "size in blocks" (interfaces/src/iextent_manager.rs:13), but
/// reserve_extent reports `aligned_size = (size + bs - 1) / bs * bs` BYTES (lib.rs:591,
/// carried by the handle and returned by publish) and get_extents / for_each_extent report
/// the slab's `element_size` in BYTES (lib.rs:646, 670). Witness (one fresh region of a
/// format with sector 4096): reserve_extent(k, 4096) → a fresh slab of element size 4096;
/// the reported size is 4096 while the extent is 1 block.
#[ensures(result.0@ == 4096 && result.1@ == 4096)]
#[ensures(result.0@ / 4096 == 1 && result.0@ != result.0@ / 4096)]
pub fn refute_em_inv_extent_size_unit() -> (u32, u32) {
    let fp = wu_fp();
    // format(): region 0 = [0, 8192) (lib.rs:457-468)
    let b = BuddyAllocator::new(0, 8192, 4096);
    proof_assert! { 1.pow2() == 2 && 8192 / 4096 == 2 };
    let mut r = RegionState::new(b, fp);
    proof_assert! { lemma_span_pos(4096, r.buddy.max_order@); r.buddy.max_order@ == 1 };
    proof_assert! { span(4096, 1) == 8192 };
    proof_assert! { rg_inv(r) };
    proof_assert! { lemma_ord_eq(1, 0, 0); slab_k(r) == 0 };
    proof_assert! { align_l(4096, 4096) == 4096 && slots_of(4096, 4096) == 1 };
    proof_assert! { first_ne(r.buddy.free_lists@, 0, 1) == 1 };
    // LEVEL-3 reachable start (format of wu_fp on a 3 x 4096-byte metadata device, before the
    // reserve): every format-start range on the parameters and on the superblock format writes
    // (offset 4096, size ((12288 - 4096) / 2) / 4096 * 4096 = 4096, data start 0), the reserve
    // ranges for the 4096-byte request, and the proved region invariants of the fresh region.
    let sb0 = Superblock::new(8192, 4096, 4096, 4096, 1, 4096, 4096, 1, 1, 0);
    proof_assert! { 12.pow2() == 4096 && 0.pow2() == 1 && p2(4096) && p2(1) };
    proof_assert! { ckoff_l(0) == 4096 && eff_l(12288, 0) == 12288 && cksize_l(12288, 0, 0, 4096) == 4096 && ds_l(12288, 0, 0, 4096) == 0 };
    proof_assert! { a_c6d2c5(12288, fp) };
    proof_assert! { l3_fmt_ranges(fp, 3, 4096, sb0) && l3_rsv_ranges(4096, fp) };
    proof_assert! { r.buddy.free_lists@.len() == 2 && r.buddy.free_lists@[1]@ == Seq::singleton(0u64) && r.buddy.free_lists@[0]@.len() == 0 };
    proof_assert! { forall<o: Int, k: Int> fv(r.buddy.free_lists@, o, k) ==> o == 1 && k == 0 };
    proof_assert! { lemma_tf2(r.buddy.free_lists@, 4096); rg_tf(r) == 8192 && r.slabs@.len() == 0 };
    proof_assert! { forall<e: Int> sc_get(r.size_classes, e) == Seq::empty() };
    proof_assert! { fdisj(r.buddy.free_lists@, r.buddy.sector_size@) };
    proof_assert! { sl_away(r) };
    proof_assert! { rg_acct(r) };
    proof_assert! { sc_ok(r) };
    proof_assert! { rg_ka(r) };
    proof_assert! { rg_core(r) && rg_good(r) && pv(r, Seq::empty()) && lg(r, Seq::empty()) };
    proof_assert! { lemma_coal_single(r.buddy.free_lists@, r.buddy.sector_size@); coal(r.buddy.free_lists@, r.buddy.sector_size@) };
    proof_assert! { nonfull_listed(r) && coal(r.buddy.free_lists@, r.buddy.sector_size@) && rg_l3(r, Seq::empty()) };
    let old = snapshot! { r };
    // reserve_extent(k, 4096) → alloc_extent(4096) (lib.rs:589)
    let res = r.alloc_extent(4096);
    proof_assert! { fresh_case(*old, r, 4096, res) };
    let listed = match res {
        Ok((d, _i, _off)) => match r.slabs.get(d) {
            Some(sl) => sl.element_size, // what get_extents reports (lib.rs:646)
            None => 0,
        },
        Err(_) => 0,
    };
    let bs = r.format_params.sector_size;
    let aligned_size = (4096u32 + bs - 1) / bs * bs; // reserve_extent (lib.rs:591)
    (aligned_size, listed)
}

/// Mutant: claims the reported size is the block count.
#[ensures(result.0@ == 1)]
pub fn refute_em_inv_extent_size_unit__mutant() -> (u32, u32) {
    let fp = wu_fp();
    let b = BuddyAllocator::new(0, 8192, 4096);
    proof_assert! { 1.pow2() == 2 && 8192 / 4096 == 2 };
    let mut r = RegionState::new(b, fp);
    proof_assert! { lemma_span_pos(4096, r.buddy.max_order@); r.buddy.max_order@ == 1 };
    proof_assert! { span(4096, 1) == 8192 };
    proof_assert! { rg_inv(r) };
    let bs = r.format_params.sector_size;
    let aligned_size = (4096u32 + bs - 1) / bs * bs;
    (aligned_size, 0)
}

// =============================================================================
// Listing (get_extents / for_each_extent)
// =============================================================================

/// **EM-INV-FREE-KEY-NEVER-VISIBLE** — "No extent listed by get_extents() or passed to a
/// for_each_extent() callback ever has the key u64::MAX." Both listing loops skip every
/// slot whose key is FREE_KEY (lib.rs:642, 666); proved over the mirrors for every
/// component state (any number of regions, any slabs).
#[requires(em_regions_wf(*em) && em_regions_ok(*em))]
#[ensures(forall<p: Int> 0 <= p && p < result@.len() ==> result@[p].key != FREE_KEY)]
#[ensures(forall<p: Int> cb@.len() <= p && p < (^cb)@.len() ==> (^cb)@[p].key != FREE_KEY)]
pub fn verify_em_inv_free_key_never_visible(em: &ExtentManager, cb: &mut Vec<Extent>) -> Vec<Extent> {
    em.for_each_extent(cb);
    em.get_extents()
}

/// Mutant: claims some listed extent carries FREE_KEY.
#[requires(em_regions_wf(*em) && em_regions_ok(*em))]
#[ensures(exists<p: Int> 0 <= p && p < result@.len() && result@[p].key == FREE_KEY)]
pub fn verify_em_inv_free_key_never_visible__mutant(em: &ExtentManager, cb: &mut Vec<Extent>) -> Vec<Extent> {
    em.for_each_extent(cb);
    em.get_extents()
}

// =============================================================================
// Buddy allocator: free-space accounting and merging
// =============================================================================

/// **EM-BUDDY-INIT-FREE** — "A new region allocator starts with all of its whole sectors
/// free, that is, total free space equal to the region size rounded down to a multiple of
/// the sector size." `BuddyAllocator::new` (buddy.rs:10-46) for every geometry `format` /
/// `initialize` can build (any base, any size, any sector size > 0), and `total_free`
/// (buddy.rs:160-166, what used_bytes/capacity reason about) returns that amount.
#[requires(sector_size@ > 0 && base@ + total@ <= u64::MAX@)]
#[ensures(tf(result.0.free_lists@, sector_size@, result.0.free_lists@.len()) == (total@ / sector_size@) * sector_size@)]
#[ensures(result.1@ == (total@ / sector_size@) * sector_size@)]
pub fn verify_em_buddy_init_free(base: u64, total: u64, sector_size: u32) -> (BuddyAllocator, u64) {
    let b = BuddyAllocator::new(base, total, sector_size);
    proof_assert! { lemma_divmod(total@, sector_size@); (total@ / sector_size@) * sector_size@ <= total@ };
    let t = b.total_free();
    (b, t)
}

/// Mutant: claims the whole region (not rounded down) is free.
#[requires(sector_size@ > 0 && base@ + total@ <= u64::MAX@)]
#[ensures(result.1@ == total@)]
pub fn verify_em_buddy_init_free__mutant(base: u64, total: u64, sector_size: u32) -> (BuddyAllocator, u64) {
    let b = BuddyAllocator::new(base, total, sector_size);
    proof_assert! { lemma_divmod(total@, sector_size@); (total@ / sector_size@) * sector_size@ <= total@ };
    let t = b.total_free();
    (b, t)
}

/// **EM-BUDDY-FREE-ACCOUNTING** — "Freeing a block that was allocated with the same size
/// increases the total free space by that block's size, so allocate-then-free restores the
/// previous free total." For every allocator state satisfying the buddy invariant and every
/// slab-sized request (a multiple of the sector size, as format enforces for slab_size,
/// lib.rs:387): `alloc` (buddy.rs:57-82) removes exactly one order-`ord(size/ss)` block's
/// bytes from the free total, `free` (buddy.rs:84-115) adds exactly that block's bytes back
/// — whether or not it merges. `result.1` is the free total between the two calls.
// LEVEL-3 D-B1: the allocator is the buddy of a region `r` satisfying the PROVED invariant
// rg_inv (rg_buddy: bd_inv + headroom; rg_params: slab > 0, slab % ss == 0, slab + ss <= MAX),
// and the request is that region's slab size — the only size its one caller, alloc_extent's
// new-slab path (region.rs:66-74), and the matching free (region.rs:81/105) ever use.
// The former `ord(size / ss) <= max_order` premise is dropped: a slab larger than the region
// makes alloc return None with the allocator unchanged (the None arm).
#[requires(rg_inv(*r) && r.buddy == *b && size == r.format_params.slab_size)]
#[ensures(match result.0 {
    Some(_) => *result.1 == tf(b.free_lists@, b.sector_size@, b.free_lists@.len()) - span(b.sector_size@, ord(size@ / b.sector_size@))
        && tf((^b).free_lists@, b.sector_size@, (^b).free_lists@.len()) == *result.1 + span(b.sector_size@, ord(size@ / b.sector_size@))
        && tf((^b).free_lists@, b.sector_size@, (^b).free_lists@.len()) == tf(b.free_lists@, b.sector_size@, b.free_lists@.len()),
    None => ^b == *b,
})]
pub fn verify_em_buddy_free_accounting(b: &mut BuddyAllocator, size: u64, r: Snapshot<RegionState>) -> (Option<u64>, Snapshot<Int>) {
    proof_assert! { rg_params(*r) && rg_buddy(*r) };
    proof_assert! { lemma_div_exact(size@, b.sector_size@); (size@ + b.sector_size@ - 1) / b.sector_size@ == size@ / b.sector_size@ };
    let r = b.alloc(size);
    let mid = snapshot! { tf(b.free_lists@, b.sector_size@, b.free_lists@.len()) };
    match r {
        Some(x) => {
            b.free(x, size);
        }
        None => {}
    }
    (r, mid)
}

/// Mutant: claims allocate-then-free leaves one block's bytes missing.
#[requires(bd_inv(*b) && b.total_usable_size@ + span(b.sector_size@, b.max_order@) <= u64::MAX@)]
#[requires(size@ > 0 && size@ % b.sector_size@ == 0 && size@ + b.sector_size@ <= u64::MAX@)]
#[requires(ord(size@ / b.sector_size@) <= b.max_order@)]
#[ensures(match result.0 {
    Some(_) => tf((^b).free_lists@, b.sector_size@, (^b).free_lists@.len())
        == tf(b.free_lists@, b.sector_size@, b.free_lists@.len()) - span(b.sector_size@, ord(size@ / b.sector_size@)),
    None => true,
})]
pub fn verify_em_buddy_free_accounting__mutant(b: &mut BuddyAllocator, size: u64) -> (Option<u64>, Snapshot<Int>) {
    proof_assert! { lemma_div_exact(size@, b.sector_size@); (size@ + b.sector_size@ - 1) / b.sector_size@ == size@ / b.sector_size@ };
    let r = b.alloc(size);
    let mid = snapshot! { tf(b.free_lists@, b.sector_size@, b.free_lists@.len()) };
    match r {
        Some(x) => {
            b.free(x, size);
        }
        None => {}
    }
    (r, mid)
}

/// `18 ^ 6 == 20` and `0 ^ 6 == 6`: the XOR "buddies" buddy.rs:96 computes for the order-1
/// blocks at 18 and 0 of a sector-3 region (span 6).
#[bitwise_proof]
#[ensures((18u64 ^ 6u64) == 20u64 && (0u64 ^ 6u64) == 6u64)]
pub fn fact_xor_18_0_6() {}

/// BM witness, part 1 (sector 3, one region [0, 24), slab_size 6 = 2 sectors): four
/// reserve_extent(_, 6) calls carve four one-slot slabs, i.e. `buddy.alloc(6)` four times
/// (region.rs:70-74), at 0, 6, 12, 18; their aborts free them (region.rs:104) in the order
/// 12, 18. Returns the allocator after those two frees.
#[ensures(bd_inv(result) && result.base_offset@ == 0 && result.total_usable_size@ == 24)]
#[ensures(result.sector_size@ == 3 && result.max_order@ == 3 && result.free_lists@.len() == 4)]
#[ensures(result.free_lists@[0]@.len() == 0 && result.free_lists@[2]@.len() == 0 && result.free_lists@[3]@.len() == 0)]
#[ensures(result.free_lists@[1]@.len() == 2 && result.free_lists@[1]@[0]@ == 12 && result.free_lists@[1]@[1]@ == 18)]
pub fn wbm_two_freed() -> BuddyAllocator {
    let mut b = BuddyAllocator::new(0, 24, 3);
    proof_assert! { 3.pow2() == 8 && 2.pow2() == 4 && 1.pow2() == 2 && 24 / 3 == 8 };
    proof_assert! { b.max_order@ == 3 && b.free_lists@[3]@ == Seq::singleton(0u64) };
    proof_assert! { b.free_lists@[0]@.len() == 0 && b.free_lists@[1]@.len() == 0 && b.free_lists@[2]@.len() == 0 };
    proof_assert! { lemma_ord_eq(2, 0, 1); ord((6 + 3 - 1) / 3) == 1 && ord(6 / 3) == 1 };
    proof_assert! { span(3, 1) == 6 && span(3, 2) == 12 && span(3, 3) == 24 };
    proof_assert! { first_ne(b.free_lists@, 1, 3) == 3 };
    let a0 = b.alloc(6).unwrap();
    proof_assert! { a0@ == 0 && b.free_lists@[1]@.len() == 1 && b.free_lists@[1]@[0]@ == 6 };
    proof_assert! { b.free_lists@[2]@.len() == 1 && b.free_lists@[2]@[0]@ == 12 && b.free_lists@[3]@.len() == 0 };
    proof_assert! { first_ne(b.free_lists@, 1, 3) == 1 };
    let a1 = b.alloc(6).unwrap();
    proof_assert! { a1@ == 6 && b.free_lists@[1]@.len() == 0 && b.free_lists@[2]@.len() == 1 && b.free_lists@[2]@[0]@ == 12 };
    proof_assert! { first_ne(b.free_lists@, 1, 3) == 2 };
    let a2 = b.alloc(6).unwrap();
    proof_assert! { a2@ == 12 && b.free_lists@[1]@.len() == 1 && b.free_lists@[1]@[0]@ == 18 && b.free_lists@[2]@.len() == 0 };
    proof_assert! { first_ne(b.free_lists@, 1, 3) == 1 };
    let a3 = b.alloc(6).unwrap();
    proof_assert! { a3@ == 18 && b.free_lists@[1]@.len() == 0 && b.free_lists@[2]@.len() == 0 && b.free_lists@[3]@.len() == 0 };
    proof_assert! { b.free_lists@[0]@.len() == 0 };
    // abort the slab at 12: list 1 is empty, so no merge (buddy.rs:100-114)
    b.free(a2, 6);
    proof_assert! { b.free_lists@[1]@.len() == 1 && b.free_lists@[1]@[0]@ == 12 };
    // abort the slab at 18: buddy.rs:96 computes 18 ^ 6 = 20, out of range (20 + 6 > 24,
    // buddy.rs:98), so 18 is pushed although its true buddy 12 is free.
    fact_xor_18_0_6();
    proof_assert! { forall<s: u64> s@ == span(3, 1) ==> s == 6u64 };
    proof_assert! { no_merge(b, a3, 1) };
    b.free(a3, 6);
    proof_assert! { b.free_lists@[1]@.len() == 2 && b.free_lists@[1]@[0]@ == 12 && b.free_lists@[1]@[1]@ == 18 };
    b
}

/// **EM-BUDDY-MERGE** — REFUTED (INDEPENDENT of the buddy mask: the root cause is
/// buddy.rs:96, not buddy.rs:136). "When a freed block's buddy is also free, the two are
/// merged, so freeing all blocks of a region makes the largest original blocks available
/// again." `free` finds the buddy as `current_offset ^ block_span`, which is the arithmetic
/// buddy only when `block_span` is a power of two, i.e. only for power-of-two sector sizes;
/// format accepts any `sector_size > 0` (lib.rs:384-401). Sector 3, region [0, 24),
/// slab 6: after slabs at 0, 6, 12, 18 are carved and the ones at 12 and 18 are freed, the
/// two free order-1 blocks [12,18) and [18,24) are buddies (12 is a multiple of the order-2
/// span 12) but stay unmerged (18 ^ 6 = 20 is out of range). Freeing the other two as well
/// (0, then 6) leaves NO order-3 block, so the original largest block [0,24) is never
/// available again: `alloc(24)` fails on an allocator with nothing allocated. The
/// suggested (non-mechanism) wording fails too — not a wording artifact.
#[ensures(result.0.free_lists@[1]@[0]@ == 12 && result.0.free_lists@[1]@[1]@ == 18)]
#[ensures(12 % span(3, 2) == 0 && 18 == 12 + span(3, 1))]
#[ensures(result.0.free_lists@[2]@.len() == 0)]
#[ensures(result.1 == None)]
pub fn support_old_refute_em_buddy_merge() -> (BuddyAllocator, Option<u64>) {
    let b2 = wbm_two_freed();
    let mut b = wbm_two_freed();
    proof_assert! { span(3, 1) == 6 && span(3, 2) == 12 && span(3, 3) == 24 && 3.pow2() == 8 };
    proof_assert! { lemma_ord_eq(2, 0, 1); ord(6 / 3) == 1 };
    fact_xor_18_0_6();
    proof_assert! { forall<s: u64> s@ == span(3, 1) ==> s == 6u64 };
    // abort the slab at 0: 0 ^ 6 = 6 is not on list 1 ([12, 18]) -> pushed, no merge
    proof_assert! { no_merge(b, 0u64, 1) };
    b.free(0, 6);
    proof_assert! { b.free_lists@[2]@.len() == 0 && b.free_lists@[3]@.len() == 0 };
    // abort the slab at 6: it merges with 0, but list 2 is empty, so nothing reaches order 3
    b.free(6, 6);
    proof_assert! { b.free_lists@[3]@.len() == 0 };
    // every block is free again; ask for the original largest block
    proof_assert! { lemma_ord_eq(8, 0, 3); ord((24 + 3 - 1) / 3) == 3 };
    proof_assert! { first_ne(b.free_lists@, 3, 3) == -1 };
    proof_assert! { b.sector_size@ == 3 && b.max_order@ == 3 };
    proof_assert! { first_ne(b.free_lists@, ord((24 + b.sector_size@ - 1) / b.sector_size@), b.max_order@) == -1 };
    proof_assert! { forall<r: Int> !alloc_post(b, b, 3, -1, r) };
    let big = b.alloc(24);
    (b2, big)
}

/// Mutant: claims the largest original block is available again.
#[ensures(result.1 != None)]
pub fn support_old_refute_em_buddy_merge__mutant() -> (BuddyAllocator, Option<u64>) {
    let b2 = wbm_two_freed();
    let mut b = wbm_two_freed();
    proof_assert! { span(3, 1) == 6 && span(3, 2) == 12 && span(3, 3) == 24 && 3.pow2() == 8 };
    proof_assert! { lemma_ord_eq(2, 0, 1); ord(6 / 3) == 1 };
    fact_xor_18_0_6();
    proof_assert! { forall<s: u64> s@ == span(3, 1) ==> s == 6u64 };
    proof_assert! { no_merge(b, 0u64, 1) };
    b.free(0, 6);
    b.free(6, 6);
    proof_assert! { lemma_ord_eq(8, 0, 3); ord((24 + 3 - 1) / 3) == 3 };
    let big = b.alloc(24);
    (b2, big)
}

// =============================================================================
// Checkpoint path (model/ckpt.rs): failure retention, durability, lost updates,
// premature release, crash atomicity
// =============================================================================

pub const WK1: u64 = 11;
pub const WK2: u64 = 12;

/// The value `wc_fp(slab, disk)` returns (logic form, for the LEVEL-3 start assertions).
#[logic(open)]
pub fn wc_fp_l(slab: u64, disk: u64) -> FormatParams {
    pearlite! { FormatParams {
        data_disk_size: disk, slab_size: slab, max_extent_size: 4u32, sector_size: 4u32,
        region_count: 1u32, metadata_alignment: 0u64, instance_id: Some(1u64), metadata_disk_ns_id: 1u32,
        metadata_region_size: 0u64,
    } }
}

/// The live write handles after wc8_two (K1 -> slot 0, K2 -> slot 1 of the slab at 0).
#[logic(open)]
pub fn hs_w2() -> Seq<(u64, usize)> {
    pearlite! { Seq::empty().push_back((0u64, 0usize)).push_back((0u64, 1usize)) }
}

/// The live write handle after wc4_one (K1 -> slot 0 of the slab at 0).
#[logic(open)]
pub fn hs_w1() -> Seq<(u64, usize)> {
    pearlite! { Seq::singleton((0u64, 0usize)) }
}

/// Format parameters with sector 4, slab `slab`, one region of `disk` bytes (FR-002 valid
/// for the values used below: 4 > 0, slab % 4 == 0, max_extent <= slab, 1 = 2^0).
#[requires(slab@ % 4 == 0 && slab@ > 0)]
#[ensures(result.sector_size@ == 4 && result.slab_size == slab && result.data_disk_size == disk)]
#[ensures(result.region_count@ == 1 && result.max_extent_size@ == 4)]
#[ensures(result == wc_fp_l(slab, disk))]
pub fn wc_fp(slab: u64, disk: u64) -> FormatParams {
    FormatParams {
        data_disk_size: disk,
        slab_size: slab,
        max_extent_size: 4,
        sector_size: 4,
        region_count: 1,
        metadata_alignment: 0,
        instance_id: Some(1),
        metadata_disk_ns_id: 1,
        metadata_region_size: 0,
    }
}

/// C8 (sector 4, slab 8, region [0, 8)): format, then reserve_extent(K1, 4) and
/// reserve_extent(K2, 4) — one 2-slot slab at 0, slot 0 (offset 0) and slot 1 (offset 4).
#[ensures(rg_inv(result) && pending_ok(result) && result.pending_frees@.len() == 0 && !result.dirty)]
#[ensures(result.slabs@.contains(0) && forall<k: Int> result.slabs@.contains(k) ==> k == 0)]
#[ensures(result.slabs@.lookup(0).bitmap.num_slots@ == 2 && result.slabs@.lookup(0).keys@.len() == 2)]
#[ensures(slot_bit(result.slabs@.lookup(0).bitmap, 0) && slot_bit(result.slabs@.lookup(0).bitmap, 1))]
#[ensures(result.slabs@.lookup(0).keys@[0] == FREE_KEY && result.slabs@.lookup(0).keys@[1] == FREE_KEY)]
#[ensures(result.slabs@.lookup(0).start_offset@ == 0 && result.slabs@.lookup(0).element_size@ == 4)]
#[ensures(rg_l3(result, hs_w2()))]
#[ensures(result.format_params == wc_fp_l(8u64, 8u64))]
pub fn wc8_two() -> RegionState {
    let fp = wc_fp(8, 8);
    let b = BuddyAllocator::new(0, 8, 4);
    proof_assert! { 1.pow2() == 2 && 8 / 4 == 2 };
    let mut r = RegionState::new(b, fp);
    proof_assert! { r.buddy.max_order@ == 1 && r.buddy.free_lists@[1]@ == Seq::singleton(0u64) && r.buddy.free_lists@[0]@.len() == 0 };
    proof_assert! { lemma_span_pos(4, 1); span(4, 1) == 8 };
    proof_assert! { rg_inv(r) };
    proof_assert! { lemma_ord_eq(2, 0, 1); slab_k(r) == 1 };
    proof_assert! { align_l(4, 4) == 4 && slots_of(8, 4) == 2 };
    proof_assert! { first_ne(r.buddy.free_lists@, 1, 1) == 1 };
    let old = snapshot! { r };
    let res = r.alloc_extent(4);
    proof_assert! { fresh_case(*old, r, 4, res) };
    proof_assert! { match res { Ok((d, _, _)) => d@ == 0, Err(_) => false } };
    proof_assert! { 1 % 2 == 1 };
    proof_assert! { Seq::<u64>::empty().push_back(0u64).ext_eq(Seq::singleton(0u64)) };
    proof_assert! { sc_get(r.size_classes, 4) == Seq::singleton(0u64) };
    let old2 = snapshot! { r };
    proof_assert! { exist_cond(*old2, 4, 0u64) };
    let res2 = r.alloc_extent(4);
    proof_assert! { exist_case(*old2, r, 4, 0u64, res2) };
    proof_assert! { match res2 { Ok((s, i, _)) => s@ == 0 && i@ == 1, Err(_) => false } };
    // LEVEL-3 reachable start: the proved region invariants with the two live handles.
    proof_assert! { r.buddy.free_lists@.len() == 2 && r.buddy.free_lists@[0]@.len() == 0 && r.buddy.free_lists@[1]@.len() == 0 };
    proof_assert! { forall<o: Int, k: Int> !fv(r.buddy.free_lists@, o, k) };
    proof_assert! { lemma_tf2(r.buddy.free_lists@, 4); rg_tf(r) == 0 && r.slabs@.len() == 1 && sk(r) == 8 };
    proof_assert! { 2.pow2() == 4 && p2(4) };
    proof_assert! { rg_inv(r) && pending_ok(r) && r.pending_frees@.len() == 0 };
    proof_assert! { fdisj(r.buddy.free_lists@, r.buddy.sector_size@) };
    proof_assert! { sl_away(r) };
    proof_assert! { rg_acct(r) };
    proof_assert! { sc_ok(r) };
    proof_assert! { rg_ka(r) };
    proof_assert! { rg_core(r) && rg_good(r) };
    proof_assert! { pv(r, hs_w2()) && lg(r, hs_w2()) };
    proof_assert! { lemma_coal_single(r.buddy.free_lists@, r.buddy.sector_size@); coal(r.buddy.free_lists@, r.buddy.sector_size@) };
    proof_assert! { nonfull_listed(r) && coal(r.buddy.free_lists@, r.buddy.sector_size@) };
    r
}

/// C4 (sector 4, slab 4, region [0, 4)): format, then reserve_extent(K1, 4) — one 1-slot
/// slab at 0; the buddy allocator has no free block left.
#[ensures(rg_inv(result) && pending_ok(result) && result.pending_frees@.len() == 0 && !result.dirty)]
#[ensures(result.slabs@.contains(0) && forall<k: Int> result.slabs@.contains(k) ==> k == 0)]
#[ensures(result.slabs@.lookup(0).bitmap.num_slots@ == 1 && result.slabs@.lookup(0).bitmap.allocated_count@ == 1)]
#[ensures(slot_bit(result.slabs@.lookup(0).bitmap, 0) && result.slabs@.lookup(0).keys@[0] == FREE_KEY)]
#[ensures(result.slabs@.lookup(0).start_offset@ == 0 && result.slabs@.lookup(0).element_size@ == 4)]
#[ensures(result.buddy.max_order@ == 0 && result.buddy.free_lists@[0]@.len() == 0 && result.buddy.base_offset@ == 0)]
#[ensures(forall<e: Int> sc_get(result.size_classes, e) == Seq::empty())]
#[ensures(result.format_params.sector_size@ == 4 && result.format_params.slab_size@ == 4)]
#[ensures(rg_l3(result, hs_w1()))]
#[ensures(result.format_params == wc_fp_l(4u64, 4u64))]
pub fn wc4_one() -> RegionState {
    let fp = wc_fp(4, 4);
    let b = BuddyAllocator::new(0, 4, 4);
    proof_assert! { 0.pow2() == 1 && 4 / 4 == 1 };
    let mut r = RegionState::new(b, fp);
    proof_assert! { r.buddy.max_order@ == 0 && r.buddy.free_lists@[0]@ == Seq::singleton(0u64) };
    proof_assert! { span(4, 0) == 4 };
    proof_assert! { rg_inv(r) };
    proof_assert! { lemma_ord_eq(1, 0, 0); slab_k(r) == 0 };
    proof_assert! { align_l(4, 4) == 4 && slots_of(4, 4) == 1 };
    proof_assert! { first_ne(r.buddy.free_lists@, 0, 0) == 0 };
    let old = snapshot! { r };
    let res = r.alloc_extent(4);
    proof_assert! { fresh_case(*old, r, 4, res) };
    proof_assert! { match res { Ok((d, _, _)) => d@ == 0, Err(_) => false } };
    // LEVEL-3 reachable start: the proved region invariants with the one live handle.
    proof_assert! { r.buddy.free_lists@.len() == 1 && r.buddy.free_lists@[0]@.len() == 0 };
    proof_assert! { forall<o: Int, k: Int> !fv(r.buddy.free_lists@, o, k) };
    proof_assert! { lemma_tf1(r.buddy.free_lists@, 4); rg_tf(r) == 0 && r.slabs@.len() == 1 && sk(r) == 4 };
    proof_assert! { 2.pow2() == 4 && p2(4) };
    proof_assert! { rg_inv(r) && pending_ok(r) && r.pending_frees@.len() == 0 };
    proof_assert! { fdisj(r.buddy.free_lists@, r.buddy.sector_size@) };
    proof_assert! { sl_away(r) };
    proof_assert! { rg_acct(r) };
    proof_assert! { sc_ok(r) };
    proof_assert! { rg_ka(r) };
    proof_assert! { rg_core(r) && rg_good(r) };
    proof_assert! { pv(r, hs_w1()) && lg(r, hs_w1()) };
    proof_assert! { lemma_coal_single(r.buddy.free_lists@, r.buddy.sector_size@); coal(r.buddy.free_lists@, r.buddy.sector_size@) };
    proof_assert! { nonfull_listed(r) && coal(r.buddy.free_lists@, r.buddy.sector_size@) };
    r
}

/// The device right after `format()` (lib.rs:496-498): superblock checkpoint_seq 0,
/// active copy 0; no checkpoint copy written yet.
#[ensures(result.sb_seq@ == 0 && result.sb_active@ == 0 && result.copy0 == None && result.copy1 == None)]
pub fn wc_fresh_dev() -> CkDev {
    CkDev { sb_seq: 0, sb_active: 0, copy0: None, copy1: None }
}

/// LEVEL-3 reachable start of the C8 witnesses: the superblock format writes for
/// `wc_fp(8, 8)` on a metadata device of 3 x 4096 bytes (lib.rs:418-494: offset 4096, size
/// ((12288 - 4096) / 2) / 4 * 4 = 4096, data start 0, seq 0 / active 0); every declared range
/// of a format start holds on it.
#[ensures(l3_fmt_ranges(wc_fp_l(8u64, 8u64), 3, 4096, result))]
#[ensures(result.checkpoint_seq@ == 0 && result.active_copy@ == 0)]
pub fn l3_sb_c8() -> Superblock {
    let sb = Superblock::new(8, 4, 8, 4, 1, 4096, 4096, 1, 1, 0);
    proof_assert! { 2.pow2() == 4 && 1.pow2() == 2 && 0.pow2() == 1 && p2(4) && p2(2) && p2(1) };
    proof_assert! { ckoff_l(0) == 4096 && eff_l(12288, 0) == 12288 && cksize_l(12288, 0, 0, 4) == 4096 && ds_l(12288, 0, 0, 4) == 0 };
    proof_assert! { a_c6d2c5(12288, wc_fp_l(8u64, 8u64)) };
    sb
}

/// LEVEL-3 reachable start of the wa_precrash / wa_full witnesses: the superblock format
/// writes for `wc_fp(8, 16)` on a metadata device of 3 x 4096 bytes (offset 4096, size 4096,
/// data start 0, seq 0 / active 0).
#[ensures(l3_fmt_ranges(wc_fp_l(8u64, 16u64), 3, 4096, result))]
#[ensures(result.checkpoint_seq@ == 0 && result.active_copy@ == 0)]
pub fn l3_sb_c8_16() -> Superblock {
    let sb = Superblock::new(16, 4, 8, 4, 1, 4096, 4096, 1, 1, 0);
    proof_assert! { 2.pow2() == 4 && 1.pow2() == 2 && 0.pow2() == 1 && p2(4) && p2(2) && p2(1) };
    proof_assert! { ckoff_l(0) == 4096 && eff_l(12288, 0) == 12288 && cksize_l(12288, 0, 0, 4) == 4096 && ds_l(12288, 0, 0, 4) == 0 };
    proof_assert! { a_c6d2c5(12288, wc_fp_l(8u64, 16u64)) };
    sb
}

/// LEVEL-3 reachable start of the C4 witnesses: the superblock format writes for
/// `wc_fp(4, 4)` on a metadata device of 3 x 4096 bytes (offset 4096, size 4096, data start
/// 0, seq 0 / active 0) — the `sb1` of wst_race.
#[ensures(l3_fmt_ranges(wc_fp_l(4u64, 4u64), 3, 4096, result))]
#[ensures(result.checkpoint_seq@ == 0 && result.active_copy@ == 0)]
#[ensures(result.instance_id@ == 1)]
pub fn l3_sb_c4() -> Superblock {
    let sb = Superblock::new(4, 4, 4, 4, 1, 4096, 4096, 1, 1, 0);
    proof_assert! { 2.pow2() == 4 && 0.pow2() == 1 && p2(4) && p2(1) };
    proof_assert! { ckoff_l(0) == 4096 && eff_l(12288, 0) == 12288 && cksize_l(12288, 0, 0, 4) == 4096 && ds_l(12288, 0, 0, 4) == 0 };
    proof_assert! { a_c6d2c5(12288, wc_fp_l(4u64, 4u64)) };
    sb
}

/// **EM-CKPT-FAIL-RETAINS-CHANGES** — "After a failed checkpoint, the next checkpoint() call
/// is not skipped as having no changes; the modifications it did not persist are written by
/// the next successful checkpoint, and slots of extents removed since the last successful
/// checkpoint stay allocated and are not handed out until then." For every component state
/// and every device behaviour, with no call interleaving the two checkpoints: a failed
/// `run_checkpoint` (any failure point: copy write, superblock write) leaves every region
/// EXACTLY as it was — dirty flags, deferred frees (`pending_frees`), slab bitmaps and keys —
/// so the next call is not skipped; and the next SUCCESSFUL checkpoint persists exactly
/// that state: a fresh component's recovery returns it.
#[requires(regions_ok(em.regions@) && em.active@ <= 1 && em.seq@ + 2 < u64::MAX@)]
#[ensures(match result.0 {
    Err(_) => *result.2 == em.regions@ && (!all_clean(em.regions@) ==> !all_clean(*result.2)),
    Ok(()) => true,
})]
#[ensures(match (result.0, result.1) {
    (Err(_), Ok(())) => !all_clean(em.regions@) ==> match result.3 { Ok(img) => *img == em.regions@, Err(_) => false },
    _ => true,
})]
pub fn narrowed_verify_em_ckpt_fail_retains_changes(
    em: &mut CkEm, d1: bool, s1: bool, d2: bool, s2: bool,
) -> (Result<(), EmError>, Result<(), EmError>, Snapshot<Seq<RegionState>>, Result<Snapshot<Seq<RegionState>>, EmError>) {
    let r1 = em.run_checkpoint(d1, s1);
    let mid = snapshot! { em.regions@ };
    proof_assert! { em.active@ <= 1 };
    let r2 = em.run_checkpoint(d2, s2);
    let rec = recover(&em.dev);
    (r1, r2, mid, rec)
}

/// Mutant: claims a failed checkpoint cleans the dirty regions.
#[requires(regions_ok(em.regions@) && em.active@ <= 1 && em.seq@ + 2 < u64::MAX@)]
#[ensures(match result.0 { Err(_) => all_clean(*result.2), Ok(()) => true })]
pub fn narrowed_verify_em_ckpt_fail_retains_changes__mutant(
    em: &mut CkEm, d1: bool, s1: bool, d2: bool, s2: bool,
) -> (Result<(), EmError>, Result<(), EmError>, Snapshot<Seq<RegionState>>, Result<Snapshot<Seq<RegionState>>, EmError>) {
    let r1 = em.run_checkpoint(d1, s1);
    let mid = snapshot! { em.regions@ };
    proof_assert! { em.active@ <= 1 };
    let r2 = em.run_checkpoint(d2, s2);
    let rec = recover(&em.dev);
    (r1, r2, mid, rec)
}

/// LOST-UPDATE witness (C8): reserve K1 and K2 (wc8_two); publish(K1); checkpoint() #1
/// begins and serialises the region; publish(K2) runs before #1 clears the dirty flags
/// (no lock spans lib.rs:299 -> :316, see model/ckpt.rs); #1 completes successfully and
/// clears `dirty` (lib.rs:316-320) although K2 is not in what it wrote. checkpoint() #2,
/// called afterwards, finds nothing dirty and returns Ok without I/O (lib.rs:284-286).
/// Returns (component, #1 result, #2 result, what a fresh component recovers).
#[ensures(result.1 == Ok(()) && result.2 == Ok(()))]
#[ensures(result.0.regions@.len() == 1 && all_clean(result.0.regions@))]
#[ensures(result.0.regions@[0].slabs@.contains(0) && result.0.regions@[0].slabs@.lookup(0).keys@.len() == 2)]
#[ensures(result.0.regions@[0].slabs@.lookup(0).keys@[1] == WK2 && slot_bit(result.0.regions@[0].slabs@.lookup(0).bitmap, 1))]
#[ensures(slot_off(result.0.regions@[0].slabs@.lookup(0), 1) == 4 && result.0.regions@[0].slabs@.lookup(0).element_size@ == 4)]
#[ensures(match result.3 { Ok(img) => img_lacks_key(*img, WK2) && img.len() == 1, Err(_) => false })]
pub fn wck_lost_update() -> (CkEm, Result<(), EmError>, Result<(), EmError>, Result<Snapshot<Seq<RegionState>>, EmError>) {
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
    // publish(K1) (lib.rs:610 -> region.rs:114-119)
    em.regions[0].publish_slot(0, 0, WK1);
    let s1 = snapshot! { em.regions@[0].slabs@.lookup(0) };
    proof_assert! { s1.keys@[0] == WK1 && s1.keys@[1] == FREE_KEY && em.regions@[0].dirty };
    proof_assert! { regions_ok(em.regions@) };
    // checkpoint() #1, phase 1 (lib.rs:276-299, checkpoint.rs:47-55)
    let img = match em.ck_begin() {
        Some(img) => img,
        None => return (em, Err(EmError::IoError), Err(EmError::IoError), Err(EmError::IoError)),
    };
    proof_assert! { *img == em.regions@ };
    // publish(K2) interleaves (another thread, region write lock only)
    em.regions[0].publish_slot(0, 1, WK2);
    proof_assert! { em.regions@[0].slabs@.lookup(0).keys@[1] == WK2 && em.regions@[0].slabs@.lookup(0).keys@[0] == WK1 };
    proof_assert! { regions_ok(em.regions@) };
    // checkpoint() #1, phases 2-4 + superblock write, all succeeding; then clear + flush
    let c1 = em.ck_write(img, true, true);
    proof_assert! { c1 == Ok(()) };
    let live = snapshot! { em.regions@ };
    em.ck_finish();
    proof_assert! { fin_rel(live[0], em.regions@[0]) && live[0].pending_frees@.len() == 0 };
    proof_assert! { em.regions@[0].slabs == live[0].slabs };
    // checkpoint() #2 (no call interleaves): nothing is dirty
    let c2 = em.run_checkpoint(true, true);
    // a fresh component's initialize() recovers from the device
    let rec = recover(&em.dev);
    proof_assert! { em.dev.sb_seq@ == 1 && em.dev.sb_active@ == 1 };
    proof_assert! { match em.dev.copy1 { Some(c) => c.seq@ == 1 && c.img == img, None => false } };
    proof_assert! { img[0].slabs@.contains(0) && forall<k: Int> img[0].slabs@.contains(k) ==> k == 0 };
    proof_assert! { img[0].slabs@.lookup(0) == *s1 && WK2 != WK1 && WK2 != FREE_KEY };
    proof_assert! { forall<j: Int> 0 <= j && j < 2 ==> s1.keys@[j] != WK2 };
    (em, c1, c2, rec)
}

/// **EM-CKPT-NO-LOST-UPDATE** — REFUTED (INDEPENDENT; root cause CKPT-SNAPSHOT-RACE:
/// run_checkpoint clears `dirty` and flushes deferred frees at lib.rs:312-321 for EVERY
/// change made since the snapshot, not only for what checkpoint.rs:51-55 serialised). "A
/// publish ... that happens while a checkpoint is in progress is persisted either by that
/// checkpoint or by a later one; the component never treats such a change as already
/// persisted when it is not." Witness `wck_lost_update`: K2, published during checkpoint
/// #1, is in no persisted copy, yet every region is clean, so checkpoint #2 returns Ok
/// without writing it; a restart loses K2.
#[ensures(result.0 && result.1 && result.2)]
pub fn refute_em_ckpt_no_lost_update() -> (bool, bool, bool) {
    let (em, c1, c2, rec) = wck_lost_update();
    let persisted_lacks = match &rec {
        Ok(img) => {
            proof_assert! { img_lacks_key(**img, WK2) };
            true
        }
        Err(_) => false,
    };
    let clean = !em.regions[0].dirty;
    let ok = match (c1, c2) {
        (Ok(()), Ok(())) => true,
        _ => false,
    };
    (persisted_lacks, clean, ok)
}

/// Mutant: claims the lost publish is still marked dirty.
#[ensures(!result.1)]
pub fn refute_em_ckpt_no_lost_update__mutant() -> (bool, bool, bool) {
    let (em, c1, c2, rec) = wck_lost_update();
    let persisted_lacks = match &rec {
        Ok(_img) => true,
        Err(_) => false,
    };
    let clean = !em.regions[0].dirty;
    let ok = match (c1, c2) {
        (Ok(()), Ok(())) => true,
        _ => false,
    };
    (persisted_lacks, clean, ok)
}

/// **EM-CKPT-POST-DURABLE** — REFUTED (INDEPENDENT; root cause CKPT-SNAPSHOT-RACE, the same
/// as EM-CKPT-NO-LOST-UPDATE). "When checkpoint() returns success, a fresh component that
/// initializes from the same metadata device lists exactly the extents that were published
/// and not removed before the call began ..." In `wck_lost_update`, K2 was published (and
/// never removed) BEFORE checkpoint() #2 began; #2 returns Ok, yet the state a fresh
/// component recovers holds no slot with key K2, so its get_extents() cannot list it. (The
/// positive half without interleaving — a successful non-skipped run persists exactly the
/// current regions and recovery returns them — is the ensures of the run_checkpoint
/// mirror, model/ckpt.rs, used by EM-CKPT-FAIL-RETAINS-CHANGES.)
#[ensures(result.0 == Ok(()))]
#[ensures(result.1.slabs@.contains(0) && result.1.slabs@.lookup(0).keys@[1] == WK2)]
#[ensures(match result.2 { Ok(img) => img_lacks_key(*img, WK2), Err(_) => false })]
pub fn refute_em_ckpt_post_durable() -> (Result<(), EmError>, RegionState, Result<Snapshot<Seq<RegionState>>, EmError>) {
    let (mut em, _c1, c2, rec) = wck_lost_update();
    let r = em.regions.pop().unwrap();
    (c2, r, rec)
}

/// Mutant: claims the recovered state contains K2.
#[ensures(match result.2 { Ok(img) => !img_lacks_key(*img, WK2), Err(_) => false })]
pub fn refute_em_ckpt_post_durable__mutant() -> (Result<(), EmError>, RegionState, Result<Snapshot<Seq<RegionState>>, EmError>) {
    let (mut em, _c1, c2, rec) = wck_lost_update();
    let r = em.regions.pop().unwrap();
    (c2, r, rec)
}

/// **EM-INIT-POST-RESTORES-CHECKPOINTED** — REFUTED (INDEPENDENT; root cause
/// CKPT-SNAPSHOT-RACE, as EM-CKPT-NO-LOST-UPDATE). "After a successful checkpoint and a
/// restart, initialize() restores every extent that was published before that checkpoint
/// ..." K2 (offset 4, size 4) was published before checkpoint() #2, which succeeded; after a
/// restart initialize() rebuilds the regions from the recovered state (recovery.rs:11-85,
/// lib.rs:552-568), in which no slot holds K2. (Without an interleaving the restore holds:
/// see the run_checkpoint/recover mirror contracts.)
#[ensures(result.0 == Ok(()))]
#[ensures(result.1.slabs@.contains(0) && result.1.slabs@.lookup(0).keys@[1] == WK2)]
#[ensures(match result.2 { Ok(img) => img_lacks_key(*img, WK2) && img.len() == 1, Err(_) => false })]
pub fn refute_em_init_post_restores_checkpointed() -> (Result<(), EmError>, RegionState, Result<Snapshot<Seq<RegionState>>, EmError>) {
    let (mut em, _c1, c2, rec) = wck_lost_update();
    let r = em.regions.pop().unwrap();
    (c2, r, rec)
}

/// Mutant: claims the restored state holds K2 in slot 1.
#[ensures(match result.2 { Ok(img) => img.len() == 1 && img[0].slabs@.contains(0) && img[0].slabs@.lookup(0).keys@[1] == WK2, Err(_) => false })]
pub fn refute_em_init_post_restores_checkpointed__mutant() -> (Result<(), EmError>, RegionState, Result<Snapshot<Seq<RegionState>>, EmError>) {
    let (mut em, _c1, c2, rec) = wck_lost_update();
    let r = em.regions.pop().unwrap();
    (c2, r, rec)
}

/// PREMATURE-RELEASE witness (C4): reserve K1 (wc4_one: one 1-slot slab at 0); publish(K1);
/// checkpoint() begins and serialises the region (K1 present); remove_extent(0) runs
/// before the checkpoint clears and flushes (no lock spans lib.rs:299 -> :316): the key
/// becomes FREE_KEY and (0, 0) is queued as a deferred free (region.rs:142-147); the
/// checkpoint completes successfully and its flush (lib.rs:318-321) releases the queued
/// slot: the slab empties and returns to the buddy allocator (region.rs:103-106). The next
/// reserve_extent(K2, 4) is handed offset 0 again — while the only persisted state still
/// records K1 at offset 0. Returns (checkpoint result, re-reservation, recovered state).
#[ensures(result.0 == Ok(()))]
#[ensures(match result.1 { Ok((s, i, off)) => s@ == 0 && i@ == 0 && off@ == 0, Err(_) => false })]
#[ensures(match result.2 { Ok(img) => img_lists(*img, Extent { key: WK1, size: 4u32, offset: 0u64 }), Err(_) => false })]
pub fn wck_released() -> (Result<(), EmError>, Result<(u64, usize, u64), EmError>, Result<Snapshot<Seq<RegionState>>, EmError>) {
    let r = wc4_one();
    let r0 = snapshot! { r };
    let mut rs: Vec<RegionState> = Vec::new();
    rs.push(r);
    let mut em = CkEm { regions: rs, active: 0, seq: 0, dev: wc_fresh_dev() };
    proof_assert! { em.regions@.len() == 1 && em.regions@[0] == *r0 };
    // LEVEL-3 reachable start (as wck_lost_update, C4 geometry, live handle K1).
    let sb0 = l3_sb_c4();
    proof_assert! { l3_fmt_ranges(wc_fp_l(4u64, 4u64), 3, 4096, sb0) && l3_rsv_ranges(4, wc_fp_l(4u64, 4u64)) };
    proof_assert! { rg_l3(em.regions@[0], hs_w1()) && regions_ok(em.regions@) };
    proof_assert! { em.active@ == sb0.active_copy@ && em.seq@ == sb0.checkpoint_seq@ && em.dev.sb_seq@ == sb0.checkpoint_seq@ && em.dev.sb_active@ == sb0.active_copy@ };
    // publish(K1)
    em.regions[0].publish_slot(0, 0, WK1);
    let s1 = snapshot! { em.regions@[0].slabs@.lookup(0) };
    proof_assert! { s1.keys@[0] == WK1 && s1.bitmap.num_slots@ == 1 && slot_bit(s1.bitmap, 0) };
    proof_assert! { regions_ok(em.regions@) };
    // checkpoint() phase 1: snapshot with K1 at slot 0
    let img = match em.ck_begin() {
        Some(img) => img,
        None => return (Err(EmError::IoError), Err(EmError::IoError), Err(EmError::IoError)),
    };
    // remove_extent(0) interleaves (region_for_offset -> region 0, lib.rs:680-684)
    let pre_rm = snapshot! { em.regions@[0] };
    proof_assert! { slot_off(*s1, 0) == 0 && s1.start_offset@ == 0 };
    proof_assert! { rm_case(*pre_rm, 0u64, 0u64, 0usize) };
    let rm = em.regions[0].remove_extent_by_offset(0);
    proof_assert! { rm == Ok(()) && rm_post(*pre_rm, em.regions@[0], 0u64, 0usize) };
    proof_assert! { em.regions@[0].pending_frees@ == Seq::singleton((0u64, 0usize)) };
    proof_assert! { rg_inv(em.regions@[0]) };
    proof_assert! { pending_ok(em.regions@[0]) };
    // checkpoint() phases 2-4 + superblock write succeed; then clear dirty + flush
    let c = em.ck_write(img, true, true);
    proof_assert! { c == Ok(()) };
    let live = snapshot! { em.regions@[0] };
    proof_assert! { live.slabs@.lookup(0).bitmap.allocated_count@ == 1 };
    em.ck_finish();
    proof_assert! { free_case(*live, em.regions@[0], 0u64, 0usize, live.slabs@.lookup(0)) };
    proof_assert! { slab_k(*live) == 0 && live.buddy.max_order@ == 0 && live.buddy.free_lists@[0]@.len() == 0 };
    proof_assert! { em.regions@[0].buddy.free_lists@[0]@.len() == 1 && em.regions@[0].buddy.free_lists@[0]@[0]@ == 0 };
    proof_assert! { forall<k: Int> !em.regions@[0].slabs@.contains(k) };
    // reserve_extent(K2, 4): size class 4 is empty -> fresh slab from the buddy allocator
    proof_assert! { forall<e: Int> sc_get(em.regions@[0].size_classes, e) == Seq::empty() };
    proof_assert! { align_l(4, 4) == 4 && slots_of(4, 4) == 1 && slab_k(em.regions@[0]) == 0 };
    proof_assert! { first_ne(em.regions@[0].buddy.free_lists@, 0, 0) == 0 };
    let before = snapshot! { em.regions@[0] };
    let again = em.regions[0].alloc_extent(4);
    proof_assert! { fresh_case(*before, em.regions@[0], 4, again) };
    let rec = recover(&em.dev);
    proof_assert! { em.dev.sb_seq@ == 1 && em.dev.sb_active@ == 1 };
    proof_assert! { match em.dev.copy1 { Some(cc) => cc.seq@ == 1 && cc.img == img, None => false } };
    proof_assert! { img.len() == 1 && img[0].slabs@.contains(0) && img[0].slabs@.lookup(0) == *s1 };
    proof_assert! { from_slot(img[0], 0, 0, Extent { key: WK1, size: 4u32, offset: 0u64 }) };
    (c, again, rec)
}

/// **EM-REMOVE-DEFERRED-FREE** — REFUTED (INDEPENDENT; root cause CKPT-SNAPSHOT-RACE, as
/// EM-CKPT-NO-LOST-UPDATE: lib.rs:318-321 flushes EVERY queued free, including those queued
/// after checkpoint.rs:51-55 serialised the region). "After an extent is removed, no part of
/// its disk slot is handed out by any reservation ... until a checkpoint that records the
/// removal (one that began after it) has completed successfully." In `wck_released` the
/// only checkpoint began BEFORE the removal and persisted K1 as live, yet after it
/// completes reserve_extent hands out the removed slot (offset 0). Not a WORDING ARTIFACT:
/// the audit's suggested form ("a completed checkpoint whose persisted state records the
/// removal") fails too — the persisted state lists K1 at offset 0.
#[ensures(result.0 == Ok(()))]
#[ensures(match result.1 { Ok((_, _, off)) => off@ == 0, Err(_) => false })]
#[ensures(match result.2 { Ok(img) => img_lists(*img, Extent { key: WK1, size: 4u32, offset: 0u64 }), Err(_) => false })]
pub fn refute_em_remove_deferred_free() -> (Result<(), EmError>, Result<(u64, usize, u64), EmError>, Result<Snapshot<Seq<RegionState>>, EmError>) {
    wck_released()
}

/// Mutant: claims the removed slot is not handed out again.
#[ensures(match result.1 { Ok((_, _, off)) => off@ != 0, Err(_) => true })]
pub fn refute_em_remove_deferred_free__mutant() -> (Result<(), EmError>, Result<(u64, usize, u64), EmError>, Result<Snapshot<Seq<RegionState>>, EmError>) {
    wck_released()
}

/// **EM-CKPT-RELEASE-ONLY-PERSISTED-REMOVALS** — REFUTED (INDEPENDENT; root cause
/// CKPT-SNAPSHOT-RACE). "A checkpoint makes reusable only the space of removals that are
/// recorded in the checkpoint it persisted." The checkpoint of `wck_released` persisted K1
/// as LIVE at offset 0 (the removal is not recorded), yet it made offset 0 reusable: the
/// next reservation receives it.
#[ensures(result.0 == Ok(()))]
#[ensures(match result.2 { Ok(img) => img_lists(*img, Extent { key: WK1, size: 4u32, offset: 0u64 }), Err(_) => false })]
#[ensures(match result.1 { Ok((s, _, off)) => s@ == 0 && off@ == 0, Err(_) => false })]
pub fn refute_em_ckpt_release_only_persisted_removals() -> (Result<(), EmError>, Result<(u64, usize, u64), EmError>, Result<Snapshot<Seq<RegionState>>, EmError>) {
    wck_released()
}

/// Mutant: claims the space stays unavailable (the reservation fails).
#[ensures(match result.1 { Ok(_) => false, Err(_) => true })]
pub fn refute_em_ckpt_release_only_persisted_removals__mutant() -> (Result<(), EmError>, Result<(u64, usize, u64), EmError>, Result<Snapshot<Seq<RegionState>>, EmError>) {
    wck_released()
}

/// **EM-CKPT-CRASH-ATOMIC** — REFUTED (INDEPENDENT; root cause CKPT-SB-ORDER:
/// write_checkpoint flips the IN-MEMORY active copy and sequence (checkpoint.rs:105-112)
/// BEFORE run_checkpoint's superblock write (lib.rs:302-307) is known to have succeeded,
/// and nothing restores them when it fails). "At every point in time, including in the
/// middle of a checkpoint, the superblock on the metadata device is valid and names an
/// active checkpoint copy that is complete and passes its CRC check, so a crash at any
/// moment recovers to either the previous completed checkpoint or the new one, never ...
/// an error." Witness (C8, device obeying IBlockDevice: every write completes or returns
/// an error): reserve K1, K2; publish K1; checkpoint #1 succeeds (copy 1 = seq 1,
/// superblock -> seq 1 / copy 1); publish K2; checkpoint #2: copy 0 = seq 2 written, the
/// SUPERBLOCK write fails (in memory now seq 2 / copy 0); checkpoint #3 (still dirty):
/// its copy write targets `1 - 0` = copy 1 — the copy the ON-DISK superblock names — and
/// overwrites it with seq 3; a crash before (or a failure of) #3's superblock write leaves
/// the superblock naming copy 1 with seq 1 while copy 1 holds seq 3: recovery
/// (recovery.rs:29-70) rejects the active copy, has no fallback (prev seq 0) and initialize
/// fails with CorruptMetadata.
#[ensures(result.0.sb_seq@ == 1 && result.0.sb_active@ == 1)]
#[ensures(match result.0.copy1 { Some(c) => c.seq@ == 3, None => false })]
#[ensures(match result.1 { Ok(_) => false, Err(_) => true })]
pub fn refute_em_ckpt_crash_atomic() -> (CkDev, Result<Snapshot<Seq<RegionState>>, EmError>) {
    let r = wc8_two();
    let r0 = snapshot! { r };
    let mut rs: Vec<RegionState> = Vec::new();
    rs.push(r);
    let mut em = CkEm { regions: rs, active: 0, seq: 0, dev: wc_fresh_dev() };
    proof_assert! { em.regions@.len() == 1 && em.regions@[0] == *r0 };
    // LEVEL-3 reachable start (as wck_lost_update).
    let sb0 = l3_sb_c8();
    proof_assert! { l3_fmt_ranges(wc_fp_l(8u64, 8u64), 3, 4096, sb0) && l3_rsv_ranges(4, wc_fp_l(8u64, 8u64)) };
    proof_assert! { rg_l3(em.regions@[0], hs_w2()) && regions_ok(em.regions@) };
    proof_assert! { em.active@ == sb0.active_copy@ && em.seq@ == sb0.checkpoint_seq@ && em.dev.sb_seq@ == sb0.checkpoint_seq@ && em.dev.sb_active@ == sb0.active_copy@ };
    em.regions[0].publish_slot(0, 0, WK1);
    proof_assert! { regions_ok(em.regions@) && !all_clean(em.regions@) };
    // checkpoint #1: both writes succeed
    let c1 = em.run_checkpoint(true, true);
    proof_assert! { c1 == Ok(()) && em.seq@ == 1 && em.active@ == 1 && em.dev.sb_seq@ == 1 && em.dev.sb_active@ == 1 };
    proof_assert! { regions_ok(em.regions@) };
    em.regions[0].publish_slot(0, 1, WK2);
    proof_assert! { regions_ok(em.regions@) && !all_clean(em.regions@) };
    // checkpoint #2: copy write succeeds, superblock write fails
    let c2 = em.run_checkpoint(true, false);
    proof_assert! { c2 != Ok(()) && em.seq@ == 2 && em.active@ == 0 && em.dev.sb_seq@ == 1 && em.dev.sb_active@ == 1 };
    proof_assert! { !all_clean(em.regions@) && regions_ok(em.regions@) };
    // checkpoint #3: copy write succeeds; the superblock write fails / the system crashes
    let c3 = em.run_checkpoint(true, false);
    proof_assert! { c3 != Ok(()) && em.seq@ == 3 && em.active@ == 1 && em.dev.sb_seq@ == 1 && em.dev.sb_active@ == 1 };
    proof_assert! { match em.dev.copy1 { Some(c) => c.seq@ == 3, None => false } };
    let rec = recover(&em.dev);
    (em.dev, rec)
}

/// Mutant: claims recovery succeeds.
#[ensures(match result.1 { Ok(_) => true, Err(_) => false })]
pub fn refute_em_ckpt_crash_atomic__mutant() -> (CkDev, Result<Snapshot<Seq<RegionState>>, EmError>) {
    let r = wc8_two();
    let r0 = snapshot! { r };
    let mut rs: Vec<RegionState> = Vec::new();
    rs.push(r);
    let mut em = CkEm { regions: rs, active: 0, seq: 0, dev: wc_fresh_dev() };
    proof_assert! { em.regions@.len() == 1 && em.regions@[0] == *r0 };
    em.regions[0].publish_slot(0, 0, WK1);
    proof_assert! { regions_ok(em.regions@) && !all_clean(em.regions@) };
    let c1 = em.run_checkpoint(true, true);
    proof_assert! { regions_ok(em.regions@) };
    em.regions[0].publish_slot(0, 1, WK2);
    proof_assert! { regions_ok(em.regions@) && !all_clean(em.regions@) };
    let c2 = em.run_checkpoint(true, false);
    let c3 = em.run_checkpoint(true, false);
    let rec = recover(&em.dev);
    (em.dev, rec)
}

// =============================================================================
// Metadata block I/O (model/blockio.rs)
// =============================================================================

/// **EM-BIO-WRITE-LAYOUT** — "A successful metadata write stores the given bytes, followed
/// by zero padding up to the next whole sector, in consecutive blocks starting at the base
/// LBA plus the requested LBA, on the configured namespace." Mirror of
/// `BlockDeviceClient::write_blocks` (block_io.rs:49-108) for every client, LBA and payload:
/// on success, block `i` (0 <= i < ceil(len / sector)) at (ns_id, base_lba + lba + i) holds
/// bytes `i*ss .. (i+1)*ss` of the payload padded with zeros. Every superblock and
/// checkpoint write of format / initialize / checkpoint goes through it (lib.rs:498, 306,
/// checkpoint.rs:97).
#[requires(c.sector_size@ > 0)]
#[requires(data@.len() + c.sector_size@ <= usize::MAX@)]
#[requires(c.base_lba@ + lba@ + (data@.len() + c.sector_size@ - 1) / c.sector_size@ <= u64::MAX@)]
#[requires((data@.len() + c.sector_size@ - 1) / c.sector_size@ * c.sector_size@ <= usize::MAX@)]
#[ensures(match result {
    Ok(()) => forall<i: Int> 0 <= i && i < (data@.len() + c.sector_size@ - 1) / c.sector_size@ ==>
        match (*(^dev).store).get((c.ns_id@, c.base_lba@ + lba@ + i)) {
            Some(blk) => blk.len() == c.sector_size@
                && forall<j: Int> 0 <= j && j < c.sector_size@ ==>
                    blk[j] == if i * c.sector_size@ + j < data@.len() { data@[i * c.sector_size@ + j] } else { 0u8 },
            None => false,
        },
    Err(_) => true,
})]
pub fn verify_em_bio_write_layout(c: &Client, dev: &mut BDev, lba: u64, data: &Vec<u8>) -> Result<(), EmError> {
    c.write_blocks(dev, lba, data)
}

/// Mutant: claims the padding is not zero (byte `len` of the first block is 1).
#[requires(c.sector_size@ > 1)]
#[requires(data@.len() == 1)]
#[requires(data@.len() + c.sector_size@ <= usize::MAX@)]
#[requires(c.base_lba@ + lba@ + (data@.len() + c.sector_size@ - 1) / c.sector_size@ <= u64::MAX@)]
#[requires((data@.len() + c.sector_size@ - 1) / c.sector_size@ * c.sector_size@ <= usize::MAX@)]
#[ensures(match result {
    Ok(()) => match (*(^dev).store).get((c.ns_id@, c.base_lba@ + lba@)) { Some(blk) => blk[1] == 1u8, None => false },
    Err(_) => true,
})]
pub fn verify_em_bio_write_layout__mutant(c: &Client, dev: &mut BDev, lba: u64, data: &Vec<u8>) -> Result<(), EmError> {
    c.write_blocks(dev, lba, data)
}

/// **EM-SETMBASE-POST-SHIFTS-METADATA-IO** — "After set_metadata_base_lba(b), every
/// subsequent metadata-device read and write (superblock and both checkpoint regions) by
/// format, initialize and checkpoint targets the block address it would use with a base of
/// zero plus b, so the superblock lives at block b." Each of format / initialize /
/// checkpoint builds a FRESH client with `get_metadata_client` (lib.rs:496, 518, 297),
/// which captures the current `metadata_base_lba` (lib.rs:200); every command of
/// write_blocks / read_blocks goes to `base_lba + lba + i` (block_io.rs:69, 119). Proved
/// for every component state, base b, namespace, sector size, and any write (LBA `lba`)
/// followed by any read (LBA `rlba`) on that client: the device's command log records
/// exactly the shifted addresses, in order (lba 0 = the superblock -> block b).
#[requires(ss@ > 0)]
#[requires(data@.len() + ss@ <= usize::MAX@ && (data@.len() + ss@ - 1) / ss@ * ss@ <= usize::MAX@)]
#[requires(b@ + lba@ + (data@.len() + ss@ - 1) / ss@ <= u64::MAX@)]
#[requires(rn@ + ss@ <= usize::MAX@ && b@ + rlba@ + (rn@ + ss@ - 1) / ss@ <= u64::MAX@)]
#[ensures((^em).metadata_base_lba == b)]
#[ensures(forall<p: Int> (*dev.log).len() <= p && p < *result.2 ==>
    (*(^dev).log)[p] == (ns@, b@ + lba@ + (p - (*dev.log).len())))]
#[ensures(forall<p: Int> *result.2 <= p && p < (*(^dev).log).len() ==>
    (*(^dev).log)[p] == (ns@, b@ + rlba@ + (p - *result.2)))]
#[ensures((*dev.log).len() <= *result.2 && *result.2 <= (*(^dev).log).len())]
pub fn verify_em_setmbase_post_shifts_metadata_io(
    em: &mut ExtentManager, b: u64, ns: u32, ss: u32, dev: &mut BDev, lba: u64, data: &Vec<u8>, rlba: u64, rn: usize,
) -> (Result<(), EmError>, Result<Vec<u8>, EmError>, Snapshot<Int>) {
    em.set_metadata_base_lba(b);
    let c = em.get_metadata_client(ns, ss);
    let w = c.write_blocks(dev, lba, data);
    let mid = snapshot! { (*dev.log).len() };
    let r = c.read_blocks(dev, rlba, rn);
    (w, r, mid)
}

/// Mutant: claims the write still targets the un-shifted address (base 0).
#[requires(ss@ > 0 && b@ > 0)]
#[requires(data@.len() + ss@ <= usize::MAX@ && (data@.len() + ss@ - 1) / ss@ * ss@ <= usize::MAX@)]
#[requires(b@ + lba@ + (data@.len() + ss@ - 1) / ss@ <= u64::MAX@)]
#[requires(data@.len() == 1)]
#[ensures(forall<p: Int> (*dev.log).len() <= p && p < (*(^dev).log).len() ==> (*(^dev).log)[p] == (ns@, lba@ + (p - (*dev.log).len())))]
pub fn verify_em_setmbase_post_shifts_metadata_io__mutant(
    em: &mut ExtentManager, b: u64, ns: u32, ss: u32, dev: &mut BDev, lba: u64, data: &Vec<u8>,
) -> Result<(), EmError> {
    em.set_metadata_base_lba(b);
    let c = em.get_metadata_client(ns, ss);
    c.write_blocks(dev, lba, data)
}

// =============================================================================
// Superblock byte layout (model/sbbytes.rs)
// =============================================================================

/// **EM-INV-SUPERBLOCK-ROUNDTRIP** — "Any superblock the component writes is accepted when
/// decoded (and by initialize()) and reads back with exactly the field values that were
/// written." Every superblock the component writes carries SUPERBLOCK_MAGIC
/// (Superblock::new, superblock.rs:41; later updates touch only active_copy and
/// checkpoint_seq, checkpoint.rs:109-110); for every such superblock,
/// `deserialize(serialize(sb))` (superblock.rs:58-168) is `Ok(sb)` — every field, including
/// the CRC check. initialize() decodes the superblock with exactly this function
/// (recovery.rs:15-16).
#[requires(sb.magic == SUPERBLOCK_MAGIC)]
#[ensures(result == Ok(*sb))]
pub fn verify_em_inv_superblock_roundtrip(sb: &Superblock) -> Result<Superblock, EmError> {
    let bytes = sb.serialize();
    Superblock::deserialize(&bytes)
}

/// Mutant: claims the decoder rejects what was written.
#[requires(sb.magic == SUPERBLOCK_MAGIC)]
#[ensures(result != Ok(*sb))]
pub fn verify_em_inv_superblock_roundtrip__mutant(sb: &Superblock) -> Result<Superblock, EmError> {
    let bytes = sb.serialize();
    Superblock::deserialize(&bytes)
}

/// **EM-INV-SUPERBLOCK-WELL-FORMED** — "Every superblock the component writes is exactly
/// 4096 bytes with all fields and the checksum inside it, begins with the magic value
/// 0x4345_5254_5553_5634 ("CERTUSV4") and version 6, has an active-copy field of 0 or 1, has
/// zero-filled reserved bytes and padding, and stores its CRC32 at byte 92." The component
/// writes three kinds of superblock: format's Superblock::new (lib.rs:487-502), the one
/// checkpoint updates in memory (checkpoint.rs:105-112) and writes (lib.rs:302-307), and —
/// after initialize — the decoded one it keeps and later updates (lib.rs:574). All three
/// satisfy `sb_wf` (magic, version 6, active copy <= 1) — the decoded one because it IS the
/// written one (round-trip) — and serialize maps every `sb_wf` superblock to bytes in
/// exactly the stated layout (`sb_layout`: 4096 bytes, fields at their offsets, reserved
/// 49..56 and padding 96..4096 zero, crc32(bytes 0..92) at 92).
#[requires(sb_wf(*sb) && a_9c0757(sb.checkpoint_seq@))] // LEVEL-3 D-B1: seq < MAX is D-RANGE-FR-003-9c0757
#[ensures(sb_wf(result.0) && sb_layout(result.1@, result.0))]
#[ensures(sb_layout(result.2@, *sb) && result.2@[48]@ <= 1)]
#[ensures(has64(result.2@, 0, SUPERBLOCK_MAGIC) && has32(result.2@, 8, FORMAT_VERSION))]
#[ensures(sb_wf(result.3) && sb_layout(result.4@, result.3))]
#[ensures(result.5 == Ok(*sb))]
pub fn verify_em_inv_superblock_well_formed(
    sb: &Superblock, dds: u64, ss: u32, slab: u64, mes: u32, rc: u32, cro: u64, crs: u64, iid: u64, ns: u32, dso: u64,
) -> (Superblock, Vec<u8>, Vec<u8>, Superblock, Vec<u8>, Result<Superblock, EmError>) {
    // format(): Superblock::new + serialize (lib.rs:487-502)
    let s0 = Superblock::new(dds, ss, slab, mes, rc, cro, crs, iid, ns, dso);
    let b0 = s0.serialize();
    // any superblock the component holds: its serialization
    let b1 = sb.serialize();
    // checkpoint: `active_copy = 1 - active_copy; checkpoint_seq = new_seq` (checkpoint.rs:105-112)
    let mut s2 = *sb;
    s2.active_copy = 1 - sb.active_copy;
    s2.checkpoint_seq = sb.checkpoint_seq + 1;
    let b2 = s2.serialize();
    // initialize(): the superblock it keeps is the decoded bytes (recovery.rs:15-16)
    let back = Superblock::deserialize(&b1);
    (s0, b0, b1, s2, b2, back)
}

/// Mutant: claims the reserved bytes carry the active copy.
#[requires(sb_wf(*sb) && sb.active_copy@ == 1)]
#[ensures(result@[49] == 1u8)]
pub fn verify_em_inv_superblock_well_formed__mutant(sb: &Superblock) -> Vec<u8> {
    sb.serialize()
}

// =============================================================================
// Size classes
// =============================================================================

/// **EM-SIZECLASS-NONFULL-LISTED** — "Every slab that still has a free slot is listed under
/// its element size, so its free slots can be found by later reservations." Maintained
/// invariant `nonfull_listed` (model/region.rs): established by format (RegionState::new,
/// no slabs) and by initialize (rebuild_region lists every recovered slab, lib.rs:563), and
/// preserved by every region mutation reachable from the public API (ROp: reserve ->
/// alloc_extent incl. its stale-entry loop and the fresh-slab path, abort/drop/publish(FREE)
/// -> free_slot, publish -> publish_slot, remove -> remove_extent_by_offset, checkpoint ->
/// flush_pending_frees). It holds even across the non-power-of-two-sector replacement of
/// W2/W3 (the replacement slab is listed under its own size).
#[requires(rg_inv(*r) && rop_pre(*r, op) && nonfull_listed(*r))]
// LEVEL-3 D-B1: the fresh region is built with the format parameters' own sector size (as
// lib.rs:465 does), so the separate `sector_size > 0` premise is gone.
#[requires(rg_in(base@, total@, fp))]
#[requires(forall<p: Int> 0 <= p && p < descs@.len() ==> desc_ok_a(descs@[p], base@, total@, fp))]
#[ensures(nonfull_listed(^r))]
#[ensures(nonfull_listed(result.0) && nonfull_listed(result.1))]
pub fn verify_em_sizeclass_nonfull_listed(
    r: &mut RegionState, op: ROp, base: u64, total: u64, fp: FormatParams, descs: &Vec<SlabDescriptor>,
) -> (RegionState, RegionState) {
    proof_assert! { lemma_rg_in(base@, total@, fp); rg_hd(base@, total@, fp) };
    proof_assert! { forall<p: Int> 0 <= p && p < descs@.len() ==> { lemma_desc_ok_a(descs@[p], base@, total@, fp); desc_ok(descs@[p], base@, total@, fp) } };
    // format(): a fresh region (lib.rs:465-467)
    let fresh = RegionState::new(BuddyAllocator::new(base, total, fp.sector_size), fp);
    // initialize(): the per-region rebuild (lib.rs:552-568)
    let rebuilt = rebuild_region(base, total, fp, descs);
    // one public-API mutation
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
    (fresh, rebuilt)
}

/// Mutant: claims some mutation unlists a non-full slab.
#[requires(rg_inv(*r) && rop_pre(*r, op) && nonfull_listed(*r))]
#[ensures(!nonfull_listed(^r))]
pub fn verify_em_sizeclass_nonfull_listed__mutant(r: &mut RegionState, op: ROp) {
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

// =============================================================================
// Listing after a non-power-of-two-sector recovery (BUDDY-MASK cascades W2 / W4)
// =============================================================================

/// **EM-GETEXT-POST-EXACT** — REFUTED. "get_extents() returns exactly the extents that have
/// been published (or recovered) ... and not since removed, each exactly once ..." ROOT
/// CAUSE = BUDDY-MASK (non-power-of-two sector size). W2 (sector 3, slab 6, region
/// [0,12)): initialize recovers slab B at 6 holding the published K3 (w2_recovered) but
/// leaves its block free (buddy.rs:136); reserve(K4,3), reserve(K5,6), reserve(K6,3) then
/// carve a fresh slab at 6 that REPLACES B (region.rs:89, w2a_replaced). K3 was never
/// removed, yet get_extents' per-region loop (lib.rs:641-654, mirror `region_extents`) lists
/// nothing with key K3 — the replacement slabs hold only FREE keys.
#[ensures(result.0.slabs@.contains(6) && result.0.slabs@.lookup(6).keys@[0] == W_K3 && W_K3 != FREE_KEY)]
#[ensures(forall<p: Int> 0 <= p && p < result.1@.len() ==> result.1@[p].key != W_K3)]
pub fn support_old_refute_em_getext_post_exact() -> (RegionState, Vec<Extent>) {
    let r0 = w2_recovered();
    let (r, _s4, _i4, _off4) = w2a_replaced();
    proof_assert! { forall<k: Int> r.slabs@.contains(k) ==> slab_inv(r.slabs@.lookup(k)) };
    proof_assert! { forall<k: Int, j: Int, e: Extent> from_slot(r, k, j, e) ==> e.key == FREE_KEY && e.key != FREE_KEY };
    let mut out: Vec<Extent> = Vec::new();
    region_extents(&r, &mut out);
    (r0, out)
}

/// Mutant: claims the recovered K3 is listed.
#[ensures(exists<p: Int> 0 <= p && p < result.1@.len() && result.1@[p].key == W_K3)]
pub fn support_old_refute_em_getext_post_exact__mutant() -> (RegionState, Vec<Extent>) {
    let r0 = w2_recovered();
    let (r, _s4, _i4, _off4) = w2a_replaced();
    let mut out: Vec<Extent> = Vec::new();
    region_extents(&r, &mut out);
    (r0, out)
}

pub const W_K5: u64 = 5;

/// W4 (W3 geometry: sector 3, slab 12, region [0,24); recovered Y = (12, 12, elem 6,
/// [K2, FREE]) left on the free lists by buddy.rs:136): reserve(K5, 6) takes Y's free slot
/// 1 (offset 18, size 6) -> H5; reserve(K3, 12) carves a slab at 0; reserve(K4, 3) carves a
/// fresh element-3 slab Z at 12, REPLACING Y while H5 is live; H5.publish() writes K5 into
/// slot 1 of Z (lib.rs:610 -> region.rs:114-119). Returns (region, H5 offset, H5 size).
#[ensures(rg_inv(result.0))]
#[ensures(result.1@ == 18 && result.2@ == 6)]
#[ensures(forall<k: Int, j: Int, e: Extent> from_slot(result.0, k, j, e) && e.key == W_K5 ==> e.offset@ == 15 && e.size@ == 3)]
pub fn w4_published() -> (RegionState, u64, u32) {
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
    let y = snapshot! { r.slabs@.lookup(12) };
    proof_assert! { y.bitmap.num_slots@ == 2 && slot_bit(y.bitmap, 0) && !slot_bit(y.bitmap, 1) };
    proof_assert! { cnt(y.bitmap, 0) == 0 && cnt(y.bitmap, 1) == 1 && cnt(y.bitmap, 2) == 1 };
    proof_assert! { r.buddy.max_order@ == 3 && r.buddy.free_lists@[3]@ == Seq::singleton(0u64) && r.buddy.free_lists@[2]@.len() == 0 };
    proof_assert! { slab_k(r) == 2 };
    proof_assert! { align_l(6, 3) == 6 && align_l(12, 3) == 12 && align_l(3, 3) == 3 };
    // reserve(K5, 6): size class 6 lists Y, which has a free slot -> slot 1, offset 18
    let o1 = snapshot! { r };
    proof_assert! { sc_get(r.size_classes, 6) == Seq::singleton(12u64) && exist_cond(*o1, 6, 12u64) };
    let h5 = r.alloc_extent(6);
    proof_assert! { exist_case(*o1, r, 6, 12u64, h5) };
    let (h5_off, h5_size) = match h5 {
        Ok((s, i, off)) => {
            proof_assert! { s@ == 12 && i@ < 2 && !slot_bit(y.bitmap, i@) };
            proof_assert! { i@ == 1 && off@ == slot_off(*y, 1) && off@ == 18 };
            (off, ((6u32 + 3 - 1) / 3 * 3)) // aligned_size (lib.rs:591)
        }
        Err(_) => (0, 0),
    };
    proof_assert! { filter_ne(Seq::singleton(12u64), 12u64) == Seq::empty() };
    proof_assert! { sc_get(r.size_classes, 12) == Seq::empty() && sc_get(r.size_classes, 3) == Seq::empty() };
    proof_assert! { r.buddy.free_lists@[3]@ == Seq::singleton(0u64) && r.buddy.free_lists@[2]@.len() == 0 };
    proof_assert! { first_ne(r.buddy.free_lists@, 2, 3) == 3 };
    // reserve(K3, 12): fresh slab at 0; [12, 24) pushed on order 2
    let o2 = snapshot! { r };
    let res2 = r.alloc_extent(12);
    proof_assert! { fresh_case(*o2, r, 12, res2) };
    proof_assert! { match res2 { Ok((d, _, _)) => d@ == 0, Err(_) => false } };
    proof_assert! { r.buddy.free_lists@[2]@ == Seq::singleton(12u64) };
    proof_assert! { first_ne(r.buddy.free_lists@, 2, 3) == 2 };
    proof_assert! { sc_get(r.size_classes, 3) == Seq::empty() };
    // reserve(K4, 3): fresh element-3 slab Z at 12, replacing Y
    let o3 = snapshot! { r };
    let res3 = r.alloc_extent(3);
    proof_assert! { fresh_case(*o3, r, 3, res3) };
    proof_assert! { match res3 { Ok((d, _, _)) => d@ == 12, Err(_) => false } };
    let z = snapshot! { r.slabs@.lookup(12) };
    proof_assert! { z.element_size@ == 3 && z.start_offset@ == 12 && z.bitmap.num_slots@ == 4 && z.keys@.len() == 4 };
    proof_assert! { forall<j: Int> 0 <= j && j < 4 ==> z.keys@[j] == FREE_KEY };
    proof_assert! { forall<k: Int> r.slabs@.contains(k) ==> k == 0 || k == 12 };
    proof_assert! { forall<j: Int> 0 <= j && j < r.slabs@.lookup(0).keys@.len() ==> r.slabs@.lookup(0).keys@[j] == FREE_KEY };
    // H5.publish(): publish_slot(12, 1, K5) on Z
    r.publish_slot(12, 1, W_K5);
    proof_assert! { r.slabs@.lookup(12).keys@ == z.keys@.set(1, W_K5) };
    proof_assert! { r.slabs@.lookup(12).element_size@ == 3 && r.slabs@.lookup(12).start_offset@ == 12 };
    proof_assert! { forall<k: Int> r.slabs@.contains(k) ==> k == 0 || k == 12 };
    proof_assert! { W_K5 != FREE_KEY };
    (r, h5_off, h5_size)
}

/// **EM-PUBLISH-POST-VISIBLE** — REFUTED. "After publish() succeeds on a handle whose key
/// is not u64::MAX, get_extents() lists exactly one extent with that handle's key, offset
/// and size." ROOT CAUSE = BUDDY-MASK (non-power-of-two sector size). In W4, H5 = (K5,
/// offset 18, size 6); its publish returns Ok (lib.rs:614-619) but writes K5 into slot 1 of
/// the replacement slab Z, so get_extents lists K5 only as (offset 15, size 3) and lists NO
/// extent (K5, 18, 6).
#[ensures(result.1@ == 18 && result.2@ == 6)]
#[ensures(forall<p: Int> 0 <= p && p < result.0@.len() ==>
    !(result.0@[p].key == W_K5 && result.0@[p].offset@ == 18 && result.0@[p].size@ == 6))]
pub fn support_old_refute_em_publish_post_visible() -> (Vec<Extent>, u64, u32) {
    let (r, off, sz) = w4_published();
    let mut out: Vec<Extent> = Vec::new();
    region_extents(&r, &mut out);
    (out, off, sz)
}

/// Mutant: claims the handle's extent is listed.
#[ensures(exists<p: Int> 0 <= p && p < result.0@.len() && result.0@[p].key == W_K5 && result.0@[p].offset@ == 18)]
pub fn support_old_refute_em_publish_post_visible__mutant() -> (Vec<Extent>, u64, u32) {
    let (r, off, sz) = w4_published();
    let mut out: Vec<Extent> = Vec::new();
    region_extents(&r, &mut out);
    (out, off, sz)
}

// =============================================================================
// pow2 witnesses (unscored): the same scenarios with a POWER-OF-TWO sector size, where the
// buddy.rs:136 mask is the aligned block start — the BUDDY-MASK refutations do not occur.
// Each replays the refutation's scenario with every size scaled from sector 3 to sector 4
// and shows the property instance that the refutation violates holds.
// =============================================================================

/// `4 & !(8-1) == 0`, `8 & !(16-1) == 0`, `16 & !(32-1) == 0`: buddy.rs:136 for the
/// recovered slab of W1p / W2p / W3p finds the region's free block 0.
#[bitwise_proof]
#[ensures((4u64 & !(8u64 - 1u64)) == 0u64 && (8u64 & !(16u64 - 1u64)) == 0u64 && (16u64 & !(32u64 - 1u64)) == 0u64)]
pub fn fact_mask_pow2() {}

/// W1p (sector 4, slab 4, region [0,8); the W1 history; recovered slab B at 4): initialize's
/// BuddyAllocator::new(0, 8, 4) + mark_allocated(4, 4) splits the free block [0,8) and keeps
/// only [0,4) free.
#[ensures(result.0@ == 4)]
#[ensures(bd_inv(result.1) && result.1.base_offset@ == 0 && result.1.total_usable_size@ == 8)]
#[ensures(result.1.sector_size@ == 4 && result.1.max_order@ == 1 && result.1.free_lists@.len() == 2)]
#[ensures(result.1.free_lists@[1]@.len() == 0 && result.1.free_lists@[0]@ == Seq::singleton(0u64))]
pub fn w1p_recovered() -> (u64, BuddyAllocator) {
    let mut b = BuddyAllocator::new(0, 8, 4);
    proof_assert! { 1.pow2() == 2 && 8 / 4 == 2 };
    proof_assert! { b.max_order@ == 1 && b.free_lists@[1]@ == Seq::singleton(0u64) && b.free_lists@[0]@.len() == 0 };
    proof_assert! { lemma_ord_eq(1, 0, 0); ord(4 / 4) == 0 };
    fact_mask_pow2();
    proof_assert! { forall<sp: u64> sp@ == span(4, 1) ==> sp == 8u64 };
    proof_assert! { split1_pre(b, 4u64, 0, 0u64) };
    let b0 = snapshot! { b };
    b.mark_allocated(4, 4);
    proof_assert! { split1_post(*b0, b, 4u64, 0, 0u64) && split1_val(*b0, 4u64, 0, 0u64) == 0 };
    proof_assert! { b.free_lists@[0]@.ext_eq(Seq::singleton(0u64)) };
    (4, b)
}

/// pow2 witness of EM-BUDDY-FREE-PRE: at W1p's `buddy.free(4, 4)` (region.rs:104) no free
/// block covers [4, 8) — the freed block is allocated.
#[ensures(forall<o: Int, k: Int> 0 <= o && o < 2 && 0 <= k && k < result.1.free_lists@[o]@.len() ==>
    !(result.1.free_lists@[o]@[k]@ <= result.0@ && result.0@ + span(4, 0) <= result.1.free_lists@[o]@[k]@ + span(4, o)))]
pub fn witness_em_buddy_free_pre_pow2() -> (u64, BuddyAllocator) {
    let r = w1p_recovered();
    proof_assert! { span(4, 0) == 4 && span(4, 1) == 8 };
    r
}

/// pow2 witness of EM-USED-RETURNS-AFTER-FREE: in W1p used_bytes() is 0 after format and 0
/// again after remove_extent(4) + checkpoint (free space 8 = capacity 8).
#[ensures(result.0@ == 8 && result.1@ == 8 && result.2@ == 8)]
pub fn witness_em_used_returns_after_free_pow2() -> (u64, u64, u64) {
    let fresh = BuddyAllocator::new(0, 8, 4);
    proof_assert! { 1.pow2() == 2 && 8 / 4 == 2 };
    proof_assert! { fresh.max_order@ == 1 && fresh.free_lists@[1]@ == Seq::singleton(0u64) && fresh.free_lists@[0]@.len() == 0 };
    proof_assert! { lemma_tf2(fresh.free_lists@, 4); tf(fresh.free_lists@, 4, 2) == 8 };
    let at_format = fresh.total_free();
    let (y, mut b) = w1p_recovered();
    proof_assert! { lemma_tf2(b.free_lists@, 4); tf(b.free_lists@, 4, 2) == 4 };
    proof_assert! { lemma_ord_eq(1, 0, 0); ord(4 / 4) == 0 && span(4, 0) == 4 };
    proof_assert! { span(4, 1) == 8 && 4 % span(4, 0) == 0 };
    b.free(y, 4); // the emptied slab B returns to the buddy allocator (region.rs:104)
    proof_assert! { tf(b.free_lists@, 4, 2) == 8 };
    let after = b.total_free();
    (at_format, b.total_usable_size, after)
}

pub const WP_K3: u64 = 3;

/// W2p format parameters (sector 4, slab 8, max extent 8, one region of 16 bytes).
#[ensures(result.data_disk_size@ == 16 && result.slab_size@ == 8 && result.sector_size@ == 4 && result.region_count@ == 1)]
pub fn w2p_fp() -> FormatParams {
    FormatParams { data_disk_size: 16, slab_size: 8, max_extent_size: 8, sector_size: 4, region_count: 1,
        metadata_alignment: 0, instance_id: Some(1), metadata_disk_ns_id: 1, metadata_region_size: 0 }
}

/// The W2p checkpointed slab B = (start 8, slab 8, element 4, keys [K3, FREE]).
#[ensures(result@.len() == 1)]
#[ensures(result@[0].start_offset@ == 8 && result@[0].slab_size@ == 8 && result@[0].element_size@ == 4)]
#[ensures(result@[0].keys@.len() == 2 && result@[0].keys@[0] == WP_K3 && result@[0].keys@[1] == FREE_KEY)]
pub fn w2p_descs() -> Vec<SlabDescriptor> {
    let mut keys: Vec<u64> = Vec::new();
    keys.push(WP_K3);
    keys.push(FREE_KEY);
    let mut v: Vec<SlabDescriptor> = Vec::new();
    v.push(SlabDescriptor { start_offset: 8, slab_size: 8, element_size: 4, keys });
    v
}

/// pow2 witness of EM-GETEXT-POST-EXACT: W2p (the W2 scenario at sector 4): after initialize,
/// reserve(K4,4) takes B's slot 1, reserve(K5,8) carves a slab at 0, and reserve(K6,4)
/// finds no space (OutOfSpace) instead of replacing B — so the recovered K3 still sits in
/// slot 0 of B, which get_extents' loop (lib.rs:641-654) visits and lists as (K3, 8, 4).
#[ensures(result.slabs@.contains(8) && result.slabs@.lookup(8).keys@[0] == WP_K3)]
#[ensures(from_slot(result, 8, 0, Extent { key: WP_K3, size: 4u32, offset: 8u64 }))]
pub fn witness_em_getext_post_exact_pow2() -> RegionState {
    let fp = w2p_fp();
    let descs = w2p_descs();
    fact_mask_pow2();
    proof_assert! { 2.pow2() == 4 && 1.pow2() == 2 && 16 / 4 == 4 };
    proof_assert! { lemma_ord_eq(2, 0, 1); ord(8 / 4) == 1 };
    proof_assert! { span(4, 1) == 8 && span(4, 2) == 16 && slots_of(8, 4) == 2 && slots_of(8, 8) == 1 };
    proof_assert! { desc_ok(descs@[0], 0, 16, fp) };
    proof_assert! { forall<sp: u64, x: u64> sp@ == span(4, 2) && x@ == 8 ==> sp == 16u64 && x == 8u64 };
    proof_assert! { mark_hit_p2(descs@[0], 0, 16, fp, 2) };
    let mut r = rebuild_region(0, 16, fp, &descs);
    proof_assert! { rebuilt_hit(r, descs@[0], 0, 16, fp) && hit_lists(r.buddy, descs@[0], 0, fp, 2) };
    proof_assert! { rebuilt_one(r, descs@[0], 0, 16, fp) && one_slab(r, descs@[0]) };
    let bb = snapshot! { r.slabs@.lookup(8) };
    proof_assert! { bb.bitmap.num_slots@ == 2 && slot_bit(bb.bitmap, 0) && !slot_bit(bb.bitmap, 1) };
    proof_assert! { cnt(bb.bitmap, 0) == 0 && cnt(bb.bitmap, 1) == 1 && cnt(bb.bitmap, 2) == 1 };
    proof_assert! { r.buddy.free_lists@[1]@ == Seq::singleton(0u64) || (r.buddy.free_lists@[1]@.len() == 1 && r.buddy.free_lists@[1]@[0]@ == 0) };
    proof_assert! { slab_k(r) == 1 && align_l(4, 4) == 4 && align_l(8, 4) == 8 };
    // reserve(K4, 4): B's free slot 1
    let o1 = snapshot! { r };
    proof_assert! { exist_cond(*o1, 4, 8u64) };
    let h4 = r.alloc_extent(4);
    proof_assert! { exist_case(*o1, r, 4, 8u64, h4) };
    proof_assert! { filter_ne(Seq::singleton(8u64), 8u64) == Seq::empty() };
    proof_assert! { sc_get(r.size_classes, 4) == Seq::empty() && sc_get(r.size_classes, 8) == Seq::empty() };
    proof_assert! { first_ne(r.buddy.free_lists@, 1, 2) == 1 };
    // reserve(K5, 8): fresh slab at 0
    let o2 = snapshot! { r };
    let h5 = r.alloc_extent(8);
    proof_assert! { fresh_case(*o2, r, 8, h5) };
    proof_assert! { match h5 { Ok((d, _, _)) => d@ == 0, Err(_) => false } };
    proof_assert! { r.buddy.free_lists@[1]@.len() == 0 && r.buddy.free_lists@[2]@.len() == 0 };
    proof_assert! { first_ne(r.buddy.free_lists@, 1, 2) == -1 };
    proof_assert! { sc_get(r.size_classes, 4) == Seq::empty() };
    // reserve(K6, 4): no free block of the slab order -> OutOfSpace, region unchanged
    let o3 = snapshot! { r };
    let h6 = r.alloc_extent(4);
    proof_assert! { fresh_case(*o3, r, 4, h6) && r == *o3 };
    proof_assert! { r.slabs@.contains(8) && r.slabs@.lookup(8).keys@[0] == WP_K3 && r.slabs@.lookup(8).element_size@ == 4 };
    proof_assert! { slot_off(r.slabs@.lookup(8), 0) == 8 && WP_K3 != FREE_KEY };
    r
}

pub const WP_K2: u64 = 2;

/// W3p format parameters (sector 4, slab 16, max extent 16, one region of 32 bytes).
#[ensures(result.data_disk_size@ == 32 && result.slab_size@ == 16 && result.sector_size@ == 4 && result.region_count@ == 1)]
pub fn w3p_fp() -> FormatParams {
    FormatParams { data_disk_size: 32, slab_size: 16, max_extent_size: 16, sector_size: 4, region_count: 1,
        metadata_alignment: 0, instance_id: Some(1), metadata_disk_ns_id: 1, metadata_region_size: 0 }
}

/// The W3p checkpointed slab Y = (start 16, slab 16, element 8, keys [K2, FREE]).
#[ensures(result@.len() == 1)]
#[ensures(result@[0].start_offset@ == 16 && result@[0].slab_size@ == 16 && result@[0].element_size@ == 8)]
#[ensures(result@[0].keys@.len() == 2 && result@[0].keys@[0] == WP_K2 && result@[0].keys@[1] == FREE_KEY)]
pub fn w3p_descs() -> Vec<SlabDescriptor> {
    let mut keys: Vec<u64> = Vec::new();
    keys.push(WP_K2);
    keys.push(FREE_KEY);
    let mut v: Vec<SlabDescriptor> = Vec::new();
    v.push(SlabDescriptor { start_offset: 16, slab_size: 16, element_size: 8, keys });
    v
}

/// W3p after initialize: Y recovered at 16 and listed in size class 8; the buddy allocator
/// holds only [0, 16) free (order 2).
#[ensures(rg_inv(result) && result.format_params.sector_size@ == 4 && result.format_params.slab_size@ == 16)]
#[ensures(result.buddy.max_order@ == 3 && result.buddy.free_lists@[3]@.len() == 0 && result.buddy.base_offset@ == 0)]
#[ensures(result.buddy.free_lists@[2]@.len() == 1 && result.buddy.free_lists@[2]@[0]@ == 0)]
#[ensures(result.buddy.free_lists@[1]@.len() == 0 && result.buddy.free_lists@[0]@.len() == 0)]
#[ensures(result.slabs@.contains(16) && forall<k: Int> result.slabs@.contains(k) ==> k == 16)]
#[ensures(result.slabs@.lookup(16).element_size@ == 8 && result.slabs@.lookup(16).start_offset@ == 16)]
#[ensures(result.slabs@.lookup(16).bitmap.num_slots@ == 2 && result.slabs@.lookup(16).bitmap.allocated_count@ == 1)]
#[ensures(slot_bit(result.slabs@.lookup(16).bitmap, 0) && !slot_bit(result.slabs@.lookup(16).bitmap, 1))]
#[ensures(result.slabs@.lookup(16).keys@[1] == FREE_KEY && result.slabs@.lookup(16).keys@.len() == 2)]
#[ensures(forall<e: Int> sc_get(result.size_classes, e) == if e == 8 { Seq::singleton(16u64) } else { Seq::empty() })]
pub fn w3p_recovered() -> RegionState {
    let fp = w3p_fp();
    let descs = w3p_descs();
    fact_mask_pow2();
    proof_assert! { 3.pow2() == 8 && 2.pow2() == 4 && 1.pow2() == 2 && 32 / 4 == 8 };
    proof_assert! { lemma_ord_eq(4, 0, 2); ord(16 / 4) == 2 };
    proof_assert! { span(4, 2) == 16 && span(4, 3) == 32 && slots_of(16, 8) == 2 };
    proof_assert! { desc_ok(descs@[0], 0, 32, fp) };
    proof_assert! { forall<sp: u64, x: u64> sp@ == span(4, 3) && x@ == 16 ==> sp == 32u64 && x == 16u64 };
    proof_assert! { mark_hit_p2(descs@[0], 0, 32, fp, 3) };
    let r = rebuild_region(0, 32, fp, &descs);
    proof_assert! { rebuilt_hit(r, descs@[0], 0, 32, fp) && hit_lists(r.buddy, descs@[0], 0, fp, 3) };
    proof_assert! { rebuilt_one(r, descs@[0], 0, 32, fp) && one_slab(r, descs@[0]) };
    let y = snapshot! { r.slabs@.lookup(16) };
    proof_assert! { y.bitmap.num_slots@ == 2 && slot_bit(y.bitmap, 0) && !slot_bit(y.bitmap, 1) };
    proof_assert! { cnt(y.bitmap, 0) == 0 && cnt(y.bitmap, 1) == 1 && cnt(y.bitmap, 2) == 1 };
    r
}

/// pow2 witness of EM-INV-SLAB-UNIFORM-ELEMENT: W3p — after initialize, reserve(K3,16) takes
/// the free [0,16) and reserve(K4,4) finds no space (OutOfSpace) instead of replacing Y;
/// size class 8 lists only Y, whose element size is 8.
#[ensures(sc_get(result.size_classes, 8) == Seq::singleton(16u64))]
#[ensures(result.slabs@.contains(16) && result.slabs@.lookup(16).element_size@ == 8)]
pub fn witness_em_inv_slab_uniform_element_pow2() -> RegionState {
    let mut r = w3p_recovered();
    proof_assert! { slab_k(r) == 2 && align_l(16, 4) == 16 && align_l(4, 4) == 4 && slots_of(16, 16) == 1 && slots_of(16, 4) == 4 };
    proof_assert! { first_ne(r.buddy.free_lists@, 2, 3) == 2 };
    let o1 = snapshot! { r };
    let a = r.alloc_extent(16);
    proof_assert! { fresh_case(*o1, r, 16, a) };
    proof_assert! { match a { Ok((d, _, _)) => d@ == 0, Err(_) => false } };
    proof_assert! { r.slabs@.contains(16) && r.slabs@.lookup(16) == o1.slabs@.lookup(16) };
    proof_assert! { r.buddy.free_lists@[2]@.len() == 0 && r.buddy.free_lists@[3]@.len() == 0 };
    proof_assert! { first_ne(r.buddy.free_lists@, 2, 3) == -1 };
    proof_assert! { sc_get(r.size_classes, 4) == Seq::empty() };
    let o2 = snapshot! { r };
    let b = r.alloc_extent(4);
    proof_assert! { fresh_case(*o2, r, 4, b) && r == *o2 };
    r
}

pub const WP_K5: u64 = 5;

/// pow2 witness of EM-PUBLISH-POST-VISIBLE: W4p — reserve(K5,8) takes Y's slot 1 (offset
/// 24, size 8); reserve(K3,16) carves a slab at 0; reserve(K4,4) finds no space (no
/// replacement); H5.publish() writes K5 into slot 1 of Y itself, which get_extents lists as
/// (K5, 24, 8) — the handle's key, offset and size.
#[ensures(result.1@ == 24 && result.2@ == 8)]
#[ensures(from_slot(result.0, 16, 1, Extent { key: WP_K5, size: 8u32, offset: 24u64 }))]
pub fn witness_em_publish_post_visible_pow2() -> (RegionState, u64, u32) {
    let mut r = w3p_recovered();
    let y = snapshot! { r.slabs@.lookup(16) };
    proof_assert! { slab_k(r) == 2 && align_l(8, 4) == 8 && align_l(16, 4) == 16 && align_l(4, 4) == 4 };
    proof_assert! { slots_of(16, 16) == 1 && slots_of(16, 4) == 4 };
    let o1 = snapshot! { r };
    proof_assert! { exist_cond(*o1, 8, 16u64) };
    let h5 = r.alloc_extent(8);
    proof_assert! { exist_case(*o1, r, 8, 16u64, h5) };
    let (off, sz) = match h5 {
        Ok((s, i, off)) => {
            proof_assert! { s@ == 16 && i@ < 2 && !slot_bit(y.bitmap, i@) };
            proof_assert! { i@ == 1 && off@ == slot_off(*y, 1) && off@ == 24 };
            (off, ((8u32 + 4 - 1) / 4 * 4)) // aligned_size (lib.rs:591)
        }
        Err(_) => (0, 0),
    };
    proof_assert! { filter_ne(Seq::singleton(16u64), 16u64) == Seq::empty() };
    proof_assert! { first_ne(r.buddy.free_lists@, 2, 3) == 2 };
    let o2 = snapshot! { r };
    let a = r.alloc_extent(16);
    proof_assert! { fresh_case(*o2, r, 16, a) };
    proof_assert! { match a { Ok((d, _, _)) => d@ == 0, Err(_) => false } };
    proof_assert! { r.slabs@.contains(16) && r.slabs@.lookup(16) == o2.slabs@.lookup(16) };
    proof_assert! { first_ne(r.buddy.free_lists@, 2, 3) == -1 };
    proof_assert! { sc_get(r.size_classes, 4) == Seq::empty() };
    let o3 = snapshot! { r };
    let b = r.alloc_extent(4);
    proof_assert! { fresh_case(*o3, r, 4, b) && r == *o3 };
    proof_assert! { r.slabs@.contains(16) && r.slabs@.lookup(16).keys@.len() == 2 };
    r.publish_slot(16, 1, WP_K5);
    proof_assert! { r.slabs@.lookup(16).keys@[1] == WP_K5 && r.slabs@.lookup(16).element_size@ == 8 };
    proof_assert! { slot_off(r.slabs@.lookup(16), 1) == 24 && WP_K5 != FREE_KEY };
    (r, off, sz)
}
