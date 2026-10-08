//! Batch-3 mirrors (pin 2cd35bac line numbers):
//! * `ExtentManager::new_inner` / `new_default` field defaults (lib.rs:110-171, 85-107);
//! * the namespace `run_checkpoint` picks (lib.rs:291-296);
//! * the checkpoint-copy LBA computation (checkpoint.rs:58-64 + :96, recovery.rs:23-26 +
//!   checkpoint.rs:123-124);
//! * an EVENT-LOGGING mirror of `BlockDeviceClient::alloc_buffer` / `write_blocks` /
//!   `read_blocks` (block_io.rs:42-47, 49-108, 110-155) and of the allocator selection in
//!   `get_metadata_client` (lib.rs:173-175, 189-213), with `error::nvme_to_em` /
//!   `error::io_error` (error.rs:15-25);
//! * the control flow of `recovery::recover` over the OUTCOME of each checkpoint-copy read
//!   (recovery.rs:11-71);
//! * `WriteHandle` with its two `Option` closures (interfaces/src/iextent_manager.rs:95-155).
//!
//! The event mirror complements `model/blockio.rs` (which models block CONTENTS): here every
//! DMA allocation, command send and completion receive is a trusted channel leaf that
//! appends one event to a ghost log, so properties about the ERROR PATHS (what happens after
//! an allocation or device failure, which allocator is used) can be stated. Buffer contents
//! are not modelled here (blockio.rs's write_blocks covers them; both mirror the same code).
use crate::model::component::*;
use crate::model::params::*;
use crate::model::region::*;
use crate::model::slab::*;
use crate::model::assume::*;
use creusot_std::prelude::*;

// ------------------------------------------------------------------ errors (error.rs)

/// `error::nvme_to_em` (error.rs:23-25): every `NvmeBlockError` becomes `IoError` (the
/// message string is not modelled).
#[ensures(result == EmError::IoError)]
pub fn nvme_to_em() -> EmError {
    EmError::IoError
}

/// `error::io_error` (error.rs:15-17).
#[ensures(result == EmError::IoError)]
pub fn io_error() -> EmError {
    EmError::IoError
}

// ------------------------------------------------------------------ new_inner

/// Mirror of `ExtentManager::new_inner` (lib.rs:110-171): `new_default()` builds every
/// field with `Default` (lib.rs:85-107) — `Mutex<u64>` → 0, `Mutex<Option<_>>` → None —
/// and no format/initialize has run. (The background checkpoint thread it starts is not
/// modelled.)
#[ensures(result.metadata_base_lba@ == 0 && result.data_base_lba@ == 0)]
#[ensures(result.regions == None && result.shared == None && result.metadata_ns_id == None)]
#[ensures(result.arena@.len() == 0 && result.dev == dev)]
pub fn new_inner(dev: MetaDevice) -> ExtentManager {
    ExtentManager {
        arena: Vec::new(),
        regions: None,
        shared: None,
        metadata_ns_id: None,
        metadata_base_lba: 0,
        data_base_lba: 0,
        dev,
    }
}

impl ExtentManager {
    /// The namespace `run_checkpoint` writes to (lib.rs:291-296):
    /// `shared.as_ref().map_or(1, |s| s.format_params.metadata_disk_ns_id)`.
    #[ensures(match self.shared { Some(s) => result == s.format_params.metadata_disk_ns_id, None => result@ == 1 })]
    pub fn ckpt_ns(&self) -> u32 {
        match &self.shared {
            Some(s) => s.format_params.metadata_disk_ns_id,
            None => 1,
        }
    }
}

// ------------------------------------------------------------------ checkpoint LBAs

/// Byte offset of checkpoint copy `copy` (checkpoint.rs:61-63, recovery.rs:23-26).
#[logic(open)]
pub fn copy_off(sb: Superblock, copy: Int) -> Int {
    pearlite! { sb.checkpoint_region_offset@ + copy * sb.checkpoint_region_size@ }
}

/// The LBA a checkpoint copy is written at (checkpoint.rs:58-64, 96) and read from
/// (checkpoint.rs:123-124 via recovery.rs:23-26): byte offset divided — truncating — by the
/// METADATA client's sector size `md_ss` (`metadata_client.sector_size()`).
#[requires(md_ss@ > 0 && copy@ <= 1)]
#[requires(a_a23fc2(sb.checkpoint_region_offset@, sb.checkpoint_region_size@))]
#[ensures(result@ == copy_off(*sb, copy@) / md_ss@)]
pub fn ckpt_lba(sb: &Superblock, copy: u8, md_ss: u32) -> u64 {
    let offset = sb.checkpoint_region_offset + copy as u64 * sb.checkpoint_region_size;
    offset / md_ss as u64
}

