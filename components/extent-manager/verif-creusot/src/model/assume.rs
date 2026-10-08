//! Level-3 NAMED ASSUMPTIONS (phase F, 2026-10-07).
//!
//! One predicate `a_<suffix>` per declared `level2_assumptions` range of
//! `verif-l3/unified_properties.yaml` (D-RANGE-...-<suffix>); its body is a faithful pearlite
//! translation of that entry's `assume_rust`, never stronger. Translation conventions (each is
//! what it takes for the Rust expression to EVALUATE to `true`):
//! * `a / b`, `a % b` panic for `b == 0`, so the predicate states `b != 0` explicitly;
//! * `a - b` on unsigned values panics (debug) for `b > a`, so it states `a >= b`;
//! * `x.checked_op(y).is_some()` is the exact mathematical bound `x op y <= MAX`;
//! * `as u128` arithmetic is exact (Int); a u64 sub-expression evaluated BEFORE the cast must
//!   itself not overflow, so its `<= u64::MAX` bound is stated;
//! * `x.is_power_of_two()` is [`p2`] (`exists k >= 0, 2^k == x`; for a fixed-width x this is
//!   the std definition).
//!
//! Section 2 holds TYPE facts (true of every value of the Rust type; not assumptions).
//! Section 3 holds the DERIVED consequences the shared model uses, each proved by a lemma
//! from section 1/2 predicates. Section 4 holds the UNDECLARED residue: conjuncts the shared
//! model still needs that are neither a declared range nor derivable from one (reported by
//! phase F; NOT assumptions a credited proof may use).
use crate::model::buddy::*;
use crate::model::component::{rbase, rsize, ds_l};
use crate::model::l2::*;
use crate::model::params::*;
use crate::model::region::{SlabDescriptor, desc_ok, starts_distinct};
use crate::model::slab::*;
use creusot_std::prelude::*;

// ============================================================ 1. declared ranges

/// D-RANGE-FR-002-217ace: `params.sector_size.is_power_of_two()`.
#[logic(open)]
pub fn a_217ace(ss: Int) -> bool {
    pearlite! { p2(ss) }
}

/// D-RANGE-FR-002-7cb7c3: `(params.slab_size / params.sector_size as u64).is_power_of_two()`.
#[logic(open)]
pub fn a_7cb7c3(slab: Int, ss: Int) -> bool {
    pearlite! { ss != 0 && p2(slab / ss) }
}

/// D-RANGE-FR-002-c6d2c5 (format on a metadata device of `md` bytes; dso = `ds_l`,
/// lib.rs:420-446): dso, usable and the uniform region size are sector multiples. Stated
/// only where `data_disk_size >= dso` for the two subtraction terms, so it is WEAKER than the
/// Rust expression (which also implies `dds >= dso`, `ss != 0`, `region_count != 0`).
#[logic(open)]
pub fn a_c6d2c5(md: Int, p: FormatParams) -> bool {
    pearlite! {
        ds_l(md, p.metadata_region_size@, p.metadata_alignment@, p.sector_size@) % p.sector_size@ == 0
        && (p.data_disk_size@ >= ds_l(md, p.metadata_region_size@, p.metadata_alignment@, p.sector_size@) ==>
            (p.data_disk_size@ - ds_l(md, p.metadata_region_size@, p.metadata_alignment@, p.sector_size@)) % p.sector_size@ == 0
            && ((p.data_disk_size@ - ds_l(md, p.metadata_region_size@, p.metadata_alignment@, p.sector_size@)) / p.region_count@)
                % p.sector_size@ == 0)
    }
}

/// D-RANGE-FR-002-84618e: `(params.data_disk_size - data_start_offset) / params.region_count as u64 > 0`.
#[logic(open)]
pub fn a_84618e(dds: Int, dso: Int, rc: Int) -> bool {
    pearlite! { dds >= dso && rc != 0 && (dds - dso) / rc > 0 }
}

