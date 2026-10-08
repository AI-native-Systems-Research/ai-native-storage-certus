//! LEVEL-3 (phase D-A, 2026-10-07): REACHABLE-START predicates for the refutation witnesses.
//!
//! A `refute_*` counts only if its concrete starting state satisfies the component's PROVED
//! invariants (`verif-l3/.run/l3/invariants.yaml`) and every declared range of
//! `level2_assumptions` listed for the property's methods. The witnesses `proof_assert!`
//! these predicates on their concrete start; nothing here is assumed.
use crate::model::assume::*;
use crate::model::component::*;
use crate::props::batch1::{apply_cop, cop_pre, em_ok, COp};
use crate::model::l2m::coal;
use crate::model::params::*;
use crate::model::region::*;
use crate::props::l2a::lg;
use creusot_std::prelude::*;

/// The PROVED region invariants on a starting region `r` whose live write handles are `hs`:
/// `lg` (= rg_good: rg_inv ⊇ bd_inv / slab_inv, p2, fdisj, sl_away, rg_acct, sc_ok, rg_ka,
/// pv(pending_frees); + pv(hs), handles not queued), `nonfull_listed`, and the buddy-level
/// `coal` of the free lists.
#[logic(open)]
pub fn rg_l3(r: RegionState, hs: Seq<(u64, usize)>) -> bool {
    pearlite! { lg(r, hs) && nonfull_listed(r) && coal(r.buddy.free_lists@, r.buddy.sector_size@) }
}

/// Every declared range a FORMAT start evaluates, for parameters `fp` on a metadata device
/// of `n` sectors of `ms` bytes (metadata_base_lba 0, the default), with `sb` the superblock
/// format writes (lib.rs:418-494: checkpoint offset `ckoff_l`, size `cksize_l`, data start
/// `ds_l`; seq / active as given) — 217ace, 7cb7c3, e0fe79, 3b58ea, 2ee144, 67ea4f, c6d2c5,
/// 84618e, dfd6b0, 1d780b on the format inputs and 91a1f6, 23f855, 0de740, 9c0757, a23fc2 on
/// the superblock (what initialize/checkpoint later read), plus the superblock parity
/// `active_copy == checkpoint_seq % 2` (true of every superblock the component writes).
#[logic(open)]
pub fn l3_fmt_ranges(fp: FormatParams, n: Int, ms: Int, sb: Superblock) -> bool {
    pearlite! {
        a_217ace(fp.sector_size@) && a_7cb7c3(fp.slab_size@, fp.sector_size@)
        && a_e0fe79(fp.slab_size@, fp.sector_size@) && a_3b58ea(fp.data_disk_size@, fp.sector_size@)
        && a_2ee144(fp.metadata_alignment@) && a_67ea4f(n, ms) && a_c6d2c5(n * ms, fp)
        && sb.data_disk_size == fp.data_disk_size && sb.sector_size == fp.sector_size
        && sb.slab_size == fp.slab_size && sb.max_extent_size == fp.max_extent_size
        && sb.region_count == fp.region_count
        && sb.checkpoint_region_offset@ == ckoff_l(fp.metadata_alignment@)
        && sb.checkpoint_region_size@ == cksize_l(n * ms, fp.metadata_region_size@, fp.metadata_alignment@, fp.sector_size@)
        && sb.data_start_offset@ == ds_l(n * ms, fp.metadata_region_size@, fp.metadata_alignment@, fp.sector_size@)
        && a_84618e(fp.data_disk_size@, sb.data_start_offset@, fp.region_count@)
        && a_dfd6b0(ms, sb.checkpoint_region_offset@, sb.checkpoint_region_size@)
        && a_1d780b(0, ms, sb.checkpoint_region_offset@, sb.checkpoint_region_size@, n * ms)
        && a_91a1f6(sb) && a_23f855(sb) && a_0de740(sb.active_copy@) && a_9c0757(sb.checkpoint_seq@)
        && a_a23fc2(sb.checkpoint_region_offset@, sb.checkpoint_region_size@)
        && sb.active_copy@ == sb.checkpoint_seq@ % 2
    }
}

/// The reserve_extent ranges 231ab0, 3783f8, a27ede for a request of `size` bytes.
#[logic(open)]
pub fn l3_rsv_ranges(size: Int, fp: FormatParams) -> bool {
    pearlite! { a_231ab0(size) && a_3783f8(size, fp.sector_size@) && a_a27ede(size, fp.max_extent_size@) }
}

/// The recovered-payload ranges 980f61 (per region list) and 3b612e (per descriptor) for a
/// region [base, base+size) of slab size `slab`.
#[logic(open)]
pub fn l3_rec_ranges(ds: Seq<SlabDescriptor>, base: Int, size: Int, slab: Int) -> bool {
    pearlite! {
        a_980f61_list(ds, base, size)
        && forall<p: Int> 0 <= p && p < ds.len() ==> a_3b612e_d(ds[p], base, size, slab)
    }
}