// ------------------------------------------------------------------ event-logged block I/O

/// Which DMA allocator a client calls: the default `DmaBuffer::new` closure built at
/// lib.rs:197-201, or the one installed by `set_dma_alloc` (identified by an id).
#[derive(::core::clone::Clone, ::core::marker::Copy)]
pub enum AllocSel {
    Default,
    User(u64),
}

/// A completion as `block_io` matches it (interfaces `Completion`): `WriteDone`/`ReadDone`
/// carrying `result.is_ok()`, `Error`, or any other variant.
#[derive(::core::clone::Clone, ::core::marker::Copy)]
pub enum Comp {
    WriteDone(bool),
    ReadDone(bool),
    Error,
    Other,
}

/// One device-side event: a DMA allocation (allocator, success), a command send
/// (namespace, LBA, success), a completion receive (`None` = the channel's recv failed).
#[derive(::core::clone::Clone, ::core::marker::Copy)]
pub enum Ev {
    Alloc(AllocSel, bool),
    Send(u32, u64, bool),
    Recv(Option<Comp>),
}

/// Ghost event log of the metadata block device client.
pub struct EvDev {
    pub log: Snapshot<Seq<Ev>>,
}

/// TRUSTED leaf: `(self.alloc)(size, align, None)` (block_io.rs:44) — may fail.
#[trusted]
#[ensures(*(^dev).log == (*dev.log).push_back(Ev::Alloc(a, match result { Ok(_) => true, Err(_) => false })))]
#[ensures(match result { Ok(v) => v@.len() == n@, Err(_) => true })]
pub fn dma_call(dev: &mut EvDev, a: AllocSel, n: usize) -> Result<Vec<u8>, ()> {
    Err(())
}

/// TRUSTED leaf: `self.channels.command_tx.send(Command::{Write,Read}Sync { ns_id, lba, .. })`
/// (block_io.rs:84-91, 127-134) — fails when the channel is closed.
#[trusted]
#[ensures(*(^dev).log == (*dev.log).push_back(Ev::Send(ns, lba, match result { Ok(_) => true, Err(_) => false })))]
pub fn chan_send(dev: &mut EvDev, ns: u32, lba: u64) -> Result<(), ()> {
    Err(())
}

/// TRUSTED leaf: `self.channels.completion_rx.recv()` (block_io.rs:93, 136).
#[trusted]
#[ensures(*(^dev).log == (*dev.log).push_back(Ev::Recv(match result { Ok(c) => Some(c), Err(_) => None })))]
pub fn chan_recv(dev: &mut EvDev) -> Result<Comp, ()> {
    Err(())
}

/// A successful event of a write (resp. read) issued with allocator `a`.
#[logic(open)]
pub fn ev_good(e: Ev, a: AllocSel, write: bool) -> bool {
    pearlite! {
        match e {
            Ev::Alloc(x, ok) => x == a && ok,
            Ev::Send(_, _, ok) => ok,
            Ev::Recv(c) => if write { c == Some(Comp::WriteDone(true)) } else { c == Some(Comp::ReadDone(true)) },
        }
    }
}

/// An allocation failure event.
#[logic(open)]
pub fn ev_alloc_fail(e: Ev) -> bool {
    pearlite! { match e { Ev::Alloc(_, ok) => !ok, _ => false } }
}

/// A device failure event of a write: the send failed, the receive failed, or the
/// completion is not a successful `WriteDone` (an error, an unexpected kind, a failed write).
#[logic(open)]
pub fn ev_dev_fail(e: Ev, write: bool) -> bool {
    pearlite! {
        match e {
            Ev::Send(_, _, ok) => !ok,
            Ev::Recv(c) => if write { c != Some(Comp::WriteDone(true)) } else { c != Some(Comp::ReadDone(true)) },
            _ => false,
        }
    }
}

/// The allocator of an allocation event (or `a` for any other event).
#[logic(open)]
pub fn ev_alloc_is(e: Ev, a: AllocSel) -> bool {
    pearlite! { match e { Ev::Alloc(x, _) => x == a, _ => true } }
}

