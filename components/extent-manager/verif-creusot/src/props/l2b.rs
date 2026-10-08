//! Level-2 batch B — ten properties refuted before only through an input outside a level-1
//! input range (`components/extent-manager/verif-l2/.run/l2_batchB.yaml`), re-proved IN
//! GENERAL inside that range. Each scored `verify_<id>` states its own assumption (the
//! `assume_rust` of l2_batchB.yaml, in pearlite) as a `#[requires]` on the entry input; where a
//! proof also needs D-RANGE-FR-002-217ace (power-of-two sector size: the batch-A region
//! invariant `lg`), it says so. Each has a failing `__mutant` twin. A `refute_<id>` here is a
//! counterexample that SATISFIES its assumption (a defect inside the range).
use crate::model::b3::*;
use crate::model::b4::*;
use crate::model::b5::*;
use crate::model::b6::*;
use crate::model::b7::*;
use crate::model::b8::*;
use crate::model::bitmap::*;
use crate::model::buddy::*;
use crate::model::component::*;
use crate::model::l2::*;
use crate::model::l2r::*;
use crate::model::listing::*;
use crate::model::params::*;
use crate::model::region::*;
use crate::model::sbbytes::*;
use crate::model::slab::*;
use crate::props::batch4::*;
use crate::props::batch5::*;
use crate::props::batch6::*;
use crate::props::batch8::*;
use crate::props::l2a::*;
use creusot_std::logic::FMap;
use crate::model::assume::*;
use creusot_std::prelude::*;

// =============================================================================
// Arithmetic helpers
// =============================================================================

/// `k * y` is a multiple of `m` when `y` is.
#[logic]
#[requires(m > 0 && y >= 0 && k >= 0 && y % m == 0)]
#[ensures((k * y) % m == 0)]
pub fn lemma_kmul_mod(y: Int, k: Int, m: Int) {
    pearlite! {
        {
            lemma_divmod(y, m);
            lemma_assoc(k, y / m, m);
            proof_assert! { k * y == m * (k * (y / m)) };
            lemma_mul_mod0(m, k * (y / m))
        }
    }
}

// =============================================================================
// EM-CKPT-POST-SEQ-MATCHES-HEADER  (assumes D-RANGE-FMT-CHECKPOINT-REGION-HEADER-87044b)
// =============================================================================

/// **EM-CKPT-POST-SEQ-MATCHES-HEADER** — "After a checkpoint succeeds, the sequence number in
/// the header of the newly active checkpoint copy equals the checkpoint sequence number stored
/// in the superblock, and reading that copy back with that sequence number is accepted and
/// returns exactly the payload that was written." LEVEL-2 under the assumption that the
/// serialized payload is shorter than 4 GiB (`payload.len() <= u32::MAX`): for EVERY payload in
/// range, EVERY sequence number `seq` (checkpoint.rs:81 writes `new_seq` into the header, :110
/// stores the same `new_seq` in the superblock) and every copy region that fits it,
/// write_checkpoint's copy write (checkpoint.rs:69-97, model/b4.rs write_ckpt_bytes) either
/// fails CorruptMetadata (payload larger than the copy region, checkpoint.rs:69-75) or succeeds
/// with header seq == `seq`, and read_checkpoint_region (checkpoint.rs:117-173) with `seq`
/// accepts it and returns exactly the payload.
#[requires(ss@ >= 16 && region_size@ <= area@.len() && region_size@ % ss@ == 0 && area@.len() + ss@ <= usize::MAX@)]
#[requires(16 + payload@.len() + ss@ <= usize::MAX@)]
#[requires(payload@.len() <= u32::MAX@)]
#[ensures(match result.0 {
    Ok(()) => dec64((^area)@.subsequence(0, 8)) == seq
        && match result.1 { Ok(v) => v@ == payload@, Err(_) => false },
    Err(e) => e == EmError::CorruptMetadata,
})]
pub fn narrowed_verify_em_ckpt_post_seq_matches_header(area: &mut Vec<u8>, ss: usize, region_size: u64, seq: u64, payload: &Vec<u8>)
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
            proof_assert! { area@.subsequence(0, 8).ext_eq(b.subsequence(0, 8)) };
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

/// Mutant: same body and requires; claims the copy read back with its own seq is rejected.
#[requires(ss@ >= 16 && region_size@ <= area@.len() && region_size@ % ss@ == 0 && area@.len() + ss@ <= usize::MAX@)]
#[requires(16 + payload@.len() + ss@ <= usize::MAX@)]
#[requires(payload@.len() <= u32::MAX@)]
#[ensures(match result.0 {
    Ok(()) => dec64((^area)@.subsequence(0, 8)) == seq
        && match result.1 { Ok(_) => false, Err(_) => true },
    Err(e) => e == EmError::CorruptMetadata,
})]
pub fn narrowed_verify_em_ckpt_post_seq_matches_header__mutant(area: &mut Vec<u8>, ss: usize, region_size: u64, seq: u64, payload: &Vec<u8>)
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
            proof_assert! { area@.subsequence(0, 8).ext_eq(b.subsequence(0, 8)) };
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

// =============================================================================
// EM-REMOVE-ERR-OFFSET-NOT-FOUND / EM-REMOVE-ERR-INTERIOR-OFFSET
//   (assume D-RANGE-FR-002-84618e)
// =============================================================================

/// D-RANGE-FR-002-84618e on the component's recorded geometry: the usable data size
/// `data_disk_size - data_start_offset` (lib.rs:442-447, recorded in the superblock) divided by
/// the region count is > 0. `rv@.len() == region_count`: format builds exactly region_count
/// regions (lib.rs:455-468; format's contract: `rv@.len() == params.region_count@` and
/// `sh.format_params == params`), so lib.rs:244's `usable / regions.len()` is that quotient.
/// The component is initialized (`_ => false`): the not-initialized case is
/// EM-REMOVE-ERR-NOT-INITIALIZED, not these obligations.
#[logic(open)]
pub fn rb_assume(em: ExtentManager) -> bool {
    pearlite! {
        match (em.regions, em.shared) {
            (Some(rv), Some(sh)) => rv@.len() == sh.format_params.region_count@
                && a_84618e(sh.format_params.data_disk_size@, sh.superblock.data_start_offset@, sh.format_params.region_count@),
            _ => false,
        }
    }
}

/// An offset inside the byte range of some slab of some current region.
#[logic(open)]
pub fn in_slab8(em: ExtentManager, offset: Int) -> bool {
    pearlite! {
        match em.regions {
            Some(rv) => exists<k: Int, s: Int> 0 <= k && k < rv@.len() && em.arena@[rv@[k]@].slabs@.contains(s)
                && em.arena@[rv@[k]@].slabs@.lookup(s).start_offset@ <= offset
                && offset < em.arena@[rv@[k]@].slabs@.lookup(s).start_offset@ + em.arena@[rv@[k]@].slabs@.lookup(s).slab_size@,
            None => false,
        }
    }
}

/// **EM-REMOVE-ERR-OFFSET-NOT-FOUND** — "When remove_extent(offset) is called with an offset at
/// which no allocated extent exists (including offsets before the data area, past the last
/// region, or not inside any slab), it returns an OffsetNotFound error carrying that offset and
/// changes nothing." LEVEL-2 under D-RANGE-FR-002-84618e (every uniform region spans >= 1
/// byte): for EVERY component state whose regions satisfy the region invariant and EVERY u64
/// offset at which no published extent starts (get_extents' extents, pub_at8), remove_extent
/// (lib.rs:223-255, 680-684 + region.rs:121-150, mirror model/b8.rs remove_extent8) returns
/// OffsetNotFound(offset) and every field of the component is unchanged.
#[requires(em_regions_wf(*em) && em_regions_ok(*em) && rm_safe8(*em))]
#[requires(rb_assume(*em))]
#[requires(!pub_at8(*em, offset@))]
#[ensures(result == Err(EmError::OffsetNotFound(offset)) && unch8(*em, ^em))]
pub fn narrowed_verify_em_remove_err_offset_not_found(em: &mut ExtentManager, offset: u64) -> Result<(), EmError> {
    proof_assert! { rb_pos8(*em) };
    em.remove_extent8(offset)
}

/// Mutant: same body and requires; claims the removal succeeds.
#[requires(em_regions_wf(*em) && em_regions_ok(*em) && rm_safe8(*em))]
#[requires(rb_assume(*em))]
#[requires(!pub_at8(*em, offset@))]
#[ensures(result == Ok(()))]
pub fn narrowed_verify_em_remove_err_offset_not_found__mutant(em: &mut ExtentManager, offset: u64) -> Result<(), EmError> {
    proof_assert! { rb_pos8(*em) };
    em.remove_extent8(offset)
}

/// **EM-REMOVE-ERR-INTERIOR-OFFSET** — "remove_extent() with an offset that falls inside a slab
/// but not at the start of a slot fails with an OffsetNotFound error and changes nothing."
/// LEVEL-2 under D-RANGE-FR-002-84618e: for every component state with the region invariant and
/// every offset inside some current slab at which no slot of any current slab starts,
/// remove_extent returns OffsetNotFound(offset) and changes nothing.
#[requires(em_regions_wf(*em) && em_regions_ok(*em) && rm_safe8(*em))]
#[requires(rb_assume(*em))]
#[requires(in_slab8(*em, offset@) && !slot_at8(*em, offset@))]
#[ensures(result == Err(EmError::OffsetNotFound(offset)) && unch8(*em, ^em))]
pub fn narrowed_verify_em_remove_err_interior_offset(em: &mut ExtentManager, offset: u64) -> Result<(), EmError> {
    proof_assert! { rb_pos8(*em) };
    let pre = snapshot! { *em };
    let r = em.remove_extent8(offset);
    proof_assert! { match r { Ok(()) => match pre.regions { Some(rv) => forall<k: Int> 0 <= k && k < rv@.len()
        && rm_step(pre.arena@[rv@[k]@], em.arena@[rv@[k]@], offset) ==> {
            lemma_rm_pub8(pre.arena@[rv@[k]@], em.arena@[rv@[k]@], offset); slot_in8(pre.arena@[rv@[k]@], offset@) },
        None => true }, Err(_) => true } };
    r
}

