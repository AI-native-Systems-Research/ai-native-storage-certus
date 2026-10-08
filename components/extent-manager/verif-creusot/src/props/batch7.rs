//! Batch-7 property proof units (`verify_<id>` / `refute_<id>`, each with a failing `__mutant`
//! twin), unscored witnesses (`witness_<id>_*`) and helpers. Model: model/b7.rs.
use crate::model::b3::*;
use crate::model::b4::*;
use crate::model::b6::*;
use crate::model::b7::*;
use crate::model::buddy::*;
use crate::model::component::*;
use crate::model::listing::*;
use crate::model::params::*;
use crate::model::region::*;
use crate::model::sbbytes::*;
use crate::model::slab::*;
use crate::props::batch2::*;
use crate::props::batch4::*;
use crate::props::batch6::*;
use crate::model::assume::*;
use creusot_std::prelude::*;

// =============================================================================
// format(): parameter checks, superblock write, error frame, checkpoint offset
// =============================================================================

/// `x` is a power of two (`x == 2^k` for some k — the std definition of is_power_of_two).
#[logic(open)]
pub fn is_pow2_7(x: u32) -> bool {
    pearlite! { exists<k: Int> 0 <= k && k.pow2() == x@ }
}

/// The bit test of lib.rs:397 (`x & (x - 1) == 0`, x != 0) holds only for the 32 powers of two.
#[bitwise_proof]
#[ensures(x != 0u32 && (x & (x - 1u32)) == 0u32 ==> x == 1u32 || x == 2u32 || x == 4u32 || x == 8u32 || x == 16u32 || x == 32u32 || x == 64u32 || x == 128u32 || x == 256u32 || x == 512u32 || x == 1024u32 || x == 2048u32 || x == 4096u32 || x == 8192u32 || x == 16384u32 || x == 32768u32 || x == 65536u32 || x == 131072u32 || x == 262144u32 || x == 524288u32 || x == 1048576u32 || x == 2097152u32 || x == 4194304u32 || x == 8388608u32 || x == 16777216u32 || x == 33554432u32 || x == 67108864u32 || x == 134217728u32 || x == 268435456u32 || x == 536870912u32 || x == 1073741824u32 || x == 2147483648u32)]
pub fn fact_pow2_bits7(x: u32) {}

/// lib.rs:397's test passes only for a power of two.
#[ensures(x != 0u32 && (x & (x - 1u32)) == 0u32 ==> is_pow2_7(x))]
pub fn pow2_bits7(x: u32) {
    fact_pow2_bits7(x);
    proof_assert! { x == 1u32 ==> 0.pow2() == 1 };
    proof_assert! { x == 2u32 ==> 1.pow2() == 2 };
    proof_assert! { x == 4u32 ==> 2.pow2() == 4 };
    proof_assert! { x == 8u32 ==> 3.pow2() == 8 };
    proof_assert! { x == 16u32 ==> 4.pow2() == 16 };
    proof_assert! { x == 32u32 ==> 5.pow2() == 32 };
    proof_assert! { x == 64u32 ==> 6.pow2() == 64 };
    proof_assert! { x == 128u32 ==> 7.pow2() == 128 };
    proof_assert! { x == 256u32 ==> 8.pow2() == 256 };
    proof_assert! { x == 512u32 ==> 9.pow2() == 512 };
    proof_assert! { x == 1024u32 ==> 10.pow2() == 1024 };
    proof_assert! { x == 2048u32 ==> 11.pow2() == 2048 };
    proof_assert! { x == 4096u32 ==> 12.pow2() == 4096 };
    proof_assert! { x == 8192u32 ==> 13.pow2() == 8192 };
    proof_assert! { x == 16384u32 ==> 14.pow2() == 16384 };
    proof_assert! { x == 32768u32 ==> 15.pow2() == 32768 };
    proof_assert! { x == 65536u32 ==> 16.pow2() == 65536 };
    proof_assert! { x == 131072u32 ==> 17.pow2() == 131072 };
    proof_assert! { x == 262144u32 ==> 18.pow2() == 262144 };
    proof_assert! { x == 524288u32 ==> 19.pow2() == 524288 };
    proof_assert! { x == 1048576u32 ==> 20.pow2() == 1048576 };
    proof_assert! { x == 2097152u32 ==> 21.pow2() == 2097152 };
    proof_assert! { x == 4194304u32 ==> 22.pow2() == 4194304 };
    proof_assert! { x == 8388608u32 ==> 23.pow2() == 8388608 };
    proof_assert! { x == 16777216u32 ==> 24.pow2() == 16777216 };
    proof_assert! { x == 33554432u32 ==> 25.pow2() == 33554432 };
    proof_assert! { x == 67108864u32 ==> 26.pow2() == 67108864 };
    proof_assert! { x == 134217728u32 ==> 27.pow2() == 134217728 };
    proof_assert! { x == 268435456u32 ==> 28.pow2() == 268435456 };
    proof_assert! { x == 536870912u32 ==> 29.pow2() == 536870912 };
    proof_assert! { x == 1073741824u32 ==> 30.pow2() == 1073741824 };
    proof_assert! { x == 2147483648u32 ==> 31.pow2() == 2147483648 };
}

/// **EM-FORMAT-ERR-REGION-COUNT** — "When format() is called with a region count that is zero or
/// is not a power of two, it returns a CorruptMetadata error." lib.rs:397-401 (after the checks of
/// lib.rs:384-396, which also return CorruptMetadata): `region_count == 0 ||
/// !region_count.is_power_of_two()`. For every component state and every device outcome, format
/// returns CorruptMetadata and changes nothing.
#[requires(wr == Ok(()) || wr == Err(EmError::IoError))]
#[requires(params.region_count == 0u32 || !is_pow2_7(params.region_count))]
#[ensures(result == Err(EmError::CorruptMetadata) && ^em == *em)]
pub fn verify_em_format_err_region_count(em: &mut ExtentManager, params: FormatParams, qn: Option<u64>, qs: Option<u32>,
                                          rnd: Option<u64>, conn: bool, sq: bool, wr: Result<(), EmError>) -> Result<(), EmError> {
    pow2_bits7(params.region_count);
    proof_assert! { !fr002(params) };
    em.format7(params, qn, qs, rnd, conn, sq, wr)
}

/// Mutant: claims a region count of 3 passes the checks.
/// LEVEL-3 (phase D-B2): mutant with EXACTLY the property's requires and body; it claims the
/// negation of the property's first postcondition.
#[requires(wr == Ok(()) || wr == Err(EmError::IoError))]
#[requires(params.region_count == 0u32 || !is_pow2_7(params.region_count))]
#[ensures(!(result == Err(EmError::CorruptMetadata) && ^em == *em))]
pub fn verify_em_format_err_region_count__mutant(em: &mut ExtentManager, params: FormatParams, qn: Option<u64>, qs: Option<u32>,
                                          rnd: Option<u64>, conn: bool, sq: bool, wr: Result<(), EmError>) -> Result<(), EmError> {
    pow2_bits7(params.region_count);
    proof_assert! { !fr002(params) };
    em.format7(params, qn, qs, rnd, conn, sq, wr)
}

/// **EM-FORMAT-ERR-SECTOR-ZERO** — "When format() is called with a sector size of zero, it
/// returns a CorruptMetadata error." lib.rs:384-386 is the first check: nothing is touched.
#[requires(wr == Ok(()) || wr == Err(EmError::IoError))]
#[requires(params.sector_size@ == 0)]
#[ensures(result == Err(EmError::CorruptMetadata) && ^em == *em)]
pub fn verify_em_format_err_sector_zero(em: &mut ExtentManager, params: FormatParams, qn: Option<u64>, qs: Option<u32>,
                                         rnd: Option<u64>, conn: bool, sq: bool, wr: Result<(), EmError>) -> Result<(), EmError> {
    em.format7(params, qn, qs, rnd, conn, sq, wr)
}

/// Mutant: claims a zero sector size is accepted.
#[requires(wr == Ok(()) || wr == Err(EmError::IoError))]
#[requires(params.sector_size@ == 0)]
#[ensures(result == Ok(()))]
pub fn verify_em_format_err_sector_zero__mutant(em: &mut ExtentManager, params: FormatParams, qn: Option<u64>, qs: Option<u32>,
                                                 rnd: Option<u64>, conn: bool, sq: bool, wr: Result<(), EmError>) -> Result<(), EmError> {
    em.format7(params, qn, qs, rnd, conn, sq, wr)
}

/// **EM-FORMAT-ERR-SLAB-NOT-SECTOR-MULTIPLE** — "When format() is called with a slab size that is
/// not an exact multiple of the sector size, it returns a CorruptMetadata error." lib.rs:387-391.
#[requires(wr == Ok(()) || wr == Err(EmError::IoError))]
#[requires(params.sector_size@ > 0 && params.slab_size@ % params.sector_size@ != 0)]
#[ensures(result == Err(EmError::CorruptMetadata) && ^em == *em)]
pub fn verify_em_format_err_slab_not_sector_multiple(em: &mut ExtentManager, params: FormatParams, qn: Option<u64>, qs: Option<u32>,
                                                      rnd: Option<u64>, conn: bool, sq: bool, wr: Result<(), EmError>) -> Result<(), EmError> {
    em.format7(params, qn, qs, rnd, conn, sq, wr)
}