/// A send event.
#[logic(open)]
pub fn ev_is_send(e: Ev) -> bool {
    pearlite! { match e { Ev::Send(_, _, _) => true, _ => false } }
}

/// What every block-I/O call guarantees about the events it appended to `l` from `n0` on:
/// * every allocation used allocator `a`;
/// * every command send is IMMEDIATELY preceded by a successful allocation (its block's
///   buffer) appended by the same call;
/// * an allocation failure or a device failure is the LAST event, and the call then
///   returned `IoError`;
/// * a failed call ended with such a failure event.
#[logic(open)]
pub fn io_post(l: Seq<Ev>, n0: Int, a: AllocSel, write: bool, ok: bool, err_io: bool) -> bool {
    pearlite! {
        n0 <= l.len()
        && (forall<p: Int> n0 <= p && p < l.len() ==> ev_alloc_is(l[p], a))
        && (forall<p: Int> n0 <= p && p < l.len() && ev_is_send(l[p]) ==> n0 < p && l[p - 1] == Ev::Alloc(a, true))
        && (forall<p: Int> n0 <= p && p < l.len() && (ev_alloc_fail(l[p]) || ev_dev_fail(l[p], write)) ==>
                p == l.len() - 1 && !ok && err_io)
        && (!ok ==> err_io && l.len() > n0 && (ev_alloc_fail(l[l.len() - 1]) || ev_dev_fail(l[l.len() - 1], write)))
    }
}

/// Mirror of `BlockDeviceClient` (block_io.rs:6-12) with its allocator identity.
#[derive(::core::clone::Clone, ::core::marker::Copy)]
pub struct EvClient {
    pub sector_size: u32,
    pub ns_id: u32,
    pub base_lba: u64,
    pub alloc: AllocSel,
}

/// The configuration fields `get_metadata_client` reads (lib.rs:98, 102).
pub struct DmaCfg {
    pub dma_alloc: Option<u64>,
    pub metadata_base_lba: u64,
}

impl DmaCfg {
    /// `new_default()`'s values of the two fields (lib.rs:85-107): `None`, 0.
    #[ensures(result.dma_alloc == None && result.metadata_base_lba@ == 0)]
    pub fn new_default() -> DmaCfg {
        DmaCfg { dma_alloc: None, metadata_base_lba: 0 }
    }

    /// Mirror of `ExtentManager::set_dma_alloc` (lib.rs:173-175).
    #[ensures((^self).dma_alloc == Some(a) && (^self).metadata_base_lba == self.metadata_base_lba)]
    pub fn set_dma_alloc(&mut self, a: u64) {
        self.dma_alloc = Some(a);
    }

    /// Mirror of `get_metadata_client` (lib.rs:189-213): the client gets
    /// `self.dma_alloc.lock().unwrap().clone().unwrap_or_else(<DmaBuffer::new closure>)`,
    /// the device's sector size and the current `metadata_base_lba`. (`connect_client` and
    /// `sector_size(ns)` failures return before any client exists.)
    #[ensures(result.alloc == match self.dma_alloc { Some(a) => AllocSel::User(a), None => AllocSel::Default })]
    #[ensures(result.ns_id == ns_id && result.sector_size == sector_size && result.base_lba == self.metadata_base_lba)]
    pub fn get_metadata_client(&self, ns_id: u32, sector_size: u32) -> EvClient {
        let alloc = match self.dma_alloc {
            Some(a) => AllocSel::User(a),
            None => AllocSel::Default,
        };
        EvClient { sector_size, ns_id, base_lba: self.metadata_base_lba, alloc }
    }
}

impl EvClient {
    /// Mirror of `alloc_buffer` (block_io.rs:42-47): calls the client's allocator; an
    /// allocator error becomes `NvmeBlockError::BlockDevice(DmaAllocationFailed)` (`Err(())`).
    #[ensures(*(^dev).log == (*dev.log).push_back(Ev::Alloc(self.alloc, match result { Ok(_) => true, Err(_) => false })))]
    #[ensures(match result { Ok(v) => v@.len() == size@, Err(_) => true })]
    pub fn alloc_buffer(&self, dev: &mut EvDev, size: usize) -> Result<Vec<u8>, ()> {
        dma_call(dev, self.alloc, size)
    }

