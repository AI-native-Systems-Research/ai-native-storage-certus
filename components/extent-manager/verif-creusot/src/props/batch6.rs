//! Batch-6 property proof units (`verify_<id>` / `refute_<id>`, each with a failing `__mutant`
//! twin), unscored witnesses (`witness_<id>_*`) and helpers.
use crate::model::b3::*;
use crate::model::b4::*;
use crate::model::b6::*;
use crate::model::bitmap::*;
use crate::model::buddy::*;
use crate::model::ckpt::*;
use crate::model::component::*;
use crate::model::listing::*;
use crate::model::params::*;
use crate::model::region::*;
use crate::model::slab::*;
use crate::props::batch1::*;
use crate::props::batch2::*;
use crate::props::batch4::*;
use crate::props::batch5::*;
use crate::props::witness::*;
use crate::model::assume::*;
use creusot_std::prelude::*;

// =============================================================================
// Read-only accessors and defaults
// =============================================================================

/// **EM-CAP-UNINIT-ZERO** — "Before the component is formatted or initialized, capacity_bytes()
/// returns zero." `new_inner` (lib.rs:110-171) leaves `regions` None (lib.rs:94, `Default`);
/// capacity_bytes (lib.rs:710-715) maps None to 0. (a) a freshly constructed component reports
/// 0; (b) so does any component whose regions are not (yet) published — a failed format leaves
/// them unchanged (component.rs `format`, Err clause).
#[requires(em_regions_wf(*em) && em.regions == None)]
#[ensures(result.0@ == 0 && result.1@ == 0)]
pub fn verify_em_cap_uninit_zero(dev: MetaDevice, em: &ExtentManager) -> (u64, u64) {
    let e0 = new_inner(dev);
    proof_assert! { em_regions_wf(e0) };
    let c0 = e0.capacity_bytes();
    let c1 = em.capacity_bytes();
    (c0, c1)
}

/// Mutant: claims a fresh component reports a non-zero capacity.
/// LEVEL-3 (phase D-B2): mutant with EXACTLY the property's requires and body; it claims the
/// negation of the property's first postcondition.
#[requires(em_regions_wf(*em) && em.regions == None)]
#[ensures(!(result.0@ == 0 && result.1@ == 0))]
pub fn verify_em_cap_uninit_zero__mutant(dev: MetaDevice, em: &ExtentManager) -> (u64, u64) {
    let e0 = new_inner(dev);
    proof_assert! { em_regions_wf(e0) };
    let c0 = e0.capacity_bytes();
    let c1 = em.capacity_bytes();
    (c0, c1)
}

/// **EM-DBASE-FRAME-READ-ONLY** — "Calling data_base_lba() changes no stored configuration,
/// extent or on-disk metadata." `data_base_lba` (lib.rs:725-727) only locks and reads the
/// `data_base_lba` mutex: the whole component state (regions, shared state, configuration, the
/// metadata device) is unchanged and the stored value is returned.
#[ensures(^em == *em)]
#[ensures(result == em.data_base_lba)]
pub fn verify_em_dbase_frame_read_only(em: &mut ExtentManager) -> u64 {
    em.data_base_lba()
}

/// Mutant: claims the call changes the stored value.
#[ensures((^em).data_base_lba != em.data_base_lba)]
pub fn verify_em_dbase_frame_read_only__mutant(em: &mut ExtentManager) -> u64 {
    em.data_base_lba()
}

/// **EM-DBASE-POST-DEFAULT-ZERO** — "If set_data_base_lba() has never been called,
/// data_base_lba() returns 0." `new_inner` builds the field with `Default` (lib.rs:99 ->
/// `Mutex<u64>` = 0); every other mutating method leaves it unchanged: set_metadata_base_lba,
/// format, initialize, and each call between them (reserve / publish / abort / remove /
/// checkpoint — batch 1's `apply_cop` frame). So on a fresh component, after any of them,
/// data_base_lba() is 0. (set_checkpoint_interval, set_dma_alloc, set_metadata_ns_id and the
/// read-only methods do not touch it: lib.rs:173-187, 694-696.)
// LEVEL-3 (phase D-B2): the intervening format() is any format call within format's own declared
// ranges (2ee144, 67ea4f, fmt_sane = e0fe79 + 3b58ea; no 7cb7c3 — format accepts slab_size 0); the
// intervening initialize() any call within initialize's ranges (sb_sane, recovered_ok; per_region
// taken abstractly — decoder fact t_dec_keys); the intervening COp one call under the PROVED em_ok
// and cop_pre.
#[requires(em_ok(*em) && cop_pre(*em, op) && em.data_base_lba@ == 0)]
#[requires(em_regions_wf(*f) && f.data_base_lba@ == 0)]
#[requires(f.dev.connected ==> a_67ea4f(f.dev.num_sectors@, f.dev.sector_size@))]
#[requires(a_2ee144(params.metadata_alignment@) && fmt_sane(params))]
#[requires(em_regions_wf(*g) && g.data_base_lba@ == 0 && sb_sane(sb) && recovered_ok(sb, per_region@))]
#[ensures(result.0@ == 0)]
#[ensures(result.1@ == 0 && result.2@ == 0 && result.3@ == 0 && result.4@ == 0)]
pub fn verify_em_dbase_post_default_zero(
    dev: MetaDevice, b: u64, em: &mut ExtentManager, op: COp, f: &mut ExtentManager, params: FormatParams,
    g: &mut ExtentManager, sb: Superblock, per_region: &Vec<Vec<SlabDescriptor>>,
) -> (u64, u64, u64, u64, u64) {
    let mut e0 = new_inner(dev);
    let d0 = e0.data_base_lba();
    e0.set_metadata_base_lba(b);
    let d1 = e0.data_base_lba();
    apply_cop(em, op);
    let d2 = em.data_base_lba();
    let _ = f.format(params);
    let d3 = f.data_base_lba();
    g.initialize(sb, per_region);
    let d4 = g.data_base_lba();
    (d0, d1, d2, d3, d4)
}

/// Mutant: claims a fresh component reports base LBA 1.
/// (same requires as the property)
// LEVEL-3 (phase D-B2): the intervening format() is any format call within format's own declared
// ranges (2ee144, 67ea4f, fmt_sane = e0fe79 + 3b58ea; no 7cb7c3 — format accepts slab_size 0); the
// intervening initialize() any call within initialize's ranges (sb_sane, recovered_ok; per_region
// taken abstractly — decoder fact t_dec_keys); the intervening COp one call under the PROVED em_ok
// and cop_pre.
#[requires(em_ok(*em) && cop_pre(*em, op) && em.data_base_lba@ == 0)]
#[requires(em_regions_wf(*f) && f.data_base_lba@ == 0)]
#[requires(f.dev.connected ==> a_67ea4f(f.dev.num_sectors@, f.dev.sector_size@))]
#[requires(a_2ee144(params.metadata_alignment@) && fmt_sane(params))]
#[requires(em_regions_wf(*g) && g.data_base_lba@ == 0 && sb_sane(sb) && recovered_ok(sb, per_region@))]
#[ensures(result.0@ == 1)]
pub fn verify_em_dbase_post_default_zero__mutant(
    dev: MetaDevice, b: u64, em: &mut ExtentManager, op: COp, f: &mut ExtentManager, params: FormatParams,
    g: &mut ExtentManager, sb: Superblock, per_region: &Vec<Vec<SlabDescriptor>>,
) -> (u64, u64, u64, u64, u64) {
    let mut e0 = new_inner(dev);
    let d0 = e0.data_base_lba();
    e0.set_metadata_base_lba(b);
    let d1 = e0.data_base_lba();
    apply_cop(em, op);
    let d2 = em.data_base_lba();
    let _ = f.format(params);
    let d3 = f.data_base_lba();
    g.initialize(sb, per_region);
    let d4 = g.data_base_lba();
    (d0, d1, d2, d3, d4)
}

// =============================================================================
// for_each_extent
// =============================================================================

/// **EM-FOREACH-FRAME-READ-ONLY** — "Calling for_each_extent() does not itself change any
/// extent, any reservation, used_bytes(), or the on-disk metadata." for_each_extent
/// (lib.rs:658-678) takes the regions read lock and each region's read lock and only reads the
/// slabs: the whole component state — every region (slabs, keys, bitmaps, buddy free lists, so
/// reservations and used_bytes), the shared state and the metadata device — is unchanged.
// LEVEL-3 (phase D-B2): only the part of the region invariant the listing reads (every slab
// well-formed, em_list_ok — implied by the PROVED em_ok, lemma_em_list); no headroom conjunct.
#[requires(em_regions_wf(*em) && em_list_ok(*em))]
#[ensures(^em == *em)]
pub fn verify_em_foreach_frame_read_only(em: &mut ExtentManager, cb: &mut Vec<Extent>) {
    em.for_each_extent(cb);
}

/// Mutant: claims the call changes the metadata device.
// LEVEL-3 (phase D-B2): only the part of the region invariant the listing reads (every slab
// well-formed, em_list_ok — implied by the PROVED em_ok, lemma_em_list); no headroom conjunct.
#[requires(em_regions_wf(*em) && em_list_ok(*em))]
#[ensures((^em).dev != em.dev)]
pub fn verify_em_foreach_frame_read_only__mutant(em: &mut ExtentManager, cb: &mut Vec<Extent>) {
    em.for_each_extent(cb);
}

/// **EM-FOREACH-UNINIT-NO-CALLS** — "Before the component is formatted or initialized,
/// for_each_extent() never calls the callback." lib.rs:660 `if let Some(regions)`: with
/// `regions` None (a fresh component, lib.rs:94) the callback observes nothing.
#[requires(em_regions_wf(*em) && em_regions_ok(*em) && em.regions == None)]
#[ensures((^cb0)@ == cb0@)]
#[ensures((^cb1)@.len() == cb1@.len())]
pub fn verify_em_foreach_uninit_no_calls(dev: MetaDevice, cb0: &mut Vec<Extent>, em: &ExtentManager, cb1: &mut Vec<Extent>) {
    let e0 = new_inner(dev);
    proof_assert! { em_regions_wf(e0) && em_regions_ok(e0) };
    e0.for_each_extent(cb0);
    proof_assert! { forall<p: Int> 0 <= p && p < cb0@.len() ==> (^cb0)@[p] == cb0@[p] };
    proof_assert! { (^cb0)@.ext_eq(cb0@) };
    em.for_each_extent(cb1);
}

/// Mutant: claims the callback is called at least once.
/// LEVEL-3 (phase D-B2): mutant with EXACTLY the property's requires and body; it claims the
/// negation of the property's first postcondition.
#[requires(em_regions_wf(*em) && em_regions_ok(*em) && em.regions == None)]
#[ensures(!((^cb0)@ == cb0@))]
pub fn verify_em_foreach_uninit_no_calls__mutant(dev: MetaDevice, cb0: &mut Vec<Extent>, em: &ExtentManager, cb1: &mut Vec<Extent>) {
    let e0 = new_inner(dev);
    proof_assert! { em_regions_wf(e0) && em_regions_ok(e0) };
    e0.for_each_extent(cb0);
    proof_assert! { forall<p: Int> 0 <= p && p < cb0@.len() ==> (^cb0)@[p] == cb0@[p] };
    proof_assert! { (^cb0)@.ext_eq(cb0@) };
    em.for_each_extent(cb1);
}

/// **EM-FOREACH-POST-SAME-AS-GET** — "for_each_extent(callback) invokes the callback exactly once
/// for each extent that get_extents() would return at the same moment, with the same key,
/// offset and size, and for no other extent." Both methods walk the same regions, slabs (BTreeMap
/// iteration, trusted `keys_det`: the order is a function of the map's contents) and slots in
/// the same order with the same FREE_KEY filter (lib.rs:632-656 vs 658-678); on the same state
/// the callback's observation sequence IS get_extents' result vector, element by element.
// LEVEL-3 (phase D-B2): only the part of the region invariant the listing reads (every slab
// well-formed, em_list_ok — implied by the PROVED em_ok, lemma_em_list); no headroom conjunct.
#[requires(em_regions_wf(*em) && em_list_ok(*em))]
#[ensures(forall<p: Int> 0 <= p && p < cb@.len() ==> (^cb)@[p] == cb@[p])]
#[ensures((^cb)@.len() == cb@.len() + result@.len())]
#[ensures(forall<p: Int> 0 <= p && p < result@.len() ==> (^cb)@[cb@.len() + p] == result@[p])]
pub fn verify_em_foreach_post_same_as_get(em: &ExtentManager, cb: &mut Vec<Extent>) -> Vec<Extent> {
    let c0 = snapshot! { *cb };
    let g = em.get6();
    em.for_each6(cb);
    proof_assert! { match em.regions {
        Some(rv) => { lemma_matches_eq(g@, 0, cb@, c0@.len(), em_tris(em.arena@, rv@, rv@.len()));
            cb@.len() == c0@.len() + g@.len() },
        None => cb@.len() == c0@.len() + g@.len() } };
    g
}