/// Mutant: same body and requires; claims the removal succeeds.
#[requires(em_regions_wf(*em) && em_regions_ok(*em) && rm_safe8(*em))]
#[requires(rb_assume(*em))]
#[requires(in_slab8(*em, offset@) && !slot_at8(*em, offset@))]
#[ensures(result == Ok(()))]
pub fn narrowed_verify_em_remove_err_interior_offset__mutant(em: &mut ExtentManager, offset: u64) -> Result<(), EmError> {
    proof_assert! { rb_pos8(*em) };
    let pre = snapshot! { *em };
    let r = em.remove_extent8(offset);
    proof_assert! { match r { Ok(()) => match pre.regions { Some(rv) => forall<k: Int> 0 <= k && k < rv@.len()
        && rm_step(pre.arena@[rv@[k]@], em.arena@[rv@[k]@], offset) ==> {
            lemma_rm_pub8(pre.arena@[rv@[k]@], em.arena@[rv@[k]@], offset); slot_in8(pre.arena@[rv@[k]@], offset@) },
        None => true }, Err(_) => true } };
    r
}

// =============================================================================
// EM-FORMAT-POST-USED-ZERO  (assumes D-RANGE-FR-002-c6d2c5)
// =============================================================================

/// D-RANGE-FR-002-c6d2c5 for format(params) on a metadata device of `md` bytes
/// (num_sectors * sector_size, lib.rs:410-415): data_start_offset (`ds_l`, lib.rs:420-446 —
/// `checkpoint_region_offset + 2 * checkpoint_region_size` when metadata_region_size > 0,
/// else 0), the usable data size and the uniform region size are whole multiples of the
/// sector size. (The two subtraction terms are only read where `data_disk_size >= dso`, the
/// only inputs on which the Rust expression is defined; format rejects the others,
/// lib.rs:447-452.)
#[logic(open)]
pub fn sec_layout(md: Int, p: FormatParams) -> bool {
    pearlite! { a_c6d2c5(md, p) }
}

/// Every region size and base of the format partition is a sector multiple.
#[logic]
#[requires(ss > 0 && n > 0 && ds >= 0 && u >= 0 && ds % ss == 0 && u % ss == 0 && (u / n) % ss == 0)]
#[requires(0 <= i && i < n)]
#[ensures(rsize(u, n, i) % ss == 0 && rbase(ds, u, n, i) % ss == 0 && rsize(u, n, i) >= 0)]
pub fn lemma_part_sec(ds: Int, u: Int, n: Int, i: Int, ss: Int) {
    pearlite! {
        {
            lemma_divmod(u, n);
            lemma_divmod(u / n, ss);
            lemma_kmul_mod(u / n, i, ss);
            lemma_kmul_mod(u / n, n - 1, ss);
            lemma_mul_le(n - 1, n, u / n);
            proof_assert! { (n - 1) * (u / n) <= n * (u / n) && n * (u / n) <= u };
            lemma_mod_add(u, (n - 1) * (u / n), ss);
            lemma_mod_add(ds, i * (u / n), ss)
        }
    }
}

/// The assumption read at format's data start offset: every region size is a sector multiple.
#[logic]
#[requires(sec_layout(md, p) && ds == ds_l(md, p.metadata_region_size@, p.metadata_alignment@, p.sector_size@))]
#[requires(0 <= ds && ds < p.data_disk_size@ && p.sector_size@ > 0 && p.region_count@ > 0 && 0 <= i && i < p.region_count@)]
#[ensures(rsize(p.data_disk_size@ - ds, p.region_count@, i) % p.sector_size@ == 0)]
#[ensures(rbase(ds, p.data_disk_size@ - ds, p.region_count@, i) % p.sector_size@ == 0)]
pub fn lemma_sec_rsize(md: Int, p: FormatParams, ds: Int, i: Int) {
    pearlite! { lemma_part_sec(ds, p.data_disk_size@ - ds, p.region_count@, i, p.sector_size@) }
}

/// The assumption read at format's data start offset `ds`.
#[logic]
#[requires(sec_layout(md, p) && ds == ds_l(md, p.metadata_region_size@, p.metadata_alignment@, p.sector_size@))]
#[requires(ds < p.data_disk_size@)]
#[ensures(ds % p.sector_size@ == 0 && (p.data_disk_size@ - ds) % p.sector_size@ == 0)]
#[ensures(((p.data_disk_size@ - ds) / p.region_count@) % p.sector_size@ == 0)]
pub fn lemma_sec_facts(md: Int, p: FormatParams, ds: Int) {}

/// Every term zero gives a zero sum.
#[logic]
#[variant(k)]
#[requires(0 <= k && k <= rv.len())]
#[requires(forall<i: Int> 0 <= i && i < rv.len() ==> rg_used(arena[rv[i]@]) == 0)]
#[ensures(used_sum(arena, rv, k) == 0)]
pub fn lemma_used_zero(arena: Seq<RegionState>, rv: Seq<usize>, k: Int) {
    if k > 0 {
        lemma_used_zero(arena, rv, k - 1)
    }
}

/// A fresh region over a whole number of sectors has no used bytes.
#[logic]
#[requires(fresh_rg(r, base, size, p) && p.sector_size@ > 0 && size >= 0 && size % p.sector_size@ == 0)]
#[ensures(rg_used(r) == 0)]
pub fn lemma_fresh_used(r: RegionState, base: Int, size: Int, p: FormatParams) {
    pearlite! { lemma_divmod(size, p.sector_size@) }
}

/// **EM-FORMAT-POST-USED-ZERO** — "Immediately after format() succeeds, used_bytes() returns
/// zero." LEVEL-2 under D-RANGE-FR-002-c6d2c5 (data start, usable size and uniform region size
/// are sector multiples): for EVERY format input accepted by the mirror's crate-standard
/// premises, format() (lib.rs:383-512, model/component.rs) followed by used_bytes()
/// (lib.rs:698-708, model/b4.rs) returns 0 whenever format succeeds: every region is a fresh
/// buddy allocator over a whole number of sectors (fresh_rg: total_free = (size / ss) * ss,
/// buddy.rs:10-46), so each term total_usable_size - total_free is 0.
#[requires(em_regions_wf(*em))]
#[requires(em.dev.connected ==> a_67ea4f(em.dev.num_sectors@, em.dev.sector_size@))]
#[requires(a_2ee144(params.metadata_alignment@))]
#[requires(sane_params(params))]
#[requires(sec_layout(em.dev.num_sectors@ * em.dev.sector_size@, params))]
#[ensures(match result.0 { Ok(()) => result.1@ == 0, Err(_) => true })]
pub fn verify_em_format_post_used_zero(em: &mut ExtentManager, params: FormatParams) -> (Result<(), EmError>, u64) {
    let pre = snapshot! { *em };
    let r = em.format(params);
    if r.is_err() {
        return (r, 0);
    }
    let md = snapshot! { pre.dev.num_sectors@ * pre.dev.sector_size@ };
    proof_assert! { sec_layout(*md, params) };
    proof_assert! { match (em.regions, em.shared) { (Some(rv), Some(sh)) =>
        sh.superblock.data_start_offset@ == ds_l(*md, params.metadata_region_size@, params.metadata_alignment@, params.sector_size@)
        && sh.superblock.data_start_offset@ < params.data_disk_size@, _ => false } };
    proof_assert! { match (em.regions, em.shared) { (Some(rv), Some(sh)) =>
        params.sector_size@ > 0 && params.region_count@ > 0 && rv@.len() == params.region_count@, _ => false } };
    proof_assert! { forall<sh: SharedState> em.shared == Some(sh) ==>
        sh.superblock.data_start_offset@ == ds_l(*md, params.metadata_region_size@, params.metadata_alignment@, params.sector_size@)
        && sh.superblock.data_start_offset@ < params.data_disk_size@ };
    proof_assert! { forall<sh: SharedState> em.shared == Some(sh) ==> {
        lemma_sec_facts(*md, params, sh.superblock.data_start_offset@);
        sh.superblock.data_start_offset@ % params.sector_size@ == 0 } };
    proof_assert! { forall<sh: SharedState> em.shared == Some(sh) ==>
        sh.superblock.data_start_offset@ % params.sector_size@ == 0
        && (params.data_disk_size@ - sh.superblock.data_start_offset@) % params.sector_size@ == 0
        && ((params.data_disk_size@ - sh.superblock.data_start_offset@) / params.region_count@) % params.sector_size@ == 0
        && params.sector_size@ > 0 && params.region_count@ > 0 && params.data_disk_size@ - sh.superblock.data_start_offset@ >= 0 };
    proof_assert! { forall<rv: Vec<usize>> em.regions == Some(rv) ==> rv@.len() == params.region_count@ };
    proof_assert! { forall<sh: SharedState, i: Int> em.shared == Some(sh) && 0 <= i && i < params.region_count@ ==> {
        lemma_part_sec(sh.superblock.data_start_offset@, params.data_disk_size@ - sh.superblock.data_start_offset@,
            params.region_count@, i, params.sector_size@);
        rsize(params.data_disk_size@ - sh.superblock.data_start_offset@, params.region_count@, i) % params.sector_size@ == 0
        && rsize(params.data_disk_size@ - sh.superblock.data_start_offset@, params.region_count@, i) >= 0 } };
    proof_assert! { forall<rv: Vec<usize>, sh: SharedState, i: Int> em.regions == Some(rv) && em.shared == Some(sh)
        && 0 <= i && i < rv@.len() ==>
        fresh_rg(em.arena@[rv@[i]@],
            rbase(sh.superblock.data_start_offset@, params.data_disk_size@ - sh.superblock.data_start_offset@, params.region_count@, i),
            rsize(params.data_disk_size@ - sh.superblock.data_start_offset@, params.region_count@, i), params) };
    proof_assert! { forall<rv: Vec<usize>, sh: SharedState, i: Int> em.regions == Some(rv) && em.shared == Some(sh)
        && 0 <= i && i < rv@.len() ==> {
        lemma_fresh_used(em.arena@[rv@[i]@],
            rbase(sh.superblock.data_start_offset@, params.data_disk_size@ - sh.superblock.data_start_offset@, params.region_count@, i),
            rsize(params.data_disk_size@ - sh.superblock.data_start_offset@, params.region_count@, i), params);
        rg_used(em.arena@[rv@[i]@]) == 0 } };
    proof_assert! { match em.regions { Some(rv) => forall<i: Int> 0 <= i && i < rv@.len() ==> rg_inv(em.arena@[rv@[i]@]), None => false } };
    proof_assert! { forall<rv: Vec<usize>> em.regions == Some(rv) ==> {
        lemma_used_zero(em.arena@, rv@, rv@.len()); used_sum(em.arena@, rv@, rv@.len()) == 0 } };
    proof_assert! { match em.regions { Some(rv) => used_terms_ok(em.arena@, rv@), None => false } };
    let u = em.used_bytes();
    (r, u)
}

