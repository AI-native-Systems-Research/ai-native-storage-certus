//! Batch-8 property proof units (`verify_<id>` / `refute_<id>`, each with a failing `__mutant`
//! twin), unscored witnesses (`witness_<id>_*`) and helper chains (`w8_*`). Model: model/b8.rs.
use crate::model::b3::*;
use crate::model::b4::*;
use crate::model::b5::*;
use crate::model::b7::*;
use crate::model::b8::*;
use crate::model::bitmap::*;
use crate::model::buddy::*;
use crate::model::component::*;
use crate::model::listing::*;
use crate::model::params::*;
use crate::model::region::*;
use crate::model::sbbytes::*;
use crate::model::slab::*;
use crate::props::batch2::*;
use crate::props::batch3::*;
use crate::props::batch4::*;
use crate::props::witness::*;
use creusot_std::logic::FMap;
use crate::model::assume::*;
use crate::model::l2::{fdisj, fv, p2};
use crate::model::l2m::{coal, lemma_coal_single};
use crate::model::l2r::{pv, rg_core, rg_good, rg_acct, rg_ka, sc_ok, sl_away};
use crate::props::l2a::lg;
use crate::props::l3start::*;
use creusot_std::prelude::*;

// =============================================================================
// Metadata block I/O: flush, read errors (block_io.rs)
// =============================================================================

/// **EM-BIO-FLUSH** — "With the volatile write cache feature, a flush reports success only when
/// the device answers the flush request with a successful flush completion; any other answer is
/// an IoError." flush8 = block_io.rs:168-180 over every device outcome.
#[ensures(match result { Ok(()) => send_ok && recv == Some(CompF::FlushDone(true)), Err(e) => e == EmError::IoError })]
#[ensures(send_ok && recv == Some(CompF::FlushDone(true)) ==> result == Ok(()))]
pub fn verify_em_bio_flush(send_ok: bool, recv: Option<CompF>) -> Result<(), EmError> {
    flush8(send_ok, recv)
}

/// Mutant: claims a FlushDone completion carrying an error is a success.
#[ensures(send_ok && recv == Some(CompF::FlushDone(false)) ==> result == Ok(()))]
pub fn verify_em_bio_flush__mutant(send_ok: bool, recv: Option<CompF>) -> Result<(), EmError> {
    flush8(send_ok, recv)
}

/// **EM-BIO-READ-ERR** — "If the device reports an error, returns an unexpected completion, or
/// its channel closes during a metadata read, the read fails with an IoError." read_blocks mirror
/// (block_io.rs:118-152): a failed send (closed command channel), a failed receive (closed
/// completion channel), an `Error` completion, an unexpected completion kind or a `ReadDone`
/// carrying an error is the LAST event of the call and the call returns `IoError`; every failure
/// of the call is `IoError`.
#[requires(c.sector_size@ > 0)]
#[requires(num_bytes@ + c.sector_size@ <= usize::MAX@)]
#[requires(c.base_lba@ + lba@ + (num_bytes@ + c.sector_size@ - 1) / c.sector_size@ <= u64::MAX@)]
#[ensures(forall<p: Int> wd.log.len() <= p && p < (^wd).log.len() && ev_dev_fail((*(^wd).log)[p], false) ==>
    p == (^wd).log.len() - 1 && result == Err(EmError::IoError))]
#[ensures(forall<e: EmError> result == Err(e) ==> e == EmError::IoError)]
pub fn verify_em_bio_read_err(c: &EvClient, wd: &mut EvDev, lba: u64, num_bytes: usize) -> Result<(), EmError> {
    c.read_blocks(wd, lba, num_bytes)
}

/// Mutant: claims a device failure during a read leaves the read successful.
#[requires(c.sector_size@ > 0)]
#[requires(num_bytes@ + c.sector_size@ <= usize::MAX@)]
#[requires(c.base_lba@ + lba@ + (num_bytes@ + c.sector_size@ - 1) / c.sector_size@ <= u64::MAX@)]
#[ensures(forall<p: Int> wd.log.len() <= p && p < (^wd).log.len() && ev_dev_fail((*(^wd).log)[p], false) ==>
    result == Ok(()))]
pub fn verify_em_bio_read_err__mutant(c: &EvClient, wd: &mut EvDev, lba: u64, num_bytes: usize) -> Result<(), EmError> {
    c.read_blocks(wd, lba, num_bytes)
}

// =============================================================================
// Slab rover, slot bitmap, superblock encoding
// =============================================================================

/// **EM-SLAB-ROVER-IN-RANGE** — REFUTED as worded; WORDING ARTIFACT (the audit's suggested form
/// is proved below). "The position from which a slab searches for its next free slot is always
/// less than the slab's number of slots." A slab with ZERO slots has rover 0 == 0 slots
/// (slab.rs:17-27). Reachable: format with sector 1, slab 2^32 (FR-002-valid) and a data area of
/// 2^32 bytes; reserve_extent(k, 1): element size 1 <= slab size (region.rs:68 passes), the buddy
/// allocator hands out [0, 2^32) (proved below: `BuddyAllocator::new(0, 2^32, 1)` as
/// lib.rs:465 builds it, then `alloc(2^32)` = Some(0), region.rs:72-75), and region.rs:77 builds
/// `Slab::new(0, 2^32, 1)` whose slot count `(2^32 / 1) as u32` truncates to 0 (slab.rs:18,
/// SLAB-SLOTS-U32): rover 0 is not < 0. alloc_slot then finds nothing and the reservation is
/// rolled back (region.rs:78-83) — the slab never yields a slot and is dropped.
#[ensures(result.0 == Some(0u64))]
#[ensures(result.1.bitmap.num_slots@ == 0 && result.1.rover@ == 0)]
#[ensures(!(result.1.rover@ < result.1.bitmap.num_slots@))]
#[ensures(result.1.element_size@ == 1 && result.1.slab_size@ == 4294967296 && result.1.start_offset@ == 0)]
pub fn support_old_refute_em_slab_rover_in_range() -> (Option<u64>, Slab) {
    let mut b = BuddyAllocator::new(0, 4294967296, 1);
    proof_assert! { 32.pow2() == 4294967296 && 31.pow2() == 2147483648 };
    proof_assert! { 4294967296 / 1 == 4294967296 };
    proof_assert! { b.max_order@ == 32 && b.free_lists@[32]@ == Seq::singleton(0u64) };
    proof_assert! { is_order(32, 4294967296) };
    proof_assert! { lemma_ord_eq(4294967296, 0, 32); ord(4294967296) == 32 };
    proof_assert! { (4294967296 + 1 - 1) / 1 == 4294967296 };
    proof_assert! { first_ne(b.free_lists@, 32, 32) == 32 };
    let d = b.alloc(4294967296); // region.rs:72-75
    proof_assert! { d != None };
    proof_assert! { match d { Some(x) => x@ == 0, None => false } };
    let s = Slab::new(0, 4294967296, 1); // region.rs:77 with disk_offset 0
    proof_assert! { slots_of(4294967296, 1) == 0 };
    (d, s)
}

/// Mutant: claims the rover of that slab is a valid slot index.
#[ensures(result.1.rover@ < result.1.bitmap.num_slots@)]
pub fn support_old_refute_em_slab_rover_in_range__mutant() -> (Option<u64>, Slab) {
    let mut b = BuddyAllocator::new(0, 4294967296, 1);
    proof_assert! { 32.pow2() == 4294967296 && 31.pow2() == 2147483648 };
    proof_assert! { 4294967296 / 1 == 4294967296 };
    proof_assert! { b.max_order@ == 32 && b.free_lists@[32]@ == Seq::singleton(0u64) };
    proof_assert! { is_order(32, 4294967296) };
    proof_assert! { lemma_ord_eq(4294967296, 0, 32); ord(4294967296) == 32 };
    proof_assert! { (4294967296 + 1 - 1) / 1 == 4294967296 };
    proof_assert! { first_ne(b.free_lists@, 32, 32) == 32 };
    let d = b.alloc(4294967296);
    let s = Slab::new(0, 4294967296, 1);
    (d, s)
}

/// EM-SLAB-ROVER-IN-RANGE, the audit's suggested form (unscored): for every slab (every slab
/// satisfies slab_inv: Slab::new establishes it, alloc_slot / free_slot / set_key preserve it),
/// when it has at least one slot the rover is a valid slot index before and after alloc_slot,
/// and a slab with no slots never yields a slot.
#[requires(slab_inv(*s))]
#[ensures(s.bitmap.num_slots@ >= 1 ==> s.rover@ < s.bitmap.num_slots@ && (^s).rover@ < (^s).bitmap.num_slots@)]
#[ensures(s.bitmap.num_slots@ == 0 ==> result == None)]
pub fn witness_em_slab_rover_in_range_intended(s: &mut Slab) -> Option<(usize, u64)> {
    s.alloc_slot()
}

/// **EM-BITMAP-FIND-FREE** — "Searching for a free slot returns a slot that is free and within
/// range whenever at least one free slot exists, and returns nothing only when every slot is
/// allocated." find_free_from (bitmap.rs:42-51) from any start position its only caller passes
/// (the rover, slab.rs:30: a valid index, or 0 for a zero-slot bitmap).
#[requires(bm_wf(*bm))]
#[requires(bm.num_slots@ == 0 || start@ < bm.num_slots@)]
#[ensures(match result { Some(i) => i@ < bm.num_slots@ && !slot_bit(*bm, i@), None => true })]
#[ensures((exists<j: Int> 0 <= j && j < bm.num_slots@ && !slot_bit(*bm, j)) ==> result != None)]
#[ensures(result == None ==> forall<j: Int> 0 <= j && j < bm.num_slots@ ==> slot_bit(*bm, j))]
pub fn verify_em_bitmap_find_free(bm: &AllocationBitmap, start: usize) -> Option<usize> {
    bm.find_free_from(start)
}

