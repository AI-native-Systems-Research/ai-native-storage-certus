//! Batch 3 — 23 further in-scope properties of `extent-manager` (pin 2cd35bac).
//!
//! One scored module per inventory id: `verify_<id>` (proof) or `refute_<id>`
//! (machine-checked negation), each with a `__mutant` twin that must FAIL. Unscored
//! `witness_<id>_pow2` modules show that a BUDDY-MASK refutation does not happen when the
//! sector size is a power of two (CREUSOT.md, ADDED section).
use crate::model::b3::*;
use crate::model::bitmap::*;
use crate::model::blockio::*;
use crate::model::buddy::*;
use crate::model::ckpt::*;
use crate::model::component::*;
use crate::model::listing::*;
use crate::model::params::*;
use crate::model::region::*;
use crate::model::slab::*;
use crate::props::batch1::*;
use crate::props::batch2::*;
use crate::props::witness::*;
use creusot_std::logic::FMap;
use crate::model::assume::*;
use crate::model::l2::{fdisj, fv, p2};
use crate::model::l2m::{coal, lemma_coal_single};
use crate::model::b8::rm_safe8;
use crate::model::l2r::{lemma_rg_good_o, pv, rg_core, rg_good, sk, rg_acct, rg_ka, sc_ok, sl_away};
use crate::props::l2a::lg;
use crate::props::l3start::*;
use creusot_std::prelude::*;

// =============================================================================
// Recovery: slab rebuild
// =============================================================================

/// **EM-RECOVER-SLAB-FROM-DESCRIPTOR** — "A slab rebuilt from a checkpoint has exactly the
/// slots with real keys marked allocated, carrying those keys, and every other slot free."
/// `recovery::slab_from_descriptor` (recovery.rs:76-85) for every descriptor whose key
/// vector has the slab's slot count (what serialize_region wrote, checkpoint.rs:22-29).
// LEVEL-3 D-B1: the descriptor premise is now the declared range 980f61 for a descriptor of
// the region [base, base+size) (+ the decoder fact t_dec_keys), the region satisfying the
// PROVED invariant em_rg_built (rg_built: base + size <= data_disk_size, a u64); the former
// `keys.len == slots_of` and `start + slab <= u64::MAX` are DERIVED (lemma_desc_slots).
#[requires(a_980f61_d(*desc, base@, size@) && t_dec_keys(*desc) && rg_built(base@, size@, fp))]
#[ensures(result.bitmap.num_slots@ == desc.keys@.len() && result.keys@.len() == desc.keys@.len())]
#[ensures(forall<i: Int> 0 <= i && i < desc.keys@.len() ==>
    (slot_bit(result.bitmap, i) == (desc.keys@[i] != FREE_KEY)) && result.keys@[i] == desc.keys@[i])]
#[ensures(forall<i: Int> 0 <= i && i < desc.keys@.len() && !slot_bit(result.bitmap, i) ==> result.keys@[i] == FREE_KEY)]
#[ensures(result.start_offset == desc.start_offset && result.slab_size == desc.slab_size && result.element_size == desc.element_size)]
#[ensures(slab_inv(result))]
pub fn verify_em_recover_slab_from_descriptor(desc: &SlabDescriptor, base: u64, size: u64, fp: FormatParams) -> Slab {
    proof_assert! { lemma_desc_slots(*desc, base@, size@); desc.keys@.len() == slots_of(desc.slab_size@, desc.element_size@) };
    proof_assert! { desc.start_offset@ + desc.slab_size@ <= u64::MAX@ };
    slab_from_descriptor(desc)
}

/// Mutant: claims slot 0 is always allocated (false when the descriptor's key 0 is FREE).
#[requires(desc.element_size@ > 0)]
#[requires(desc.keys@.len() == slots_of(desc.slab_size@, desc.element_size@))]
#[requires(desc.start_offset@ + desc.slab_size@ <= u64::MAX@)]
#[requires(desc.keys@.len() > 0)]
#[ensures(slot_bit(result.bitmap, 0))]
pub fn verify_em_recover_slab_from_descriptor__mutant(desc: &SlabDescriptor) -> Slab {
    slab_from_descriptor(desc)
}

// =============================================================================
// Metadata addressing defaults
// =============================================================================

/// **EM-SETMBASE-POST-DEFAULT-ZERO** — "If set_metadata_base_lba() has never been called,
/// the metadata base offset is zero and the superblock is read and written at block 0 of
/// the metadata namespace." `new_inner` (lib.rs:110-171) builds `metadata_base_lba` with
/// `Default` (0); every metadata client is built by `get_metadata_client` (lib.rs:189-213),
/// which captures it (lib.rs:204); the superblock is `write_blocks(0, ..)` / `read_blocks(0,
/// SUPERBLOCK_SIZE)` (lib.rs:498, 306; recovery.rs:15). For every metadata sector size (level 3:
/// widened from `<= 4096`) and namespace: the superblock write's commands go to blocks 0, 1, .. of that
/// namespace (block 0 first), and so do the superblock read's.
#[requires(ss@ > 0)]
#[requires(sb_bytes@.len() == 4096)]
#[ensures(result.0.metadata_base_lba@ == 0)]
#[ensures(forall<p: Int> (*bd.log).len() <= p && p < *result.2 ==> (*(^bd).log)[p] == (ns@, p - (*bd.log).len()))]
#[ensures(forall<p: Int> *result.2 <= p && p < (*(^bd).log).len() ==> (*(^bd).log)[p] == (ns@, p - *result.2))]
#[ensures(result.1 == Ok(()) ==> (*bd.log).len() < *result.2 && (*(^bd).log)[(*bd.log).len()] == (ns@, 0))]
pub fn verify_em_setmbase_post_default_zero(
    dev: MetaDevice, ns: u32, ss: u32, bd: &mut BDev, sb_bytes: &Vec<u8>,
) -> (ExtentManager, Result<(), EmError>, Snapshot<Int>, Result<Vec<u8>, EmError>) {
    let em = new_inner(dev);
    let c = em.get_metadata_client(ns, ss);
    proof_assert! { lemma_divmod(4096 + ss@ - 1, ss@); lemma_mul_le(1, ss@, (4096 + ss@ - 1) / ss@);
        (4096 + ss@ - 1) / ss@ * ss@ <= 4096 + ss@ - 1 && (4096 + ss@ - 1) / ss@ <= 4096 && (4096 + ss@ - 1) / ss@ >= 1 };
    let w = c.write_blocks(bd, 0, sb_bytes);
    let mid = snapshot! { (*bd.log).len() };
    let r = c.read_blocks(bd, 0, 4096);
    (em, w, mid, r)
}

/// Mutant: claims the superblock write starts at block 1.
#[requires(ss@ > 0)]
#[requires(sb_bytes@.len() == 4096)]
#[ensures(result.1 == Ok(()) ==> (*(^bd).log)[(*bd.log).len()] == (ns@, 1))]
pub fn verify_em_setmbase_post_default_zero__mutant(
    dev: MetaDevice, ns: u32, ss: u32, bd: &mut BDev, sb_bytes: &Vec<u8>,
) -> (ExtentManager, Result<(), EmError>) {
    let em = new_inner(dev);
    let c = em.get_metadata_client(ns, ss);
    proof_assert! { lemma_divmod(4096 + ss@ - 1, ss@); lemma_mul_le(1, ss@, (4096 + ss@ - 1) / ss@);
        (4096 + ss@ - 1) / ss@ * ss@ <= 4096 + ss@ - 1 && (4096 + ss@ - 1) / ss@ <= 4096 && (4096 + ss@ - 1) / ss@ >= 1 };
    let w = c.write_blocks(bd, 0, sb_bytes);
    (em, w)
}

/// **EM-CKPT-USES-FORMATTED-NAMESPACE** — "Checkpoints write to the metadata namespace
/// recorded at format time (or in the recovered superblock), not to some other namespace."
/// run_checkpoint takes `shared.format_params.metadata_disk_ns_id` (lib.rs:291-296, mirror
/// `ckpt_ns`) and builds its client for it (lib.rs:297). (a) after a successful format that
/// is `params.metadata_disk_ns_id` (lib.rs:501); (b) after initialize it is the recovered
/// superblock's `metadata_disk_ns_id` (lib.rs:529); (c) no reserve/publish/abort/remove/
/// checkpoint changes it (format_params frame); (d) every block command of a write on the
/// client built for it goes to that namespace (block_io.rs:84-91).
#[requires(em_regions_wf(*f))]
#[requires(f.dev.connected ==> a_67ea4f(f.dev.num_sectors@, f.dev.sector_size@))]
#[requires(a_2ee144(params.metadata_alignment@))]
#[requires(sane_params(params))]
#[requires(em_regions_wf(*g) && sb_sane(sb) && recovered_ok(sb, per_region@))]
#[requires(em_ok(*e) && cop_pre(*e, op))]
#[requires(ss@ > 0)]
#[requires(data@.len() + ss@ <= usize::MAX@)]
#[requires((data@.len() + ss@ - 1) / ss@ * ss@ <= usize::MAX@)]
#[requires(e.metadata_base_lba@ + lba@ + (data@.len() + ss@ - 1) / ss@ <= u64::MAX@)]
#[ensures(match result.0 { Ok(()) => result.1 == params.metadata_disk_ns_id, Err(_) => true })]
#[ensures(result.2 == sb.metadata_disk_ns_id)]
#[ensures(result.3 == result.4)]
#[ensures(forall<p: Int> (*bd.log).len() <= p && p < (*(^bd).log).len() ==> (*(^bd).log)[p].0 == result.4@)]
pub fn verify_em_ckpt_uses_formatted_namespace(
    f: &mut ExtentManager, params: FormatParams,
    g: &mut ExtentManager, sb: Superblock, per_region: &Vec<Vec<SlabDescriptor>>,
    e: &mut ExtentManager, op: COp,
    ss: u32, bd: &mut BDev, lba: u64, data: &Vec<u8>,
) -> (Result<(), EmError>, u32, u32, u32, u32, Result<(), EmError>) {
    let r = f.format(params);
    let a = f.ckpt_ns();
    g.initialize(sb, per_region);
    let b = g.ckpt_ns();
    let before = e.ckpt_ns();
    apply_cop(e, op);
    let after = e.ckpt_ns();
    let c = e.get_metadata_client(after, ss);
    let w = c.write_blocks(bd, lba, data);
    (r, a, b, before, after, w)
}

/// Mutant: claims a formatted component checkpoints to namespace 1 whatever was formatted.
#[requires(em_regions_wf(*f))]
#[requires(f.dev.connected ==> a_67ea4f(f.dev.num_sectors@, f.dev.sector_size@))]
#[requires(a_2ee144(params.metadata_alignment@))]
#[requires(sane_params(params))]
#[ensures(match result.0 { Ok(()) => result.1@ == 1, Err(_) => true })]
pub fn verify_em_ckpt_uses_formatted_namespace__mutant(f: &mut ExtentManager, params: FormatParams) -> (Result<(), EmError>, u32) {
    let r = f.format(params);
    let a = f.ckpt_ns();
    (r, a)
}

// =============================================================================
// Checkpoint sequence
// =============================================================================

/// **EM-INV-SEQ-AGREE** — "The component's recorded checkpoint sequence number always equals
/// the sequence number held in its in-memory superblock." Maintained invariant `seq_agree`:
/// established by format (lib.rs:500-504: 0 and Superblock::new's 0) and by initialize
/// (lib.rs:570-574: both `sb.checkpoint_seq`), preserved by every reserve / publish / abort /
/// remove (no write to `shared`) and checkpoint (checkpoint.rs:106-112 writes both under the
/// same `shared` lock; a failed checkpoint writes neither). A failed format leaves `shared`
/// unchanged.
#[requires(em_regions_wf(*f) && seq_agree(*f))]
#[requires(f.dev.connected ==> a_67ea4f(f.dev.num_sectors@, f.dev.sector_size@))]
#[requires(a_2ee144(params.metadata_alignment@))]
#[requires(sane_params(params))]
#[requires(em_regions_wf(*g) && sb_sane(sb) && recovered_ok(sb, per_region@))]
#[requires(em_ok(*e) && cop_pre(*e, op) && seq_agree(*e))]
#[ensures(seq_agree(^f))]
#[ensures(seq_agree(^g))]
#[ensures(seq_agree(^e))]
pub fn verify_em_inv_seq_agree(
    f: &mut ExtentManager, params: FormatParams,
    g: &mut ExtentManager, sb: Superblock, per_region: &Vec<Vec<SlabDescriptor>>,
    e: &mut ExtentManager, op: COp,
) -> Result<(), EmError> {
    let r = f.format(params);
    g.initialize(sb, per_region);
    apply_cop(e, op);
    r
}

/// Mutant: claims a call leaves the two sequence numbers different.
#[requires(em_ok(*e) && cop_pre(*e, op) && seq_agree(*e))]
#[requires(e.shared != None)]
#[ensures(!seq_agree(^e))]
pub fn verify_em_inv_seq_agree__mutant(e: &mut ExtentManager, op: COp) {
    apply_cop(e, op);
}

// =============================================================================
// Metadata block I/O error paths and allocator (event mirror, model/b3.rs)
// =============================================================================

/// **EM-BIO-ALLOC-ERR** — "If a DMA buffer for metadata I/O cannot be allocated, the read or
/// write fails with an IoError and nothing is sent to the device for that block." Mirrors of
/// write_blocks (block_io.rs:49-108) and read_blocks (block_io.rs:110-155) for every client,
/// LBA and length and every allocator/device behaviour: an allocation failure
/// (`alloc_buffer`, block_io.rs:42-47 → `nvme_to_em`) is the LAST event of the call and the
/// call returns `IoError`; every command send is immediately preceded by its own block's
/// successful allocation.
#[requires(c.sector_size@ > 0)]
#[requires(data@.len() + c.sector_size@ <= usize::MAX@)]
#[requires((data@.len() + c.sector_size@ - 1) / c.sector_size@ * c.sector_size@ <= usize::MAX@)]
#[requires(c.base_lba@ + lba@ + (data@.len() + c.sector_size@ - 1) / c.sector_size@ <= u64::MAX@)]
#[requires(n@ + c.sector_size@ <= usize::MAX@)]
#[requires(c.base_lba@ + rlba@ + (n@ + c.sector_size@ - 1) / c.sector_size@ <= u64::MAX@)]
#[ensures(forall<p: Int> wd.log.len() <= p && p < (^wd).log.len() && ev_alloc_fail((*(^wd).log)[p]) ==>
    p == (^wd).log.len() - 1 && result.0 == Err(EmError::IoError))]
#[ensures(forall<p: Int> wd.log.len() <= p && p < (^wd).log.len() && ev_is_send((*(^wd).log)[p]) ==>
    wd.log.len() < p && (*(^wd).log)[p - 1] == Ev::Alloc(c.alloc, true))]
#[ensures(forall<p: Int> rd.log.len() <= p && p < (^rd).log.len() && ev_alloc_fail((*(^rd).log)[p]) ==>
    p == (^rd).log.len() - 1 && result.1 == Err(EmError::IoError))]
