//! Batch 4 — 23 further in-scope properties of `extent-manager` (pin 2cd35bac).
//!
//! One scored module per inventory id: `verify_<id>` (proof) or `refute_<id>`
//! (machine-checked negation), each with a `__mutant` twin that must FAIL. Unscored
//! `witness_<id>_pow2` / `witness_<id>_sequential` modules show the property instance that a
//! refutation violates holds once the named root cause is out of the way.
use crate::model::b3::*;
use crate::model::b4::*;
use crate::model::bitmap::*;
use crate::model::buddy::*;
use crate::model::ckpt::*;
use crate::model::component::*;
use crate::model::listing::*;
use crate::model::params::*;
use crate::model::region::*;
use crate::model::sbbytes::*;
use crate::model::slab::*;
use crate::props::batch1::*;
use crate::props::batch2::*;
use crate::props::batch3::*;
use crate::props::witness::*;
use creusot_std::logic::FMap;
use crate::model::assume::*;
use crate::model::l2::{fdisj, fv, p2};
use crate::model::l2m::{coal, lemma_coal_single};
use crate::model::l2r::{lemma_rg_good_o, pv, rg_core, rg_good, sk, rg_acct, rg_ka, sc_ok, sl_away};
use crate::props::l2a::lg;
use crate::props::l3start::*;
use creusot_std::prelude::*;

// =============================================================================
// WriteHandle::publish result
// =============================================================================

/// **EM-PUBLISH-POST-RETURNED-EXTENT** — "When publish() succeeds, the extent it returns has
/// the same key, offset and size as the handle reported." reserve_extent (lib.rs:584-630)
/// builds `WriteHandle::new(key, offset, aligned_size, ..)` (lib.rs:623-629), whose accessors
/// report exactly those values; `publish` (iextent_manager.rs:132-139 → the publish closure,
/// lib.rs:597-616) returns `Extent { key, offset, size: aligned_size }` on BOTH branches (the
/// FREE_KEY discard and the normal publish). Proved for every component state, key and size.
#[requires(em_regions_wf(*em) && em_regions_ok(*em))]
#[requires(size@ > 0)]
#[requires(forall<i: Int> 0 <= i && i < em.arena@.len() ==> a_3783f8(size@, em.arena@[i].format_params.sector_size@))]
#[ensures(match result.0 {
    Ok(h) => h.key == key && result.1 == Ok((h.key, h.offset, h.size)),
    Err(_) => true,
})]
pub fn verify_em_publish_post_returned_extent(em: &mut ExtentManager, key: u64, size: u32) -> (Result<Handle, EmError>, Result<(u64, u64, u32), EmError>) {
    let r = em.reserve_extent(key, size);
    match r {
        Ok(h) => {
            proof_assert! { rsv_slot(em.arena@[h.region@], h) };
            proof_assert! { match em.regions { Some(rv) => exists<i: Int> 0 <= i && i < rv@.len() && h.region == rv@[i], None => false } };
            proof_assert! { rg_inv(em.arena@[h.region@]) };
            proof_assert! { slab_inv(em.arena@[h.region@].slabs@.lookup(h.slab_start@)) };
            proof_assert! { free_ok(em.arena@[h.region@], h.slab_start, h.slot_idx) };
            let w = WH::new(h);
            let (p, _after) = w.publish(em);
            (r, p)
        }
        Err(e) => (r, Err(e)),
    }
}

/// Mutant: claims the returned extent's size differs from the handle's.
#[requires(em_regions_wf(*em) && em_regions_ok(*em))]
#[requires(size@ > 0)]
#[requires(forall<i: Int> 0 <= i && i < em.arena@.len() ==> a_3783f8(size@, em.arena@[i].format_params.sector_size@))]
#[ensures(match result.0 { Ok(h) => result.1 != Ok((h.key, h.offset, h.size)), Err(_) => true })]
pub fn verify_em_publish_post_returned_extent__mutant(em: &mut ExtentManager, key: u64, size: u32) -> (Result<Handle, EmError>, Result<(u64, u64, u32), EmError>) {
    let r = em.reserve_extent(key, size);
    match r {
        Ok(h) => {
            proof_assert! { rsv_slot(em.arena@[h.region@], h) };
            proof_assert! { match em.regions { Some(rv) => exists<i: Int> 0 <= i && i < rv@.len() && h.region == rv@[i], None => false } };
            proof_assert! { rg_inv(em.arena@[h.region@]) };
            proof_assert! { slab_inv(em.arena@[h.region@].slabs@.lookup(h.slab_start@)) };
            proof_assert! { free_ok(em.arena@[h.region@], h.slab_start, h.slot_idx) };
            let w = WH::new(h);
            let (p, _after) = w.publish(em);
            (r, p)
        }
        Err(e) => (r, Err(e)),
    }
}

// =============================================================================
// Instance id
// =============================================================================

/// **EM-FORMAT-POST-INSTANCE-ID-GIVEN** — "When format() succeeds with an explicit instance
/// identifier, that exact identifier is stored in the superblock and get_instance_id()
/// subsequently returns it." (a) format with `instance_id: Some(id)` (lib.rs:471-473) records
/// `id` in the superblock it writes to the device (`dev.sb`, lib.rs:483-498) and publishes
/// (lib.rs:507); get_instance_id (lib.rs:686-692) returns it; (b) no later
/// reserve/publish/abort/remove/checkpoint changes what get_instance_id returns (apply_cop:
/// shared_frame).
// LEVEL-3 (phase D-B2): format's own ranges only — fmt_sane (e0fe79 + 3b58ea, both list format) in
// place of sane_params: no 7cb7c3 (its methods omit format; the real format accepts slab_size 0,
// lib.rs:388-405, and so does the mirror now).
#[requires(em_regions_wf(*f))]
#[requires(f.dev.connected ==> a_67ea4f(f.dev.num_sectors@, f.dev.sector_size@))]
#[requires(a_2ee144(params.metadata_alignment@))]
#[requires(fmt_sane(params) && params.instance_id == Some(id))]
#[requires(em_ok(*e) && cop_pre(*e, op))]
#[ensures(match result.0 {
    Ok(()) => (^f).dev.sb != None && match (^f).dev.sb { Some(x) => x.instance_id == id, None => false } && result.1 == Ok(id),
    Err(_) => true,
})]
#[ensures(result.2 == result.3)]
pub fn verify_em_format_post_instance_id_given(
    f: &mut ExtentManager, params: FormatParams, id: u64, e: &mut ExtentManager, op: COp,
) -> (Result<(), EmError>, Result<u64, EmError>, Result<u64, EmError>, Result<u64, EmError>) {
    let r = f.format(params);
    let got = f.get_instance_id();
    let a = e.get_instance_id();
    apply_cop(e, op);
    let b = e.get_instance_id();
    (r, got, a, b)
}

/// Mutant: claims the stored id is `id + 1`.
/// LEVEL-3 (phase D-B2): mutant with EXACTLY the property's requires and body; it claims the
/// negation of the property's first postcondition.
// LEVEL-3 (phase D-B2): format's own ranges only — fmt_sane (e0fe79 + 3b58ea, both list format) in
// place of sane_params: no 7cb7c3 (its methods omit format; the real format accepts slab_size 0,
// lib.rs:388-405, and so does the mirror now).
#[requires(em_regions_wf(*f))]
#[requires(f.dev.connected ==> a_67ea4f(f.dev.num_sectors@, f.dev.sector_size@))]
#[requires(a_2ee144(params.metadata_alignment@))]
#[requires(fmt_sane(params) && params.instance_id == Some(id))]
#[requires(em_ok(*e) && cop_pre(*e, op))]
#[ensures(!(match result.0 {
    Ok(()) => (^f).dev.sb != None && match (^f).dev.sb { Some(x) => x.instance_id == id, None => false } && result.1 == Ok(id),
    Err(_) => true,
}))]
pub fn verify_em_format_post_instance_id_given__mutant(
    f: &mut ExtentManager, params: FormatParams, id: u64, e: &mut ExtentManager, op: COp,
) -> (Result<(), EmError>, Result<u64, EmError>, Result<u64, EmError>, Result<u64, EmError>) {
    let r = f.format(params);
    let got = f.get_instance_id();
    let a = e.get_instance_id();
    apply_cop(e, op);
    let b = e.get_instance_id();
    (r, got, a, b)
}

/// **EM-INIT-POST-INSTANCE-ID** — "After initialize() succeeds, get_instance_id() returns the
/// instance identifier stored in the persisted superblock, i.e. the one recorded when the
/// device was formatted." (a) For every recovered superblock `sb`: initialize (lib.rs:570-577)
/// publishes `superblock: sb` and get_instance_id returns `sb.instance_id`; (b) the chain from
/// format: the superblock format writes to the device carries the formatted id, and a
/// component initialized from it returns that id; (c) a checkpoint's superblock rewrite
/// (lib.rs:302-307) only changes checkpoint_seq / active_copy (sb_frame), so the persisted
/// id stays the formatted one.
#[requires(em_regions_wf(*g) && sb_sane(sb) && recovered_ok(sb, per_region@))]
#[requires(em_regions_wf(*f))]
#[requires(f.dev.connected ==> a_67ea4f(f.dev.num_sectors@, f.dev.sector_size@))]
#[requires(a_2ee144(params.metadata_alignment@))]
#[requires(sane_params(params))]
#[requires(em_regions_wf(*h))]
#[ensures(result.0 == Ok(sb.instance_id))]
#[ensures(match result.1 {
    Ok(()) => match (f.shared, (^f).dev.sb) { (_, Some(x)) => result.2 == Ok(x.instance_id) && result.3 == Ok(x.instance_id), _ => false },
    Err(_) => true,
})]
pub fn verify_em_init_post_instance_id(
    g: &mut ExtentManager, sb: Superblock, per_region: &Vec<Vec<SlabDescriptor>>,
    f: &mut ExtentManager, params: FormatParams, h: &mut ExtentManager,
) -> (Result<u64, EmError>, Result<(), EmError>, Result<u64, EmError>, Result<u64, EmError>) {
    g.initialize(sb, per_region);
    let a = g.get_instance_id();
    let r = f.format(params);
    let mut fid = Err(EmError::NotInitialized);
    let mut hid = Err(EmError::NotInitialized);
    if r.is_ok() {
        fid = f.get_instance_id();
        match f.dev.sb {
            Some(sb0) => {
                let empty: Vec<Vec<SlabDescriptor>> = Vec::new();
                proof_assert! { sb_sane(sb0) && recovered_ok(sb0, empty@) };
                h.initialize(sb0, &empty);
                hid = h.get_instance_id();
            }
            None => {}
        }
    }
    (a, r, fid, hid)
}

/// Mutant: claims initialize reports instance id 0.
#[requires(em_regions_wf(*g) && sb_sane(sb) && recovered_ok(sb, per_region@))]
#[ensures(result == Ok(0u64))]
pub fn verify_em_init_post_instance_id__mutant(g: &mut ExtentManager, sb: Superblock, per_region: &Vec<Vec<SlabDescriptor>>) -> Result<u64, EmError> {
    g.initialize(sb, per_region);
    g.get_instance_id()
}

// =============================================================================
// Buddy allocator accounting
// =============================================================================

/// Every order below `ord_from(b, k)` (from `k`) is too small for `b`.
#[logic]
#[variant(64 - k)]
#[requires(0 <= k && k <= 64)]
#[ensures(forall<j: Int> k <= j && j < ord_from(b, k) ==> j.pow2() < b)]
pub fn lemma_ord_from_min(b: Int, k: Int) {
    if k < 64 && k.pow2() < b {
        lemma_ord_from_min(b, k + 1)
    }
}

/// **EM-BUDDY-ALLOC-ACCOUNTING** — "A successful allocation reduces the total free space by
/// exactly the size of the block handed out, which is the smallest power-of-two number of
/// sectors that holds the request." BuddyAllocator::alloc (buddy.rs:57-82) for every
/// allocator satisfying the buddy invariant and every request size: on success total_free()
/// (buddy.rs:159-166) drops by exactly span(ss, k) = 2^k sectors with k = ord(ceil(size/ss)):
/// 2^k sectors hold the request, 2^(k-1) do not (k > 0); on failure nothing changes.
// LEVEL-3 D-B1: the allocator is the buddy of a region `r` satisfying the PROVED invariant
// rg_good (inside lg: rg_inv + rg_acct + ...), and the request is the region's slab size —
// the only size alloc_extent's new-slab path (region.rs:66-74), its one caller, requests.
// The former premises are DERIVED: `size + ss <= MAX` is rg_params' headroom; `tf <= MAX`
// follows from rg_acct (free bytes + slab bytes == the whole-sector usable size <= total <= MAX).
#[requires(rg_good(*r) && r.buddy == *b && size == r.format_params.slab_size)]
#[ensures(match result.0 {
    Some(_) => result.1@ == result.2@ + *result.3
        && *result.3 == result.4.pow2() * b.sector_size@
        && result.4.pow2() * b.sector_size@ >= size@
        && (*result.4 > 0 ==> (*result.4 - 1).pow2() * b.sector_size@ < size@),
    None => result.1 == result.2 && ^b == *b,
})]
pub fn verify_em_buddy_alloc_accounting(b: &mut BuddyAllocator, size: u64, r: Snapshot<RegionState>) -> (Option<u64>, u64, u64, Snapshot<Int>, Snapshot<Int>) {
    proof_assert! { rg_inv(*r) && rg_params(*r) && rg_buddy(*r) && rg_acct(*r) && bd_inv(*b) };
    proof_assert! { size@ + b.sector_size@ <= u64::MAX@ };
    proof_assert! { lemma_divmod(b.total_usable_size@, b.sector_size@);
        (b.total_usable_size@ / b.sector_size@) * b.sector_size@ <= b.total_usable_size@ };
    proof_assert! { lemma_divmod(r.format_params.slab_size@, r.format_params.sector_size@);
        lemma_ord_ge(r.format_params.slab_size@ / r.format_params.sector_size@); slab_k(*r) >= 0 };
    proof_assert! { lemma_pow2(slab_k(*r)); slab_k(*r).pow2() >= 1 && sk(*r) >= 0 };
    proof_assert! { lemma_mul_le(0, r.slabs@.len(), sk(*r)); r.slabs@.len() * sk(*r) >= 0 };
    proof_assert! { tf(b.free_lists@, b.sector_size@, b.free_lists@.len()) <= u64::MAX@ };
    let ss = snapshot! { b.sector_size@ };
    let blocks = snapshot! { (size@ + *ss - 1) / *ss };
    let k = snapshot! { ord(*blocks) };
    proof_assert! { lemma_divmod(size@ + *ss - 1, *ss); *blocks >= 0 && *blocks <= 18446744073709551616 };
    proof_assert! { lemma_ord_ge(*blocks); k.pow2() >= *blocks && 0 <= *k };
    proof_assert! { lemma_ord_from_min(*blocks, 0); *k > 0 ==> (*k - 1).pow2() < *blocks };
    proof_assert! { lemma_mul_le(*blocks, k.pow2(), *ss); *blocks * *ss <= k.pow2() * *ss };
    proof_assert! { *blocks * *ss >= size@ };
    proof_assert! { *k > 0 ==> { lemma_mul_le((*k - 1).pow2() + 1, *blocks, *ss);
        ((*k - 1).pow2() + 1) * *ss <= *blocks * *ss && *blocks * *ss < size@ + *ss } };
    proof_assert! { *k > 0 ==> { lemma_distrib((*k - 1).pow2(), 1, *ss); (*k - 1).pow2() * *ss < size@ } };
    let before = b.total_free();
    let r = b.alloc(size);
    let mut after = before;
    let blk = snapshot! { span(*ss, *k) };
    if r.is_some() {
        proof_assert! { tf(b.free_lists@, *ss, b.free_lists@.len()) <= u64::MAX@ };
        after = b.total_free();
    }
    (r, before, after, blk, k)
}