/// Mutant: claims a search always finds a slot.
#[requires(bm_wf(*bm))]
#[requires(bm.num_slots@ == 0 || start@ < bm.num_slots@)]
#[ensures(result != None)]
pub fn verify_em_bitmap_find_free__mutant(bm: &AllocationBitmap, start: usize) -> Option<usize> {
    bm.find_free_from(start)
}

/// **EM-SB-CORRUPTION-DETECTED** — "Decoding a superblock whose checksummed bytes were altered,
/// whose magic number is wrong, or that is shorter than 4096 bytes fails with a CorruptMetadata
/// error." deserialize7 (superblock.rs:99-168, exact), for every byte buffer.
#[ensures(buf@.len() < 4096 ==> result == Err(EmError::CorruptMetadata))]
#[ensures(buf@.len() >= 4096 && d64(buf@, 0) != SUPERBLOCK_MAGIC ==> result == Err(EmError::CorruptMetadata))]
#[ensures(buf@.len() >= 4096 && d32(buf@, 92) != crc32(buf@.subsequence(0, 92)) ==> result == Err(EmError::CorruptMetadata))]
#[ensures(forall<o: Seq<u8>> sb_ok7(o) && buf@.len() >= 4096 && d32(buf@, 92) == d32(o, 92)
    && crc32(buf@.subsequence(0, 92)) != crc32(o.subsequence(0, 92)) ==> result == Err(EmError::CorruptMetadata))]
#[ensures(match result { Ok(_) => sb_ok7(buf@), Err(e) => e == EmError::CorruptMetadata })]
pub fn verify_em_sb_corruption_detected(buf: &Vec<u8>) -> Result<Superblock, EmError> {
    deserialize7(buf)
}

/// Mutant: claims a buffer with a wrong magic number decodes.
#[ensures(buf@.len() >= 4096 && d64(buf@, 0) != SUPERBLOCK_MAGIC ==> result != Err(EmError::CorruptMetadata))]
pub fn verify_em_sb_corruption_detected__mutant(buf: &Vec<u8>) -> Result<Superblock, EmError> {
    deserialize7(buf)
}

/// **EM-SB-NEW-DEFAULTS** — "A newly created superblock carries the expected magic number and
/// current format version, a checkpoint sequence of zero and copy 0 as the active copy."
/// Superblock::new (superblock.rs:28-56) for every argument.
#[ensures(result.magic == SUPERBLOCK_MAGIC)]
#[ensures(result.version == FORMAT_VERSION && result.version@ == 6)]
#[ensures(result.checkpoint_seq@ == 0 && result.active_copy@ == 0)]
pub fn verify_em_sb_new_defaults(
    dds: u64, ss: u32, slab: u64, mes: u32, rc: u32, cro: u64, crs: u64, iid: u64, ns: u32, dso: u64,
) -> Superblock {
    Superblock::new(dds, ss, slab, mes, rc, cro, crs, iid, ns, dso)
}

/// Mutant: claims a new superblock names copy 1 as active.
#[ensures(result.active_copy@ == 1)]
pub fn verify_em_sb_new_defaults__mutant(
    dds: u64, ss: u32, slab: u64, mes: u32, rc: u32, cro: u64, crs: u64, iid: u64, ns: u32, dso: u64,
) -> Superblock {
    Superblock::new(dds, ss, slab, mes, rc, cro, crs, iid, ns, dso)
}

// =============================================================================
// remove_extent
// =============================================================================

/// **EM-REMOVE-ERR-NOT-INITIALIZED** — "When remove_extent() is called before either format()
/// or initialize() has succeeded, it returns a NotInitialized error." remove_extent8: while the
/// regions are not published (a fresh component, every failed format / initialize — their error
/// frames keep them None) — and also in format's window between publishing the regions and the
/// shared state (lib.rs:506-507: lib.rs:233-236) — every offset gets NotInitialized and nothing
/// changes.
#[requires(em_regions_wf(*em) && em_regions_ok(*em))]
#[requires(em.regions == None || em.shared == None)]
#[ensures(result == Err(EmError::NotInitialized) && unch8(*em, ^em))]
pub fn verify_em_remove_err_not_initialized(em: &mut ExtentManager, offset: u64) -> Result<(), EmError> {
    em.remove_extent8(offset)
}

/// Mutant: claims the removal succeeds.
#[requires(em_regions_wf(*em) && em_regions_ok(*em))]
#[requires(em.regions == None || em.shared == None)]
#[ensures(result == Ok(()))]
pub fn verify_em_remove_err_not_initialized__mutant(em: &mut ExtentManager, offset: u64) -> Result<(), EmError> {
    em.remove_extent8(offset)
}

/// LEVEL-3 (phase D-B2): rm_safe8 from the PROVED invariants em_regions_wf + em_layout + em_rg_built:
/// the first current region lies in [data_start_offset, ...) and ends within the data disk size.
#[logic]
#[requires(em_regions_wf(em) && em_layout(em) && em_rg_built(em))]
#[ensures(rm_safe8(em))]
pub fn lemma_rm_safe8(em: ExtentManager) {
    pearlite! {
        match (em.regions, em.shared) {
            (Some(rv), Some(sh)) => {
                proof_assert! { rv@.len() > 0 };
                proof_assert! { rg_within(em.arena@[rv@[0]@], sh.superblock.data_start_offset@, sh.superblock.data_disk_size@) };
                proof_assert! { rg_built(em.arena@[rv@[0]@].buddy.base_offset@, em.arena@[rv@[0]@].buddy.total_usable_size@, sh.format_params) };
                ()
            }
            _ => (),
        }
    }
}

/// **EM-REMOVE-NO-PANIC** — "remove_extent() never crashes for any offset value; every offset
/// that does not identify a published extent yields an error." remove_extent8 (lib.rs:223-255,
/// 680-684 and region.rs:121-150): for EVERY u64 offset every arithmetic operation, cast,
/// index and unwrap carries a proved VC (u64 subtraction lib.rs:243 under rm_safe8; division by
/// the region count > 0 and by region_bytes != 0; the saturating subtraction; the slab lookups),
/// and a success removes a published extent starting exactly at that offset.
// LEVEL-3 (phase D-B2): rm_safe8 is no longer a premise — it is DERIVED (lemma_rm_safe8) from three
// PROVED invariants: em_regions_wf (a non-empty region list), em_layout (every region starts at or
// after the data start offset) and em_rg_built (every region ends within the format data disk
// size) — so initialize states are covered too (em_layout / em_rg_built have initialize bases).
#[requires(em_regions_wf(*em) && em_regions_ok(*em) && em_layout(*em) && em_rg_built(*em))]
#[ensures(match result { Ok(()) => pub_at8(*em, offset@), Err(e) => e == EmError::NotInitialized || e == EmError::OffsetNotFound(offset) })]
#[ensures(!pub_at8(*em, offset@) ==> result != Ok(()) && unch8(*em, ^em))]
pub fn verify_em_remove_no_panic(em: &mut ExtentManager, offset: u64) -> Result<(), EmError> {
    proof_assert! { lemma_rm_safe8(*em); rm_safe8(*em) };
    em.remove_extent8(offset)
}

/// Mutant: claims every removal succeeds.
// LEVEL-3 (phase D-B2): rm_safe8 is no longer a premise — it is DERIVED (lemma_rm_safe8) from three
// PROVED invariants: em_regions_wf (a non-empty region list), em_layout (every region starts at or
// after the data start offset) and em_rg_built (every region ends within the format data disk
// size) — so initialize states are covered too (em_layout / em_rg_built have initialize bases).
#[requires(em_regions_wf(*em) && em_regions_ok(*em) && em_layout(*em) && em_rg_built(*em))]
#[ensures(result == Ok(()))]
pub fn verify_em_remove_no_panic__mutant(em: &mut ExtentManager, offset: u64) -> Result<(), EmError> {
    proof_assert! { lemma_rm_safe8(*em); rm_safe8(*em) };
    em.remove_extent8(offset)
}

/// EM-REMOVE-NO-PANIC support (unscored): a successful format establishes rm_safe8 (and the
/// region invariants) — every state reached from it by reserve / publish / abort / remove /
/// checkpoint keeps `shared`'s geometry (their frames), so rm_safe8 stays true.
#[requires(em_regions_wf(*em))]
#[requires(em.dev.connected ==> a_67ea4f(em.dev.num_sectors@, em.dev.sector_size@))]
#[requires(a_2ee144(params.metadata_alignment@))]
#[requires(sane_params(params))]
#[ensures(result == Ok(()) ==> rm_safe8(^em) && em_regions_wf(^em) && em_regions_ok(^em))]
pub fn witness_em_remove_no_panic_after_format(em: &mut ExtentManager, params: FormatParams) -> Result<(), EmError> {
    em.format(params)
}