#[ensures(forall<p: Int> rd.log.len() <= p && p < (^rd).log.len() && ev_is_send((*(^rd).log)[p]) ==>
    rd.log.len() < p && (*(^rd).log)[p - 1] == Ev::Alloc(c.alloc, true))]
pub fn verify_em_bio_alloc_err(
    c: &EvClient, wd: &mut EvDev, lba: u64, data: &Vec<u8>, rd: &mut EvDev, rlba: u64, n: usize,
) -> (Result<(), EmError>, Result<(), EmError>) {
    let w = c.write_blocks(wd, lba, data);
    let r = c.read_blocks(rd, rlba, n);
    (w, r)
}

/// Mutant: claims an allocation failure still lets the write succeed.
#[requires(c.sector_size@ > 0)]
#[requires(data@.len() + c.sector_size@ <= usize::MAX@)]
#[requires((data@.len() + c.sector_size@ - 1) / c.sector_size@ * c.sector_size@ <= usize::MAX@)]
#[requires(c.base_lba@ + lba@ + (data@.len() + c.sector_size@ - 1) / c.sector_size@ <= u64::MAX@)]
#[ensures(forall<p: Int> wd.log.len() <= p && p < (^wd).log.len() && ev_alloc_fail((*(^wd).log)[p]) ==> result == Ok(()))]
pub fn verify_em_bio_alloc_err__mutant(c: &EvClient, wd: &mut EvDev, lba: u64, data: &Vec<u8>) -> Result<(), EmError> {
    c.write_blocks(wd, lba, data)
}

/// **EM-BIO-WRITE-ERR** — "If the device reports an error, returns an unexpected completion,
/// or its channel closes during a metadata write, the write fails with an IoError and no
/// further blocks of that write are sent." write_blocks mirror (block_io.rs:82-104): a failed
/// send (closed command channel), a failed receive (closed completion channel), an
/// `Error` completion, an unexpected completion kind, or a `WriteDone` carrying an error is
/// the LAST event of the call — nothing is sent after it — and the call returns `IoError`
/// (`io_error` / `nvme_to_em`, error.rs:15-25). Every failure of the call is `IoError`.
#[requires(c.sector_size@ > 0)]
#[requires(data@.len() + c.sector_size@ <= usize::MAX@)]
#[requires((data@.len() + c.sector_size@ - 1) / c.sector_size@ * c.sector_size@ <= usize::MAX@)]
#[requires(c.base_lba@ + lba@ + (data@.len() + c.sector_size@ - 1) / c.sector_size@ <= u64::MAX@)]
#[ensures(forall<p: Int> wd.log.len() <= p && p < (^wd).log.len() && ev_dev_fail((*(^wd).log)[p], true) ==>
    p == (^wd).log.len() - 1 && result == Err(EmError::IoError))]
#[ensures(forall<e: EmError> result == Err(e) ==> e == EmError::IoError)]
pub fn verify_em_bio_write_err(c: &EvClient, wd: &mut EvDev, lba: u64, data: &Vec<u8>) -> Result<(), EmError> {
    c.write_blocks(wd, lba, data)
}

/// Mutant: claims a device failure is followed by a further event of the same write.
#[requires(c.sector_size@ > 0)]
#[requires(data@.len() + c.sector_size@ <= usize::MAX@)]
#[requires((data@.len() + c.sector_size@ - 1) / c.sector_size@ * c.sector_size@ <= usize::MAX@)]
#[requires(c.base_lba@ + lba@ + (data@.len() + c.sector_size@ - 1) / c.sector_size@ <= u64::MAX@)]
#[ensures(forall<p: Int> wd.log.len() <= p && p < (^wd).log.len() && ev_dev_fail((*(^wd).log)[p], true) ==>
    p < (^wd).log.len() - 1)]
pub fn verify_em_bio_write_err__mutant(c: &EvClient, wd: &mut EvDev, lba: u64, data: &Vec<u8>) -> Result<(), EmError> {
    c.write_blocks(wd, lba, data)
}

/// **EM-DMA-POST-ALLOCATOR-USED** — "After set_dma_alloc(alloc) on the concrete
/// ExtentManager, every I/O buffer used for metadata-device transfers is obtained from the
/// supplied allocator; without it, the default DMA buffer allocator is used."
/// set_dma_alloc (lib.rs:173-175) stores the allocator; every metadata transfer of format /
/// initialize / checkpoint goes through a client built by get_metadata_client (lib.rs:496,
/// 518, 297), which takes `dma_alloc.clone().unwrap_or_else(<DmaBuffer::new>)`
/// (lib.rs:197-201); every buffer write_blocks / read_blocks use comes from the client's
/// `alloc_buffer` (block_io.rs:42-47, 57, 73, 120). For every allocator id, namespace,
/// sector size, transfer and device behaviour: after set_dma_alloc(a) every allocation
/// event of a write and of a read is `User(a)`; on a fresh component (never set) every
/// allocation event of a write is `Default`.
#[requires(ss@ > 0)]
#[requires(data@.len() + ss@ <= usize::MAX@)]
#[requires((data@.len() + ss@ - 1) / ss@ * ss@ <= usize::MAX@)]
#[requires(cfg.metadata_base_lba@ + lba@ + (data@.len() + ss@ - 1) / ss@ <= u64::MAX@)]
#[requires(lba@ + (data@.len() + ss@ - 1) / ss@ <= u64::MAX@)]
#[requires(n@ + ss@ <= usize::MAX@)]
#[requires(cfg.metadata_base_lba@ + rlba@ + (n@ + ss@ - 1) / ss@ <= u64::MAX@)]
#[ensures(forall<p: Int> wd.log.len() <= p && p < (^wd).log.len() ==> ev_alloc_is((*(^wd).log)[p], AllocSel::User(a)))]
#[ensures(forall<p: Int> rd.log.len() <= p && p < (^rd).log.len() ==> ev_alloc_is((*(^rd).log)[p], AllocSel::User(a)))]
#[ensures(forall<p: Int> dd.log.len() <= p && p < (^dd).log.len() ==> ev_alloc_is((*(^dd).log)[p], AllocSel::Default))]
pub fn verify_em_dma_post_allocator_used(
    cfg: &mut DmaCfg, a: u64, ns: u32, ss: u32,
    wd: &mut EvDev, lba: u64, data: &Vec<u8>, rd: &mut EvDev, rlba: u64, n: usize, dd: &mut EvDev,
) -> (Result<(), EmError>, Result<(), EmError>, Result<(), EmError>) {
    cfg.set_dma_alloc(a);
    let c = cfg.get_metadata_client(ns, ss);
    let w = c.write_blocks(wd, lba, data);
    let r = c.read_blocks(rd, rlba, n);
    let fresh = DmaCfg::new_default();
    let c0 = fresh.get_metadata_client(ns, ss);
    let w0 = c0.write_blocks(dd, lba, data);
    (w, r, w0)
}

/// Mutant: claims the default allocator is still used after set_dma_alloc.
#[requires(ss@ > 0)]
#[requires(data@.len() + ss@ <= usize::MAX@)]
#[requires((data@.len() + ss@ - 1) / ss@ * ss@ <= usize::MAX@)]
#[requires(cfg.metadata_base_lba@ + lba@ + (data@.len() + ss@ - 1) / ss@ <= u64::MAX@)]
#[ensures(forall<p: Int> wd.log.len() <= p && p < (^wd).log.len() ==> ev_alloc_is((*(^wd).log)[p], AllocSel::Default))]
pub fn verify_em_dma_post_allocator_used__mutant(
    cfg: &mut DmaCfg, a: u64, ns: u32, ss: u32, wd: &mut EvDev, lba: u64, data: &Vec<u8>,
) -> Result<(), EmError> {
    cfg.set_dma_alloc(a);
    let c = cfg.get_metadata_client(ns, ss);
    c.write_blocks(wd, lba, data)
}

/// **EM-ERR-DEVICE-AS-IOERROR** — REFUTED (INDEPENDENT; new root cause RECOVER-IOERR-MASKED:
/// recovery.rs:29-45 and 50-66 discard the error of each checkpoint-copy read and
/// recovery.rs:68-70 returns CorruptMetadata, so a device read failure on the checkpoint
/// copies reaches initialize()'s caller as CorruptMetadata). "Every failure reported by the
/// metadata block device reaches the caller as an IoError." Witness (LEVEL-3: restarted from a
/// PARITY-CONSISTENT superblock): superblock read fine, checkpoint_seq 2, active copy 0 —
/// exactly what the device holds after format (seq 0 / copy 0) and two successful checkpoints
/// (copy 1 / seq 1, then copy 0 / seq 2); the device fails BOTH copy reads (read_blocks
/// surfaces each as IoError, block_io.rs:136-151 → error.rs:23-25); initialize() → recover
/// returns Err(CorruptMetadata). (With checkpoint_seq 1 / copy 1 a single failed read of the
/// active copy suffices: prev_seq 0 skips the fallback, recovery.rs:49.)
#[ensures(result.0 == Err(EmError::IoError) && result.1 == Err(EmError::IoError))]
#[ensures(result.2 == Err(EmError::CorruptMetadata))]
pub fn refute_em_err_device_as_ioerror() -> (Result<(), EmError>, Result<(), EmError>, Result<Recovered, EmError>) {
    // LEVEL-3 reachable start: the superblock of format (wc_fp(8, 8) on 3 x 4096 bytes) after
    // two checkpoints — every format-start range, 0de740 / 9c0757 / a23fc2 / 91a1f6 / 23f855 on
    // it, and the parity active_copy == checkpoint_seq % 2 (seq 2 / copy 0).
    let mut sb2 = l3_sb_c8();
    sb2.checkpoint_seq = 2;
    proof_assert! { l3_fmt_ranges(wc_fp_l(8u64, 8u64), 3, 4096, sb2) };
    proof_assert! { sb2.checkpoint_seq@ == 2 && sb2.active_copy@ == 0 && sb2.active_copy@ == sb2.checkpoint_seq@ % 2 };
    let ra = read_ckpt(RdOut::DevErr, 2);
    let rb = read_ckpt(RdOut::DevErr, 1);
    let rec = recover_rd(sb2.checkpoint_seq, sb2.active_copy, RdOut::DevErr, RdOut::DevErr);
    (ra, rb, rec)
}

/// Mutant: claims the caller sees the device failure as IoError.
#[ensures(result == Err(EmError::IoError))]
pub fn refute_em_err_device_as_ioerror__mutant() -> Result<Recovered, EmError> {
    recover_rd(2, 0, RdOut::DevErr, RdOut::DevErr)
}

// =============================================================================
// used_bytes <= capacity_bytes
// =============================================================================

/// One region's term of `used_bytes` (lib.rs:704) in a RELEASE build:
/// `total_usable_size() - total_free()` with wrapping subtraction (a debug build panics on
/// the underflow instead).
#[requires(bd_shape(*b))]
#[requires(tf(b.free_lists@, b.sector_size@, b.free_lists@.len()) <= u64::MAX@)]
#[ensures(result@ == if tf(b.free_lists@, b.sector_size@, b.free_lists@.len()) <= b.total_usable_size@ {
    b.total_usable_size@ - tf(b.free_lists@, b.sector_size@, b.free_lists@.len())
} else {
    b.total_usable_size@ - tf(b.free_lists@, b.sector_size@, b.free_lists@.len()) + 18446744073709551616
})]
pub fn region_used_release(b: &BuddyAllocator) -> u64 {
    let u = b.total_usable_size();
    let f = b.total_free();
    if f <= u { u - f } else { u + (u64::MAX - f) + 1 }
}

/// **EM-USED-AT-MOST-CAPACITY** — REFUTED. "used_bytes() is always less than or equal to
/// capacity_bytes()." ROOT CAUSE = BUDDY-MASK (non-power-of-two sector size). W1 (sector 3,
/// one region [0,6), capacity_bytes() = 6): after initialize the recovered slab B at 3 was
/// never taken off the free lists (buddy.rs:136), and remove_extent(3) + checkpoint return
/// its block a second time (region.rs:104) — free space 9 > capacity 6, so
/// `total_usable_size - total_free` (lib.rs:704) underflows: a debug build panics inside
/// used_bytes(), a release build returns 2^64 - 3 > 6.
#[ensures(result.0@ == 6 && result.1@ == 9)]
#[ensures(result.2@ == 18446744073709551613 && result.2@ > result.0@)]
pub fn support_old_refute_em_used_at_most_capacity() -> (u64, u64, u64) {
    let a = w1_freed();
    proof_assert! { lemma_tf2(a.free_lists@, 3); tf(a.free_lists@, 3, 2) == 9 };
    let cap = a.total_usable_size();
    let free = a.total_free();
    proof_assert! { bd_shape(a) };
    let used = region_used_release(&a);
    (cap, free, used)
}

/// Mutant: claims used <= capacity in W1.
#[ensures(result.2@ <= result.0@)]
pub fn support_old_refute_em_used_at_most_capacity__mutant() -> (u64, u64, u64) {
    let a = w1_freed();
    proof_assert! { lemma_tf2(a.free_lists@, 3); tf(a.free_lists@, 3, 2) == 9 };
    let cap = a.total_usable_size();
    let free = a.total_free();
    proof_assert! { bd_shape(a) };
    let used = region_used_release(&a);
    (cap, free, used)
}

/// pow2 witness of EM-USED-AT-MOST-CAPACITY: W1p (the W1 history at sector 4, region [0,8))
/// ends with free space 8 = capacity 8, so used_bytes() = 0 <= capacity_bytes() = 8.
#[ensures(result.1@ - result.2@ == 0 && result.1@ - result.2@ <= result.1@)]
pub fn witness_em_used_at_most_capacity_pow2() -> (u64, u64, u64) {
    witness_em_used_returns_after_free_pow2()
}

// =============================================================================
// format(): layout, regions, instance id, emptiness
// =============================================================================

/// `ckoff_l(a) >= 4096`: copy 0 never starts inside the superblock.
#[logic]
#[requires(a >= 0)]
#[ensures(ckoff_l(a) >= 4096)]
pub fn lemma_ckoff_ge(a: Int) {
    pearlite! { if a > 0 { lemma_divmod(4096 + a - 1, a) } }
}

/// `ckoff_l(a) + 2 * cksize_l(..) <= eff_l(..)` whenever the size is non-zero.
#[logic]
#[requires(md >= 0 && mrs >= 0 && a >= 0 && ss > 0)]
#[requires(cksize_l(md, mrs, a, ss) > 0)]
#[ensures(ckoff_l(a) + 2 * cksize_l(md, mrs, a, ss) <= eff_l(md, mrs))]
pub fn lemma_cklayout_fits(md: Int, mrs: Int, a: Int, ss: Int) {
    pearlite! {
        lemma_divmod(eff_l(md, mrs) - ckoff_l(a), 2);
        lemma_divmod((eff_l(md, mrs) - ckoff_l(a)) / 2, ss)
    }
}