/// Mutant: claims the callback is called once more than get_extents lists.
// LEVEL-3 (phase D-B2): only the part of the region invariant the listing reads (every slab
// well-formed, em_list_ok — implied by the PROVED em_ok, lemma_em_list); no headroom conjunct.
#[requires(em_regions_wf(*em) && em_list_ok(*em))]
#[ensures((^cb)@.len() == cb@.len() + result@.len() + 1)]
pub fn verify_em_foreach_post_same_as_get__mutant(em: &ExtentManager, cb: &mut Vec<Extent>) -> Vec<Extent> {
    let g = em.get6();
    em.for_each6(cb);
    g
}

// =============================================================================
// format() error cases (model/b6.rs format6: lib.rs:383-481 + component.rs format)
// =============================================================================

/// **EM-FORMAT-ERR-DEVICE-NOT-CONNECTED** — "If no metadata block device has been connected,
/// format() fails with a NotInitialized error and leaves the component's state unchanged."
/// lib.rs:406-409 (`metadata_device.get()` -> not_initialized), reached once the FR-002
/// parameter checks (lib.rs:384-401) pass — an invalid parameter set is rejected first with
/// CorruptMetadata. Nothing is written or published before (the first effect is lib.rs:498).
// LEVEL-3 (phase D-B2): format6's requires are path-wise, so the alignment / device-size / slab
// ranges (never reached on this path) are gone. `fr002(params)` stays: the code tests the
// parameters FIRST (lib.rs:384-401), so for invalid parameters the claim is false
// (CorruptMetadata) — a precedence premise (advisory: NOT CREDITED).
#[requires(em_regions_wf(*em) && !em.dev.connected && fr002(params))]
#[ensures(result == Err(EmError::NotInitialized) && ^em == *em)]
pub fn narrowed_verify_em_format_err_device_not_connected(
    em: &mut ExtentManager, params: FormatParams, qn: Option<u64>, qs: Option<u32>, rnd: Option<u64>,
) -> Result<(), EmError> {
    em.format6(params, qn, qs, rnd)
}

/// Mutant: claims the error is IoError.
// LEVEL-3 (phase D-B2): format6's requires are path-wise, so the alignment / device-size / slab
// ranges (never reached on this path) are gone. `fr002(params)` stays: the code tests the
// parameters FIRST (lib.rs:384-401), so for invalid parameters the claim is false
// (CorruptMetadata) — a precedence premise (advisory: NOT CREDITED).
#[requires(em_regions_wf(*em) && !em.dev.connected && fr002(params))]
#[ensures(result == Err(EmError::IoError))]
pub fn narrowed_verify_em_format_err_device_not_connected__mutant(
    em: &mut ExtentManager, params: FormatParams, qn: Option<u64>, qs: Option<u32>, rnd: Option<u64>,
) -> Result<(), EmError> {
    em.format6(params, qn, qs, rnd)
}

/// **EM-FORMAT-ERR-DEVICE-QUERY** — "If the metadata device cannot report its sector count or
/// sector size for the configured metadata namespace, format() fails with an IoError and leaves
/// the component's state unchanged." lib.rs:410-415: `num_sectors(ns)` / `sector_size(ns)`
/// errors map through `nvme_to_em` to IoError (error.rs:23-25); nothing has been changed yet.
// LEVEL-3 (phase D-B2): path-wise format6 — the device-size / alignment / slab ranges are gone.
// `fr002(params)` stays (precedence: invalid parameters are rejected first, lib.rs:384-401).
#[requires(em_regions_wf(*em) && em.dev.connected && fr002(params))]
#[requires(qn == None || qs == None)]
#[ensures(result == Err(EmError::IoError) && ^em == *em)]
pub fn narrowed_verify_em_format_err_device_query(
    em: &mut ExtentManager, params: FormatParams, qn: Option<u64>, qs: Option<u32>, rnd: Option<u64>,
) -> Result<(), EmError> {
    em.format6(params, qn, qs, rnd)
}

/// Mutant: claims the error is NotInitialized.
// LEVEL-3 (phase D-B2): path-wise format6 — the device-size / alignment / slab ranges are gone.
// `fr002(params)` stays (precedence: invalid parameters are rejected first, lib.rs:384-401).
#[requires(em_regions_wf(*em) && em.dev.connected && fr002(params))]
#[requires(qn == None || qs == None)]
#[ensures(result == Err(EmError::NotInitialized))]
pub fn narrowed_verify_em_format_err_device_query__mutant(
    em: &mut ExtentManager, params: FormatParams, qn: Option<u64>, qs: Option<u32>, rnd: Option<u64>,
) -> Result<(), EmError> {
    em.format6(params, qn, qs, rnd)
}

/// The layout checks of lib.rs:430-452 pass for the device's answers (n sectors of s bytes).
#[logic(open)]
pub fn layout_ok6(params: FormatParams, n: u64, s: u32) -> bool {
    pearlite! {
        cksize_l(n@ * s@, params.metadata_region_size@, params.metadata_alignment@, params.sector_size@) > 0
        && params.data_disk_size@ > ds_l(n@ * s@, params.metadata_region_size@, params.metadata_alignment@, params.sector_size@)
    }
}

/// **EM-FORMAT-ERR-INSTANCE-ID-GENERATION** — "When no instance id is supplied and a random one
/// cannot be generated, format() fails with an IoError and leaves the component's state
/// unchanged." lib.rs:471-481: opening or reading /dev/urandom fails -> io_error(..)?, before the
/// superblock write (lib.rs:498) and the publishes (lib.rs:506-507). (Reached once every earlier
/// check passed; disclosed assumption metadata_alignment + 4096 <= u64::MAX, see
/// EM-FORMAT-ERR-METADATA-TOO-SMALL.)
// LEVEL-3 (phase D-B2): the format ranges are format's own (2ee144, 67ea4f, fmt_sane = e0fe79 +
// 3b58ea; no 7cb7c3). The precedence premises (fr002, a connected device that answers, the layout
// checks) stay: every earlier failing check returns its own error first (advisory: NOT CREDITED).
#[requires(em_regions_wf(*em) && em.dev.connected)]
#[requires(a_67ea4f(em.dev.num_sectors@, em.dev.sector_size@))]
#[requires(a_2ee144(params.metadata_alignment@) && fmt_sane(params) && fr002(params))]
#[requires(match (qn, qs) { (Some(n), Some(s)) => a_67ea4f(n@, s@) && layout_ok6(params, n, s), _ => false })]
#[requires(params.instance_id == None && rnd == None)]
#[ensures(result == Err(EmError::IoError) && ^em == *em)]
pub fn narrowed_verify_em_format_err_instance_id_generation(
    em: &mut ExtentManager, params: FormatParams, qn: Option<u64>, qs: Option<u32>, rnd: Option<u64>,
) -> Result<(), EmError> {
    em.format6(params, qn, qs, rnd)
}

/// Mutant: claims format still succeeds.
// LEVEL-3 (phase D-B2): the format ranges are format's own (2ee144, 67ea4f, fmt_sane = e0fe79 +
// 3b58ea; no 7cb7c3). The precedence premises (fr002, a connected device that answers, the layout
// checks) stay: every earlier failing check returns its own error first (advisory: NOT CREDITED).
#[requires(em_regions_wf(*em) && em.dev.connected)]
#[requires(a_67ea4f(em.dev.num_sectors@, em.dev.sector_size@))]
#[requires(a_2ee144(params.metadata_alignment@) && fmt_sane(params) && fr002(params))]
#[requires(match (qn, qs) { (Some(n), Some(s)) => a_67ea4f(n@, s@) && layout_ok6(params, n, s), _ => false })]
#[requires(params.instance_id == None && rnd == None)]
#[ensures(result == Ok(()))]
pub fn narrowed_verify_em_format_err_instance_id_generation__mutant(
    em: &mut ExtentManager, params: FormatParams, qn: Option<u64>, qs: Option<u32>, rnd: Option<u64>,
) -> Result<(), EmError> {
    em.format6(params, qn, qs, rnd)
}

/// **EM-FORMAT-ERR-MAX-EXTENT-EXCEEDS-SLAB** — "When format() is called with a maximum extent
/// size strictly greater than the slab size, it returns a CorruptMetadata error; a maximum extent
/// size equal to the slab size is accepted." (a) lib.rs:392-396 (every earlier check also returns
/// CorruptMetadata): max > slab -> CorruptMetadata, state unchanged. (b) max == slab passes that
/// check: on the concrete valid input class `fmt_ok_inputs` (FR-002 parameters, a connected
/// metadata device with room, instance id given) format returns Ok or — only from the device —
/// IoError, never CorruptMetadata.
// LEVEL-3 (phase D-B2): (a) for EVERY component, device and parameter set with max > slab (no
// range: the max-extent check precedes every arithmetic, and every earlier check also returns
// CorruptMetadata); (b) WIDENED from the one-region / separate-device / explicit-id input class
// to EVERY parameter set with max == slab and every device answer: the max-extent check does not
// fire — format's validation (lib.rs:383-481) has exactly the outcome it has for the same
// parameters with max_extent_size 0 (which no check can reject for its size).
#[requires(em_regions_wf(*em) && params.max_extent_size@ > params.slab_size@)]
#[requires(p2.max_extent_size@ == p2.slab_size@)]
#[requires(match (q2n, q2s) { (Some(n), Some(s)) => fr002(p2) && d2.connected ==>
    a_67ea4f(n@, s@) && a_2ee144(p2.metadata_alignment@), _ => true })]
#[ensures(result.0 == Err(EmError::CorruptMetadata) && ^em == *em)]
#[ensures(result.1 == result.2)]
pub fn verify_em_format_err_max_extent_exceeds_slab(
    em: &mut ExtentManager, params: FormatParams, qn: Option<u64>, qs: Option<u32>, rnd: Option<u64>,
    d2: &MetaDevice, p2: FormatParams, q2n: Option<u64>, q2s: Option<u32>, r2: Option<u64>,
) -> (Result<(), EmError>, Result<u64, EmError>, Result<u64, EmError>) {
    let a = em.format6(params, qn, qs, rnd);
    let p0 = FormatParams { max_extent_size: 0u32, ..p2 };
    proof_assert! { fr002(p0) == fr002(p2) };
    let b = format_pre6(d2, p2, q2n, q2s, r2);
    let c = format_pre6(d2, p0, q2n, q2s, r2);
    (a, b, c)
}

/// Mutant: claims max == slab is rejected with CorruptMetadata.
// LEVEL-3 (phase D-B2): (a) for EVERY component, device and parameter set with max > slab (no
// range: the max-extent check precedes every arithmetic, and every earlier check also returns
// CorruptMetadata); (b) WIDENED from the one-region / separate-device / explicit-id input class
// to EVERY parameter set with max == slab and every device answer: the max-extent check does not
// fire — format's validation (lib.rs:383-481) has exactly the outcome it has for the same
// parameters with max_extent_size 0 (which no check can reject for its size).
#[requires(em_regions_wf(*em) && params.max_extent_size@ > params.slab_size@)]
#[requires(p2.max_extent_size@ == p2.slab_size@)]
#[requires(match (q2n, q2s) { (Some(n), Some(s)) => fr002(p2) && d2.connected ==>
    a_67ea4f(n@, s@) && a_2ee144(p2.metadata_alignment@), _ => true })]