/// The REGION-BYTES-ZERO geometry: metadata device 4 x 4096 bytes; FR-002-valid parameters with
/// sector 4, slab 4, max extent 4, EIGHT regions over a 4-byte data device (separate-device
/// mode): usable 4 > 0 passes lib.rs:447-452, region_bytes = 4 / 8 = 0, regions 0..6 are empty
/// and region 7 is [0, 4) (lib.rs:455-464).
#[ensures(result.0.connected && result.0.sector_size@ == 4096 && result.0.num_sectors@ == 4)]
#[ensures(result.1.data_disk_size@ == 4 && result.1.slab_size@ == 4 && result.1.max_extent_size@ == 4)]
#[ensures(result.1.sector_size@ == 4 && result.1.region_count@ == 8 && result.1.metadata_alignment@ == 0)]
#[ensures(result.1.metadata_region_size@ == 0 && result.1.instance_id == Some(1u64))]
pub fn w8_rb0_geometry() -> (MetaDevice, FormatParams) {
    let dev = MetaDevice { connected: true, sector_size: 4096, num_sectors: 4, sb: None, ckpt0: None, ckpt1: None };
    let params = FormatParams {
        data_disk_size: 4,
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

/// format() on the REGION-BYTES-ZERO geometry. On success: 8 regions, no slab anywhere, region 7
/// is a fresh [0, 4), rm_safe8 holds and region_bytes is 0.
#[ensures(match result.0 {
    Ok(()) => em_regions_wf(result.1) && em_regions_ok(result.1) && rm_safe8(result.1) && rb_zero8(result.1)
        && match (result.1.regions, result.1.shared) {
            (Some(rv), Some(sh)) => rv@.len() == 8 && rv@[7]@ < result.1.arena@.len()
                && (forall<i: Int> 0 <= i && i < 8 ==> result.1.arena@[rv@[i]@].slabs@ == FMap::empty())
                && fresh_rg(result.1.arena@[rv@[7]@], 0, 4, result.2)
                && sh.superblock.data_start_offset@ == 0 && sh.format_params.data_disk_size@ == 4,
            _ => false,
        },
    Err(e) => e == EmError::IoError,
})]
#[ensures(result.2.sector_size@ == 4 && result.2.slab_size@ == 4 && result.2.region_count@ == 8)]
pub fn w8_rb0_format() -> (Result<(), EmError>, ExtentManager, FormatParams) {
    let (dev, params) = w8_rb0_geometry();
    let mut em = new_inner(dev);
    proof_assert! { em_regions_wf(em) };
    proof_assert! { fmt_ok8(em, params) };
    let r = em.format(params);
    if r.is_err() {
        return (r, em, params);
    }
    proof_assert! { ds_l(16384, 0, 0, 4) == 0 };
    proof_assert! { 4 / 8 == 0 && rbase(0, 4, 8, 7) == 0 && rsize(4, 8, 7) == 4 };
    proof_assert! { match em.regions { Some(rv) => rv@.len() == 8 && forall<i: Int> 0 <= i && i < 8 ==>
        fresh_rg(em.arena@[rv@[i]@], rbase(0, 4, 8, i), rsize(4, 8, i), params), None => false } };
    (r, em, params)
}

/// **EM-REMOVE-ERR-OFFSET-NOT-FOUND** — REFUTED. INDEPENDENT candidate bug (NOT BUDDY-MASK:
/// sector 4), NEW ROOT CAUSE = REGION-BYTES-ZERO: format accepts any usable data size > 0
/// (lib.rs:447-452) and sizes regions `usable / region_count` (lib.rs:455), which is 0 when the
/// usable bytes are fewer than the region count (the last region gets everything, lib.rs:460-464);
/// region_for_offset then answers every offset with NotInitialized("region size is zero")
/// (lib.rs:244-247). "When remove_extent(offset) is called with an offset at which no allocated
/// extent exists ..., it returns an OffsetNotFound error carrying that offset." Witness: format
/// (8 regions, 4-byte data device, sector 4) succeeds; nothing is reserved; remove_extent(0)
/// returns NotInitialized.
#[ensures(match result.0 {
    Ok(()) => !slot_at8(result.1, 0) && result.2 == Err(EmError::NotInitialized)
        && result.2 != Err(EmError::OffsetNotFound(0u64)),
    Err(e) => e == EmError::IoError,
})]
pub fn support_old_refute_em_remove_err_offset_not_found() -> (Result<(), EmError>, ExtentManager, Result<(), EmError>) {
    let (r, mut em, _p) = w8_rb0_format();
    if r.is_err() {
        return (r, em, Ok(()));
    }
    let pre = snapshot! { em };
    let rm = em.remove_extent8(0);
    proof_assert! { rm == Err(EmError::NotInitialized) && unch8(*pre, em) };
    (r, em, rm)
}

/// Mutant: claims the removal reports OffsetNotFound(0).
#[ensures(match result.0 { Ok(()) => result.2 == Err(EmError::OffsetNotFound(0u64)), Err(_) => true })]
pub fn support_old_refute_em_remove_err_offset_not_found__mutant() -> (Result<(), EmError>, ExtentManager, Result<(), EmError>) {
    let (r, mut em, _p) = w8_rb0_format();
    if r.is_err() {
        return (r, em, Ok(()));
    }
    let rm = em.remove_extent8(0);
    (r, em, rm)
}

/// EM-REMOVE-ERR-OFFSET-NOT-FOUND, positive half (unscored): whenever region_bytes > 0, an
/// offset with no published extent gets OffsetNotFound(offset) and nothing changes.
#[requires(em_regions_wf(*em) && em_regions_ok(*em) && rm_safe8(*em) && rb_pos8(*em))]
#[requires(!pub_at8(*em, offset@))]
#[ensures(result == Err(EmError::OffsetNotFound(offset)) && unch8(*em, ^em))]
pub fn witness_em_remove_err_offset_not_found_rb_pos(em: &mut ExtentManager, offset: u64) -> Result<(), EmError> {
    em.remove_extent8(offset)
}

/// REGION-BYTES-ZERO geometry + reserve_extent(7, 4): key 7 -> region 7 (7 & 7, lib.rs:219),
/// a fresh one-slot slab [0, 4) (region.rs:66-91). Then remove_extent(2).
#[ensures(match result.0 {
    Ok(()) => match result.1.regions {
        Some(rv) => rv@.len() == 8 && rv@[7]@ < result.1.arena@.len()
            && result.1.arena@[rv@[7]@].slabs@.contains(0)
            && result.1.arena@[rv@[7]@].slabs@.lookup(0).start_offset@ == 0
            && result.1.arena@[rv@[7]@].slabs@.lookup(0).slab_size@ == 4
            && result.1.arena@[rv@[7]@].slabs@.lookup(0).element_size@ == 4
            && result.1.arena@[rv@[7]@].slabs@.lookup(0).bitmap.num_slots@ == 1
            && !slot_at8(result.1, 2)
            && result.2 == Err(EmError::NotInitialized),
        None => false,
    },
    Err(e) => e == EmError::IoError,
})]
pub fn w8_rb0_interior() -> (Result<(), EmError>, ExtentManager, Result<(), EmError>) {
    let (r, mut em, params) = w8_rb0_format();
    if r.is_err() {
        return (r, em, Ok(()));
    }
    let n = match &em.regions {
        Some(rv) => rv.len(),
        None => return (r, em, Ok(())),
    };
    fact_and_7();
    let idx = and_mask(7usize, n - 1);
    proof_assert! { idx@ == 7 };
    let r7 = match &em.regions {
        Some(rv) => rv[idx],
        None => return (r, em, Ok(())),
    };
    proof_assert! { match em.regions { Some(rv) => rv@[7] == r7 && r7@ < em.arena@.len(), None => false } };
    let fr = snapshot! { em.arena@[r7@] };
    proof_assert! { fresh_rg(*fr, 0, 4, params) && rg_inv(*fr) };
    proof_assert! { 0.pow2() == 1 && 4 / 4 == 1 };
    proof_assert! { fr.buddy.max_order@ == 0 && fr.buddy.free_lists@[0]@ == Seq::singleton(0u64) };
    proof_assert! { lemma_ord_eq(1, 0, 0); slab_k(*fr) == 0 && span(4, 0) == 4 };
    proof_assert! { align_l(4, 4) == 4 && slots_of(4, 4) == 1 };
    proof_assert! { first_ne(fr.buddy.free_lists@, 0, 0) == 0 };
    proof_assert! { fr.buddy.base_offset@ == 0 };
    let arena0 = snapshot! { em.arena };
    let a1 = em.arena[r7].alloc_extent(4);
    proof_assert! { fresh_case(*fr, em.arena@[r7@], 4, a1) };
    proof_assert! { match a1 { Ok((d, _, _)) => d@ == 0, Err(_) => false } };
    proof_assert! { em.arena@[r7@].slabs@ == FMap::empty().insert(0, em.arena@[r7@].slabs@.lookup(0)) };
    proof_assert! { forall<j: Int> 0 <= j && j < em.arena@.len() && j != r7@ ==> em.arena@[j] == arena0[j] };
    proof_assert! { em_regions_wf(em) && em_regions_ok(em) && rm_safe8(em) && rb_zero8(em) };
    proof_assert! { match em.regions { Some(rv) => forall<i: Int> 0 <= i && i < 7 ==> em.arena@[rv@[i]@].slabs@ == FMap::empty(), None => false } };
    proof_assert! { match em.regions { Some(rv) => forall<i: Int> 0 <= i && i < 7 ==> !slot_in8(em.arena@[rv@[i]@], 2), None => false } };
    proof_assert! { forall<s: Int> em.arena@[r7@].slabs@.contains(s) ==> s == 0 };
    proof_assert! { slot_off(em.arena@[r7@].slabs@.lookup(0), 0) == 0 };
    proof_assert! { !slot_in8(em.arena@[r7@], 2) };
    proof_assert! { !slot_at8(em, 2) };
    let rm = em.remove_extent8(2);
    (r, em, rm)
}