/// Mutant: claims such a slab size reaches the device.
#[requires(wr == Ok(()) || wr == Err(EmError::IoError))]
#[requires(params.sector_size@ > 0 && params.slab_size@ % params.sector_size@ != 0)]
#[ensures(result != Err(EmError::CorruptMetadata))]
pub fn verify_em_format_err_slab_not_sector_multiple__mutant(em: &mut ExtentManager, params: FormatParams, qn: Option<u64>, qs: Option<u32>,
                                                              rnd: Option<u64>, conn: bool, sq: bool, wr: Result<(), EmError>) -> Result<(), EmError> {
    em.format7(params, qn, qs, rnd, conn, sq, wr)
}

/// **EM-FORMAT-ERR-SUPERBLOCK-WRITE** — "If writing the new superblock to the metadata device
/// fails, format() returns an IoError and the component does not adopt the new layout."
/// lib.rs:496-498 `metadata_client.write_blocks(0, &sb_data)?` precedes the two publishes
/// (lib.rs:506-507): when every earlier check passed and the write reports its error (IoError,
/// block_io.rs:49-108), format returns that IoError with regions / shared state unchanged.
#[requires(fr002(params) ==> a_2ee144(params.metadata_alignment@) && fmt_sane(params))]
#[requires(fr002(params) ==> match (qn, qs) { (Some(n), Some(s)) => a_67ea4f(n@, s@), _ => true })]
#[requires(fmt_reach7(*em, params, qn, qs, rnd) && conn && sq)]
#[ensures(result == Err(EmError::IoError) && fmt_frame7(*em, ^em))]
#[ensures((^em).regions == em.regions && (^em).shared == em.shared)]
pub fn verify_em_format_err_superblock_write(em: &mut ExtentManager, params: FormatParams, qn: Option<u64>, qs: Option<u32>,
                                              rnd: Option<u64>, conn: bool, sq: bool) -> Result<(), EmError> {
    em.format7(params, qn, qs, rnd, conn, sq, Err(EmError::IoError))
}

/// Mutant: claims the failed write still publishes the layout.
#[requires(fr002(params) ==> a_2ee144(params.metadata_alignment@) && fmt_sane(params))]
#[requires(fr002(params) ==> match (qn, qs) { (Some(n), Some(s)) => a_67ea4f(n@, s@), _ => true })]
#[requires(fmt_reach7(*em, params, qn, qs, rnd) && conn && sq)]
#[ensures((^em).shared != em.shared)]
pub fn verify_em_format_err_superblock_write__mutant(em: &mut ExtentManager, params: FormatParams, qn: Option<u64>, qs: Option<u32>,
                                                      rnd: Option<u64>, conn: bool, sq: bool) -> Result<(), EmError> {
    em.format7(params, qn, qs, rnd, conn, sq, Err(EmError::IoError))
}

/// **EM-FORMAT-FRAME-ON-ERROR** — "When format() fails for any reason, the component's in-memory
/// state (whether uninitialized or holding a previous layout and its extents) is exactly what it
/// was before the call." Every `return Err` / `?` of lib.rs:383-498 precedes the only two
/// in-memory writes (lib.rs:506-507): on every error path regions, shared state, namespace and
/// base LBAs are unchanged and every existing region object (the previous layout's regions,
/// with their slabs and extents) is untouched (arena prefix). The regions format built before
/// failing are unreachable new `Arc`s (new arena entries).
#[requires(fr002(params) ==> a_2ee144(params.metadata_alignment@) && fmt_sane(params))]
#[requires(fr002(params) ==> match (qn, qs) { (Some(n), Some(s)) => a_67ea4f(n@, s@), _ => true })]
#[requires(wr == Ok(()) || wr == Err(EmError::IoError))]
#[ensures(match result { Err(_) => fmt_frame7(*em, ^em), Ok(()) => true })]
pub fn verify_em_format_frame_on_error(em: &mut ExtentManager, params: FormatParams, qn: Option<u64>, qs: Option<u32>,
                                        rnd: Option<u64>, conn: bool, sq: bool, wr: Result<(), EmError>) -> Result<(), EmError> {
    em.format7(params, qn, qs, rnd, conn, sq, wr)
}

/// Mutant: claims a failed format changes the published regions.
#[requires(fr002(params) ==> a_2ee144(params.metadata_alignment@) && fmt_sane(params))]
#[requires(fr002(params) ==> match (qn, qs) { (Some(n), Some(s)) => a_67ea4f(n@, s@), _ => true })]
#[requires(wr == Ok(()) || wr == Err(EmError::IoError))]
#[ensures(match result { Err(_) => (^em).regions != em.regions, Ok(()) => true })]
pub fn verify_em_format_frame_on_error__mutant(em: &mut ExtentManager, params: FormatParams, qn: Option<u64>, qs: Option<u32>,
                                                rnd: Option<u64>, conn: bool, sq: bool, wr: Result<(), EmError>) -> Result<(), EmError> {
    em.format7(params, qn, qs, rnd, conn, sq, wr)
}

/// `align_up(4096, a)` is a multiple of `a` and at least 4096.
#[logic]
#[requires(a > 0)]
#[ensures(ckoff_l(a) % a == 0 && ckoff_l(a) >= 4096)]
pub fn lemma_ckoff_min7(a: Int) {
    pearlite! {
        lemma_divmod(4096 + a - 1, a);
        lemma_mod_mul7((4096 + a - 1) / a, a);
        lemma_ckoff_ge(a)
    }
}

/// `(q * a) % a == 0`.
#[logic]
#[requires(a > 0 && q >= 0)]
#[ensures((q * a) % a == 0)]
pub fn lemma_mod_mul7(q: Int, a: Int) {
    pearlite! { lemma_divmod(q * a, a) }
}

/// Every multiple `m` of `a` that is >= 4096 is >= align_up(4096, a).
#[logic]
#[requires(a > 0 && m >= 4096 && m % a == 0)]
#[ensures(ckoff_l(a) <= m)]
pub fn lemma_mult_ge7(a: Int, m: Int) {
    pearlite! {
        lemma_divmod(m, a); lemma_divmod(4096 + a - 1, a);
        lemma_div_le7(m / a, (4096 + a - 1) / a, a)
    }
}

/// If `q*a <= 4095 + a` and `k*a >= 4096` then `q <= k` (so `q*a <= k*a`).
#[logic]
#[requires(a > 0 && k >= 0 && q >= 0 && q * a <= 4095 + a && k * a >= 4096)]
#[ensures(q * a <= k * a)]
pub fn lemma_div_le7(k: Int, q: Int, a: Int) {
    pearlite! {
        if q > k { lemma_mul_le(k + 1, q, a); lemma_distrib(k, 1, a) }
    }
}

/// **EM-FORMAT-POST-CKPT-REGION-OFFSET** — REFUTED. INDEPENDENT candidate bug, ROOT CAUSE =
/// FORMAT-ALIGN-OVERFLOW (batch 6): lib.rs:423 `(sb_size + alignment - 1) / alignment * alignment`
/// is unbounded (interfaces iextent_manager.rs:52-55 documents no limit on metadata_alignment).
/// "After format() succeeds, the recorded checkpoint region offset is the smallest multiple of the
/// configured metadata alignment that is >= the superblock size (4096 bytes), or the superblock
/// size itself when the alignment is zero." metadata_alignment = u64::MAX (the smallest multiple
/// >= 4096 is u64::MAX itself), a 16 KiB metadata device (32 x 512), metadata_region_size 0, data
/// sector 512, data disk 1 MiB. RELEASE build: the sum wraps to 4094, 4094 / u64::MAX = 0, so
/// checkpoint_region_offset = 0 (< 4096, the superblock's own bytes); the region size is 8192 > 0
/// and the usable data space 1 MiB > 0, so both CorruptMetadata checks (lib.rs:434-438, 449-452)
/// pass and format records offset 0 in the superblock (lib.rs:483-494). DEBUG build: the sum
/// overflows -> panic (no success). Bounded half PROVED: `witness_em_format_post_ckpt_region_offset_bounded`.
#[ensures(4096 + (18446744073709551615u64@ - 1) > u64::MAX@)]
#[ensures(result.0@ == 0 && result.0@ < 4096 && result.1@ == 8192 && result.2@ == 1048576)]
#[ensures(ckoff_l(18446744073709551615u64@) == 18446744073709551615u64@)]
pub fn refute_em_format_post_ckpt_region_offset() -> (u64, u64, u64) {
    fact_wrap_4094();
    let r = layout_release(16384, 0, u64::MAX, 512, 1048576);
    proof_assert! { forall<w: u64> w == 4096u64 + (18446744073709551615u64 - 1u64) ==> w@ == 4094 };
    proof_assert! { 4094 / 18446744073709551615 * 18446744073709551615 == 0 };
    proof_assert! { 16384 / 2 / 512 * 512 == 8192 };
    proof_assert! { (4096 + 18446744073709551615 - 1) / 18446744073709551615 == 1 };
    r
}