/// Mutant: claims the free space drops by exactly the requested size.
#[requires(bd_inv(*b))]
#[requires(size@ + b.sector_size@ <= u64::MAX@)]
#[requires(tf(b.free_lists@, b.sector_size@, b.free_lists@.len()) <= u64::MAX@)]
#[ensures(match result.0 { Some(_) => result.1@ == result.2@ + size@, None => true })]
pub fn verify_em_buddy_alloc_accounting__mutant(b: &mut BuddyAllocator, size: u64) -> (Option<u64>, u64, u64) {
    let before = b.total_free();
    let r = b.alloc(size);
    let mut after = before;
    if r.is_some() {
        after = b.total_free();
    }
    (r, before, after)
}

// =============================================================================
// format(): metadata layout, capacity
// =============================================================================

/// **EM-FORMAT-POST-METADATA-FITS** — "After format() succeeds, the superblock and the two
/// checkpoint copies lie entirely within the usable metadata device (checkpoint offset plus
/// two checkpoint region sizes is at most the device size) and, when a non-zero metadata
/// region size is configured, within that region size (equality allowed); the two copies do
/// not overlap each other or the superblock." format's layout (lib.rs:417-438) as recorded in
/// the superblock: superblock [0, 4096), copy 0 [off, off + size), copy 1
/// [off + size, off + 2 size) with off >= 4096, size > 0, off + 2 size <= metadata device bytes
/// and <= metadata_region_size when that is non-zero. (Byte ranges of the LAYOUT; the LBAs the
/// I/O path derives from them are CKPT-LAYOUT-SECTOR, see EM-INV-CHECKPOINT-SECTOR-ALIGNED.)
#[requires(em_regions_wf(*f))]
#[requires(f.dev.connected ==> a_67ea4f(f.dev.num_sectors@, f.dev.sector_size@))]
#[requires(a_2ee144(params.metadata_alignment@))]
#[requires(sane_params(params))]
#[ensures(match result {
    Ok(()) => match (^f).shared {
        Some(sh) => SUPERBLOCK_SIZE@ <= sh.superblock.checkpoint_region_offset@
            && sh.superblock.checkpoint_region_size@ > 0
            && sh.superblock.checkpoint_region_offset@ + 2 * sh.superblock.checkpoint_region_size@ <= f.dev.num_sectors@ * f.dev.sector_size@
            && (params.metadata_region_size@ > 0 ==>
                sh.superblock.checkpoint_region_offset@ + 2 * sh.superblock.checkpoint_region_size@ <= params.metadata_region_size@)
            && (^f).dev.sb == Some(sh.superblock),
        None => false,
    },
    Err(_) => true,
})]
pub fn verify_em_format_post_metadata_fits(f: &mut ExtentManager, params: FormatParams) -> Result<(), EmError> {
    let md = snapshot! { f.dev.num_sectors@ * f.dev.sector_size@ };
    let r = f.format(params);
    proof_assert! { lemma_ckoff_ge(params.metadata_alignment@); ckoff_l(params.metadata_alignment@) >= 4096 };
    proof_assert! { match (r, f.shared) {
        (Ok(()), Some(sh)) => sh.superblock.checkpoint_region_size@ > 0 ==> {
            lemma_cklayout_fits(*md, params.metadata_region_size@, params.metadata_alignment@, params.sector_size@);
            true },
        _ => true } };
    r
}

/// Mutant: claims the two copies always fill the metadata device exactly.
#[requires(em_regions_wf(*f))]
#[requires(f.dev.connected ==> a_67ea4f(f.dev.num_sectors@, f.dev.sector_size@))]
#[requires(a_2ee144(params.metadata_alignment@))]
#[requires(sane_params(params))]
#[ensures(match result {
    Ok(()) => match (^f).shared {
        Some(sh) => sh.superblock.checkpoint_region_offset@ + 2 * sh.superblock.checkpoint_region_size@ == f.dev.num_sectors@ * f.dev.sector_size@,
        None => false },
    Err(_) => true,
})]
pub fn verify_em_format_post_metadata_fits__mutant(f: &mut ExtentManager, params: FormatParams) -> Result<(), EmError> {
    f.format(params)
}

/// The capacities of a partition laid out by format / initialize (lib.rs:459-464, 540-545)
/// sum to the partitioned size: `k * (u / n)` for the first `k < n` regions, `u` for all.
#[logic]
#[variant(k)]
#[requires(0 <= k && k <= rv.len() && rv.len() == n && n > 0 && u >= 0)]
#[requires(forall<i: Int> 0 <= i && i < rv.len() ==> arena[rv[i]@].buddy.total_usable_size@ == rsize(u, n, i))]
#[ensures(cap_sum(arena, rv, k) == if k < n { k * (u / n) } else { u })]
pub fn lemma_cap_part(arena: Seq<RegionState>, rv: Seq<usize>, u: Int, n: Int, k: Int) {
    pearlite! {
        if k > 0 {
            lemma_cap_part(arena, rv, u, n, k - 1);
            lemma_distrib(k - 1, 1, u / n)
        }
    }
}

/// **EM-CAP-POST-FORMULA** — "After format() succeeds, capacity_bytes() equals the data device
/// size configured at format time minus the data start offset: in shared-device mode (non-zero
/// metadata region size) that is the data device size minus the reserved metadata area, and in
/// separate-device mode (metadata region size of zero) it is exactly the full data device
/// size." format's regions are fresh buddy allocators whose usable sizes partition
/// data_disk_size - data_start_offset (lib.rs:447-464, fresh_rg); capacity_bytes sums them
/// (lib.rs:710-715). Proved for every parameter set and metadata device.
// LEVEL-3 (phase D-B2): format's own ranges only — fmt_sane (e0fe79 + 3b58ea, both list format) in
// place of sane_params: no 7cb7c3 (its methods omit format; the real format accepts slab_size 0,
// lib.rs:388-405, and so does the mirror now).
#[requires(em_regions_wf(*f))]
#[requires(f.dev.connected ==> a_67ea4f(f.dev.num_sectors@, f.dev.sector_size@))]
#[requires(a_2ee144(params.metadata_alignment@))]
#[requires(fmt_sane(params))]
#[ensures(match result.0 {
    Ok(()) => match (^f).shared {
        Some(sh) => result.1@ == params.data_disk_size@ - sh.superblock.data_start_offset@
            && (params.metadata_region_size@ > 0 ==> result.1@ == params.data_disk_size@
                - (sh.superblock.checkpoint_region_offset@ + 2 * sh.superblock.checkpoint_region_size@))
            && (params.metadata_region_size@ == 0 ==> result.1 == params.data_disk_size),
        None => false,
    },
    Err(_) => true,
})]
pub fn verify_em_cap_post_formula(f: &mut ExtentManager, params: FormatParams) -> (Result<(), EmError>, u64) {
    let r = f.format(params);
    match r {
        Ok(()) => {
            proof_assert! { match (f.regions, f.shared) {
                (Some(rv), Some(sh)) => {
                    lemma_cap_part(f.arena@, rv@, params.data_disk_size@ - sh.superblock.data_start_offset@, params.region_count@, rv@.len());
                    cap_sum(f.arena@, rv@, rv@.len()) == params.data_disk_size@ - sh.superblock.data_start_offset@ },
                _ => false } };
            let c = f.capacity_bytes();
            (r, c)
        }
        Err(_) => (r, 0),
    }
}

/// Mutant: claims capacity is the full data device size in every mode.
#[requires(em_regions_wf(*f))]
#[requires(f.dev.connected ==> a_67ea4f(f.dev.num_sectors@, f.dev.sector_size@))]
#[requires(a_2ee144(params.metadata_alignment@))]
#[requires(fmt_sane(params))]
#[ensures(match result.0 { Ok(()) => result.1 == params.data_disk_size, Err(_) => true })]
pub fn verify_em_cap_post_formula__mutant(f: &mut ExtentManager, params: FormatParams) -> (Result<(), EmError>, u64) {
    let r = f.format(params);
    match r {
        Ok(()) => {
            proof_assert! { match (f.regions, f.shared) {
                (Some(rv), Some(sh)) => {
                    lemma_cap_part(f.arena@, rv@, params.data_disk_size@ - sh.superblock.data_start_offset@, params.region_count@, rv@.len());
                    cap_sum(f.arena@, rv@, rv@.len()) == params.data_disk_size@ - sh.superblock.data_start_offset@ },
                _ => false } };
            let c = f.capacity_bytes();
            (r, c)
        }
        Err(_) => (r, 0),
    }
}

// =============================================================================
// initialize(): layout, never-checkpointed device, persisted-empty format
// =============================================================================

/// capacity_bytes() of a component just initialized from `sb` (lib.rs:533-545 + 710-715).
#[requires(em_regions_wf(*g) && sb_sane(sb) && recovered_ok(sb, per_region@))]
#[ensures(match ((^g).regions, (^g).shared) { (Some(_), Some(sh)) => sh.superblock == sb, _ => false })]
#[ensures(result@ == sb.data_disk_size@ - sb.data_start_offset@)]
#[ensures(em_regions_wf(^g) && em_regions_ok(^g))]
#[ensures(match (^g).regions { Some(rv) => forall<i: Int> 0 <= i && i < rv@.len() ==>
    init_rg_ok((^g).arena@[rv@[i]@], dsel(per_region@, i)), None => false })]
pub fn init_capacity(g: &mut ExtentManager, sb: Superblock, per_region: &Vec<Vec<SlabDescriptor>>) -> u64 {
    g.initialize(sb, per_region);
    proof_assert! { match g.regions {
        Some(rv) => {
            lemma_cap_part(g.arena@, rv@, sb.data_disk_size@ - sb.data_start_offset@, sb.region_count@, rv@.len());
            cap_sum(g.arena@, rv@, rv@.len()) == sb.data_disk_size@ - sb.data_start_offset@ },
        None => false } };
    g.capacity_bytes()
}

/// **EM-INIT-POST-LAYOUT-RESTORED** — "After initialize() succeeds, capacity_bytes() equals the
/// data disk size minus the data start offset recorded in the superblock, and both values
/// equal those established when the device was formatted, in both shared-device and
/// separate-device mode." (a) For every recovered superblock and descriptors: initialize
/// rebuilds the regions over [data_start_offset, data_disk_size) with format's partition
/// (lib.rs:533-545, now an ensures of the mirror: init_geo) and capacity_bytes returns
/// data_disk_size - data_start_offset of `sb`; (b) chained from format (any mode): the
/// superblock format writes carries format's data_disk_size and data_start_offset, and a
/// component initialized from it reports format's capacity. (Checkpoints rewrite the device
/// superblock with these fields unchanged: sb_frame, EM-INV-SEQ-AGREE batch.)
#[requires(em_regions_wf(*g) && sb_sane(sb) && recovered_ok(sb, per_region@))]
#[requires(em_regions_wf(*f))]
#[requires(f.dev.connected ==> a_67ea4f(f.dev.num_sectors@, f.dev.sector_size@))]
#[requires(a_2ee144(params.metadata_alignment@))]
#[requires(sane_params(params))]
#[requires(em_regions_wf(*h))]
#[ensures(result.0@ == sb.data_disk_size@ - sb.data_start_offset@)]
#[ensures(match result.1 {
    Ok(()) => match ((^f).shared, (^h).shared) {
        (Some(fs), Some(hs)) => result.2 == result.3
            && hs.superblock.data_disk_size == fs.superblock.data_disk_size
            && hs.superblock.data_start_offset == fs.superblock.data_start_offset
            && result.3@ == hs.superblock.data_disk_size@ - hs.superblock.data_start_offset@,
        _ => false,
    },
    Err(_) => true,
})]
pub fn verify_em_init_post_layout_restored(
    g: &mut ExtentManager, sb: Superblock, per_region: &Vec<Vec<SlabDescriptor>>,
    f: &mut ExtentManager, params: FormatParams, h: &mut ExtentManager,
) -> (u64, Result<(), EmError>, u64, u64) {
    let c = init_capacity(g, sb, per_region);
    let r = f.format(params);
    match r {
        Ok(()) => {
            proof_assert! { match (f.regions, f.shared) {
                (Some(rv), Some(sh)) => {
                    lemma_cap_part(f.arena@, rv@, params.data_disk_size@ - sh.superblock.data_start_offset@, params.region_count@, rv@.len());
                    cap_sum(f.arena@, rv@, rv@.len()) == params.data_disk_size@ - sh.superblock.data_start_offset@ },
                _ => false } };
            let fc = f.capacity_bytes();
            match f.dev.sb {
                Some(sb0) => {
                    let empty: Vec<Vec<SlabDescriptor>> = Vec::new();
                    proof_assert! { sb_sane(sb0) && recovered_ok(sb0, empty@) };
                    let hc = init_capacity(h, sb0, &empty);
                    (c, r, fc, hc)
                }
                None => (c, r, fc, 0),
            }
        }
        Err(_) => (c, r, 0, 0),
    }
}

/// Mutant: claims the restored capacity is the full data disk size.
#[requires(em_regions_wf(*g) && sb_sane(sb) && recovered_ok(sb, per_region@))]
#[ensures(result == sb.data_disk_size)]
pub fn verify_em_init_post_layout_restored__mutant(g: &mut ExtentManager, sb: Superblock, per_region: &Vec<Vec<SlabDescriptor>>) -> u64 {
    init_capacity(g, sb, per_region)
}

/// `region_count` empty descriptor lists (recovery.rs:18-21).
#[ensures(result@.len() == n@)]
#[ensures(forall<i: Int> 0 <= i && i < result@.len() ==> result@[i]@.len() == 0)]
pub fn empty_regions(n: usize) -> Vec<Vec<SlabDescriptor>> {
    let mut pr: Vec<Vec<SlabDescriptor>> = Vec::new();
    let mut i: usize = 0;
    #[invariant(i@ <= n@ && pr@.len() == i@)]
    #[invariant(forall<j: Int> 0 <= j && j < i@ ==> pr@[j]@.len() == 0)]
    while i < n {
        pr.push(Vec::new());
        i += 1;
    }
    pr
}

/// initialize from a never-checkpointed superblock + get_extents (recovery.rs:18-21,
/// lib.rs:514-582, 632-656).
#[requires(em_regions_wf(*g) && sb_sane(sb) && sb.checkpoint_seq@ == 0 && sb.active_copy@ <= 1)]
#[ensures(result.0 == Ok(Recovered::Empty) && result.1@.len() == 0)]
#[ensures(result.2@ == sb.data_disk_size@ - sb.data_start_offset@)]
#[ensures(match (^g).shared { Some(sh) => sh.superblock == sb, None => false })]
pub fn init_never_ckpt(g: &mut ExtentManager, sb: Superblock, act: RdOut, inact: RdOut) -> (Result<Recovered, EmError>, Vec<Extent>, u64) {
    let rec = recover_rd(sb.checkpoint_seq, sb.active_copy, act, inact);
    let pr = empty_regions(sb.region_count as usize);
    proof_assert! { forall<i: Int> 0 <= i && i < pr@.len() ==> dsel(pr@, i) == Seq::empty() };
    proof_assert! { recovered_ok(sb, pr@) };
    let cap = init_capacity(g, sb, &pr);
    proof_assert! { match g.regions { Some(rv) => forall<i: Int> 0 <= i && i < rv@.len() ==>
        dsel(pr@, i) == Seq::empty() && starts_distinct(dsel(pr@, i)) && rebuilt_all(g.arena@[rv@[i]@], dsel(pr@, i)), None => false } };
    proof_assert! { match g.regions { Some(rv) => forall<i: Int, k: Int> 0 <= i && i < rv@.len() ==>
        !g.arena@[rv@[i]@].slabs@.contains(k), None => false } };
    proof_assert! { match g.regions { Some(rv) => forall<e: Extent> !listed_one(*g, rv@, e), None => false } };
    let ex = g.get_extents();
    proof_assert! { match g.regions { Some(rv) => ex@.len() > 0 ==> listed_one(*g, rv@, ex@[0]), None => false } };
    (rec, ex, cap)
}