#[ensures(result.1 == Err(EmError::CorruptMetadata))]
pub fn verify_em_format_err_max_extent_exceeds_slab__mutant(
    em: &mut ExtentManager, params: FormatParams, qn: Option<u64>, qs: Option<u32>, rnd: Option<u64>,
    d2: &MetaDevice, p2: FormatParams, q2n: Option<u64>, q2s: Option<u32>, r2: Option<u64>,
) -> (Result<(), EmError>, Result<u64, EmError>, Result<u64, EmError>) {
    let a = em.format6(params, qn, qs, rnd);
    let p0 = FormatParams { max_extent_size: 0u32, ..p2 };
    let b = format_pre6(d2, p2, q2n, q2s, r2);
    let c = format_pre6(d2, p0, q2n, q2s, r2);
    (a, b, c)
}

/// **EM-FORMAT-ERR-NO-USABLE-DATA** — "If no data space remains after the metadata reservation
/// (the data disk is not larger than the data start offset), format() fails with a
/// CorruptMetadata error." lib.rs:442-452: data_start_offset = 0 (separate devices) or
/// checkpoint_region_offset + 2 * checkpoint_region_size (shared device); `saturating_sub` and
/// the `usable_data_size == 0` test. Reached after the earlier checks pass; the state is unchanged.
/// (Disclosed assumption metadata_alignment + 4096 <= u64::MAX, see EM-FORMAT-ERR-METADATA-TOO-SMALL.)
// LEVEL-3 (phase D-B2): path-wise format6 — `fr002` and the slab ranges are gone (an invalid
// parameter set is rejected with the same CorruptMetadata; the slab geometry is never read on
// this path). The device answered and the checkpoint reservation succeeded (the statement's
// "after the metadata reservation"); 2ee144 / 67ea4f are the ranges of the arithmetic it reaches.
#[requires(em_regions_wf(*em) && em.dev.connected)]
#[requires(match (qn, qs) { (Some(n), Some(s)) => (fr002(params) ==> a_67ea4f(n@, s@) && a_2ee144(params.metadata_alignment@))
    && cksize_l(n@ * s@, params.metadata_region_size@, params.metadata_alignment@, params.sector_size@) > 0
    && params.data_disk_size@ <= ds_l(n@ * s@, params.metadata_region_size@, params.metadata_alignment@, params.sector_size@), _ => false })]
#[ensures(result == Err(EmError::CorruptMetadata) && ^em == *em)]
pub fn verify_em_format_err_no_usable_data(
    em: &mut ExtentManager, params: FormatParams, qn: Option<u64>, qs: Option<u32>, rnd: Option<u64>,
) -> Result<(), EmError> {
    em.format6(params, qn, qs, rnd)
}

/// Mutant: claims the error is IoError.
// LEVEL-3 (phase D-B2): path-wise format6 — `fr002` and the slab ranges are gone (an invalid
// parameter set is rejected with the same CorruptMetadata; the slab geometry is never read on
// this path). The device answered and the checkpoint reservation succeeded (the statement's
// "after the metadata reservation"); 2ee144 / 67ea4f are the ranges of the arithmetic it reaches.
#[requires(em_regions_wf(*em) && em.dev.connected)]
#[requires(match (qn, qs) { (Some(n), Some(s)) => (fr002(params) ==> a_67ea4f(n@, s@) && a_2ee144(params.metadata_alignment@))
    && cksize_l(n@ * s@, params.metadata_region_size@, params.metadata_alignment@, params.sector_size@) > 0
    && params.data_disk_size@ <= ds_l(n@ * s@, params.metadata_region_size@, params.metadata_alignment@, params.sector_size@), _ => false })]
#[ensures(result == Err(EmError::IoError))]
pub fn verify_em_format_err_no_usable_data__mutant(
    em: &mut ExtentManager, params: FormatParams, qn: Option<u64>, qs: Option<u32>, rnd: Option<u64>,
) -> Result<(), EmError> {
    em.format6(params, qn, qs, rnd)
}

/// `align_up(4096, a) >= 4096` whenever it does not overflow (lib.rs:420-424).
#[logic]
#[requires(a >= 0)]
#[ensures(ckoff_l(a) >= 4096)]
pub fn lemma_ckoff_ge(a: Int) {
    pearlite! { if a == 0 { () } else { lemma_divmod(4096 + a - 1, a) } }
}

/// `x < 2 * s ==> (x / 2) / s * s == 0` for `s > 0`.
#[logic]
#[requires(s > 0 && 0 <= x && x < 2 * s)]
#[ensures((x / 2) / s * s == 0)]
pub fn lemma_small_half(x: Int, s: Int) {
    pearlite! { { lemma_divmod(x, 2); lemma_divmod(x / 2, s) } }
}

/// Unscored positive half of EM-FORMAT-ERR-METADATA-TOO-SMALL (bounded alignment, the
/// assumption every other format proof of this crate carries): when the metadata space after
/// the 4096-byte superblock (capped by a non-zero metadata_region_size) is smaller than two data
/// sectors, format returns CorruptMetadata and changes nothing.
#[requires(em_regions_wf(*em) && em.dev.connected)]
#[requires(a_67ea4f(em.dev.num_sectors@, em.dev.sector_size@))]
#[requires(a_2ee144(params.metadata_alignment@) && sane_params(params) && fr002(params))]
#[requires(match (qn, qs) { (Some(n), Some(s)) => a_67ea4f(n@, s@)
    && eff_l(n@ * s@, params.metadata_region_size@) < 4096 + 2 * params.sector_size@, _ => false })]
#[ensures(result == Err(EmError::CorruptMetadata) && ^em == *em)]
pub fn witness_em_format_err_metadata_too_small_bounded(
    em: &mut ExtentManager, params: FormatParams, qn: Option<u64>, qs: Option<u32>, rnd: Option<u64>,
) -> Result<(), EmError> {
    proof_assert! { lemma_ckoff_ge(params.metadata_alignment@); ckoff_l(params.metadata_alignment@) >= 4096 };
    proof_assert! { match (qn, qs) { (Some(n), Some(s)) =>
        eff_l(n@ * s@, params.metadata_region_size@) >= ckoff_l(params.metadata_alignment@) ==> {
            lemma_small_half(eff_l(n@ * s@, params.metadata_region_size@) - ckoff_l(params.metadata_alignment@), params.sector_size@);
            cksize_l(n@ * s@, params.metadata_region_size@, params.metadata_alignment@, params.sector_size@) == 0 },
        _ => true } };
    proof_assert! { match (qn, qs) { (Some(n), Some(s)) =>
        cksize_l(n@ * s@, params.metadata_region_size@, params.metadata_alignment@, params.sector_size@) == 0, _ => true } };
    em.format6(params, qn, qs, rnd)
}

/// **EM-FORMAT-ERR-METADATA-TOO-SMALL** — PROVED (LEVEL-3, under the CORRECTED
/// D-RANGE-FMT-METADATA-DEVICE-LAYOUT-2ee144 `metadata_alignment <= u64::MAX - SUPERBLOCK_SIZE`
/// and D-RANGE-FR-002-67ea4f, the metadata-device byte size lib.rs:410-415 multiplies; the
/// level-2 refutation at u64::MAX - 4095 is outside the corrected range and retired).
/// "When the metadata space available after the superblock (capped by the metadata region
/// size when that is non-zero) is too small to hold two checkpoint copies of at least one
/// sector each, format() returns a CorruptMetadata error." For a connected metadata device
/// that answers its size queries (the space is then defined), with any parameters: if the
/// FR-002 checks fail format already returns CorruptMetadata (lib.rs:384-401); otherwise the
/// checkpoint offset is >= 4096 (`lemma_ckoff_ge`), so fewer than two sectors remain and
/// lib.rs:430-438 returns CorruptMetadata. Nothing changes. No other premise: em_regions_wf is
/// a PROVED invariant (invariants.yaml); the slab-geometry ranges format needs to BUILD regions
/// are not reached on this path (format6 requires them only under `fmt6_passes`).
// LEVEL-3 (phase D-B2): format6's requires are path-wise — the receptacle's own size (a_67ea4f of
// em.dev) is never needed on this path and is dropped; 2ee144 / 67ea4f are required only where
// the code reaches their arithmetic (the FR-002 checks passed).
#[requires(em_regions_wf(*em))]
#[requires(fr002(params) ==> a_2ee144(params.metadata_alignment@) && a_67ea4f(n@, s@))]
#[requires(em.dev.connected && eff_l(n@ * s@, params.metadata_region_size@) < 4096 + 2 * params.sector_size@)]
#[ensures(result == Err(EmError::CorruptMetadata) && ^em == *em)]
pub fn verify_em_format_err_metadata_too_small(
    em: &mut ExtentManager, params: FormatParams, n: u64, s: u32, rnd: Option<u64>,
) -> Result<(), EmError> {
    proof_assert! { lemma_ckoff_ge(params.metadata_alignment@); ckoff_l(params.metadata_alignment@) >= 4096 };
    proof_assert! { fr002(params) && eff_l(n@ * s@, params.metadata_region_size@) >= ckoff_l(params.metadata_alignment@) ==> {
        lemma_small_half(eff_l(n@ * s@, params.metadata_region_size@) - ckoff_l(params.metadata_alignment@), params.sector_size@);
        cksize_l(n@ * s@, params.metadata_region_size@, params.metadata_alignment@, params.sector_size@) == 0 } };
    proof_assert! { fr002(params) ==> cksize_l(n@ * s@, params.metadata_region_size@, params.metadata_alignment@, params.sector_size@) == 0 };
    proof_assert! { !fmt6_passes(em.dev, params, Some(n), Some(s)) };
    em.format6(params, Some(n), Some(s), rnd)
}

/// Mutant: same requires and body; claims format reports an IoError instead.
// LEVEL-3 (phase D-B2): format6's requires are path-wise — the receptacle's own size (a_67ea4f of
// em.dev) is never needed on this path and is dropped; 2ee144 / 67ea4f are required only where
// the code reaches their arithmetic (the FR-002 checks passed).
#[requires(em_regions_wf(*em))]
#[requires(fr002(params) ==> a_2ee144(params.metadata_alignment@) && a_67ea4f(n@, s@))]
#[requires(em.dev.connected && eff_l(n@ * s@, params.metadata_region_size@) < 4096 + 2 * params.sector_size@)]
#[ensures(result == Err(EmError::IoError))]
pub fn verify_em_format_err_metadata_too_small__mutant(
    em: &mut ExtentManager, params: FormatParams, n: u64, s: u32, rnd: Option<u64>,
) -> Result<(), EmError> {
    proof_assert! { lemma_ckoff_ge(params.metadata_alignment@); ckoff_l(params.metadata_alignment@) >= 4096 };
    proof_assert! { fr002(params) && eff_l(n@ * s@, params.metadata_region_size@) >= ckoff_l(params.metadata_alignment@) ==> {
        lemma_small_half(eff_l(n@ * s@, params.metadata_region_size@) - ckoff_l(params.metadata_alignment@), params.sector_size@);
        cksize_l(n@ * s@, params.metadata_region_size@, params.metadata_alignment@, params.sector_size@) == 0 } };
    proof_assert! { fr002(params) ==> cksize_l(n@ * s@, params.metadata_region_size@, params.metadata_alignment@, params.sector_size@) == 0 };
    proof_assert! { !fmt6_passes(em.dev, params, Some(n), Some(s)) };
    em.format6(params, Some(n), Some(s), rnd)
}

/// `4096 + (u64::MAX - 1)` wraps to 4094; `4094 / u64::MAX == 0`.
#[bitwise_proof]
#[ensures(4096u64 + (18446744073709551615u64 - 1u64) == 4094u64)]
pub fn fact_wrap_4094() {}