/// D-RANGE-FMT-METADATA-DEVICE-LAYOUT-2ee144 (corrected 2026-10-07):
/// `params.metadata_alignment <= u64::MAX - SUPERBLOCK_SIZE as u64` (SUPERBLOCK_SIZE = 4096).
#[logic(open)]
pub fn a_2ee144(a: Int) -> bool {
    pearlite! { a <= u64::MAX@ - 4096 }
}

/// D-RANGE-FMT-METADATA-DEVICE-LAYOUT-dfd6b0:
/// `checkpoint_region_offset % ms == 0 && checkpoint_region_size % ms == 0`.
#[logic(open)]
pub fn a_dfd6b0(ms: Int, cro: Int, crs: Int) -> bool {
    pearlite! { ms != 0 && cro % ms == 0 && crs % ms == 0 }
}

/// D-RANGE-FR-036-1d780b: `(base_lba as u128) * (ms as u128)
/// + ((cro + 2 * crs) as u128) <= metadata_disk_size as u128` (the `cro + 2 * crs` sum is u64).
#[logic(open)]
pub fn a_1d780b(base_lba: Int, ms: Int, cro: Int, crs: Int, mds: Int) -> bool {
    pearlite! { cro + 2 * crs <= u64::MAX@ && base_lba * ms + (cro + 2 * crs) <= mds }
}

/// D-RANGE-FMT-CHECKPOINT-PAYLOAD-e0fe79: `params.slab_size / params.sector_size as u64 <= u32::MAX as u64`.
#[logic(open)]
pub fn a_e0fe79(slab: Int, ss: Int) -> bool {
    pearlite! { ss != 0 && slab / ss <= u32::MAX@ }
}

/// D-RANGE-FMT-CHECKPOINT-REGION-HEADER-87044b: `payload.len() <= u32::MAX as usize`.
#[logic(open)]
pub fn a_87044b(plen: Int) -> bool {
    pearlite! { plen <= u32::MAX@ }
}

/// D-RANGE-FR-005-231ab0: `size > 0`.
#[logic(open)]
pub fn a_231ab0(size: Int) -> bool {
    pearlite! { size > 0 }
}

/// D-RANGE-FR-005-3783f8 (corrected 2026-10-07):
/// `size as u64 + format_params.sector_size as u64 <= u32::MAX as u64`.
#[logic(open)]
pub fn a_3783f8(size: Int, ss: Int) -> bool {
    pearlite! { size + ss <= u32::MAX@ }
}

/// D-RANGE-FR-002-a27ede: `size <= format_params.max_extent_size`.
#[logic(open)]
pub fn a_a27ede(size: Int, mes: Int) -> bool {
    pearlite! { size <= mes }
}

/// D-RANGE-FR-002-91a1f6: `sb.sector_size > 0 && sb.slab_size % sb.sector_size as u64 == 0
/// && sb.max_extent_size as u64 <= sb.slab_size && sb.region_count.is_power_of_two()
/// && sb.data_start_offset <= sb.data_disk_size`.
#[logic(open)]
pub fn a_91a1f6(sb: Superblock) -> bool {
    pearlite! {
        sb.sector_size@ > 0 && sb.slab_size@ % sb.sector_size@ == 0
        && sb.max_extent_size@ <= sb.slab_size@ && p2(sb.region_count@)
        && sb.data_start_offset@ <= sb.data_disk_size@
    }
}

/// D-RANGE-KE-SUPERBLOCK-23f855: `sb.version == FORMAT_VERSION`.
#[logic(open)]
pub fn a_23f855(sb: Superblock) -> bool {
    pearlite! { sb.version == FORMAT_VERSION }
}

/// D-RANGE-FMT-SUPERBLOCK-0de740: `sb.active_copy <= 1`.
#[logic(open)]
pub fn a_0de740(active: Int) -> bool {
    pearlite! { active <= 1 }
}