/// **EM-REMOVE-ERR-INTERIOR-OFFSET** — REFUTED. INDEPENDENT candidate bug (NOT BUDDY-MASK:
/// sector 4), ROOT CAUSE = REGION-BYTES-ZERO (this batch; lib.rs:244-247 vs lib.rs:447-455).
/// "remove_extent() with an offset that falls inside a slab but not at the start of a slot fails
/// with an OffsetNotFound error and changes nothing." Witness w8_rb0_interior: format (8 regions,
/// 4-byte data device, sector 4); reserve_extent(7, 4) carves the one-slot slab [0, 4) in region
/// 7; offset 2 lies inside it and no slot of any slab starts at 2; remove_extent(2) returns
/// NotInitialized, not OffsetNotFound. (Nothing changes — that half holds.)
#[ensures(match result.0 {
    Ok(()) => !slot_at8(result.1, 2) && result.2 == Err(EmError::NotInitialized)
        && result.2 != Err(EmError::OffsetNotFound(2u64)),
    Err(e) => e == EmError::IoError,
})]
pub fn support_old_refute_em_remove_err_interior_offset() -> (Result<(), EmError>, ExtentManager, Result<(), EmError>) {
    w8_rb0_interior()
}

/// Mutant: claims the removal reports OffsetNotFound(2).
#[ensures(match result.0 { Ok(()) => result.2 == Err(EmError::OffsetNotFound(2u64)), Err(_) => true })]
pub fn support_old_refute_em_remove_err_interior_offset__mutant() -> (Result<(), EmError>, ExtentManager, Result<(), EmError>) {
    w8_rb0_interior()
}

/// EM-REMOVE-ERR-INTERIOR-OFFSET, positive half (unscored): whenever region_bytes > 0, an offset
/// at which no slot of any current slab starts gets OffsetNotFound(offset) and nothing changes.
#[requires(em_regions_wf(*em) && em_regions_ok(*em) && rm_safe8(*em) && rb_pos8(*em))]
#[requires(!slot_at8(*em, offset@))]
#[ensures(result == Err(EmError::OffsetNotFound(offset)) && unch8(*em, ^em))]
pub fn witness_em_remove_err_interior_offset_rb_pos(em: &mut ExtentManager, offset: u64) -> Result<(), EmError> {
    em.remove_extent8(offset)
}

/// **EM-REMOVE-LAST-REGION-TAIL** — REFUTED. INDEPENDENT (NOT BUDDY-MASK: sector 4), ROOT CAUSE =
/// REGION-TAIL-LOOKUP (batch 3; this is the code reader's own suspicion): region_for_offset
/// (lib.rs:243-253) divides by the uniform region size and rejects index >= region_count, which is
/// where the last region's remainder (lib.rs:460-464) lands. "remove_extent() finds and removes
/// any published extent, including one placed in the extra tail space that makes the last region
/// larger than the others." Witness wt_tail_remove (batch 3): 8 regions over 36 bytes; K2 is
/// published at offset 32 in region 7 = [28, 36) (its tail); remove_extent(32) returns
/// OffsetNotFound(32) and the slot still holds K2.
#[ensures(match result.0 {
    Ok(()) => result.1@ == 32 && result.2 == Err(EmError::OffsetNotFound(32u64))
        && match result.3.regions {
            Some(rv) => rv@.len() == 8 && result.3.arena@[rv@[7]@].slabs@.contains(32)
                && from_slot(result.3.arena@[rv@[7]@], 32, 0, Extent { key: WT_K2, size: 4u32, offset: 32u64 }),
            None => false,
        },
    Err(e) => e == EmError::IoError,
})]
pub fn refute_em_remove_last_region_tail() -> (Result<(), EmError>, u64, Result<(), EmError>, ExtentManager) {
    wt_tail_remove()
}

/// Mutant: claims the tail extent is removed.
#[ensures(match result.0 { Ok(()) => result.2 == Ok(()), Err(_) => true })]
pub fn refute_em_remove_last_region_tail__mutant() -> (Result<(), EmError>, u64, Result<(), EmError>, ExtentManager) {
    wt_tail_remove()
}

// =============================================================================
// reserve_extent
// =============================================================================

/// **EM-RESERVE-ERR-NOT-INITIALIZED** — "When reserve_extent() is called before either format()
/// or initialize() has succeeded, it returns a NotInitialized error." reserve_extent8: while the
/// regions are not published (fresh component, every failed format / initialize), region_for_key
/// (lib.rs:211-218) fails first — for EVERY key and size (0 and u32::MAX included) — and nothing
/// changes.
#[requires(em_regions_wf(*em) && em_regions_ok(*em))]
#[requires(em.regions == None)]
#[ensures(match result { Err(e) => e == EmError::NotInitialized, Ok(_) => false })]
#[ensures(unch8(*em, ^em))]
pub fn verify_em_reserve_err_not_initialized(em: &mut ExtentManager, key: u64, size: u32) -> Result<Handle, EmError> {
    em.reserve_extent8(key, size)
}

/// Mutant: claims the reservation succeeds.
#[requires(em_regions_wf(*em) && em_regions_ok(*em))]
#[requires(em.regions == None)]
#[ensures(match result { Ok(_) => true, Err(_) => false })]
pub fn verify_em_reserve_err_not_initialized__mutant(em: &mut ExtentManager, key: u64, size: u32) -> Result<Handle, EmError> {
    em.reserve_extent8(key, size)
}

/// **EM-RESERVE-POST-HANDLE-FIELDS** — "When reserve_extent(key, size) succeeds, the returned
/// write handle's key() is exactly the key passed in, and its extent_size() is the requested size
/// rounded up to the next multiple of the sector size, so it is at least the requested size and
/// less than the requested size plus one sector." reserve_extent8 (lib.rs:584-630; the size is
/// lib.rs:591) for every initialised component state, key and size > 0 below u32::MAX - sector.
#[requires(em_regions_wf(*em) && em_regions_ok(*em))]
#[requires(em.regions != None ==> size@ > 0)]
// LEVEL-3 (phase D-B2): 3783f8 on the CURRENT regions only (rsv_cur_ok; stale arena entries are
// never reserved from).
#[requires(em.regions != None ==> rsv_cur_ok(*em, size@))]
#[ensures(match result {
    Ok(h) => h.key == key
        && h.size@ == (size@ + em.arena@[h.region@].format_params.sector_size@ - 1) / em.arena@[h.region@].format_params.sector_size@
            * em.arena@[h.region@].format_params.sector_size@
        && size@ <= h.size@ && h.size@ < size@ + em.arena@[h.region@].format_params.sector_size@,
    Err(_) => true,
})]
pub fn verify_em_reserve_post_handle_fields(em: &mut ExtentManager, key: u64, size: u32) -> Result<Handle, EmError> {
    em.reserve_extent8(key, size)
}

/// Mutant: claims the handle's size is below the requested size.
#[requires(em_regions_wf(*em) && em_regions_ok(*em))]
#[requires(em.regions != None ==> size@ > 0)]
// LEVEL-3 (phase D-B2): 3783f8 on the CURRENT regions only (rsv_cur_ok; stale arena entries are
// never reserved from).
#[requires(em.regions != None ==> rsv_cur_ok(*em, size@))]
#[ensures(match result { Ok(h) => h.size@ < size@, Err(_) => true })]
pub fn verify_em_reserve_post_handle_fields__mutant(em: &mut ExtentManager, key: u64, size: u32) -> Result<Handle, EmError> {
    em.reserve_extent8(key, size)
}

/// format() with one region: sector 4, max extent `mes`, slab `slab` = data size (separate-device
/// mode, metadata device 4 x 4096 bytes). On success region 0 is a fresh [0, slab).
#[requires(slab@ == 4 || slab@ == 8)]
#[requires(mes@ <= slab@)]
#[ensures(result.2.sector_size@ == 4 && result.2.slab_size == slab && result.2.max_extent_size == mes)]
#[ensures(result.2.data_disk_size == slab && result.2.region_count@ == 1)]
#[ensures(match result.0 {
    Ok(()) => em_regions_wf(result.1) && em_regions_ok(result.1)
        && match (result.1.regions, result.1.shared) {
            (Some(rv), Some(sh)) => rv@.len() == 1 && rv@[0]@ < result.1.arena@.len()
                && fresh_rg(result.1.arena@[rv@[0]@], 0, slab@, result.2) && rg_inv(result.1.arena@[rv@[0]@])
                && sh.superblock.data_start_offset@ == 0,
            _ => false,
        },
    Err(e) => e == EmError::IoError,
})]
#[ensures(match result.0 { Ok(()) => seq_agree(result.1) && em_layout(result.1), Err(_) => true })]
pub fn w8_one(slab: u64, mes: u32) -> (Result<(), EmError>, ExtentManager, FormatParams) {
    let dev = MetaDevice { connected: true, sector_size: 4096, num_sectors: 4, sb: None, ckpt0: None, ckpt1: None };
    let params = FormatParams {
        data_disk_size: slab,
        slab_size: slab,
        max_extent_size: mes,
        sector_size: 4,
        region_count: 1,
        metadata_alignment: 0,
        instance_id: Some(1),
        metadata_disk_ns_id: 1,
        metadata_region_size: 0,
    };
    let mut em = new_inner(dev);
    proof_assert! { em_regions_wf(em) };
    proof_assert! { fmt_ok_inputs(em, params) };
    let r = em.format(params);
    if r.is_err() {
        return (r, em, params);
    }
    proof_assert! { ds_l(16384, 0, 0, 4) == 0 };
    proof_assert! { rbase(0, slab@, 1, 0) == 0 && rsize(slab@, 1, 0) == slab@ };
    (r, em, params)
}