    /// Mirror of `write_blocks` (block_io.rs:49-108); the byte copies (lines 58-63, 75-77)
    /// are omitted (contents: model/blockio.rs).
    #[requires(self.sector_size@ > 0)]
    #[requires(data@.len() + self.sector_size@ <= usize::MAX@)]
    #[requires((data@.len() + self.sector_size@ - 1) / self.sector_size@ * self.sector_size@ <= usize::MAX@)]
    #[requires(self.base_lba@ + lba@ + (data@.len() + self.sector_size@ - 1) / self.sector_size@ <= u64::MAX@)]
    #[ensures(forall<p: Int> 0 <= p && p < dev.log.len() ==> (*(^dev).log)[p] == (*dev.log)[p])]
    #[ensures(io_post(*(^dev).log, dev.log.len(), self.alloc, true, result == Ok(()), result == Err(EmError::IoError)))]
    #[ensures(forall<e: EmError> result == Err(e) ==> e == EmError::IoError)]
    pub fn write_blocks(&self, dev: &mut EvDev, lba: u64, data: &Vec<u8>) -> Result<(), EmError> {
        let ss = self.sector_size as usize;
        let num_blocks = (data.len() + ss - 1) / ss;
        let buf_size = num_blocks * ss;
        let d0 = snapshot! { *dev.log };
        let _buf = match self.alloc_buffer(dev, buf_size) {
            Ok(b) => b,
            Err(_) => return Err(nvme_to_em()),
        };
        let mut i: usize = 0;
        #[invariant(i@ <= num_blocks@)]
        #[invariant(d0.len() < dev.log.len())]
        #[invariant(forall<p: Int> 0 <= p && p < d0.len() ==> (*dev.log)[p] == d0[p])]
        #[invariant(forall<p: Int> d0.len() <= p && p < dev.log.len() ==> ev_good((*dev.log)[p], self.alloc, true))]
        #[invariant(forall<p: Int> d0.len() <= p && p < dev.log.len() && ev_is_send((*dev.log)[p]) ==>
            d0.len() < p && (*dev.log)[p - 1] == Ev::Alloc(self.alloc, true))]
        while i < num_blocks {
            let block_lba = self.base_lba + lba + i as u64;
            let l1 = snapshot! { *dev.log };
            let _block_buf = match self.alloc_buffer(dev, ss) {
                Ok(b) => b,
                Err(_) => return Err(nvme_to_em()),
            };
            proof_assert! { (*dev.log)[l1.len()] == Ev::Alloc(self.alloc, true) };
            match chan_send(dev, self.ns_id, block_lba) {
                Ok(()) => {}
                Err(()) => return Err(io_error()),
            }
            match chan_recv(dev) {
                Ok(Comp::WriteDone(ok)) => {
                    if !ok {
                        return Err(nvme_to_em());
                    }
                }
                Ok(Comp::Error) => return Err(nvme_to_em()),
                Ok(_) => return Err(io_error()),
                Err(()) => return Err(io_error()),
            }
            i += 1;
        }
        Ok(())
    }

    /// Mirror of `read_blocks` (block_io.rs:110-155); the byte copy (lines 137-141) is
    /// omitted.
    #[requires(self.sector_size@ > 0)]
    #[requires(num_bytes@ + self.sector_size@ <= usize::MAX@)]
    #[requires(self.base_lba@ + lba@ + (num_bytes@ + self.sector_size@ - 1) / self.sector_size@ <= u64::MAX@)]
    #[ensures(forall<p: Int> 0 <= p && p < dev.log.len() ==> (*(^dev).log)[p] == (*dev.log)[p])]
    #[ensures(io_post(*(^dev).log, dev.log.len(), self.alloc, false, result == Ok(()), result == Err(EmError::IoError)))]
    #[ensures(forall<e: EmError> result == Err(e) ==> e == EmError::IoError)]
    pub fn read_blocks(&self, dev: &mut EvDev, lba: u64, num_bytes: usize) -> Result<(), EmError> {
        let ss = self.sector_size as usize;
        let num_blocks = (num_bytes + ss - 1) / ss;
        let d0 = snapshot! { *dev.log };
        let mut i: usize = 0;
        #[invariant(i@ <= num_blocks@)]
        #[invariant(d0.len() <= dev.log.len())]
        #[invariant(forall<p: Int> 0 <= p && p < d0.len() ==> (*dev.log)[p] == d0[p])]
        #[invariant(forall<p: Int> d0.len() <= p && p < dev.log.len() ==> ev_good((*dev.log)[p], self.alloc, false))]
        #[invariant(forall<p: Int> d0.len() <= p && p < dev.log.len() && ev_is_send((*dev.log)[p]) ==>
            d0.len() < p && (*dev.log)[p - 1] == Ev::Alloc(self.alloc, true))]
        while i < num_blocks {
            let block_lba = self.base_lba + lba + i as u64;
            let l1 = snapshot! { *dev.log };
            let _buf = match self.alloc_buffer(dev, ss) {
                Ok(b) => b,
                Err(_) => return Err(nvme_to_em()),
            };
            proof_assert! { (*dev.log)[l1.len()] == Ev::Alloc(self.alloc, true) };
            match chan_send(dev, self.ns_id, block_lba) {
                Ok(()) => {}
                Err(()) => return Err(io_error()),
            }
            match chan_recv(dev) {
                Ok(Comp::ReadDone(ok)) => {
                    if !ok {
                        return Err(nvme_to_em());
                    }
                }
                Ok(Comp::Error) => return Err(nvme_to_em()),
                Ok(_) => return Err(io_error()),
                Err(()) => return Err(io_error()),
            }
            i += 1;
        }
        Ok(())
    }
}