/// D-RANGE-FMT-CHECKPOINT-PAYLOAD-980f61, per descriptor `d` of region i = [base, base+size):
/// `d.element_size > 0 && d.element_size as u64 <= d.slab_size
/// && d.keys.len() as u64 == d.slab_size / d.element_size as u64` and the in-region half of
/// `within_region_disjoint` (d lies in region i's byte range).
#[logic(open)]
pub fn a_980f61_d(d: SlabDescriptor, base: Int, size: Int) -> bool {
    pearlite! {
        d.element_size@ > 0 && d.element_size@ <= d.slab_size@
        && d.keys@.len() == d.slab_size@ / d.element_size@
        && base <= d.start_offset@ && d.start_offset@ + d.slab_size@ <= base + size
    }
}

/// D-RANGE-FMT-CHECKPOINT-PAYLOAD-980f61 for one region's list `ds`: every descriptor
/// satisfies [`a_980f61_d`] and overlaps no other descriptor of the list.
#[logic(open)]
pub fn a_980f61_list(ds: Seq<SlabDescriptor>, base: Int, size: Int) -> bool {
    pearlite! {
        forall<p: Int> 0 <= p && p < ds.len() ==> a_980f61_d(ds[p], base, size)
            && (forall<q: Int> 0 <= q && q < ds.len() && q != p ==>
                disj(ds[p].start_offset@, ds[p].slab_size@, ds[q].start_offset@, ds[q].slab_size@))
    }
}

/// D-RANGE-FMT-CHECKPOINT-PAYLOAD-980f61, whole payload: `regs.len() == sb.region_count` and
/// every list well-formed for its region of the superblock geometry.
#[logic(open)]
pub fn a_980f61(sb: Superblock, regs: Seq<Vec<SlabDescriptor>>) -> bool {
    pearlite! {
        regs.len() == sb.region_count@
        && forall<i: Int> 0 <= i && i < regs.len() ==>
            a_980f61_list(regs[i]@,
                rbase(sb.data_start_offset@, sb.data_disk_size@ - sb.data_start_offset@, sb.region_count@, i),
                rsize(sb.data_disk_size@ - sb.data_start_offset@, sb.region_count@, i))
    }
}

/// D-RANGE-FR-002-3b58ea: `params.data_disk_size.checked_mul(2).and_then(|x| x.checked_add(params.sector_size as u64)).is_some()`.
#[logic(open)]
pub fn a_3b58ea(dds: Int, ss: Int) -> bool {
    pearlite! { 2 * dds + ss <= u64::MAX@ }
}

/// D-RANGE-FR-002-67ea4f: `(metadata_bd.num_sectors(ns)?).checked_mul(metadata_bd.sector_size(ns)? as u64).is_some()`.
#[logic(open)]
pub fn a_67ea4f(n: Int, s: Int) -> bool {
    pearlite! { n * s <= u64::MAX@ }
}

/// D-RANGE-FR-004-a23fc2: `sb.checkpoint_region_offset.checked_add(2 * sb.checkpoint_region_size).is_some()`
/// (the u64 product `2 * crs` must not overflow either; implied by the sum bound).
#[logic(open)]
pub fn a_a23fc2(cro: Int, crs: Int) -> bool {
    pearlite! { cro + 2 * crs <= u64::MAX@ }
}

/// D-RANGE-FR-003-9c0757: `sb.checkpoint_seq < u64::MAX`.
#[logic(open)]
pub fn a_9c0757(seq: Int) -> bool {
    pearlite! { seq < u64::MAX@ }
}

/// D-RANGE-FR-004-3b612e, per descriptor `d` of region i = [base, base+size) with
/// superblock slab size `slab`: `d.slab_size == sb.slab_size
/// && (d.start_offset - region_base(i)) % sb.slab_size == 0 && d.start_offset + sb.slab_size <= region_end(i)`.
#[logic(open)]
pub fn a_3b612e_d(d: SlabDescriptor, base: Int, size: Int, slab: Int) -> bool {
    pearlite! {
        d.slab_size@ == slab && d.start_offset@ >= base && slab != 0
        && (d.start_offset@ - base) % slab == 0 && d.start_offset@ + slab <= base + size
    }
}