/// lib.rs:418-452 in a RELEASE build for metadata size `md` (lib.rs:413), metadata_region_size
/// `mrs`, alignment `a`, data sector `ss`, data disk `dd`: (checkpoint_region_offset,
/// checkpoint_region_size, usable_data_size). Only the alignment sum can overflow here
/// (`effective - offset` is guarded, `data_start_offset` is 0 for mrs = 0).
#[requires(a@ > 0 && ss@ > 0 && mrs@ == 0)]
#[ensures(exists<w: u64> w == 4096u64 + (a - 1u64) && result.0@ == w@ / a@ * a@)]
#[ensures(result.1@ == (if md@ >= result.0@ { (md@ - result.0@) / 2 / ss@ * ss@ } else { 0 }))]
#[ensures(result.2@ == dd@)]
pub fn layout_release(md: u64, mrs: u64, a: u64, ss: u32, dd: u64) -> (u64, u64, u64) {
    let (_w, checkpoint_region_offset) = ckoff_release(a);
    let effective_metadata_size = if mrs > 0 { if md <= mrs { md } else { mrs } } else { md };
    let remaining = if effective_metadata_size >= checkpoint_region_offset { effective_metadata_size - checkpoint_region_offset } else { 0 };
    let sector_size_u64 = ss as u64;
    proof_assert! { lemma_divmod(remaining@ / 2, sector_size_u64@); (remaining@ / 2) / sector_size_u64@ * sector_size_u64@ <= remaining@ / 2 };
    let checkpoint_region_size = (remaining / 2) / sector_size_u64 * sector_size_u64;
    let data_start_offset: u64 = 0; // mrs == 0 (lib.rs:442-446)
    let usable = if dd >= data_start_offset { dd - data_start_offset } else { 0 };
    (checkpoint_region_offset, checkpoint_region_size, usable)
}

/// **EM-FORMAT-ERR-METADATA-TOO-SMALL** — REFUTED. INDEPENDENT candidate bug, NEW ROOT CAUSE =
/// FORMAT-ALIGN-OVERFLOW: lib.rs:423 computes `(sb_size + alignment - 1) / alignment * alignment`
/// with no bound on `metadata_alignment` (interfaces iextent_manager.rs:52-55 documents none).
/// "When the metadata space available after the superblock (capped by the metadata region size
/// when that is non-zero) is too small to hold two checkpoint copies of at least one sector
/// each, format() returns a CorruptMetadata error." metadata_alignment = u64::MAX, sector 512, a
/// 4608-byte metadata device (9 x 512), metadata_region_size 0, data disk 1 MiB: the space after
/// the superblock is 512 < 2 x 512. DEBUG build: `4096 + (u64::MAX - 1)` overflows -> panic, no
/// error returned. RELEASE build: the sum wraps to 4094, 4094 / u64::MAX = 0, so
/// checkpoint_region_offset = 0 (copy 0 placed OVER the superblock at byte 0), the region size is
/// 2048 > 0 and the usable data space 1 MiB > 0: both CorruptMetadata checks (lib.rs:434-438,
/// 449-452) pass.
#[ensures(4096 + (18446744073709551615u64@ - 1) > u64::MAX@)]
#[ensures(4608 - 4096 < 2 * 512)]
#[ensures(result.0@ == 0 && result.1@ == 2048 && result.2@ == 1048576)]
pub fn support_old_refute_em_format_err_metadata_too_small() -> (u64, u64, u64) {
    fact_wrap_4094();
    let r = layout_release(4608, 0, u64::MAX, 512, 1048576);
    proof_assert! { forall<w: u64> w == 4096u64 + (18446744073709551615u64 - 1u64) ==> w@ == 4094 };
    proof_assert! { 4094 / 18446744073709551615 * 18446744073709551615 == 0 };
    proof_assert! { 4608 / 2 / 512 * 512 == 2048 };
    r
}

/// Mutant: claims the release-build layout leaves no room (the check would fire).
#[ensures(result.1@ == 0)]
pub fn support_old_refute_em_format_err_metadata_too_small__mutant() -> (u64, u64, u64) {
    fact_wrap_4094();
    let r = layout_release(4608, 0, u64::MAX, 512, 1048576);
    proof_assert! { forall<w: u64> w == 4096u64 + (18446744073709551615u64 - 1u64) ==> w@ == 4094 };
    proof_assert! { 4094 / 18446744073709551615 * 18446744073709551615 == 0 };
    proof_assert! { 4608 / 2 / 512 * 512 == 2048 };
    r
}

// =============================================================================
// Checkpoint at device-command granularity (model/b6.rs Ck6)
// =============================================================================

#[logic(open)]
pub fn ev_is_data(e: Ev6) -> bool {
    pearlite! { match e { Ev6::Data(_, _, _, _) => true, _ => false } }
}
#[logic(open)]
pub fn ev_data_ok(e: Ev6) -> bool {
    pearlite! { match e { Ev6::Data(_, _, _, ok) => ok, _ => false } }
}
#[logic(open)]
pub fn ev_data_copy(e: Ev6) -> Int {
    pearlite! { match e { Ev6::Data(c, _, _, _) => c, _ => -1 } }
}
#[logic(open)]
pub fn ev_flush(e: Ev6, b: bool) -> bool {
    pearlite! { match e { Ev6::Flush(ok) => ok == b, _ => false } }
}
#[logic(open)]
pub fn ev_is_sb(e: Ev6) -> bool {
    pearlite! { match e { Ev6::Sb(_, _, _) => true, _ => false } }
}
#[logic(open)]
pub fn ev_sb_ok(e: Ev6) -> bool {
    pearlite! { match e { Ev6::Sb(_, _, ok) => ok, _ => false } }
}
#[logic(open)]
pub fn ev_sb_active(e: Ev6) -> Int {
    pearlite! { match e { Ev6::Sb(_, a, _) => a, _ => -1 } }
}
#[logic(open)]
pub fn ev_is_hook(e: Ev6) -> bool {
    pearlite! { match e { Ev6::Hook => true, _ => false } }
}
#[logic(open)]
pub fn vw(b: bool) -> Int {
    pearlite! { if b { 1 } else { 0 } }
}
/// Event `i` of what checkpoint `b` appended after state `a`.
#[logic(open)]
pub fn nev(a: Ck6, b: Ck6, i: Int) -> Ev6 {
    pearlite! { (*b.log)[a.log.len() + i] }
}
/// Number of events appended.
#[logic(open)]
pub fn nlen(a: Ck6, b: Ck6) -> Int {
    pearlite! { b.log.len() - a.log.len() }
}
/// The attempt reaches the copy write: initialized, something dirty, client created, payload fits.
#[logic(open)]
pub fn ck6_io(s: Ck6, plen: Int, io: Io6) -> bool {
    pearlite! { (match s.regions { Some(rs) => any_dirty6(rs@), None => false }) && io.client && 16 + plen <= s.ck_size@ }
}

/// **EM-CKPT-POST-FLUSH-WHEN-ENABLED** — "When the volatile_write_cache feature is enabled, a
/// checkpoint that writes data issues a flush to the metadata device after its writes, including
/// the superblock write, and reports success only after that flush has succeeded."
/// checkpoint.rs:101-102 flushes after the copy write and lib.rs:308-310 after the superblock
/// write (`flush()?`, block_io.rs:161-180). With vwc: a successful checkpoint's commands are
/// exactly copy write (ok), flush (ok), superblock write (ok), flush (ok); a flush is issued
/// whenever the superblock write succeeded, and success is reported iff that flush succeeded.
#[requires(ck6_pre(*ck, plen@) && ck.vwc && ck6_io(*ck, plen@, io))]
#[ensures(result == Ok(()) ==> nlen(*ck, ^ck) >= 4 && ev_data_ok(nev(*ck, ^ck, 0)) && ev_flush(nev(*ck, ^ck, 1), true)
    && ev_sb_ok(nev(*ck, ^ck, 2)) && ev_flush(nev(*ck, ^ck, 3), true))]
#[ensures(io.data && io.fl1 && io.sb ==> nlen(*ck, ^ck) >= 4 && ev_sb_ok(nev(*ck, ^ck, 2)) && ev_flush(nev(*ck, ^ck, 3), io.fl2))]
#[ensures(io.data && io.fl1 && io.sb ==> ((result == Ok(())) == io.fl2))]
pub fn verify_em_ckpt_post_flush_when_enabled(ck: &mut Ck6, io: Io6, plen: u64) -> Result<(), EmError> {
    ck.run6(io, plen)
}

/// Mutant: claims a successful checkpoint issues nothing after the superblock write.
#[requires(ck6_pre(*ck, plen@) && ck.vwc && ck6_io(*ck, plen@, io))]
#[ensures(result == Ok(()) ==> nlen(*ck, ^ck) == 3)]
pub fn verify_em_ckpt_post_flush_when_enabled__mutant(ck: &mut Ck6, io: Io6, plen: u64) -> Result<(), EmError> {
    ck.run6(io, plen)
}

/// **EM-CKPT-POST-SUPERBLOCK-AFTER-DATA** — "A checkpoint updates the superblock to point at the
/// new checkpoint copy only after that checkpoint's data has been completely and successfully
/// written (and, with the volatile write cache feature, flushed) to the metadata device."
/// write_checkpoint returns on a failed copy write / flush (`?`, checkpoint.rs:97, 102) before
/// run_checkpoint's superblock write (lib.rs:302-307). For every outcome of every command and
/// either feature setting: a superblock write is issued only as the command right after a
/// SUCCESSFUL copy write (and its successful flush with vwc), and the superblock it writes names
/// exactly the copy that was written.
// LEVEL-3 (phase D-B2): `ck.regions != None` dropped (uninitialized: no command at all, the claim
// holds vacuously).
#[requires(ck6_pre(*ck, plen@))]
#[ensures(forall<p: Int> 0 <= p && p < nlen(*ck, ^ck) && ev_is_sb(nev(*ck, ^ck, p)) ==>
    p == 1 + vw(ck.vwc) && ev_data_ok(nev(*ck, ^ck, 0)) && (ck.vwc ==> ev_flush(nev(*ck, ^ck, 1), true))
    && ev_sb_active(nev(*ck, ^ck, p)) == ev_data_copy(nev(*ck, ^ck, 0)))]
#[ensures(forall<p: Int> 0 <= p && p < ck.log.len() ==> (*(^ck).log)[p] == (*ck.log)[p])]
pub fn verify_em_ckpt_post_superblock_after_data(ck: &mut Ck6, io: Io6, plen: u64) -> Result<(), EmError> {
    ck.run6(io, plen)
}

/// Mutant: claims the superblock write can come first.
// LEVEL-3 (phase D-B2): `ck.regions != None` dropped (uninitialized: no command at all, the claim
// holds vacuously).
#[requires(ck6_pre(*ck, plen@))]
#[ensures(forall<p: Int> 0 <= p && p < nlen(*ck, ^ck) && ev_is_sb(nev(*ck, ^ck, p)) ==> p == 0)]
pub fn verify_em_ckpt_post_superblock_after_data__mutant(ck: &mut Ck6, io: Io6, plen: u64) -> Result<(), EmError> {
    ck.run6(io, plen)
}

/// **EM-CKPT-POST-HOOK-INVOKED** — "When a post-checkpoint hook is registered, each checkpoint
/// that writes and succeeds invokes it exactly once, synchronously, after the superblock update
/// and before checkpoint() returns; the hook is not invoked for a checkpoint attempt that fails
/// or that is skipped because nothing changed." lib.rs:365-367 is the last statement before
/// `Ok(())`; every failure returns earlier (`?`), the clean case at lib.rs:288-290. Over every
/// outcome and feature setting: a successful writing checkpoint appends exactly one hook call,
/// as its LAST action, after the successful superblock write (and its flush with vwc); a failed
/// attempt appends none; a clean one appends nothing at all.
#[requires(ck6_pre(*ck, plen@) && ck.hook && ck.regions != None)]
#[ensures(result == Ok(()) && ck6_io(*ck, plen@, io) ==>
    nlen(*ck, ^ck) == 3 + 2 * vw(ck.vwc) && ev_is_hook(nev(*ck, ^ck, 2 + 2 * vw(ck.vwc)))
    && ev_sb_ok(nev(*ck, ^ck, 1 + vw(ck.vwc))) && (ck.vwc ==> ev_flush(nev(*ck, ^ck, 3), true))
    && (forall<p: Int> 0 <= p && p < 2 + 2 * vw(ck.vwc) ==> !ev_is_hook(nev(*ck, ^ck, p))))]
#[ensures(result != Ok(()) ==> forall<p: Int> 0 <= p && p < nlen(*ck, ^ck) ==> !ev_is_hook(nev(*ck, ^ck, p)))]
#[ensures(match ck.regions { Some(rs) => !any_dirty6(rs@) ==> result == Ok(()) && ^ck == *ck, None => true })]
pub fn verify_em_ckpt_post_hook_invoked(ck: &mut Ck6, io: Io6, plen: u64) -> Result<(), EmError> {
    ck.run6(io, plen)
}