/// Mutant: claims the release-build offset is still at least the superblock size.
#[ensures(result.0@ >= 4096)]
pub fn refute_em_format_post_ckpt_region_offset__mutant() -> (u64, u64, u64) {
    fact_wrap_4094();
    let r = layout_release(16384, 0, u64::MAX, 512, 1048576);
    proof_assert! { forall<w: u64> w == 4096u64 + (18446744073709551615u64 - 1u64) ==> w@ == 4094 };
    proof_assert! { 4094 / 18446744073709551615 * 18446744073709551615 == 0 };
    r
}

/// Unscored: within `metadata_alignment + 4096 <= u64::MAX` (no wrap) the property HOLDS — on
/// success the superblock records align_up(4096, a), the smallest multiple of a >= 4096 (4096
/// for a = 0).
#[requires(a_2ee144(params.metadata_alignment@))]
#[requires(fr002(params) ==> sane_params(params))]
#[requires(fr002(params) ==> match (qn, qs) { (Some(n), Some(s)) => a_67ea4f(n@, s@), _ => true })]
#[requires(wr == Ok(()) || wr == Err(EmError::IoError))]
#[ensures(result == Ok(()) ==> match (^em).shared {
    Some(sh) => (params.metadata_alignment@ == 0 ==> sh.superblock.checkpoint_region_offset@ == 4096)
        && (params.metadata_alignment@ > 0 ==>
            sh.superblock.checkpoint_region_offset@ % params.metadata_alignment@ == 0
            && sh.superblock.checkpoint_region_offset@ >= 4096
            && forall<m: Int> m >= 4096 && m % params.metadata_alignment@ == 0 ==> sh.superblock.checkpoint_region_offset@ <= m),
    None => false,
})]
pub fn witness_em_format_post_ckpt_region_offset_bounded(em: &mut ExtentManager, params: FormatParams, qn: Option<u64>, qs: Option<u32>,
                                                          rnd: Option<u64>, conn: bool, sq: bool, wr: Result<(), EmError>) -> Result<(), EmError> {
    proof_assert! { params.metadata_alignment@ > 0 ==> { lemma_ckoff_min7(params.metadata_alignment@); true } };
    proof_assert! { params.metadata_alignment@ > 0 ==> forall<m: Int> m >= 4096 && m % params.metadata_alignment@ == 0 ==> {
        lemma_mult_ge7(params.metadata_alignment@, m); ckoff_l(params.metadata_alignment@) <= m } };
    em.format7(params, qn, qs, rnd, conn, sq, wr)
}

// =============================================================================
// get_extents / get_instance_id
// =============================================================================

/// **EM-GETEXT-FRAME-READ-ONLY** — "Calling get_extents() does not change any extent, any
/// reservation, used_bytes(), or the on-disk metadata." get_extents (lib.rs:632-656) takes the
/// regions read lock and each region's read lock and only reads slab keys: the whole component
/// state — every region (slabs, keys, bitmaps, buddy free lists, hence reservations and
/// used_bytes), the shared state and the metadata device — is unchanged.
// LEVEL-3 (phase D-B2): only the part of the region invariant the listing reads (every slab
// well-formed, em_list_ok — implied by the PROVED em_ok, lemma_em_list); no headroom conjunct.
#[requires(em_regions_wf(*em) && em_list_ok(*em))]
#[ensures(^em == *em)]
pub fn verify_em_getext_frame_read_only(em: &mut ExtentManager) -> Vec<Extent> {
    em.get_extents()
}

/// Mutant: claims the call changes the metadata device.
// LEVEL-3 (phase D-B2): only the part of the region invariant the listing reads (every slab
// well-formed, em_list_ok — implied by the PROVED em_ok, lemma_em_list); no headroom conjunct.
#[requires(em_regions_wf(*em) && em_list_ok(*em))]
#[ensures((^em).dev != em.dev)]
pub fn verify_em_getext_frame_read_only__mutant(em: &mut ExtentManager) -> Vec<Extent> {
    em.get_extents()
}

/// **EM-GETEXT-UNINIT-EMPTY** — "Before the component is formatted or initialized, get_extents()
/// returns an empty list." lib.rs:654 `None => Vec::new()`: (a) a freshly constructed component
/// (new_inner, regions None); (b) any component whose regions are not published — a failed
/// format / initialize leaves them unchanged (format7 / initialize7 error frames).
#[requires(em_regions_wf(*em) && em_regions_ok(*em) && em.regions == None)]
#[ensures(result.0@.len() == 0 && result.1@.len() == 0)]
pub fn verify_em_getext_uninit_empty(dev: MetaDevice, em: &ExtentManager) -> (Vec<Extent>, Vec<Extent>) {
    let e0 = new_inner(dev);
    proof_assert! { em_regions_wf(e0) && em_regions_ok(e0) };
    let a = e0.get_extents();
    let b = em.get_extents();
    (a, b)
}

/// Mutant: claims a fresh component lists an extent.
/// LEVEL-3 (phase D-B2): mutant with EXACTLY the property's requires and body; it claims the
/// negation of the property's first postcondition.
#[requires(em_regions_wf(*em) && em_regions_ok(*em) && em.regions == None)]
#[ensures(!(result.0@.len() == 0 && result.1@.len() == 0))]
pub fn verify_em_getext_uninit_empty__mutant(dev: MetaDevice, em: &ExtentManager) -> (Vec<Extent>, Vec<Extent>) {
    let e0 = new_inner(dev);
    proof_assert! { em_regions_wf(e0) && em_regions_ok(e0) };
    let a = e0.get_extents();
    let b = em.get_extents();
    (a, b)
}

/// **EM-INSTID-ERR-NOT-INITIALIZED** — "When get_instance_id() is called before either format()
/// or initialize() has succeeded, it returns a NotInitialized error." lib.rs:686-690: `shared`
/// None -> NotInitialized. (a) a fresh component; (b) a fresh component after a FAILED format;
/// (c) after a FAILED initialize (both error frames keep `shared` None); (d) any state with
/// `shared` None.
#[requires(fr002(params) ==> a_2ee144(params.metadata_alignment@) && fmt_sane(params))]
#[requires(fr002(params) ==> match (qn, qs) { (Some(n), Some(s)) => a_67ea4f(n@, s@), _ => true })]
#[requires(wr == Ok(()) || wr == Err(EmError::IoError))]
#[requires(d2.connected && conn && sq ==> init_pre7(sbr, rd_a, rd_i))]
#[requires(em.shared == None)]
#[ensures(result.0 == Err(EmError::NotInitialized))]
#[ensures(result.1 != Ok(()) ==> result.2 == Err(EmError::NotInitialized))]
#[ensures(result.3 != Ok(()) ==> result.4 == Err(EmError::NotInitialized))]
#[ensures(result.5 == Err(EmError::NotInitialized))]
pub fn verify_em_instid_err_not_initialized(
    dev: MetaDevice, d1: MetaDevice, d2: MetaDevice, params: FormatParams, qn: Option<u64>, qs: Option<u32>, rnd: Option<u64>, conn: bool, sq: bool,
    wr: Result<(), EmError>, sbr: Result<Vec<u8>, EmError>, rd_a: Result<Vec<u8>, EmError>, rd_i: Result<Vec<u8>, EmError>,
    em: &ExtentManager,
) -> (Result<u64, EmError>, Result<(), EmError>, Result<u64, EmError>, Result<(), EmError>, Result<u64, EmError>, Result<u64, EmError>) {
    let e0 = new_inner(dev);
    let a = e0.get_instance_id();
    let mut e1 = new_inner(d1);
    let f = e1.format7(params, qn, qs, rnd, conn, sq, wr);
    let b = e1.get_instance_id();
    let mut e2 = new_inner(d2);
    proof_assert! { em_regions_wf(e2) };
    let g = e2.initialize7(conn, sq, sbr, rd_a, rd_i);
    let c = e2.get_instance_id();
    let d = em.get_instance_id();
    (a, f, b, g, c, d)
}