/// **EM-FORMAT-POST-DATA-START** — "When format() succeeds with a non-zero metadata region
/// size (shared-device mode), the recorded data start offset is the first byte immediately
/// after the reserved metadata area (superblock plus both checkpoint copies); with a metadata
/// region size of zero (separate-device mode), the data start offset is zero." format
/// (lib.rs:417-446, 483-494) for every parameter set and metadata device: on success the
/// superblock it records (and publishes in `shared`) has data_start_offset =
/// checkpoint_region_offset + 2 * checkpoint_region_size when metadata_region_size > 0, where
/// copy 0 starts after the 4096-byte superblock and the whole reserved area fits in
/// metadata_region_size; and 0 when metadata_region_size == 0.
#[requires(em_regions_wf(*f))]
#[requires(f.dev.connected ==> a_67ea4f(f.dev.num_sectors@, f.dev.sector_size@))]
#[requires(a_2ee144(params.metadata_alignment@))]
#[requires(sane_params(params))]
#[ensures(match result {
    Ok(()) => match (^f).shared {
        Some(sh) => (params.metadata_region_size@ > 0 ==>
                sh.superblock.data_start_offset@ == sh.superblock.checkpoint_region_offset@ + 2 * sh.superblock.checkpoint_region_size@
                && sh.superblock.checkpoint_region_offset@ >= 4096
                && sh.superblock.data_start_offset@ <= params.metadata_region_size@)
            && (params.metadata_region_size@ == 0 ==> sh.superblock.data_start_offset@ == 0),
        None => false,
    },
    Err(_) => true,
})]
pub fn verify_em_format_post_data_start(f: &mut ExtentManager, params: FormatParams) -> Result<(), EmError> {
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

/// Mutant: claims the data start offset is always 0.
#[requires(em_regions_wf(*f))]
#[requires(f.dev.connected ==> a_67ea4f(f.dev.num_sectors@, f.dev.sector_size@))]
#[requires(a_2ee144(params.metadata_alignment@))]
#[requires(sane_params(params))]
#[ensures(match result { Ok(()) => match (^f).shared { Some(sh) => sh.superblock.data_start_offset@ == 0, None => true }, Err(_) => true })]
pub fn verify_em_format_post_data_start__mutant(f: &mut ExtentManager, params: FormatParams) -> Result<(), EmError> {
    f.format(params)
}

/// Region `i` and `i + 1` of a format layout are adjacent; the last ends at `ds + usable`.
#[logic]
#[requires(n > 0 && 0 <= i && usable >= 0)]
#[ensures(i + 1 < n ==> rbase(ds, usable, n, i) + rsize(usable, n, i) == rbase(ds, usable, n, i + 1))]
#[ensures(i + 1 < n ==> rsize(usable, n, i) == usable / n)]
#[ensures(i == n - 1 ==> rbase(ds, usable, n, i) + rsize(usable, n, i) == ds + usable)]
pub fn lemma_partition(ds: Int, usable: Int, n: Int, i: Int) {
    pearlite! { lemma_distrib(i, 1, usable / n) }
}

/// **EM-FORMAT-POST-REGION-PARTITION** — "After format() succeeds there are exactly
/// region-count regions; they are contiguous, non-overlapping, start at the data start offset
/// and together end exactly at the data disk size, with every region except the last having
/// the same size." format's region loop (lib.rs:454-468): region i is a fresh buddy
/// allocator over [base_i, base_i + size_i) with base_i = ds + i*(usable/n) and size_i =
/// usable/n except the last (usable - (n-1)*(usable/n)), usable = data_disk_size - ds.
/// Proved for every parameter set and device: count n; region 0 starts at ds; region i ends
/// where region i+1 starts; the last ends at data_disk_size; all but the last have size
/// usable/n.
#[requires(em_regions_wf(*f))]
#[requires(f.dev.connected ==> a_67ea4f(f.dev.num_sectors@, f.dev.sector_size@))]
#[requires(a_2ee144(params.metadata_alignment@))]
#[requires(sane_params(params))]
#[ensures(match result {
    Ok(()) => match ((^f).regions, (^f).shared) {
        (Some(rv), Some(sh)) => rv@.len() == params.region_count@
            && (^f).arena@[rv@[0]@].buddy.base_offset == sh.superblock.data_start_offset
            && (forall<i: Int> 0 <= i && i + 1 < rv@.len() ==>
                (^f).arena@[rv@[i]@].buddy.base_offset@ + (^f).arena@[rv@[i]@].buddy.total_usable_size@
                    == (^f).arena@[rv@[i + 1]@].buddy.base_offset@
                && (^f).arena@[rv@[i]@].buddy.total_usable_size == (^f).arena@[rv@[0]@].buddy.total_usable_size)
            && (^f).arena@[rv@[rv@.len() - 1]@].buddy.base_offset@ + (^f).arena@[rv@[rv@.len() - 1]@].buddy.total_usable_size@
                == params.data_disk_size@,
        _ => false,
    },
    Err(_) => true,
})]
pub fn verify_em_format_post_region_partition(f: &mut ExtentManager, params: FormatParams) -> Result<(), EmError> {
    let r = f.format(params);
    proof_assert! { match (r, f.regions, f.shared) {
        (Ok(()), Some(rv), Some(sh)) => rv@.len() == params.region_count@ && rv@.len() > 0
            && sh.superblock.data_start_offset@ < params.data_disk_size@
            && forall<i: Int> 0 <= i && i < rv@.len() ==>
                f.arena@[rv@[i]@].buddy.base_offset@ == rbase(sh.superblock.data_start_offset@, params.data_disk_size@ - sh.superblock.data_start_offset@, params.region_count@, i)
                && f.arena@[rv@[i]@].buddy.total_usable_size@ == rsize(params.data_disk_size@ - sh.superblock.data_start_offset@, params.region_count@, i),
        _ => true } };
    proof_assert! { match (r, f.regions, f.shared) {
        (Ok(()), Some(rv), Some(sh)) => {
            lemma_partition(sh.superblock.data_start_offset@, params.data_disk_size@ - sh.superblock.data_start_offset@, params.region_count@, rv@.len() - 1);
            f.arena@[rv@[rv@.len() - 1]@].buddy.base_offset@ + f.arena@[rv@[rv@.len() - 1]@].buddy.total_usable_size@ == params.data_disk_size@ },
        _ => true } };
    proof_assert! { match (r, f.regions, f.shared) {
        (Ok(()), Some(rv), Some(sh)) => forall<i: Int> 0 <= i && i < rv@.len() ==> {
            lemma_partition(sh.superblock.data_start_offset@, params.data_disk_size@ - sh.superblock.data_start_offset@, params.region_count@, i);
            lemma_partition(sh.superblock.data_start_offset@, params.data_disk_size@ - sh.superblock.data_start_offset@, params.region_count@, 0);
            true },
        _ => true } };
    r
}

/// Mutant: claims the last region has the same size as the first (false when the usable
/// size is not a multiple of the region count).
#[requires(em_regions_wf(*f))]
#[requires(f.dev.connected ==> a_67ea4f(f.dev.num_sectors@, f.dev.sector_size@))]
#[requires(a_2ee144(params.metadata_alignment@))]
#[requires(sane_params(params))]
#[ensures(match result {
    Ok(()) => match (^f).regions {
        Some(rv) => (^f).arena@[rv@[rv@.len() - 1]@].buddy.total_usable_size == (^f).arena@[rv@[0]@].buddy.total_usable_size,
        None => true },
    Err(_) => true,
})]
pub fn verify_em_format_post_region_partition__mutant(f: &mut ExtentManager, params: FormatParams) -> Result<(), EmError> {
    f.format(params)
}

/// **EM-FORMAT-POST-INSTANCE-ID-GENERATED** — "When format() succeeds with no instance
/// identifier supplied, the component chooses an identifier, stores it in the persisted
/// superblock, and get_instance_id() returns that same value on every later call, including
/// after a later initialize()." (a) format with `instance_id: None` draws an id from
/// /dev/urandom (lib.rs:471-481), writes it in the superblock to the device (lib.rs:483-498;
/// `dev.sb`) and get_instance_id returns it (lib.rs:686-692); (b) a later initialize() of the
/// same component from that persisted superblock (no checkpoint yet: recovery.rs:18-21 yields
/// empty regions) returns the same id; (c) no reserve/publish/abort/remove/checkpoint changes
/// get_instance_id, and a checkpoint that rewrites the device superblock writes the in-memory
/// one, with the same id (lib.rs:302-307).
#[requires(em_regions_wf(*f))]
#[requires(f.dev.connected ==> a_67ea4f(f.dev.num_sectors@, f.dev.sector_size@))]
#[requires(a_2ee144(params.metadata_alignment@))]
#[requires(sane_params(params) && params.instance_id == None)]
#[requires(em_ok(*e) && cop_pre(*e, op))]
#[requires(em_ok(*c) && em_together(*c))]
#[requires(match c.regions { Some(rv) => forall<i: Int> 0 <= i && i < rv@.len() ==> pending_ok(c.arena@[rv@[i]@]), None => true })]
#[requires(match (c.dev.sb, c.shared) { (Some(x), Some(s)) => x.instance_id == s.superblock.instance_id, _ => true })]
#[ensures(match result.0 {
    Ok(()) => match (^f).dev.sb {
        Some(x) => result.1 == Ok(x.instance_id) && result.2 == Ok(x.instance_id),
        None => false,
    },
    Err(_) => true,
})]
#[ensures(result.3 == result.4)]
#[ensures(match result.5 {
    Ok(()) => match ((^c).dev.sb, (^c).shared) { (Some(x), Some(s)) => x.instance_id == s.superblock.instance_id, _ => true },
    Err(_) => true,
})]
#[ensures(match (c.shared, (^c).shared) { (Some(s0), Some(s1)) => s1.superblock.instance_id == s0.superblock.instance_id, _ => true })]
pub fn narrowed_verify_em_format_post_instance_id_generated(
    f: &mut ExtentManager, params: FormatParams, e: &mut ExtentManager, op: COp, c: &mut ExtentManager,
) -> (Result<(), EmError>, Result<u64, EmError>, Result<u64, EmError>, Result<u64, EmError>, Result<u64, EmError>, Result<(), EmError>) {
    let r = f.format(params);
    let mut id1 = Err(EmError::NotInitialized);
    let mut id2 = Err(EmError::NotInitialized);
    if r.is_ok() {
        id1 = f.get_instance_id();
        match f.dev.sb {
            Some(sb0) => {
                let empty: Vec<Vec<SlabDescriptor>> = Vec::new();
                proof_assert! { sb0.sector_size@ > 0 && sb0.slab_size@ % sb0.sector_size@ == 0 };
                proof_assert! { sb0.max_extent_size@ <= sb0.slab_size@ };
                proof_assert! { p2(sb0.region_count@) };
                proof_assert! { sb0.data_start_offset@ <= sb0.data_disk_size@ };
                proof_assert! { a_91a1f6(sb0) };
                proof_assert! { a_7cb7c3(sb0.slab_size@, sb0.sector_size@) && a_e0fe79(sb0.slab_size@, sb0.sector_size@) };
                proof_assert! { a_3b58ea(sb0.data_disk_size@, sb0.sector_size@) };
                proof_assert! { a_a23fc2(sb0.checkpoint_region_offset@, sb0.checkpoint_region_size@) };
                proof_assert! { sb_sane(sb0) };
                proof_assert! { recovered_ok(sb0, empty@) };
                f.initialize(sb0, &empty);
                id2 = f.get_instance_id();
            }
            None => {}
        }
    }
    let a = e.get_instance_id();
    apply_cop(e, op);
    let b = e.get_instance_id();
    let ck = c.checkpoint();
    (r, id1, id2, a, b, ck)
}

/// Mutant: claims the generated id is 0.
#[requires(em_regions_wf(*f))]
#[requires(f.dev.connected ==> a_67ea4f(f.dev.num_sectors@, f.dev.sector_size@))]
#[requires(a_2ee144(params.metadata_alignment@))]
#[requires(sane_params(params) && params.instance_id == None)]
#[ensures(match result.0 { Ok(()) => result.1 == Ok(0u64), Err(_) => true })]
pub fn narrowed_verify_em_format_post_instance_id_generated__mutant(f: &mut ExtentManager, params: FormatParams) -> (Result<(), EmError>, Result<u64, EmError>) {
    let r = f.format(params);
    let id1 = f.get_instance_id();
    (r, id1)
}

/// **EM-FORMAT-POST-EMPTY** — "Immediately after format() succeeds, get_extents() returns an
/// empty list and for_each_extent() never invokes its callback, even if extents existed
/// before the call." format publishes freshly built regions with no slabs (lib.rs:457-468,
/// 506; fresh_rg); the listing loops (lib.rs:632-678) only report slots of the current
/// regions' slabs. Proved for every prior component state (any extents in old regions,
/// which stay in the arena but are no longer current).
// LEVEL-3 (phase D-B2): format's own ranges only — fmt_sane (e0fe79 + 3b58ea, both list format) in
// place of sane_params: no 7cb7c3 (its methods omit format; the real format accepts slab_size 0,
// lib.rs:388-405, and so does the mirror now).
#[requires(em_regions_wf(*f))]
#[requires(f.dev.connected ==> a_67ea4f(f.dev.num_sectors@, f.dev.sector_size@))]
#[requires(a_2ee144(params.metadata_alignment@))]
#[requires(fmt_sane(params))]
#[ensures(match result.0 { Ok(()) => result.1@.len() == 0 && (^cb)@.len() == cb@.len(), Err(_) => true })]
pub fn verify_em_format_post_empty(f: &mut ExtentManager, params: FormatParams, cb: &mut Vec<Extent>) -> (Result<(), EmError>, Vec<Extent>) {
    let r = f.format(params);
    match r {
        Ok(()) => {
            proof_assert! { match f.regions { Some(rv) => forall<i: Int> 0 <= i && i < rv@.len() ==>
                f.arena@[rv@[i]@].slabs@ == FMap::empty(), None => false } };
            proof_assert! { match f.regions { Some(rv) => forall<i: Int, k: Int> 0 <= i && i < rv@.len() ==>
                !f.arena@[rv@[i]@].slabs@.contains(k), None => false } };
            proof_assert! { match f.regions { Some(rv) => forall<e: Extent> !listed_one(*f, rv@, e), None => false } };
            let ex = f.get_extents();
            proof_assert! { match f.regions { Some(rv) => ex@.len() > 0 ==> listed_one(*f, rv@, ex@[0]), None => false } };
            let c0 = snapshot! { *cb };
            f.for_each_extent(cb);
            proof_assert! { match f.regions { Some(rv) => cb@.len() > c0@.len() ==> listed_one(*f, rv@, cb@[c0@.len()]), None => false } };
            (r, ex)
        }
        Err(_) => (r, Vec::new()),
    }
}