/// Every declared range a FORMAT start evaluates on the inputs of `em.format(p)`: the
/// parameter ranges 217ace, 7cb7c3, e0fe79, 3b58ea, 2ee144, the metadata-device ranges
/// 67ea4f, c6d2c5, 84618e, dfd6b0 and 1d780b (`ms` = the metadata device's sector size,
/// base LBA = `em.metadata_base_lba`).
#[logic(open)]
pub fn l3_em_fmt_inputs(em: ExtentManager, p: FormatParams) -> bool {
    pearlite! {
        a_217ace(p.sector_size@) && a_7cb7c3(p.slab_size@, p.sector_size@)
        && a_e0fe79(p.slab_size@, p.sector_size@) && a_3b58ea(p.data_disk_size@, p.sector_size@)
        && a_2ee144(p.metadata_alignment@) && a_67ea4f(em.dev.num_sectors@, em.dev.sector_size@)
        && a_c6d2c5(em.dev.num_sectors@ * em.dev.sector_size@, p)
        && a_84618e(p.data_disk_size@, ds_l(em.dev.num_sectors@ * em.dev.sector_size@, p.metadata_region_size@, p.metadata_alignment@, p.sector_size@), p.region_count@)
        && a_dfd6b0(em.dev.sector_size@, ckoff_l(p.metadata_alignment@),
            cksize_l(em.dev.num_sectors@ * em.dev.sector_size@, p.metadata_region_size@, p.metadata_alignment@, p.sector_size@))
        && a_1d780b(em.metadata_base_lba@, em.dev.sector_size@, ckoff_l(p.metadata_alignment@),
            cksize_l(em.dev.num_sectors@ * em.dev.sector_size@, p.metadata_region_size@, p.metadata_alignment@, p.sector_size@),
            em.dev.num_sectors@ * em.dev.sector_size@)
    }
}

// ============================================================ D2: rg_built as a PROVED invariant

/// LEVEL-3 (phase D-A) PROVED INVARIANT: every current region's own parameters are the
/// component's format parameters, and its geometry satisfies [`rg_built`] for them
/// (`slab_size % sector_size == 0` and `base + size <= data_disk_size`). Established by format
/// (FR-002 check lib.rs:391; em_layout + superblock.data_disk_size == params.data_disk_size)
/// and by initialize (91a1f6 inside sb_sane; em_layout; format_params rebuilt from the
/// superblock, lib.rs:521-532), preserved by every operation (rg_frame keeps each region's
/// base / size / format_params, shared_frame keeps format_params; set_* touch neither).
/// The `rg_built(base, size, fp)` premise of the isolated-region drivers (inside `rg_in`) is
/// this invariant instantiated at a current region (`lemma_em_rg_built_inst`).
#[logic(open)]
pub fn em_rg_built(em: ExtentManager) -> bool {
    pearlite! {
        match (em.regions, em.shared) {
            (Some(rv), Some(sh)) => forall<i: Int> 0 <= i && i < rv@.len() ==>
                em.arena@[rv@[i]@].format_params == sh.format_params
                && rg_built(em.arena@[rv@[i]@].buddy.base_offset@, em.arena@[rv@[i]@].buddy.total_usable_size@, sh.format_params),
            _ => true,
        }
    }
}

/// The instance the isolated-region drivers use: a current region of a component satisfying
/// the invariant satisfies `rg_built` for its own geometry and parameters.
#[logic]
#[requires(em_rg_built(em))]
#[requires(em.shared != None)]
#[requires(match em.regions { Some(rv) => 0 <= i && i < rv@.len(), None => false })]
#[ensures(match em.regions { Some(rv) => rg_built(em.arena@[rv@[i]@].buddy.base_offset@,
    em.arena@[rv@[i]@].buddy.total_usable_size@, em.arena@[rv@[i]@].format_params), None => false })]
pub fn lemma_em_rg_built_inst(em: ExtentManager, i: Int) {}