/// Mutant: claims a fresh component already has an instance id.
/// LEVEL-3 (phase D-B2): mutant with EXACTLY the property's requires and body; it claims the
/// negation of the property's first postcondition.
#[requires(fr002(params) ==> a_2ee144(params.metadata_alignment@) && fmt_sane(params))]
#[requires(fr002(params) ==> match (qn, qs) { (Some(n), Some(s)) => a_67ea4f(n@, s@), _ => true })]
#[requires(wr == Ok(()) || wr == Err(EmError::IoError))]
#[requires(d2.connected && conn && sq ==> init_pre7(sbr, rd_a, rd_i))]
#[requires(em.shared == None)]
#[ensures(!(result.0 == Err(EmError::NotInitialized)))]
pub fn verify_em_instid_err_not_initialized__mutant(
    dev: MetaDevice, d1: MetaDevice, d2: MetaDevice, params: FormatParams, qn: Option<u64>, qs: Option<u32>, rnd: Option<u64>, conn: bool, sq: bool,
    wr: Result<(), EmError>, sbr: Result<Vec<u8>, EmError>, rd_a: Result<Vec<u8>, EmError>, rd_i: Result<Vec<u8>, EmError>,
    em: &ExtentManager,
) -> (Result<u64, EmError>, Result<(), EmError>, Result<u64, EmError>, Result<(), EmError>, Result<u64, EmError>, Result<u64, EmError>) {
    let e0 = new_inner(dev);
    let a = e0.get_instance_id();
    let mut e1 = new_inner(d1);
    let f = e1.format7(params, qn, qs, rnd, conn, sq, wr);
    let b = e1.get_instance_id();
    let mut e2 = new_inner(d2);
    proof_assert! { em_regions_wf(e2) };
    let g = e2.initialize7(conn, sq, sbr, rd_a, rd_i);
    let c = e2.get_instance_id();
    let d = em.get_instance_id();
    (a, f, b, g, c, d)
}

// =============================================================================
// initialize(): error paths (model/b7.rs initialize7 = init_front7 `?` + component.rs initialize)
// =============================================================================

/// **EM-INIT-ERR-DEVICE-NOT-CONNECTED** — "If no metadata block device has been connected,
/// initialize() fails with a NotInitialized error and leaves the component's state unchanged."
/// lib.rs:518 -> get_metadata_client lib.rs:185-189 `metadata_device.get().map_err(not_initialized)?`
/// is the first fallible step; nothing has been written. For every superblock / checkpoint content.
#[requires(em_regions_wf(*em) && !em.dev.connected)]
#[ensures(result == Err(EmError::NotInitialized) && ^em == *em)]
pub fn verify_em_init_err_device_not_connected(em: &mut ExtentManager, conn: bool, sq: bool, sbr: Result<Vec<u8>, EmError>,
                                                rd_a: Result<Vec<u8>, EmError>, rd_i: Result<Vec<u8>, EmError>) -> Result<(), EmError> {
    em.initialize7(conn, sq, sbr, rd_a, rd_i)
}

/// Mutant: claims an unconnected device is reported as an I/O error.
#[requires(em_regions_wf(*em) && !em.dev.connected)]
#[ensures(result == Err(EmError::IoError))]
pub fn verify_em_init_err_device_not_connected__mutant(em: &mut ExtentManager, conn: bool, sq: bool, sbr: Result<Vec<u8>, EmError>,
                                                        rd_a: Result<Vec<u8>, EmError>, rd_i: Result<Vec<u8>, EmError>) -> Result<(), EmError> {
    em.initialize7(conn, sq, sbr, rd_a, rd_i)
}

/// **EM-INIT-FRAME-ON-ERROR** — "When initialize() fails for any reason, the component's in-memory
/// state is exactly what it was before the call." Every error of initialize comes from
/// get_metadata_client or recover (lib.rs:518-519, init_front7, which takes `&self`); the rebuild
/// (lib.rs:521-575) has no error path and the only writes are lib.rs:576-577. So Err => the whole
/// component state is unchanged.
#[requires(em_regions_wf(*em))]
#[requires(em.dev.connected && conn && sq ==> init_pre7(sbr, rd_a, rd_i))]
#[ensures(match result { Err(_) => ^em == *em, Ok(()) => true })]
pub fn verify_em_init_frame_on_error(em: &mut ExtentManager, conn: bool, sq: bool, sbr: Result<Vec<u8>, EmError>,
                                      rd_a: Result<Vec<u8>, EmError>, rd_i: Result<Vec<u8>, EmError>) -> Result<(), EmError> {
    em.initialize7(conn, sq, sbr, rd_a, rd_i)
}

/// Mutant: claims a failed initialize publishes regions.
#[requires(em_regions_wf(*em))]
#[requires(em.dev.connected && conn && sq ==> init_pre7(sbr, rd_a, rd_i))]
#[ensures(match result { Err(_) => (^em).regions != em.regions, Ok(()) => true })]
pub fn verify_em_init_frame_on_error__mutant(em: &mut ExtentManager, conn: bool, sq: bool, sbr: Result<Vec<u8>, EmError>,
                                              rd_a: Result<Vec<u8>, EmError>, rd_i: Result<Vec<u8>, EmError>) -> Result<(), EmError> {
    em.initialize7(conn, sq, sbr, rd_a, rd_i)
}

/// **EM-INIT-ERR-IO** — "If the metadata device fails while the superblock is being read,
/// initialize() returns an IoError; a device failure while reading checkpoint data never leads
/// initialize() to report success with partially recovered state." (a) recovery.rs:15
/// `read_blocks(0, SUPERBLOCK_SIZE)?` propagates read_blocks' error, which is IoError for every
/// device failure (block_io.rs:110-155, batch 3 EvClient::read_blocks / nvme_to_em): sbr ==
/// Err(IoError) => Err(IoError), state unchanged. (b) On success the published regions are
/// rebuilt (init_rg_ok) from EXACTLY the complete decoding (`parsed` + `ck_complete`) of one
/// checkpoint copy whose read_checkpoint_region returned Ok — a copy whose read failed (rd ==
/// Err, e.g. a device error) contributes nothing — or from no checkpoint at all (seq 0).
#[requires(em_regions_wf(*em) && em.dev.connected)]
#[requires(init_pre7(sbr, rd_a, rd_i))]
#[ensures(sbr == Err(EmError::IoError) ==> result == Err(EmError::IoError) && ^em == *em)]
#[ensures(result == Ok(()) ==> exists<sb: Superblock, pr: Seq<Vec<SlabDescriptor>>> front_ok7(sbr, rd_a, rd_i, sb, pr)
    && (sb.checkpoint_seq@ == 0 || good7(rd_a, pr) || good7(rd_i, pr))
    && match ((^em).regions, (^em).shared) {
        (Some(rv), Some(sh)) => sh.superblock == sb
            && forall<i: Int> 0 <= i && i < rv@.len() ==> init_rg_ok((^em).arena@[rv@[i]@], dsel(pr, i)),
        _ => false,
    })]
pub fn verify_em_init_err_io(em: &mut ExtentManager, sbr: Result<Vec<u8>, EmError>,
                             rd_a: Result<Vec<u8>, EmError>, rd_i: Result<Vec<u8>, EmError>) -> Result<(), EmError> {
    em.initialize7(true, true, sbr, rd_a, rd_i)
}

/// Mutant: claims a superblock read failure is reported as corruption.
#[requires(em_regions_wf(*em) && em.dev.connected)]
#[requires(init_pre7(sbr, rd_a, rd_i))]
#[ensures(sbr == Err(EmError::IoError) ==> result == Err(EmError::CorruptMetadata))]
pub fn verify_em_init_err_io__mutant(em: &mut ExtentManager, sbr: Result<Vec<u8>, EmError>,
                                     rd_a: Result<Vec<u8>, EmError>, rd_i: Result<Vec<u8>, EmError>) -> Result<(), EmError> {
    em.initialize7(true, true, sbr, rd_a, rd_i)
}

/// **EM-INIT-ERR-SUPERBLOCK-INVALID** — "When the persisted superblock is shorter than one
/// superblock, does not carry the expected magic value ("CERTUSV4"), or its stored CRC32
/// checksum does not match its contents, initialize() returns a CorruptMetadata error."
/// recovery.rs:16 `Superblock::deserialize(&sb_data)?`: superblock.rs:100-102 (length),
/// 106-112 (magic 0x4345_5254_5553_5634), 143-150 (CRC32 of bytes 0..92 vs bytes 92..96).
/// For ANY bytes the device returns (any length, any content).
#[requires(em_regions_wf(*em) && em.dev.connected)]
#[requires(rec_pre7(Ok(b), rd_a, rd_i))]
#[requires(b@.len() < 4096 || d64(b@, 0) != SUPERBLOCK_MAGIC || d32(b@, 92) != crc32(b@.subsequence(0, 92)))]
#[ensures(result == Err(EmError::CorruptMetadata) && ^em == *em)]
pub fn verify_em_init_err_superblock_invalid(em: &mut ExtentManager, b: Vec<u8>,
                                             rd_a: Result<Vec<u8>, EmError>, rd_i: Result<Vec<u8>, EmError>) -> Result<(), EmError> {
    em.initialize7(true, true, Ok(b), rd_a, rd_i)
}

/// Mutant: claims a bad-magic superblock is accepted.
/// LEVEL-3 (phase D-B2): mutant with EXACTLY the property's requires and body; it claims the
/// negation of the property's first postcondition.
#[requires(em_regions_wf(*em) && em.dev.connected)]
#[requires(rec_pre7(Ok(b), rd_a, rd_i))]
#[requires(b@.len() < 4096 || d64(b@, 0) != SUPERBLOCK_MAGIC || d32(b@, 92) != crc32(b@.subsequence(0, 92)))]
#[ensures(!(result == Err(EmError::CorruptMetadata) && ^em == *em))]
pub fn verify_em_init_err_superblock_invalid__mutant(em: &mut ExtentManager, b: Vec<u8>,
                                             rd_a: Result<Vec<u8>, EmError>, rd_i: Result<Vec<u8>, EmError>) -> Result<(), EmError> {
    em.initialize7(true, true, Ok(b), rd_a, rd_i)
}