/// Mutant: claims a failed attempt ends with the hook call.
#[requires(ck6_pre(*ck, plen@) && ck.hook && ck.regions != None)]
#[ensures(result != Ok(()) ==> nlen(*ck, ^ck) > 0 && ev_is_hook(nev(*ck, ^ck, nlen(*ck, ^ck) - 1)))]
pub fn verify_em_ckpt_post_hook_invoked__mutant(ck: &mut Ck6, io: Io6, plen: u64) -> Result<(), EmError> {
    ck.run6(io, plen)
}

/// **EM-CKPT-ERR-TOO-LARGE** — "When the serialized state does not fit in one checkpoint copy,
/// checkpoint() fails with a CorruptMetadata error and leaves the persisted metadata and the
/// in-memory active copy and sequence number unchanged; a payload that exactly fills the copy is
/// accepted." checkpoint.rs:69-75 tests `16 + payload.len() > region_size` before any write and
/// before the in-memory superblock update (checkpoint.rs:105-112); run_checkpoint returns the
/// error (`?`, lib.rs:299) before the superblock write. Too large: CorruptMetadata, no command
/// issued, active copy / sequence / dirty flags unchanged. Exactly full: no CorruptMetadata; the
/// copy write is issued.
#[requires(ck6_pre(*ck, plen@) && io.client)]
#[requires(match ck.regions { Some(rs) => any_dirty6(rs@), None => false })]
#[ensures(16 + plen@ > ck.ck_size@ ==> result == Err(EmError::CorruptMetadata) && ^ck == *ck)]
#[ensures(16 + plen@ == ck.ck_size@ ==> result != Err(EmError::CorruptMetadata) && nlen(*ck, ^ck) >= 1 && ev_is_data(nev(*ck, ^ck, 0)))]
pub fn verify_em_ckpt_err_too_large(ck: &mut Ck6, io: Io6, plen: u64) -> Result<(), EmError> {
    ck.run6(io, plen)
}

/// Mutant: claims an exactly-full payload is rejected.
#[requires(ck6_pre(*ck, plen@) && io.client)]
#[requires(match ck.regions { Some(rs) => any_dirty6(rs@), None => false })]
#[ensures(16 + plen@ == ck.ck_size@ ==> result == Err(EmError::CorruptMetadata))]
pub fn verify_em_ckpt_err_too_large__mutant(ck: &mut Ck6, io: Io6, plen: u64) -> Result<(), EmError> {
    ck.run6(io, plen)
}

/// **EM-CKPT-POST-SKIP-WHEN-CLEAN** — "When nothing has been published or removed since the last
/// successful checkpoint (or since format or initialize), checkpoint() returns success without
/// writing anything to the metadata device." (a) run_checkpoint (lib.rs:276-290): every region
/// clean -> Ok with no command issued (not even get_metadata_client) and no state change.
/// (b) only publish (region.rs:118) and removal (region.rs:148) set a dirty flag: a reservation
/// (alloc_extent) and an abort (free_slot) leave it unchanged; a successful checkpoint leaves
/// every region clean (lib.rs:316-321, `run6` Ok clause); format / initialize build clean regions
/// (component.rs fresh_rg / rebuild_region: `!dirty`).
/// LEVEL-3 (phase D-B2): no checkpoint range on the clean path (run6 reads nothing there); the
/// later checkpoint `c2` is ANY checkpoint call (clean, failing or successful), under the
/// checkpoint ranges only when it writes (`ck6_io` dropped).
#[requires(match ck.regions { Some(rs) => !any_dirty6(rs@), None => false })]
#[requires(rg_inv(*r1) && sz@ > 0 && a_3783f8(sz@, r1.format_params.sector_size@))]
#[requires(rg_inv(*r2) && free_ok(*r2, s, i))]
#[requires(match c2.regions { Some(rs) => any_dirty6(rs@) ==> ck6_pre(*c2, plen@), None => true })]
#[ensures(result.0 == Ok(()) && ^ck == *ck)]
#[ensures((^r1).dirty == r1.dirty && (^r2).dirty == r2.dirty)]
#[ensures(result.1 == Ok(()) ==> match (^c2).regions { Some(rs) => !any_dirty6(rs@), None => false })]
pub fn verify_em_ckpt_post_skip_when_clean(
    ck: &mut Ck6, io: Io6, plen: u64, r1: &mut RegionState, sz: u32, r2: &mut RegionState, s: u64, i: usize, c2: &mut Ck6,
) -> (Result<(), EmError>, Result<(), EmError>) {
    let a = ck.run6(io, plen);
    let _ = r1.alloc_extent(sz);
    r2.free_slot(s, i);
    let b = c2.run6(io, plen);
    (a, b)
}

/// Mutant: claims a clean checkpoint issues a command (same requires as the property).
#[requires(match ck.regions { Some(rs) => !any_dirty6(rs@), None => false })]
#[requires(rg_inv(*r1) && sz@ > 0 && a_3783f8(sz@, r1.format_params.sector_size@))]
#[requires(rg_inv(*r2) && free_ok(*r2, s, i))]
#[requires(match c2.regions { Some(rs) => any_dirty6(rs@) ==> ck6_pre(*c2, plen@), None => true })]
#[ensures(nlen(*ck, ^ck) >= 1)]
pub fn verify_em_ckpt_post_skip_when_clean__mutant(
    ck: &mut Ck6, io: Io6, plen: u64, r1: &mut RegionState, sz: u32, r2: &mut RegionState, s: u64, i: usize, c2: &mut Ck6,
) -> (Result<(), EmError>, Result<(), EmError>) {
    let a = ck.run6(io, plen);
    let _ = r1.alloc_extent(sz);
    r2.free_slot(s, i);
    let b = c2.run6(io, plen);
    (a, b)
}

// =============================================================================
// checkpoint() coalescing (lib.rs:729-761)
// =============================================================================

/// **EM-CKPT-COMPLETED-SEQ-MONOTONIC** — "The count of completed checkpoints used to coalesce
/// concurrent requests never decreases." batch 4's coalescing protocol `CoSys` (one atomic step
/// per `checkpoint_coalesce` critical section; any number of caller threads; any schedule of
/// entries, condvar wake-ups — notify_all or spurious — and run_checkpoint returns with any
/// outcome): the recorded history of `completed_seq` after every step is non-decreasing.
/// lib.rs:753-754 assigns `completed_seq = needed` only on success, and the runner's `needed`
/// exceeds `completed_seq` (co_bound: it passed lib.rs:738 and no other thread runs).
// LEVEL-3 (phase D-B2): the schedule bound `2 * sched.len() + 2 <= u64::MAX` is gone — CoSys::enter
// models the completed_seq overflow of lib.rs:731-735 (D-B1), so no step needs headroom.
#[ensures(result.1@.len() == sched@.len() + 1)]
#[ensures(forall<i: Int, j: Int> 0 <= i && i <= j && j < result.1@.len() ==> result.1@[i]@ <= result.1@[j]@)]
#[ensures(result.1@[sched@.len()] == result.0.co.completed_seq)]
pub fn verify_em_ckpt_completed_seq_monotonic(n: usize, sched: &Vec<(usize, CoOp)>) -> (CoSys, Vec<u64>) {
    let mut sys = co_init(n);
    let mut hist: Vec<u64> = Vec::new();
    hist.push(sys.co.completed_seq);
    let mut k: usize = 0;
    #[invariant(k@ <= sched@.len() && co_inv(sys) && sys.ts@.len() == n@)]
    #[invariant(hist@.len() == k@ + 1 && hist@[k@] == sys.co.completed_seq)]
    #[invariant(forall<i: Int, j: Int> 0 <= i && i <= j && j < hist@.len() ==> hist@[i]@ <= hist@[j]@)]
    while k < sched.len() {
        let (t, op) = sched[k];
        if t < sys.ts.len() {
            match op {
                CoOp::Enter => sys.enter(t),
                CoOp::Wake => sys.wake(t),
                CoOp::Finish(ok) => sys.finish(t, ok),
            }
        }
        hist.push(sys.co.completed_seq);
        k += 1;
    }
    (sys, hist)
}

/// Mutant: claims the count strictly increases at every step.
#[ensures(forall<i: Int, j: Int> 0 <= i && i < j && j < result.1@.len() ==> result.1@[i]@ < result.1@[j]@)]
pub fn verify_em_ckpt_completed_seq_monotonic__mutant(n: usize, sched: &Vec<(usize, CoOp)>) -> (CoSys, Vec<u64>) {
    let mut sys = co_init(n);
    let mut hist: Vec<u64> = Vec::new();
    hist.push(sys.co.completed_seq);
    let mut k: usize = 0;
    #[invariant(k@ <= sched@.len() && co_inv(sys) && sys.ts@.len() == n@)]
    #[invariant(hist@.len() == k@ + 1)]
    while k < sched.len() {
        let (t, op) = sched[k];
        if t < sys.ts.len() {
            match op {
                CoOp::Enter => sys.enter(t),
                CoOp::Wake => sys.wake(t),
                CoOp::Finish(ok) => sys.finish(t, ok),
            }
        }
        hist.push(sys.co.completed_seq);
        k += 1;
    }
    (sys, hist)
}

/// Every pending caller's `needed` is at least 1.
#[logic(open)]
pub fn needs_pos(ts: Seq<Th>) -> bool {
    pearlite! { forall<i: Int> 0 <= i && i < ts.len() ==> match ts[i] { Th::Idle => true, Th::Waiting(n) => n@ >= 1, Th::Running(n) => n@ >= 1 } }
}

/// One scheduler step of the caller threads.
#[derive(::core::clone::Clone, ::core::marker::Copy)]
pub enum Op6 {
    Enter,
    Wake,
    Finish,
}

/// **EM-CKPT-ERR-NOT-INITIALIZED** — "When checkpoint() is called before either format() or
/// initialize() has succeeded, it returns a NotInitialized error." run_checkpoint (lib.rs:276-282)
/// returns NotInitialized while `regions` is None; and checkpoint() (lib.rs:729-761) can return
/// Ok without running only once `completed_seq >= needed >= 1`, i.e. after a SUCCESSFUL run. For
/// any number of caller threads (user threads and the background timer) and any schedule of
/// entries, wake-ups and run completions on a component that is not initialized, every value a
/// checkpoint() call returns is Err(NotInitialized).
#[requires(ck.regions == None)]
#[requires(sys.co.completed_seq@ == 0 && needs_pos(sys.ts@))]
#[ensures(forall<p: Int> 0 <= p && p < result@.len() ==> result@[p] == Err(EmError::NotInitialized))]
#[ensures((^ck).regions == None && (^sys).co.completed_seq@ == 0)]
pub fn verify_em_ckpt_err_not_initialized(ck: &mut Ck6, sys: &mut Cs6, sched: &Vec<(usize, Op6)>, io: Io6, plen: u64) -> Vec<Result<(), EmError>> {
    let mut rets: Vec<Result<(), EmError>> = Vec::new();
    let mut k: usize = 0;
    #[invariant(k@ <= sched@.len() && ck.regions == None)]
    #[invariant(sys.co.completed_seq@ == 0 && needs_pos(sys.ts@))]
    #[invariant(forall<p: Int> 0 <= p && p < rets@.len() ==> rets@[p] == Err(EmError::NotInitialized))]
    while k < sched.len() {
        let (t, op) = sched[k];
        if t < sys.ts.len() {
            match op {
                Op6::Enter => match sys.ts[t] {
                    Th::Idle => {
                        if sys.enter6(t) {
                            rets.push(Ok(()));
                        }
                    }
                    _ => {}
                },
                Op6::Wake => {
                    if sys.wake6(t) {
                        rets.push(Ok(()));
                    }
                }
                Op6::Finish => match sys.ts[t] {
                    Th::Running(_) => {
                        let r = ck.run6(io, plen);
                        let ok = match r {
                            Ok(()) => true,
                            Err(_) => false,
                        };
                        sys.finish6(t, ok);
                        rets.push(r);
                    }
                    _ => {}
                },
            }
        }
        k += 1;
    }
    rets
}