/// Mutant: claims get_extents lists something right after format.
#[requires(em_regions_wf(*f))]
#[requires(f.dev.connected ==> a_67ea4f(f.dev.num_sectors@, f.dev.sector_size@))]
#[requires(a_2ee144(params.metadata_alignment@))]
#[requires(fmt_sane(params))]
#[ensures(match result.0 { Ok(()) => result.1@.len() > 0, Err(_) => true })]
pub fn verify_em_format_post_empty__mutant(f: &mut ExtentManager, params: FormatParams) -> (Result<(), EmError>, Vec<Extent>) {
    let r = f.format(params);
    match r {
        Ok(()) => {
            let ex = f.get_extents();
            (r, ex)
        }
        Err(_) => (r, Vec::new()),
    }
}

// =============================================================================
// Checkpoint copy alignment (new root cause CKPT-LAYOUT-SECTOR)
// =============================================================================

/// The CKPT-LAYOUT-SECTOR geometry: a metadata device of 4 sectors of 4096 bytes and
/// format parameters with a 512-byte DATA sector (separate-device mode, alignment 0).
#[ensures(result.0.connected && result.0.sector_size@ == 4096 && result.0.num_sectors@ == 4)]
#[ensures(result.1.sector_size@ == 512 && result.1.slab_size@ == 4096 && result.1.max_extent_size@ == 4096)]
#[ensures(result.1.region_count@ == 1 && result.1.metadata_region_size@ == 0 && result.1.metadata_alignment@ == 0)]
#[ensures(result.1.data_disk_size@ == 1048576 && result.1.instance_id == Some(7u64))]
pub fn wl_geometry() -> (MetaDevice, FormatParams) {
    let dev = MetaDevice { connected: true, sector_size: 4096, num_sectors: 4, sb: None, ckpt0: None, ckpt1: None };
    let params = FormatParams {
        data_disk_size: 1048576,
        slab_size: 4096,
        max_extent_size: 4096,
        sector_size: 512,
        region_count: 1,
        metadata_alignment: 0,
        instance_id: Some(7),
        metadata_disk_ns_id: 1,
        metadata_region_size: 0,
    };
    (dev, params)
}

/// LEVEL-3 RETIRED (support only; EM-INV-CHECKPOINT-SECTOR-ALIGNED is NOT among the 21 level-1 refutations — it was
/// excluded at level 1). Its state has checkpoint_region_size 6144 on a 4096-byte-sector metadata
/// device, i.e. OUTSIDE D-RANGE-FMT-METADATA-DEVICE-LAYOUT-dfd6b0 (the copy layout is a multiple
/// of the metadata sector size). Kept unchanged as the documented out-of-range behaviour.
/// (Level-2 text:) **EM-INV-CHECKPOINT-SECTOR-ALIGNED** — REFUTED (INDEPENDENT; new root cause
/// CKPT-LAYOUT-SECTOR: lib.rs:430-432 rounds checkpoint_region_size to the DATA sector size
/// `params.sector_size` and lib.rs:420-424 aligns copy 0 to `metadata_alignment`, never to
/// the metadata device's own sector size read at lib.rs:410-415, while checkpoint.rs:96 and
/// :124 address each copy by TRUNCATING division by that metadata sector size). "Both
/// checkpoint copies begin on a sector boundary of the metadata device, so a checkpoint is
/// written exactly where it will later be read." Witness (valid FR-002 parameters, 512-byte
/// data sectors, a 16 KiB metadata device with 4096-byte sectors): format succeeds with copy
/// 0 at 4096 and checkpoint_region_size 6144, so copy 1 starts at byte 10240 — not a multiple
/// of 4096; write_checkpoint/read_checkpoint_region address it at LBA 2 = byte 8192, INSIDE
/// copy 0 [4096, 10240); and a copy-0 checkpoint of the maximum size the size check admits
/// (16 + payload <= 6144 bytes, checkpoint.rs:69-75) is padded to 2 sectors (checkpoint.rs:
/// 90-93) and written to LBAs 1..2, i.e. over copy 1's header sector.
#[ensures(match result.0 {
    Ok(()) => result.1@ == 10240 && result.1@ % 4096 != 0
        && result.2@ == 1 && result.3@ == 2 && result.3@ * 4096 < 10240 && result.3@ * 4096 >= 4096
        && result.4@ == 6144 && (6144 + 4096 - 1) / 4096 == 2 && result.2@ + 2 - 1 == result.3@,
    Err(e) => e == EmError::IoError,
})]
pub fn support_old_refute_em_inv_checkpoint_sector_aligned() -> (Result<(), EmError>, u64, u64, u64, u64) {
    let (dev, params) = wl_geometry();
    let mut em = new_inner(dev);
    proof_assert! { em_regions_wf(em) };
    proof_assert! { fmt_ok_inputs(em, params) };
    proof_assert! { ckoff_l(0) == 4096 && eff_l(16384, 0) == 16384 };
    proof_assert! { cksize_l(16384, 0, 0, 512) == 6144 };
    proof_assert! { 3.pow2() == 8 && 4096 / 512 == 8 && p2(8) && sane_params(params) };
    proof_assert! { em.dev.num_sectors@ * em.dev.sector_size@ == 16384 };
    let r = em.format(params);
    proof_assert! { match (r, em.shared) { (Ok(()), Some(sh)) => sh.superblock.checkpoint_region_size@ == 6144
        && sh.superblock.checkpoint_region_offset@ == 4096, _ => true } };
    match &em.shared {
        Some(sh) => {
            let sb = sh.superblock;
            let off1 = sb.checkpoint_region_offset + sb.checkpoint_region_size;
            let lba0 = ckpt_lba(&sb, 0, 4096);
            let lba1 = ckpt_lba(&sb, 1, 4096);
            (r, off1, lba0, lba1, sb.checkpoint_region_size)
        }
        None => (r, 0, 0, 0, 0),
    }
}

/// Mutant: claims copy 1 starts on a metadata-sector boundary.
#[ensures(match result.0 { Ok(()) => result.1@ % 4096 == 0, Err(_) => true })]
pub fn support_old_refute_em_inv_checkpoint_sector_aligned__mutant() -> (Result<(), EmError>, u64, u64, u64, u64) {
    let (dev, params) = wl_geometry();
    let mut em = new_inner(dev);
    proof_assert! { em_regions_wf(em) };
    proof_assert! { fmt_ok_inputs(em, params) };
    proof_assert! { ckoff_l(0) == 4096 && eff_l(16384, 0) == 16384 };
    proof_assert! { cksize_l(16384, 0, 0, 512) == 6144 };
    proof_assert! { 3.pow2() == 8 && 4096 / 512 == 8 && p2(8) && sane_params(params) };
    proof_assert! { em.dev.num_sectors@ * em.dev.sector_size@ == 16384 };
    let r = em.format(params);
    proof_assert! { match (r, em.shared) { (Ok(()), Some(sh)) => sh.superblock.checkpoint_region_size@ == 6144
        && sh.superblock.checkpoint_region_offset@ == 4096, _ => true } };
    match &em.shared {
        Some(sh) => {
            let sb = sh.superblock;
            let off1 = sb.checkpoint_region_offset + sb.checkpoint_region_size;
            let lba0 = ckpt_lba(&sb, 0, 4096);
            let lba1 = ckpt_lba(&sb, 1, 4096);
            (r, off1, lba0, lba1, sb.checkpoint_region_size)
        }
        None => (r, 0, 0, 0, 0),
    }
}

// =============================================================================
// remove_extent + checkpoint
// =============================================================================

/// Witness (C4, as `wck_released`): reserve K1 (wc4_one); publish(K1); checkpoint() #1
/// begins and serialises the region (K1 present); remove_extent(0) runs and RETURNS before
/// #1 clears and flushes (no lock spans lib.rs:299 -> :316); #1 completes successfully.
/// Then checkpoint() #2 is CALLED AFTER the removal returned (a caller's checkpoint() that
/// found #1 in progress waits for it and runs its own round, lib.rs:729-750): every region
/// is clean (lib.rs:316-320 cleared the dirty flag the removal set), so #2 returns Ok
/// without I/O (lib.rs:284-286). A fresh component recovers what the device holds: K1 at
/// offset 0. Returns (#1 result, removal result, #2 result, recovered image).
#[ensures(result.0 == Ok(()) && result.1 == Ok(()) && result.2 == Ok(()))]
#[ensures(match result.3 { Ok(img) => img_lists(*img, Extent { key: WK1, size: 4u32, offset: 0u64 }), Err(_) => false })]
pub fn wck_removed_then_ckpt() -> (Result<(), EmError>, Result<(), EmError>, Result<(), EmError>, Result<Snapshot<Seq<RegionState>>, EmError>) {
    let r = wc4_one();
    let r0 = snapshot! { r };
    let mut rs: Vec<RegionState> = Vec::new();
    rs.push(r);
    let mut em = CkEm { regions: rs, active: 0, seq: 0, dev: wc_fresh_dev() };
    proof_assert! { em.regions@.len() == 1 && em.regions@[0] == *r0 };
    // LEVEL-3 reachable start: format's superblock (all format-start ranges), proved region
    // invariants with the live handle K1, the reserve ranges, CkEm = that superblock.
    let sb0 = l3_sb_c4();
    proof_assert! { l3_fmt_ranges(wc_fp_l(4u64, 4u64), 3, 4096, sb0) && l3_rsv_ranges(4, wc_fp_l(4u64, 4u64)) };
    proof_assert! { rg_l3(em.regions@[0], hs_w1()) && regions_ok(em.regions@) };
    proof_assert! { em.active@ == sb0.active_copy@ && em.seq@ == sb0.checkpoint_seq@ && em.dev.sb_seq@ == sb0.checkpoint_seq@ && em.dev.sb_active@ == sb0.active_copy@ };
    // publish(K1)
    em.regions[0].publish_slot(0, 0, WK1);
    let s1 = snapshot! { em.regions@[0].slabs@.lookup(0) };
    proof_assert! { s1.keys@[0] == WK1 && s1.bitmap.num_slots@ == 1 && slot_bit(s1.bitmap, 0) };
    proof_assert! { regions_ok(em.regions@) };
    // checkpoint() #1 phase 1: snapshot with K1 at slot 0
    let img = match em.ck_begin() {
        Some(img) => img,
        None => return (Err(EmError::IoError), Err(EmError::IoError), Err(EmError::IoError), Err(EmError::IoError)),
    };
    // remove_extent(0) interleaves and returns Ok
    let pre_rm = snapshot! { em.regions@[0] };
    proof_assert! { slot_off(*s1, 0) == 0 && s1.start_offset@ == 0 };
    proof_assert! { rm_case(*pre_rm, 0u64, 0u64, 0usize) };
    let rm = em.regions[0].remove_extent_by_offset(0);
    proof_assert! { rm == Ok(()) && rm_post(*pre_rm, em.regions@[0], 0u64, 0usize) };
    proof_assert! { em.regions@[0].pending_frees@ == Seq::singleton((0u64, 0usize)) };
    proof_assert! { rg_inv(em.regions@[0]) };
    proof_assert! { pending_ok(em.regions@[0]) };
    // checkpoint() #1 phases 2-4 + superblock write succeed; then clear dirty + flush
    let c = em.ck_write(img, true, true);
    proof_assert! { c == Ok(()) };
    em.ck_finish();
    proof_assert! { all_clean(em.regions@) && regions_ok(em.regions@) };
    let dev1 = snapshot! { em.dev };
    // checkpoint() #2, called after remove_extent(0) returned
    let c2 = em.run_checkpoint(true, true);
    proof_assert! { em.dev == *dev1 };
    let rec = recover(&em.dev);
    proof_assert! { em.dev.sb_seq@ == 1 && em.dev.sb_active@ == 1 };
    proof_assert! { match em.dev.copy1 { Some(cc) => cc.seq@ == 1 && cc.img == img, None => false } };
    proof_assert! { img.len() == 1 && img[0].slabs@.contains(0) && img[0].slabs@.lookup(0) == *s1 };
    proof_assert! { from_slot(img[0], 0, 0, Extent { key: WK1, size: 4u32, offset: 0u64 }) };
    (c, rm, c2, rec)
}

/// **EM-REMOVE-POST-PERSISTED-BY-CHECKPOINT** — REFUTED (INDEPENDENT; root cause
/// CKPT-SNAPSHOT-RACE: lib.rs:312-321 clears `dirty` for changes made after
/// checkpoint.rs:51-55 serialised the region). "An extent removed before a successful
/// checkpoint is not listed after a fresh component recovers from the metadata device."
/// `wck_removed_then_ckpt`: remove_extent(0) of the published K1 returns Ok while the
/// background checkpoint #1 is between its snapshot and its commit; checkpoint() #2, called
/// after the removal, returns Ok — without writing anything, because #1 cleared the dirty
/// flag the removal had set; the device's only checkpoint still records K1 at offset 0, so
/// a fresh component's recovery lists it. Reachable by a single-threaded caller (the 30 s
/// background checkpoint thread is #1).
#[ensures(result.0 == Ok(()) && result.1 == Ok(()))]
#[ensures(match result.2 { Ok(img) => img_lists(*img, Extent { key: WK1, size: 4u32, offset: 0u64 }), Err(_) => false })]
pub fn refute_em_remove_post_persisted_by_checkpoint() -> (Result<(), EmError>, Result<(), EmError>, Result<Snapshot<Seq<RegionState>>, EmError>) {
    let (_c1, rm, c2, rec) = wck_removed_then_ckpt();
    (rm, c2, rec)
}

/// Mutant: claims the checkpoint after the removal failed (so the property would hold vacuously).
#[ensures(result.1 != Ok(()))]
pub fn refute_em_remove_post_persisted_by_checkpoint__mutant() -> (Result<(), EmError>, Result<(), EmError>, Result<Snapshot<Seq<RegionState>>, EmError>) {
    let (_c1, rm, c2, rec) = wck_removed_then_ckpt();
    (rm, c2, rec)
}

/// The removed slot `(s, i)` is reusable in region `b`: its slab is still there with the
/// slot's bit clear and is listed under its element size (so reserve_extent of that size
/// class finds it, region.rs:47-60), or the slab itself was released to the buddy
/// allocator (removed from the slab map, region.rs:104-107).
#[logic(open)]
pub fn slot_reusable(b: RegionState, s: u64, i: usize) -> bool {
    pearlite! {
        match b.slabs@.get(s@) {
            Some(sl) => !slot_bit(sl.bitmap, i@) && listed(sc_get(b.size_classes, sl.element_size@), s@),
            None => true,
        }
    }
}

/// **EM-REMOVE-POST-REUSABLE-AFTER-CHECKPOINT** — "After a checkpoint that persists a removal
/// completes successfully, the removed extent's slot is freed and may be handed out by
/// subsequent reservations in the same region." For every region state (invariant +
/// EM-SIZECLASS-NONFULL-LISTED, no earlier deferred frees) and every published extent at
/// `off` = slot `(s, i)` (rm_case): remove_extent_by_offset queues the free
/// (region.rs:143-148); a checkpoint with no call interleaving (lib.rs:275-321: snapshot,
/// copy + superblock write, then `dirty = false; flush_pending_frees()`) that succeeds has
/// persisted the removal (the image's slot holds FREE_KEY) and leaves the slot reusable.
// LEVEL-3 D-B1: WIDENED — any number of regions (the removal is in region k), any earlier
// deferred frees in that region (their slots hold FREE_KEY: the pv(pending_frees) part of the
// PROVED region invariant lg), and the real ck_finish over ALL regions (model/ckpt.rs).
#[requires(regions_ok(em.regions@) && k@ < em.regions@.len())]
#[requires(nonfull_listed(em.regions@[k@]) && pv(em.regions@[k@], em.regions@[k@].pending_frees@))]
#[requires(rm_case(em.regions@[k@], off, s, i))]
#[ensures(result.0 == Ok(()))]
#[ensures(result.1 == Ok(()) ==> slot_reusable((^em).regions@[k@], s, i))]
#[ensures(result.1 == Ok(()) ==> result.2.len() == em.regions@.len() && result.2[k@].slabs@.contains(s@)
    && result.2[k@].slabs@.lookup(s@).keys@[i@] == FREE_KEY)]