/// **EM-INIT-ERR-BOTH-COPIES-INVALID** — "When the superblock is valid but neither the active
/// checkpoint copy nor a valid previous checkpoint in the other copy can be read and validated
/// (including when the active one is the very first checkpoint), initialize() fails with a
/// CorruptMetadata error instead of reporting success." recovery.rs:29-70: the active copy's
/// read_checkpoint_region fails (device error or header/CRC/seq check — any error) or its payload
/// does not decode; the inactive copy is tried only when prev_seq = seq - 1 > 0 (seq 1 = the very
/// first checkpoint: no fallback) and fails likewise -> CorruptMetadata (recovery.rs:68-70); state
/// unchanged. "Valid superblock": decodes (sb_ok7) with active_copy <= 1 (rec_pre7 — see
/// SB-ACTIVE-COPY-UNCHECKED for > 1).
#[requires(em_regions_wf(*em) && em.dev.connected)]
#[requires(rec_pre7(Ok(b), rd_a, rd_i))]
#[requires(sb_ok7(b@) && sb_of(b@).checkpoint_seq@ > 0)]
#[requires(bad7(rd_a) && (sb_of(b@).checkpoint_seq@ == 1 || bad7(rd_i)))]
#[ensures(result == Err(EmError::CorruptMetadata) && ^em == *em)]
pub fn verify_em_init_err_both_copies_invalid(em: &mut ExtentManager, b: Vec<u8>,
                                              rd_a: Result<Vec<u8>, EmError>, rd_i: Result<Vec<u8>, EmError>) -> Result<(), EmError> {
    em.initialize7(true, true, Ok(b), rd_a, rd_i)
}

/// Mutant: claims the failure is reported as an I/O error.
#[requires(em_regions_wf(*em) && em.dev.connected)]
#[requires(rec_pre7(Ok(b), rd_a, rd_i))]
#[requires(sb_ok7(b@) && sb_of(b@).checkpoint_seq@ > 0)]
#[requires(bad7(rd_a) && (sb_of(b@).checkpoint_seq@ == 1 || bad7(rd_i)))]
#[ensures(result == Err(EmError::IoError))]
pub fn verify_em_init_err_both_copies_invalid__mutant(em: &mut ExtentManager, b: Vec<u8>,
                                                      rd_a: Result<Vec<u8>, EmError>, rd_i: Result<Vec<u8>, EmError>) -> Result<(), EmError> {
    em.initialize7(true, true, Ok(b), rd_a, rd_i)
}

/// The checkpoint copy recover decodes (active, or the fallback) passed its checks but is truncated.
#[logic(open)]
pub fn malformed7(b: Seq<u8>, rd_a: Result<Vec<u8>, EmError>, rd_i: Result<Vec<u8>, EmError>) -> bool {
    pearlite! {
        match rd_a {
            Ok(d) => !ck_complete(d@),
            Err(_) => sb_of(b).checkpoint_seq@ > 1 && match rd_i { Ok(d) => !ck_complete(d@), Err(_) => false },
        }
    }
}

/// **EM-INIT-ERR-PAYLOAD-MALFORMED** — "If a checkpoint passes its checksum but its contents are
/// truncated (missing region count, slab count, slab header or slot keys), initialize() fails
/// with a CorruptMetadata error." recovery.rs:36 / :57 `deserialize_slabs(&data)?` on the payload
/// read_checkpoint_region returned Ok (CRC passed); deser7 (checkpoint.rs:183-241) fails exactly
/// on truncation (`ck_complete`) -> CorruptMetadata, state unchanged.
#[requires(em_regions_wf(*em) && em.dev.connected)]
#[requires(rec_pre7(Ok(b), rd_a, rd_i))]
#[requires(sb_ok7(b@) && sb_of(b@).checkpoint_seq@ > 0 && malformed7(b@, rd_a, rd_i))]
#[ensures(result == Err(EmError::CorruptMetadata) && ^em == *em)]
pub fn verify_em_init_err_payload_malformed(em: &mut ExtentManager, b: Vec<u8>,
                                            rd_a: Result<Vec<u8>, EmError>, rd_i: Result<Vec<u8>, EmError>) -> Result<(), EmError> {
    em.initialize7(true, true, Ok(b), rd_a, rd_i)
}

/// Mutant: claims a truncated payload is accepted.
#[requires(em_regions_wf(*em) && em.dev.connected)]
#[requires(rec_pre7(Ok(b), rd_a, rd_i))]
#[requires(sb_ok7(b@) && sb_of(b@).checkpoint_seq@ > 0 && malformed7(b@, rd_a, rd_i))]
#[ensures(result == Ok(()))]
pub fn verify_em_init_err_payload_malformed__mutant(em: &mut ExtentManager, b: Vec<u8>,
                                                    rd_a: Result<Vec<u8>, EmError>, rd_i: Result<Vec<u8>, EmError>) -> Result<(), EmError> {
    em.initialize7(true, true, Ok(b), rd_a, rd_i)
}

/// **EM-CKPT-DECODE-TRUNCATED** — "Decoding checkpoint contents that end before the region count,
/// a slab count, a slab header or a slab's keys are complete fails with a CorruptMetadata error and
/// never reads past the end of the data." deserialize_slabs (checkpoint.rs:183-241, mirror deser7):
/// for EVERY byte string (of a length a Rust slice can have), Err(CorruptMetadata) exactly when it
/// is truncated (`ck_complete` false: fewer than 4 bytes for the region count, or `end_regions`
/// finds a slab count / 24-byte slab header / 8 x num_slots key vector that does not fit), else Ok
/// with the positional decoding. "Never reads past the end": every `data[pos..pos + n]` of deser7 /
/// region7 / read_keys7 carries a proved bounds VC (rd32 / rd64 require pos + n <= len).
#[requires(data@.len() <= 9223372036854775807)]
#[ensures(!ck_complete(data@) ==> result == Err(EmError::CorruptMetadata))]
#[ensures(data@.len() < 4 ==> result == Err(EmError::CorruptMetadata))]
#[ensures(match result { Ok(v) => ck_complete(data@) && parsed(data@, v@), Err(e) => e == EmError::CorruptMetadata && !ck_complete(data@) })]
pub fn verify_em_ckpt_decode_truncated(data: &Vec<u8>) -> Result<Vec<Vec<SlabDescriptor>>, EmError> {
    deser7(data)
}

/// Mutant: claims every payload of at least 4 bytes decodes.
/// LEVEL-3 (phase D-B2): mutant with EXACTLY the property's requires and body; it claims the
/// negation of the property's first postcondition.
#[requires(data@.len() <= 9223372036854775807)]
#[ensures(!(!ck_complete(data@) ==> result == Err(EmError::CorruptMetadata)))]
pub fn verify_em_ckpt_decode_truncated__mutant(data: &Vec<u8>) -> Result<Vec<Vec<SlabDescriptor>>, EmError> {
    deser7(data)
}

// =============================================================================
// Buddy allocator
// =============================================================================

/// **EM-BUDDY-ALLOC-NONE-ONLY-IF-EXHAUSTED** — "Allocation fails only when no free block of the
/// needed size or larger exists." BuddyAllocator::alloc (buddy.rs:57-82): when it returns None,
/// nothing changed and EVERY free block (any order, any list) is smaller than the request:
/// orders below ord(ceil(size/ss)) are too small (2^o sectors < the needed count), and every list
/// from that order up to max_order is empty (buddy.rs:65-71).
// LEVEL-3 (phase D-B2): the allocator is a region's buddy (rg_inv, PROVED) and the request is that
// region's slab size — the only size its one caller (alloc_extent's new-slab path, region.rs:66-74)
// requests (as EM-BUDDY-ALLOC-ACCOUNTING, D-B1). `bd_inv` and `size + ss <= u64::MAX` are DERIVED
// (rg_buddy; rg_params' slab + ss headroom).
#[requires(rg_inv(*r) && r.buddy == *b && size == r.format_params.slab_size)]
#[ensures(result == None ==> ^b == *b && forall<o: Int, k: Int> 0 <= o && o < b.free_lists@.len() && 0 <= k && k < b.free_lists@[o]@.len() ==>
    span(b.sector_size@, o) < size@)]