/// One region [0, 8), slab 8, max extent 4: reserve_extent(key, 8) — region_for_key (one region)
/// and alloc_extent(8) (region.rs:41-91: element size 8 <= slab size 8, fresh slab [0, 8)), then
/// the handle size lib.rs:591.
#[ensures(match result.0 {
    Ok(()) => result.2 == Ok((0u64, 0usize, 0u64)) && result.3@ == 8 && result.1.max_extent_size@ == 4,
    Err(e) => e == EmError::IoError,
})]
pub fn w8_oversize() -> (Result<(), EmError>, FormatParams, Result<(u64, usize, u64), EmError>, u32) {
    let (r, mut em, params) = w8_one(8, 4);
    if r.is_err() {
        return (r, params, Err(EmError::OutOfSpace), 0);
    }
    let r0 = match &em.regions {
        Some(rv) => rv[0],
        None => return (r, params, Err(EmError::OutOfSpace), 0),
    };
    proof_assert! { match em.regions { Some(rv) => rv@[0] == r0 && r0@ < em.arena@.len(), None => false } };
    let fr = snapshot! { em.arena@[r0@] };
    proof_assert! { fresh_rg(*fr, 0, 8, params) && rg_inv(*fr) };
    proof_assert! { 1.pow2() == 2 && 8 / 4 == 2 };
    proof_assert! { fr.buddy.max_order@ == 1 && fr.buddy.free_lists@[1]@ == Seq::singleton(0u64) && fr.buddy.free_lists@[0]@.len() == 0 };
    proof_assert! { is_order(1, 2) };
    proof_assert! { lemma_ord_eq(2, 0, 1); slab_k(*fr) == 1 };
    proof_assert! { align_l(8, 4) == 8 && slots_of(8, 8) == 1 };
    proof_assert! { first_ne(fr.buddy.free_lists@, 1, 1) == 1 };
    proof_assert! { fr.buddy.base_offset@ == 0 };
    let a = em.arena[r0].alloc_extent(8);
    proof_assert! { fresh_case(*fr, em.arena@[r0@], 8, a) };
    proof_assert! { match a { Ok((d, i, o)) => d@ == 0 && i@ == 0 && o@ == 0, Err(_) => false } };
    let bs = em.arena[r0].format_params.sector_size;
    proof_assert! { bs@ == 4 };
    let aligned = (8u32 + bs - 1) / bs * bs; // lib.rs:591
    (r, params, a, aligned)
}

/// **EM-RESERVE-ERR-OVERSIZE** — REFUTED. INDEPENDENT candidate bug (NOT BUDDY-MASK: sector 4),
/// NEW ROOT CAUSE = RESERVE-MAX-EXTENT-UNCHECKED (the code reader's suspicion): alloc_extent
/// rejects only `element_size > slab_size` (region.rs:66-70); nothing compares the request with
/// format_params.max_extent_size (stored by format, lib.rs:392-396 checks only max <= slab).
/// "reserve_extent() rejects a size larger than the configured maximum extent size with an error
/// and reserves nothing." Witness: format (one region, sector 4, slab 8, max extent 4, 8-byte data
/// device; FR-002-valid) succeeds; reserve_extent(k, 8) — twice the maximum — succeeds with the
/// fresh slab [0, 8), slot 0, and a handle of extent_size 8.
#[ensures(match result.0 {
    Ok(()) => (match result.2 { Ok(_) => true, Err(_) => false }) && result.3@ == 8 && result.3@ > result.1.max_extent_size@,
    Err(e) => e == EmError::IoError,
})]
pub fn refute_em_reserve_err_oversize() -> (Result<(), EmError>, FormatParams, Result<(u64, usize, u64), EmError>, u32) {
    w8_oversize()
}

/// Mutant: claims the oversize reservation fails.
#[ensures(match result.0 { Ok(()) => match result.2 { Ok(_) => false, Err(_) => true }, Err(_) => true })]
pub fn refute_em_reserve_err_oversize__mutant() -> (Result<(), EmError>, FormatParams, Result<(u64, usize, u64), EmError>, u32) {
    w8_oversize()
}

/// One region [0, 4), slab 4: the first steps of alloc_extent(size) (region.rs:41-75) when the
/// ELEMENT size computed at region.rs:42 is `es`: the size class `es` is empty (fresh region),
/// `es <= slab_size` passes region.rs:68, and the buddy allocator hands out the slab block [0, 4).
/// The next statement, region.rs:77, is `Slab::new(0, 4, es)`.
#[requires(es@ <= 4)]
#[ensures(match result.0 {
    Ok(()) => result.1 == None && result.2 == Some(0u64) && result.3@ == 4,
    Err(e) => e == EmError::IoError,
})]
pub fn w8_carve(es: u32) -> (Result<(), EmError>, Option<u64>, Option<u64>, u64) {
    let (r, mut em, params) = w8_one(4, 4);
    if r.is_err() {
        return (r, None, None, 0);
    }
    let r0 = match &em.regions {
        Some(rv) => rv[0],
        None => return (r, None, None, 0),
    };
    proof_assert! { match em.regions { Some(rv) => rv@[0] == r0 && r0@ < em.arena@.len(), None => false } };
    let fr = snapshot! { em.arena@[r0@] };
    proof_assert! { fresh_rg(*fr, 0, 4, params) && rg_inv(*fr) };
    proof_assert! { 0.pow2() == 1 && 4 / 4 == 1 };
    proof_assert! { fr.buddy.max_order@ == 0 && fr.buddy.free_lists@[0]@ == Seq::singleton(0u64) };
    proof_assert! { is_order(0, 1) };
    proof_assert! { lemma_ord_eq(1, 0, 0); ord((4 + 4 - 1) / 4) == 0 };
    proof_assert! { first_ne(fr.buddy.free_lists@, 0, 0) == 0 };
    let first = em.arena[r0].size_classes.get_slabs_first(es); // region.rs:48
    let slab_size = em.arena[r0].format_params.slab_size; // region.rs:66
    proof_assert! { slab_size@ == 4 && es@ <= slab_size@ }; // region.rs:68 passes
    let d = em.arena[r0].buddy.alloc(slab_size); // region.rs:72-75
    proof_assert! { d != None };
    proof_assert! { match d { Some(x) => x@ == fr.buddy.base_offset@ + 0, None => false } };
    (r, first, d, slab_size)
}

/// **EM-RESERVE-ERR-ZERO-SIZE** — REFUTED. INDEPENDENT candidate bug (NOT BUDDY-MASK: sector 4),
/// NEW ROOT CAUSE = RESERVE-ZERO-DIV (the code reader's suspicion): region.rs:42 rounds size 0 to
/// element size `(0 + ss - 1) / ss * ss` = 0, nothing rejects it (region.rs:68 only rejects
/// element_size > slab_size; reserve_extent lib.rs:584-589 has no check), and region.rs:77 calls
/// Slab::new(.., 0) whose `slab_size / element_size` (slab.rs:18) divides by zero — a panic in
/// debug AND release builds (while the region still has room for a slab; otherwise OutOfSpace).
/// "reserve_extent() with a size of zero fails with an error, reserves nothing and does not
/// crash." Witness: format (one region [0, 4), sector 4, slab 4) succeeds; reserve_extent(k, 0):
/// element size 0, size class 0 empty, 0 <= 4, buddy alloc(4) = Some(0) — the next statement is
/// Slab::new(0, 4, 0), whose precondition (element_size > 0) fails.
#[ensures(result.0@ == 0)]
#[ensures(match result.1 { Ok(()) => result.2 == None && result.3 == Some(0u64) && result.4@ == 4, Err(e) => e == EmError::IoError })]
pub fn refute_em_reserve_err_zero_size() -> (u32, Result<(), EmError>, Option<u64>, Option<u64>, u64) {
    let es = align_l8(0, 4); // region.rs:42
    let (r, first, d, slab) = w8_carve(es);
    (es, r, first, d, slab)
}

/// Mutant: claims the element size is positive.
#[ensures(result.0@ > 0)]
pub fn refute_em_reserve_err_zero_size__mutant() -> (u32, Result<(), EmError>, Option<u64>, Option<u64>, u64) {
    let es = align_l8(0, 4);
    let (r, first, d, slab) = w8_carve(es);
    (es, r, first, d, slab)
}

/// `align_to_sector_size(size, ss)` (region.rs:37-39) in range (debug == release): the u32
/// `size + ss` itself does not overflow (refutation helper, used with size 0; not a premise of
/// any credited proof).
#[requires(ss@ > 0 && size@ + ss@ <= u32::MAX@)]
#[ensures(result@ == align_l(size@, ss@))]
pub fn align_l8(size: u32, ss: u32) -> u32 {
    proof_assert! { lemma_divmod(size@ + ss@ - 1, ss@); (size@ + ss@ - 1) / ss@ * ss@ <= size@ + ss@ - 1 };
    (size + ss - 1) / ss * ss
}