pub fn verify_em_remove_post_reusable_after_checkpoint(
    em: &mut CkEm, k: usize, off: u64, s: u64, i: usize, ok_data: bool, ok_sb: bool,
) -> (Result<(), EmError>, Result<(), EmError>, Snapshot<Seq<RegionState>>) {
    let pre = snapshot! { em.regions@[k@] };
    let pre_all = snapshot! { em.regions@ };
    let r = em.regions[k].remove_extent_by_offset(off);
    proof_assert! { rm_post(*pre, em.regions@[k@], s, i) };
    proof_assert! { forall<j: Int> 0 <= j && j < em.regions@.len() && j != k@ ==> em.regions@[j] == pre_all[j] };
    proof_assert! { forall<p: Int> 0 <= p && p < pre.pending_frees@.len() ==> pre.pending_frees@[p] != (s, i) };
    proof_assert! { em.regions@[k@].pending_frees@[em.regions@[k@].pending_frees@.len() - 1] == (s, i) };
    proof_assert! { forall<t: Int> em.regions@[k@].slabs@.contains(t) == pre.slabs@.contains(t) };
    proof_assert! { forall<t: Int> pre.slabs@.contains(t) ==> em.regions@[k@].slabs@.lookup(t).bitmap == pre.slabs@.lookup(t).bitmap };
    proof_assert! { forall<t: u64, j: usize> free_ok(em.regions@[k@], t, j) == free_ok(*pre, t, j) };
    proof_assert! { forall<p: Int> 0 <= p && p < pre.pending_frees@.len() ==>
        em.regions@[k@].pending_frees@[p] == pre.pending_frees@[p] };
    proof_assert! { free_ok(em.regions@[k@], s, i) && em.regions@[k@].slabs@.contains(s@) };
    proof_assert! { em.regions@[k@].pending_frees@.len() == pre.pending_frees@.len() + 1 };
    proof_assert! { forall<p: Int> 0 <= p && p < em.regions@[k@].pending_frees@.len() ==>
        em.regions@[k@].slabs@.contains(em.regions@[k@].pending_frees@[p].0@)
        && free_ok(em.regions@[k@], em.regions@[k@].pending_frees@[p].0, em.regions@[k@].pending_frees@[p].1) };
    proof_assert! { forall<p: Int, q: Int> 0 <= p && p < q && q < pre.pending_frees@.len() ==>
        em.regions@[k@].pending_frees@[p] != em.regions@[k@].pending_frees@[q] };
    proof_assert! { forall<p: Int> 0 <= p && p < pre.pending_frees@.len() ==>
        em.regions@[k@].pending_frees@[p] != em.regions@[k@].pending_frees@[pre.pending_frees@.len()] };
    proof_assert! { pending_ok(em.regions@[k@]) };
    proof_assert! { regions_ok(em.regions@) };
    let rk = snapshot! { em.regions@[k@] };
    proof_assert! { nonfull_listed(*rk) };
    let img = match em.ck_begin() {
        Some(img) => img,
        None => return (r, Err(EmError::IoError), snapshot! { Seq::empty() }),
    };
    let c = em.ck_write(img, ok_data, ok_sb);
    if c.is_err() {
        return (r, c, img);
    }
    proof_assert! { em.regions@[k@] == *rk };
    em.ck_finish();
    let fin = snapshot! { em.regions@[k@] };
    proof_assert! { fin_more(*rk, *fin) };
    proof_assert! { slot_gone(*fin, s, i) };
    proof_assert! { nonfull_listed(*fin) && rg_inv(*fin) };
    proof_assert! { fin.slabs@.contains(s@) ==> {
        slab_inv(fin.slabs@.lookup(s@)) && i@ < fin.slabs@.lookup(s@).bitmap.num_slots@ } };
    proof_assert! { fin.slabs@.contains(s@) ==> {
        lemma_cnt_lt(fin.slabs@.lookup(s@).bitmap, i@, fin.slabs@.lookup(s@).bitmap.num_slots@);
        fin.slabs@.lookup(s@).bitmap.allocated_count@ < fin.slabs@.lookup(s@).bitmap.num_slots@ } };
    proof_assert! { slot_reusable(*fin, s, i) };
    (r, c, img)
}

/// Mutant: claims the slot is still allocated after the checkpoint.
#[requires(regions_ok(em.regions@) && em.regions@.len() == 1)]
#[requires(em.regions@[0].pending_frees@.len() == 0 && nonfull_listed(em.regions@[0]))]
#[requires(rm_case(em.regions@[0], off, s, i))]
#[ensures(result.1 == Ok(()) ==> match (^em).regions@[0].slabs@.get(s@) { Some(sl) => slot_bit(sl.bitmap, i@), None => false })]
pub fn verify_em_remove_post_reusable_after_checkpoint__mutant(
    em: &mut CkEm, off: u64, s: u64, i: usize, ok_data: bool, ok_sb: bool,
) -> (Result<(), EmError>, Result<(), EmError>, Snapshot<Seq<RegionState>>) {
    let pre = snapshot! { em.regions@[0] };
    let r = em.regions[0].remove_extent_by_offset(off);
    proof_assert! { rm_post(*pre, em.regions@[0], s, i) };
    proof_assert! { em.regions@[0].pending_frees@ == Seq::singleton((s, i)) };
    proof_assert! { pending_ok(em.regions@[0]) && regions_ok(em.regions@) };
    let img = match em.ck_begin() {
        Some(img) => img,
        None => return (r, Err(EmError::IoError), snapshot! { Seq::empty() }),
    };
    let c = em.ck_write(img, ok_data, ok_sb);
    if c.is_err() {
        return (r, c, img);
    }
    let mid = snapshot! { em.regions@[0] };
    em.regions[0].dirty = false;
    proof_assert! { pending_ok(em.regions@[0]) && nonfull_listed(em.regions@[0]) };
    let mid2 = snapshot! { em.regions@[0] };
    em.regions[0].flush_pending_frees();
    proof_assert! { lemma_free_case_pending(*mid2, *mid, em.regions@[0], s, i, mid2.slabs@.lookup(s@));
        free_case(*mid, em.regions@[0], s, i, mid.slabs@.lookup(s@)) };
    (r, c, img)
}

// =============================================================================
// abort + used_bytes (new root cause RECOVER-EMPTY-SLAB)
// =============================================================================

pub const WA_K1: u64 = 21;
pub const WA_K2: u64 = 22;

/// No bit set below `n` gives a zero count.
#[logic]
#[variant(n)]
#[requires(0 <= n)]
#[requires(forall<j: Int> 0 <= j && j < n ==> !slot_bit(a, j))]
#[ensures(cnt(a, n) == 0)]
pub fn lemma_cnt_zero(a: AllocationBitmap, n: Int) {
    if n > 0 {
        lemma_cnt_zero(a, n - 1)
    }
}

/// `0 & !(16 - 1) == 0` (the mask buddy.rs:136 computes for offset 0 at order 2, sector 4).
#[bitwise_proof]
#[ensures((0u64 & !(16u64 - 1u64)) == 0u64)]
pub fn fact_mask_0_16() {}

/// RECOVER-EMPTY-SLAB pre-crash history (sector 4, slab 8, one region [0,16), FR-002 valid):
/// format; reserve_extent(K1, 4) carves slab A = [0, 8) (2 slots); publish K1;
/// remove_extent(0) (key -> FREE_KEY, slot queued as a deferred free, region.rs:143-148);
/// checkpoint() — no call interleaving — succeeds and persists A while its only slot is
/// still pending-free, i.e. with keys [FREE, FREE] (serialize_region writes EVERY slab of
/// the map, checkpoint.rs:18-30). Returns what a fresh component's recovery reads back.
#[ensures(match result {
    Ok(img) => img.len() == 1 && img[0].slabs@.contains(0)
        && img[0].slabs@.lookup(0).start_offset@ == 0 && img[0].slabs@.lookup(0).slab_size@ == 8
        && img[0].slabs@.lookup(0).element_size@ == 4 && img[0].slabs@.lookup(0).keys@.len() == 2
        && img[0].slabs@.lookup(0).keys@[0] == FREE_KEY && img[0].slabs@.lookup(0).keys@[1] == FREE_KEY,
    Err(_) => false,
})]
pub fn wa_precrash() -> Result<Snapshot<Seq<RegionState>>, EmError> {
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
    let old = snapshot! { r };
    let res = r.alloc_extent(4);
    proof_assert! { fresh_case(*old, r, 4, res) };
    proof_assert! { match res { Ok((d, _, _)) => d@ == 0, Err(_) => false } };
    proof_assert! { r.slabs@.contains(0) && forall<k: Int> r.slabs@.contains(k) ==> k == 0 };
    let a0 = snapshot! { r.slabs@.lookup(0) };
    proof_assert! { a0.keys@.len() == 2 && a0.keys@[0] == FREE_KEY && a0.keys@[1] == FREE_KEY };
    proof_assert! { slot_bit(a0.bitmap, 0) && a0.start_offset@ == 0 && a0.element_size@ == 4 && a0.slab_size@ == 8 };
    // LEVEL-3 reachable start (after format + reserve K1): format-start ranges of
    // wc_fp(8, 16) (3 x 4096-byte metadata device), reserve ranges, proved region invariants
    // with the live handle K1.
    let sb0 = l3_sb_c8_16();
    proof_assert! { l3_fmt_ranges(wc_fp_l(8u64, 16u64), 3, 4096, sb0) && l3_rsv_ranges(4, wc_fp_l(8u64, 16u64)) };
    proof_assert! { r.buddy.free_lists@.len() == 3 && r.buddy.free_lists@[2]@.len() == 0 && r.buddy.free_lists@[0]@.len() == 0
        && r.buddy.free_lists@[1]@ == Seq::singleton(8u64) };
    proof_assert! { forall<o: Int, k: Int> fv(r.buddy.free_lists@, o, k) ==> o == 1 && k == 0 };
    proof_assert! { lemma_tf3(r.buddy.free_lists@, 4); rg_tf(r) == 8 && r.slabs@.len() == 1 && sk(r) == 8 };
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
    // publish(K1) (lib.rs:610)
    r.publish_slot(0, 0, WA_K1);
    let pre_rm = snapshot! { r };
    proof_assert! { slot_off(pre_rm.slabs@.lookup(0), 0) == 0 && pre_rm.slabs@.lookup(0).keys@[0] == WA_K1 };
    proof_assert! { rm_case(*pre_rm, 0u64, 0u64, 0usize) };
    // remove_extent(0) (lib.rs:680-684; one region)
    let rm = r.remove_extent_by_offset(0);
    proof_assert! { rm == Ok(()) && rm_post(*pre_rm, r, 0u64, 0usize) };
    proof_assert! { r.slabs@.lookup(0).keys@ == a0.keys@.set(0, WA_K1).set(0, FREE_KEY) };
    proof_assert! { r.slabs@.lookup(0).keys@[0] == FREE_KEY && r.slabs@.lookup(0).keys@[1] == FREE_KEY };
    proof_assert! { pending_ok(r) };
    let snap = snapshot! { r };
    let mut rs: Vec<RegionState> = Vec::new();
    rs.push(r);
    let mut em = CkEm { regions: rs, active: 0, seq: 0, dev: wc_fresh_dev() };
    proof_assert! { em.regions@.len() == 1 && em.regions@[0] == *snap && em.regions@[0].dirty };
    proof_assert! { regions_ok(em.regions@) && !all_clean(em.regions@) };
    // checkpoint() (lib.rs:275-370), devices succeed
    let c = em.run_checkpoint(true, true);
    proof_assert! { c == Ok(()) };
    // crash; a fresh component's recovery reads the device
    let rec = recover(&em.dev);
    proof_assert! { match rec { Ok(img) => *img == Seq::singleton(*snap) || img.len() == 1 && img[0] == *snap, Err(_) => false } };
    rec
}

/// The checkpointed descriptor of slab A as deserialize_slabs returns it (checkpoint.rs:183-241):
/// (start 0, slab 8, element 4, keys [FREE, FREE]).
#[ensures(result@.len() == 1)]
#[ensures(result@[0].start_offset@ == 0 && result@[0].slab_size@ == 8 && result@[0].element_size@ == 4)]
#[ensures(result@[0].keys@.len() == 2 && result@[0].keys@[0] == FREE_KEY && result@[0].keys@[1] == FREE_KEY)]
pub fn wa_descs() -> Vec<SlabDescriptor> {
    let mut keys: Vec<u64> = Vec::new();
    keys.push(FREE_KEY);
    keys.push(FREE_KEY);
    let mut v: Vec<SlabDescriptor> = Vec::new();
    v.push(SlabDescriptor { start_offset: 0, slab_size: 8, element_size: 4, keys });
    v
}