pub fn verify_em_buddy_alloc_none_only_if_exhausted(b: &mut BuddyAllocator, size: u64, r: Snapshot<RegionState>) -> Option<u64> {
    proof_assert! { bd_inv(*b) && size@ + b.sector_size@ <= u64::MAX@ };
    let ss = snapshot! { b.sector_size@ };
    let blocks = snapshot! { (size@ + *ss - 1) / *ss };
    let k = snapshot! { ord(*blocks) };
    let fl = snapshot! { b.free_lists@ };
    let mo = snapshot! { b.max_order@ };
    proof_assert! { lemma_divmod(size@ + *ss - 1, *ss); *blocks >= 0 && *blocks <= 18446744073709551616 && *blocks * *ss <= size@ + *ss - 1 };
    proof_assert! { lemma_ord_ge(*blocks); 0 <= *k };
    proof_assert! { lemma_ord_from_min(*blocks, 0); forall<j: Int> 0 <= j && j < *k ==> j.pow2() < *blocks };
    proof_assert! { forall<j: Int> 0 <= j && j < *k ==> { lemma_mul_le(j.pow2(), *blocks - 1, *ss); j.pow2() * *ss <= (*blocks - 1) * *ss } };
    proof_assert! { *blocks >= 1 ==> { lemma_distrib(*blocks - 1, 1, *ss); (*blocks - 1) * *ss < size@ } };
    proof_assert! { forall<j: Int> 0 <= j && j < *k ==> span(*ss, j) < size@ };
    proof_assert! { lemma_first_ne(*fl, *k, *mo); true };
    b.alloc(size)
}

/// Mutant: claims failure means every free list is empty.
// LEVEL-3 (phase D-B2): the allocator is a region's buddy (rg_inv, PROVED) and the request is that
// region's slab size — the only size its one caller (alloc_extent's new-slab path, region.rs:66-74)
// requests (as EM-BUDDY-ALLOC-ACCOUNTING, D-B1). `bd_inv` and `size + ss <= u64::MAX` are DERIVED
// (rg_buddy; rg_params' slab + ss headroom).
#[requires(rg_inv(*r) && r.buddy == *b && size == r.format_params.slab_size)]
#[ensures(result == None ==> forall<o: Int> 0 <= o && o < b.free_lists@.len() ==> b.free_lists@[o]@.len() == 0)]
pub fn verify_em_buddy_alloc_none_only_if_exhausted__mutant(b: &mut BuddyAllocator, size: u64, r: Snapshot<RegionState>) -> Option<u64> {
    proof_assert! { bd_inv(*b) && size@ + b.sector_size@ <= u64::MAX@ };
    b.alloc(size)
}

/// **EM-BUDDY-ALLOC-WITHIN** — "A successful allocation returns a block that starts at or after
/// the region base, is at least the requested size, and ends at or before the end of the region."
/// buddy.rs:57-82: the returned block is [r, r + span(ss, k)) with k = ord(ceil(size/ss)):
/// base <= r, span >= size, and r + span <= base + total_usable_size (the popped block lies inside
/// the region, bd_inside, and its lower order-k part is returned).
// LEVEL-3 (phase D-B2): the allocator is a region's buddy (rg_inv, PROVED) and the request is that
// region's slab size — the only size its one caller (alloc_extent's new-slab path, region.rs:66-74)
// requests (as EM-BUDDY-ALLOC-ACCOUNTING, D-B1). `bd_inv` and `size + ss <= u64::MAX` are DERIVED
// (rg_buddy; rg_params' slab + ss headroom).
#[requires(rg_inv(*r) && r.buddy == *b && size == r.format_params.slab_size)]
#[ensures(forall<r: u64> result == Some(r) ==> b.base_offset@ <= r@
    && span(b.sector_size@, ord((size@ + b.sector_size@ - 1) / b.sector_size@)) >= size@
    && r@ + span(b.sector_size@, ord((size@ + b.sector_size@ - 1) / b.sector_size@)) <= b.base_offset@ + b.total_usable_size@)]
pub fn verify_em_buddy_alloc_within(b: &mut BuddyAllocator, size: u64, r: Snapshot<RegionState>) -> Option<u64> {
    proof_assert! { bd_inv(*b) && size@ + b.sector_size@ <= u64::MAX@ };
    let ss = snapshot! { b.sector_size@ };
    let blocks = snapshot! { (size@ + *ss - 1) / *ss };
    let k = snapshot! { ord(*blocks) };
    proof_assert! { lemma_divmod(size@ + *ss - 1, *ss); *blocks >= 0 && *blocks <= 18446744073709551616 };
    proof_assert! { lemma_ord_ge(*blocks); k.pow2() >= *blocks && 0 <= *k };
    proof_assert! { lemma_mul_le(*blocks, k.pow2(), *ss); *blocks * *ss <= k.pow2() * *ss };
    proof_assert! { *blocks * *ss >= size@ };
    b.alloc(size)
}

/// Mutant: claims the block never ends exactly at the region end.
// LEVEL-3 (phase D-B2): the allocator is a region's buddy (rg_inv, PROVED) and the request is that
// region's slab size — the only size its one caller (alloc_extent's new-slab path, region.rs:66-74)
// requests (as EM-BUDDY-ALLOC-ACCOUNTING, D-B1). `bd_inv` and `size + ss <= u64::MAX` are DERIVED
// (rg_buddy; rg_params' slab + ss headroom).
#[requires(rg_inv(*r) && r.buddy == *b && size == r.format_params.slab_size)]
#[ensures(forall<r: u64> result == Some(r) ==>
    r@ + span(b.sector_size@, ord((size@ + b.sector_size@ - 1) / b.sector_size@)) < b.base_offset@ + b.total_usable_size@)]
pub fn verify_em_buddy_alloc_within__mutant(b: &mut BuddyAllocator, size: u64, r: Snapshot<RegionState>) -> Option<u64> {
    proof_assert! { bd_inv(*b) && size@ + b.sector_size@ <= u64::MAX@ };
    b.alloc(size)
}

// =============================================================================
// initialize(): what recovery does NOT validate (refutations)
// =============================================================================

/// `has32` at `p` gives the decoded value (batch 4's trusted from_le_bytes fact).
#[logic]
#[requires(0 <= p && p + 4 <= s.len() && has32(s, p, v))]
#[ensures(d32(s, p) == v)]
pub fn lemma_d32_7(s: Seq<u8>, p: Int, v: u32) {
    pearlite! { lemma_dec32(s.subsequence(p, p + 4), v) }
}

/// `has64` at `p` gives the decoded value.
#[logic]
#[requires(0 <= p && p + 8 <= s.len() && has64(s, p, v))]
#[ensures(d64(s, p) == v)]
pub fn lemma_d64_7(s: Seq<u8>, p: Int, v: u64) {
    pearlite! { lemma_dec64(s.subsequence(p, p + 8), v) }
}

/// A superblock with the layout every chain below uses: sector / slab / max extent 4 bytes, data
/// from byte 0, checkpoint copies of 4096 bytes from byte 4096, active copy 0, instance id 7.
#[ensures(result.magic == SUPERBLOCK_MAGIC && result.version == version && result.data_disk_size == dd)]
#[ensures(result.sector_size@ == 4 && result.slab_size@ == 4 && result.max_extent_size@ == 4)]
#[ensures(result.region_count == rc && result.checkpoint_seq == seq && result.active_copy@ == 0)]
#[ensures(result.checkpoint_region_offset@ == 4096 && result.checkpoint_region_size@ == 4096)]
#[ensures(result.instance_id@ == 7 && result.metadata_disk_ns_id@ == 1 && result.data_start_offset@ == 0)]
pub fn mk_sb7(version: u32, dd: u64, rc: u32, seq: u64) -> Superblock {
    Superblock {
        magic: SUPERBLOCK_MAGIC,
        version,
        data_disk_size: dd,
        sector_size: 4,
        slab_size: 4,
        max_extent_size: 4,
        region_count: rc,
        checkpoint_seq: seq,
        active_copy: 0,
        checkpoint_region_offset: 4096,
        checkpoint_region_size: 4096,
        instance_id: 7,
        metadata_disk_ns_id: 1,
        data_start_offset: 0,
    }
}

/// The superblock sector as written by `Superblock::serialize` (superblock.rs:58-97, the
/// component's own encoder, magic + CRC correct) for `sb`: it decodes to `sb`.
#[requires(sb.magic == SUPERBLOCK_MAGIC)]
#[ensures(sb_ok7(result@) && sb_of(result@) == sb)]
pub fn sb_bytes7(sb: Superblock) -> Vec<u8> {
    let b = sb.serialize();
    let r = deserialize7(&b);
    proof_assert! { r == Ok(sb) };
    b
}

/// A connected metadata device (512-byte sectors, 64 sectors), nothing written yet.
#[ensures(result.connected)]
pub fn dev7() -> MetaDevice {
    MetaDevice { connected: true, sector_size: 512, num_sectors: 64, sb: None, ckpt0: None, ckpt1: None }
}

