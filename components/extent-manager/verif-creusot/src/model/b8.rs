//! Batch-8 model additions (pin 2cd35bac line numbers). New file; no existing contract changed.
//! * `flush8` — `BlockDeviceClient::flush` (block_io.rs:161-180, cfg feature
//!   `volatile_write_cache`) with the command send and the completion receive as explicit
//!   device OUTCOMES (any outcome is allowed by the IBlockDevice contract).
//! * `ExtentManager::remove_extent8` — `remove_extent` (lib.rs:680-684) with `region_for_offset`
//!   (lib.rs:223-255) inlined FAITHFULLY: the subtraction `data_disk_size - data_start_offset`
//!   (lib.rs:243) is the real `u64` subtraction (no-underflow is a proof obligation, `rm_safe8`),
//!   and the contract states every error KIND (component.rs's mirror does not).
//! * `ExtentManager::reserve_extent8` — `reserve_extent` (lib.rs:584-630) with the handle's
//!   `extent_size` (lib.rs:591) and the uninitialised case in the contract.
//! * `ExtentManager::run_ck_front8` — `run_checkpoint` up to the metadata client
//!   (lib.rs:275-297: not initialised / nothing dirty / receptacle unbound), `bg_log8` — the
//!   background loop's error filter (lib.rs:152-159), `disconnect8` — `Receptacle::disconnect`
//!   (component-core receptacle.rs:110-117) on the public `metadata_device` field.
//! * `tsys_new8` — the timer part of `new_inner` (lib.rs:106-120) over batch 5's `TSys`.
//! * `release_align8` — region.rs:37-39 with RELEASE-build (wrapping) u32 arithmetic.
use crate::model::b3::*;
use crate::model::b5::*;
use crate::model::buddy::*;
use crate::model::component::*;
use crate::model::params::*;
use crate::model::region::*;
use crate::model::slab::*;
use crate::model::assume::*;
use creusot_std::prelude::*;

// =============================================================================
// BlockDeviceClient::flush (block_io.rs:161-180)
// =============================================================================

/// A completion as `flush` matches it (interfaces `Completion`): `FlushDone` carrying
/// `result.is_ok()`, `Error`, or any other variant.
#[derive(::core::clone::Clone, ::core::marker::Copy)]
pub enum CompF {
    FlushDone(bool),
    Error,
    Other,
}

/// Mirror of `BlockDeviceClient::flush` (block_io.rs:168-180). `send_ok` is the outcome of
/// `command_tx.send(Command::FlushSync { ns_id })` (block_io.rs:169-172: Err when the channel is
/// closed); `recv` is what `completion_rx.recv()` returns (`None` = the channel's recv failed).
/// `error::nvme_to_em` / `error::io_error` both build `IoError` (error.rs:15-25).
#[ensures(match result {
    Ok(()) => send_ok && recv == Some(CompF::FlushDone(true)),
    Err(e) => e == EmError::IoError && !(send_ok && recv == Some(CompF::FlushDone(true))),
})]
pub fn flush8(send_ok: bool, recv: Option<CompF>) -> Result<(), EmError> {
    if !send_ok {
        return Err(io_error());
    }
    match recv {
        Some(CompF::FlushDone(ok)) => {
            if ok {
                Ok(())
            } else {
                Err(nvme_to_em())
            }
        }
        Some(CompF::Error) => Err(nvme_to_em()),
        Some(CompF::Other) => Err(io_error()),
        None => Err(io_error()),
    }
}

// =============================================================================
// remove_extent / region_for_offset (lib.rs:223-255, 680-684)
// =============================================================================

/// What lib.rs:243 needs to not underflow: the published data area does not start past the
/// data disk. format establishes it (format_build: data_start_offset < data_disk_size); so does
/// initialize from a superblock satisfying sb_sane.
#[logic(open)]
pub fn rm_safe8(em: ExtentManager) -> bool {
    pearlite! {
        match (em.regions, em.shared) {
            (Some(_), Some(sh)) => sh.superblock.data_start_offset@ <= sh.format_params.data_disk_size@,
            _ => true,
        }
    }
}

/// `region_bytes` (lib.rs:243-244) is zero.
#[logic(open)]
pub fn rb_zero8(em: ExtentManager) -> bool {
    pearlite! {
        match (em.regions, em.shared) {
            (Some(rv), Some(sh)) => (sh.format_params.data_disk_size@ - sh.superblock.data_start_offset@) / rv@.len() == 0,
            _ => false,
        }
    }
}

/// Initialised and `region_bytes` > 0.
#[logic(open)]
pub fn rb_pos8(em: ExtentManager) -> bool {
    pearlite! {
        match (em.regions, em.shared) {
            (Some(rv), Some(sh)) => (sh.format_params.data_disk_size@ - sh.superblock.data_start_offset@) / rv@.len() > 0,
            _ => false,
        }
    }
}