// ------------------------------------------------------------------ recovery control flow

/// Outcome of `checkpoint::read_checkpoint_region` (checkpoint.rs:117-173) for one copy:
/// one of its `read_blocks` calls failed on the device (`?` propagates read_blocks' error,
/// which is `IoError` for every device failure — EM-BIO-*), a header/length/CRC check
/// failed (`CorruptMetadata`), or the header was read with sequence number `s` and the
/// payload passed its CRC.
#[derive(::core::clone::Clone, ::core::marker::Copy)]
pub enum RdOut {
    DevErr,
    Bad,
    Good(u64),
}

/// Mirror of `read_checkpoint_region`'s result for outcome `o` (seq check checkpoint.rs:136-140).
#[ensures(match o {
    RdOut::DevErr => result == Err(EmError::IoError),
    RdOut::Bad => result == Err(EmError::CorruptMetadata),
    RdOut::Good(s) => if s == expected { result == Ok(()) } else { result == Err(EmError::CorruptMetadata) },
})]
pub fn read_ckpt(o: RdOut, expected: u64) -> Result<(), EmError> {
    match o {
        RdOut::DevErr => Err(nvme_to_em()),
        RdOut::Bad => Err(EmError::CorruptMetadata),
        RdOut::Good(s) => {
            if s != expected {
                Err(EmError::CorruptMetadata)
            } else {
                Ok(())
            }
        }
    }
}

/// Which state `recover` returns: no checkpoint yet (empty regions), or copy 0 / copy 1.
#[derive(::core::clone::Clone, ::core::marker::Copy)]
pub enum Recovered {
    Empty,
    Copy(u8),
}

/// Mirror of `recovery::recover` (recovery.rs:11-71) after a successful superblock read and
/// decode (recovery.rs:15-16; a failing superblock read propagates read_blocks' `IoError`),
/// over the outcomes `act` / `inact` of reading the active and the inactive copy. The
/// `deserialize_slabs(&data)?` decode failures (recovery.rs:31, 56) are not modelled (they
/// are CorruptMetadata of bytes that passed the CRC, not device failures). `active <= 1`
/// (the code computes `1 - active_copy`, recovery.rs:26).
#[requires(active@ <= 1)]
#[ensures(sb_seq@ == 0 ==> result == Ok(Recovered::Empty))]
#[ensures(sb_seq@ > 0 && act == RdOut::Good(sb_seq) ==> result == Ok(Recovered::Copy(active)))]
#[ensures(forall<q: u64> sb_seq@ > 1 && q@ == sb_seq@ - 1 && act != RdOut::Good(sb_seq) && inact == RdOut::Good(q) ==>
    result == Ok(Recovered::Copy(if active@ == 0 { 1u8 } else { 0u8 })))]
#[ensures(sb_seq@ > 0 && act != RdOut::Good(sb_seq)
    && (sb_seq@ == 1 || forall<q: u64> q@ == sb_seq@ - 1 ==> inact != RdOut::Good(q)) ==> result == Err(EmError::CorruptMetadata))]