// ============================================================ 2. TYPE facts

/// A `sector_size()` answer is a `u32` (checkpoint.rs:91, block_io.rs): the model keeps it
/// in a wider field.
#[logic(open)]
pub fn t_u32(x: Int) -> bool {
    pearlite! { 0 <= x && x <= u32::MAX@ }
}

// ============================================================ 3. DERIVED (proved here)

/// `x.is_power_of_two()` implies `x >= 1`.
#[logic]
#[requires(p2(x))]
#[ensures(x >= 1)]
pub fn lemma_p2_pos(x: Int) {
    pearlite! { proof_assert! { forall<k: Int> 0 <= k && k.pow2() == x ==> { lemma_pow2(k); x >= 1 } } }
}

/// The parameter headroom the shared model derives from 7cb7c3 + e0fe79 + 3b58ea.
#[logic(open)]
pub fn hd_params(slab: Int, ss: Int, dds: Int) -> bool {
    pearlite! { ss > 0 && slab > 0 && slab + ss <= u64::MAX@ && 2 * dds + ss <= u64::MAX@ }
}

/// slab > 0 from 7cb7c3 (`slab / ss >= 1` and `ss > 0`).
#[logic]
#[requires(a_7cb7c3(slab, ss) && 0 <= slab && 0 <= ss)]
#[ensures(ss > 0 && slab >= ss && slab > 0)]
pub fn lemma_7cb7c3_pos(slab: Int, ss: Int) {
    pearlite! {
        lemma_p2_pos(slab / ss);
        lemma_divmod(slab, ss);
        lemma_mul_le(1, slab / ss, ss)
    }
}

/// slab + ss <= u64::MAX from e0fe79 for a u32 sector size.
#[logic]
#[requires(a_e0fe79(slab, ss) && 0 <= slab && t_u32(ss))]
#[ensures(slab + ss <= u64::MAX@)]
pub fn lemma_e0fe79_hd(slab: Int, ss: Int) {
    pearlite! {
        lemma_divmod(slab, ss);
        lemma_mul_le(slab / ss + 2, 4294967297, ss);
        lemma_mul_le(ss, 4294967295, 4294967297);
        proof_assert! { slab + ss <= ss * (slab / ss) + 2 * ss - 1 };
        proof_assert! { ss * (slab / ss) + 2 * ss == (slab / ss + 2) * ss };
        proof_assert! { (slab / ss + 2) * ss <= 4294967297 * ss };
        proof_assert! { 4294967297 * ss <= 4294967297 * 4294967295 }
    }
}

/// The three a_* parameter ranges give the headroom.
#[logic]
#[requires(a_7cb7c3(slab, ss) && a_e0fe79(slab, ss) && a_3b58ea(dds, ss))]
#[requires(0 <= slab && t_u32(ss) && 0 <= dds)]
#[ensures(hd_params(slab, ss, dds))]
pub fn lemma_hd_params(slab: Int, ss: Int, dds: Int) {
    pearlite! { lemma_7cb7c3_pos(slab, ss); lemma_e0fe79_hd(slab, ss) }
}

/// `p2(n)` with `n = 2^j <= 2^64` has buddy order `j`: `2^ord(n) == n`.
#[logic]
#[requires(p2(n) && n <= 18446744073709551616)]
#[ensures(ord(n).pow2() == n)]
pub fn lemma_ord_p2(n: Int) {
    pearlite! {
        proof_assert! { forall<j: Int> 0 <= j && j.pow2() == n ==> {
            lemma_ord_ge(n);
            lemma_pow2_mono(0, j);
            (j > 64 ==> { lemma_pow2_strict(64, j); proof_assert! { 64.pow2() == 18446744073709551616 }; false })
            && (j <= 64 ==> ((j == 0 || { lemma_pow2_strict(j - 1, j); (j - 1).pow2() < n })
                && { lemma_ord_eq(n, 0, j); ord(n) == j }))
        } };
        proof_assert! { exists<j: Int> 0 <= j && j.pow2() == n }
    }
}