/// Mutant: claims the error is IoError.
#[requires(ck.regions == None)]
#[requires(sys.co.completed_seq@ == 0 && needs_pos(sys.ts@))]
#[ensures(forall<p: Int> 0 <= p && p < result@.len() ==> result@[p] == Err(EmError::IoError))]
pub fn verify_em_ckpt_err_not_initialized__mutant(ck: &mut Ck6, sys: &mut Cs6, sched: &Vec<(usize, Op6)>, io: Io6, plen: u64) -> Vec<Result<(), EmError>> {
    let mut rets: Vec<Result<(), EmError>> = Vec::new();
    let mut k: usize = 0;
    #[invariant(k@ <= sched@.len() && ck.regions == None)]
    #[invariant(sys.co.completed_seq@ == 0 && needs_pos(sys.ts@))]
    while k < sched.len() {
        let (t, op) = sched[k];
        if t < sys.ts.len() {
            match op {
                Op6::Finish => match sys.ts[t] {
                    Th::Running(_) => {
                        let r = ck.run6(io, plen);
                        sys.finish6(t, false);
                        rets.push(r);
                    }
                    _ => {}
                },
                _ => {}
            }
        }
        k += 1;
    }
    rets
}

// =============================================================================
// CKPT-SB-ORDER chain (model/ckpt.rs): #1 succeeds, #2's superblock write fails, #3 succeeds
// =============================================================================

/// C8 region (sector 4, slab 8, one 2-slot slab at 0, both slots reserved). publish(K1);
/// checkpoint #1 (all writes succeed); publish(K2); checkpoint #2 (copy write succeeds, the
/// superblock write fails: IoError, permitted by IBlockDevice); checkpoint #3 (all succeed).
/// Returns the three results and the device right before and right after #3.
#[ensures(result.0 == Ok(()) && result.1 != Ok(()) && result.2 == Ok(()))]
#[ensures(result.3.sb_seq@ == 1 && result.3.sb_active@ == 1)]
#[ensures(match copy_at(*result.3, 1u8) { Some(c) => c.seq@ == 1, None => false })]
#[ensures(result.4.sb_seq@ == 3 && result.4.sb_active@ == 1)]
#[ensures(match copy_at(*result.4, 1u8) { Some(c) => c.seq@ == 3, None => false })]
pub fn w6_sb_chain() -> (Result<(), EmError>, Result<(), EmError>, Result<(), EmError>, Snapshot<CkDev>, Snapshot<CkDev>) {
    let r = wc8_two();
    let mut rs: Vec<RegionState> = Vec::new();
    rs.push(r);
    let mut em = CkEm { regions: rs, active: 0, seq: 0, dev: wc_fresh_dev() };
    em.regions[0].publish_slot(0, 0, WK1);
    proof_assert! { em.regions@[0].dirty && em.regions@[0].pending_frees@.len() == 0 };
    proof_assert! { regions_ok(em.regions@) && !all_clean(em.regions@) };
    let r1 = em.run_checkpoint(true, true);
    proof_assert! { em.seq@ == 1 && em.active@ == 1 && em.dev.sb_seq@ == 1 && em.dev.sb_active@ == 1 };
    let s1 = snapshot! { em.regions@[0] };
    proof_assert! { s1.pending_frees@.len() == 0 && s1.slabs@.contains(0) && s1.slabs@.lookup(0).keys@.len() == 2 };
    em.regions[0].publish_slot(0, 1, WK2);
    proof_assert! { em.regions@[0].dirty && em.regions@[0].pending_frees@.len() == 0 };
    proof_assert! { regions_ok(em.regions@) && !all_clean(em.regions@) };
    let r2 = em.run_checkpoint(true, false);
    let d2 = snapshot! { em.dev };
    proof_assert! { em.seq@ == 2 && em.active@ == 0 };
    proof_assert! { regions_ok(em.regions@) && !all_clean(em.regions@) };
    let r3 = em.run_checkpoint(true, true);
    let d3 = snapshot! { em.dev };
    (r1, r2, r3, d2, d3)
}

/// **EM-CKPT-POST-SEQ-INCREASES** — REFUTED. ROOT CAUSE = CKPT-SB-ORDER (checkpoint.rs:105-112
/// advance the in-memory active copy / checkpoint seq before run_checkpoint's superblock write,
/// lib.rs:302-307, is known to have succeeded; nothing rolls them back) — not the buddy mask.
/// "Each checkpoint that performs I/O and succeeds stores in the superblock a checkpoint sequence
/// number that is exactly one greater than the one stored before it (hence strictly greater), and
/// makes active the checkpoint copy that was not active before." `w6_sb_chain`: after #1 the
/// on-disk superblock stores seq 1 / active copy 1; #2 writes copy 0, fails at the superblock
/// write and leaves the in-memory superblock at seq 2 / copy 0; #3 then SUCCEEDS and stores seq
/// 3 (= 1 + 2) and active copy 1 — the copy that was already active.
#[ensures(result.2 == Ok(()))]
#[ensures(result.4.sb_seq@ != result.3.sb_seq@ + 1)]
#[ensures(result.4.sb_active == result.3.sb_active)]
pub fn refute_em_ckpt_post_seq_increases() -> (Result<(), EmError>, Result<(), EmError>, Result<(), EmError>, Snapshot<CkDev>, Snapshot<CkDev>) {
    let (r1, r2, r3, d2, d3) = w6_sb_chain();
    (r1, r2, r3, d2, d3)
}

/// Mutant: claims #3 stored exactly one more than the previously stored seq.
#[ensures(result.4.sb_seq@ == result.3.sb_seq@ + 1)]
pub fn refute_em_ckpt_post_seq_increases__mutant() -> (Result<(), EmError>, Result<(), EmError>, Result<(), EmError>, Snapshot<CkDev>, Snapshot<CkDev>) {
    let (r1, r2, r3, d2, d3) = w6_sb_chain();
    (r1, r2, r3, d2, d3)
}

/// Unscored sequential half of EM-CKPT-POST-SEQ-INCREASES: while the in-memory superblock agrees
/// with the stored one (no earlier failed superblock write), a successful checkpoint that performs
/// I/O stores seq + 1 and the other copy.
#[requires(regions_ok(em.regions@) && !all_clean(em.regions@))]
#[requires(em.active@ <= 1 && em.seq@ < u64::MAX@)]
#[requires(em.dev.sb_seq == em.seq && em.dev.sb_active == em.active)]
#[ensures(result == Ok(()) ==> (^em).dev.sb_seq@ == em.dev.sb_seq@ + 1 && (^em).dev.sb_active@ == 1 - em.dev.sb_active@)]
pub fn witness_em_ckpt_post_seq_increases_sequential(em: &mut CkEm, ok_data: bool, ok_sb: bool) -> Result<(), EmError> {
    em.run_checkpoint(ok_data, ok_sb)
}

/// Unscored second route for EM-CKPT-POST-WRITES-INACTIVE-COPY (CKPT-SB-ORDER): in
/// `w6_sb_chain` the superblock on the device names copy 1 (seq 1) when #3 starts; #3 writes
/// copy 1 (seq 3) — the copy the stored superblock calls active is overwritten.
#[ensures(result.0.sb_active@ == 1 && result.1.sb_active@ == 1)]
#[ensures(match (copy_at(*result.0, 1u8), copy_at(*result.1, 1u8)) { (Some(a), Some(b)) => a.seq@ == 1 && b.seq@ == 3, _ => false })]
pub fn witness_em_ckpt_post_writes_inactive_copy_sb_order() -> (Snapshot<CkDev>, Snapshot<CkDev>) {
    let (_, _, _, d2, d3) = w6_sb_chain();
    (d2, d3)
}

/// Unscored: the copy-level half of the audit's suggested wording of
/// EM-CKPT-POST-WRITES-INACTIVE-COPY, while the in-memory and stored superblocks agree: whatever
/// the device outcomes, the copy that was active is left exactly as it was, and on success the
/// stored superblock names the copy just written, which holds this checkpoint's image.
#[requires(regions_ok(em.regions@) && !all_clean(em.regions@))]
#[requires(em.active@ <= 1 && em.seq@ < u64::MAX@)]
#[requires(em.dev.sb_seq == em.seq && em.dev.sb_active == em.active)]
#[ensures(copy_at((^em).dev, em.dev.sb_active) == copy_at(em.dev, em.dev.sb_active))]
#[ensures(result == Ok(()) ==> (^em).dev.sb_active@ == 1 - em.dev.sb_active@
    && match copy_at((^em).dev, (^em).dev.sb_active) { Some(c) => c.seq == (^em).dev.sb_seq, None => false })]
pub fn witness_em_ckpt_post_writes_inactive_copy_intended_copies(em: &mut CkEm, ok_data: bool, ok_sb: bool) -> Result<(), EmError> {
    let img = match em.ck_begin() {
        Some(img) => img,
        None => return Ok(()),
    };
    em.ck_write(img, ok_data, ok_sb)
}

// =============================================================================
// CKPT-LAYOUT-SECTOR: copy writes on a metadata device whose sector is larger than the data sector
// =============================================================================

/// region.rs:118 / 148 (publish / removal): the region's dirty flag is set.
#[requires(match c.regions { Some(rs) => i@ < rs@.len(), None => false })]
#[ensures(match (c.regions, (^c).regions) { (Some(a), Some(b)) => b@ == a@.set(i@, true), _ => false })]
#[ensures((^c).active == c.active && (^c).seq == c.seq && (^c).ck_off == c.ck_off && (^c).ck_size == c.ck_size)]
#[ensures((^c).ms == c.ms && (^c).hook == c.hook && (^c).vwc == c.vwc && (^c).log == c.log)]
pub fn ck6_publish(c: &mut Ck6, i: usize) {
    match &mut c.regions {
        Some(rs) => rs[i] = true,
        None => {}
    }
}

/// Data sector 512, metadata sector 4096, 16 KiB metadata device, alignment 0: format gives
/// checkpoint_region_offset 4096, checkpoint_region_size (12288 / 2) rounded to 512 = 6144
/// (lib.rs:420-432): copy 0 = [4096, 10240), copy 1 = [10240, 16384). One dirty region.
#[ensures(result.ck_off@ == 4096 && result.ck_size@ == 6144 && result.ms@ == 4096)]
#[ensures(result.active@ == 0 && result.seq@ == 0 && !result.hook && !result.vwc && result.log.len() == 0)]
#[ensures(match result.regions { Some(rs) => rs@.len() == 1 && rs@[0], None => false })]
pub fn w6_layout_ck() -> Ck6 {
    let mut rs: Vec<bool> = Vec::new();
    rs.push(true);
    Ck6 { regions: Some(rs), active: 0, seq: 0, ck_off: 4096, ck_size: 6144, ms: 4096, hook: false, vwc: false, log: snapshot! { Seq::empty() } }
}

/// The all-succeeding device.
#[ensures(result.client && result.data && result.fl1 && result.sb && result.fl2)]
pub fn io6_ok() -> Io6 {
    Io6 { client: true, data: true, fl1: true, sb: true, fl2: true }
}