/// **EM-INIT-POST-NEVER-CHECKPOINTED** — "When the persisted superblock records that no
/// checkpoint has ever been written, initialize() succeeds and the component lists no
/// extents." recover (recovery.rs:18-21, mirror recover_rd) returns empty descriptor lists for
/// checkpoint_seq 0 WITHOUT reading either copy (any copy-read outcome); initialize rebuilds
/// every region from no descriptor (rebuilt_all: no slab); get_extents lists nothing.
#[requires(em_regions_wf(*g) && sb_sane(sb) && sb.checkpoint_seq@ == 0 && sb.active_copy@ <= 1)]
#[ensures(result.0 == Ok(Recovered::Empty) && result.1@.len() == 0)]
pub fn verify_em_init_post_never_checkpointed(g: &mut ExtentManager, sb: Superblock, act: RdOut, inact: RdOut) -> (Result<Recovered, EmError>, Vec<Extent>) {
    let (rec, ex, _cap) = init_never_ckpt(g, sb, act, inact);
    (rec, ex)
}

/// Mutant: claims something is listed.
#[requires(em_regions_wf(*g) && sb_sane(sb) && sb.checkpoint_seq@ == 0 && sb.active_copy@ <= 1)]
#[ensures(result.1@.len() > 0)]
pub fn verify_em_init_post_never_checkpointed__mutant(g: &mut ExtentManager, sb: Superblock, act: RdOut, inact: RdOut) -> (Result<Recovered, EmError>, Vec<Extent>) {
    let (rec, ex, _cap) = init_never_ckpt(g, sb, act, inact);
    (rec, ex)
}

/// **EM-FORMAT-POST-PERSISTED-EMPTY** — "After format() succeeds, a fresh component connected
/// to the same metadata device can initialize() successfully and then lists no extents and
/// reports the same instance id and capacity." format writes a superblock with
/// checkpoint_seq 0 (lib.rs:483-498, `dev.sb`); a fresh component (any prior state, its
/// metadata namespace / base LBA set to format's as documented) reads it back (the byte
/// round trip is EM-INV-SUPERBLOCK-ROUNDTRIP, proved in batch 2), recovers empty regions
/// (recovery.rs:18-21) for every copy-read outcome, lists nothing, and reports format's
/// instance id and capacity.
#[requires(em_regions_wf(*f))]
#[requires(f.dev.connected ==> a_67ea4f(f.dev.num_sectors@, f.dev.sector_size@))]
#[requires(a_2ee144(params.metadata_alignment@))]
#[requires(sane_params(params))]
#[requires(em_regions_wf(*g))]
#[ensures(match result.0 {
    Ok(()) => result.1 == Ok(Recovered::Empty) && result.2@.len() == 0
        && result.3 == result.4 && result.5 == result.6 && result.3 != Err(EmError::NotInitialized),
    Err(_) => true,
})]
pub fn verify_em_format_post_persisted_empty(
    f: &mut ExtentManager, params: FormatParams, g: &mut ExtentManager, act: RdOut, inact: RdOut,
) -> (Result<(), EmError>, Result<Recovered, EmError>, Vec<Extent>, Result<u64, EmError>, Result<u64, EmError>, u64, u64) {
    let r = f.format(params);
    match r {
        Ok(()) => {
            proof_assert! { match (f.regions, f.shared) {
                (Some(rv), Some(sh)) => {
                    lemma_cap_part(f.arena@, rv@, params.data_disk_size@ - sh.superblock.data_start_offset@, params.region_count@, rv@.len());
                    cap_sum(f.arena@, rv@, rv@.len()) == params.data_disk_size@ - sh.superblock.data_start_offset@ },
                _ => false } };
            let fid = f.get_instance_id();
            let fc = f.capacity_bytes();
            match f.dev.sb {
                Some(sb0) => {
                    proof_assert! { sb_sane(sb0) && sb0.checkpoint_seq@ == 0 && sb0.active_copy@ == 0 };
                    let (rec, ex, gc) = init_never_ckpt(g, sb0, act, inact);
                    let gid = g.get_instance_id();
                    (r, rec, ex, fid, gid, fc, gc)
                }
                None => (r, Err(EmError::IoError), Vec::new(), fid, Err(EmError::IoError), fc, 0),
            }
        }
        Err(_) => (r, Err(EmError::IoError), Vec::new(), Err(EmError::IoError), Err(EmError::IoError), 0, 0),
    }
}

/// Mutant: claims the fresh component reports a different capacity.
#[requires(em_regions_wf(*f))]
#[requires(f.dev.connected ==> a_67ea4f(f.dev.num_sectors@, f.dev.sector_size@))]
#[requires(a_2ee144(params.metadata_alignment@))]
#[requires(sane_params(params))]
#[requires(em_regions_wf(*g))]
#[ensures(match result.0 { Ok(()) => result.5 != result.6, Err(_) => true })]
pub fn verify_em_format_post_persisted_empty__mutant(
    f: &mut ExtentManager, params: FormatParams, g: &mut ExtentManager, act: RdOut, inact: RdOut,
) -> (Result<(), EmError>, Result<Recovered, EmError>, Vec<Extent>, Result<u64, EmError>, Result<u64, EmError>, u64, u64) {
    let r = f.format(params);
    match r {
        Ok(()) => {
            proof_assert! { match (f.regions, f.shared) {
                (Some(rv), Some(sh)) => {
                    lemma_cap_part(f.arena@, rv@, params.data_disk_size@ - sh.superblock.data_start_offset@, params.region_count@, rv@.len());
                    cap_sum(f.arena@, rv@, rv@.len()) == params.data_disk_size@ - sh.superblock.data_start_offset@ },
                _ => false } };
            let fid = f.get_instance_id();
            let fc = f.capacity_bytes();
            match f.dev.sb {
                Some(sb0) => {
                    proof_assert! { sb_sane(sb0) && sb0.checkpoint_seq@ == 0 && sb0.active_copy@ == 0 };
                    let (rec, ex, gc) = init_never_ckpt(g, sb0, act, inact);
                    let gid = g.get_instance_id();
                    (r, rec, ex, fid, gid, fc, gc)
                }
                None => (r, Err(EmError::IoError), Vec::new(), fid, Err(EmError::IoError), fc, 0),
            }
        }
        Err(_) => (r, Err(EmError::IoError), Vec::new(), Err(EmError::IoError), Err(EmError::IoError), 0, 0),
    }
}

// =============================================================================
// used_bytes right after format (new root cause REGION-SECTOR-REMAINDER)
// =============================================================================

/// The REGION-SECTOR-REMAINDER geometry: metadata device 4 x 4096 bytes; FR-002-valid
/// parameters with sector 4, slab 4, max extent 4, EIGHT regions over a 48-byte data device
/// (12 whole sectors; separate-device mode): every region is 48 / 8 = 6 bytes.
#[ensures(result.0.connected && result.0.sector_size@ == 4096 && result.0.num_sectors@ == 4)]
#[ensures(result.1.data_disk_size@ == 48 && result.1.slab_size@ == 4 && result.1.max_extent_size@ == 4)]
#[ensures(result.1.sector_size@ == 4 && result.1.region_count@ == 8 && result.1.metadata_alignment@ == 0)]
#[ensures(result.1.metadata_region_size@ == 0 && result.1.instance_id == Some(1u64))]
pub fn wu_geometry() -> (MetaDevice, FormatParams) {
    let dev = MetaDevice { connected: true, sector_size: 4096, num_sectors: 4, sb: None, ckpt0: None, ckpt1: None };
    let params = FormatParams {
        data_disk_size: 48,
        slab_size: 4,
        max_extent_size: 4,
        sector_size: 4,
        region_count: 8,
        metadata_alignment: 0,
        instance_id: Some(1),
        metadata_disk_ns_id: 1,
        metadata_region_size: 0,
    };
    (dev, params)
}

/// format() on the REGION-SECTOR-REMAINDER geometry, then used_bytes().
#[ensures(match result.0 { Ok(()) => result.1@ == 16, Err(e) => e == EmError::IoError })]
pub fn wu_format_used() -> (Result<(), EmError>, u64) {
    let (dev, params) = wu_geometry();
    let mut em = new_inner(dev);
    proof_assert! { em_regions_wf(em) };
    proof_assert! { fmt_ok8(em, params) };
    let r = em.format(params);
    if r.is_err() {
        return (r, 0);
    }
    proof_assert! { params.slab_size@ > 0 && em_regions_ok(em) };  // LEVEL-3 D-B2: format ensures em_regions_ok for slab_size > 0
    proof_assert! { ds_l(16384, 0, 0, 4) == 0 };
    proof_assert! { forall<i: Int> 0 <= i && i < 8 ==> rsize(48, 8, i) == 6 };
    proof_assert! { 0.pow2() == 1 && 6 / 4 == 1 && span(4, 0) == 4 };
    proof_assert! { match em.regions { Some(rv) => rv@.len() == 8 && forall<i: Int> 0 <= i && i < 8 ==>
        fresh_rg(em.arena@[rv@[i]@], rbase(0, 48, 8, i), 6, params), None => false } };
    proof_assert! { match em.regions { Some(rv) => forall<i: Int> 0 <= i && i < 8 ==>
        em.arena@[rv@[i]@].buddy.total_usable_size@ == 6
        && tf(em.arena@[rv@[i]@].buddy.free_lists@, 4, em.arena@[rv@[i]@].buddy.free_lists@.len()) == 4
        && em.arena@[rv@[i]@].buddy.sector_size@ == 4
        && rg_used(em.arena@[rv@[i]@]) == 2, None => false } };
    proof_assert! { match em.regions { Some(rv) => forall<i: Int> 0 <= i && i < rv@.len() ==> rg_inv(em.arena@[rv@[i]@]), None => false } };
    proof_assert! { match em.regions { Some(rv) => rv@.len() == 8 && forall<i: Int> 0 <= i && i < rv@.len() ==> rg_used(em.arena@[rv@[i]@]) == 2, None => false } };
    proof_assert! { match em.regions { Some(rv) => { lemma_used_const(em.arena@, rv@, 2, 8); used_sum(em.arena@, rv@, 8) == 8 * 2 }, None => false } };
    proof_assert! { match em.regions { Some(rv) => used_sum(em.arena@, rv@, 8) == 16, None => false } };
    proof_assert! { match em.regions { Some(rv) => used_terms_ok(em.arena@, rv@), None => false } };
    let u = em.used_bytes();
    (r, u)
}

/// **EM-FORMAT-POST-USED-ZERO** — REFUTED (INDEPENDENT; new root cause
/// REGION-SECTOR-REMAINDER: format sizes each region `usable / region_count` bytes
/// (lib.rs:455-464) with no rounding to whole sectors, BuddyAllocator::new tracks only
/// `total_usable_size / sector_size` whole sectors (buddy.rs:11-37), and used_bytes reports
/// `total_usable_size - total_free` per region (lib.rs:704) — the sub-sector remainder of
/// every region counts as used). "Immediately after format() succeeds, used_bytes() returns
/// zero." Witness (sector 4 — a power of two, NOT the buddy mask; FR-002-valid; the data
/// device is 12 WHOLE sectors): 8 regions of 6 bytes, each with one free 4-byte sector, so
/// used_bytes() = 8 * 2 = 16 right after a successful format. The only other format
/// outcome on these inputs is the device's own IoError on the superblock write.
#[ensures(match result.0 { Ok(()) => result.1@ == 16 && result.1@ != 0, Err(e) => e == EmError::IoError })]
pub fn support_old_refute_em_format_post_used_zero() -> (Result<(), EmError>, u64) {
    wu_format_used()
}

/// Mutant: claims used_bytes() is 0 after the successful format.
#[ensures(match result.0 { Ok(()) => result.1@ == 0, Err(_) => true })]
pub fn support_old_refute_em_format_post_used_zero__mutant() -> (Result<(), EmError>, u64) {
    wu_format_used()
}

// =============================================================================
// CKPT-SB-ORDER chain (batch 2's refute_em_ckpt_crash_atomic, every step exposed)
// =============================================================================

/// C8 (sector 4, slab 8): publish K1; checkpoint #1 ok (copy 1 = seq 1, superblock seq 1 /
/// copy 1); publish K2; checkpoint #2: copy 0 = seq 2 written, superblock write FAILS
/// (IoError) — the in-memory superblock already says seq 2 / copy 0 (checkpoint.rs:105-112);
/// checkpoint #3 (still dirty): its copy write targets `1 - 0` = copy 1, the copy the
/// ON-DISK superblock names, and overwrites it with seq 3; its superblock write fails too
/// (or the system crashes before it). Returns (#1, #2, #3 results, device after #2,
/// recovery after #2, device after #3, recovery after #3, image #1 persisted).
#[ensures(result.0 == Ok(()) && result.1 == Err(EmError::IoError) && result.2 == Err(EmError::IoError))]
#[ensures(result.3.sb_seq@ == 1 && result.3.sb_active@ == 1)]
#[ensures(match result.3.copy1 { Some(c) => c.seq@ == 1 && c.img == result.7, None => false })]
#[ensures(result.4 == Ok(result.7))]
#[ensures(result.5.sb_seq@ == 1 && result.5.sb_active@ == 1)]
#[ensures(match result.5.copy1 { Some(c) => c.seq@ == 3, None => false })]
#[ensures(match result.6 { Ok(_) => false, Err(_) => true })]
pub fn wsb_chain() -> (Result<(), EmError>, Result<(), EmError>, Result<(), EmError>, Snapshot<CkDev>,
    Result<Snapshot<Seq<RegionState>>, EmError>, Snapshot<CkDev>, Result<Snapshot<Seq<RegionState>>, EmError>, Snapshot<Seq<RegionState>>) {
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
    em.regions[0].publish_slot(0, 0, WK1);
    proof_assert! { regions_ok(em.regions@) && !all_clean(em.regions@) };
    let img1 = snapshot! { em.regions@ };
    // checkpoint #1: both writes succeed
    let c1 = em.run_checkpoint(true, true);
    proof_assert! { c1 == Ok(()) && em.seq@ == 1 && em.active@ == 1 && em.dev.sb_seq@ == 1 && em.dev.sb_active@ == 1 };
    proof_assert! { match em.dev.copy1 { Some(c) => c.seq@ == 1 && *c.img == *img1, None => false } };
    proof_assert! { regions_ok(em.regions@) };
    em.regions[0].publish_slot(0, 1, WK2);
    proof_assert! { regions_ok(em.regions@) && !all_clean(em.regions@) };
    // checkpoint #2: copy write succeeds, superblock write fails
    let c2 = em.run_checkpoint(true, false);
    proof_assert! { c2 != Ok(()) && em.seq@ == 2 && em.active@ == 0 && em.dev.sb_seq@ == 1 && em.dev.sb_active@ == 1 };
    proof_assert! { match em.dev.copy1 { Some(c) => c.seq@ == 1 && *c.img == *img1, None => false } };
    proof_assert! { !all_clean(em.regions@) && regions_ok(em.regions@) };
    let d2 = snapshot! { em.dev };
    let rec2 = recover(&em.dev);
    proof_assert! { match em.dev.copy1 { Some(c) => rec2 == Ok(c.img), None => false } };
    // checkpoint #3: copy write succeeds; the superblock write fails / the system crashes
    let c3 = em.run_checkpoint(true, false);
    proof_assert! { c3 != Ok(()) && em.seq@ == 3 && em.active@ == 1 && em.dev.sb_seq@ == 1 && em.dev.sb_active@ == 1 };
    proof_assert! { match em.dev.copy1 { Some(c) => c.seq@ == 3, None => false } };
    let d3 = snapshot! { em.dev };
    let rec3 = recover(&em.dev);
    (c1, c2, c3, d2, rec2, d3, rec3, img1)
}