/// Mutant: same body and requires; claims used_bytes() is non-zero after a successful format.
#[requires(em_regions_wf(*em))]
#[requires(em.dev.connected ==> a_67ea4f(em.dev.num_sectors@, em.dev.sector_size@))]
#[requires(a_2ee144(params.metadata_alignment@))]
#[requires(sane_params(params))]
#[requires(sec_layout(em.dev.num_sectors@ * em.dev.sector_size@, params))]
#[ensures(match result.0 { Ok(()) => result.1@ != 0, Err(_) => true })]
pub fn verify_em_format_post_used_zero__mutant(em: &mut ExtentManager, params: FormatParams) -> (Result<(), EmError>, u64) {
    let pre = snapshot! { *em };
    let r = em.format(params);
    if r.is_err() {
        return (r, 0);
    }
    let md = snapshot! { pre.dev.num_sectors@ * pre.dev.sector_size@ };
    proof_assert! { sec_layout(*md, params) };
    proof_assert! { match (em.regions, em.shared) { (Some(rv), Some(sh)) =>
        sh.superblock.data_start_offset@ == ds_l(*md, params.metadata_region_size@, params.metadata_alignment@, params.sector_size@)
        && sh.superblock.data_start_offset@ < params.data_disk_size@, _ => false } };
    proof_assert! { match (em.regions, em.shared) { (Some(rv), Some(sh)) =>
        params.sector_size@ > 0 && params.region_count@ > 0 && rv@.len() == params.region_count@, _ => false } };
    proof_assert! { forall<sh: SharedState> em.shared == Some(sh) ==>
        sh.superblock.data_start_offset@ == ds_l(*md, params.metadata_region_size@, params.metadata_alignment@, params.sector_size@)
        && sh.superblock.data_start_offset@ < params.data_disk_size@ };
    proof_assert! { forall<sh: SharedState> em.shared == Some(sh) ==> {
        lemma_sec_facts(*md, params, sh.superblock.data_start_offset@);
        sh.superblock.data_start_offset@ % params.sector_size@ == 0 } };
    proof_assert! { forall<sh: SharedState> em.shared == Some(sh) ==>
        sh.superblock.data_start_offset@ % params.sector_size@ == 0
        && (params.data_disk_size@ - sh.superblock.data_start_offset@) % params.sector_size@ == 0
        && ((params.data_disk_size@ - sh.superblock.data_start_offset@) / params.region_count@) % params.sector_size@ == 0
        && params.sector_size@ > 0 && params.region_count@ > 0 && params.data_disk_size@ - sh.superblock.data_start_offset@ >= 0 };
    proof_assert! { forall<rv: Vec<usize>> em.regions == Some(rv) ==> rv@.len() == params.region_count@ };
    proof_assert! { forall<sh: SharedState, i: Int> em.shared == Some(sh) && 0 <= i && i < params.region_count@ ==> {
        lemma_part_sec(sh.superblock.data_start_offset@, params.data_disk_size@ - sh.superblock.data_start_offset@,
            params.region_count@, i, params.sector_size@);
        rsize(params.data_disk_size@ - sh.superblock.data_start_offset@, params.region_count@, i) % params.sector_size@ == 0
        && rsize(params.data_disk_size@ - sh.superblock.data_start_offset@, params.region_count@, i) >= 0 } };
    proof_assert! { forall<rv: Vec<usize>, sh: SharedState, i: Int> em.regions == Some(rv) && em.shared == Some(sh)
        && 0 <= i && i < rv@.len() ==>
        fresh_rg(em.arena@[rv@[i]@],
            rbase(sh.superblock.data_start_offset@, params.data_disk_size@ - sh.superblock.data_start_offset@, params.region_count@, i),
            rsize(params.data_disk_size@ - sh.superblock.data_start_offset@, params.region_count@, i), params) };
    proof_assert! { forall<rv: Vec<usize>, sh: SharedState, i: Int> em.regions == Some(rv) && em.shared == Some(sh)
        && 0 <= i && i < rv@.len() ==> {
        lemma_fresh_used(em.arena@[rv@[i]@],
            rbase(sh.superblock.data_start_offset@, params.data_disk_size@ - sh.superblock.data_start_offset@, params.region_count@, i),
            rsize(params.data_disk_size@ - sh.superblock.data_start_offset@, params.region_count@, i), params);
        rg_used(em.arena@[rv@[i]@]) == 0 } };
    proof_assert! { match em.regions { Some(rv) => forall<i: Int> 0 <= i && i < rv@.len() ==> rg_inv(em.arena@[rv@[i]@]), None => false } };
    proof_assert! { forall<rv: Vec<usize>> em.regions == Some(rv) ==> {
        lemma_used_zero(em.arena@, rv@, rv@.len()); used_sum(em.arena@, rv@, rv@.len()) == 0 } };
    proof_assert! { match em.regions { Some(rv) => used_terms_ok(em.arena@, rv@), None => false } };
    let u = em.used_bytes();
    (r, u)
}

// =============================================================================
// EM-FORMAT-ERR-METADATA-TOO-SMALL  (assumes D-RANGE-FMT-METADATA-DEVICE-LAYOUT-2ee144)
// =============================================================================

/// An FR-002-valid parameter set with metadata_alignment = u64::MAX - 4095, the LARGEST value
/// the level-2 (pre-correction) form of 2ee144, `<= u64::MAX - (SUPERBLOCK_SIZE as u64 - 1)`,
/// admitted (one past the corrected bound); sector 512, slab 512, one region, a 1 MiB data disk, metadata_region_size 0.
#[ensures(result.metadata_alignment@ == 18446744073709547520 && result.sector_size@ == 512)]
#[ensures(result.metadata_region_size@ == 0 && result.slab_size@ == 512 && result.max_extent_size@ == 512)]
#[ensures(result.region_count@ == 1 && result.data_disk_size@ == 1048576)]
pub fn wtoo_params() -> FormatParams {
    FormatParams {
        data_disk_size: 1048576,
        slab_size: 512,
        max_extent_size: 512,
        sector_size: 512,
        region_count: 1,
        metadata_alignment: 18446744073709547520,
        instance_id: Some(1),
        metadata_disk_ns_id: 1,
        metadata_region_size: 0,
    }
}

/// LEVEL-3 RETIRED (support only, not a refutation): alignment u64::MAX - 4095 lies OUTSIDE
/// the CORRECTED D-RANGE-FMT-METADATA-DEVICE-LAYOUT-2ee144 (`metadata_alignment <= u64::MAX -
/// SUPERBLOCK_SIZE`); the property is proved inside it by `verify_em_format_err_metadata_too_small`
/// (props/batch6.rs). Kept as the documented one-past-the-bound behaviour.
/// (Level-2 text:) **EM-FORMAT-ERR-METADATA-TOO-SMALL** — REFUTED INSIDE THE RANGE (LEVEL-2, assumption
/// D-RANGE-FMT-METADATA-DEVICE-LAYOUT-2ee144 holds). "When the metadata space available after
/// the superblock ... is too small to hold two checkpoint copies of at least one sector each,
/// format() returns a CorruptMetadata error." ROOT CAUSE (FORMAT-ALIGN-OVERFLOW, off by one
/// against the assumption): lib.rs:423 is `(sb_size + alignment - 1) / alignment * alignment`,
/// which Rust evaluates as `(sb_size + alignment) - 1`; the assumption bounds `alignment` by
/// u64::MAX - 4095, so `4096 + alignment` reaches u64::MAX + 1 at the bound. Witness: the
/// parameters `wtoo_params` (FR-002-valid: the checks lib.rs:387-404 pass), a connected
/// metadata device of 4608 bytes (9 x 512: 512 bytes after the superblock < 2 x 512): format
/// reaches lib.rs:423 (alignment != 0) and `4096 + alignment` overflows u64 — a DEBUG build
/// panics (no CorruptMetadata). (A RELEASE build wraps to 0 - 1 = u64::MAX, u64::MAX /
/// alignment * alignment = alignment, remaining 0, and does return CorruptMetadata.)
#[ensures(result.metadata_alignment@ <= u64::MAX@ - (4096 - 1))]
#[ensures(fr002(result) && result.metadata_alignment@ != 0)]
#[ensures(4096 + result.metadata_alignment@ > u64::MAX@)]
#[ensures(eff_l(4608, result.metadata_region_size@) - 4096 < 2 * result.sector_size@)]
pub fn support_old_refute_em_format_err_metadata_too_small_at_bound() -> FormatParams {
    fact_pow2_one();
    wtoo_params()
}

/// Mutant: claims lib.rs:423's first addition does not overflow on that input.
#[ensures(4096 + result.metadata_alignment@ <= u64::MAX@)]
pub fn support_old_refute_em_format_err_metadata_too_small_at_bound__mutant() -> FormatParams {
    fact_pow2_one();
    wtoo_params()
}

// =============================================================================
// Runtime slabs: EM-SLAB-ROVER-IN-RANGE / EM-INV-KEY-VECTOR-LENGTH
//   (assume D-RANGE-FMT-CHECKPOINT-PAYLOAD-e0fe79)
// =============================================================================

/// Division is antitone in the divisor: `x / b <= x / a` for `0 < a <= b`.
#[logic]
#[requires(0 < a && a <= b && x >= 0)]
#[ensures(x / b <= x / a)]
pub fn lemma_div_anti(x: Int, a: Int, b: Int) {
    pearlite! {
        {
            lemma_divmod(x, a);
            lemma_divmod(x, b);
            lemma_mul_le(a, b, x / b);
            proof_assert! { (x / b) * a <= x };
            lemma_divmod((x / b) * a, a);
            lemma_mul_mod0(a, x / b)
        }
    }
}