/// `em_rg_built`: BASE (format `f`, initialize `g`, both from any well-formed state) and STEP
/// (every component operation `op` via apply_cop, batch1.rs). A failed format leaves regions
/// and shared unchanged (and the old arena prefix), so the invariant carries over.
#[requires(em_regions_wf(*f) && em_rg_built(*f))]
#[requires(f.dev.connected ==> a_67ea4f(f.dev.num_sectors@, f.dev.sector_size@))]
#[requires(a_2ee144(params.metadata_alignment@))]
#[requires(sane_params(params))]
#[requires(em_regions_wf(*g) && sb_sane(sb) && recovered_ok(sb, per_region@))]
#[requires(em_ok(*e) && cop_pre(*e, op) && em_rg_built(*e))]
#[ensures(em_rg_built(^f))]
#[ensures(em_rg_built(^g))]
#[ensures(em_rg_built(^e))]
pub fn inv_em_rg_built(
    f: &mut ExtentManager, params: FormatParams,
    g: &mut ExtentManager, sb: Superblock, per_region: &Vec<Vec<SlabDescriptor>>,
    e: &mut ExtentManager, op: COp,
) -> Result<(), EmError> {
    let r = f.format(params);
    g.initialize(sb, per_region);
    proof_assert! { a_91a1f6(sb) };
    apply_cop(e, op);
    r
}

/// Mutant: claims an operation breaks the invariant.
#[requires(em_ok(*e) && cop_pre(*e, op) && em_rg_built(*e))]
#[requires(e.regions != None && e.shared != None)]
#[ensures(!em_rg_built(^e))]
pub fn inv_em_rg_built__mutant(e: &mut ExtentManager, op: COp) {
    apply_cop(e, op);
}

// ============================================================ D-B1: em_together, em_cap_ok as PROVED invariants

/// LEVEL-3 (phase D-B1) PROVED INVARIANT of the atomic-call component model: the region list
/// and the shared state are published together (`(regions == None) == (shared == None)`).
/// BASE: `new` (both None), `format` (Ok: both Some; Err: both unchanged), `initialize`
/// (both Some); STEP: every operation (apply_cop keeps `regions` and, by shared_frame, the
/// Some/None shape of `shared`). This is the invariant of the model in which every call is
/// one atomic step; the real format/initialize publish the two under two locks
/// (lib.rs:506-507, 576-577), and a concurrent observer inside that window is the REFUTED
/// EM-INV-INITIALIZED-TOGETHER — outside this model.
#[requires(em_regions_wf(*f) && em_together(*f))]
#[requires(f.dev.connected ==> a_67ea4f(f.dev.num_sectors@, f.dev.sector_size@))]
#[requires(a_2ee144(params.metadata_alignment@))]
#[requires(sane_params(params))]
#[requires(em_regions_wf(*g) && sb_sane(sb) && recovered_ok(sb, per_region@))]
#[requires(em_ok(*e) && cop_pre(*e, op) && em_together(*e))]
#[ensures(em_together(result.1))]
#[ensures(em_together(^f))]
#[ensures(em_together(^g))]
#[ensures(em_together(^e))]
pub fn inv_em_together(
    dev: MetaDevice,
    f: &mut ExtentManager, params: FormatParams,
    g: &mut ExtentManager, sb: Superblock, per_region: &Vec<Vec<SlabDescriptor>>,
    e: &mut ExtentManager, op: COp,
) -> (Result<(), EmError>, ExtentManager) {
    let n = crate::model::b3::new_inner(dev);
    let r = f.format(params);
    g.initialize(sb, per_region);
    apply_cop(e, op);
    (r, n)
}

/// Mutant: claims an operation breaks the invariant.
#[requires(em_ok(*e) && cop_pre(*e, op) && em_together(*e))]
#[requires(e.regions != None)]
#[ensures(!em_together(^e))]
pub fn inv_em_together__mutant(e: &mut ExtentManager, op: COp) {
    apply_cop(e, op);
}

/// The component's capacity (the sum of the current regions' sizes, capacity_bytes
/// lib.rs:710-715) fits u64.
#[logic(open)]
pub fn em_cap_ok(em: ExtentManager) -> bool {
    pearlite! { match em.regions { Some(rv) => cap_sum(em.arena@, rv@, rv@.len()) <= u64::MAX@, None => true } }
}

/// The uniform partition (rbase / rsize, lib.rs:458-466 and 538-545) sums to the usable size.
#[logic]
#[variant(k)]
#[requires(0 <= k && k <= rv.len() && rv.len() == n && n > 0 && usable >= 0)]
#[requires(forall<i: Int> 0 <= i && i < rv.len() ==> arena[rv[i]@].buddy.total_usable_size@ == rsize(usable, n, i))]
#[ensures(k < n ==> cap_sum(arena, rv, k) == k * (usable / n))]
#[ensures(k == n ==> cap_sum(arena, rv, k) == usable)]
pub fn lemma_cap_part(arena: Seq<RegionState>, rv: Seq<usize>, n: Int, usable: Int, k: Int) {
    if k > 0 {
        lemma_cap_part(arena, rv, n, usable, k - 1)
    }
}