/// Some slot of some slab of current region `k` starts at `offset`.
#[logic(open)]
pub fn slot_in8(r: RegionState, offset: Int) -> bool {
    pearlite! {
        exists<s: Int, j: Int> r.slabs@.contains(s) && 0 <= j && j < r.slabs@.lookup(s).bitmap.num_slots@
            && slot_off(r.slabs@.lookup(s), j) == offset
    }
}

/// A PUBLISHED extent of region `r` starts at `offset` (a slot holding a key != FREE_KEY,
/// what get_extents lists, lib.rs:639-648).
#[logic(open)]
pub fn pub_in8(r: RegionState, offset: Int) -> bool {
    pearlite! {
        exists<s: Int, j: Int> r.slabs@.contains(s) && 0 <= j && j < r.slabs@.lookup(s).bitmap.num_slots@
            && slot_off(r.slabs@.lookup(s), j) == offset && r.slabs@.lookup(s).keys@[j] != FREE_KEY
    }
}

/// Some current region has a slot starting at `offset`.
#[logic(open)]
pub fn slot_at8(em: ExtentManager, offset: Int) -> bool {
    pearlite! {
        match em.regions {
            Some(rv) => exists<k: Int> 0 <= k && k < rv@.len() && slot_in8(em.arena@[rv@[k]@], offset),
            None => false,
        }
    }
}

/// Some current region has a published extent starting at `offset`.
#[logic(open)]
pub fn pub_at8(em: ExtentManager, offset: Int) -> bool {
    pearlite! {
        match em.regions {
            Some(rv) => exists<k: Int> 0 <= k && k < rv@.len() && pub_in8(em.arena@[rv@[k]@], offset),
            None => false,
        }
    }
}

/// A removal step names a published slot starting at `offset`.
#[logic]
#[requires(rm_step(a, b, offset))]
#[ensures(pub_in8(a, offset@) && slot_in8(a, offset@))]
pub fn lemma_rm_pub8(a: RegionState, b: RegionState, offset: u64) {}

/// The component state is unchanged (every field; the region arena by its view).
#[logic(open)]
pub fn unch8(a: ExtentManager, b: ExtentManager) -> bool {
    pearlite! {
        b.arena@ == a.arena@ && b.regions == a.regions && b.shared == a.shared && b.dev == a.dev
        && b.metadata_base_lba == a.metadata_base_lba && b.data_base_lba == a.data_base_lba
        && b.metadata_ns_id == a.metadata_ns_id
    }
}

/// A successful remove: exactly one current region took a removal step, nothing else changed.
#[logic(open)]
pub fn rm_ok8(a: ExtentManager, b: ExtentManager, offset: u64) -> bool {
    pearlite! {
        b.arena@.len() == a.arena@.len()
        && b.regions == a.regions && b.shared == a.shared && b.dev == a.dev
        && b.metadata_base_lba == a.metadata_base_lba && b.data_base_lba == a.data_base_lba
        && b.metadata_ns_id == a.metadata_ns_id
        && match a.regions {
            Some(rv) => exists<k: Int> 0 <= k && k < rv@.len()
                && rm_step(a.arena@[rv@[k]@], b.arena@[rv@[k]@], offset)
                && pub_in8(a.arena@[rv@[k]@], offset@)
                && (forall<i: Int> 0 <= i && i < a.arena@.len() && i != rv@[k]@ ==> b.arena@[i] == a.arena@[i]),
            None => false,
        }
    }
}