/// **EM-CKPT-ERR-IO-FAILURE** — REFUTED (INDEPENDENT; ROOT CAUSE = CKPT-SB-ORDER:
/// write_checkpoint advances the in-memory active_copy / checkpoint_seq (checkpoint.rs:105-112)
/// before run_checkpoint's superblock write (lib.rs:302-307) is known to have succeeded, and
/// nothing restores them on failure). "When a metadata-device write fails during a
/// checkpoint, checkpoint() returns an IoError and the superblock on disk continues to name
/// the previously active copy, so the device still recovers to the last completed
/// checkpoint." In `wsb_chain` checkpoint #3's superblock write fails: it returns IoError and
/// the on-disk superblock still names copy 1 (seq 1), but copy 1 was just overwritten by #3's
/// copy write (it went to `1 - active` of the ADVANCED in-memory superblock), so recovery
/// finds no valid copy instead of the last completed checkpoint #1. (After the FIRST failure,
/// #2, the device does still recover #1 — the claim fails from the second one on.)
#[ensures(result.0 == Err(EmError::IoError))]
#[ensures(result.1.sb_active@ == 1 && result.1.sb_seq@ == 1)]
#[ensures(match result.2 { Ok(_) => false, Err(_) => true })]
#[ensures(result.3 == Ok(result.4))]
pub fn refute_em_ckpt_err_io_failure() -> (Result<(), EmError>, Snapshot<CkDev>, Result<Snapshot<Seq<RegionState>>, EmError>,
    Result<Snapshot<Seq<RegionState>>, EmError>, Snapshot<Seq<RegionState>>) {
    let (_c1, _c2, c3, _d2, rec2, d3, rec3, img1) = wsb_chain();
    (c3, d3, rec3, rec2, img1)
}

/// Mutant: claims the device still recovers after #3.
#[ensures(match result.2 { Ok(_) => true, Err(_) => false })]
pub fn refute_em_ckpt_err_io_failure__mutant() -> (Result<(), EmError>, Snapshot<CkDev>, Result<Snapshot<Seq<RegionState>>, EmError>,
    Result<Snapshot<Seq<RegionState>>, EmError>, Snapshot<Seq<RegionState>>) {
    let (_c1, _c2, c3, _d2, rec2, d3, rec3, img1) = wsb_chain();
    (c3, d3, rec3, rec2, img1)
}

/// **EM-CKPT-FAILED-SUPERBLOCK-WRITE** — REFUTED (INDEPENDENT; ROOT CAUSE = CKPT-SB-ORDER, as
/// EM-CKPT-ERR-IO-FAILURE; this is the code reader's own note). "If a checkpoint fails after
/// writing its data but before the superblock reaches the device, later checkpoints never
/// overwrite the checkpoint copy that the on-disk superblock still names as active, so the
/// device always recovers to a valid checkpoint." In `wsb_chain` checkpoint #2 fails exactly
/// there (copy 0 written, superblock write IoError): the on-disk superblock names copy 1
/// (seq 1); the next checkpoint #3 overwrites copy 1 with seq 3, and when its own superblock
/// write does not land the device recovers to no valid checkpoint (CorruptMetadata).
#[ensures(result.0 == Err(EmError::IoError))]
#[ensures(result.1.sb_active@ == 1)]
#[ensures(match result.1.copy1 { Some(c) => c.seq@ == 1, None => false })]
#[ensures(result.2.sb_active@ == 1)]
#[ensures(match result.2.copy1 { Some(c) => c.seq@ == 3, None => false })]
#[ensures(match result.3 { Ok(_) => false, Err(_) => true })]
pub fn refute_em_ckpt_failed_superblock_write() -> (Result<(), EmError>, Snapshot<CkDev>, Snapshot<CkDev>, Result<Snapshot<Seq<RegionState>>, EmError>) {
    let (_c1, c2, _c3, d2, _rec2, d3, rec3, _img1) = wsb_chain();
    (c2, d2, d3, rec3)
}

/// Mutant: claims #3 left copy 1 (the on-disk active copy) with seq 1.
#[ensures(match result.2.copy1 { Some(c) => c.seq@ == 1, None => false })]
pub fn refute_em_ckpt_failed_superblock_write__mutant() -> (Result<(), EmError>, Snapshot<CkDev>, Snapshot<CkDev>, Result<Snapshot<Seq<RegionState>>, EmError>) {
    let (_c1, c2, _c3, d2, _rec2, d3, rec3, _img1) = wsb_chain();
    (c2, d2, d3, rec3)
}

// =============================================================================
// Checkpoint after a fallback recovery (new root cause CKPT-FALLBACK-OVERWRITE)
// =============================================================================

pub const WF_K1: u64 = 31;

/// A device on which the ACTIVE copy (copy 0, superblock seq 2) no longer reads back
/// (bit rot / a torn write: `None` — any copy whose header seq or CRC fails is the same
/// to recovery) while the inactive copy 1 holds the previous checkpoint (seq 1).
/// initialize() falls back to copy 1 (recovery.rs:47-58) and installs the UNCHANGED
/// superblock (active copy 0, seq 2) as its in-memory superblock (lib.rs:570-573). The
/// caller then reserves K1, K2 (C8 region: fresh region + two reservations, `wc8_two`) and
/// publishes K1; the next checkpoint writes `1 - 0` = copy 1 — the copy just recovered from,
/// the ONLY valid one — with seq 3, and its superblock write fails (or the system crashes
/// before it lands). Returns (initial recovery, recovered image, checkpoint result, device
/// before / after the checkpoint, recovery afterwards).
#[ensures(result.0 == Ok(result.1))]
#[ensures(result.2 == Err(EmError::IoError))]
#[ensures(result.3.sb_seq@ == 2 && result.3.sb_active@ == 0 && result.3.copy0 == None)]
#[ensures(match result.3.copy1 { Some(c) => c.seq@ == 1 && c.img == result.1, None => false })]
#[ensures(result.4.sb_seq@ == 2 && result.4.sb_active@ == 0 && result.4.copy0 == None)]
#[ensures(match result.4.copy1 { Some(c) => c.seq@ == 3, None => false })]
#[ensures(match result.5 { Ok(_) => false, Err(_) => true })]
pub fn wfb_chain() -> (Result<Snapshot<Seq<RegionState>>, EmError>, Snapshot<Seq<RegionState>>, Result<(), EmError>,
    Snapshot<CkDev>, Snapshot<CkDev>, Result<Snapshot<Seq<RegionState>>, EmError>) {
    let fresh = RegionState::new(BuddyAllocator::new(0, 8, 4), wc_fp(8, 8));
    let img1 = snapshot! { Seq::singleton(fresh) };
    let d = CkDev { sb_seq: 2, sb_active: 0, copy0: None, copy1: Some(CkCopy { seq: 1, img: img1 }) };
    let rec0 = recover(&d);
    proof_assert! { rec0 == Ok(img1) };
    // initialize(): regions rebuilt from copy 1; in-memory superblock = sb (active 0, seq 2)
    let r = wc8_two();
    let r0 = snapshot! { r };
    let mut rs: Vec<RegionState> = Vec::new();
    rs.push(r);
    let mut em = CkEm { regions: rs, active: 0, seq: 2, dev: d };
    proof_assert! { em.regions@.len() == 1 && em.regions@[0] == *r0 };
    // LEVEL-3 reachable start (initialize of a device written by format + two checkpoints:
    // superblock seq 2 / active 0 — parity-consistent; copy 1 = the empty region, an empty
    // descriptor list, so 980f61 / 3b612e hold vacuously): the format-start ranges on that
    // superblock, the proved region invariants of the region (rebuilt empty + reserves K1/K2,
    // live handles), the reserve ranges.
    let mut sb2 = l3_sb_c8();
    sb2.checkpoint_seq = 2;
    proof_assert! { l3_fmt_ranges(wc_fp_l(8u64, 8u64), 3, 4096, sb2) && l3_rsv_ranges(4, wc_fp_l(8u64, 8u64)) };
    proof_assert! { l3_rec_ranges(Seq::empty(), 0, 8, 8) && fresh.slabs@ == FMap::empty() };
    proof_assert! { rg_l3(em.regions@[0], hs_w2()) && regions_ok(em.regions@) };
    proof_assert! { em.active@ == sb2.active_copy@ && em.seq@ == sb2.checkpoint_seq@ && em.dev.sb_seq@ == sb2.checkpoint_seq@ && em.dev.sb_active@ == sb2.active_copy@ };
    // publish(K1) (lib.rs:610)
    em.regions[0].publish_slot(0, 0, WF_K1);
    proof_assert! { regions_ok(em.regions@) && !all_clean(em.regions@) };
    let before = snapshot! { em.dev };
    // checkpoint(): copy write lands on copy 1; superblock write fails
    let c = em.run_checkpoint(true, false);
    proof_assert! { em.active@ == 1 && em.seq@ == 3 && em.dev.sb_seq@ == 2 && em.dev.sb_active@ == 0 };
    proof_assert! { em.dev.copy0 == None };
    proof_assert! { match em.dev.copy1 { Some(c) => c.seq@ == 3, None => false } };
    let after = snapshot! { em.dev };
    let rec = recover(&em.dev);
    (rec0, img1, c, before, after, rec)
}

/// **EM-CKPT-AFTER-FALLBACK-PRESERVES-VALID** — REFUTED (INDEPENDENT; new root cause
/// CKPT-FALLBACK-OVERWRITE: after a fallback recovery `recover` returns the superblock
/// unchanged (recovery.rs:47-58: active_copy still names the corrupt copy), initialize
/// installs it as the in-memory superblock (lib.rs:570-573), and write_checkpoint writes
/// `1 - active_copy` (checkpoint.rs:61-63) — the copy recovery just used, the only valid one —
/// while the on-disk superblock keeps naming the corrupt copy until lib.rs:302-307 lands).
/// "After initialize() recovered from the previous checkpoint copy because the active one was
/// invalid, the next checkpoint does not overwrite that only valid copy before a new valid
/// checkpoint is durable elsewhere." Witness `wfb_chain` (sector 4, slab 8, single-threaded,
/// device within the IBlockDevice contract: one write returns an error): the next checkpoint
/// overwrites copy 1 (seq 1 -> 3) while nothing valid is durable elsewhere (copy 0 unreadable,
/// superblock unchanged), and the device recovers to no checkpoint at all. Not the same cause
/// as CKPT-SB-ORDER: no earlier superblock write failed.
#[ensures(result.0 == Ok(result.1))]
#[ensures(match result.3.copy1 { Some(c) => c.seq@ == 1, None => false })]
#[ensures(result.3.copy0 == None && result.4.copy0 == None)]
#[ensures(match result.4.copy1 { Some(c) => c.seq@ == 3, None => false })]
#[ensures(result.4.sb_seq == result.3.sb_seq && result.4.sb_active == result.3.sb_active)]
#[ensures(match result.5 { Ok(_) => false, Err(_) => true })]
pub fn refute_em_ckpt_after_fallback_preserves_valid() -> (Result<Snapshot<Seq<RegionState>>, EmError>, Snapshot<Seq<RegionState>>,
    Result<(), EmError>, Snapshot<CkDev>, Snapshot<CkDev>, Result<Snapshot<Seq<RegionState>>, EmError>) {
    wfb_chain()
}

/// Mutant: claims the recovered-from copy 1 still holds seq 1 after the checkpoint.
#[ensures(match result.4.copy1 { Some(c) => c.seq@ == 1, None => false })]
pub fn refute_em_ckpt_after_fallback_preserves_valid__mutant() -> (Result<Snapshot<Seq<RegionState>>, EmError>, Snapshot<Seq<RegionState>>,
    Result<(), EmError>, Snapshot<CkDev>, Snapshot<CkDev>, Result<Snapshot<Seq<RegionState>>, EmError>) {
    wfb_chain()
}

// =============================================================================
// Continuing after initialize (CKPT-SNAPSHOT-RACE) and used_bytes after recovery
// (RECOVER-EMPTY-SLAB)
// =============================================================================

/// `wa_precrash` with every phase exposed: format; reserve K1 (slab A = [0,8)); publish K1;
/// remove_extent(0); checkpoint() — no interleaving, devices succeed. Returns (device, what
/// recovery reads back, used_bytes() right after the checkpoint completed).
#[ensures(result.0.sb_seq@ == 1 && result.0.sb_active@ == 1)]
#[ensures(match result.1 {
    Ok(img) => img.len() == 1 && img[0].slabs@.contains(0)
        && img[0].slabs@.lookup(0).start_offset@ == 0 && img[0].slabs@.lookup(0).slab_size@ == 8
        && img[0].slabs@.lookup(0).element_size@ == 4 && img[0].slabs@.lookup(0).keys@.len() == 2
        && img[0].slabs@.lookup(0).keys@[0] == FREE_KEY && img[0].slabs@.lookup(0).keys@[1] == FREE_KEY,
    Err(_) => false,
})]
#[ensures(result.2@ == 0)]
pub fn wa_full() -> (CkDev, Result<Snapshot<Seq<RegionState>>, EmError>, u64) {
    let fp = wc_fp(8, 16);
    let b = BuddyAllocator::new(0, 16, 4);
    proof_assert! { 2.pow2() == 4 && 16 / 4 == 4 };
    let mut r = RegionState::new(b, fp);
    proof_assert! { r.buddy.max_order@ == 2 && r.buddy.free_lists@[2]@ == Seq::singleton(0u64)
        && r.buddy.free_lists@[0]@.len() == 0 && r.buddy.free_lists@[1]@.len() == 0 };
    proof_assert! { lemma_span_pos(4, 2); span(4, 2) == 16 && span(4, 1) == 8 };
    proof_assert! { rg_inv(r) };
    proof_assert! { lemma_ord_eq(2, 0, 1); slab_k(r) == 1 };
    proof_assert! { align_l(4, 4) == 4 && slots_of(8, 4) == 2 };
    proof_assert! { first_ne(r.buddy.free_lists@, 1, 2) == 2 };
    proof_assert! { lemma_tf3(r.buddy.free_lists@, 4); tf(r.buddy.free_lists@, 4, 3) == 16 };
    let old = snapshot! { r };
    let res = r.alloc_extent(4);
    proof_assert! { fresh_case(*old, r, 4, res) };
    proof_assert! { match res { Ok((d, _, _)) => d@ == 0, Err(_) => false } };
    proof_assert! { r.buddy.free_lists@.len() == 3 && r.buddy.free_lists@[2]@.len() == 0
        && r.buddy.free_lists@[1]@.len() == 1 && r.buddy.free_lists@[0]@.len() == 0 };
    proof_assert! { lemma_tf3(r.buddy.free_lists@, 4); tf(r.buddy.free_lists@, 4, r.buddy.free_lists@.len()) == 8 };
    proof_assert! { r.slabs@.contains(0) && forall<k: Int> r.slabs@.contains(k) ==> k == 0 };
    let a0 = snapshot! { r.slabs@.lookup(0) };
    proof_assert! { a0.keys@.len() == 2 && a0.keys@[0] == FREE_KEY && a0.keys@[1] == FREE_KEY };
    proof_assert! { slot_bit(a0.bitmap, 0) && a0.start_offset@ == 0 && a0.element_size@ == 4 && a0.slab_size@ == 8 };
    proof_assert! { a0.bitmap.allocated_count@ == 1 };
    // LEVEL-3 reachable start (after format + reserve K1), as batch3 wa_precrash.
    let sb0 = l3_sb_c8_16();
    proof_assert! { l3_fmt_ranges(wc_fp_l(8u64, 16u64), 3, 4096, sb0) && l3_rsv_ranges(4, wc_fp_l(8u64, 16u64)) };
    proof_assert! { r.buddy.free_lists@[1]@ == Seq::singleton(8u64) };
    proof_assert! { forall<o: Int, k: Int> fv(r.buddy.free_lists@, o, k) ==> o == 1 && k == 0 };
    proof_assert! { rg_tf(r) == 8 && r.slabs@.len() == 1 && sk(r) == 8 };
    proof_assert! { p2(4) && pending_ok(r) && r.pending_frees@.len() == 0 };
    proof_assert! { sc_get(r.size_classes, 4) == Seq::singleton(0u64) };
    proof_assert! { fdisj(r.buddy.free_lists@, r.buddy.sector_size@) };
    proof_assert! { sl_away(r) };
    proof_assert! { rg_acct(r) };
    proof_assert! { sc_ok(r) };
    proof_assert! { rg_ka(r) };
    proof_assert! { rg_core(r) && rg_good(r) };
    proof_assert! { pv(r, hs_w1()) && lg(r, hs_w1()) };
    proof_assert! { lemma_coal_single(r.buddy.free_lists@, r.buddy.sector_size@); coal(r.buddy.free_lists@, r.buddy.sector_size@) };
    proof_assert! { nonfull_listed(r) && coal(r.buddy.free_lists@, r.buddy.sector_size@) && rg_l3(r, hs_w1()) };
    r.publish_slot(0, 0, WA_K1);
    let pre_rm = snapshot! { r };
    proof_assert! { slot_off(pre_rm.slabs@.lookup(0), 0) == 0 && pre_rm.slabs@.lookup(0).keys@[0] == WA_K1 };
    proof_assert! { rm_case(*pre_rm, 0u64, 0u64, 0usize) };
    let rm = r.remove_extent_by_offset(0);
    proof_assert! { rm == Ok(()) && rm_post(*pre_rm, r, 0u64, 0usize) };
    proof_assert! { r.slabs@.lookup(0).keys@ == a0.keys@.set(0, WA_K1).set(0, FREE_KEY) };
    proof_assert! { r.slabs@.lookup(0).keys@[0] == FREE_KEY && r.slabs@.lookup(0).keys@[1] == FREE_KEY };
    proof_assert! { r.slabs@.lookup(0).bitmap.allocated_count@ == 1 };
    proof_assert! { pending_ok(r) && r.pending_frees@ == Seq::singleton((0u64, 0usize)) };
    let snap = snapshot! { r };
    let mut rs: Vec<RegionState> = Vec::new();
    rs.push(r);
    let mut em = CkEm { regions: rs, active: 0, seq: 0, dev: wc_fresh_dev() };
    proof_assert! { em.regions@.len() == 1 && em.regions@[0] == *snap && em.regions@[0].dirty };
    proof_assert! { regions_ok(em.regions@) && !all_clean(em.regions@) };
    // checkpoint() (lib.rs:275-370) phase by phase, devices succeed, no interleaving
    let img = match em.ck_begin() {
        Some(img) => img,
        None => return (em.dev, Err(EmError::IoError), 1),
    };
    let c = em.ck_write(img, true, true);
    proof_assert! { c == Ok(()) && em.regions@[0] == *snap };
    // lib.rs:316-319 for the one region
    em.regions[0].dirty = false;
    let mid = snapshot! { em.regions@[0] };
    proof_assert! { mid.slabs == snap.slabs && mid.buddy == snap.buddy && mid.pending_frees == snap.pending_frees };
    em.regions[0].flush_pending_frees();
    proof_assert! { mid.slabs@.lookup(0).bitmap.allocated_count@ == 1 && slab_k(*mid) == 1 && mid.buddy.sector_size@ == 4 };
    proof_assert! { tf(mid.buddy.free_lists@, 4, mid.buddy.free_lists@.len()) == 8 };
    proof_assert! { flush1_tf(*mid, em.regions@[0], 0u64) };
    proof_assert! { tf(em.regions@[0].buddy.free_lists@, 4, em.regions@[0].buddy.free_lists@.len()) == 16 };
    proof_assert! { rg_inv(em.regions@[0]) && bd_shape(em.regions@[0].buddy) && em.regions@[0].buddy.total_usable_size@ == 16 };
    proof_assert! { em.regions@[0].buddy.sector_size@ == 4 };
    let used = region_used_release(&em.regions[0].buddy);
    let rec = recover(&em.dev);
    proof_assert! { match rec { Ok(i) => *i == *img, Err(_) => false } };
    (em.dev, rec, used)
}