/// **EM-RESERVE-NO-SIZE-OVERFLOW** — REFUTED. INDEPENDENT candidate bug (NOT BUDDY-MASK: sector 4),
/// NEW ROOT CAUSE = RESERVE-ALIGN-OVERFLOW: region.rs:38 computes `size + sector_size - 1` in u32
/// with no bound on size (lib.rs:591 repeats it). "reserve_extent() with a size so large that
/// rounding it up to a whole number of sectors would overflow fails with an error and does not
/// crash or reserve a smaller extent." size = u32::MAX, sector 4: DEBUG build — 4294967295 + 3
/// overflows u32 at region.rs:38: panic. RELEASE build — the sum wraps to 2, element size
/// 2 / 4 * 4 = 0 (release_align8), and the reservation continues exactly as for size 0 (w8_carve:
/// format one region [0, 4); size class 0 empty, 0 <= slab 4, buddy alloc(4) = Some(0)) into
/// Slab::new(0, 4, 0): division by zero (slab.rs:18), a panic. Either build crashes.
#[ensures(result.0 && result.1@ == 0)]
#[ensures(match result.2 { Ok(()) => result.3 == None && result.4 == Some(0u64), Err(e) => e == EmError::IoError })]
pub fn refute_em_reserve_no_size_overflow() -> (bool, u32, Result<(), EmError>, Option<u64>, Option<u64>) {
    let size: u32 = 4294967295;
    let ss: u32 = 4;
    let overflows = (size as u64) + (ss as u64) - 1 > 4294967295; // region.rs:38 debug overflow check
    let es = release_align8(size, ss); // region.rs:38 release (wrapping)
    proof_assert! { (4294967295 + 4 - 1 - 4294967296) / 4 * 4 == 0 };
    let (r, first, d, _slab) = w8_carve(es);
    (overflows, es, r, first, d)
}

/// Mutant: claims the rounding does not overflow.
#[ensures(!result.0)]
pub fn refute_em_reserve_no_size_overflow__mutant() -> (bool, u32, Result<(), EmError>, Option<u64>, Option<u64>) {
    let size: u32 = 4294967295;
    let ss: u32 = 4;
    let overflows = (size as u64) + (ss as u64) - 1 > 4294967295;
    let es = release_align8(size, ss);
    proof_assert! { (4294967295 + 4 - 1 - 4294967296) / 4 * 4 == 0 };
    let (r, first, d, _slab) = w8_carve(es);
    (overflows, es, r, first, d)
}

/// `1 & 7 == 1`: region_for_key (lib.rs:219) sends key 1 to region 1 of 8.
#[bitwise_proof]
#[ensures((1usize & 7usize) == 1usize)]
pub fn fact_and_1_8() {}

/// REGION-SECTOR-REMAINDER geometry (batch 4 wu_geometry: 8 regions x 6 bytes, sector 4, slab 4):
/// format, then reserve_extent(1, 4): key 1 -> region 1 = [6, 12) (lib.rs:219), alloc_extent(4)
/// carves the fresh slab at the region's base 6 (buddy offsets are base + multiples of the sector,
/// buddy.rs:28-37), slot 0: offset 6.
#[ensures(match result.0 {
    Ok(()) => result.1 == Ok((6u64, 0usize, 6u64)),
    Err(e) => e == EmError::IoError,
})]
pub fn w8_offset6() -> (Result<(), EmError>, Result<(u64, usize, u64), EmError>) {
    let (dev, params) = wu_geometry();
    let mut em = new_inner(dev);
    proof_assert! { em_regions_wf(em) };
    proof_assert! { fmt_ok8(em, params) };
    let r = em.format(params);
    if r.is_err() {
        return (r, Err(EmError::OutOfSpace));
    }
    proof_assert! { ds_l(16384, 0, 0, 4) == 0 };
    proof_assert! { rbase(0, 48, 8, 1) == 6 && rsize(48, 8, 1) == 6 };
    let n = match &em.regions {
        Some(rv) => rv.len(),
        None => return (r, Err(EmError::OutOfSpace)),
    };
    fact_and_1_8();
    let idx = and_mask(1usize, n - 1);
    proof_assert! { idx@ == 1 };
    let r1 = match &em.regions {
        Some(rv) => rv[idx],
        None => return (r, Err(EmError::OutOfSpace)),
    };
    proof_assert! { match em.regions { Some(rv) => rv@[1] == r1 && r1@ < em.arena@.len(), None => false } };
    let fr = snapshot! { em.arena@[r1@] };
    proof_assert! { fresh_rg(*fr, 6, 6, params) && rg_inv(*fr) };
    proof_assert! { 0.pow2() == 1 && 6 / 4 == 1 };
    proof_assert! { fr.buddy.max_order@ == 0 && fr.buddy.free_lists@[0]@ == Seq::singleton(0u64) };
    proof_assert! { lemma_ord_eq(1, 0, 0); slab_k(*fr) == 0 && span(4, 0) == 4 };
    proof_assert! { align_l(4, 4) == 4 && slots_of(4, 4) == 1 };
    proof_assert! { first_ne(fr.buddy.free_lists@, 0, 0) == 0 };
    proof_assert! { fr.buddy.base_offset@ == 6 };
    let a = em.arena[r1].alloc_extent(4);
    proof_assert! { fresh_case(*fr, em.arena@[r1@], 4, a) };
    proof_assert! { match a { Ok((d, i, o)) => d@ == 6 && i@ == 0 && o@ == 6, Err(_) => false } };
    (r, a)
}

/// **EM-RESERVE-POST-OFFSET-SECTOR-ALIGNED** — REFUTED. INDEPENDENT (NOT BUDDY-MASK: sector 4),
/// ROOT CAUSE = REGION-SECTOR-REMAINDER (batch 4): format sizes regions `usable / region_count`
/// bytes (lib.rs:455-464) with no rounding to whole sectors, so region i starts at data_start +
/// i * region_bytes, which need not be a sector multiple; the buddy allocator's offsets are the
/// region base plus sector multiples (buddy.rs:28-37), so every extent of such a region is
/// misaligned. "When reserve_extent() succeeds, the returned handle's extent_offset() is a disk
/// byte offset that is an exact multiple of the sector size." Witness (batch 4 wu_geometry,
/// FR-002-valid, the data device is 12 WHOLE sectors): 8 regions of 6 bytes; reserve_extent(1, 4)
/// returns the handle offset 6 (lib.rs:589, 626), and 6 mod 4 = 2. (A second route, not checked:
/// shared-device mode with a sector size that does not divide checkpoint_region_offset, e.g.
/// sector 3 and offset 4096 — data_start_offset lib.rs:443 is then not a sector multiple.)
#[ensures(match result.0 {
    Ok(()) => match result.1 { Ok((_, _, off)) => off@ == 6 && off@ % 4 == 2, Err(_) => false },
    Err(e) => e == EmError::IoError,
})]
pub fn support_old_refute_em_reserve_post_offset_sector_aligned() -> (Result<(), EmError>, Result<(u64, usize, u64), EmError>) {
    w8_offset6()
}

/// Mutant: claims the reserved offset is a multiple of the sector size.
#[ensures(match result.0 { Ok(()) => match result.1 { Ok((_, _, off)) => off@ % 4 == 0, Err(_) => true }, Err(_) => true })]
pub fn support_old_refute_em_reserve_post_offset_sector_aligned__mutant() -> (Result<(), EmError>, Result<(u64, usize, u64), EmError>) {
    w8_offset6()
}

/// W3 (witness.rs, sector 3) after w3_stale's history, with the facts this batch needs: the slab
/// map is exactly {0 -> Z0 (element 12, full), 12 -> Z (element 3)}, size class 6 still lists 12,
/// and the buddy allocator has no free block of the slab order or above.
#[ensures(rg_inv(result) && result.format_params.sector_size@ == 3)]
#[ensures(forall<k: Int> result.slabs@.contains(k) ==> k == 0 || k == 12)]
#[ensures(result.slabs@.contains(0) && result.slabs@.lookup(0).element_size@ == 12)]
#[ensures(result.slabs@.contains(12) && result.slabs@.lookup(12).element_size@ == 3 && result.slabs@.lookup(12).start_offset@ == 12)]
#[ensures(result.slabs@.lookup(12).bitmap.num_slots@ == 4 && result.slabs@.lookup(12).bitmap.allocated_count@ == 1)]
#[ensures(slot_bit(result.slabs@.lookup(12).bitmap, 0))]
#[ensures(forall<j: Int> 0 < j && j < 4 ==> !slot_bit(result.slabs@.lookup(12).bitmap, j))]
#[ensures(sc_get(result.size_classes, 6) == Seq::singleton(12u64))]
#[ensures(first_ne(result.buddy.free_lists@, slab_k(result), result.buddy.max_order@) == -1)]
pub fn w8_stale() -> RegionState {
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
    proof_assert! { forall<k: Int> r.slabs@.contains(k) ==> k == 12 };
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
    proof_assert! { r.slabs@.contains(0) && r.slabs@.lookup(0).element_size@ == 12 };
    proof_assert! { forall<k: Int> r.slabs@.contains(k) ==> k == 0 || k == 12 };
    proof_assert! { r.buddy.free_lists@[2]@ == Seq::singleton(12u64) && r.buddy.free_lists@[3]@.len() == 0 };
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
    proof_assert! { r.slabs@.lookup(0) == old2.slabs@.lookup(0) };
    proof_assert! { forall<k: Int> r.slabs@.contains(k) ==> k == 0 || k == 12 };
    proof_assert! { r.buddy.free_lists@[2]@.len() == 0 && r.buddy.free_lists@[3]@.len() == 0 };
    proof_assert! { slab_k(r) == 2 && r.buddy.max_order@ == 3 };
    proof_assert! { first_ne(r.buddy.free_lists@, 2, 3) == -1 };
    r
}