/// **EM-INIT-ERR-SUPERBLOCK-VERSION** — REFUTED. INDEPENDENT candidate bug, NEW ROOT CAUSE =
/// SB-VERSION-UNCHECKED: superblock.rs:114 reads `version` and :152-167 return it, but nothing
/// compares it with FORMAT_VERSION = 6 (superblock.rs:6) — not deserialize, not recover
/// (recovery.rs:15-21), not initialize (lib.rs:521-581). "If the persisted superblock carries a
/// format version other than the one this code writes, initialize() fails with a CorruptMetadata
/// error instead of interpreting the layout." A superblock sector with version 7 and otherwise
/// this code's layout (magic, CRC32 correct — produced here by the component's own serialize),
/// no checkpoint yet (seq 0), on a connected device: initialize returns Ok and publishes that
/// superblock (version 7) and its layout. Needs a superblock this component did not write
/// (another format version's writer) — exactly the input the requirement is about.
#[ensures(result.0 == Ok(()))]
#[ensures(match result.1.shared { Some(sh) => sh.superblock.version@ == 7 && sh.superblock.version != FORMAT_VERSION, None => false })]
pub fn refute_em_init_err_superblock_version() -> (Result<(), EmError>, ExtentManager) {
    let sb = mk_sb7(7, 8, 1, 0);
    let b = sb_bytes7(sb);
    let mut em = new_inner(dev7());
    proof_assert! { em_regions_wf(em) && sb_sane(sb) };
    let r = em.initialize7(true, true, Ok(b), Err(EmError::IoError), Err(EmError::IoError));
    (r, em)
}

/// Mutant: claims initialize rejects the version-7 superblock.
#[ensures(result == Err(EmError::CorruptMetadata))]
pub fn refute_em_init_err_superblock_version__mutant() -> Result<(), EmError> {
    let sb = mk_sb7(7, 8, 1, 0);
    let b = sb_bytes7(sb);
    let mut em = new_inner(dev7());
    proof_assert! { em_regions_wf(em) && sb_sane(sb) };
    em.initialize7(true, true, Ok(b), Err(EmError::IoError), Err(EmError::IoError))
}

/// `3 & (3 - 1) != 0`: 3 fails format's power-of-two test (lib.rs:397).
#[bitwise_proof]
#[ensures((3u32 & (3u32 - 1u32)) != 0u32)]
pub fn fact_three7() {}

/// LEVEL-3 RETIRED (support only; EM-INIT-ERR-SUPERBLOCK-GEOMETRY is NOT among the 21 level-1
/// refutations, it was excluded at level 1). The level-2 witness — initialize of a CRC-valid
/// superblock with region_count 3 — starts OUTSIDE D-RANGE-FR-002-91a1f6
/// (`sb.region_count.is_power_of_two()`, part of initialize7's precondition `sb_sane` since
/// phase F), so the initialize7 call can no longer be made; what is kept is the input fact:
/// the witness superblock has region_count 3, which format's own power-of-two test rejects
/// (lib.rs:397). (Level-2 text: INIT-SB-GEOMETRY-UNCHECKED — the FR-002 checks of format are
/// not repeated on recovery, recovery.rs:15-21, lib.rs:521-545.)
#[ensures((3u32 & (3u32 - 1u32)) != 0u32)]
#[ensures(result.region_count@ == 3)]
pub fn support_old_refute_em_init_err_superblock_geometry() -> Superblock {
    fact_three7();
    mk_sb7(6, 12, 3, 0)
}

/// Mutant: claims the witness superblock passes format's power-of-two test.
#[ensures((result.region_count & (result.region_count - 1u32)) == 0u32)]
pub fn support_old_refute_em_init_err_superblock_geometry__mutant() -> Superblock {
    fact_three7();
    mk_sb7(6, 12, 3, 0)
}

/// A checkpoint payload that records ZERO regions (checkpoint.rs:186-188: region count 0).
#[ensures(result@.len() == 4 && d32(result@, 0)@ == 0 && ck_complete(result@))]
pub fn payload_zero7() -> Vec<u8> {
    let mut d: Vec<u8> = creusot_std::vec![0u8; 4];
    put32(&mut d, 0, 0u32);
    proof_assert! { lemma_d32_7(d@, 0, 0u32); d32(d@, 0)@ == 0 };
    proof_assert! { end_regions(d@, 4, 0) == Some(4) };
    d
}

/// **EM-INIT-ERR-REGION-COUNT-MISMATCH** — REFUTED. INDEPENDENT candidate bug, NEW ROOT CAUSE =
/// INIT-REGION-COUNT-UNCHECKED: deserialize_slabs returns as many regions as the payload says
/// (checkpoint.rs:186-197) and initialize never compares that with sb.region_count: lib.rs:546-550
/// `if i < per_region_data.len() { .. } else { Vec::new() }` silently invents empty regions for a
/// short list and ignores the extra entries of a long one. "If the recovered checkpoint records a
/// number of regions different from the superblock's region count, initialize() fails with a
/// CorruptMetadata error rather than dropping or inventing regions." A valid superblock with
/// region_count 1 and checkpoint seq 1; the active copy (read_checkpoint_region Ok: header, seq
/// and CRC accepted) holds a payload recording 0 regions: initialize returns Ok with 1 (invented,
/// empty) region. Needs a checkpoint copy this component did not write (it always writes
/// region_count regions, checkpoint.rs:47-55).
#[ensures(result.0 == Ok(()))]
#[ensures(d32(result.2@, 0)@ == 0)]
#[ensures(match (result.1.regions, result.1.shared) {
    (Some(rv), Some(sh)) => rv@.len() == 1 && sh.superblock.region_count@ == 1,
    _ => false,
})]
pub fn refute_em_init_err_region_count_mismatch() -> (Result<(), EmError>, ExtentManager, Vec<u8>) {
    let sb = mk_sb7(6, 8, 1, 1);
    let b = sb_bytes7(sb);
    let d = payload_zero7();
    let dg = snapshot! { d };
    let d2 = payload_zero7();
    let mut em = new_inner(dev7());
    proof_assert! { em_regions_wf(em) && sb_sane(sb) };
    proof_assert! { forall<pr: Seq<Vec<SlabDescriptor>>> parsed(dg@, pr) ==> pr.len() == 0 };
    let r = em.initialize7(true, true, Ok(b), Ok(d), Err(EmError::IoError));
    (r, em, d2)
}

/// Mutant: claims the mismatch is rejected.
#[ensures(result == Err(EmError::CorruptMetadata))]
pub fn refute_em_init_err_region_count_mismatch__mutant() -> Result<(), EmError> {
    let sb = mk_sb7(6, 8, 1, 1);
    let b = sb_bytes7(sb);
    let d = payload_zero7();
    let dg = snapshot! { d };
    let mut em = new_inner(dev7());
    proof_assert! { em_regions_wf(em) && sb_sane(sb) };
    proof_assert! { forall<pr: Seq<Vec<SlabDescriptor>>> parsed(dg@, pr) ==> pr.len() == 0 };
    em.initialize7(true, true, Ok(b), Ok(d), Err(EmError::IoError))
}

/// Writes one slab record with ONE key at `q` (checkpoint.rs:20-30 layout: start, slab size,
/// element size, slot count, keys).
#[requires(q@ + 32 <= buf@.len())]
#[ensures((^buf)@.len() == buf@.len())]
#[ensures(has64((^buf)@, q@, start) && has64((^buf)@, q@ + 8, size) && has32((^buf)@, q@ + 16, elem))]
#[ensures(has32((^buf)@, q@ + 20, 1u32) && has64((^buf)@, q@ + 24, key))]
#[ensures(forall<i: Int> 0 <= i && i < buf@.len() && (i < q@ || i >= q@ + 32) ==> (^buf)@[i] == buf@[i])]
pub fn put_slab1_7(buf: &mut Vec<u8>, q: usize, start: u64, size: u64, elem: u32, key: u64) {
    put64(buf, q, start);
    put64(buf, q + 8, size);
    put32(buf, q + 16, elem);
    put32(buf, q + 20, 1u32);
    put64(buf, q + 24, key);
}

/// A recovered slab record for the dup chain: [4, 8), element 4, one key.
#[logic(open)]
pub fn dup_desc7(s: SlabDescriptor) -> bool {
    pearlite! { s.start_offset@ == 4 && s.slab_size@ == 4 && s.element_size@ == 4 && s.keys@.len() == 1 }
}

/// A checkpoint payload recording ONE region with TWO slab records for the SAME range [4, 8)
/// (start 4, slab 4, element 4, one key 1 each): overlapping slabs.
#[ensures(result@.len() == 72 && ck_complete(result@))]
#[ensures(forall<pr: Seq<Vec<SlabDescriptor>>> parsed(result@, pr) ==>
    pr.len() == 1 && pr[0]@.len() == 2 && dup_desc7(pr[0]@[0]) && dup_desc7(pr[0]@[1]))]