impl ExtentManager {
    /// Mirror of `remove_extent` (lib.rs:680-684) with `region_for_offset` (lib.rs:223-255)
    /// inlined; the `regions` read lock and the `shared` mutex are each one atomic step.
    #[requires(em_regions_wf(*self) && em_regions_ok(*self))]
    #[requires(rm_safe8(*self))]
    #[ensures(match result { Err(_) => unch8(*self, ^self), Ok(()) => rm_ok8(*self, ^self, offset) })]
    #[ensures(match result { Err(e) => e == EmError::NotInitialized || e == EmError::OffsetNotFound(offset), Ok(()) => true })]
    #[ensures(self.regions == None ==> result == Err(EmError::NotInitialized))]
    #[ensures(self.shared == None ==> result == Err(EmError::NotInitialized))]
    #[ensures(rb_zero8(*self) ==> result == Err(EmError::NotInitialized))]
    #[ensures(rb_pos8(*self) ==> match result { Err(e) => e == EmError::OffsetNotFound(offset), Ok(()) => true })]
    pub fn remove_extent8(&mut self, offset: u64) -> Result<(), EmError> {
        let n = match &self.regions {
            Some(rv) => rv.len(),
            None => return Err(EmError::NotInitialized), // lib.rs:228-230
        };
        let (data_disk_size, data_start_offset) = match &self.shared {
            Some(s) => (s.format_params.data_disk_size, s.superblock.data_start_offset),
            None => return Err(EmError::NotInitialized), // lib.rs:233-236
        };
        let usable = data_disk_size - data_start_offset; // lib.rs:243
        let region_bytes = usable / n as u64; // lib.rs:244
        if region_bytes == 0 {
            return Err(EmError::NotInitialized); // lib.rs:245-247 "region size is zero"
        }
        let relative_offset = if offset >= data_start_offset { offset - data_start_offset } else { 0 }; // saturating_sub (lib.rs:249)
        let idx = (relative_offset / region_bytes) as usize; // lib.rs:250
        if idx >= n {
            return Err(EmError::OffsetNotFound(offset)); // lib.rs:251-253
        }
        let r = match &self.regions {
            Some(rv) => rv[idx],
            None => return Err(EmError::NotInitialized),
        };
        proof_assert! { match self.regions { Some(rv) => rv@[idx@] == r && r@ < self.arena@.len() && rg_inv(self.arena@[r@]), None => false } };
        let pre = snapshot! { *self };
        let res = self.arena[r].remove_extent_by_offset(offset); // lib.rs:682-683
        proof_assert! { match res { Err(_) => self.arena@.ext_eq(pre.arena@), Ok(()) => rm_step(pre.arena@[r@], self.arena@[r@], offset) } };
        proof_assert! { match res { Err(_) => true, Ok(()) => { lemma_rm_pub8(pre.arena@[r@], self.arena@[r@], offset); pub_in8(pre.arena@[r@], offset@) } } };
        proof_assert! { forall<i: Int> 0 <= i && i < self.arena@.len() && i != r@ ==> self.arena@[i] == pre.arena@[i] };
        res
    }

    /// Mirror of `reserve_extent` (lib.rs:584-630): region_for_key, alloc_extent under the
    /// region's write lock, `aligned_size = (size + bs - 1) / bs * bs` (lib.rs:591), the handle
    /// (lib.rs:623-629; key, offset, aligned size; the closures capture the slot).
    #[requires(em_regions_wf(*self) && em_regions_ok(*self))]
    #[requires(self.regions != None ==> size@ > 0)]
    #[requires(self.regions != None ==> rsv_cur_ok(*self, size@))]
    #[ensures(self.regions == None ==> result == Err(EmError::NotInitialized) && unch8(*self, ^self))]
    #[ensures((^self).regions == self.regions && (^self).shared == self.shared && (^self).arena@.len() == self.arena@.len())]
    #[ensures(match result {
        Ok(h) => h.key == key && h.region@ < self.arena@.len()
            && (match self.regions { Some(rv) => exists<i: Int> 0 <= i && i < rv@.len() && rv@[i] == h.region, None => false })
            && h.size@ == align_l(size@, self.arena@[h.region@].format_params.sector_size@)
            && size@ <= h.size@ && h.size@ < size@ + self.arena@[h.region@].format_params.sector_size@
            && rsv_slot((^self).arena@[h.region@], h),
        Err(_) => true,
    })]
    pub fn reserve_extent8(&mut self, key: u64, size: u32) -> Result<Handle, EmError> {
        let region = self.region_for_key(key)?;
        proof_assert! { region@ < self.arena@.len() && rg_inv(self.arena@[region@]) };
        let pre = snapshot! { *self };
        let (slab_start, slot_idx, offset) = self.arena[region].alloc_extent(size)?;
        proof_assert! { rg_frame(pre.arena@[region@], self.arena@[region@]) };
        let bs = self.arena[region].format_params.sector_size;
        proof_assert! { bs == pre.arena@[region@].format_params.sector_size && bs@ > 0 };
        proof_assert! { lemma_divmod(size@ + bs@ - 1, bs@);
            size@ + bs@ - 1 == bs@ * ((size@ + bs@ - 1) / bs@) + (size@ + bs@ - 1) % bs@ };
        proof_assert! { (size@ + bs@ - 1) / bs@ * bs@ == bs@ * ((size@ + bs@ - 1) / bs@) };
        proof_assert! { a_3783f8(size@, bs@) };
        let aligned_size = (size + bs - 1) / bs * bs; // lib.rs:591, same evaluation order (3783f8)
        Ok(Handle { region, key, offset, size: aligned_size, slab_start, slot_idx })
    }
}

// =============================================================================
// Background checkpoint error filter (lib.rs:152-159) and the front of run_checkpoint
// =============================================================================

/// Some current region is dirty (lib.rs:276-282).
#[logic(open)]
pub fn any_dirty8(em: ExtentManager) -> bool {
    pearlite! {
        match em.regions {
            Some(rv) => exists<k: Int> 0 <= k && k < rv@.len() && em.arena@[rv@[k]@].dirty,
            None => false,
        }
    }
}