/// The whole uniform partition sums to the usable size.
#[logic]
#[requires(rv.len() == n && n > 0 && usable >= 0)]
#[requires(forall<i: Int> 0 <= i && i < rv.len() ==> arena[rv[i]@].buddy.total_usable_size@ == rsize(usable, n, i))]
#[ensures(cap_sum(arena, rv, rv.len()) == usable)]
pub fn lemma_cap_full(arena: Seq<RegionState>, rv: Seq<usize>, n: Int, usable: Int) {
    lemma_cap_part(arena, rv, n, usable, rv.len())
}

/// LEVEL-3 (phase D-B1) PROVED INVARIANT `em_cap_ok`. BASE: `new` (no regions), `format` (Ok:
/// the regions are the uniform partition of data_disk_size - data_start_offset, so the sum is
/// that difference; Err: regions unchanged), `initialize` (the same partition of the recovered
/// superblock, data_start_offset <= data_disk_size by 91a1f6 inside sb_sane); STEP: every
/// operation (regions unchanged, rg_frame keeps every size: lemma_cap_frame).
#[requires(em_regions_wf(*f) && em_cap_ok(*f))]
#[requires(f.dev.connected ==> a_67ea4f(f.dev.num_sectors@, f.dev.sector_size@))]
#[requires(a_2ee144(params.metadata_alignment@))]
#[requires(sane_params(params))]
#[requires(em_regions_wf(*g) && sb_sane(sb) && recovered_ok(sb, per_region@))]
#[requires(em_ok(*e) && cop_pre(*e, op) && em_cap_ok(*e))]
#[ensures(em_cap_ok(result.1))]
#[ensures(em_cap_ok(^f))]
#[ensures(em_cap_ok(^g))]
#[ensures(em_cap_ok(^e))]
pub fn inv_em_cap_ok(
    dev: MetaDevice,
    f: &mut ExtentManager, params: FormatParams,
    g: &mut ExtentManager, sb: Superblock, per_region: &Vec<Vec<SlabDescriptor>>,
    e: &mut ExtentManager, op: COp,
) -> (Result<(), EmError>, ExtentManager) {
    let n = crate::model::b3::new_inner(dev);
    let r = f.format(params);
    if r.is_ok() {
        proof_assert! { match (f.regions, f.shared) {
            (Some(rv), Some(sh)) => rv@.len() == params.region_count@ && params.region_count@ > 0
                && sh.superblock.data_start_offset@ < params.data_disk_size@
                && (forall<i: Int> 0 <= i && i < rv@.len() ==>
                    f.arena@[rv@[i]@].buddy.total_usable_size@
                        == rsize(params.data_disk_size@ - sh.superblock.data_start_offset@, params.region_count@, i)),
            _ => false,
        } };
        proof_assert! { match (f.regions, f.shared) {
            (Some(rv), Some(sh)) => {
                lemma_cap_full(f.arena@, rv@, params.region_count@, params.data_disk_size@ - sh.superblock.data_start_offset@);
                cap_sum(f.arena@, rv@, rv@.len()) == params.data_disk_size@ - sh.superblock.data_start_offset@
            },
            _ => false,
        } };
    }
    g.initialize(sb, per_region);
    proof_assert! { a_91a1f6(sb) && sb.data_start_offset@ <= sb.data_disk_size@ };
    proof_assert! { lemma_p2_pos(sb.region_count@); sb.region_count@ > 0 };
    proof_assert! { match g.regions {
        Some(rv) => rv@.len() == sb.region_count@ && forall<i: Int> 0 <= i && i < rv@.len() ==>
            g.arena@[rv@[i]@].buddy.total_usable_size@
                == rsize(sb.data_disk_size@ - sb.data_start_offset@, sb.region_count@, i),
        None => true,
    } };
    proof_assert! { match g.regions {
        Some(rv) => {
            lemma_cap_full(g.arena@, rv@, sb.region_count@, sb.data_disk_size@ - sb.data_start_offset@);
            cap_sum(g.arena@, rv@, rv@.len()) == sb.data_disk_size@ - sb.data_start_offset@
        },
        None => true,
    } };
    let old = snapshot! { *e };
    apply_cop(e, op);
    proof_assert! { match e.regions {
        Some(rv) => { lemma_cap_frame(old.arena@, e.arena@, rv@, rv@.len()); true },
        None => true,
    } };
    (r, n)
}

/// Mutant: claims format leaves a capacity that does not fit u64.
#[requires(em_regions_wf(*f) && em_cap_ok(*f))]
#[requires(f.dev.connected ==> a_67ea4f(f.dev.num_sectors@, f.dev.sector_size@))]
#[requires(a_2ee144(params.metadata_alignment@))]
#[requires(sane_params(params))]
#[ensures(result == Ok(()) ==> !em_cap_ok(^f))]
pub fn inv_em_cap_ok__mutant(f: &mut ExtentManager, params: FormatParams) -> Result<(), EmError> {
    f.format(params)
}