/// **EM-INIT-POST-USED-BYTES** — REFUTED (INDEPENDENT; ROOT CAUSE = RECOVER-EMPTY-SLAB, the
/// code reader's own note: serialize_region (checkpoint.rs:18-30) persists a slab whose only
/// occupied slot is pending-free, the checkpoint's own flush (lib.rs:316-321) then releases
/// it, and initialize (lib.rs:552-565) re-reserves it as an allocated, EMPTY slab). "After
/// initialize() succeeds, used_bytes() equals the value the component reported right after
/// the recovered checkpoint completed, which is the number of slabs recovered from the
/// checkpoint multiplied by the slab size." Witness (sector 4 — NOT the buddy mask): right
/// after the checkpoint completed used_bytes() = 0 (`wa_full`); after a restart initialize
/// rebuilds slab A from that very checkpoint and used_bytes() = 8 (`wa_recovered_reserve_abort`,
/// batch 3) = 1 slab x 8 bytes. The two halves of the requirement disagree; the first fails.
#[ensures(result.0@ == 0 && result.1@ == 8 && result.0 != result.1)]
#[ensures(match result.2 { Ok(img) => img.len() == 1 && img[0].slabs@.contains(0) && img[0].slabs@.lookup(0).slab_size@ == 8, Err(_) => false })]
pub fn refute_em_init_post_used_bytes() -> (u64, u64, Result<Snapshot<Seq<RegionState>>, EmError>) {
    let (_dev, rec, used_ckpt) = wa_full();
    let (used_init, _u1, _u2) = wa_recovered_reserve_abort();
    (used_ckpt, used_init, rec)
}

/// Mutant: claims used_bytes() is preserved across the restart.
#[ensures(result.0 == result.1)]
pub fn refute_em_init_post_used_bytes__mutant() -> (u64, u64, Result<Snapshot<Seq<RegionState>>, EmError>) {
    let (_dev, rec, used_ckpt) = wa_full();
    let (used_init, _u1, _u2) = wa_recovered_reserve_abort();
    (used_ckpt, used_init, rec)
}

pub const WC_K1: u64 = 41;
pub const WC_K2: u64 = 42;

/// After initialize() from `wa_full`'s device (superblock seq 1 / copy 1; slab A rebuilt EMPTY
/// from the decoded descriptor — the descs_of round-trip assumption, as batch 3), the caller
/// reserves K1 and K2 (A's two slots) and publishes K1; checkpoint() #1 begins (the 30 s
/// background thread, or a concurrent caller) and serialises the region; publish(K2) runs
/// before #1 clears the dirty flags (CKPT-SNAPSHOT-RACE, lib.rs:299 -> 316); #1 completes
/// (copy 0 = seq 2: continues from the recovered seq 1); the caller's checkpoint() #2 finds
/// nothing dirty and returns Ok without I/O. A restart's recovery lacks K2.
/// Returns (#1, #2, device, the live region, what recovery reads back).
#[ensures(result.0 == Ok(()) && result.1 == Ok(()))]
#[ensures(result.2.sb_seq@ == 2 && result.2.sb_active@ == 0)]
#[ensures(exists<j: Int> result.3.slabs@.contains(0) && 0 <= j && j < result.3.slabs@.lookup(0).keys@.len()
    && result.3.slabs@.lookup(0).keys@[j] == WC_K2)]
#[ensures(match result.4 { Ok(img) => img_lacks_key(*img, WC_K2), Err(_) => false })]
pub fn wcont_lost() -> (Result<(), EmError>, Result<(), EmError>, Snapshot<CkDev>, RegionState, Result<Snapshot<Seq<RegionState>>, EmError>) {
    let (dev, _rec, _used) = wa_full();
    let fp = wc_fp(8, 16);
    let descs = wa_descs();
    fact_mask_0_16();
    proof_assert! { lemma_span_pos(4, 2); span(4, 2) == 16 && span(4, 1) == 8 && 2.pow2() == 4 };
    proof_assert! { lemma_ord_eq(2, 0, 1); ord(8 / 4) == 1 };
    proof_assert! { forall<sp: u64, x: u64> sp@ == span(4, 2) && x@ == 0 ==> sp == 16u64 && x == 0u64 };
    proof_assert! { desc_ok(descs@[0], 0, 16, fp) };
    proof_assert! { mark_hit_p2(descs@[0], 0, 16, fp, 2) };
    let mut r = rebuild_region(0, 16, fp, &descs);
    proof_assert! { one_slab(r, descs@[0]) };
    // LEVEL-3 reachable start (initialize of the wa_full device: superblock seq 1 / active 1,
    // parity-consistent): format-start ranges on that superblock, recovered-payload ranges
    // 980f61 / 3b612e on the descriptor, proved region invariants of the rebuilt region.
    let mut sb1 = l3_sb_c8_16();
    sb1.checkpoint_seq = 1;
    sb1.active_copy = 1;
    proof_assert! { l3_fmt_ranges(wc_fp_l(8u64, 16u64), 3, 4096, sb1) && l3_rsv_ranges(4, wc_fp_l(8u64, 16u64)) };
    proof_assert! { dev.sb_seq@ == sb1.checkpoint_seq@ && dev.sb_active@ == sb1.active_copy@ };
    proof_assert! { 8 / 4 == 2 && 8 % 8 == 0 && l3_rec_ranges(descs@, 0, 16, 8) };
    proof_assert! { starts_distinct(descs@) && p2(4) };
    proof_assert! { lemma_rg_good_o(r); rg_good(r) };
    proof_assert! { pv(r, Seq::empty()) && lg(r, Seq::empty()) };
    proof_assert! { lemma_tf3(r.buddy.free_lists@, 4); r.buddy.free_lists@.len() == 3 };
    proof_assert! { hit_lists(r.buddy, descs@[0], 0, fp, 2) };
    proof_assert! { forall<o: Int, k: Int> fv(r.buddy.free_lists@, o, k) ==> o == 1 && k == 0 };
    proof_assert! { lemma_coal_single(r.buddy.free_lists@, r.buddy.sector_size@); coal(r.buddy.free_lists@, r.buddy.sector_size@) };
    proof_assert! { nonfull_listed(r) && coal(r.buddy.free_lists@, r.buddy.sector_size@) && rg_l3(r, Seq::empty()) };
    proof_assert! { sc_get(r.size_classes, align_l(4, 4)) == Seq::singleton(0u64) && align_l(4, 4) == 4 };
    proof_assert! { r.slabs@.contains(0) && r.slabs@.lookup(0).bitmap.num_slots@ == 2 };
    proof_assert! { forall<j: Int> 0 <= j && j < 2 ==> !slot_bit(r.slabs@.lookup(0).bitmap, j) };
    proof_assert! { slab_inv(r.slabs@.lookup(0)) };
    proof_assert! { lemma_cnt_zero(r.slabs@.lookup(0).bitmap, 2); r.slabs@.lookup(0).bitmap.allocated_count@ == 0 };
    proof_assert! { forall<k: Int> r.slabs@.contains(k) ==> k == 0 };
    // reserve_extent(K1, 4)
    proof_assert! { exist_cond(r, 4, 0u64) };
    let o1 = snapshot! { r };
    let a1 = r.alloc_extent(4);
    proof_assert! { exist_case(*o1, r, 4, 0u64, a1) };
    let i1 = match a1 {
        Ok((_, i, _)) => i,
        Err(_) => return (Err(EmError::IoError), Err(EmError::IoError), snapshot! { dev }, r, Err(EmError::IoError)),
    };
    proof_assert! { slot_bit(r.slabs@.lookup(0).bitmap, i1@) && r.slabs@.lookup(0).bitmap.allocated_count@ == 1 && i1@ < 2 };
    proof_assert! { sc_get(r.size_classes, 4) == Seq::singleton(0u64) };
    proof_assert! { forall<k: Int> r.slabs@.contains(k) ==> k == 0 };
    // reserve_extent(K2, 4)
    proof_assert! { exist_cond(r, 4, 0u64) };
    let o2 = snapshot! { r };
    let a2 = r.alloc_extent(4);
    proof_assert! { exist_case(*o2, r, 4, 0u64, a2) };
    let i2 = match a2 {
        Ok((_, i, _)) => i,
        Err(_) => return (Err(EmError::IoError), Err(EmError::IoError), snapshot! { dev }, r, Err(EmError::IoError)),
    };
    proof_assert! { i2 != i1 && i2@ < 2 };
    proof_assert! { forall<k: Int> r.slabs@.contains(k) ==> k == 0 };
    proof_assert! { r.slabs@.lookup(0).keys@.len() == 2 && r.slabs@.lookup(0).keys@[0] == FREE_KEY && r.slabs@.lookup(0).keys@[1] == FREE_KEY };
    proof_assert! { rg_inv(r) && pending_ok(r) && r.pending_frees@.len() == 0 };
    let r0 = snapshot! { r };
    let mut rs: Vec<RegionState> = Vec::new();
    rs.push(r);
    // initialize(): in-memory superblock = recovered superblock (seq 1, active copy 1)
    let mut em = CkEm { regions: rs, active: 1, seq: 1, dev };
    proof_assert! { em.regions@.len() == 1 && em.regions@[0] == *r0 };
    // publish(K1)
    em.regions[0].publish_slot(0, i1, WC_K1);
    let s1 = snapshot! { em.regions@[0].slabs@.lookup(0) };
    proof_assert! { s1.keys@ == r0.slabs@.lookup(0).keys@.set(i1@, WC_K1) };
    proof_assert! { regions_ok(em.regions@) && !all_clean(em.regions@) };
    // checkpoint() #1, phase 1
    let img = match em.ck_begin() {
        Some(img) => img,
        None => return (Err(EmError::IoError), Err(EmError::IoError), snapshot! { em.dev }, em.regions.pop().unwrap(), Err(EmError::IoError)),
    };
    proof_assert! { *img == em.regions@ };
    // publish(K2) interleaves
    em.regions[0].publish_slot(0, i2, WC_K2);
    proof_assert! { em.regions@[0].slabs@.lookup(0).keys@[i2@] == WC_K2 };
    proof_assert! { regions_ok(em.regions@) };
    // checkpoint() #1, phases 2-4 + superblock write; then clear + flush
    let c1 = em.ck_write(img, true, true);
    proof_assert! { c1 == Ok(()) && em.seq@ == 2 && em.active@ == 0 };
    let live = snapshot! { em.regions@ };
    em.ck_finish();
    proof_assert! { fin_rel(live[0], em.regions@[0]) && live[0].pending_frees@.len() == 0 };
    proof_assert! { em.regions@[0].slabs == live[0].slabs };
    proof_assert! { live[0].slabs@.contains(0) && live[0].slabs@.lookup(0).keys@[i2@] == WC_K2 && i2@ < live[0].slabs@.lookup(0).keys@.len() };
    // checkpoint() #2: nothing dirty
    let c2 = em.run_checkpoint(true, true);
    proof_assert! { em.regions@[0].slabs == live[0].slabs };
    let rec = recover(&em.dev);
    proof_assert! { em.dev.sb_seq@ == 2 && em.dev.sb_active@ == 0 };
    proof_assert! { match em.dev.copy0 { Some(c) => c.seq@ == 2 && c.img == img, None => false } };
    proof_assert! { img[0].slabs@.contains(0) && forall<k: Int> img[0].slabs@.contains(k) ==> k == 0 };
    proof_assert! { img[0].slabs@.lookup(0) == *s1 && WC_K2 != WC_K1 && WC_K2 != FREE_KEY };
    proof_assert! { forall<j: Int> 0 <= j && j < 2 ==> s1.keys@[j] != WC_K2 };
    let d = snapshot! { em.dev };
    let last = em.regions.pop().unwrap();
    proof_assert! { last.slabs == live[0].slabs };
    proof_assert! { last.slabs@.contains(0) && 0 <= i2@ && i2@ < last.slabs@.lookup(0).keys@.len() && last.slabs@.lookup(0).keys@[i2@] == WC_K2 };
    (c1, c2, d, last, rec)
}