/// **EM-RESERVE-ERR-OUT-OF-SPACE** — REFUTED. ROOT CAUSE = BUDDY-MASK (non-power-of-two sector
/// size). "When the request cannot be satisfied because the key's region has no free slot of the
/// required size and no room for a new slab ..., reserve_extent() returns an OutOfSpace error and
/// makes no reservation." W3 (sector 3, one region [0, 24)): after the recovery whose buddy.rs:136
/// mask missed, the recovered element-6 slab Y at 12 was replaced by the fresh element-3 slab Z
/// (region.rs:89) while size class 6 still lists 12 (lib.rs:563). Now NO slab has element size 6
/// and NO buddy block of the slab order is free — yet reserve_extent(_, 6) succeeds: alloc_extent
/// trusts the stale entry (region.rs:48-59 never checks the slab's element size) and reserves
/// slot 1..3 of Z, a 3-byte slot, reporting size 6. (The post-state is returned; alloc_extent's
/// exist_case keeps the buddy state and every slab's element size, so both hypotheses held before
/// the call too.)
#[ensures(forall<k: Int> result.0.slabs@.contains(k) ==> result.0.slabs@.lookup(k).element_size@ != 6)]
#[ensures(first_ne(result.0.buddy.free_lists@, slab_k(result.0), result.0.buddy.max_order@) == -1)]
#[ensures(match result.1 {
    Ok((s, i, _)) => s@ == 12 && 1 <= i@ && i@ < 4 && result.0.slabs@.lookup(12).element_size@ == 3
        && slot_bit(result.0.slabs@.lookup(12).bitmap, i@),
    Err(_) => false,
})]
pub fn refute_em_reserve_err_out_of_space() -> (RegionState, Result<(u64, usize, u64), EmError>) {
    let mut r = w8_stale();
    proof_assert! { align_l(6, 3) == 6 };
    let old = snapshot! { r };
    proof_assert! { exist_cond(*old, 6, 12u64) };
    let res = r.alloc_extent(6);
    proof_assert! { exist_case(*old, r, 6, 12u64, res) };
    proof_assert! { r.buddy == old.buddy };
    proof_assert! { forall<k: Int> r.slabs@.contains(k) ==> k == 0 || k == 12 };
    proof_assert! { r.slabs@.lookup(0) == old.slabs@.lookup(0) };
    (r, res)
}

/// Mutant: claims the reservation fails with OutOfSpace.
#[ensures(match result.1 { Ok(_) => false, Err(e) => e == EmError::OutOfSpace })]
pub fn refute_em_reserve_err_out_of_space__mutant() -> (RegionState, Result<(u64, usize, u64), EmError>) {
    let mut r = w8_stale();
    proof_assert! { align_l(6, 3) == 6 };
    let old = snapshot! { r };
    proof_assert! { exist_cond(*old, 6, 12u64) };
    let res = r.alloc_extent(6);
    proof_assert! { exist_case(*old, r, 6, 12u64, res) };
    (r, res)
}

/// pow2 witness of EM-RESERVE-ERR-OUT-OF-SPACE (unscored): W3p (sector 4, region [0, 32); batch 2
/// w3p_recovered — mark_allocated hits, Y = element 8 at 16 is taken off the free lists). After
/// reserve(K3, 16) carves [0, 16), no slab has element size 4 and no free block of the slab order
/// remains; reserve(_, 4) returns OutOfSpace and the region is unchanged.
#[ensures(forall<k: Int> result.0.slabs@.contains(k) ==> result.0.slabs@.lookup(k).element_size@ != 4)]
#[ensures(first_ne(result.0.buddy.free_lists@, slab_k(result.0), result.0.buddy.max_order@) == -1)]
#[ensures(result.1 == Err(EmError::OutOfSpace))]
pub fn witness_em_reserve_err_out_of_space_pow2() -> (RegionState, Result<(u64, usize, u64), EmError>) {
    let mut r = w3p_recovered();
    proof_assert! { slab_k(r) == 2 && align_l(16, 4) == 16 && align_l(4, 4) == 4 && slots_of(16, 16) == 1 && slots_of(16, 4) == 4 };
    proof_assert! { first_ne(r.buddy.free_lists@, 2, 3) == 2 };
    let o1 = snapshot! { r };
    let a = r.alloc_extent(16);
    proof_assert! { fresh_case(*o1, r, 16, a) };
    proof_assert! { match a { Ok((d, _, _)) => d@ == 0, Err(_) => false } };
    proof_assert! { r.slabs@.contains(16) && r.slabs@.lookup(16) == o1.slabs@.lookup(16) };
    proof_assert! { r.slabs@.contains(0) && r.slabs@.lookup(0).element_size@ == 16 };
    proof_assert! { forall<k: Int> r.slabs@.contains(k) ==> k == 0 || k == 16 };
    proof_assert! { r.buddy.free_lists@[2]@.len() == 0 && r.buddy.free_lists@[3]@.len() == 0 };
    proof_assert! { first_ne(r.buddy.free_lists@, 2, 3) == -1 };
    proof_assert! { sc_get(r.size_classes, 4) == Seq::empty() };
    let o2 = snapshot! { r };
    let b = r.alloc_extent(4);
    proof_assert! { fresh_case(*o2, r, 4, b) && r == *o2 };
    (r, b)
}

// =============================================================================
// set_checkpoint_interval / background checkpoint thread
// =============================================================================

/// One region [0, 4) after format; reserve_extent(1, 4) + publish (alloc_extent, then the
/// publish closure's publish_slot, lib.rs:610 — the region is now dirty); then
/// `metadata_device.disconnect()`; then the background thread's work step: `this.checkpoint()`
/// reaches run_checkpoint (no checkpoint in progress, lib.rs:747-750), which fails at
/// get_metadata_client with NotInitialized (lib.rs:297, 186-189); bg_log8 (lib.rs:154-157).
#[ensures(match result.0 {
    Ok(()) => result.1.regions != None && result.1.shared != None && !result.1.dev.connected
        && any_dirty8(result.1)
        && result.2 == Err(EmError::NotInitialized) && !result.3,
    Err(e) => e == EmError::IoError,
})]
pub fn w8_bg_disconnected() -> (Result<(), EmError>, ExtentManager, Result<(), EmError>, bool) {
    let (r, mut em, params) = w8_one(4, 4);
    if r.is_err() {
        return (r, em, Ok(()), false);
    }
    let r0 = match &em.regions {
        Some(rv) => rv[0],
        None => return (r, em, Ok(()), false),
    };
    proof_assert! { match em.regions { Some(rv) => rv@[0] == r0 && r0@ < em.arena@.len(), None => false } };
    let fr = snapshot! { em.arena@[r0@] };
    proof_assert! { fresh_rg(*fr, 0, 4, params) && rg_inv(*fr) };
    proof_assert! { 0.pow2() == 1 && 4 / 4 == 1 };
    proof_assert! { fr.buddy.max_order@ == 0 && fr.buddy.free_lists@[0]@ == Seq::singleton(0u64) };
    // LEVEL-3 reachable start (format of one region [0, 4)): the proved component invariants and
    // the proved region invariants of the region. The property's only method,
    // set_checkpoint_interval, is listed by no declared range.
    proof_assert! { em_regions_wf(em) && em_regions_ok(em) && em_layout(em) && seq_agree(em) && rm_safe8(em) };
    proof_assert! { 2.pow2() == 4 && p2(4) };
    proof_assert! { forall<o: Int, k: Int> fv(fr.buddy.free_lists@, o, k) ==> o == 0 && k == 0 };
    proof_assert! { rg_tf(*fr) == 4 && fr.slabs@.len() == 0 };
    proof_assert! { fdisj(fr.buddy.free_lists@, fr.buddy.sector_size@) };
    proof_assert! { sl_away(*fr) };
    proof_assert! { rg_acct(*fr) };
    proof_assert! { sc_ok(*fr) };
    proof_assert! { rg_ka(*fr) };
    proof_assert! { rg_core(*fr) && rg_good(*fr) && pv(*fr, Seq::empty()) && lg(*fr, Seq::empty()) };
    proof_assert! { lemma_coal_single(fr.buddy.free_lists@, fr.buddy.sector_size@); coal(fr.buddy.free_lists@, fr.buddy.sector_size@) };
    proof_assert! { nonfull_listed(*fr) && coal(fr.buddy.free_lists@, fr.buddy.sector_size@) && rg_l3(*fr, Seq::empty()) };
    proof_assert! { lemma_ord_eq(1, 0, 0); slab_k(*fr) == 0 };
    proof_assert! { align_l(4, 4) == 4 && slots_of(4, 4) == 1 };
    proof_assert! { first_ne(fr.buddy.free_lists@, 0, 0) == 0 };
    let a = em.arena[r0].alloc_extent(4);
    proof_assert! { fresh_case(*fr, em.arena@[r0@], 4, a) };
    let (s, i) = match a {
        Ok((s, i, _)) => (s, i),
        Err(_) => return (r, em, Ok(()), false),
    };
    proof_assert! { em.arena@[r0@].slabs@.contains(s@) && slab_inv(em.arena@[r0@].slabs@.lookup(s@)) };
    em.arena[r0].publish_slot(s, i, 1);
    proof_assert! { em.arena@[r0@].dirty };
    proof_assert! { em_regions_wf(em) };
    proof_assert! { match em.regions { Some(rv) => rv@[0] == r0, None => false } };
    proof_assert! { any_dirty8(em) };
    em.disconnect8();
    let front = em.run_ck_front8();
    let ck = match front {
        Ok(_) => Ok(()),
        Err(e) => Err(e),
    };
    let logged = bg_log8(ck);
    (r, em, ck, logged)
}