/// The element size `align(size, ss)` of a reservation of `size >= 1` bytes is at least one
/// sector, and its slab of `slab_size` bytes (a sector multiple, at least the element size)
/// holds `slab_size / element_size` slots, between 1 and `slab_size / ss <= u32::MAX`: the
/// slot count `(slab_size / element_size) as u32` (slab.rs:18) does not truncate.
#[logic]
#[requires(ss > 0 && size > 0 && align_l(size, ss) <= slab && slab / ss <= u32::MAX@)]
#[ensures(align_l(size, ss) >= ss)]
#[ensures(1 <= slab / align_l(size, ss) && slab / align_l(size, ss) <= u32::MAX@)]
#[ensures(slots_of(slab, align_l(size, ss)) == slab / align_l(size, ss))]
pub fn lemma_es_slots(size: Int, ss: Int, slab: Int) {
    pearlite! {
        {
            lemma_divmod(size + ss - 1, ss);
            proof_assert! { (size + ss - 1) / ss >= 1 };
            lemma_mul_le(1, (size + ss - 1) / ss, ss);
            lemma_div_anti(slab, ss, align_l(size, ss));
            lemma_divmod(slab, align_l(size, ss));
            lemma_mul_mod0(align_l(size, ss), 1);
            proof_assert! { slab / align_l(size, ss) >= 1 };
            lemma_divmod(slab / align_l(size, ss), 4294967296)
        }
    }
}

/// One operation the component performs on a slab (region.rs uses exactly these):
/// `alloc_slot` (region.rs:52, 78), `free_slot` of an allocated in-range slot (region.rs:96;
/// the abort / FREE_KEY-publish / deferred-free paths), `set_key` of an in-range slot
/// (region.rs:113).
#[derive(::core::clone::Clone, ::core::marker::Copy)]
pub enum SOp {
    Alloc,
    Free(usize),
    Key(usize, u64),
}

/// Apply one slab operation (an out-of-range / unallocated index is never passed by the
/// component; such an op is skipped).
#[requires(slab_inv(*s))]
#[ensures(slab_inv(^s))]
#[ensures((^s).start_offset == s.start_offset && (^s).slab_size == s.slab_size && (^s).element_size == s.element_size)]
#[ensures((^s).bitmap.num_slots == s.bitmap.num_slots && (^s).keys@.len() == s.keys@.len())]
pub fn apply_sop(s: &mut Slab, op: SOp) {
    match op {
        SOp::Alloc => {
            let _ = s.alloc_slot();
        }
        SOp::Free(i) => {
            if i < s.bitmap.num_slots as usize {
                if s.bitmap.is_set(i) {
                    s.free_slot(i);
                }
            }
        }
        SOp::Key(i, k) => {
            if i < s.keys.len() {
                s.set_key(i, k);
            }
        }
    }
}

/// region.rs:37 `(size + ss - 1) / ss * ss` under D-RANGE-FR-005-3783f8 (`size + ss <= u32::MAX`),
/// in the code's own evaluation order — the same mirror as model/region.rs align_to_sector_size.
#[requires(ss@ > 0 && size@ > 0 && a_3783f8(size@, ss@))]
#[ensures(result@ == align_l(size@, ss@))]
pub fn align_rel_l2b(size: u32, ss: u32) -> u32 {
    proof_assert! { lemma_divmod(size@ + ss@ - 1, ss@); (size@ + ss@ - 1) / ss@ * ss@ <= size@ + ss@ - 1 };
    (size + ss - 1) / ss * ss
}

/// The life of every slab the component creates at run time: alloc_extent's fresh slab
/// (region.rs:42 element size, region.rs:68 guard passed, region.rs:72-77 carved at the buddy
/// block `start`), then ANY sequence of slab operations. Every intermediate state satisfies
/// `slab_inv` with `slab_size / element_size` slots (>= 1) and that many keys.
#[requires(ss@ > 0 && slab_size@ % ss@ == 0)]
#[requires(size@ > 0 && a_3783f8(size@, ss@))]
#[requires(align_l(size@, ss@) <= slab_size@)]
#[requires(start@ + slab_size@ <= u64::MAX@)]
#[requires(slab_size@ / ss@ <= u32::MAX@)]
#[ensures(slab_inv(result) && result.slab_size == slab_size && result.element_size@ == align_l(size@, ss@))]
#[ensures(result.bitmap.num_slots@ == slab_size@ / result.element_size@ && result.bitmap.num_slots@ >= 1)]
#[ensures(result.keys@.len() == result.bitmap.num_slots@)]
pub fn slab_life(start: u64, slab_size: u64, ss: u32, size: u32, ops: &Vec<SOp>) -> Slab {
    let es = align_rel_l2b(size, ss); // region.rs:42
    proof_assert! { lemma_es_slots(size@, ss@, slab_size@); slots_of(slab_size@, es@) == slab_size@ / es@ && slab_size@ / es@ >= 1 };
    let mut s = Slab::new(start, slab_size, es); // region.rs:77
    let mut k: usize = 0;
    #[invariant(k@ <= ops@.len())]
    #[invariant(slab_inv(s) && s.slab_size == slab_size && s.element_size == es)]
    #[invariant(s.bitmap.num_slots@ == slab_size@ / es@ && s.bitmap.num_slots@ >= 1)]
    #[invariant(s.rover@ < s.bitmap.num_slots@)]
    while k < ops.len() {
        apply_sop(&mut s, ops[k]);
        k += 1;
    }
    s
}

/// LEVEL-3 (phase D-B2): the life of every slab initialize RECOVERS (slab_from_descriptor,
/// recovery.rs:76-85) from a descriptor in the declared payload range (desc_ok_a = 980f61 +
/// 3b612e + the decoder fact t_dec_keys; the descriptor is taken abstractly) of a region built
/// within the data disk (rg_built — the PROVED invariant em_rg_built), then ANY sequence of slab
/// operations: slab_inv, `slab_size / element_size` (>= 1) slots and that many keys throughout.
#[requires(desc_ok_a(*desc, *base, *size, *fp) && rg_built(*base, *size, *fp) && *base >= 0 && *size >= 0)]
#[ensures(slab_inv(result) && result.slab_size == desc.slab_size && result.element_size == desc.element_size)]
#[ensures(result.bitmap.num_slots@ == result.slab_size@ / result.element_size@ && result.bitmap.num_slots@ >= 1)]
#[ensures(result.keys@.len() == result.bitmap.num_slots@ && result.rover@ < result.bitmap.num_slots@)]
pub fn slab_life_rec(desc: &SlabDescriptor, base: Snapshot<Int>, size: Snapshot<Int>, fp: Snapshot<FormatParams>, ops: &Vec<SOp>) -> Slab {
    proof_assert! { lemma_desc_slots(*desc, *base, *size); u_desc_slots_u32(*desc) };
    proof_assert! { desc.slab_size@ / desc.element_size@ >= 1 };
    proof_assert! { slots_of(desc.slab_size@, desc.element_size@) == desc.slab_size@ / desc.element_size@ };
    proof_assert! { desc.start_offset@ + desc.slab_size@ <= fp.data_disk_size@ && fp.data_disk_size@ <= u64::MAX@ };
    let mut s = slab_from_descriptor(desc);
    let mut k: usize = 0;
    #[invariant(k@ <= ops@.len())]
    #[invariant(slab_inv(s) && s.slab_size == desc.slab_size && s.element_size == desc.element_size)]
    #[invariant(s.bitmap.num_slots@ == desc.slab_size@ / desc.element_size@ && s.bitmap.num_slots@ >= 1)]
    #[invariant(s.rover@ < s.bitmap.num_slots@)]
    while k < ops.len() {
        apply_sop(&mut s, ops[k]);
        k += 1;
    }
    s
}

/// **EM-SLAB-ROVER-IN-RANGE** — "The position from which a slab searches for its next free
/// slot is always less than the slab's number of slots." LEVEL-2 under
/// D-RANGE-FMT-CHECKPOINT-PAYLOAD-e0fe79 (`slab_size / sector_size <= u32::MAX`): for every
/// slab the component carves at run time (alloc_extent with FR-002 parameters, a reservation
/// size in the documented range, region.rs:68 passed) and after ANY sequence of
/// alloc_slot / free_slot / set_key, the rover is a valid slot index (the slab has
/// slab_size / element_size >= 1 slots; slab.rs:18 does not truncate).
// LEVEL-3 (phase D-B2): WIDENED to recovered slabs too — result.1 is a slab initialize rebuilds
// from a descriptor in the declared payload ranges (slab_life_rec: 980f61 + 3b612e + decoder fact
// t_dec_keys, region within the data disk: em_rg_built). The fresh-slab half's premises are the
// call site's (alloc_extent's new-slab path): ss > 0 and slab % ss == 0 (rg_params of the region),
// align <= slab (region.rs:68 guard), start + slab <= MAX (the carved buddy block, good_block +
// bd_shape), slab / ss <= u32::MAX (e0fe79), and the reserve ranges 231ab0 / 3783f8.
#[requires(ss@ > 0 && slab_size@ % ss@ == 0)]
#[requires(size@ > 0 && a_3783f8(size@, ss@))]
#[requires(align_l(size@, ss@) <= slab_size@)]
#[requires(start@ + slab_size@ <= u64::MAX@)]
#[requires(slab_size@ / ss@ <= u32::MAX@)]
#[requires(desc_ok_a(*desc, *base, *rsize, *fp) && rg_built(*base, *rsize, *fp) && *base >= 0 && *rsize >= 0)]
#[ensures(result.0.rover@ < result.0.bitmap.num_slots@ && result.1.rover@ < result.1.bitmap.num_slots@)]
pub fn verify_em_slab_rover_in_range(start: u64, slab_size: u64, ss: u32, size: u32, ops: &Vec<SOp>, desc: &SlabDescriptor, base: Snapshot<Int>, rsize: Snapshot<Int>, fp: Snapshot<FormatParams>, ops2: &Vec<SOp>) -> (Slab, Slab) {
    (slab_life(start, slab_size, ss, size, ops), slab_life_rec(desc, base, rsize, fp, ops2))
}

/// Mutant: same body and requires; claims the rover has run off the slot range.
// LEVEL-3 (phase D-B2): WIDENED to recovered slabs too — result.1 is a slab initialize rebuilds
// from a descriptor in the declared payload ranges (slab_life_rec: 980f61 + 3b612e + decoder fact
// t_dec_keys, region within the data disk: em_rg_built). The fresh-slab half's premises are the
// call site's (alloc_extent's new-slab path): ss > 0 and slab % ss == 0 (rg_params of the region),
// align <= slab (region.rs:68 guard), start + slab <= MAX (the carved buddy block, good_block +
// bd_shape), slab / ss <= u32::MAX (e0fe79), and the reserve ranges 231ab0 / 3783f8.
#[requires(ss@ > 0 && slab_size@ % ss@ == 0)]
#[requires(size@ > 0 && a_3783f8(size@, ss@))]
#[requires(align_l(size@, ss@) <= slab_size@)]
#[requires(start@ + slab_size@ <= u64::MAX@)]
#[requires(slab_size@ / ss@ <= u32::MAX@)]
#[requires(desc_ok_a(*desc, *base, *rsize, *fp) && rg_built(*base, *rsize, *fp) && *base >= 0 && *rsize >= 0)]
#[ensures(result.0.rover@ >= result.0.bitmap.num_slots@ || result.1.rover@ >= result.1.bitmap.num_slots@)]
pub fn verify_em_slab_rover_in_range__mutant(start: u64, slab_size: u64, ss: u32, size: u32, ops: &Vec<SOp>, desc: &SlabDescriptor, base: Snapshot<Int>, rsize: Snapshot<Int>, fp: Snapshot<FormatParams>, ops2: &Vec<SOp>) -> (Slab, Slab) {
    (slab_life(start, slab_size, ss, size, ops), slab_life_rec(desc, base, rsize, fp, ops2))
}