/// **EM-INIT-POST-CONTINUE** — REFUTED (INDEPENDENT; ROOT CAUSE = CKPT-SNAPSHOT-RACE:
/// lib.rs:312-321 clears `dirty` and flushes deferred frees for changes made after
/// checkpoint.rs:51-55 serialised the region; no lock spans the two). "After initialize()
/// succeeds, a subsequent change followed by a successful checkpoint is recovered by the next
/// initialize(); checkpointing continues correctly from the recovered sequence number."
/// Witness `wcont_lost`: after initialize (recovered seq 1) the change publish(K2) is
/// followed by a checkpoint() that returns Ok, the sequence did continue (superblock seq 2),
/// yet the next initialize recovers a state without K2. Without an interleaving the property
/// holds: `witness_em_init_post_continue_sequential` (unscored).
#[ensures(result.0 == Ok(()) && result.1.sb_seq@ == 2)]
#[ensures(exists<j: Int> result.2.slabs@.contains(0) && 0 <= j && j < result.2.slabs@.lookup(0).keys@.len()
    && result.2.slabs@.lookup(0).keys@[j] == WC_K2)]
#[ensures(match result.3 { Ok(img) => img_lacks_key(*img, WC_K2), Err(_) => false })]
pub fn refute_em_init_post_continue() -> (Result<(), EmError>, Snapshot<CkDev>, RegionState, Result<Snapshot<Seq<RegionState>>, EmError>) {
    let (_c1, c2, d, last, rec) = wcont_lost();
    (c2, d, last, rec)
}

/// Mutant: claims the recovered state holds K2 somewhere.
#[ensures(match result.3 { Ok(img) => !img_lacks_key(*img, WC_K2), Err(_) => false })]
pub fn refute_em_init_post_continue__mutant() -> (Result<(), EmError>, Snapshot<CkDev>, RegionState, Result<Snapshot<Seq<RegionState>>, EmError>) {
    let (_c1, c2, d, last, rec) = wcont_lost();
    (c2, d, last, rec)
}

/// Sequential half of EM-INIT-POST-CONTINUE (unscored): for every post-initialize checkpoint
/// state (in-memory superblock = the recovered one: any active copy <= 1 and seq < u64::MAX,
/// any device) in which a change left a region dirty and no call interleaves the checkpoint,
/// a successful checkpoint writes seq + 1 and a fresh recovery returns exactly the regions it
/// serialised.
#[requires(regions_ok(em.regions@) && em.active@ <= 1 && em.seq@ < u64::MAX@ && !all_clean(em.regions@))]
#[ensures(match result.0 {
    Ok(()) => (^em).dev.sb_seq@ == em.seq@ + 1 && match result.1 { Ok(img) => *img == em.regions@, Err(_) => false },
    Err(e) => e == EmError::IoError,
})]
pub fn witness_em_init_post_continue_sequential(em: &mut CkEm, ok_data: bool, ok_sb: bool) -> (Result<(), EmError>, Result<Snapshot<Seq<RegionState>>, EmError>) {
    let c = em.run_checkpoint(ok_data, ok_sb);
    let rec = recover(&em.dev);
    (c, rec)
}

// =============================================================================
// BUDDY-MASK refutations (non-power-of-two sector size) and their pow2 witnesses
// =============================================================================

/// **EM-BUDDY-MARK-ALLOCATED** — REFUTED. ROOT CAUSE = BUDDY-MASK (non-power-of-two sector
/// size): buddy.rs:136 finds the free block enclosing the range with `offset & !(span - 1)`,
/// correct only when `span` (a power of two times sector_size) is a power of two. "Marking a
/// free range as allocated (during recovery) removes exactly that range from the free space,
/// reducing the free total by its block size, and leaves every other free byte free." W1
/// (sector 3, region [0,6), FR-002-valid): BuddyAllocator::new(0, 6, 3) has the whole region
/// [0,6) free (one order-1 block at 0, free total 6); initialize's mark_allocated(3, 3)
/// (lib.rs:552-555) computes the mask 3 & !(6-1) = 2, finds no free block at 2, and returns
/// with NOTHING removed: the range [3,6) stays free and the free total stays 6 (not 3).
#[ensures(*result.0 == 6 && *result.1 == 6)]
#[ensures(result.2.free_lists@.len() == 2 && result.2.free_lists@[1]@ == Seq::singleton(0u64) && span(3, 1) == 6)]
pub fn support_old_refute_em_buddy_mark_allocated() -> (Snapshot<Int>, Snapshot<Int>, BuddyAllocator) {
    let fresh = BuddyAllocator::new(0, 6, 3);
    proof_assert! { 1.pow2() == 2 && 6 / 3 == 2 };
    proof_assert! { fresh.max_order@ == 1 && fresh.free_lists@[1]@ == Seq::singleton(0u64) && fresh.free_lists@[0]@.len() == 0 };
    proof_assert! { fresh.free_lists@.len() == 2 };
    proof_assert! { lemma_tf2(fresh.free_lists@, 3); tf(fresh.free_lists@, 3, 2) == 6 };
    let before = snapshot! { tf(fresh.free_lists@, 3, 2) };
    // w1_recovered = format's BuddyAllocator::new(0, 6, 3) + mark_allocated(3, 3)
    let (_y, b) = w1_recovered();
    proof_assert! { lemma_tf2(b.free_lists@, 3); tf(b.free_lists@, 3, 2) == 6 };
    let after = snapshot! { tf(b.free_lists@, 3, 2) };
    proof_assert! { span(3, 1) == 6 };
    (before, after, b)
}

/// Mutant: claims the free total dropped to 3.
#[ensures(*result.1 == 3)]
pub fn support_old_refute_em_buddy_mark_allocated__mutant() -> (Snapshot<Int>, Snapshot<Int>, BuddyAllocator) {
    let fresh = BuddyAllocator::new(0, 6, 3);
    proof_assert! { 1.pow2() == 2 && 6 / 3 == 2 };
    proof_assert! { fresh.max_order@ == 1 && fresh.free_lists@[1]@ == Seq::singleton(0u64) && fresh.free_lists@[0]@.len() == 0 };
    proof_assert! { lemma_tf2(fresh.free_lists@, 3); tf(fresh.free_lists@, 3, 2) == 6 };
    let before = snapshot! { tf(fresh.free_lists@, 3, 2) };
    let (_y, b) = w1_recovered();
    proof_assert! { lemma_tf2(b.free_lists@, 3); tf(b.free_lists@, 3, 2) == 6 };
    let after = snapshot! { tf(b.free_lists@, 3, 2) };
    (before, after, b)
}

/// pow2 witness of EM-BUDDY-MARK-ALLOCATED: W1p (sector 4, region [0,8)): mark_allocated(4, 4)
/// takes exactly [4,8) off the free space (free total 8 -> 4) and leaves [0,4) free.
#[ensures(*result.0 == 8 && *result.1 == 4 && *result.0 - *result.1 == span(4, 0))]
#[ensures(result.2.free_lists@[0]@ == Seq::singleton(0u64) && result.2.free_lists@[1]@.len() == 0)]
pub fn witness_em_buddy_mark_allocated_pow2() -> (Snapshot<Int>, Snapshot<Int>, BuddyAllocator) {
    let fresh = BuddyAllocator::new(0, 8, 4);
    proof_assert! { 1.pow2() == 2 && 8 / 4 == 2 };
    proof_assert! { fresh.max_order@ == 1 && fresh.free_lists@[1]@ == Seq::singleton(0u64) && fresh.free_lists@[0]@.len() == 0 };
    proof_assert! { lemma_tf2(fresh.free_lists@, 4); tf(fresh.free_lists@, 4, 2) == 8 };
    let before = snapshot! { tf(fresh.free_lists@, 4, 2) };
    let (_y, b) = w1p_recovered();
    proof_assert! { lemma_tf2(b.free_lists@, 4); tf(b.free_lists@, 4, 2) == 4 };
    let after = snapshot! { tf(b.free_lists@, 4, 2) };
    proof_assert! { span(4, 0) == 4 };
    (before, after, b)
}

/// **EM-INV-KEY-SHARDING** — REFUTED. ROOT CAUSE = BUDDY-MASK (non-power-of-two sector
/// size), witness W3 (batch 1, EM-INV-WITHIN-USABLE-RANGE). "An extent reserved for key k
/// lies inside the contiguous data-device byte range of region number (k AND (region_count
/// minus 1)) ..." W3 has ONE region, so every key maps to region `k & 0 = 0` (lib.rs:219),
/// whose range is [0, 24); after the non-power-of-two recovery cascade one of three
/// reserve_extent(_, 6) calls is handed [21, 27), which ends past 24. (The index part —
/// region_for_key uses `key & (len - 1)` — is what `and_mask` states.)
#[ensures(result.4@ == 0)]
#[ensures(result.3@ == 6)]
#[ensures(result.0@ + result.3@ > 24 || result.1@ + result.3@ > 24 || result.2@ + result.3@ > 24)]
pub fn support_old_refute_em_inv_key_sharding() -> (u64, u64, u64, u32, usize) {
    let (o1, o2, o3, a) = support_old_refute_em_inv_within_usable_range();
    let idx = and_mask(W_K2 as usize, 0);
    (o1, o2, o3, a, idx)
}

/// Mutant: claims every reported extent lies inside region 0 = [0, 24).
#[ensures(result.0@ + result.3@ <= 24 && result.1@ + result.3@ <= 24 && result.2@ + result.3@ <= 24)]
pub fn support_old_refute_em_inv_key_sharding__mutant() -> (u64, u64, u64, u32, usize) {
    let (o1, o2, o3, a) = support_old_refute_em_inv_within_usable_range();
    let idx = and_mask(W_K2 as usize, 0);
    (o1, o2, o3, a, idx)
}

/// pow2 witness of EM-INV-KEY-SHARDING: W4p (the W3/W4 scenario at sector 4; one region
/// [0, 32), key & 0 = 0): the reservation served from the recovered slab reports [24, 32),
/// inside region 0.
#[ensures(result.0@ == 0 && result.1@ + result.2@ <= 32 && result.1@ >= 0)]
pub fn witness_em_inv_key_sharding_pow2() -> (usize, u64, u32) {
    let (_r, off, sz) = witness_em_publish_post_visible_pow2();
    let idx = and_mask(WP_K5 as usize, 0);
    (idx, off, sz)
}

/// **EM-INIT-POST-RECOVERED-SLOTS-OCCUPIED** — REFUTED. ROOT CAUSE = BUDDY-MASK
/// (non-power-of-two sector size), witness W2b (batch 1). "After initialize() succeeds, every
/// slot whose recovered key is not the FREE_KEY sentinel is treated as occupied, so no later
/// reserve_extent() returns space overlapping the disk range of any recovered extent." W2
/// (sector 3, slab 6, one region [0,12)): initialize recovers K3 in slot 0 of slab B at 6,
/// i.e. the extent [6, 9) (`w2_recovered`); the mask misses (6 & !(12-1) = 4) so B's block
/// stays on the free lists; reserve(K4,3), reserve(K5,6), reserve(K6,6) (`w2b_replaced`):
/// the third is handed offset 6 with size 6 — [6, 12) overlaps the recovered [6, 9).
#[ensures(result.0.slabs@.contains(6) && result.0.slabs@.lookup(6).keys@[0] == W_K3 && W_K3 != FREE_KEY)]
#[ensures(slot_off(result.0.slabs@.lookup(6), 0) == 6 && result.0.slabs@.lookup(6).element_size@ == 3)]
#[ensures(result.1@ == 6 && result.1@ < 6 + 3 && 6 < result.1@ + 6)]
pub fn support_old_refute_em_init_post_recovered_slots_occupied() -> (RegionState, u64) {
    let rec = w2_recovered();
    let (_r, _s4, _i4, _off4, off6) = w2b_replaced();
    (rec, off6)
}

/// Mutant: claims the third reservation starts at or after the recovered extent's end.
#[ensures(result.1@ >= 9)]
pub fn support_old_refute_em_init_post_recovered_slots_occupied__mutant() -> (RegionState, u64) {
    let rec = w2_recovered();
    let (_r, _s4, _i4, _off4, off6) = w2b_replaced();
    (rec, off6)
}

/// pow2 witness of EM-INIT-POST-RECOVERED-SLOTS-OCCUPIED: W2p (the W2 history at sector 4,
/// one region [0,16); recovered K3 = [8, 12) in slot 0 of B at 8): reserve(K4,4) gets [12,16)
/// (B's slot 1), reserve(K5,8) gets [0,8), reserve(K6,8) gets OutOfSpace — no reservation
/// overlaps [8, 12).
#[ensures(match result.0 { Ok((_, _, off)) => off@ == 12, Err(_) => false })]
#[ensures(match result.1 { Ok((_, _, off)) => off@ == 0, Err(_) => false })]
#[ensures(result.2 == Err(EmError::OutOfSpace))]
#[ensures((*result.3).slabs@.contains(8) && (*result.3).slabs@.lookup(8).keys@[0] == WP_K3 && slot_off((*result.3).slabs@.lookup(8), 0) == 8)]
#[ensures((*result.3).slabs@.lookup(8).element_size@ == 4)]
pub fn witness_em_init_post_recovered_slots_occupied_pow2() -> (Result<(u64, usize, u64), EmError>, Result<(u64, usize, u64), EmError>,
    Result<(u64, usize, u64), EmError>, Snapshot<RegionState>) {
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
    let rec = snapshot! { r };
    // reserve(K4, 4): B's free slot 1
    let o1 = snapshot! { r };
    proof_assert! { exist_cond(*o1, 4, 8u64) };
    let h4 = r.alloc_extent(4);
    proof_assert! { exist_case(*o1, r, 4, 8u64, h4) };
    proof_assert! { match h4 { Ok((s, i, off)) => s@ == 8 && i@ < 2 && !slot_bit(bb.bitmap, i@), Err(_) => false } };
    proof_assert! { match h4 { Ok((s, i, off)) => i@ == 1 && off@ == slot_off(*bb, 1) && off@ == 12, Err(_) => false } };
    proof_assert! { filter_ne(Seq::singleton(8u64), 8u64) == Seq::empty() };
    proof_assert! { sc_get(r.size_classes, 4) == Seq::empty() && sc_get(r.size_classes, 8) == Seq::empty() };
    proof_assert! { first_ne(r.buddy.free_lists@, 1, 2) == 1 };
    // reserve(K5, 8): fresh slab at 0
    let o2 = snapshot! { r };
    let h5 = r.alloc_extent(8);
    proof_assert! { fresh_case(*o2, r, 8, h5) };
    proof_assert! { match h5 { Ok((d, _, off)) => d@ == 0 && off@ == 0, Err(_) => false } };
    proof_assert! { r.buddy.free_lists@[1]@.len() == 0 && r.buddy.free_lists@[2]@.len() == 0 };
    proof_assert! { first_ne(r.buddy.free_lists@, 1, 2) == -1 };
    proof_assert! { sc_get(r.size_classes, 8) == Seq::empty() };
    // reserve(K6, 8): no free block of the slab order -> OutOfSpace
    let o3 = snapshot! { r };
    let h6 = r.alloc_extent(8);
    proof_assert! { fresh_case(*o3, r, 8, h6) && r == *o3 };
    proof_assert! { rec.slabs@.lookup(8).keys@[0] == WP_K3 && slot_off(rec.slabs@.lookup(8), 0) == 8 };
    (h4, h5, h6, rec)
}

pub const WR_K6: u64 = 66;