/// Under 7cb7c3 and `slab % ss == 0` the slab's buddy block is exactly the slab:
/// `span(ss, ord(slab / ss)) == slab`.
#[logic]
#[requires(a_7cb7c3(slab, ss) && 0 <= slab && t_u32(ss) && slab % ss == 0 && slab <= u64::MAX@)]
#[ensures(span(ss, ord(slab / ss)) == slab)]
pub fn lemma_slab_span(slab: Int, ss: Int) {
    pearlite! {
        lemma_7cb7c3_pos(slab, ss);
        lemma_divmod(slab, ss);
        lemma_div_le_self(slab, ss);
        lemma_ord_p2(slab / ss)
    }
}

/// `x / m <= x` for `m >= 1`.
#[logic]
#[requires(m >= 1 && x >= 0)]
#[ensures(x / m <= x)]
pub fn lemma_div_le_self(x: Int, m: Int) {
    pearlite! { lemma_divmod(x, m); lemma_mul_le(1, m, x / m) }
}

/// DECODER fact (LEVEL-3 phase D-A): every slab descriptor initialize receives comes out of
/// `deserialize_slabs` (recovery.rs:31/56 -> checkpoint.rs:215-224), which reads the slot count
/// as a `u32` field and decodes exactly that many keys — so `keys.len() <= u32::MAX`. Proved of
/// every decoder output by `lemma_parsed_keys_u32` (model/b7.rs, over the exact deser7 mirror).
#[logic(open)]
pub fn t_dec_keys(d: SlabDescriptor) -> bool {
    pearlite! { d.keys@.len() <= u32::MAX@ }
}

/// `u_desc_slots_u32` is DERIVED from 980f61 (`keys.len == slab / es`) and the decoder fact.
#[logic]
#[requires(a_980f61_d(d, base, size) && t_dec_keys(d))]
#[ensures(u_desc_slots_u32(d))]
pub fn lemma_desc_slots(d: SlabDescriptor, base: Int, size: Int) {}

/// TYPE fact: a Rust slice / Vec length never exceeds `isize::MAX`.
#[logic(open)]
pub fn t_slice_len(n: Int) -> bool {
    pearlite! { n <= 9223372036854775807 }
}

/// Region-level construction premise, DERIVED at the component level (NOT an assumption):
/// `slab % ss == 0` is format's own check (lib.rs:391) / a_91a1f6 on initialize, and the
/// region lies inside the data disk (`base + size <= data_disk_size`: format's / initialize's
/// partition, rbase + rsize <= data_disk_size). LEVEL-3 phase D-A: a PROVED INVARIANT of every
/// current region — `em_rg_built` (props/l3start.rs; base: format and initialize, step: every
/// operation, `inv_em_rg_built`); a region-level driver's `rg_built(base, size, fp)` premise is
/// that invariant at a current region (`lemma_em_rg_built_inst`).
#[logic(open)]
pub fn rg_built(base: Int, size: Int, fp: FormatParams) -> bool {
    pearlite! { fp.slab_size@ % fp.sector_size@ == 0 && base + size <= fp.data_disk_size@ }
}

/// What a region constructor (fresh / rebuilt region) relies on: the declared ranges
/// 217ace, 7cb7c3, e0fe79, 3b58ea on the region's parameters plus the derived [`rg_built`].
#[logic(open)]
pub fn rg_in(base: Int, size: Int, fp: FormatParams) -> bool {
    pearlite! {
        a_217ace(fp.sector_size@) && a_7cb7c3(fp.slab_size@, fp.sector_size@) && a_e0fe79(fp.slab_size@, fp.sector_size@)
        && a_3b58ea(fp.data_disk_size@, fp.sector_size@) && rg_built(base, size, fp)
    }
}