/// After the crash: initialize() rebuilds region [0,16) from A's descriptor (lib.rs:552-565):
/// A is marked allocated in the buddy allocator (one order-1 block [8,16) left free) and
/// re-created EMPTY, listed under size class 4. Then reserve_extent(K2, 4) takes slot 0 of A
/// (region.rs:47-60, buddy untouched) and its abort (lib.rs:618-621 → region.rs:94-107)
/// empties A, which free_slot returns to the buddy allocator. Returns the region's
/// used_bytes() (lib.rs:704) before the reservation, after it, and after the abort.
#[ensures(result.0@ == 8 && result.1@ == 8 && result.2@ == 0)]
pub fn wa_recovered_reserve_abort() -> (u64, u64, u64) {
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
    proof_assert! { hit_lists(r.buddy, descs@[0], 0, fp, 2) };
    proof_assert! { r.buddy.free_lists@.len() == 3 && r.buddy.free_lists@[1]@.len() == 1 && r.buddy.free_lists@[0]@.len() == 0 && r.buddy.free_lists@[2]@.len() == 0 };
    proof_assert! { lemma_tf3(r.buddy.free_lists@, 4); tf(r.buddy.free_lists@, 4, 3) == 8 };
    proof_assert! { r.buddy.total_usable_size@ == 16 && r.buddy.sector_size@ == 4 };
    proof_assert! { rg_inv(r) && bd_shape(r.buddy) };
    // LEVEL-3 reachable start (initialize of the wa_precrash device): recovered-payload
    // ranges 980f61 / 3b612e on the descriptor, the format-start ranges of the superblock it
    // was written under, and the proved region invariants of the rebuilt region (no handles).
    let sb0 = l3_sb_c8_16();
    proof_assert! { l3_fmt_ranges(wc_fp_l(8u64, 16u64), 3, 4096, sb0) && l3_rsv_ranges(4, wc_fp_l(8u64, 16u64)) };
    proof_assert! { 8 / 4 == 2 && 8 % 8 == 0 && l3_rec_ranges(descs@, 0, 16, 8) };
    proof_assert! { starts_distinct(descs@) && p2(4) };
    proof_assert! { lemma_rg_good_o(r); rg_good(r) };
    proof_assert! { pv(r, Seq::empty()) && lg(r, Seq::empty()) };
    proof_assert! { forall<o: Int, k: Int> fv(r.buddy.free_lists@, o, k) ==> o == 1 && k == 0 };
    proof_assert! { lemma_coal_single(r.buddy.free_lists@, r.buddy.sector_size@); coal(r.buddy.free_lists@, r.buddy.sector_size@) };
    proof_assert! { nonfull_listed(r) && coal(r.buddy.free_lists@, r.buddy.sector_size@) && rg_l3(r, Seq::empty()) };
    let used0 = region_used_release(&r.buddy);
    // reserve_extent(K2, 4): size class 4 lists the recovered, EMPTY slab A
    proof_assert! { sc_get(r.size_classes, align_l(4, 4)) == Seq::singleton(0u64) && align_l(4, 4) == 4 };
    proof_assert! { r.slabs@.contains(0) && r.slabs@.lookup(0).bitmap.num_slots@ == 2 };
    proof_assert! { forall<j: Int> 0 <= j && j < 2 ==> !slot_bit(r.slabs@.lookup(0).bitmap, j) };
    proof_assert! { slab_inv(r.slabs@.lookup(0)) };
    proof_assert! { lemma_cnt_zero(r.slabs@.lookup(0).bitmap, 2); r.slabs@.lookup(0).bitmap.allocated_count@ == 0 };
    proof_assert! { exist_cond(r, 4, 0u64) };
    let old = snapshot! { r };
    let res = r.alloc_extent(4);
    proof_assert! { exist_case(*old, r, 4, 0u64, res) };
    let (s, i) = match res {
        Ok((s, i, _)) => (s, i),
        Err(_) => return (used0, 0, 0),
    };
    proof_assert! { s@ == 0 && r.buddy == old.buddy };
    proof_assert! { r.slabs@.lookup(0).bitmap.allocated_count@ == 1 && slot_bit(r.slabs@.lookup(0).bitmap, i@) };
    proof_assert! { rg_inv(r) && bd_shape(r.buddy) };
    let used1 = region_used_release(&r.buddy);
    // WriteHandle::abort -> free_slot(0, i)
    proof_assert! { free_ok(r, s, i) };
    let pre = snapshot! { r };
    proof_assert! { slab_k(*pre) == 1 && pre.buddy.sector_size@ == 4 };
    r.free_slot(s, i);
    proof_assert! { tf(r.buddy.free_lists@, 4, r.buddy.free_lists@.len()) == 16 };
    proof_assert! { rg_inv(r) && bd_shape(r.buddy) && r.buddy.total_usable_size@ == 16 };
    let used2 = region_used_release(&r.buddy);
    (used0, used1, used2)
}

/// **EM-ABORT-POST-USED-RESTORED** — REFUTED (INDEPENDENT; new root cause RECOVER-EMPTY-SLAB:
/// serialize_region (checkpoint.rs:18-30) persists every slab of the map, including one whose
/// only occupied slots are pending-free or reserved-but-unpublished, and initialize
/// (lib.rs:552-565) re-creates it as an allocated slab with NO allocated slot, while
/// free_slot (region.rs:104-107) returns a slab to the buddy allocator only when a free
/// empties it — so the first reservation's abort on such a slab releases the whole slab).
/// "If nothing else happens in between, used_bytes() after aborting a reservation equals
/// used_bytes() before that reservation was made." Witness (sector 4 — a power of two, NOT
/// the buddy mask): pre-crash history `wa_precrash` persists slab A = [0,8) with keys
/// [FREE, FREE]; after initialize used_bytes() = 8 (A allocated, no extent in it);
/// reserve_extent(K2, 4) takes A's slot 0 (used stays 8); abort() → used_bytes() = 0 != 8.
#[ensures(result.0@ == 8 && result.2@ == 0 && result.0 != result.2)]
#[ensures(match result.3 { Ok(img) => img.len() == 1 && img[0].slabs@.contains(0)
    && img[0].slabs@.lookup(0).keys@ == result.4@[0].keys@ && img[0].slabs@.lookup(0).start_offset == result.4@[0].start_offset
    && img[0].slabs@.lookup(0).element_size == result.4@[0].element_size && img[0].slabs@.lookup(0).slab_size == result.4@[0].slab_size,
    Err(_) => false })]
pub fn refute_em_abort_post_used_restored() -> (u64, u64, u64, Result<Snapshot<Seq<RegionState>>, EmError>, Vec<SlabDescriptor>) {
    let rec = wa_precrash();
    let d = wa_descs();
    proof_assert! { match rec { Ok(img) => img[0].slabs@.lookup(0).keys@.ext_eq(d@[0].keys@), Err(_) => true } };
    let (u0, u1, u2) = wa_recovered_reserve_abort();
    (u0, u1, u2, rec, d)
}

/// Mutant: claims the abort restored used_bytes().
#[ensures(result.2 == result.0)]
pub fn refute_em_abort_post_used_restored__mutant() -> (u64, u64, u64) {
    wa_recovered_reserve_abort()
}

// =============================================================================
// remove_extent of an extent in the last region's tail (new root cause REGION-TAIL-LOOKUP)
// =============================================================================

pub const WT_K1: u64 = 7;
pub const WT_K2: u64 = 15;

/// `15 & 7 == 7` and `7 & 7 == 7`: region_for_key (lib.rs:223) sends keys 7 and 15 to region 7 of 8.
#[bitwise_proof]
#[ensures((15usize & 7usize) == 7usize && (7usize & 7usize) == 7usize)]
pub fn fact_and_7() {}

/// The REGION-TAIL geometry: metadata device 4 x 4096 bytes; FR-002-valid parameters with
/// sector 4, slab 4, max extent 4, EIGHT regions over a 36-byte data device (separate-device
/// mode): region_bytes = 36 / 8 = 4, regions 0..6 are [4i, 4i+4), region 7 is [28, 36).
#[ensures(result.0.connected && result.0.sector_size@ == 4096 && result.0.num_sectors@ == 4)]
#[ensures(result.1.data_disk_size@ == 36 && result.1.slab_size@ == 4 && result.1.max_extent_size@ == 4)]
#[ensures(result.1.sector_size@ == 4 && result.1.region_count@ == 8 && result.1.metadata_alignment@ == 0)]
#[ensures(result.1.metadata_region_size@ == 0 && result.1.instance_id == Some(1u64))]
pub fn wt_geometry() -> (MetaDevice, FormatParams) {
    let dev = MetaDevice { connected: true, sector_size: 4096, num_sectors: 4, sb: None, ckpt0: None, ckpt1: None };
    let params = FormatParams {
        data_disk_size: 36,
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

/// Witness: format (8 regions); reserve_extent(7, 4) and reserve_extent(15, 4) both go to
/// region 7 (key & 7, lib.rs:223) whose buddy allocator (two 4-byte sectors from 28) hands
/// out the one-slot slabs [28,32) and [32,36) (region.rs:66-91); publish the second handle
/// (K2 at offset 32); remove_extent(32): region_for_offset computes (32 - 0) / 4 = 8 >= 8
/// regions (lib.rs:247-256) and returns OffsetNotFound(32) — the extent stays.
/// Returns (format result, the reservation's offset, remove result, the component).
#[ensures(match result.0 {
    Ok(()) => result.1@ == 32 && result.2 == Err(EmError::OffsetNotFound(32u64))
        && match result.3.regions {
            Some(rv) => rv@.len() == 8 && result.3.arena@[rv@[7]@].slabs@.contains(32)
                && from_slot(result.3.arena@[rv@[7]@], 32, 0, Extent { key: WT_K2, size: 4u32, offset: 32u64 }),
            None => false,
        },
    Err(e) => e == EmError::IoError,
})]
pub fn wt_tail_remove() -> (Result<(), EmError>, u64, Result<(), EmError>, ExtentManager) {
    let (dev, params) = wt_geometry();
    let mut em = new_inner(dev);
    proof_assert! { em_regions_wf(em) };
    proof_assert! { fmt_ok8(em, params) };
    let r = em.format(params);
    if r.is_err() {
        return (r, 0, Ok(()), em);
    }
    proof_assert! { params.slab_size@ > 0 && em_regions_ok(em) };  // LEVEL-3 D-B2: format ensures em_regions_ok for slab_size > 0
    proof_assert! { ds_l(16384, 0, 0, 4) == 0 };
    proof_assert! { rbase(0, 36, 8, 7) == 28 && rsize(36, 8, 7) == 8 };
    proof_assert! { 1.pow2() == 2 && 8 / 4 == 2 };
    let rv0 = match &em.regions {
        Some(rv) => rv.len(),
        None => return (r, 0, Ok(()), em),
    };
    fact_and_7();
    let idx = and_mask(WT_K2 as usize, rv0 - 1);
    proof_assert! { idx@ == 7 };
    let r7 = match &em.regions {
        Some(rv) => rv[idx],
        None => return (r, 0, Ok(()), em),
    };
    proof_assert! { match em.regions { Some(rv) => rv@[7] == r7 && r7@ < em.arena@.len(), None => false } };
    let fr = snapshot! { em.arena@[r7@] };
    proof_assert! { fresh_rg(*fr, 28, 8, params) && rg_inv(*fr) };
    proof_assert! { fr.buddy.max_order@ == 1 && fr.buddy.free_lists@[1]@ == Seq::singleton(0u64) && fr.buddy.free_lists@[0]@.len() == 0 };
    // LEVEL-3 reachable start (format of wt_geometry, before the two reserves): the proved
    // component invariants, the declared ranges of remove_extent / get_extents /
    // for_each_extent (217ace, 84618e, 91a1f6 and 3b58ea; 980f61 is vacuous: nothing was
    // recovered) and of the reserves (231ab0, 3783f8, a27ede), and the proved region invariants
    // of region 7 (the one the witness uses).
    proof_assert! { 2.pow2() == 4 && 3.pow2() == 8 && 0.pow2() == 1 && p2(4) && p2(8) && p2(1) };
    proof_assert! { em_regions_wf(em) && em_regions_ok(em) && em_layout(em) && seq_agree(em) && rm_safe8(em) };
    proof_assert! { a_217ace(params.sector_size@) && a_3b58ea(params.data_disk_size@, params.sector_size@) && l3_rsv_ranges(4, params) };
    proof_assert! { match em.shared { Some(sh) => sh.superblock.data_start_offset@ == 0 && sh.superblock.data_disk_size@ == 36
        && a_91a1f6(sh.superblock) && a_84618e(params.data_disk_size@, sh.superblock.data_start_offset@, params.region_count@), None => false } };
    proof_assert! { forall<o: Int, k: Int> fv(fr.buddy.free_lists@, o, k) ==> o == 1 && k == 0 };
    proof_assert! { lemma_tf2(fr.buddy.free_lists@, 4); rg_tf(*fr) == 8 && fr.slabs@.len() == 0 };
    proof_assert! { fdisj(fr.buddy.free_lists@, fr.buddy.sector_size@) };
    proof_assert! { sl_away(*fr) };
    proof_assert! { rg_acct(*fr) };
    proof_assert! { sc_ok(*fr) };
    proof_assert! { rg_ka(*fr) };
    proof_assert! { rg_core(*fr) && rg_good(*fr) && pv(*fr, Seq::empty()) && lg(*fr, Seq::empty()) };
    proof_assert! { lemma_coal_single(fr.buddy.free_lists@, fr.buddy.sector_size@); coal(fr.buddy.free_lists@, fr.buddy.sector_size@) };
    proof_assert! { nonfull_listed(*fr) && coal(fr.buddy.free_lists@, fr.buddy.sector_size@) && rg_l3(*fr, Seq::empty()) };
    proof_assert! { lemma_ord_eq(1, 0, 0); slab_k(*fr) == 0 && span(4, 0) == 4 && span(4, 1) == 8 };
    proof_assert! { align_l(4, 4) == 4 && slots_of(4, 4) == 1 };
    proof_assert! { first_ne(fr.buddy.free_lists@, 0, 1) == 1 };
    proof_assert! { fr.buddy.base_offset@ == 28 };
    let arena0 = snapshot! { em.arena };
    // reserve_extent(7, 4): fresh slab [28, 32)
    let a1 = em.arena[r7].alloc_extent(4);
    proof_assert! { fresh_case(*fr, em.arena@[r7@], 4, a1) };
    proof_assert! { match a1 { Ok((d, _, _)) => d@ == 28, Err(_) => false } };
    let s1 = snapshot! { em.arena@[r7@] };
    proof_assert! { alloc_post(fr.buddy, s1.buddy, 0, 1, 28) };
    proof_assert! { pushed(s1.buddy.free_lists@[0]@, fr.buddy.free_lists@[0]@, 0 + span(4, 0)) };
    proof_assert! { s1.buddy.free_lists@[1]@.len() == 0 && s1.buddy.free_lists@[0]@ == Seq::singleton(4u64) };
    proof_assert! { forall<e: Int> sc_get(s1.size_classes, e) == Seq::empty() };
    proof_assert! { first_ne(s1.buddy.free_lists@, 0, 1) == 0 && slab_k(*s1) == 0 };
    // reserve_extent(15, 4): size class 4 is empty (the one-slot slab is full) -> fresh slab [32, 36)
    let a2 = em.arena[r7].alloc_extent(4);
    proof_assert! { fresh_case(*s1, em.arena@[r7@], 4, a2) };
    let (st, off) = match a2 {
        Ok((d, _, o)) => (d, o),
        Err(_) => return (r, 0, Ok(()), em),
    };
    proof_assert! { st@ == 32 && off@ == 32 };
    proof_assert! { forall<j: Int> 0 <= j && j < em.arena@.len() && j != r7@ ==> em.arena@[j] == arena0[j] };
    proof_assert! { em_regions_wf(em) && em_regions_ok(em) };
    let h = Handle { region: r7, key: WT_K2, offset: off, size: 4, slab_start: st, slot_idx: 0 };
    proof_assert! { em.arena@[r7@].slabs@.contains(32) && em.arena@[r7@].slabs@.lookup(32).keys@.len() == 1 };
    let s2 = snapshot! { em.arena@[r7@].slabs@.lookup(32) };
    proof_assert! { s2.keys@[0] == FREE_KEY && s2.start_offset@ == 32 && s2.element_size@ == 4 && s2.bitmap.num_slots@ == 1 };
    let pre_pub = snapshot! { em };
    proof_assert! { pre_pub.arena@[r7@].slabs@.get(32) == Some(*s2) };
    let _p = em.publish(h);
    proof_assert! { pub_post(*pre_pub, em, h) };
    proof_assert! { em.arena@[r7@].slabs@.contains(32) && em.arena@[r7@].slabs@.lookup(32).keys@ == s2.keys@.set(0, WT_K2) };
    proof_assert! { em.arena@[r7@].slabs@.lookup(32).keys@[0] == WT_K2 };
    proof_assert! { em.arena@[r7@].slabs@.lookup(32).bitmap == s2.bitmap && em.arena@[r7@].slabs@.lookup(32).element_size == s2.element_size
        && em.arena@[r7@].slabs@.lookup(32).start_offset == s2.start_offset };
    proof_assert! { em_regions_wf(em) && em_regions_ok(em) };
    proof_assert! { 36 / 8 == 4 && 32 / 4 == 8 && 36 - 0 == 36 };
    proof_assert! { match (em.regions, em.shared) {
        (Some(rv), Some(sh)) => rv@.len() == 8 && sh.format_params.data_disk_size@ == 36 && sh.superblock.data_start_offset@ == 0,
        _ => false } };
    proof_assert! { match (em.regions, em.shared) {
        (Some(rv), Some(sh)) => sh.format_params.data_disk_size@ - sh.superblock.data_start_offset@ == 36
            && (if 32 >= sh.superblock.data_start_offset@ { 32 - sh.superblock.data_start_offset@ } else { 0 }) == 32,
        _ => false } };
    proof_assert! { match (em.regions, em.shared) {
        (Some(rv), Some(sh)) => (sh.format_params.data_disk_size@ - sh.superblock.data_start_offset@) / rv@.len() == 4,
        _ => false } };
    proof_assert! { match (em.regions, em.shared) {
        (Some(rv), Some(sh)) => rv@.len() == 8 && sh.format_params.data_disk_size@ == 36 && sh.superblock.data_start_offset@ == 0
            && (sh.format_params.data_disk_size@ - sh.superblock.data_start_offset@) / rv@.len() == 4
            && (if 32 >= sh.superblock.data_start_offset@ { 32 - sh.superblock.data_start_offset@ } else { 0 })
                / ((sh.format_params.data_disk_size@ - sh.superblock.data_start_offset@) / rv@.len()) == 8,
        _ => false } };
    let before_rm = snapshot! { em };
    let rm = em.remove_extent(32);
    proof_assert! { rm == Err(EmError::OffsetNotFound(32u64)) && em == *before_rm };
    proof_assert! { em.arena@[r7@].slabs@.lookup(32).keys@[0] == WT_K2 };
    proof_assert! { slot_off(em.arena@[r7@].slabs@.lookup(32), 0) == 32 && em.arena@[r7@].slabs@.lookup(32).element_size@ == 4 };
    (r, off, rm, em)
}