pub fn recover_rd(sb_seq: u64, active: u8, act: RdOut, inact: RdOut) -> Result<Recovered, EmError> {
    if sb_seq == 0 {
        return Ok(Recovered::Empty);
    }
    match read_ckpt(act, sb_seq) {
        Ok(()) => return Ok(Recovered::Copy(active)),
        Err(_e) => {
            // component.log_warn("recovery_fallback: ...") (recovery.rs:40-45)
        }
    }
    let prev_seq = sb_seq - 1; // `saturating_sub(1)`, sb_seq > 0 here (recovery.rs:48)
    if prev_seq > 0 {
        match read_ckpt(inact, prev_seq) {
            Ok(()) => return Ok(Recovered::Copy(1 - active)),
            Err(_e) => {
                // component.log_error("corruption_detected: ...") (recovery.rs:60-64)
            }
        }
    }
    Err(EmError::CorruptMetadata)
}

// ------------------------------------------------------------------ WriteHandle

/// `WriteHandle` (iextent_manager.rs:95-101): the captured handle values and whether each
/// of the two `Option<Box<dyn FnOnce>>` closures is still present.
#[derive(::core::clone::Clone, ::core::marker::Copy)]
pub struct WH {
    pub h: Handle,
    pub publish_fn: bool,
    pub abort_fn: bool,
}

impl WH {
    /// `WriteHandle::new` (iextent_manager.rs:104-121): both closures present.
    #[ensures(result.h == h && result.publish_fn && result.abort_fn)]
    pub fn new(h: Handle) -> WH {
        WH { h, publish_fn: true, abort_fn: true }
    }

    /// `impl Drop for WriteHandle` (iextent_manager.rs:149-155): runs the abort closure if
    /// it is still present (lib.rs:618-621 → `free_slot`).
    #[requires(self.abort_fn ==> self.h.region@ < em.arena@.len() && rg_inv(em.arena@[self.h.region@])
        && free_ok(em.arena@[self.h.region@], self.h.slab_start, self.h.slot_idx))]
    #[ensures(!self.abort_fn ==> ^em == *em)]
    #[ensures(self.abort_fn ==> (^em).arena@.len() == em.arena@.len() && rg_inv((^em).arena@[self.h.region@])
        && (forall<i: Int> 0 <= i && i < em.arena@.len() && i != self.h.region@ ==> (^em).arena@[i] == em.arena@[i])
        && (^em).regions == em.regions && (^em).shared == em.shared
        && forall<sl: Slab> em.arena@[self.h.region@].slabs@.get(self.h.slab_start@) == Some(sl) ==>
            free_case(em.arena@[self.h.region@], (^em).arena@[self.h.region@], self.h.slab_start, self.h.slot_idx, sl))]
    pub fn drop_wh(mut self, em: &mut ExtentManager) {
        if self.abort_fn {
            self.abort_fn = false;
            em.abort(self.h);
        }
    }

    /// `WriteHandle::publish(mut self)` (iextent_manager.rs:132-139): take the publish
    /// closure (`expect`), take the abort closure, run the publish closure (lib.rs:597-616);
    /// `self` is dropped when `publish` returns (Drop, iextent_manager.rs:149-155).
    /// `result.1` is the component right after the publish closure ran.
    #[requires(self.publish_fn)]
    #[requires(self.h.region@ < em.arena@.len() && rg_inv(em.arena@[self.h.region@]))]
    #[requires(self.h.key == FREE_KEY ==> free_ok(em.arena@[self.h.region@], self.h.slab_start, self.h.slot_idx))]
    #[requires(match em.arena@[self.h.region@].slabs@.get(self.h.slab_start@) { Some(sl) => self.h.slot_idx@ < sl.keys@.len(), None => true })]
    #[ensures(^em == *result.1)]
    #[ensures(pub_post(*em, *result.1, self.h))]
    #[ensures(result.1.arena@.len() == em.arena@.len() && rg_inv(result.1.arena@[self.h.region@]))]
    #[ensures(forall<i: Int> 0 <= i && i < em.arena@.len() && i != self.h.region@ ==> result.1.arena@[i] == em.arena@[i])]
    #[ensures(result.1.regions == em.regions && result.1.shared == em.shared)]
    #[ensures(result.0 == Ok((self.h.key, self.h.offset, self.h.size)))]
    pub fn publish(mut self, em: &mut ExtentManager) -> (Result<(u64, u64, u32), EmError>, Snapshot<ExtentManager>) {
        self.publish_fn = false; // self.publish_fn.take().expect(..)
        self.abort_fn = false; // self.abort_fn.take()
        let r = em.publish(self.h);
        let after = snapshot! { *em };
        self.drop_wh(em); // `self` goes out of scope
        (r, after)
    }
}