/// LEVEL-3 RETIRED (support only; EM-CKPT-POST-WRITES-INACTIVE-COPY is NOT among the 21 level-1 refutations — it was
/// excluded at level 1). Its state has checkpoint_region_size 6144 on a 4096-byte-sector metadata
/// device, i.e. OUTSIDE D-RANGE-FMT-METADATA-DEVICE-LAYOUT-dfd6b0 (the copy layout is a multiple
/// of the metadata sector size). Kept unchanged as the documented out-of-range behaviour.
/// (Level-2 text:) **EM-CKPT-POST-WRITES-INACTIVE-COPY** — REFUTED. ROOT CAUSE = CKPT-LAYOUT-SECTOR (batch 3:
/// lib.rs:430-432 sizes the copies in DATA sectors and lib.rs:420-424 aligns copy 0 only to
/// metadata_alignment, while checkpoint.rs:91-96 truncates the copy offset to, and pads the
/// write up to, the METADATA device's sector) — not the buddy mask. "A checkpoint that performs
/// I/O writes its data only into the checkpoint copy that was inactive when it started and writes
/// nothing outside that copy; the copy that was active when it started remains byte-for-byte
/// unchanged, and on success the superblock names the written copy as the new active copy."
/// Geometry `w6_layout_ck` (copy 0 = [4096,10240), copy 1 = [10240,16384), metadata sector 4096),
/// every command succeeds. Checkpoint #1 (copy 0 active) writes copy 1 at LBA 10240/4096 = 2,
/// i.e. bytes [8192, 12288) — inside copy 0, the ACTIVE copy. After a publish, checkpoint #2
/// (copy 1 active) writes copy 0 with a payload that exactly fills a copy (16 + 6128 = 6144
/// bytes, accepted by checkpoint.rs:69-75): bytes [4096, 12288) — past copy 0's end and over the
/// first sector of copy 1 as #1 wrote it (byte 8192 = where recovery reads copy 1, also LBA 2),
/// the copy that was active. Both checkpoints succeed. Fails in the audit's suggested wording too
/// ("writes nothing else except the superblock"): not a wording artifact.
#[ensures(result.0 == Ok(()) && result.1 == Ok(()))]
#[ensures(ev_data_copy(*result.2) == 1 && ev_data_copy(*result.3) == 0)]
#[ensures(match *result.2 { Ev6::Data(_, st, ln, ok) => ok && st == 8192 && ln == 4096 && 4096 <= st && st < 10240, _ => false })]
#[ensures(match *result.3 { Ev6::Data(_, st, ln, ok) => ok && st == 4096 && ln == 8192 && st + ln > 10240 && st <= 8192 && 8192 < st + ln, _ => false })]
#[ensures(result.4@ == 1)]
pub fn support_old_refute_em_ckpt_post_writes_inactive_copy() -> (Result<(), EmError>, Result<(), EmError>, Snapshot<Ev6>, Snapshot<Ev6>, u8) {
    let mut c = w6_layout_ck();
    let io = io6_ok();
    proof_assert! { ck6_hd(c, 100) && ck6_io(c, 100, io) };
    let l0 = snapshot! { *c.log };
    let r1 = c.run6(io, 100);
    proof_assert! { (4096 + 6144) / 4096 * 4096 == 8192 && (16 + 100 + 4096 - 1) / 4096 * 4096 == 4096 };
    let e1 = snapshot! { (*c.log)[l0.len()] };
    let act = c.active;
    ck6_publish(&mut c, 0);
    proof_assert! { c.active@ == 1 && ck6_hd(c, 6128) && ck6_io(c, 6128, io) };
    let l1 = snapshot! { *c.log };
    let r2 = c.run6(io, 6128);
    proof_assert! { (4096 + 0 * 6144) / 4096 * 4096 == 4096 && (16 + 6128 + 4096 - 1) / 4096 * 4096 == 8192 };
    let e2 = snapshot! { (*c.log)[l1.len()] };
    (r1, r2, e1, e2, act)
}

/// Mutant: claims #2's copy write stays inside copy 0.
#[ensures(match *result.3 { Ev6::Data(_, st, ln, _) => st + ln <= 10240, _ => true })]
pub fn support_old_refute_em_ckpt_post_writes_inactive_copy__mutant() -> (Result<(), EmError>, Result<(), EmError>, Snapshot<Ev6>, Snapshot<Ev6>, u8) {
    let mut c = w6_layout_ck();
    let io = io6_ok();
    proof_assert! { ck6_hd(c, 100) && ck6_io(c, 100, io) };
    let l0 = snapshot! { *c.log };
    let r1 = c.run6(io, 100);
    let e1 = snapshot! { (*c.log)[l0.len()] };
    let act = c.active;
    ck6_publish(&mut c, 0);
    proof_assert! { c.active@ == 1 && ck6_hd(c, 6128) && ck6_io(c, 6128, io) };
    let l1 = snapshot! { *c.log };
    let r2 = c.run6(io, 6128);
    proof_assert! { (4096 + 0 * 6144) / 4096 * 4096 == 4096 && (16 + 6128 + 4096 - 1) / 4096 * 4096 == 8192 };
    let e2 = snapshot! { (*c.log)[l1.len()] };
    (r1, r2, e1, e2, act)
}

#[logic]
#[requires(m > 0 && x >= 0 && x % m == 0)]
#[ensures(x / m * m == x)]
pub fn lemma_div_exact6(x: Int, m: Int) {
    lemma_divmod(x, m)
}

#[logic]
#[requires(m > 0 && x >= 0 && y >= 0 && x % m == 0 && y % m == 0)]
#[ensures((x + y) % m == 0)]
pub fn lemma_mod_add6(x: Int, y: Int, m: Int) {
    pearlite! { { lemma_divmod(x, m); lemma_divmod(y, m); lemma_distrib(x / m, y / m, m);
        crate::model::blockio::lemma_divmod_at(x / m + y / m, m, 0) } }
}

#[logic]
#[requires(m > 0 && 0 <= v && v <= y && y % m == 0)]
#[ensures((v + m - 1) / m * m <= y)]
pub fn lemma_round_up_le6(v: Int, y: Int, m: Int) {
    pearlite! { { lemma_divmod(v + m - 1, m); lemma_divmod(y, m);
        if (v + m - 1) / m > y / m { lemma_mul_le(y / m + 1, (v + m - 1) / m, m) } else { lemma_mul_le((v + m - 1) / m, y / m, m) } } }
}

/// Unscored: the byte-level half of the suggested wording of EM-CKPT-POST-WRITES-INACTIVE-COPY
/// holds whenever the metadata sector divides checkpoint_region_offset and checkpoint_region_size
/// (e.g. equal data and metadata sectors): the attempt issues exactly one copy write, to the copy
/// that was inactive, inside that copy's byte range; every other command is a flush, the
/// superblock write (naming that copy) or the hook call.
#[requires(ck6_pre(*ck, plen@) && ck6_io(*ck, plen@, io))]
#[requires(ck.ck_off@ % ck.ms@ == 0 && ck.ck_size@ % ck.ms@ == 0)]
#[ensures(match nev(*ck, ^ck, 0) { Ev6::Data(c, st, ln, _) =>
    (ck.active@ == 0 ==> c == 1 && st == ck.ck_off@ + ck.ck_size@ && st + ln <= ck.ck_off@ + 2 * ck.ck_size@)
    && (ck.active@ == 1 ==> c == 0 && st == ck.ck_off@ && st + ln <= ck.ck_off@ + ck.ck_size@), _ => false })]
#[ensures(forall<p: Int> 1 <= p && p < nlen(*ck, ^ck) ==> !ev_is_data(nev(*ck, ^ck, p)))]
#[ensures(forall<p: Int> 0 <= p && p < nlen(*ck, ^ck) && ev_is_sb(nev(*ck, ^ck, p)) ==> ev_sb_active(nev(*ck, ^ck, p)) == 1 - ck.active@)]
pub fn witness_em_ckpt_post_writes_inactive_copy_intended(ck: &mut Ck6, io: Io6, plen: u64) -> Result<(), EmError> {
    proof_assert! { lemma_mul_le(1 - ck.active@, 1, ck.ck_size@); (1 - ck.active@) * ck.ck_size@ <= ck.ck_size@ };
    proof_assert! { ck.active@ == 1 ==> (1 - ck.active@) * ck.ck_size@ == 0 };
    proof_assert! { ck.active@ == 0 ==> (1 - ck.active@) * ck.ck_size@ == ck.ck_size@ };
    proof_assert! { ck.active@ == 0 ==> { lemma_mod_add6(ck.ck_off@, ck.ck_size@, ck.ms@); (ck.ck_off@ + ck.ck_size@) % ck.ms@ == 0 } };
    proof_assert! { { lemma_div_exact6(ck.ck_off@ + (1 - ck.active@) * ck.ck_size@, ck.ms@); ck_start(*ck) == ck.ck_off@ + (1 - ck.active@) * ck.ck_size@ } };
    proof_assert! { { lemma_round_up_le6(16 + plen@, ck.ck_size@, ck.ms@); ck_len(*ck, plen@) <= ck.ck_size@ } };
    ck.run6(io, plen)
}

// =============================================================================
// EM-CKPT-POST-COALESCED-CALLER-COVERED: coalescing after a FAILED run (new root cause)
// =============================================================================

/// The coalescer at construction (lib.rs:53-57) with three caller threads.
#[ensures(result.co.completed_seq@ == 0 && !result.co.in_progress && result.ts@.len() == 3)]
#[ensures(forall<i: Int> 0 <= i && i < 3 ==> result.ts@[i] == Th::Idle)]
pub fn cs6_three() -> Cs6 {
    let mut ts: Vec<Th> = Vec::new();
    ts.push(Th::Idle);
    ts.push(Th::Idle);
    ts.push(Th::Idle);
    Cs6 { co: Co { completed_seq: 0, in_progress: false }, ts }
}

/// The schedule: thread A (0) runs a checkpoint whose copy write FAILS; B (1), which arrived
/// during A's run, then runs; C (2) publishes K2 and calls checkpoint() during B's run and is
/// satisfied by B. The component side runs on model/ckpt.rs (sector 4, C8 region): the phases of
/// each run are exactly the critical sections of write_checkpoint / run_checkpoint.
/// Returns (C's wake-up returned Ok, coalescer when C entered, coalescer when B finished, what a
/// fresh component recovers, B's run result).
#[ensures(result.0)]
#[ensures(match result.1.ts@[1] { Th::Running(n) => n@ == 2, _ => false })]
#[ensures(match result.1.ts@[2] { Th::Waiting(n) => n@ == 2, _ => false })]
#[ensures(result.1.co.completed_seq@ == 0 && result.1.co.in_progress)]
#[ensures(result.2.co.completed_seq@ == 2 && result.2.ts@[1] == Th::Idle)]
#[ensures(match result.2.ts@[2] { Th::Waiting(n) => n@ == 2, _ => false })]
#[ensures(match result.3 { Ok(img) => img_lacks_key(*img, WK2), Err(_) => false })]
#[ensures(result.4 == Ok(()))]
pub fn w6_coalesce_chain() -> (bool, Snapshot<Cs6>, Snapshot<Cs6>, Result<Snapshot<Seq<RegionState>>, EmError>, Result<(), EmError>) {
    let mut sys = cs6_three();
    let r = wc8_two();
    let r0 = snapshot! { r };
    let mut rs: Vec<RegionState> = Vec::new();
    rs.push(r);
    let mut em = CkEm { regions: rs, active: 0, seq: 0, dev: wc_fresh_dev() };
    proof_assert! { em.regions@[0] == *r0 };
    // publish(K1) (lib.rs:610 -> region.rs:114-119): the region is dirty
    em.regions[0].publish_slot(0, 0, WK1);
    let s1 = snapshot! { em.regions@[0].slabs@.lookup(0) };
    proof_assert! { s1.keys@[0] == WK1 && s1.keys@[1] == FREE_KEY && em.regions@[0].dirty };
    proof_assert! { regions_ok(em.regions@) && !all_clean(em.regions@) };
    // A calls checkpoint(): nothing in progress -> needed 1, A runs (lib.rs:730-748)
    let _a = sys.enter6(0);
    // B calls checkpoint() while A runs -> needed 0 + 2, waits (lib.rs:744)
    let _b = sys.enter6(1);
    // A's run: serialise, then the copy write FAILS (IoError, IBlockDevice contract)
    let img_a = match em.ck_begin() {
        Some(img) => img,
        None => return (false, snapshot! { sys }, snapshot! { sys }, Err(EmError::IoError), Err(EmError::IoError)),
    };
    let _ra = em.ck_write(img_a, false, true);
    proof_assert! { em.regions@[0].slabs@.lookup(0) == *s1 && em.active@ == 0 && em.seq@ == 0 };
    // A re-locks: failure, completed_seq stays 0, in_progress cleared, notify_all (lib.rs:752-758)
    sys.finish6(0, false);
    // B wakes: 0 < 2 and nothing in progress -> B runs (lib.rs:738-748)
    let _b2 = sys.wake6(1);
    // B's run, phase 1: serialise the region (checkpoint.rs:47-55)
    let img_b = match em.ck_begin() {
        Some(img) => img,
        None => return (false, snapshot! { sys }, snapshot! { sys }, Err(EmError::IoError), Err(EmError::IoError)),
    };
    proof_assert! { *img_b == em.regions@ };
    // C: publish(K2) completes; THEN C calls checkpoint(): B in progress -> needed 0 + 2 = 2
    em.regions[0].publish_slot(0, 1, WK2);
    proof_assert! { regions_ok(em.regions@) };
    let _c = sys.enter6(2);
    let at_c = snapshot! { sys };
    // B's run, phases 2-4 + superblock write, all succeed; clear dirty + flush (lib.rs:312-321)
    let rb = em.ck_write(img_b, true, true);
    em.ck_finish();
    // B re-locks: completed_seq = 2 (lib.rs:753-754)
    sys.finish6(1, true);
    let at_b = snapshot! { sys };
    // C wakes: 2 >= 2 -> return Ok(()) (lib.rs:738-740) without any checkpoint started after its call
    let c_ok = sys.wake6(2);
    // a fresh component's initialize() recovers from the device
    let rec = recover(&em.dev);
    proof_assert! { em.dev.sb_seq@ == 1 && em.dev.sb_active@ == 1 };
    proof_assert! { match em.dev.copy1 { Some(c) => c.seq@ == 1 && c.img == img_b, None => false } };
    proof_assert! { img_b[0].slabs@.contains(0) && forall<k: Int> img_b[0].slabs@.contains(k) ==> k == 0 };
    proof_assert! { img_b[0].slabs@.lookup(0) == *s1 && WK2 != WK1 && WK2 != FREE_KEY };
    proof_assert! { forall<j: Int> 0 <= j && j < 2 ==> s1.keys@[j] != WK2 };
    (c_ok, at_c, at_b, rec, rb)
}