/// **EM-REMOVE-POST-HIDDEN** — REFUTED (INDEPENDENT; new root cause REGION-TAIL-LOOKUP:
/// region_for_offset (lib.rs:247-256) maps an offset to region `(offset - ds) / (usable /
/// region_count)` and rejects index >= region_count, but format/initialize give the LAST region
/// the remainder `usable - (n-1)*(usable/n)` (lib.rs:460-464, 541-545), so every offset in
/// its last `usable mod n` bytes maps to index n and is reported OffsetNotFound). "When
/// remove_extent(offset) is called with the offset of a published extent, it succeeds and that
/// extent immediately stops appearing." Witness `wt_tail_remove` (sector 4 — a power of two,
/// NOT the buddy mask; 8 regions over 36 bytes): K2 is published at offset 32 of region 7
/// = [28,36); remove_extent(32) returns OffsetNotFound(32) and the slot the listing loops visit
/// still holds K2 at offset 32. Realistic instance: 512-byte sectors, 1024 regions, any data
/// size whose usable bytes mod 1024 are >= 512 (e.g. 1024*2^20 + 1023 bytes): the last
/// region's final sector is never removable.
#[ensures(match result.0 {
    Ok(()) => result.1@ == 32 && result.2 == Err(EmError::OffsetNotFound(32u64)),
    Err(e) => e == EmError::IoError,
})]
pub fn refute_em_remove_post_hidden() -> (Result<(), EmError>, u64, Result<(), EmError>) {
    let (r, off, rm, _em) = wt_tail_remove();
    (r, off, rm)
}

/// Mutant: claims the removal succeeded.
#[ensures(match result.0 { Ok(()) => result.2 == Ok(()), Err(_) => true })]
pub fn refute_em_remove_post_hidden__mutant() -> (Result<(), EmError>, u64, Result<(), EmError>) {
    let (r, off, rm, _em) = wt_tail_remove();
    (r, off, rm)
}

// =============================================================================
// Visibility: reserve / publish / drop
// =============================================================================

/// A slot holding no new key also yields no new listed extent.
#[logic]
#[requires(no_new_keys(a, b) && from_slot(b, k, j, e))]
#[ensures(from_slot(a, k, j, e))]
pub fn lemma_nnk_from_slot(a: RegionState, b: RegionState, k: Int, j: Int, e: Extent) {}

/// **EM-RESERVE-POST-NOT-VISIBLE** — "A successful reserve_extent() does not make the new
/// extent visible; until publish() is called on its handle, the extent appears neither in
/// get_extents() nor in for_each_extent()." reserve_extent (lib.rs:584-630) only allocates
/// a slot (region.rs:41-92: alloc_slot and Slab::new leave every key as it was / FREE_KEY;
/// the key is written only by the publish closure, lib.rs:610). Proved for every component
/// state, key and size: every extent get_extents() lists — and every extent for_each_extent()
/// passes its callback — after the reservation was already listed by get_extents() before it,
/// so the reservation makes nothing visible; in particular nothing is listed from the
/// reserved slot unless that exact extent was already listed (it holds no new key).
#[requires(em_ok(*em))]
#[requires(a_231ab0(size@))]
// LEVEL-3 D-B1: 3783f8 only for the CURRENT regions (the one reserve_extent picks is one of them), not stale arena entries.
#[requires(rsv_cur_ok(*em, size@))]
#[ensures(forall<p: Int> 0 <= p && p < result.2@.len() ==> in_from(result.0@, 0, result.2@[p]))]
#[ensures(forall<p: Int> cb@.len() <= p && p < (^cb)@.len() ==> in_from(result.0@, 0, (^cb)@[p]))]
pub fn verify_em_reserve_post_not_visible(
    em: &mut ExtentManager, key: u64, size: u32, cb: &mut Vec<Extent>,
) -> (Vec<Extent>, Result<Handle, EmError>, Vec<Extent>) {
    let before = em.get_extents();
    let old = snapshot! { *em };
    let h = em.reserve_extent(key, size);
    let after = em.get_extents();
    em.for_each_extent(cb);
    proof_assert! { em.regions == old.regions };
    proof_assert! { match em.regions { Some(rv) => forall<i: Int, k: Int, j: Int, e: Extent> 0 <= i && i < rv@.len()
        && from_slot(em.arena@[rv@[i]@], k, j, e) ==> {
            lemma_nnk_from_slot(old.arena@[rv@[i]@], em.arena@[rv@[i]@], k, j, e);
            from_slot(old.arena@[rv@[i]@], k, j, e) && in_from(before@, 0, e) }, None => true } };
    proof_assert! { match em.regions { Some(rv) => forall<e: Extent> listed_one(*em, rv@, e) ==> in_from(before@, 0, e), None => true } };
    (before, h, after)
}

/// Mutant: claims the reservation's slot is listed afterwards (nothing makes it so).
#[requires(em_ok(*em))]
#[requires(size@ > 0)]
#[requires(forall<i: Int> 0 <= i && i < em.arena@.len() ==> a_3783f8(size@, em.arena@[i].format_params.sector_size@))]
#[ensures(match result.1 { Ok(h) => in_from(result.2@, 0, Extent { key: h.key, size: h.size, offset: h.offset }), Err(_) => true })]
pub fn verify_em_reserve_post_not_visible__mutant(
    em: &mut ExtentManager, key: u64, size: u32,
) -> (Vec<Extent>, Result<Handle, EmError>, Vec<Extent>) {
    let before = em.get_extents();
    let h = em.reserve_extent(key, size);
    let after = em.get_extents();
    (before, h, after)
}

/// **EM-PUBLISH-FRAME-NOT-UNDONE** — "Once publish() has succeeded, the handle going out of
/// scope does not release the slot or hide the extent; the published extent stays visible
/// until it is removed." WriteHandle::publish (iextent_manager.rs:132-139) takes BOTH
/// closures before running the publish closure, so the Drop at the end of publish
/// (iextent_manager.rs:149-155) finds `abort_fn == None` and does nothing. Proved for every
/// component state and every live reservation handle (its slot exists and is allocated,
/// key not FREE_KEY): the drop leaves the component exactly as the publish closure did; the
/// slot stays allocated; get_extents() and for_each_extent() afterwards list (key, offset,
/// element size) of the handle (listing completeness, model/listing.rs).
#[requires(em_ok(*em))]
#[requires(match em.regions { Some(rv) => exists<i: Int> 0 <= i && i < rv@.len() && rv@[i] == h.region, None => false })]
#[requires(h.region@ < em.arena@.len() && rsv_slot(em.arena@[h.region@], h) && h.key != FREE_KEY)]
#[ensures(^em == *result.1)]
#[ensures((^em).arena@[h.region@].slabs@.contains(h.slab_start@)
    && slot_bit((^em).arena@[h.region@].slabs@.lookup(h.slab_start@).bitmap, h.slot_idx@))]
#[ensures(in_from(result.2@, 0, Extent { key: h.key, size: (^em).arena@[h.region@].slabs@.lookup(h.slab_start@).element_size, offset: h.offset }))]
#[ensures(in_from((^cb)@, cb@.len(), Extent { key: h.key, size: (^em).arena@[h.region@].slabs@.lookup(h.slab_start@).element_size, offset: h.offset }))]
pub fn verify_em_publish_frame_not_undone(
    em: &mut ExtentManager, h: Handle, cb: &mut Vec<Extent>,
) -> (Result<(u64, u64, u32), EmError>, Snapshot<ExtentManager>, Vec<Extent>) {
    let w = WH::new(h);
    let sl = snapshot! { em.arena@[h.region@].slabs@.lookup(h.slab_start@) };
    proof_assert! { em.arena@[h.region@].slabs@.get(h.slab_start@) == Some(*sl) };
    proof_assert! { slab_inv(*sl) && h.slot_idx@ < sl.keys@.len() };
    let (r, mid) = w.publish(em);
    proof_assert! { em.arena@[h.region@].slabs@.lookup(h.slab_start@).keys@ == sl.keys@.set(h.slot_idx@, h.key) };
    proof_assert! { em_regions_wf(*em) && em_regions_ok(*em) };
    proof_assert! { from_slot(em.arena@[h.region@], h.slab_start@, h.slot_idx@,
        Extent { key: h.key, size: em.arena@[h.region@].slabs@.lookup(h.slab_start@).element_size, offset: h.offset }) };
    let ex = em.get_extents();
    em.for_each_extent(cb);
    (r, mid, ex)
}

/// Mutant: claims the drop after publish released the slot.
#[requires(em_ok(*em))]
#[requires(match em.regions { Some(rv) => exists<i: Int> 0 <= i && i < rv@.len() && rv@[i] == h.region, None => false })]
#[requires(h.region@ < em.arena@.len() && rsv_slot(em.arena@[h.region@], h) && h.key != FREE_KEY)]
#[ensures(!(^em).arena@[h.region@].slabs@.contains(h.slab_start@)
    || !slot_bit((^em).arena@[h.region@].slabs@.lookup(h.slab_start@).bitmap, h.slot_idx@))]
pub fn verify_em_publish_frame_not_undone__mutant(em: &mut ExtentManager, h: Handle) -> Result<(u64, u64, u32), EmError> {
    let w = WH::new(h);
    let (r, _mid) = w.publish(em);
    r
}

// =============================================================================
// initialize(): which copy, and exactly which extents
// =============================================================================

/// The checkpoint round trip (serialize_region checkpoint.rs:15-33 + deserialize_slabs
/// checkpoint.rs:183-241, NOT modelled byte-wise — an ASSUMPTION of these proofs): `ds`
/// lists every slab of the persisted region `ir` exactly once, with its start offset, sizes
/// and key vector.
#[logic(open)]
pub fn descs_of(ir: RegionState, ds: Seq<SlabDescriptor>) -> bool {
    pearlite! {
        starts_distinct(ds)
        && (forall<k: Int> ir.slabs@.contains(k) ==> has_desc(ds, ds.len(), k))
        && (forall<p: Int> 0 <= p && p < ds.len() ==> ir.slabs@.contains(ds[p].start_offset@)
            && ir.slabs@.lookup(ds[p].start_offset@).start_offset == ds[p].start_offset
            && ir.slabs@.lookup(ds[p].start_offset@).slab_size == ds[p].slab_size
            && ir.slabs@.lookup(ds[p].start_offset@).element_size == ds[p].element_size
            && ir.slabs@.lookup(ds[p].start_offset@).keys@ == ds[p].keys@)
    }
}

/// A slab of a region satisfying the invariant has as many keys as slots.
#[logic]
#[requires(rg_inv(ir) && ir.slabs@.contains(k))]
#[ensures(slab_inv(ir.slabs@.lookup(k)) && ir.slabs@.lookup(k).keys@.len() == ir.slabs@.lookup(k).bitmap.num_slots@)]
pub fn lemma_slab_unfold(ir: RegionState, k: Int) {}

/// At the start of descriptor `p`, the rebuilt region and the persisted one have the same
/// occupied slots, with the same extents.
#[logic]
#[requires(rebuilt_all(r, ds) && descs_of(ir, ds) && rg_inv(ir) && 0 <= p && p < ds.len())]
#[ensures(from_slot(r, ds[p].start_offset@, j, e) == from_slot(ir, ds[p].start_offset@, j, e))]
pub fn lemma_equiv_at(r: RegionState, ir: RegionState, ds: Seq<SlabDescriptor>, p: Int, j: Int, e: Extent) {
    pearlite! { lemma_slab_unfold(ir, ds[p].start_offset@) }
}

/// `get_extents()` after rebuilding every region `i` of `img` from `pr[i]` lists exactly
/// `img_lists(img, .)`.
#[logic(open)]
pub fn lists_exactly(s: Seq<Extent>, img: Seq<RegionState>) -> bool {
    pearlite! {
        (forall<p: Int> 0 <= p && p < s.len() ==> img_lists(img, s[p]))
        && (forall<e: Extent> img_lists(img, e) ==> in_from(s, 0, e))
    }
}