/// **EM-INV-KEY-VECTOR-LENGTH** — "Every slab keeps exactly one key entry per slot, so the
/// number of key entries equals the slab size divided by the element size." LEVEL-2 under
/// D-RANGE-FMT-CHECKPOINT-PAYLOAD-e0fe79: for every slab carved at run time and after ANY
/// sequence of slab operations, keys.len() == num_slots == slab_size / element_size.
// LEVEL-3 (phase D-B2): WIDENED to recovered slabs too — result.1 is a slab initialize rebuilds
// from a descriptor in the declared payload ranges (slab_life_rec: 980f61 + 3b612e + decoder fact
// t_dec_keys, region within the data disk: em_rg_built). The fresh-slab half's premises are the
// call site's (alloc_extent's new-slab path): ss > 0 and slab % ss == 0 (rg_params of the region),
// align <= slab (region.rs:68 guard), start + slab <= MAX (the carved buddy block, good_block +
// bd_shape), slab / ss <= u32::MAX (e0fe79), and the reserve ranges 231ab0 / 3783f8.
#[requires(ss@ > 0 && slab_size@ % ss@ == 0)]
#[requires(size@ > 0 && a_3783f8(size@, ss@))]
#[requires(align_l(size@, ss@) <= slab_size@)]
#[requires(start@ + slab_size@ <= u64::MAX@)]
#[requires(slab_size@ / ss@ <= u32::MAX@)]
#[requires(desc_ok_a(*desc, *base, *rsize, *fp) && rg_built(*base, *rsize, *fp) && *base >= 0 && *rsize >= 0)]
#[ensures(result.0.keys@.len() == result.0.bitmap.num_slots@ && result.1.keys@.len() == result.1.bitmap.num_slots@)]
#[ensures(result.0.keys@.len() == result.0.slab_size@ / result.0.element_size@ && result.1.keys@.len() == result.1.slab_size@ / result.1.element_size@)]
pub fn verify_em_inv_key_vector_length(start: u64, slab_size: u64, ss: u32, size: u32, ops: &Vec<SOp>, desc: &SlabDescriptor, base: Snapshot<Int>, rsize: Snapshot<Int>, fp: Snapshot<FormatParams>, ops2: &Vec<SOp>) -> (Slab, Slab) {
    (slab_life(start, slab_size, ss, size, ops), slab_life_rec(desc, base, rsize, fp, ops2))
}

/// Mutant: same body and requires; claims the key vector is shorter than slab / element.
// LEVEL-3 (phase D-B2): WIDENED to recovered slabs too — result.1 is a slab initialize rebuilds
// from a descriptor in the declared payload ranges (slab_life_rec: 980f61 + 3b612e + decoder fact
// t_dec_keys, region within the data disk: em_rg_built). The fresh-slab half's premises are the
// call site's (alloc_extent's new-slab path): ss > 0 and slab % ss == 0 (rg_params of the region),
// align <= slab (region.rs:68 guard), start + slab <= MAX (the carved buddy block, good_block +
// bd_shape), slab / ss <= u32::MAX (e0fe79), and the reserve ranges 231ab0 / 3783f8.
#[requires(ss@ > 0 && slab_size@ % ss@ == 0)]
#[requires(size@ > 0 && a_3783f8(size@, ss@))]
#[requires(align_l(size@, ss@) <= slab_size@)]
#[requires(start@ + slab_size@ <= u64::MAX@)]
#[requires(slab_size@ / ss@ <= u32::MAX@)]
#[requires(desc_ok_a(*desc, *base, *rsize, *fp) && rg_built(*base, *rsize, *fp) && *base >= 0 && *rsize >= 0)]
#[ensures(result.0.keys@.len() < result.0.slab_size@ / result.0.element_size@ || result.1.keys@.len() < result.1.slab_size@ / result.1.element_size@)]
pub fn verify_em_inv_key_vector_length__mutant(start: u64, slab_size: u64, ss: u32, size: u32, ops: &Vec<SOp>, desc: &SlabDescriptor, base: Snapshot<Int>, rsize: Snapshot<Int>, fp: Snapshot<FormatParams>, ops2: &Vec<SOp>) -> (Slab, Slab) {
    (slab_life(start, slab_size, ss, size, ops), slab_life_rec(desc, base, rsize, fp, ops2))
}

// =============================================================================
// EM-RESERVE-POST-OFFSET-SECTOR-ALIGNED
//   (assumes D-RANGE-FR-002-c6d2c5 and D-RANGE-FR-002-217ace)
// =============================================================================

/// `align(size, ss)` is a sector multiple.
#[logic]
#[requires(ss > 0 && size >= 0)]
#[ensures(align_l(size, ss) % ss == 0 && align_l(size, ss) >= 0)]
pub fn lemma_align_mult(size: Int, ss: Int) {
    pearlite! { { lemma_divmod(size + ss - 1, ss); lemma_mul_mod0(ss, (size + ss - 1) / ss) } }
}

/// Every slot of a slab of a region whose base is a sector multiple, with an element size
/// that is a sector multiple, starts at a sector multiple: the slab starts at base + a
/// multiple of its order-K block span(ss, K) = 2^K * ss (slab_ok: good_block).
#[logic]
#[requires(rg_inv(r) && r.slabs@.contains(s) && 0 <= i)]
#[requires(r.buddy.base_offset@ % r.format_params.sector_size@ == 0)]
#[requires(r.slabs@.lookup(s).element_size@ % r.format_params.sector_size@ == 0)]
#[ensures(slot_off(r.slabs@.lookup(s), i) % r.format_params.sector_size@ == 0)]
pub fn lemma_off_aligned(r: RegionState, s: Int, i: Int) {
    pearlite! {
        {
            lemma_pow2(0);
            proof_assert! { span(r.format_params.sector_size@, 0) == r.format_params.sector_size@ };
            lemma_span_div(r.slabs@.lookup(s).start_offset@ - r.buddy.base_offset@, r.format_params.sector_size@, 0, slab_k(r));
            lemma_mod_add(r.buddy.base_offset@, r.slabs@.lookup(s).start_offset@ - r.buddy.base_offset@, r.format_params.sector_size@);
            lemma_kmul_mod(r.slabs@.lookup(s).element_size@, i, r.format_params.sector_size@);
            proof_assert! { i * r.slabs@.lookup(s).element_size@ == r.slabs@.lookup(s).element_size@ * i };
            lemma_mod_add(r.slabs@.lookup(s).start_offset@, i * r.slabs@.lookup(s).element_size@, r.format_params.sector_size@)
        }
    }
}

/// The format part: under D-RANGE-FR-002-c6d2c5 every region format() builds starts at a
/// sector multiple (base = data_start_offset + i * (usable / region_count), lib.rs:459).
#[logic(open)]
pub fn bases_aligned(em: ExtentManager, ss: Int) -> bool {
    pearlite! {
        match em.regions {
            Some(rv) => forall<i: Int> 0 <= i && i < rv@.len() ==> em.arena@[rv@[i]@].buddy.base_offset@ % ss == 0,
            None => true,
        }
    }
}

/// The two halves shared by the scored module and its mutant: format() places every region
/// on a sector multiple; then on any reachable region (lg, D-RANGE-FR-002-217ace) whose base
/// is a sector multiple, any call keeps the base and the invariant, and the next
/// reservation returns a sector-multiple offset.
#[requires(em_regions_wf(*em))]
#[requires(em.dev.connected ==> a_67ea4f(em.dev.num_sectors@, em.dev.sector_size@))]
#[requires(a_2ee144(params.metadata_alignment@))]
#[requires(sane_params(params))]
#[requires(sec_layout(em.dev.num_sectors@ * em.dev.sector_size@, params))]
#[requires(lg(*r, hs@) && lop_pre(*r, hs@, op))]
#[requires(r.buddy.base_offset@ % r.format_params.sector_size@ == 0)]
#[requires(sz@ > 0 && a_3783f8(sz@, r.format_params.sector_size@))]
#[ensures(match result.0 { Ok(()) => bases_aligned(^em, params.sector_size@), Err(_) => true })]
#[ensures(lg(^r, (^hs)@) && rg_frame(*r, ^r))]
#[ensures(match result.1 { Ok((_, _, off)) => off@ % r.format_params.sector_size@ == 0, Err(_) => true })]
pub fn rsv_aligned(em: &mut ExtentManager, params: FormatParams, r: &mut RegionState, hs: &mut Vec<(u64, usize)>, op: LOp, sz: u32)
    -> (Result<(), EmError>, Result<(u64, usize, u64), EmError>) {
    let pre = snapshot! { *em };
    let f = em.format(params);
    let md = snapshot! { pre.dev.num_sectors@ * pre.dev.sector_size@ };
    if f.is_ok() {
        proof_assert! { forall<sh: SharedState> em.shared == Some(sh) ==>
            sh.superblock.data_start_offset@ == ds_l(*md, params.metadata_region_size@, params.metadata_alignment@, params.sector_size@)
            && sh.superblock.data_start_offset@ < params.data_disk_size@ };
        proof_assert! { forall<sh: SharedState> em.shared == Some(sh) ==> {
            lemma_sec_facts(*md, params, sh.superblock.data_start_offset@);
            sh.superblock.data_start_offset@ % params.sector_size@ == 0 } };
        proof_assert! { forall<rv: Vec<usize>> em.regions == Some(rv) ==> rv@.len() == params.region_count@ };
        proof_assert! { params.sector_size@ > 0 && params.region_count@ > 0 };
        proof_assert! { forall<sh: SharedState, i: Int> em.shared == Some(sh) && 0 <= i && i < params.region_count@ ==> {
            lemma_sec_rsize(*md, params, sh.superblock.data_start_offset@, i);
            rbase(sh.superblock.data_start_offset@, params.data_disk_size@ - sh.superblock.data_start_offset@, params.region_count@, i)
                % params.sector_size@ == 0 } };
        proof_assert! { forall<rv: Vec<usize>, sh: SharedState, i: Int> em.regions == Some(rv) && em.shared == Some(sh)
            && 0 <= i && i < rv@.len() ==>
            fresh_rg(em.arena@[rv@[i]@],
                rbase(sh.superblock.data_start_offset@, params.data_disk_size@ - sh.superblock.data_start_offset@, params.region_count@, i),
                rsize(params.data_disk_size@ - sh.superblock.data_start_offset@, params.region_count@, i), params) };
        proof_assert! { forall<rv: Vec<usize>, i: Int> em.regions == Some(rv) && 0 <= i && i < rv@.len() ==>
            em.arena@[rv@[i]@].buddy.base_offset@ % params.sector_size@ == 0 };
    }
    let a0 = snapshot! { *r };
    apply_lop(r, hs, op);
    proof_assert! { r.format_params == a0.format_params && r.buddy.base_offset == a0.buddy.base_offset };
    let a = snapshot! { *r };
    let res = reserve_h(r, hs, sz);
    proof_assert! { match res { Ok((s, i, off)) => {
        lemma_align_mult(sz@, a.format_params.sector_size@);
        lemma_off_aligned(*r, s@, i@);
        off@ % a.format_params.sector_size@ == 0 }, Err(_) => true } };
    (f, res)
}