/// **EM-CKPT-POST-COALESCED-CALLER-COVERED** — REFUTED. INDEPENDENT candidate bug, NEW ROOT CAUSE
/// = CKPT-COALESCE-FAILED-RUN: lib.rs:731-735 gives a caller that arrives during a run
/// `needed = completed_seq + 2`, assuming the running checkpoint will advance completed_seq to
/// exactly completed_seq + 1; but a FAILED run leaves completed_seq unchanged (lib.rs:753-755) and
/// a waiter whose `needed` is already completed_seq + 2 then runs — a caller arriving during THAT
/// run computes the same completed_seq + 2 and is satisfied by it (lib.rs:738), although it
/// started before the call. "A checkpoint() call, including one that arrives while another
/// checkpoint is in progress and is satisfied by waiting, returns success only once a checkpoint
/// that started after the call began has completed, so every modification finished before the
/// call is persisted." `w6_coalesce_chain`: A's run fails at the copy write; B (needed 2) runs and
/// serialises; C's publish(K2) completes, then C calls checkpoint() (needed 0 + 2 = 2) and waits;
/// B succeeds (completed_seq = 2); C returns Ok(()) — the only checkpoint completed during C's
/// call is B's, started before it — and the persisted state lacks K2. The audit's suggested
/// wording ("every publish and removal that completed before the call began is persisted") fails
/// too: not a wording artifact.
#[ensures(result.0)]
#[ensures(match result.1.ts@[1] { Th::Running(n) => n@ == 2, _ => false })]
#[ensures(result.1.co.completed_seq@ == 0 && result.2.co.completed_seq@ == 2)]
#[ensures(match result.3 { Ok(img) => img_lacks_key(*img, WK2), Err(_) => false })]
pub fn refute_em_ckpt_post_coalesced_caller_covered() -> (bool, Snapshot<Cs6>, Snapshot<Cs6>, Result<Snapshot<Seq<RegionState>>, EmError>, Result<(), EmError>) {
    w6_coalesce_chain()
}

/// Mutant: claims C is NOT satisfied by B's run.
#[ensures(!result.0)]
pub fn refute_em_ckpt_post_coalesced_caller_covered__mutant() -> (bool, Snapshot<Cs6>, Snapshot<Cs6>, Result<Snapshot<Seq<RegionState>>, EmError>, Result<(), EmError>) {
    w6_coalesce_chain()
}

// =============================================================================
// EM-PUBLISH-FRAME-OTHER-EXTENTS (BUDDY-MASK, W2a)
// =============================================================================

pub const W6_K7: u64 = 77;
pub const W6P_K4: u64 = 4;

/// **EM-PUBLISH-FRAME-OTHER-EXTENTS** — REFUTED. ROOT CAUSE = BUDDY-MASK (non-power-of-two
/// sector size). "publish() changes only the published slot; the key, offset, size and
/// visibility of every other extent stay the same." W2a (sector 3, slab 6, one region [0,12)):
/// after initialize the recovered slab B's block [6,12) stays on the free lists (buddy.rs:136);
/// H4 = reserve(K4, 3) names (6, slot 1); reserve(K5, 6) and reserve(K6, 3) carve a fresh slab E
/// at 6 that REPLACES B (region.rs:89) while H4 is live (`w2a_replaced`). reserve(K7, 3) then
/// gets E's free slot 1 — the slot H4 names — and publish(K7) makes (K7, offset 9, size 3)
/// visible. Publishing H4 (lib.rs:610 -> region.rs:114-119) writes K4 into that slot: the other
/// extent K7 is no longer listed anywhere.
#[ensures(from_slot(*result.0, 6, 1, Extent { key: W6_K7, size: 3u32, offset: 9u64 }))]
#[ensures(!from_region(*result.1, Extent { key: W6_K7, size: 3u32, offset: 9u64 }))]
#[ensures(from_slot(*result.1, 6, 1, Extent { key: W_K4, size: 3u32, offset: 9u64 }))]
#[ensures(result.2@ == 6 && result.3@ == 1)]
pub fn support_old_refute_em_publish_frame_other_extents() -> (Snapshot<RegionState>, Snapshot<RegionState>, u64, usize) {
    let (mut r, s4, i4, _off4) = w2a_replaced();
    let e0 = snapshot! { r.slabs@.lookup(6) };
    proof_assert! { slab_ok(r, *e0) && slab_inv(*e0) && e0.keys@.len() == 2 };
    proof_assert! { align_l(3, 3) == 3 };
    let o1 = snapshot! { r };
    proof_assert! { sc_get(r.size_classes, 3) == Seq::singleton(6u64) && exist_cond(*o1, 3, 6u64) };
    // reserve(K7, 3): size class 3 lists E, which has a free slot -> slot 1, offset 9
    let res = r.alloc_extent(3);
    proof_assert! { exist_case(*o1, r, 3, 6u64, res) };
    proof_assert! { match res { Ok((s, i, off)) => s@ == 6 && i@ < 2 && !slot_bit(e0.bitmap, i@) && off@ == slot_off(*e0, i@), Err(_) => false } };
    proof_assert! { match res { Ok((s, i, off)) => i@ == 1 && off@ == 9, Err(_) => false } };
    proof_assert! { r.slabs@.lookup(6).keys == e0.keys && r.slabs@.lookup(6).start_offset == e0.start_offset
        && r.slabs@.lookup(6).element_size == e0.element_size && r.slabs@.lookup(6).bitmap.num_slots == e0.bitmap.num_slots };
    proof_assert! { r.slabs@.lookup(0) == o1.slabs@.lookup(0) && forall<k: Int> r.slabs@.contains(k) ==> k == 6 || k == 0 };
    // publish(K7) (region.rs:114-119)
    r.publish_slot(6, 1, W6_K7);
    let before = snapshot! { r };
    proof_assert! { before.slabs@.lookup(6).keys@ == e0.keys@.set(1, W6_K7) && W6_K7 != FREE_KEY };
    proof_assert! { slot_off(before.slabs@.lookup(6), 1) == 9 };
    proof_assert! { from_slot(*before, 6, 1, Extent { key: W6_K7, size: 3u32, offset: 9u64 }) };
    // publish(H4) — the handle reserved before the replacement (lib.rs:597-621)
    r.publish_slot(s4, i4, W_K4);
    proof_assert! { r.slabs@.lookup(6).keys@ == e0.keys@.set(1, W6_K7).set(1, W_K4) };
    proof_assert! { r.slabs@.lookup(6).keys@[0] == FREE_KEY && r.slabs@.lookup(6).keys@[1] == W_K4 && W_K4 != W6_K7 };
    proof_assert! { r.slabs@.lookup(0) == o1.slabs@.lookup(0) && forall<k: Int> r.slabs@.contains(k) ==> k == 6 || k == 0 };
    proof_assert! { forall<j: Int> 0 <= j && j < r.slabs@.lookup(0).keys@.len() ==> r.slabs@.lookup(0).keys@[j] != W6_K7 };
    proof_assert! { forall<j: Int> 0 <= j && j < 2 ==> r.slabs@.lookup(6).keys@[j] != W6_K7 };
    proof_assert! { forall<k: Int, j: Int> !from_slot(r, k, j, Extent { key: W6_K7, size: 3u32, offset: 9u64 }) };
    let after = snapshot! { r };
    (before, after, s4, i4)
}

/// Mutant: claims K7 is still listed after H4's publish.
#[ensures(from_region(*result.1, Extent { key: W6_K7, size: 3u32, offset: 9u64 }))]
pub fn support_old_refute_em_publish_frame_other_extents__mutant() -> (Snapshot<RegionState>, Snapshot<RegionState>, u64, usize) {
    let (mut r, s4, i4, _off4) = w2a_replaced();
    let e0 = snapshot! { r.slabs@.lookup(6) };
    proof_assert! { slab_ok(r, *e0) && slab_inv(*e0) && e0.keys@.len() == 2 };
    proof_assert! { align_l(3, 3) == 3 };
    let o1 = snapshot! { r };
    proof_assert! { sc_get(r.size_classes, 3) == Seq::singleton(6u64) && exist_cond(*o1, 3, 6u64) };
    let res = r.alloc_extent(3);
    proof_assert! { exist_case(*o1, r, 3, 6u64, res) };
    proof_assert! { r.slabs@.lookup(6).keys == e0.keys };
    r.publish_slot(6, 1, W6_K7);
    let before = snapshot! { r };
    r.publish_slot(s4, i4, W_K4);
    let after = snapshot! { r };
    (before, after, s4, i4)
}

/// pow2 witness of EM-PUBLISH-FRAME-OTHER-EXTENTS: W2p (sector 4) — reserve(K4, 4) takes B's
/// free slot 1 (offset 12); reserve(K5, 8) carves D at 0; a further reserve(K6, 4) finds no space
/// (no replacement of B is possible). Publishing H4 writes only (8, slot 1), which held no extent:
/// the recovered (K3, 8, 4) and every other slab are unchanged.
#[ensures(from_slot(*result.0, 8, 0, Extent { key: WP_K3, size: 4u32, offset: 8u64 }))]
#[ensures(from_slot(*result.1, 8, 0, Extent { key: WP_K3, size: 4u32, offset: 8u64 }))]
#[ensures(result.0.slabs@.lookup(8).keys@[1] == FREE_KEY)]
#[ensures(forall<k: Int> k != 8 ==> result.1.slabs@.get(k) == result.0.slabs@.get(k))]
#[ensures(from_slot(*result.1, 8, 1, Extent { key: W6P_K4, size: 4u32, offset: 12u64 }))]
pub fn witness_em_publish_frame_other_extents_pow2() -> (Snapshot<RegionState>, Snapshot<RegionState>) {
    let mut r = w5p_rec();
    let s0 = snapshot! { r };
    proof_assert! { slot_off(s0.slabs@.lookup(8), 0) == 8 && WP_K3 != FREE_KEY };
    proof_assert! { align_l(4, 4) == 4 && align_l(8, 4) == 8 && slots_of(8, 8) == 1 };
    proof_assert! { exist_cond(*s0, 4, 8u64) };
    let h4 = r.alloc_extent(4);
    proof_assert! { exist_case(*s0, r, 4, 8u64, h4) };
    proof_assert! { match h4 { Ok((s, i, off)) => s@ == 8 && i@ == 1 && off@ == 12, Err(_) => false } };
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
    let before = snapshot! { r };
    proof_assert! { before.slabs@.lookup(8).keys@[1] == FREE_KEY && before.slabs@.lookup(8).keys@.len() == 2 };
    // publish(H4) (lib.rs:610 -> region.rs:114-119)
    r.publish_slot(8, 1, W6P_K4);
    proof_assert! { r.slabs@.lookup(8).keys@[0] == WP_K3 && r.slabs@.lookup(8).keys@[1] == W6P_K4 };
    proof_assert! { slot_off(r.slabs@.lookup(8), 1) == 12 && W6P_K4 != FREE_KEY };
    let after = snapshot! { r };
    (before, after)
}