/// W2 for the checkpoint frame: after initialize (`w2_recovered`: B at 6 holds K3 in slot 0),
/// remove_extent(6) (K3 -> FREE_KEY, (6, 0) queued as a deferred free, region.rs:142-148);
/// reserve(K5, 6) carves D at 0 (the buddy splits [0,12), [6,12) left on order 1);
/// reserve(K6, 6) carves E = [6,12) — B's block, never taken off the free lists
/// (buddy.rs:136) — and E REPLACES B at key 6 (region.rs:89); publish(K6) writes K6 into E's
/// slot 0. The queued free (6, 0) now names E's slot 0. checkpoint() (no interleaving, devices
/// succeed) flushes it (lib.rs:316-319 -> region.rs:94-107): E empties and is dropped, so
/// the published K6 is no longer listed. Returns (checkpoint result, region before, region
/// after).
#[ensures(result.0 == Ok(()))]
#[ensures(from_slot(*result.1, 6, 0, Extent { key: WR_K6, size: 6u32, offset: 6u64 }))]
#[ensures(!from_region(*result.2, Extent { key: WR_K6, size: 6u32, offset: 6u64 }))]
#[ensures((*result.1).buddy.total_usable_size == (*result.2).buddy.total_usable_size)]
pub fn wfr_chain() -> (Result<(), EmError>, Snapshot<RegionState>, Snapshot<RegionState>) {
    let mut r = w2_recovered();
    proof_assert! { forall<k: Int> r.slabs@.contains(k) ==> k == 6 };
    // remove_extent(6)
    let pre = snapshot! { r };
    proof_assert! { slot_off(pre.slabs@.lookup(6), 0) == 6 && W_K3 != FREE_KEY };
    proof_assert! { rm_case(*pre, 6u64, 6u64, 0usize) };
    let rm = r.remove_extent_by_offset(6);
    proof_assert! { rm == Ok(()) && rm_post(*pre, r, 6u64, 0usize) };
    proof_assert! { r.pending_frees@ == Seq::singleton((6u64, 0usize)) };
    proof_assert! { forall<k: Int> r.slabs@.contains(k) ==> k == 6 };
    // reserve(K5, 6): size class 6 empty -> fresh slab D at 0
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
    proof_assert! { forall<k: Int> r.slabs@.contains(k) ==> k == 6 || k == 0 };
    proof_assert! { forall<j: Int> 0 <= j && j < r.slabs@.lookup(0).keys@.len() ==> r.slabs@.lookup(0).keys@[j] == FREE_KEY };
    proof_assert! { r.pending_frees@ == Seq::singleton((6u64, 0usize)) };
    proof_assert! { first_ne(r.buddy.free_lists@, 1, 2) == 1 };
    // reserve(K6, 6): fresh slab E at 6, replacing B
    let o2 = snapshot! { r };
    let a2 = r.alloc_extent(6);
    proof_assert! { fresh_case(*o2, r, 6, a2) };
    proof_assert! { match a2 { Ok((d, _, _)) => d@ == 6, Err(_) => false } };
    let e = snapshot! { r.slabs@.lookup(6) };
    proof_assert! { fresh_one(*e, 6, 6, 6) };
    proof_assert! { e.bitmap.num_slots@ == 1 && slot_bit(e.bitmap, 0) && e.bitmap.allocated_count@ == 1 && e.keys@.len() == 1 };
    proof_assert! { forall<k: Int> r.slabs@.contains(k) ==> k == 6 || k == 0 };
    proof_assert! { r.slabs@.lookup(0) == o2.slabs@.lookup(0) };
    proof_assert! { forall<j: Int> 0 <= j && j < r.slabs@.lookup(0).keys@.len() ==> r.slabs@.lookup(0).keys@[j] == FREE_KEY };
    // publish(K6)
    r.publish_slot(6, 0, WR_K6);
    proof_assert! { r.slabs@.lookup(6).keys@ == e.keys@.set(0, WR_K6) && r.slabs@.lookup(6).bitmap == e.bitmap };
    proof_assert! { r.slabs@.lookup(6).keys@[0] == WR_K6 && slot_off(r.slabs@.lookup(6), 0) == 6 && r.slabs@.lookup(6).element_size@ == 6 };
    proof_assert! { r.slabs@.lookup(0) == o2.slabs@.lookup(0) };
    proof_assert! { forall<k: Int> r.slabs@.contains(k) ==> k == 6 || k == 0 };
    proof_assert! { r.pending_frees@ == Seq::singleton((6u64, 0usize)) };
    proof_assert! { pending_ok(r) && rg_inv(r) && r.dirty };
    proof_assert! { from_slot(r, 6, 0, Extent { key: WR_K6, size: 6u32, offset: 6u64 }) };
    let before = snapshot! { r };
    let mut rs: Vec<RegionState> = Vec::new();
    rs.push(r);
    let mut em = CkEm { regions: rs, active: 0, seq: 0, dev: wc_fresh_dev() };
    proof_assert! { em.regions@.len() == 1 && em.regions@[0] == *before };
    proof_assert! { regions_ok(em.regions@) && !all_clean(em.regions@) };
    let c = em.run_checkpoint(true, true);
    proof_assert! { c == Ok(()) };
    let after = snapshot! { em.regions@[0] };
    proof_assert! { fin_rel(*before, *after) };
    proof_assert! { free_case(*before, *after, 6u64, 0usize, before.slabs@.lookup(6)) };
    proof_assert! { after.slabs@ == before.slabs@.remove(6) };
    proof_assert! { forall<k: Int> after.slabs@.contains(k) ==> k == 0 };
    proof_assert! { after.slabs@.lookup(0) == before.slabs@.lookup(0) };
    proof_assert! { forall<j: Int> 0 <= j && j < after.slabs@.lookup(0).keys@.len() ==> after.slabs@.lookup(0).keys@[j] == FREE_KEY };
    proof_assert! { forall<k: Int, j: Int> !from_slot(*after, k, j, Extent { key: WR_K6, size: 6u32, offset: 6u64 }) };
    (c, before, after)
}

/// **EM-CKPT-FRAME-ENUMERATION-UNCHANGED** — REFUTED. ROOT CAUSE = BUDDY-MASK
/// (non-power-of-two sector size). "checkpoint() does not change which extents get_extents()
/// lists, their keys, offsets or sizes, or the reported capacity." Witness `wfr_chain` (W2,
/// sector 3): after the non-power-of-two recovery a fresh slab replaces the recovered one at
/// its key while a deferred free still names its slot 0; checkpoint() flushes that free into
/// the REPLACEMENT slab and drops the published extent (K6, 6, 6): get_extents() (sound and
/// complete over the slot facts, model/listing.rs) lists it before the checkpoint and not
/// after. (The capacity part holds: the checkpoint mirror preserves every region's geometry,
/// rg_frame.)
#[ensures(result.0 == Ok(()))]
#[ensures(from_region(*result.1, Extent { key: WR_K6, size: 6u32, offset: 6u64 }))]
#[ensures(!from_region(*result.2, Extent { key: WR_K6, size: 6u32, offset: 6u64 }))]
pub fn support_old_refute_em_ckpt_frame_enumeration_unchanged() -> (Result<(), EmError>, Snapshot<RegionState>, Snapshot<RegionState>) {
    wfr_chain()
}

/// Mutant: claims the extent is still listed after the checkpoint.
#[ensures(from_region(*result.2, Extent { key: WR_K6, size: 6u32, offset: 6u64 }))]
pub fn support_old_refute_em_ckpt_frame_enumeration_unchanged__mutant() -> (Result<(), EmError>, Snapshot<RegionState>, Snapshot<RegionState>) {
    wfr_chain()
}

/// pow2 witness of EM-CKPT-FRAME-ENUMERATION-UNCHANGED: W2p (sector 4; recovered K3 in slot 0
/// of B at 8): remove_extent(8) queues (8, 0); reserve(K5,8) carves D at 0; reserve(K6,8) finds
/// no space (no replacement). checkpoint() flushes (8, 0) into B itself; the listing is the
/// same before and after (here: empty — K3 was removed, D holds no key) and so is the
/// capacity.
#[ensures(result.0 == Ok(()))]
#[ensures(forall<e: Extent> from_region(*result.1, e) == from_region(*result.2, e))]
#[ensures((*result.1).buddy.total_usable_size == (*result.2).buddy.total_usable_size)]
pub fn witness_em_ckpt_frame_enumeration_unchanged_pow2() -> (Result<(), EmError>, Snapshot<RegionState>, Snapshot<RegionState>) {
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
    proof_assert! { bb.keys@[0] == WP_K3 && bb.keys@[1] == FREE_KEY && bb.keys@.len() == 2 };
    proof_assert! { r.buddy.free_lists@[1]@ == Seq::singleton(0u64) || (r.buddy.free_lists@[1]@.len() == 1 && r.buddy.free_lists@[1]@[0]@ == 0) };
    proof_assert! { slab_k(r) == 1 && align_l(8, 4) == 8 };
    proof_assert! { forall<k: Int> r.slabs@.contains(k) ==> k == 8 };
    proof_assert! { r.pending_frees@.len() == 0 };
    // remove_extent(8)
    let pre = snapshot! { r };
    proof_assert! { slot_off(pre.slabs@.lookup(8), 0) == 8 && WP_K3 != FREE_KEY };
    proof_assert! { rm_case(*pre, 8u64, 8u64, 0usize) };
    let rm = r.remove_extent_by_offset(8);
    proof_assert! { rm == Ok(()) && rm_post(*pre, r, 8u64, 0usize) };
    proof_assert! { r.pending_frees@ == Seq::singleton((8u64, 0usize)) };
    proof_assert! { r.slabs@.lookup(8).keys@[0] == FREE_KEY && r.slabs@.lookup(8).keys@[1] == FREE_KEY && r.slabs@.lookup(8).keys@.len() == 2 };
    proof_assert! { sc_get(r.size_classes, 8) == Seq::empty() };
    proof_assert! { first_ne(r.buddy.free_lists@, 1, 2) == 1 };
    // reserve(K5, 8): fresh slab D at 0
    let o2 = snapshot! { r };
    let h5 = r.alloc_extent(8);
    proof_assert! { fresh_case(*o2, r, 8, h5) };
    proof_assert! { match h5 { Ok((d, _, _)) => d@ == 0, Err(_) => false } };
    proof_assert! { r.buddy.free_lists@[1]@.len() == 0 && r.buddy.free_lists@[2]@.len() == 0 };
    proof_assert! { first_ne(r.buddy.free_lists@, 1, 2) == -1 };
    proof_assert! { forall<k: Int> r.slabs@.contains(k) ==> k == 8 || k == 0 };
    proof_assert! { r.slabs@.lookup(8) == o2.slabs@.lookup(8) };
    proof_assert! { forall<j: Int> 0 <= j && j < r.slabs@.lookup(0).keys@.len() ==> r.slabs@.lookup(0).keys@[j] == FREE_KEY };
    // reserve(K6, 8): OutOfSpace, region unchanged
    let o3 = snapshot! { r };
    let h6 = r.alloc_extent(8);
    proof_assert! { fresh_case(*o3, r, 8, h6) && r == *o3 };
    proof_assert! { pending_ok(r) && rg_inv(r) && r.dirty };
    proof_assert! { forall<k: Int, j: Int> 0 <= j && r.slabs@.contains(k) && j < r.slabs@.lookup(k).keys@.len() ==> r.slabs@.lookup(k).keys@[j] == FREE_KEY };
    let before = snapshot! { r };
    let mut rs: Vec<RegionState> = Vec::new();
    rs.push(r);
    let mut em = CkEm { regions: rs, active: 0, seq: 0, dev: wc_fresh_dev() };
    proof_assert! { em.regions@.len() == 1 && em.regions@[0] == *before };
    proof_assert! { regions_ok(em.regions@) && !all_clean(em.regions@) };
    let c = em.run_checkpoint(true, true);
    let after = snapshot! { em.regions@[0] };
    proof_assert! { fin_rel(*before, *after) };
    proof_assert! { free_case(*before, *after, 8u64, 0usize, before.slabs@.lookup(8)) };
    proof_assert! { before.slabs@.lookup(8).bitmap.allocated_count@ == 1 };
    proof_assert! { after.slabs@ == before.slabs@.remove(8) };
    proof_assert! { forall<k: Int, j: Int> 0 <= j && after.slabs@.contains(k) && j < after.slabs@.lookup(k).keys@.len() ==> after.slabs@.lookup(k).keys@[j] == FREE_KEY };
    proof_assert! { forall<e: Extent> !from_region(*before, e) && !from_region(*after, e) };
    (c, before, after)
}

// =============================================================================
// Single checkpoint writer (coalescing protocol, lib.rs:729-761)
// =============================================================================

/// The coalescer right after construction (`CheckpointCoalesce::default()`, lib.rs:53-57)
/// with `n` caller threads, none inside checkpoint().
#[ensures(co_inv(result) && result.ts@.len() == n@ && result.co.completed_seq@ == 0 && !result.co.in_progress)]
pub fn co_init(n: usize) -> CoSys {
    let mut ts: Vec<Th> = Vec::new();
    let mut i: usize = 0;
    #[invariant(i@ <= n@ && ts@.len() == i@)]
    #[invariant(forall<j: Int> 0 <= j && j < i@ ==> ts@[j] == Th::Idle)]
    while i < n {
        ts.push(Th::Idle);
        i += 1;
    }
    CoSys { co: Co { completed_seq: 0, in_progress: false }, ts }
}

/// **EM-CKPT-INV-SINGLE-WRITER** — "At most one checkpoint I/O operation is in progress at
/// any instant, regardless of how many callers (including the background timer) request
/// checkpoints concurrently." The coalescing protocol of checkpoint() (lib.rs:729-761):
/// each `checkpoint_coalesce` critical section is one atomic step (Mutex), a condvar wait
/// releases the lock and may wake at any time (notify_all or spuriously), and ALL checkpoint
/// I/O happens inside run_checkpoint() (lib.rs:750 -> 297-310), between `drop(state)` and
/// the re-lock — the `Running` phase. Proved for ANY number of caller threads (user threads
/// and the background timer, lib.rs:154, call the same function) and ANY schedule of their
/// steps (enter / wake-up / run_checkpoint returns with any outcome): the inductive
/// invariant co_inv — every running thread implies `in_progress`, and no two threads run —
/// holds after every step, so at every instant at most one checkpoint I/O is in progress.
// LEVEL-3 D-B1: the schedule-length headroom premise `2 * sched.len() + 2 <= u64::MAX` is
// gone — CoSys::enter now models the completed_seq overflow of lib.rs:731-735 (debug panic /
// release wrap: no checkpoint starts) instead of excluding it.
#[ensures(co_inv(result) && co_single(result) && result.ts@.len() == n@)]
pub fn verify_em_ckpt_inv_single_writer(n: usize, sched: &Vec<(usize, CoOp)>) -> CoSys {
    let mut sys = co_init(n);
    let mut k: usize = 0;
    #[invariant(k@ <= sched@.len() && co_inv(sys) && sys.ts@.len() == n@)]
    while k < sched.len() {
        let (t, op) = sched[k];
        if t < sys.ts.len() {
            match op {
                CoOp::Enter => sys.enter(t),
                CoOp::Wake => sys.wake(t),
                CoOp::Finish(ok) => sys.finish(t, ok),
            }
        }
        k += 1;
    }
    sys
}

/// Mutant step: checkpoint() WITHOUT the `in_progress` test (lib.rs:741) — every caller with
/// `completed_seq < needed` starts run_checkpoint at once.
#[requires(t@ < sys.ts@.len())]
#[ensures((^sys).ts@.len() == sys.ts@.len())]
pub fn enter_unguarded(sys: &mut CoSys, t: usize) {
    match sys.ts[t] {
        Th::Idle => {
            if sys.co.completed_seq < u64::MAX {
                let needed = sys.co.completed_seq + 1;
                sys.co.in_progress = true;
                sys.ts[t] = Th::Running(needed);
            }
        }
        _ => {}
    }
}

/// Mutant: the same claim for the protocol without the `in_progress` guard (must FAIL).
#[ensures(co_single(result) && result.ts@.len() == n@)]
pub fn verify_em_ckpt_inv_single_writer__mutant(n: usize, sched: &Vec<(usize, CoOp)>) -> CoSys {
    let mut sys = co_init(n);
    let mut k: usize = 0;
    #[invariant(k@ <= sched@.len() && sys.ts@.len() == n@)]
    while k < sched.len() {
        let (t, op) = sched[k];
        if t < sys.ts.len() {
            match op {
                CoOp::Enter => enter_unguarded(&mut sys, t),
                _ => {}
            }
        }
        k += 1;
    }
    sys
}

// =============================================================================
// Checkpoint copy bytes: header seq / round trip (new root cause CKPT-LEN-U32) and CRC
// =============================================================================