/// **EM-RESERVE-POST-OFFSET-SECTOR-ALIGNED** — "When reserve_extent() succeeds, the returned
/// handle's extent_offset() is a disk byte offset that is an exact multiple of the sector
/// size." LEVEL-2 under D-RANGE-FR-002-c6d2c5 (data start, usable size and uniform region size
/// are sector multiples) AND D-RANGE-FR-002-217ace (power-of-two sector size, carried by the
/// batch-A region invariant lg: it is what makes every size-class entry name a slab of its
/// own element size, sc_ok). format() places every region on a sector multiple
/// (`bases_aligned`); for EVERY reachable region (lg) on a sector-multiple base, after ANY
/// call, a successful reservation (region.rs:40-91 via lib.rs:589; the handle's
/// extent_offset is that offset, lib.rs:626) returns a sector-multiple offset.
#[requires(em_regions_wf(*em))]
#[requires(em.dev.connected ==> a_67ea4f(em.dev.num_sectors@, em.dev.sector_size@))]
#[requires(a_2ee144(params.metadata_alignment@))]
#[requires(sane_params(params))]
#[requires(sec_layout(em.dev.num_sectors@ * em.dev.sector_size@, params))]
#[requires(lg(*r, hs@) && lop_pre(*r, hs@, op))]
#[requires(r.buddy.base_offset@ % r.format_params.sector_size@ == 0)]
#[requires(sz@ > 0 && a_3783f8(sz@, r.format_params.sector_size@))]
#[ensures(match result.0 { Ok(()) => bases_aligned(^em, params.sector_size@), Err(_) => true })]
#[ensures((^r).buddy.base_offset@ % (^r).format_params.sector_size@ == 0)]
#[ensures(match result.1 { Ok((_, _, off)) => off@ % r.format_params.sector_size@ == 0, Err(_) => true })]
pub fn narrowed_verify_em_reserve_post_offset_sector_aligned(em: &mut ExtentManager, params: FormatParams, r: &mut RegionState,
    hs: &mut Vec<(u64, usize)>, op: LOp, sz: u32) -> (Result<(), EmError>, Result<(u64, usize, u64), EmError>) {
    rsv_aligned(em, params, r, hs, op, sz)
}

/// Mutant: same body and requires; claims a successful reservation is off the sector grid.
#[requires(em_regions_wf(*em))]
#[requires(em.dev.connected ==> a_67ea4f(em.dev.num_sectors@, em.dev.sector_size@))]
#[requires(a_2ee144(params.metadata_alignment@))]
#[requires(sane_params(params))]
#[requires(sec_layout(em.dev.num_sectors@ * em.dev.sector_size@, params))]
#[requires(lg(*r, hs@) && lop_pre(*r, hs@, op))]
#[requires(r.buddy.base_offset@ % r.format_params.sector_size@ == 0)]
#[requires(sz@ > 0 && a_3783f8(sz@, r.format_params.sector_size@))]
#[ensures(match result.1 { Ok((_, _, off)) => off@ % r.format_params.sector_size@ != 0, Err(_) => true })]
pub fn narrowed_verify_em_reserve_post_offset_sector_aligned__mutant(em: &mut ExtentManager, params: FormatParams, r: &mut RegionState,
    hs: &mut Vec<(u64, usize)>, op: LOp, sz: u32) -> (Result<(), EmError>, Result<(u64, usize, u64), EmError>) {
    rsv_aligned(em, params, r, hs, op, sz)
}

// =============================================================================
// EM-BUDDY-MARK-ALLOCATED-PRE
//   (assumes D-RANGE-FMT-CHECKPOINT-PAYLOAD-980f61 and D-RANGE-FR-002-217ace)
// =============================================================================

/// Descriptor geometry (the part of desc_ok the buddy rebuild reads): the configured slab
/// size, on an order-K buddy block (`dsk(fp)` bytes, aligned to it) inside the region.
#[logic(open)]
pub fn dgeo(d: SlabDescriptor, base: Int, size: Int, fp: FormatParams) -> bool {
    pearlite! {
        d.slab_size == fp.slab_size
        && d.start_offset@ >= base
        && (d.start_offset@ - base) % dsk(fp) == 0
        && d.start_offset@ - base + dsk(fp) <= size
    }
}

/// D-RANGE-FMT-CHECKPOINT-PAYLOAD-980f61 for the descriptors `ds` recovered for one region
/// (base, size) of the superblock's geometry `fp` (the region's list `regs[i]`; `regs.len() ==
/// region_count` is what lets initialize pair list i with region i, lib.rs:538-565): every
/// descriptor has a non-zero element size not above its slab size, exactly
/// slab_size / element_size keys, lies on a buddy block of the slab order inside region i
/// (`dgeo`), and overlaps no other descriptor of the list.
#[logic(open)]
pub fn descs_sane(ds: Seq<SlabDescriptor>, base: Int, size: Int, fp: FormatParams) -> bool {
    pearlite! {
        a_980f61_list(ds, base, size)
        && forall<p: Int> 0 <= p && p < ds.len() ==> a_3b612e_d(ds[p], base, size, fp.slab_size@)
    }
}

/// The statement as a precondition of mark_allocated(abs, size): the range (its order-K
/// buddy block `[abs - base, + span(ss, ord(size / ss)))`) lies inside the region and every
/// byte of it is currently free.
#[logic(open)]
pub fn mark_pre_ok(b: BuddyAllocator, abs: Int, size: Int) -> bool {
    pearlite! {
        abs >= b.base_offset@
        && abs - b.base_offset@ + span(b.sector_size@, ord(size / b.sector_size@)) <= b.total_usable_size@
        && forall<z: Int> abs - b.base_offset@ <= z && z < abs - b.base_offset@ + span(b.sector_size@, ord(size / b.sector_size@)) ==>
            ffree(b.free_lists@, b.sector_size@, z)
    }
}

/// `mark_allocated` (buddy.rs:117-157) called only under the statement's precondition.
#[requires(bd_inv(*b) && ord(size@ / b.sector_size@) <= b.max_order@)]
#[requires(mark_pre_ok(*b, abs@, size@))]
#[ensures(bd_inv(^b) && bd_aligned(^b))]
#[ensures((^b).base_offset == b.base_offset && (^b).total_usable_size == b.total_usable_size)]
#[ensures((^b).sector_size == b.sector_size && (^b).max_order == b.max_order)]
#[ensures((^b).free_lists@.len() == b.free_lists@.len())]
#[ensures(mk_spec(b.free_lists@, (^b).free_lists@, b.sector_size@, abs@ - b.base_offset@, ord(size@ / b.sector_size@)))]
pub fn mark_checked(b: &mut BuddyAllocator, abs: u64, size: u64) {
    b.mark_allocated(abs, size)
}

/// A block inside a free block is entirely free.
#[logic]
#[requires(inblk(fl, ss, kk, t))]
#[ensures(forall<z: Int> t <= z && z < t + span(ss, kk) ==> ffree(fl, ss, z))]
pub fn lemma_inblk_free(fl: Seq<Vec<u64>>, ss: Int, kk: Int, t: Int) {}

/// lemma_desc_k over the descriptor geometry only.
#[logic]
#[requires(dgeo(d, base, size, fp))]
#[requires(fp.sector_size@ > 0 && b.sector_size == fp.sector_size && b.total_usable_size@ == size)]
#[requires(b.total_usable_size@ / b.sector_size@ >= 1 ==> b.total_usable_size@ / b.sector_size@ < (b.max_order@ + 1).pow2())]
#[requires(0 <= b.max_order@)]
#[ensures(ord(d.slab_size@ / fp.sector_size@) <= b.max_order@)]
pub fn lemma_dgeo_k(d: SlabDescriptor, b: BuddyAllocator, base: Int, size: Int, fp: FormatParams) {
    pearlite! {
        lemma_ord_ge(fp.slab_size@ / fp.sector_size@);
        lemma_span_pos(fp.sector_size@, ord(fp.slab_size@ / fp.sector_size@));
        lemma_pow2(ord(fp.slab_size@ / fp.sector_size@));
        lemma_div_le(ord(fp.slab_size@ / fp.sector_size@).pow2(), fp.sector_size@, size);
        lemma_pow2_strict(ord(fp.slab_size@ / fp.sector_size@), b.max_order@ + 1)
    }
}