impl ExtentManager {
    /// Mirror of `run_checkpoint` up to `get_metadata_client(ns_id)?` (lib.rs:275-297, 185-189):
    /// `Err(NotInitialized)` while regions is None; `Ok(false)` (= return Ok, nothing written)
    /// when no region is dirty; `Err(NotInitialized)` when the `metadata_device` receptacle is
    /// not bound (lib.rs:186-189 "metadata block device not connected"); `Ok(true)` = the
    /// checkpoint continues (its later errors are IoError / CorruptMetadata: Ck6 / component.rs).
    #[requires(em_regions_wf(*self))]
    #[ensures(self.regions == None ==> result == Err(EmError::NotInitialized))]
    #[ensures(self.regions != None && !any_dirty8(*self) ==> result == Ok(false))]
    #[ensures(any_dirty8(*self) && !self.dev.connected ==> result == Err(EmError::NotInitialized))]
    #[ensures(any_dirty8(*self) && self.dev.connected ==> result == Ok(true))]
    pub fn run_ck_front8(&self) -> Result<bool, EmError> {
        let n = match &self.regions {
            Some(rv) => rv.len(),
            None => return Err(EmError::NotInitialized),
        };
        let mut any_dirty = false;
        let mut i: usize = 0;
        #[invariant(i@ <= n@)]
        #[invariant(match self.regions { Some(rv) => rv@.len() == n@
            && (any_dirty == exists<k: Int> 0 <= k && k < i@ && self.arena@[rv@[k]@].dirty), None => false })]
        while i < n {
            match &self.regions {
                Some(rv) => {
                    if self.arena[rv[i]].dirty {
                        any_dirty = true;
                    }
                }
                None => {}
            }
            i += 1;
        }
        if !any_dirty {
            return Ok(false);
        }
        if !self.dev.connected {
            return Err(EmError::NotInitialized);
        }
        Ok(true)
    }

    /// `self.metadata_device.disconnect()` (component-core receptacle.rs:110-117): the public
    /// receptacle field becomes unbound; nothing else changes.
    #[ensures(!(^self).dev.connected)]
    #[ensures((^self).arena == self.arena && (^self).regions == self.regions && (^self).shared == self.shared)]
    #[ensures((^self).dev.sb == self.dev.sb)]
    pub fn disconnect8(&mut self) {
        self.dev.connected = false;
    }
}

/// The background loop's handling of `this.checkpoint()`'s result (lib.rs:154-157): returns
/// whether `log_error` is called.
#[ensures(result == match res { Ok(()) => false, Err(EmError::NotInitialized) => false, Err(_) => true })]
pub fn bg_log8(res: Result<(), EmError>) -> bool {
    match res {
        Ok(()) => false,
        Err(EmError::NotInitialized) => false,
        Err(_) => true,
    }
}

// =============================================================================
// new_inner's timer setup (lib.rs:106-120) over batch 5's TSys
// =============================================================================

/// `ExtentManager::new_inner` (lib.rs:106-120) restricted to the timer: `new_default()` builds
/// `CheckpointTimerState::default()` (interval None, shutdown false; lib.rs:62-67), then
/// `set_interval(Some(Duration::from_secs(30)))` (lib.rs:110-112; ticks are seconds), then the
/// thread is spawned at its loop head (lib.rs:120-121). No checkpoint has run.
#[ensures(result.interval == Some(30u64) && result.ph == TPh::Top && !result.shutdown && result.ckpts@ == 0)]
#[ensures(*result.now == 0)]
pub fn tsys_new8() -> TSys {
    let mut t = TSys { interval: None, now: snapshot! { 0 }, ph: TPh::Top, shutdown: false, ckpts: 0 };
    t.set_interval(Some(30));
    t
}

// =============================================================================
// align_to_sector_size, release build (region.rs:37-39)
// =============================================================================

/// `(size + sector_size - 1) / sector_size * sector_size` with wrapping u32 `+` (a debug build
/// panics on the overflow instead). Exact.
#[requires(ss@ > 0)]
#[ensures(size@ + ss@ - 1 <= u32::MAX@ ==> result@ == (size@ + ss@ - 1) / ss@ * ss@)]
#[ensures(size@ + ss@ - 1 > u32::MAX@ ==> result@ == (size@ + ss@ - 1 - 4294967296) / ss@ * ss@)]
pub fn release_align8(size: u32, ss: u32) -> u32 {
    let w: u64 = size as u64 + ss as u64 - 1;
    let v: u64 = if w > 4294967295 { w - 4294967296 } else { w };
    proof_assert! { lemma_divmod(v@, ss@); v@ / ss@ * ss@ <= v@ };
    let q: u64 = v / ss as u64 * ss as u64;
    proof_assert! { q@ <= v@ && q@ < 4294967296 };
    as_u32(q)
}