/// Round trip (unscored; the positive half of EM-CKPT-POST-SEQ-MATCHES-HEADER): for every
/// payload shorter than 4 GiB, a copy written by write_checkpoint with seq `s` reads back
/// with expected seq `s`, is ACCEPTED, and returns exactly the payload.
#[requires(ss@ >= 16 && region_size@ <= area@.len() && region_size@ % ss@ == 0 && area@.len() + ss@ <= usize::MAX@)]
#[requires(16 + payload@.len() + ss@ <= usize::MAX@)]
#[requires(payload@.len() < 4294967296)]
#[ensures(match result.0 {
    Ok(()) => match result.1 { Ok(v) => v@ == payload@, Err(_) => false },
    Err(e) => e == EmError::CorruptMetadata,
})]
pub fn witness_em_ckpt_post_seq_matches_header_small(area: &mut Vec<u8>, ss: usize, region_size: u64, seq: u64, payload: &Vec<u8>)
    -> (Result<(), EmError>, Result<Vec<u8>, EmError>) {
    let w = write_ckpt_bytes(area, ss, region_size, seq, payload);
    match w {
        Ok(()) => {
            let l = as_u32(payload.len() as u64);
            proof_assert! { is_trunc32(l, payload@.len()) && l@ == payload@.len() };
            let n = snapshot! { alignup(16 + payload@.len(), ss@) };
            let b = snapshot! { area@.subsequence(0, *n) };
            proof_assert! { blob_ok(*b, seq, l, payload@, ss@) };
            proof_assert! { lemma_divmod(16 + payload@.len() + ss@ - 1, ss@); lemma_alignup_le(16 + payload@.len(), ss@, region_size@); 16 + payload@.len() <= *n && *n <= area@.len() };
            proof_assert! { forall<j: Int> 0 <= j && j < *n ==> area@[j] == b[j] };
            proof_assert! { lemma_dec64(area@.subsequence(0, 8), seq); dec64(area@.subsequence(0, 8)) == seq };
            proof_assert! { lemma_dec32(area@.subsequence(8, 12), l); dec32(area@.subsequence(8, 12)) == l };
            proof_assert! { chk(area@, 16 + payload@.len()).ext_eq(chk(*b, 16 + payload@.len())) };
            proof_assert! { lemma_dec32(area@.subsequence(12, 16), crc32(chk(*b, 16 + payload@.len())));
                dec32(area@.subsequence(12, 16)) == crc32(chk(area@, 16 + payload@.len())) };
            proof_assert! { rd_accepts(area@, region_size@, seq) };
            proof_assert! { area@.subsequence(16, 16 + payload@.len()).ext_eq(payload@) };
            let r = read_ckpt_bytes(area, ss, region_size, seq);
            (w, r)
        }
        Err(e) => (w, Err(e)),
    }
}

/// **EM-CKPT-POST-SEQ-MATCHES-HEADER** — REFUTED (INDEPENDENT; new root cause CKPT-LEN-U32:
/// write_checkpoint checks the FULL payload length against the region size
/// (checkpoint.rs:69-75) but stores it as `payload.len() as u32` (checkpoint.rs:82), and
/// read_checkpoint_region trusts that 32-bit field (checkpoint.rs:133, 142-172)). "After a
/// checkpoint succeeds, the sequence number in the header of the newly active checkpoint copy
/// equals the checkpoint sequence number stored in the superblock, and reading that copy back
/// with that sequence number is accepted and returns exactly the payload that was written."
/// For EVERY payload of exactly 4 GiB (2^32 bytes: about 2^29 slots of 8-byte keys) and every
/// copy region that fits it (separate-device mode, metadata_region_size 0, on a metadata device
/// of >= 8 GiB + 4 KiB): the write succeeds with the header length field 0, and reading the copy
/// back with its own seq returns either CorruptMetadata or an EMPTY payload — never the 4 GiB
/// that were written. (The header seq does equal the superblock's: checkpoint.rs:81 and
/// :110 store the same `new_seq`; payloads < 4 GiB round-trip:
/// `witness_em_ckpt_post_seq_matches_header_small`.)
#[requires(ss@ >= 16 && region_size@ <= area@.len() && region_size@ % ss@ == 0 && area@.len() + ss@ <= usize::MAX@)]
#[requires(16 + payload@.len() + ss@ <= usize::MAX@)]
#[requires(payload@.len() == 4294967296 && 16 + payload@.len() <= region_size@)]
#[ensures(result.0 == Ok(()))]
#[ensures(match result.1 { Ok(v) => v@.len() == 0 && v@ != payload@, Err(e) => e == EmError::CorruptMetadata })]
pub fn support_old_refute_em_ckpt_post_seq_matches_header(area: &mut Vec<u8>, ss: usize, region_size: u64, seq: u64, payload: &Vec<u8>)
    -> (Result<(), EmError>, Result<Vec<u8>, EmError>) {
    let w = write_ckpt_bytes(area, ss, region_size, seq, payload);
    match w {
        Ok(()) => {
            let l = as_u32(payload.len() as u64);
            proof_assert! { is_trunc32(l, payload@.len()) && l@ == 0 };
            let n = snapshot! { alignup(16 + payload@.len(), ss@) };
            let b = snapshot! { area@.subsequence(0, *n) };
            proof_assert! { blob_ok(*b, seq, l, payload@, ss@) };
            proof_assert! { lemma_divmod(16 + payload@.len() + ss@ - 1, ss@); lemma_alignup_le(16 + payload@.len(), ss@, region_size@); 16 + payload@.len() <= *n && *n <= area@.len() };
            proof_assert! { forall<j: Int> 0 <= j && j < 12 ==> area@[j] == b[j] };
            proof_assert! { lemma_dec32(area@.subsequence(8, 12), l); dec32(area@.subsequence(8, 12))@ == 0 };
            let r = read_ckpt_bytes(area, ss, region_size, seq);
            proof_assert! { match r { Ok(v) => v@ == area@.subsequence(16, 16) && v@.len() == 0, Err(_) => true } };
            (w, r)
        }
        Err(e) => (w, Err(e)),
    }
}

/// Mutant: claims the 4 GiB copy reads back as written.
#[requires(ss@ >= 16 && region_size@ <= area@.len() && region_size@ % ss@ == 0 && area@.len() + ss@ <= usize::MAX@)]
#[requires(16 + payload@.len() + ss@ <= usize::MAX@)]
#[requires(payload@.len() == 4294967296 && 16 + payload@.len() <= region_size@)]
#[ensures(match result.1 { Ok(v) => v@ == payload@, Err(_) => false })]
pub fn support_old_refute_em_ckpt_post_seq_matches_header__mutant(area: &mut Vec<u8>, ss: usize, region_size: u64, seq: u64, payload: &Vec<u8>)
    -> (Result<(), EmError>, Result<Vec<u8>, EmError>) {
    let w = write_ckpt_bytes(area, ss, region_size, seq, payload);
    let r = read_ckpt_bytes(area, ss, region_size, seq);
    (w, r)
}

/// **EM-CKPT-READ-CRC** — "Reading a checkpoint copy whose header or payload bytes were altered
/// after writing fails with a CorruptMetadata error because the checksum no longer matches."
/// read_checkpoint_region (checkpoint.rs:117-173) for EVERY content of the copy region (any
/// alteration of any written copy) and every expected seq: it accepts EXACTLY when the header
/// seq matches, 16 + the header length fits the region, and the stored CRC field equals
/// crc32fast of the first 16 + length bytes with a zeroed CRC field (rd_accepts) — so a copy
/// whose checksum no longer matches is rejected, and every rejection is CorruptMetadata
/// (never IoError, never a panic); an accepted copy returns exactly bytes 16..16+length.
/// (The unaltered copy is accepted: `witness_em_ckpt_post_seq_matches_header_small`.)
#[requires(ss@ > 0 && region_size@ <= area@.len() && region_size@ % ss@ == 0 && area@.len() + ss@ <= usize::MAX@)]
#[requires(region_size@ > 0 || ss@ <= area@.len())]
#[ensures(dec32(area@.subsequence(12, 16)) != crc32(chk(area@, 16 + dec32(area@.subsequence(8, 12))@)) ==> result == Err(EmError::CorruptMetadata))]
#[ensures(match result {
    Ok(v) => rd_accepts(area@, region_size@, expected) && v@ == area@.subsequence(16, 16 + dec32(area@.subsequence(8, 12))@),
    Err(e) => e == EmError::CorruptMetadata,
})]
pub fn verify_em_ckpt_read_crc(area: &Vec<u8>, ss: usize, region_size: u64, expected: u64) -> Result<Vec<u8>, EmError> {
    read_ckpt_bytes(area, ss, region_size, expected)
}

/// Mutant: claims every copy whose header seq matches is accepted (the CRC is ignored).
#[requires(ss@ > 0 && region_size@ <= area@.len() && region_size@ % ss@ == 0 && area@.len() + ss@ <= usize::MAX@)]
#[requires(region_size@ > 0 || ss@ <= area@.len())]
#[ensures(dec64(area@.subsequence(0, 8)) == expected && 16 + dec32(area@.subsequence(8, 12))@ <= region_size@ ==> result != Err(EmError::CorruptMetadata))]
pub fn verify_em_ckpt_read_crc__mutant(area: &Vec<u8>, ss: usize, region_size: u64, expected: u64) -> Result<Vec<u8>, EmError> {
    read_ckpt_bytes(area, ss, region_size, expected)
}

// =============================================================================
// format() racing a checkpoint in flight (new root cause FORMAT-CKPT-RACE)
// =============================================================================

pub const WS_K1: u64 = 51;

/// format #1 (sector 4, slab 4, one region [0,4)), reserve_extent(K1, 4), publish K1; the
/// background checkpoint thread runs checkpoint(): write_checkpoint (checkpoint.rs:35-115,
/// under regions.read()) writes copy 1 = seq 1 and advances the in-memory superblock; the
/// caller's format #2 (instance id 2) writes ITS superblock (lib.rs:496-498, no lock shared
/// with run_checkpoint); the checkpoint thread then writes the in-memory superblock it holds
/// — format #1's, seq 1 / copy 1 (lib.rs:302-307: format has not reached lib.rs:507 yet); format
/// #2 publishes its fresh regions and returns Ok (lib.rs:506-511). A restart's recovery reads
/// format #1's superblock and copy 1: K1, an extent of the EARLIER format.
#[ensures(result.0 == Ok(()))]
#[ensures(result.1.instance_id@ == 2 && result.1.checkpoint_seq@ == 0)]
#[ensures(match result.2 {
    Ok((sb, img)) => sb.instance_id@ == 1 && img_lists(*img, Extent { key: WS_K1, size: 4u32, offset: 0u64 }),
    Err(_) => false,
})]
pub fn wst_race() -> (Result<(), EmError>, Superblock, Result<(Superblock, Snapshot<Seq<RegionState>>), EmError>) {
    let mut r = wc4_one();
    proof_assert! { rg_l3(r, hs_w1()) }; // LEVEL-3: proved region invariants at the start (format + reserve K1)
    r.publish_slot(0, 0, WS_K1);
    proof_assert! { slot_off(r.slabs@.lookup(0), 0) == 0 && r.slabs@.lookup(0).keys@[0] == WS_K1 && WS_K1 != FREE_KEY };
    proof_assert! { from_slot(r, 0, 0, Extent { key: WS_K1, size: 4u32, offset: 0u64 }) };
    let r0 = snapshot! { r };
    // format #1's superblock (lib.rs:483-494: Superblock::new, checkpoint_seq 0, active copy 0)
    let sb1 = l3_sb_c4(); // = Superblock::new(4, 4, 4, 4, 1, 4096, 4096, 1, 1, 0)
    let mut rs: Vec<RegionState> = Vec::new();
    rs.push(r);
    let mut em = FEm { regions: rs, sb: sb1, dev: FDev { sb: Some(sb1), copy0: None, copy1: None } };
    proof_assert! { em.regions@ == Seq::singleton(*r0) };
    // LEVEL-3 reachable start (format #1 + reserve K1 + publish K1): format-start ranges on
    // format #1's superblock (and the identical geometry of format #2), reserve ranges, proved
    // region invariants (rg_l3 asserted on the start region right after wc4_one; after
    // publish K1 the region invariant rg_inv / pending_ok / nonfull_listed still holds).
    proof_assert! { l3_fmt_ranges(wc_fp_l(4u64, 4u64), 3, 4096, sb1) && l3_rsv_ranges(4, wc_fp_l(4u64, 4u64)) };
    proof_assert! { rg_inv(em.regions@[0]) && pending_ok(em.regions@[0]) && nonfull_listed(em.regions@[0]) };
    // background checkpoint: write_checkpoint
    em.write_checkpoint();
    proof_assert! { em.sb.checkpoint_seq@ == 1 && em.sb.active_copy@ == 1 && em.sb.instance_id@ == 1 };
    proof_assert! { match em.dev.copy1 { Some(c) => c.seq@ == 1 && *c.img == Seq::singleton(*r0), None => false } };
    // format #2 (caller): its superblock write
    let sb2 = Superblock::new(4, 4, 4, 4, 1, 4096, 4096, 2, 1, 0);
    em.format_write_sb(sb2);
    // background checkpoint: run_checkpoint's superblock write, still format #1's
    em.write_superblock();
    // format #2: publish + Ok
    let fresh = RegionState::new(BuddyAllocator::new(0, 4, 4), wc_fp(4, 4));
    let mut nrs: Vec<RegionState> = Vec::new();
    nrs.push(fresh);
    let f = em.format_publish(nrs, sb2);
    proof_assert! { match em.dev.sb { Some(x) => x.instance_id@ == 1 && x.checkpoint_seq@ == 1 && x.active_copy@ == 1, None => false } };
    let rec = recover_f(&em.dev);
    proof_assert! { match rec { Ok((x, img)) => *img == Seq::singleton(*r0) && x.instance_id@ == 1, Err(_) => false } };
    proof_assert! { match rec { Ok((_, img)) => from_region(img[0], Extent { key: WS_K1, size: 4u32, offset: 0u64 }), Err(_) => false } };
    (f, em.sb, rec)
}

/// **EM-INIT-NO-STALE-FORMAT** — REFUTED (INDEPENDENT; new root cause FORMAT-CKPT-RACE: format
/// writes its superblock (lib.rs:496-498) and publishes the new state (lib.rs:506-507) without
/// excluding a checkpoint in flight — it takes neither `checkpoint_coalesce` nor a lock held
/// across run_checkpoint's superblock write (lib.rs:302-307) — so that write can land after
/// format's and re-install the earlier format's superblock). "initialize() never returns
/// extents from a checkpoint that was written under an earlier format of the same metadata
/// device." Witness `wst_race`: format #2 returns Ok, but the device superblock is format
/// #1's (instance id 1, seq 1) and a fresh initialize() recovers format #1's copy with K1. The
/// window lasts until the new component's first non-skipped checkpoint (a component that
/// never becomes dirty never closes it). Reachable by one caller: the 30 s background
/// thread (lib.rs:106-165) is the other party. Without a concurrent checkpoint the old copies
/// are never accepted (every copy recovery accepts carries a seq the NEW format wrote).
#[ensures(result.0 == Ok(()))]
#[ensures(match result.2 {
    Ok((sb, img)) => sb.instance_id != result.1.instance_id && img_lists(*img, Extent { key: WS_K1, size: 4u32, offset: 0u64 }),
    Err(_) => false,
})]
pub fn refute_em_init_no_stale_format() -> (Result<(), EmError>, Superblock, Result<(Superblock, Snapshot<Seq<RegionState>>), EmError>) {
    wst_race()
}

/// Mutant: claims the restart recovers the NEW format's superblock.
#[ensures(match result.2 { Ok((sb, _)) => sb.instance_id == result.1.instance_id, Err(_) => true })]
pub fn refute_em_init_no_stale_format__mutant() -> (Result<(), EmError>, Superblock, Result<(Superblock, Snapshot<Seq<RegionState>>), EmError>) {
    wst_race()
}