/// Before marking descriptor i: its block avoids the first i descriptors' blocks
/// (distinct aligned starts of one block size), so rb1 puts it inside a free block.
#[logic]
#[requires(p2(fp.sector_size@) && starts_distinct(ds) && rb1(bpre, b0, ds, i, base, size, fp))]
#[requires(0 <= i && i < ds.len())]
#[requires(forall<p: Int> 0 <= p && p < ds.len() ==> dgeo(ds[p], base, size, fp))]
#[requires(fp.sector_size@ > 0 && fp.slab_size@ > 0)]
#[ensures(inblk(bpre.free_lists@, fp.sector_size@, ord(fp.slab_size@ / fp.sector_size@), ds[i].start_offset@ - base))]
#[ensures(mk_hyp(bpre.free_lists@, fp.sector_size@, ds[i].start_offset@ - base, ord(fp.slab_size@ / fp.sector_size@)))]
pub fn lemma_pre_inblk(bpre: BuddyAllocator, b0: BuddyAllocator, ds: Seq<SlabDescriptor>, i: Int, base: Int, size: Int, fp: FormatParams) {
    pearlite! {
        lemma_ord_ge(fp.slab_size@ / fp.sector_size@);
        lemma_span_pos(fp.sector_size@, ord(fp.slab_size@ / fp.sector_size@));
        proof_assert! { forall<p: Int> 0 <= p && p < i ==> ds[p].start_offset != ds[i].start_offset };
        proof_assert! { forall<p: Int> 0 <= p && p < i ==>
            (ds[i].start_offset@ - base < ds[p].start_offset@ - base ==> {
                lemma_aligned_disj2(ds[i].start_offset@ - base, ds[p].start_offset@ - base, dsk(fp));
                disj(ds[i].start_offset@ - base, dsk(fp), ds[p].start_offset@ - base, dsk(fp)) })
            && (ds[p].start_offset@ - base < ds[i].start_offset@ - base ==> {
                lemma_aligned_disj2(ds[p].start_offset@ - base, ds[i].start_offset@ - base, dsk(fp));
                disj(ds[i].start_offset@ - base, dsk(fp), ds[p].start_offset@ - base, dsk(fp)) }) };
        proof_assert! { off_descs(ds, i, base, fp, ds[i].start_offset@ - base) };
        lemma_rb1_elim(bpre, b0, ds, i, base, size, fp);
        proof_assert! { dgeo(ds[i], base, size, fp) && dsk(fp) == span(fp.sector_size@, ord(fp.slab_size@ / fp.sector_size@)) };
        proof_assert! { inblk(bpre.free_lists@, fp.sector_size@, ord(fp.slab_size@ / fp.sector_size@), ds[i].start_offset@ - base) };
        proof_assert! { fdisj(bpre.free_lists@, fp.sector_size@) && ord(fp.slab_size@ / fp.sector_size@) >= 0 }
    }
}

/// rb1's step over the descriptor geometry only (lemma_rb_step with dgeo for desc_ok).
#[logic]
#[requires(p2(fp.sector_size@) && starts_distinct(ds) && rb1(bpre, b0, ds, i, base, size, fp))]
#[requires(0 <= i && i < ds.len())]
#[requires(forall<p: Int> 0 <= p && p < ds.len() ==> dgeo(ds[p], base, size, fp))]
#[requires(bpre.sector_size == fp.sector_size && bpost.sector_size == fp.sector_size && fp.sector_size@ > 0 && fp.slab_size@ > 0)]
#[requires(mk_spec(bpre.free_lists@, bpost.free_lists@, fp.sector_size@, ds[i].start_offset@ - base, ord(ds[i].slab_size@ / fp.sector_size@)))]
#[requires(bd_aligned(bpost))]
#[ensures(rb1(bpost, b0, ds, i + 1, base, size, fp))]
pub fn lemma_rb_step_g(bpre: BuddyAllocator, bpost: BuddyAllocator, b0: BuddyAllocator, ds: Seq<SlabDescriptor>, i: Int, base: Int, size: Int, fp: FormatParams) {
    pearlite! {
        lemma_ord_ge(fp.slab_size@ / fp.sector_size@);
        lemma_span_pos(fp.sector_size@, ord(fp.slab_size@ / fp.sector_size@));
        proof_assert! { ds[i].slab_size == fp.slab_size };
        lemma_pre_inblk(bpre, b0, ds, i, base, size, fp);
        lemma_rb1_elim(bpre, b0, ds, i, base, size, fp);
        lemma_mk_spec_elim(bpre.free_lists@, bpost.free_lists@, fp.sector_size@, ds[i].start_offset@ - base, ord(fp.slab_size@ / fp.sector_size@));
        proof_assert! { mk_post(bpre.free_lists@, bpost.free_lists@, fp.sector_size@, ds[i].start_offset@ - base, ord(fp.slab_size@ / fp.sector_size@)) };
        proof_assert! { dsk(fp) == span(fp.sector_size@, ord(fp.slab_size@ / fp.sector_size@)) && dsk(fp) > 0 };
        proof_assert! { forall<o: Int, k: Int> fv(bpost.free_lists@, o, k) ==>
            fs(bpost.free_lists@, o, k) % span(fp.sector_size@, o) == 0 && fs(bpost.free_lists@, o, k) >= 0 };
        proof_assert! { forall<t: Int> 0 <= t && t % dsk(fp) == 0 && t + dsk(fp) <= size && off_descs(ds, i + 1, base, fp, t) ==> {
            lemma_cov_mark(bpre.free_lists@, bpost.free_lists@, fp.sector_size@, ord(fp.slab_size@ / fp.sector_size@), ds[i].start_offset@ - base, t);
            inblk(bpost.free_lists@, fp.sector_size@, ord(fp.slab_size@ / fp.sector_size@), t) } };
        proof_assert! { forall<p: Int> 0 <= p && p < i ==>
            faway(bpost.free_lists@, fp.sector_size@, ds[p].start_offset@ - base, dsk(fp)) };
        proof_assert! { faway(bpost.free_lists@, fp.sector_size@, ds[i].start_offset@ - base, dsk(fp)) };
        proof_assert! { forall<p: Int> 0 <= p && p < i + 1 ==>
            faway(bpost.free_lists@, fp.sector_size@, ds[p].start_offset@ - base, dsk(fp)) };
        proof_assert! { tf(bpost.free_lists@, fp.sector_size@, bpost.free_lists@.len())
            == tf(b0.free_lists@, fp.sector_size@, b0.free_lists@.len()) - (i + 1) * dsk(fp) };
        lemma_rb1_intro(bpost, b0, ds, i + 1, base, size, fp)
    }
}

/// Non-overlapping descriptors of a positive slab size start at distinct offsets.
#[logic]
#[requires(descs_sane(ds, base, size, fp))]
#[requires(a_7cb7c3(fp.slab_size@, fp.sector_size@) && fp.slab_size@ % fp.sector_size@ == 0)]
#[ensures(starts_distinct(ds))]
#[ensures(forall<p: Int> 0 <= p && p < ds.len() ==> dgeo(ds[p], base, size, fp))]
pub fn lemma_sane_distinct(ds: Seq<SlabDescriptor>, base: Int, size: Int, fp: FormatParams) {
    pearlite! {
        lemma_slab_span(fp.slab_size@, fp.sector_size@);
        proof_assert! { dsk(fp) == fp.slab_size@ };
        lemma_starts_distinct(ds, base, size)
    }
}

/// initialize()'s buddy rebuild for one region (lib.rs:552-555: `BuddyAllocator::new` then
/// `mark_allocated(desc.start_offset, desc.slab_size)` for every recovered descriptor, in
/// order), with every mark made through `mark_checked`: its precondition — the statement —
/// is proved at every call. Afterwards no recovered range is free.
#[requires(p2(fp.sector_size@))]
#[requires(rg_in(base@, size@, fp))]
#[requires(descs_sane(descs@, base@, size@, fp))]
#[ensures(result.base_offset == base && result.total_usable_size == size && result.sector_size == fp.sector_size)]
#[ensures(forall<p: Int> 0 <= p && p < descs@.len() ==>
    faway(result.free_lists@, fp.sector_size@, descs@[p].start_offset@ - base@, dsk(fp)))]
pub fn rebuild_marks(base: u64, size: u64, fp: FormatParams, descs: &Vec<SlabDescriptor>) -> BuddyAllocator {
    proof_assert! { lemma_rg_in(base@, size@, fp); rg_hd(base@, size@, fp) };
    proof_assert! { lemma_sane_distinct(descs@, base@, size@, fp); starts_distinct(descs@) };
    let mut buddy = BuddyAllocator::new(base, size, fp.sector_size);
    let b0 = snapshot! { buddy };
    g_rb_init(b0, snapshot! { descs@ }, snapshot! { base@ }, snapshot! { size@ }, snapshot! { fp });
    let mut i: usize = 0;
    #[invariant(i@ <= descs@.len())]
    #[invariant(bd_inv(buddy) && buddy.base_offset == base && buddy.total_usable_size == size)]
    #[invariant(buddy.sector_size == fp.sector_size && buddy.max_order == b0.max_order)]
    #[invariant(rb1(buddy, *b0, descs@, i@, base@, size@, fp))]
    while i < descs.len() {
        proof_assert! { lemma_sane_distinct(descs@, base@, size@, fp); forall<p: Int> 0 <= p && p < descs@.len() ==> dgeo(descs@[p], base@, size@, fp) };
        proof_assert! { lemma_dgeo_k(descs@[i@], *b0, base@, size@, fp); ord(descs@[i@].slab_size@ / buddy.sector_size@) <= buddy.max_order@ };
        proof_assert! { lemma_pre_inblk(buddy, *b0, descs@, i@, base@, size@, fp);
            lemma_inblk_free(buddy.free_lists@, fp.sector_size@, ord(fp.slab_size@ / fp.sector_size@), descs@[i@].start_offset@ - base@);
            mark_pre_ok(buddy, descs@[i@].start_offset@, descs@[i@].slab_size@) };
        let bpre = snapshot! { buddy };
        mark_checked(&mut buddy, descs[i].start_offset, descs[i].slab_size);
        proof_assert! { lemma_rb_step_g(*bpre, buddy, *b0, descs@, i@, base@, size@, fp); rb1(buddy, *b0, descs@, i@ + 1, base@, size@, fp) };
        i += 1;
    }
    proof_assert! { lemma_rb1_elim(buddy, *b0, descs@, descs@.len(), base@, size@, fp); true };
    buddy
}