pub fn payload_dup7() -> Vec<u8> {
    let mut d: Vec<u8> = creusot_std::vec![0u8; 72];
    put32(&mut d, 0, 1u32);
    put32(&mut d, 4, 2u32);
    put_slab1_7(&mut d, 8, 4, 4, 4, 1);
    put_slab1_7(&mut d, 40, 4, 4, 4, 1);
    proof_assert! { has32(d@, 0, 1u32) && has32(d@, 4, 2u32) };
    proof_assert! { lemma_d32_7(d@, 0, 1u32); lemma_d32_7(d@, 4, 2u32); d32(d@, 0)@ == 1 && d32(d@, 4)@ == 2 };
    proof_assert! { lemma_d64_7(d@, 8, 4u64); lemma_d64_7(d@, 16, 4u64); lemma_d32_7(d@, 24, 4u32); lemma_d32_7(d@, 28, 1u32);
        d64(d@, 8)@ == 4 && d64(d@, 16)@ == 4 && d32(d@, 24)@ == 4 && d32(d@, 28)@ == 1 };
    proof_assert! { lemma_d64_7(d@, 40, 4u64); lemma_d64_7(d@, 48, 4u64); lemma_d32_7(d@, 56, 4u32); lemma_d32_7(d@, 60, 1u32);
        d64(d@, 40)@ == 4 && d64(d@, 48)@ == 4 && d32(d@, 56)@ == 4 && d32(d@, 60)@ == 1 };
    proof_assert! { end_slabs(d@, 72, 0) == Some(72) };
    proof_assert! { end_slabs(d@, 40, 1) == end_slabs(d@, 72, 0) };
    proof_assert! { end_slabs(d@, 8, 2) == end_slabs(d@, 40, 1) };
    proof_assert! { end_regions(d@, 72, 0) == Some(72) };
    proof_assert! { end_regions(d@, 4, 1) == end_regions(d@, 72, 0) };
    proof_assert! { forall<pr: Seq<Vec<SlabDescriptor>>> parsed(d@, pr) ==> pr.len() == 1 && rg_len(pr, 0) == 0 && region_at(d@, 4, pr[0]@) };
    proof_assert! { forall<ss: Seq<SlabDescriptor>> region_at(d@, 4, ss) ==> ss.len() == 2 && sl_len(ss, 0) == 0 && slab_at(d@, 8, ss[0]) };
    proof_assert! { forall<ss: Seq<SlabDescriptor>> region_at(d@, 4, ss) ==> dup_desc7(ss[0]) && sl_len(ss, 1) == 32 };
    proof_assert! { forall<ss: Seq<SlabDescriptor>> region_at(d@, 4, ss) ==> slab_at(d@, 40, ss[1]) && dup_desc7(ss[1]) };
    d
}

/// The dup descriptor fits region 0 = [0, 8) of `sb` (desc_ok_sb: right slab size, one slot,
/// aligned, inside) — the per-descriptor facts initialize relies on hold; only DISJOINTNESS fails.
#[logic]
#[requires(dup_desc7(s) && sb.slab_size@ == 4 && sb.sector_size@ == 4)]
#[ensures(desc_ok_sb(s, 0, 8, sb))]
pub fn lemma_dup_ok7(s: SlabDescriptor, sb: Superblock) {
    pearlite! { lemma_ord_eq(1, 0, 0) }
}

/// The DUP chain: a valid superblock (one region [0, 8), sector / slab 4, checkpoint seq 1) and an
/// active checkpoint copy (read_checkpoint_region Ok) holding `payload_dup7`; initialize on it.
#[ensures(result.0 == Ok(()) && result.2@.len() == 72)]
#[ensures(forall<pr: Seq<Vec<SlabDescriptor>>> parsed(result.2@, pr) ==>
    pr.len() == 1 && pr[0]@.len() == 2 && dup_desc7(pr[0]@[0]) && dup_desc7(pr[0]@[1]))]
#[ensures(match (result.1.regions, result.1.shared) {
    (Some(rv), Some(sh)) => rv@.len() == 1 && sh.superblock.region_count@ == 1 && sh.superblock.data_disk_size@ == 8,
    _ => false,
})]
pub fn w7_dup() -> (Result<(), EmError>, ExtentManager, Vec<u8>) {
    let sb = mk_sb7(6, 8, 1, 1);
    let b = sb_bytes7(sb);
    let d = payload_dup7();
    let dg = snapshot! { d };
    let d2 = payload_dup7();
    let mut em = new_inner(dev7());
    proof_assert! { em_regions_wf(em) && sb_sane(sb) };
    proof_assert! { forall<s: SlabDescriptor> dup_desc7(s) ==> { lemma_dup_ok7(s, sb); desc_ok_sb(s, 0, 8, sb) } };
    proof_assert! { rbase(0, 8, 1, 0) == 0 && rsize(8, 1, 0) == 8 };
    proof_assert! { forall<pr: Seq<Vec<SlabDescriptor>>> parsed(dg@, pr) ==> recovered_ok(sb, pr) };
    let r = em.initialize7(true, true, Ok(b), Ok(d), Err(EmError::IoError));
    (r, em, d2)
}

/// **EM-INIT-ERR-DESCRIPTOR-INCONSISTENT** — REFUTED. INDEPENDENT candidate bug, NEW ROOT CAUSE =
/// INIT-DESC-UNVALIDATED: no recovered slab descriptor is validated — deserialize_slabs
/// (checkpoint.rs:183-241) checks only lengths, initialize (lib.rs:552-565) calls mark_allocated
/// (which silently ignores a range it cannot find, buddy.rs:117-157) and slab_from_descriptor
/// (recovery.rs:76-85: Slab::new divides by element_size, keys indexed by position) and inserts
/// the slab without any check. "If a recovered checkpoint describes a slab that is internally
/// inconsistent (element size zero, a number of keys different from the number of slots its sizes
/// imply, lying outside its region, or overlapping another slab), initialize() fails with a
/// CorruptMetadata error." The DUP chain: a CRC-accepted checkpoint copy describing two slabs
/// over the SAME range [4, 8) of region [0, 8): initialize returns Ok. (Element size zero instead
/// panics in Slab::new — also not CorruptMetadata.) Needs a checkpoint copy this component did not
/// write — the input the requirement is about.
#[ensures(result.0 == Ok(()))]
#[ensures(forall<pr: Seq<Vec<SlabDescriptor>>> parsed(result.1@, pr) ==>
    pr[0]@.len() == 2 && pr[0]@[0].start_offset == pr[0]@[1].start_offset && pr[0]@[0].slab_size@ == 4 && pr[0]@[1].slab_size@ == 4)]
pub fn refute_em_init_err_descriptor_inconsistent() -> (Result<(), EmError>, Vec<u8>) {
    let (r, _em, d) = w7_dup();
    (r, d)
}

/// Mutant: claims the overlapping descriptors are rejected.
#[ensures(result == Err(EmError::CorruptMetadata))]
pub fn refute_em_init_err_descriptor_inconsistent__mutant() -> Result<(), EmError> {
    let (r, _em, _d) = w7_dup();
    r
}

/// **EM-BUDDY-MARK-ALLOCATED-PRE** — REFUTED. Same root cause INIT-DESC-UNVALIDATED (independent
/// of BUDDY-MASK: power-of-two sector size 4). "A range is only marked allocated when it lies in
/// the region and is currently entirely free." lib.rs:552-555 calls `buddy.mark_allocated(desc.
/// start_offset, desc.slab_size)` for every recovered descriptor in order, with no check. On the
/// DUP chain's input (initialize returns Ok, so the rebuild runs): BuddyAllocator::new(0, 8, 4) +
/// the first mark_allocated(4, 4) (batch 2's `w1p_recovered`) leave only [0, 4) free, and the
/// second descriptor's mark_allocated(4, 4) is then made on [4, 8), which holds no free byte; the
/// call is a silent no-op (buddy.rs:157).
#[ensures(result.0 == Ok(()))]
#[ensures(bd_inv(result.1) && result.1.base_offset@ == 0 && result.1.total_usable_size@ == 8 && result.1.free_lists@.len() == 2)]
#[ensures(forall<o: Int, k: Int> 0 <= o && o < 2 && 0 <= k && k < result.1.free_lists@[o]@.len() ==>
    result.1.free_lists@[o]@[k]@ + span(4, o) <= 4 || result.1.free_lists@[o]@[k]@ >= 8)]
#[ensures(*result.2 == result.1)]
pub fn support_old_refute_em_buddy_mark_allocated_pre() -> (Result<(), EmError>, BuddyAllocator, Snapshot<BuddyAllocator>) {
    let (r, _em, _d) = w7_dup();
    let (_y, mut b1) = w1p_recovered();
    proof_assert! { span(4, 0) == 4 && span(4, 1) == 8 };
    proof_assert! { lemma_ord_eq(1, 0, 0); ord(4 / 4) == 0 };
    let before = snapshot! { b1 };
    b1.mark_allocated(4, 4);
    (r, b1, before)
}

/// Mutant: claims a free block covers [4, 8) when the second descriptor is marked.
#[ensures(exists<o: Int, k: Int> 0 <= o && o < 2 && 0 <= k && k < result.free_lists@[o]@.len() &&
    result.free_lists@[o]@[k]@ <= 4 && 8 <= result.free_lists@[o]@[k]@ + span(4, o))]
pub fn support_old_refute_em_buddy_mark_allocated_pre__mutant() -> BuddyAllocator {
    let (_y, b1) = w1p_recovered();
    proof_assert! { span(4, 0) == 4 && span(4, 1) == 8 };
    b1
}