/// **EM-SETINT-BG-NOT-INIT-SILENT** — REFUTED. INDEPENDENT candidate bug (NOT BUDDY-MASK: no
/// recovery involved), NEW ROOT CAUSE = NOTINIT-OVERLOADED: get_metadata_client reports an
/// unbound metadata_device receptacle as NotInitialized("metadata block device not connected")
/// (lib.rs:186-189), and the background loop silences every NotInitialized (lib.rs:155) — so a
/// background checkpoint of an INITIALISED component that fails because its metadata device was
/// disconnected is never logged. "A background checkpoint that fails only because the component is
/// not yet initialized logs no error, whereas any other background checkpoint failure is logged as
/// an error." Witness: format (one region) succeeds; reserve + publish (region dirty); the public
/// `metadata_device.disconnect()` (receptacle.rs:110-117); the background work step: checkpoint()
/// -> run_checkpoint -> Err(NotInitialized) -> no log_error, although regions and shared state
/// are published. (Positive half proved: witness_em_setint_bg_not_init_silent_kinds.)
#[ensures(match result.0 {
    Ok(()) => result.1.regions != None && result.1.shared != None && result.2 == Err(EmError::NotInitialized) && !result.3,
    Err(e) => e == EmError::IoError,
})]
pub fn refute_em_setint_bg_not_init_silent() -> (Result<(), EmError>, ExtentManager, Result<(), EmError>, bool) {
    w8_bg_disconnected()
}

/// Mutant: claims the failure is logged.
#[ensures(match result.0 { Ok(()) => result.3, Err(_) => true })]
pub fn refute_em_setint_bg_not_init_silent__mutant() -> (Result<(), EmError>, ExtentManager, Result<(), EmError>, bool) {
    w8_bg_disconnected()
}

/// EM-SETINT-BG-NOT-INIT-SILENT, positive half (unscored): on a component whose regions are not
/// published the background checkpoint fails with NotInitialized and logs nothing; any other
/// failure kind (IoError, CorruptMetadata, ...) is logged.
#[requires(em_regions_wf(*em) && em.regions == None)]
#[ensures(!result.0)]
#[ensures(result.1 == match res { Ok(()) => false, Err(EmError::NotInitialized) => false, Err(_) => true })]
pub fn witness_em_setint_bg_not_init_silent_kinds(em: &ExtentManager, res: Result<(), EmError>) -> (bool, bool) {
    let front = em.run_ck_front8();
    let ck = match front {
        Ok(_) => Ok(()),
        Err(e) => Err(e),
    };
    (bg_log8(ck), bg_log8(res))
}

/// The thread is blocked in a wait (lib.rs:134 / 138).
#[logic(open)]
pub fn blocked8(p: TPh) -> bool {
    pearlite! { match p { TPh::Wait(_) => true, TPh::WaitInf => true, _ => false } }
}

/// **EM-SETINT-CHANGE-NO-SPURIOUS** — "Changing the checkpoint interval does not by itself trigger
/// a background checkpoint; the thread starts waiting with the new interval." TSys (batch 5;
/// lib.rs:120-163 vs set_interval lib.rs:70-73), for every timer state and new interval: the
/// change runs no checkpoint and never moves the thread toward one — a thread blocked in its wait
/// is woken NOT timed out (lib.rs:134-135), re-locks, `continue`s (lib.rs:147-149) and enters a
/// new wait on the NEW interval (deadline now + d, or the untimed wait for None); a thread that is
/// not blocked is not affected (its next loop iteration reads the new interval, lib.rs:128-132).
/// A thread already timed out / working was triggered by the OLD interval's expiry before the
/// change (TIMER-TIMEOUT-STALE, batch 5) — not by the change.
#[requires(!t.shutdown)]
#[ensures((^t).ckpts == t.ckpts && (^t).interval == iv && (^t).now == t.now)]
#[ensures(blocked8(t.ph) ==> match iv {
    Some(d) => match (^t).ph { TPh::Wait(dl) => *dl == *t.now + d@, _ => false },
    None => (^t).ph == TPh::WaitInf,
})]
#[ensures(!blocked8(t.ph) ==> (^t).ph == t.ph)]
pub fn verify_em_setint_change_no_spurious(t: &mut TSys, iv: Option<u64>) {
    let was_blocked = match t.ph {
        TPh::Wait(_) => true,
        TPh::WaitInf => true,
        _ => false,
    };
    t.set_interval(iv);
    if was_blocked {
        t.relock();
        t.th_enter();
    }
}

/// Mutant: claims the change makes a blocked thread run a checkpoint.
#[requires(!t.shutdown)]
#[ensures(blocked8(t.ph) ==> (^t).ckpts@ == t.ckpts@ + 1)]
pub fn verify_em_setint_change_no_spurious__mutant(t: &mut TSys, iv: Option<u64>) {
    let was_blocked = match t.ph {
        TPh::Wait(_) => true,
        TPh::WaitInf => true,
        _ => false,
    };
    t.set_interval(iv);
    if was_blocked {
        t.relock();
        t.th_enter();
    }
}

/// **EM-SETINT-DEFAULT-30S** — "A newly created component has automatic checkpoints enabled with an
/// interval of 30 seconds." new_inner (lib.rs:106-120; tsys_new8): the interval is Some(30 s) and
/// the freshly spawned thread's first wait is the timed one with deadline 30 s (lib.rs:128-136).
#[ensures(result.interval == Some(30u64) && result.ckpts@ == 0 && !result.shutdown)]
#[ensures(match result.ph { TPh::Wait(dl) => *dl == 30, _ => false })]
pub fn verify_em_setint_default_30s() -> TSys {
    let mut t = tsys_new8();
    t.th_enter();
    t
}

/// Mutant: claims automatic checkpoints are disabled by default.
#[ensures(result.interval == None)]
pub fn verify_em_setint_default_30s__mutant() -> TSys {
    let mut t = tsys_new8();
    t.th_enter();
    t
}

// =============================================================================
// used_bytes
// =============================================================================

/// **EM-USED-NO-UNDERFLOW** — REFUTED. ROOT CAUSE = BUDDY-MASK (non-power-of-two sector size).
/// "In every region the free space tracked by the allocator never exceeds the region's usable
/// size, so computing used bytes never underflows." W1 (sector 3, one region [0, 6), batch 1-3):
/// after initialize the recovered slab B at 3 was never taken off the free lists (buddy.rs:136),
/// and remove_extent(3) + checkpoint return its block a second time (region.rs:104): tracked free
/// space 9 > usable 6, so lib.rs:704 `total_usable_size - total_free` underflows — a debug-build
/// panic, release 2^64 - 3.
#[ensures(result.0@ == 6 && result.1@ == 9 && result.1@ > result.0@)]
#[ensures(result.2@ == 18446744073709551613)]
pub fn support_old_refute_em_used_no_underflow() -> (u64, u64, u64) {
    let a = w1_freed();
    proof_assert! { lemma_tf2(a.free_lists@, 3); tf(a.free_lists@, 3, 2) == 9 };
    let usable = a.total_usable_size();
    let free = a.total_free();
    proof_assert! { bd_shape(a) };
    let used = region_used_release(&a);
    (usable, free, used)
}

/// Mutant: claims the free space stays within the usable size.
#[ensures(result.1@ <= result.0@)]
pub fn support_old_refute_em_used_no_underflow__mutant() -> (u64, u64, u64) {
    let a = w1_freed();
    proof_assert! { lemma_tf2(a.free_lists@, 3); tf(a.free_lists@, 3, 2) == 9 };
    let usable = a.total_usable_size();
    let free = a.total_free();
    proof_assert! { bd_shape(a) };
    let used = region_used_release(&a);
    (usable, free, used)
}

/// pow2 witness of EM-USED-NO-UNDERFLOW (unscored): W1p (the W1 history at sector 4, region
/// [0, 8)): free space 8 at format and 8 again after remove + checkpoint — never above the usable
/// 8, so used_bytes() = 0 does not underflow.
#[ensures(result.0@ <= result.1@ && result.2@ <= result.1@)]
pub fn witness_em_used_no_underflow_pow2() -> (u64, u64, u64) {
    witness_em_used_returns_after_free_pow2()
}

/// **EM-USED-UNINIT-ZERO** — "Before the component is formatted or initialized, used_bytes()
/// returns zero." used_bytes (lib.rs:698-708, b4.rs): `regions.as_ref().map_or(0, ..)` — for every
/// state whose regions are not published (a fresh component, every failed format / initialize).
#[requires(em_regions_wf(*em) && em.regions == None)]
#[ensures(result@ == 0)]
pub fn verify_em_used_uninit_zero(em: &ExtentManager) -> u64 {
    em.used_bytes()
}

/// Mutant: claims a non-zero value.
#[requires(em_regions_wf(*em) && em.regions == None)]
#[ensures(result@ == 1)]
pub fn verify_em_used_uninit_zero__mutant(em: &ExtentManager) -> u64 {
    em.used_bytes()
}