/// **EM-BUDDY-MARK-ALLOCATED-PRE** — "A range is only marked allocated when it lies in the
/// region and is currently entirely free." LEVEL-2 under D-RANGE-FMT-CHECKPOINT-PAYLOAD-980f61
/// (the recovered payload is one this component serialised for this superblock: sane,
/// non-overlapping descriptors on buddy blocks inside their region) AND D-RANGE-FR-002-217ace
/// (power-of-two sector size: the batch-A buddy theory — mask_fact, lemma_cov_mark — that a
/// not-yet-marked aligned block lies inside one free block). In initialize()'s rebuild of
/// EVERY region from EVERY descriptor list in range (lib.rs:552-555, `rebuild_marks`), every
/// call mark_allocated(desc.start_offset, desc.slab_size) is made through `mark_checked`, whose
/// precondition `mark_pre_ok` is the statement (the block lies in the region and every byte of
/// it is free) — proved at every call; afterwards no recovered range is free.
#[requires(p2(fp.sector_size@))]
#[requires(rg_in(base@, size@, fp))]
#[requires(descs_sane(descs@, base@, size@, fp))]
#[ensures(forall<p: Int> 0 <= p && p < descs@.len() ==>
    faway(result.free_lists@, fp.sector_size@, descs@[p].start_offset@ - base@, dsk(fp)))]
pub fn verify_em_buddy_mark_allocated_pre(base: u64, size: u64, fp: FormatParams, descs: &Vec<SlabDescriptor>) -> BuddyAllocator {
    proof_assert! { lemma_rg_in(base@, size@, fp); rg_hd(base@, size@, fp) };
    rebuild_marks(base, size, fp, descs)
}

/// Mutant: same body and requires; claims the first recovered range is still (partly) free.
#[requires(p2(fp.sector_size@))]
#[requires(rg_in(base@, size@, fp))]
#[requires(descs_sane(descs@, base@, size@, fp))]
#[ensures(descs@.len() > 0 ==> !faway(result.free_lists@, fp.sector_size@, descs@[0].start_offset@ - base@, dsk(fp)))]
pub fn verify_em_buddy_mark_allocated_pre__mutant(base: u64, size: u64, fp: FormatParams, descs: &Vec<SlabDescriptor>) -> BuddyAllocator {
    proof_assert! { lemma_rg_in(base@, size@, fp); rg_hd(base@, size@, fp) };
    rebuild_marks(base, size, fp, descs)
}

// =============================================================================
// EM-INV-CHECKPOINT-ROUNDTRIP — REFUTED INSIDE D-RANGE-FMT-CHECKPOINT-PAYLOAD-e0fe79
//   (root cause CKPT-LEN-U32, checkpoint.rs:82)
// =============================================================================

/// FR-002-valid geometry inside the assumption: sector 1, slab_size N = 2^29 - 4 (so
/// slab_size / sector_size = N <= u32::MAX), max extent 1, one region of 2^29 bytes; format's
/// region (lib.rs:465-466) after reserve_extent(k, 1): ONE slab at 0 with element size 1 and
/// N slots (no truncation: N < 2^32).
#[ensures(rg_inv(result.0) && one_key(result.0, 0))]
#[ensures(result.0.slabs@.lookup(0).bitmap.num_slots@ == 536870908)]
#[ensures(result.0.slabs@.lookup(0).slab_size@ == 536870908 && result.0.slabs@.lookup(0).element_size@ == 1)]
#[ensures(result.1.slab_size@ / result.1.sector_size@ <= u32::MAX@)]
#[ensures(result.1.sector_size@ == 1 && result.1.slab_size@ == 536870908 && result.1.region_count@ == 1)]
pub fn w_ckl_region() -> (RegionState, FormatParams) {
    let fp = FormatParams {
        data_disk_size: 536870912,
        slab_size: 536870908,
        max_extent_size: 1,
        sector_size: 1,
        region_count: 1,
        metadata_alignment: 0,
        instance_id: Some(1),
        metadata_disk_ns_id: 1,
        metadata_region_size: 0,
    };
    proof_assert! { fp.slab_size@ % fp.sector_size@ == 0 && fp.max_extent_size@ <= fp.slab_size@ };
    let b = BuddyAllocator::new(0, 536870912, 1);
    proof_assert! { 29.pow2() == 536870912 && 536870912 / 1 == 536870912 };
    proof_assert! { b.max_order@ == 29 && b.free_lists@[29]@ == Seq::singleton(0u64) };
    let mut r = RegionState::new(b, fp);
    proof_assert! { span(1, 29) == 536870912 };
    proof_assert! { rg_inv(r) };
    proof_assert! { 28.pow2() == 268435456 };
    proof_assert! { lemma_ord_eq(536870908, 0, 29); ord(536870908 / 1) == 29 };
    proof_assert! { first_ne(r.buddy.free_lists@, 29, 29) == 29 };
    proof_assert! { align_l(1, 1) == 1 && slots_of(536870908, 1) == 536870908 };
    let old = snapshot! { r };
    let res = r.alloc_extent(1);
    proof_assert! { fresh_case(*old, r, 1, res) };
    proof_assert! { match res { Ok((d, _, _)) => d@ == 0, Err(_) => false } };
    proof_assert! { r.slabs@.ext_eq(old.slabs@.insert(0, r.slabs@.lookup(0))) };
    (r, fp)
}

/// write_checkpoint's payload (checkpoint.rs:48-55) for that one-region component: the
/// region count (u32 LE) then serialize_region. Its length is 4 + 28 + 8 * N = 2^32.
#[ensures(result.0@.len() == 4294967296)]
#[ensures(result.2.slab_size@ / result.2.sector_size@ <= u32::MAX@)]
#[ensures(rg_inv(result.1) && one_key(result.1, 0) && result.1.slabs@.lookup(0).bitmap.num_slots@ == 536870908)]
pub fn w_ckl_payload() -> (Vec<u8>, RegionState, FormatParams) {
    let (r, fp) = w_ckl_region();
    let mut payload: Vec<u8> = Vec::new();
    let rc = to_le_bytes32(1u32);
    append(&mut payload, &rc);
    proof_assert! { exists<k: Int> one_key(r, k) };
    let data = serialize_region(&r);
    proof_assert! { data@.len() == 28 + 8 * 536870908 };
    append(&mut payload, &data);
    (payload, r, fp)
}

/// Copy write + read back of a payload of exactly 2^32 bytes (batch 4's CKPT-LEN-U32 steps):
/// the write succeeds, the read yields CorruptMetadata or an empty payload.
#[requires(ss@ >= 16 && region_size@ <= area@.len() && region_size@ % ss@ == 0 && area@.len() + ss@ <= usize::MAX@)]
#[requires(16 + payload@.len() + ss@ <= usize::MAX@)]
#[requires(payload@.len() == 4294967296 && 16 + payload@.len() <= region_size@)]
#[ensures(result.0 == Ok(()))]
#[ensures(match result.1 { Ok(v) => v@.len() == 0, Err(e) => e == EmError::CorruptMetadata })]
pub fn w_ckl_copy(area: &mut Vec<u8>, ss: usize, region_size: u64, seq: u64, payload: &Vec<u8>)
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

/// LEVEL-3 RETIRED (support only, not a refutation): the 2^32-byte payload below lies OUTSIDE
/// D-RANGE-FMT-CHECKPOINT-REGION-HEADER-87044b (`payload.len() <= u32::MAX`), a declared range
/// whose methods [checkpoint, initialize] cover EM-INV-CHECKPOINT-ROUNDTRIP; inside the range
/// the CKPT-LEN-U32 truncation cannot occur. Kept as the documented out-of-range behaviour.
/// (Level-2 text:) **EM-INV-CHECKPOINT-ROUNDTRIP** — REFUTED INSIDE THE RANGE (LEVEL-2: the assumption
/// D-RANGE-FMT-CHECKPOINT-PAYLOAD-e0fe79, slab_size / sector_size <= u32::MAX, HOLDS: N =
/// 2^29 - 4). "... decoding any checkpoint copy the component has written yields exactly those
/// per-region slab descriptors and key lists ..." ROOT CAUSE = CKPT-LEN-U32 (batch 4):
/// write_checkpoint checks the FULL payload length against the copy region
/// (checkpoint.rs:69-75) but stores `payload.len() as u32` (checkpoint.rs:82), and
/// read_checkpoint_region trusts that field (checkpoint.rs:133-172). The component's payload
/// for the in-range state `w_ckl_region` (format: sector 1, slab 2^29 - 4, one 2^29-byte
/// region; one reserve_extent(k, 1)) is exactly 2^32 bytes; for EVERY copy region that fits it
/// the write succeeds, and reading the copy back with its own seq yields CorruptMetadata or
/// an EMPTY payload, whose decoding (deserialize_slabs, checkpoint.rs:186-188) is
/// CorruptMetadata — never the slab descriptor that was written.
#[requires(ss@ >= 16 && region_size@ <= area@.len() && region_size@ % ss@ == 0 && area@.len() + ss@ <= usize::MAX@)]
#[requires(16 + 4294967296 + ss@ <= usize::MAX@)]
#[requires(16 + 4294967296 <= region_size@)]
#[ensures(result.0.slab_size@ / result.0.sector_size@ <= u32::MAX@)]
#[ensures(result.1 == Ok(()))]
#[ensures(match result.2 { Ok(_) => false, Err(e) => e == EmError::CorruptMetadata })]
pub fn support_old_refute_em_inv_checkpoint_roundtrip_len32(area: &mut Vec<u8>, ss: usize, region_size: u64, seq: u64)
    -> (FormatParams, Result<(), EmError>, Result<Vec<Vec<SlabDescriptor>>, EmError>) {
    let (payload, _r, fp) = w_ckl_payload();
    let (w, r) = w_ckl_copy(area, ss, region_size, seq, &payload);
    match r {
        Ok(v) => {
            proof_assert! { !ck_complete(v@) };
            let d = deser7(&v);
            (fp, w, d)
        }
        Err(e) => (fp, w, Err(e)),
    }
}

/// Mutant: same body and requires; claims the copy decodes (to the written descriptors).
#[requires(ss@ >= 16 && region_size@ <= area@.len() && region_size@ % ss@ == 0 && area@.len() + ss@ <= usize::MAX@)]
#[requires(16 + 4294967296 + ss@ <= usize::MAX@)]
#[requires(16 + 4294967296 <= region_size@)]
#[ensures(match result.2 { Ok(_) => true, Err(_) => false })]
pub fn support_old_refute_em_inv_checkpoint_roundtrip_len32__mutant(area: &mut Vec<u8>, ss: usize, region_size: u64, seq: u64)
    -> (FormatParams, Result<(), EmError>, Result<Vec<Vec<SlabDescriptor>>, EmError>) {
    let (payload, _r, fp) = w_ckl_payload();
    let (w, r) = w_ckl_copy(area, ss, region_size, seq, &payload);
    match r {
        Ok(v) => {
            proof_assert! { !ck_complete(v@) };
            let d = deser7(&v);
            (fp, w, d)
        }
        Err(e) => (fp, w, Err(e)),
    }
}