/// The raw headroom the region constructors' bodies use (DERIVED from [`rg_in`] by [`lemma_rg_in`]).
#[logic(open)]
pub fn rg_hd(base: Int, size: Int, fp: FormatParams) -> bool {
    pearlite! {
        fp.sector_size@ > 0 && fp.slab_size@ > 0 && fp.slab_size@ % fp.sector_size@ == 0
        && fp.slab_size@ + fp.sector_size@ <= u64::MAX@ && base + size <= u64::MAX@ && 2 * size + fp.sector_size@ <= u64::MAX@
    }
}

#[logic]
#[requires(rg_in(base, size, fp) && 0 <= base && 0 <= size)]
#[ensures(rg_hd(base, size, fp))]
pub fn lemma_rg_in(base: Int, size: Int, fp: FormatParams) {
    pearlite! { lemma_hd_params(fp.slab_size@, fp.sector_size@, fp.data_disk_size@) }
}

/// A recovered descriptor in the declared ranges 980f61 (per descriptor) + 3b612e, plus the
/// decoder fact `t_dec_keys` (LEVEL-3 phase D-A: replaces the undeclared `u_desc_slots_u32`,
/// now DERIVED by `lemma_desc_slots`).
#[logic(open)]
pub fn desc_ok_a(d: SlabDescriptor, base: Int, size: Int, fp: FormatParams) -> bool {
    pearlite! { a_980f61_d(d, base, size) && a_3b612e_d(d, base, size, fp.slab_size@) && t_dec_keys(d) }
}

/// [`desc_ok`] (the span form the rebuild uses) is DERIVED from [`desc_ok_a`] under 7cb7c3 and
/// `slab % ss == 0`.
#[logic]
#[requires(desc_ok_a(d, base, size, fp) && a_7cb7c3(fp.slab_size@, fp.sector_size@) && fp.slab_size@ % fp.sector_size@ == 0)]
#[ensures(desc_ok(d, base, size, fp))]
pub fn lemma_desc_ok_a(d: SlabDescriptor, base: Int, size: Int, fp: FormatParams) {
    pearlite! {
        lemma_slab_span(fp.slab_size@, fp.sector_size@);
        lemma_desc_slots(d, base, size);
        proof_assert! { d.slab_size@ / d.element_size@ >= 0 };
        proof_assert! { slots_of(d.slab_size@, d.element_size@) == d.slab_size@ / d.element_size@ }
    }
}

/// 980f61's non-overlap gives pairwise distinct starts (slab sizes are positive).
#[logic]
#[requires(a_980f61_list(ds, base, size))]
#[requires(forall<p: Int> 0 <= p && p < ds.len() ==> ds[p].slab_size@ > 0)]
#[ensures(starts_distinct(ds))]
pub fn lemma_starts_distinct(ds: Seq<SlabDescriptor>, base: Int, size: Int) {}

// ============================================================ 4. UNDECLARED residue

/// (LEVEL-3 phase D-A: no longer a premise anywhere — DERIVED from 980f61 + the decoder fact
/// `t_dec_keys` by `lemma_desc_slots`.) Formerly UNDECLARED: a recovered descriptor's slot count fits u32 (`slab_size / element_size <
/// 2^32`). 980f61 gives `keys.len == slab_size / element_size`; the model's slab (and the
/// real `Slab::new`, slab.rs:18) keeps `(slab/es) as u32`. Not implied by any declared range
/// (e0fe79 bounds slab/SECTOR_size, and 980f61 does not make element_size >= sector_size).
#[logic(open)]
pub fn u_desc_slots_u32(d: SlabDescriptor) -> bool {
    pearlite! { d.slab_size@ / d.element_size@ < 4294967296 }
}