/// What a fresh component's initialize() + get_extents() produce from superblock `sb` and the
/// descriptors `per_region` decoded from persisted image `img` (shared by the two INIT ids).
#[requires(em_regions_wf(*g) && sb_sane(sb) && recovered_ok(sb, per_region@))]
#[requires(sb.region_count@ == per_region@.len() && img.len() == per_region@.len())]
#[requires(forall<i: Int> 0 <= i && i < per_region@.len() ==> descs_of(img[i], per_region@[i]@) && rg_inv(img[i]))]
#[ensures(lists_exactly(result@, *img))]
pub fn init_and_list(g: &mut ExtentManager, sb: Superblock, per_region: &Vec<Vec<SlabDescriptor>>, img: Snapshot<Seq<RegionState>>) -> Vec<Extent> {
    g.initialize(sb, per_region);
    let ex = g.get_extents();
    proof_assert! { match g.regions { Some(rv) => rv@.len() == per_region@.len()
        && forall<i: Int> 0 <= i && i < rv@.len() ==> dsel(per_region@, i) == per_region@[i]@
            && init_rg_ok(g.arena@[rv@[i]@], per_region@[i]@) && starts_distinct(per_region@[i]@)
            && rebuilt_all(g.arena@[rv@[i]@], per_region@[i]@), None => false } };
    proof_assert! { match g.regions { Some(rv) => forall<i: Int, p: Int, j: Int, e: Extent> 0 <= i && i < rv@.len()
        && 0 <= p && p < per_region@[i]@.len() ==> {
        lemma_equiv_at(g.arena@[rv@[i]@], img[i], per_region@[i]@, p, j, e);
        from_slot(g.arena@[rv@[i]@], per_region@[i]@[p].start_offset@, j, e) == from_slot(img[i], per_region@[i]@[p].start_offset@, j, e) }, None => false } };
    proof_assert! { match g.regions { Some(rv) => forall<i: Int, k: Int> 0 <= i && i < rv@.len()
        && (g.arena@[rv@[i]@].slabs@.contains(k) || img[i].slabs@.contains(k)) ==>
        has_desc(per_region@[i]@, per_region@[i]@.len(), k), None => false } };
    proof_assert! { match g.regions { Some(rv) => forall<i: Int, k: Int, j: Int, e: Extent> 0 <= i && i < rv@.len() ==>
        from_slot(g.arena@[rv@[i]@], k, j, e) == from_slot(img[i], k, j, e), None => false } };
    proof_assert! { match g.regions { Some(rv) => forall<i: Int, e: Extent> 0 <= i && i < rv@.len() ==>
        from_region(g.arena@[rv@[i]@], e) == from_region(img[i], e), None => false } };
    proof_assert! { match g.regions { Some(rv) => forall<e: Extent> listed_one(*g, rv@, e) ==> img_lists(*img, e), None => false } };
    proof_assert! { match g.regions { Some(rv) => forall<p: Int> 0 <= p && p < ex@.len() ==> listed_one(*g, rv@, ex@[p]), None => false } };
    proof_assert! { match g.regions { Some(rv) => forall<i: Int, k: Int, j: Int, e: Extent> 0 <= i && i < rv@.len()
        && from_slot(img[i], k, j, e) ==> from_slot(g.arena@[rv@[i]@], k, j, e) && in_from(ex@, 0, e), None => false } };
    ex
}

/// **EM-INIT-POST-ACTIVE-COPY-USED** — "When the checkpoint copy named active by the
/// superblock is readable and passes its CRC check, initialize() rebuilds state from that
/// active copy, and afterwards the component lists exactly the extents recorded in it, each
/// with its recorded key, offset and size." recover (recovery.rs:29-37, mirror ckpt.rs
/// `recover`) returns the active copy's image when it carries the superblock's sequence
/// number; initialize (lib.rs:546-565: rebuild_region per region from the decoded
/// descriptors) and get_extents (lib.rs:632-656, sound AND complete) then list exactly
/// `img_lists(active image)` — every listed extent is a (key != FREE, slot offset, element
/// size) of a persisted slot and every such slot is listed. For every device content, every
/// superblock and every persisted image whose descriptors round-trip (`descs_of`).
#[requires(d.sb_seq@ > 0)]
#[requires(match copy_at(*d, d.sb_active) {
    Some(c) => c.seq == d.sb_seq && c.img.len() == per_region@.len()
        && (forall<i: Int> 0 <= i && i < per_region@.len() ==> descs_of(c.img[i], per_region@[i]@) && rg_inv(c.img[i])),
    None => false })]
#[requires(em_regions_wf(*g) && sb_sane(sb) && recovered_ok(sb, per_region@) && sb.region_count@ == per_region@.len())]
#[ensures(match copy_at(*d, d.sb_active) {
    Some(c) => result.0 == Ok(c.img) && lists_exactly(result.1@, *c.img),
    None => false })]
pub fn narrowed_verify_em_init_post_active_copy_used(
    d: &CkDev, g: &mut ExtentManager, sb: Superblock, per_region: &Vec<Vec<SlabDescriptor>>,
) -> (Result<Snapshot<Seq<RegionState>>, EmError>, Vec<Extent>) {
    let rec = recover(d);
    let img = match &rec {
        Ok(i) => *i,
        Err(_) => snapshot! { Seq::empty() },
    };
    proof_assert! { match copy_at(*d, d.sb_active) { Some(c) => img == c.img, None => false } };
    let ex = init_and_list(g, sb, per_region, img);
    (rec, ex)
}

/// Mutant: claims the component lists nothing after recovering a non-empty active copy.
#[requires(d.sb_seq@ > 0)]
#[requires(match copy_at(*d, d.sb_active) {
    Some(c) => c.seq == d.sb_seq && c.img.len() == per_region@.len()
        && (forall<i: Int> 0 <= i && i < per_region@.len() ==> descs_of(c.img[i], per_region@[i]@) && rg_inv(c.img[i])),
    None => false })]
#[requires(em_regions_wf(*g) && sb_sane(sb) && recovered_ok(sb, per_region@) && sb.region_count@ == per_region@.len())]
#[ensures(result.1@.len() == 0)]
pub fn narrowed_verify_em_init_post_active_copy_used__mutant(
    d: &CkDev, g: &mut ExtentManager, sb: Superblock, per_region: &Vec<Vec<SlabDescriptor>>,
) -> (Result<Snapshot<Seq<RegionState>>, EmError>, Vec<Extent>) {
    let rec = recover(d);
    let img = match &rec {
        Ok(i) => *i,
        Err(_) => snapshot! { Seq::empty() },
    };
    let ex = init_and_list(g, sb, per_region, img);
    (rec, ex)
}

/// **EM-INIT-POST-FALLBACK-TO-INACTIVE** — "When the active checkpoint copy is unreadable or
/// invalid (a CRC failure or a media error) and the other copy holds a valid checkpoint with
/// the immediately preceding sequence number, initialize() succeeds by rebuilding state from
/// that other copy and lists exactly the extents recorded in it." (a) recovery's control
/// flow (recovery.rs:29-58, mirror recover_rd): for an active-copy read that fails on the
/// device (IoError), fails a header/length/CRC check, or carries another sequence number,
/// and an inactive copy that reads back with sequence number seq-1 (> 0), recover picks the
/// inactive copy; (b) at image level (ckpt.rs recover) it returns the inactive copy's image,
/// and (c) initialize + get_extents list exactly that image's extents.
#[requires(d.sb_seq@ > 1 && d.sb_active@ <= 1)]
#[requires(match copy_at(*d, d.sb_active) { Some(a) => a.seq != d.sb_seq, None => true })]
#[requires(match copy_at(*d, if d.sb_active@ == 0 { 1u8 } else { 0u8 }) {
    Some(c) => c.seq@ == d.sb_seq@ - 1 && c.img.len() == per_region@.len()
        && (forall<i: Int> 0 <= i && i < per_region@.len() ==> descs_of(c.img[i], per_region@[i]@) && rg_inv(c.img[i])),
    None => false })]
#[requires(em_regions_wf(*g) && sb_sane(sb) && recovered_ok(sb, per_region@) && sb.region_count@ == per_region@.len())]
#[requires(match act { RdOut::Good(s) => s != d.sb_seq, _ => true })]
#[ensures(result.2 == Ok(Recovered::Copy(if d.sb_active@ == 0 { 1u8 } else { 0u8 })))]
#[ensures(match copy_at(*d, if d.sb_active@ == 0 { 1u8 } else { 0u8 }) {
    Some(c) => result.0 == Ok(c.img) && lists_exactly(result.1@, *c.img),
    None => false })]
pub fn verify_em_init_post_fallback_to_inactive(
    d: &CkDev, g: &mut ExtentManager, sb: Superblock, per_region: &Vec<Vec<SlabDescriptor>>, act: RdOut,
) -> (Result<Snapshot<Seq<RegionState>>, EmError>, Vec<Extent>, Result<Recovered, EmError>) {
    let prev = d.sb_seq - 1;
    let flow = recover_rd(d.sb_seq, d.sb_active, act, RdOut::Good(prev));
    let rec = recover(d);
    let img = match &rec {
        Ok(i) => *i,
        Err(_) => snapshot! { Seq::empty() },
    };
    let ex = init_and_list(g, sb, per_region, img);
    (rec, ex, flow)
}

/// Mutant: claims recovery fails in the fallback case.
#[requires(d.sb_seq@ > 1 && d.sb_active@ <= 1)]
#[requires(match act { RdOut::Good(s) => s != d.sb_seq, _ => true })]
#[ensures(result == Err(EmError::CorruptMetadata))]
pub fn verify_em_init_post_fallback_to_inactive__mutant(d: &CkDev, act: RdOut) -> Result<Recovered, EmError> {
    let prev = d.sb_seq - 1;
    recover_rd(d.sb_seq, d.sb_active, act, RdOut::Good(prev))
}

/// **EM-INIT-POST-FREE-SLOTS-REUSABLE** — "After initialize() succeeds, slots whose recovered
/// key is the FREE_KEY sentinel (including those of reservations that were not yet
/// published when the checkpoint was taken) are not listed as extents and are available to
/// be handed out by later reserve_extent() calls." For every recovered superblock and
/// descriptors (distinct starts per region): after initialize (lib.rs:546-565), every slot j
/// of every rebuilt slab whose descriptor key is FREE_KEY (a) yields no listed extent
/// (get_extents soundness: listed extents come from non-FREE slots) and (b) is clear in the
/// slab's bitmap (recovery.rs:79-84) while the slab is listed under its element size
/// (EM-SIZECLASS-NONFULL-LISTED, established by the rebuild), which is where reserve_extent
/// of that size looks first (region.rs:47-60).
#[requires(em_regions_wf(*g) && sb_sane(sb) && recovered_ok(sb, per_region@))]
#[requires(a_980f61(sb, per_region@))]
#[ensures(match (^g).regions {
    Some(rv) => forall<i: Int, p: Int, j: Int> 0 <= i && i < rv@.len() && i < per_region@.len()
        && 0 <= p && p < per_region@[i]@.len() && 0 <= j && j < per_region@[i]@[p].keys@.len()
        && per_region@[i]@[p].keys@[j] == FREE_KEY ==>
        (forall<q: Int> 0 <= q && q < result@.len() ==>
            !from_slot((^g).arena@[rv@[i]@], per_region@[i]@[p].start_offset@, j, result@[q]))
        && (^g).arena@[rv@[i]@].slabs@.contains(per_region@[i]@[p].start_offset@)
        && !slot_bit((^g).arena@[rv@[i]@].slabs@.lookup(per_region@[i]@[p].start_offset@).bitmap, j)
        && listed(sc_get((^g).arena@[rv@[i]@].size_classes, per_region@[i]@[p].element_size@), per_region@[i]@[p].start_offset@),
    None => false,
})]
pub fn verify_em_init_post_free_slots_reusable(
    g: &mut ExtentManager, sb: Superblock, per_region: &Vec<Vec<SlabDescriptor>>,
) -> Vec<Extent> {
    // LEVEL-3 (phase D-A): distinct starts per recovered list are DERIVED from 980f61's
    // non-overlap (lemma_starts_distinct; slab sizes are positive by 3b612e inside desc_ok_sb).
    proof_assert! { forall<i: Int> 0 <= i && i < per_region@.len() ==> {
        lemma_starts_distinct(per_region@[i]@,
            rbase(sb.data_start_offset@, sb.data_disk_size@ - sb.data_start_offset@, sb.region_count@, i),
            rsize(sb.data_disk_size@ - sb.data_start_offset@, sb.region_count@, i));
        starts_distinct(per_region@[i]@) } };
    g.initialize(sb, per_region);
    proof_assert! { match g.regions { Some(rv) => forall<i: Int> 0 <= i && i < rv@.len() && i < per_region@.len() ==>
        dsel(per_region@, i) == per_region@[i]@ && init_rg_ok(g.arena@[rv@[i]@], per_region@[i]@)
        && rebuilt_all(g.arena@[rv@[i]@], per_region@[i]@) && nonfull_listed(g.arena@[rv@[i]@])
        && rg_inv(g.arena@[rv@[i]@]), None => false } };
    proof_assert! { match g.regions { Some(rv) => forall<i: Int, p: Int, j: Int> 0 <= i && i < rv@.len() && i < per_region@.len()
        && 0 <= p && p < per_region@[i]@.len() && 0 <= j && j < per_region@[i]@[p].keys@.len()
        && per_region@[i]@[p].keys@[j] == FREE_KEY ==> {
            let sl = g.arena@[rv@[i]@].slabs@.lookup(per_region@[i]@[p].start_offset@);
            slab_is_desc(sl, per_region@[i]@[p]) && slab_inv(sl) && !slot_bit(sl.bitmap, j)
            && { lemma_cnt_lt(sl.bitmap, j, sl.bitmap.num_slots@); sl.bitmap.allocated_count@ < sl.bitmap.num_slots@ } },
        None => false } };
    g.get_extents()
}

/// Mutant: claims a FREE slot of a recovered slab is allocated.
#[requires(em_regions_wf(*g) && sb_sane(sb) && recovered_ok(sb, per_region@))]
#[requires(a_980f61(sb, per_region@))]
#[requires(per_region@.len() > 0 && per_region@[0]@.len() > 0 && per_region@[0]@[0].keys@.len() > 0 && per_region@[0]@[0].keys@[0] == FREE_KEY)]
#[ensures(match (^g).regions {
    Some(rv) => slot_bit((^g).arena@[rv@[0]@].slabs@.lookup(per_region@[0]@[0].start_offset@).bitmap, 0),
    None => true,
})]
pub fn verify_em_init_post_free_slots_reusable__mutant(
    g: &mut ExtentManager, sb: Superblock, per_region: &Vec<Vec<SlabDescriptor>>,
) {
    // LEVEL-3 (phase D-A): distinct starts per recovered list are DERIVED from 980f61's
    // non-overlap (lemma_starts_distinct; slab sizes are positive by 3b612e inside desc_ok_sb).
    proof_assert! { forall<i: Int> 0 <= i && i < per_region@.len() ==> {
        lemma_starts_distinct(per_region@[i]@,
            rbase(sb.data_start_offset@, sb.data_disk_size@ - sb.data_start_offset@, sb.region_count@, i),
            rsize(sb.data_disk_size@ - sb.data_start_offset@, sb.region_count@, i));
        starts_distinct(per_region@[i]@) } };
    g.initialize(sb, per_region);
}
