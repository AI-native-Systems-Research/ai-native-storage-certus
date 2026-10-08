//! Batch-6 model additions (pin 2cd35bac):
//! * `Ck6` — `run_checkpoint` (lib.rs:275-370) + `write_checkpoint` (checkpoint.rs:35-113) at
//!   DEVICE-COMMAND granularity: every metadata-device command the checkpoint issues (copy write,
//!   flush, superblock write) and the post-checkpoint hook call are appended to a ghost event log
//!   with their outcome. The outcome of each command is an explicit oracle (`Io6`): the
//!   IBlockDevice contract lets any command fail. `volatile_write_cache` (cfg feature, FR-030) is
//!   the `vwc` flag. Region contents are reduced to their dirty flags and the serialised payload
//!   length `plen` (the slab serialisation is batch 2/5's `serialize_region`).
//! * `Cs6` — the coalescing protocol of `checkpoint()` (lib.rs:729-761) with EXACT step
//!   contracts and the value each caller returns (batch 4's `CoSys` proves the invariant only).
//! * `format_pre6` / `ExtentManager::format6` — `format` (lib.rs:383-511) with the device queries
//!   (lib.rs:410-415) and the instance-id generation (lib.rs:471-481) as explicit outcomes; the
//!   remainder (lib.rs:457-510) is component.rs's `format` mirror.
//! * `ckoff_release` — lib.rs:420-424 with RELEASE-build (wrapping) arithmetic.
//! * `get6` / `for_each6` — get_extents / for_each_extent (lib.rs:632-678) with an EXACT result
//!   (the extents in iteration order), over the new trusted leaf `SlabMap::keys_det` (BTreeMap
//!   iteration order is a function of the map's contents).
use crate::model::b4::*;
use crate::model::buddy::*;
use crate::model::component::*;
use crate::model::listing::*;
use crate::model::params::*;
use crate::model::region::*;
use crate::model::slab::*;
use creusot_std::logic::FMap;
use crate::model::assume::*;
use creusot_std::prelude::*;

// =============================================================================
// Ck6: checkpoint at device-command granularity
// =============================================================================

/// One metadata-device command of a checkpoint, or the hook call.
pub enum Ev6 {
    /// `metadata_client.write_blocks(lba, &blob)` of a checkpoint copy (checkpoint.rs:96-97):
    /// copy index, first byte (`lba * sector_size`), byte length, outcome.
    Data(Int, Int, Int, bool),
    /// `metadata_client.flush()` (checkpoint.rs:102, lib.rs:310): outcome.
    Flush(bool),
    /// `metadata_client.write_blocks(0, &superblock.serialize())` (lib.rs:302-307): the
    /// superblock's checkpoint_seq and active_copy, outcome.
    Sb(Int, Int, bool),
    /// `hook()` (lib.rs:365-367).
    Hook,
}

/// Device outcomes of one checkpoint attempt (each command may fail, IBlockDevice contract).
#[derive(::core::clone::Clone, ::core::marker::Copy)]
pub struct Io6 {
    /// `get_metadata_client(ns_id)` (lib.rs:297): device bound and client created.
    pub client: bool,
    /// the copy write (checkpoint.rs:97)
    pub data: bool,
    /// the flush after the copy write (checkpoint.rs:102, volatile_write_cache only)
    pub fl1: bool,
    /// the superblock write (lib.rs:306)
    pub sb: bool,
    /// the flush after the superblock write (lib.rs:310, volatile_write_cache only)
    pub fl2: bool,
}

/// The state the checkpoint path reads and writes.
pub struct Ck6 {
    /// `regions` (None before format/initialize) reduced to each region's `dirty` flag.
    pub regions: Option<Vec<bool>>,
    /// in-memory `shared.superblock.active_copy` / `shared.checkpoint_seq`
    pub active: u8,
    pub seq: u64,
    /// `superblock.checkpoint_region_offset` / `checkpoint_region_size`
    pub ck_off: u64,
    pub ck_size: u64,
    /// `metadata_client.sector_size()` (checkpoint.rs:91)
    pub ms: u64,
    /// `post_checkpoint_hook` is registered (lib.rs:177-179)
    pub hook: bool,
    /// `cfg(feature = "volatile_write_cache")`
    pub vwc: bool,
    /// every command issued so far
    pub log: Snapshot<Seq<Ev6>>,
}

#[logic(open)]
pub fn any_dirty6(rs: Seq<bool>) -> bool {
    pearlite! { exists<i: Int> 0 <= i && i < rs.len() && rs[i] }
}

/// First byte of the copy write: `(region_offset / sector_size) * sector_size` (checkpoint.rs:96).
#[logic(open)]
pub fn ck_start(s: Ck6) -> Int {
    pearlite! { (s.ck_off@ + (1 - s.active@) * s.ck_size@) / s.ms@ * s.ms@ }
}

/// Byte length of the copy write: header + payload rounded up to the sector (checkpoint.rs:91-93).
#[logic(open)]
pub fn ck_len(s: Ck6, plen: Int) -> Int {
    pearlite! { (16 + plen + s.ms@ - 1) / s.ms@ * s.ms@ }
}

/// The command sequence of a checkpoint attempt that passed the size check.
#[logic(open)]
pub fn ck_evs(s: Ck6, plen: Int, io: Io6) -> Seq<Ev6> {
    pearlite! {
        let s0 = Seq::singleton(Ev6::Data(1 - s.active@, ck_start(s), ck_len(s, plen), io.data));
        if !io.data { s0 } else {
            let s1 = if s.vwc { s0.push_back(Ev6::Flush(io.fl1)) } else { s0 };
            if s.vwc && !io.fl1 { s1 } else {
                let s2 = s1.push_back(Ev6::Sb(s.seq@ + 1, 1 - s.active@, io.sb));
                if !io.sb { s2 } else {
                    let s3 = if s.vwc { s2.push_back(Ev6::Flush(io.fl2)) } else { s2 };
                    if s.vwc && !io.fl2 { s3 } else if s.hook { s3.push_back(Ev6::Hook) } else { s3 }
                }
            }
        }
    }
}

/// Every command of the attempt succeeded.
#[logic(open)]
pub fn io_ok(io: Io6, vwc: bool) -> bool {
    pearlite! { io.data && (!vwc || io.fl1) && io.sb && (!vwc || io.fl2) }
}

/// The in-memory superblock update (checkpoint.rs:105-112) happened: the copy write (and its
/// flush) succeeded.
#[logic(open)]
pub fn io_data_ok(io: Io6, vwc: bool) -> bool {
    pearlite! { io.data && (!vwc || io.fl1) }
}

/// The checkpoint path's declared ranges (phase F, level 3): 0de740 (active copy 0/1),
/// 9c0757 (seq < MAX), dfd6b0 (metadata sector divides the layout => ms != 0), the u64 layout
/// bound (a23fc2 on initialize; format's own postcondition on a format start; the u64 part of
/// 1d780b), 87044b (payload < 4 GiB) and the TYPE fact that ms is a u32 sector size.
#[logic(open)]
pub fn ck6_pre(s: Ck6, plen: Int) -> bool {
    pearlite! {
        a_0de740(s.active@) && a_9c0757(s.seq@) && a_dfd6b0(s.ms@, s.ck_off@, s.ck_size@)
        && a_a23fc2(s.ck_off@, s.ck_size@) && a_87044b(plen) && t_u32(s.ms@)
    }
}

/// DERIVED from [`ck6_pre`] (linear arithmetic): the headroom run6 uses.
#[logic(open)]
pub fn ck6_hd(s: Ck6, plen: Int) -> bool {
    pearlite! {
        s.active@ <= 1 && s.seq@ < u64::MAX@ && s.ms@ > 0
        && s.ck_off@ + 2 * s.ck_size@ <= u64::MAX@
        && 16 + plen + s.ms@ <= u64::MAX@
    }
}

impl Ck6 {
    /// Mirror of `run_checkpoint` (lib.rs:275-370) with `write_checkpoint` (checkpoint.rs:35-113)
    /// inlined, at device-command granularity.
    // Level 3: the derived headroom [`ck6_hd`]; every caller discharges it from [`ck6_pre`].
    // LEVEL-3 (phase D-B2): required only on the path that reaches its arithmetic (some region
    // dirty and the metadata client obtained, lib.rs:288-297): a clean checkpoint reads nothing.
    #[requires(match self.regions { Some(rs) => any_dirty6(rs@) && io.client ==> ck6_hd(*self, plen@), None => true })]
    #[ensures(self.regions == None ==> result == Err(EmError::NotInitialized) && ^self == *self)]
    #[ensures(match self.regions { Some(rs) => !any_dirty6(rs@) ==> result == Ok(()) && ^self == *self, None => true })]
    #[ensures(match self.regions { Some(rs) => any_dirty6(rs@) && !io.client ==> result == Err(EmError::IoError) && ^self == *self, None => true })]
    #[ensures(match self.regions { Some(rs) => any_dirty6(rs@) && io.client && 16 + plen@ > self.ck_size@ ==>
        result == Err(EmError::CorruptMetadata) && ^self == *self, None => true })]
    #[ensures(match self.regions { Some(rs) => any_dirty6(rs@) && io.client && 16 + plen@ <= self.ck_size@ ==>
        *(^self).log == self.log.concat(ck_evs(*self, plen@, io))
        && (result == Ok(()) == io_ok(io, self.vwc))
        && (result != Ok(()) ==> result == Err(EmError::IoError))
        && (io_data_ok(io, self.vwc) ==> (^self).active@ == 1 - self.active@ && (^self).seq@ == self.seq@ + 1)
        && (!io_data_ok(io, self.vwc) ==> (^self).active == self.active && (^self).seq == self.seq)
        && (io_ok(io, self.vwc) ==> match (^self).regions { Some(rs2) => rs2@.len() == rs@.len() && !any_dirty6(rs2@), None => false })
        && (!io_ok(io, self.vwc) ==> (^self).regions == self.regions), None => true })]
    #[ensures((^self).ck_off == self.ck_off && (^self).ck_size == self.ck_size && (^self).ms == self.ms)]
    #[ensures((^self).hook == self.hook && (^self).vwc == self.vwc)]
    #[ensures(((^self).regions == None) == (self.regions == None))]
    pub fn run6(&mut self, io: Io6, plen: u64) -> Result<(), EmError> {
        // lib.rs:276-290: any region dirty?
        let any_dirty = match &self.regions {
            None => return Err(EmError::NotInitialized),
            Some(rs) => {
                let mut d = false;
                let mut i: usize = 0;
                #[invariant(i@ <= rs@.len())]
                #[invariant(d == exists<j: Int> 0 <= j && j < i@ && rs@[j])]
                while i < rs.len() {
                    if rs[i] {
                        d = true;
                    }
                    i += 1;
                }
                d
            }
        };
        if !any_dirty {
            return Ok(());
        }
        // lib.rs:297: let metadata_client = self.get_metadata_client(ns_id)?;
        if !io.client {
            return Err(EmError::IoError);
        }
        // checkpoint.rs:58-67
        let inactive: u8 = 1 - self.active;
        let region_offset = self.ck_off + inactive as u64 * self.ck_size;
        let region_size = self.ck_size;
        let new_seq = self.seq + 1;
        // checkpoint.rs:69-75
        let total_needed = 16 + plen;
        if total_needed > region_size {
            return Err(EmError::CorruptMetadata);
        }
        // checkpoint.rs:89-93: pad to the metadata sector size
        let aligned_len = (total_needed + self.ms - 1) / self.ms * self.ms;
        // checkpoint.rs:96-97
        let lba = region_offset / self.ms;
        let st = snapshot! { lba@ * self.ms@ };
        self.log = snapshot! { self.log.push_back(Ev6::Data(inactive@, *st, aligned_len@, io.data)) };
        if !io.data {
            return Err(EmError::IoError);
        }
        // checkpoint.rs:101-102
        if self.vwc {
            self.log = snapshot! { self.log.push_back(Ev6::Flush(io.fl1)) };
            if !io.fl1 {
                return Err(EmError::IoError);
            }
        }
        // checkpoint.rs:105-112
        self.active = inactive;
        self.seq = new_seq;
        // lib.rs:302-307
        self.log = snapshot! { self.log.push_back(Ev6::Sb(self.seq@, self.active@, io.sb)) };
        if !io.sb {
            return Err(EmError::IoError);
        }
        // lib.rs:308-310
        if self.vwc {
            self.log = snapshot! { self.log.push_back(Ev6::Flush(io.fl2)) };
            if !io.fl2 {
                return Err(EmError::IoError);
            }
        }
        // lib.rs:312-321: r.dirty = false for every region
        match &mut self.regions {
            Some(rs) => {
                let n = snapshot! { rs@.len() };
                let mut i: usize = 0;
                #[invariant(i@ <= rs@.len() && rs@.len() == *n)]
                #[invariant(forall<j: Int> 0 <= j && j < i@ ==> !rs@[j])]
                while i < rs.len() {
                    rs[i] = false;
                    i += 1;
                }
            }
            None => {}
        }
        // lib.rs:365-367
        if self.hook {
            self.log = snapshot! { self.log.push_back(Ev6::Hook) };
        }
        Ok(())
    }
}

// =============================================================================
// Cs6: checkpoint() coalescing (lib.rs:729-761) with exact steps and return values
// =============================================================================

/// The coalescer plus one program counter per caller thread (batch 4's `Co` / `Th`).
pub struct Cs6 {
    pub co: Co,
    pub ts: Vec<Th>,
}

/// `needed` computed on entry (lib.rs:731-735).
#[logic(open)]
pub fn need_of(co: Co) -> Int {
    pearlite! { if co.in_progress { co.completed_seq@ + 2 } else { co.completed_seq@ + 1 } }
}

impl Cs6 {
    /// One evaluation of the loop lib.rs:737-745 for thread `t` with `needed`; returns true
    /// iff `t` returns `Ok(())` here (lib.rs:738-740).
    #[requires(t@ < self.ts@.len())]
    #[ensures(result == (self.co.completed_seq@ >= needed@))]
    #[ensures(self.co.completed_seq@ >= needed@ ==> (^self).co == self.co && (^self).ts@ == self.ts@.set(t@, Th::Idle))]
    #[ensures(self.co.completed_seq@ < needed@ && !self.co.in_progress ==>
        (^self).co.completed_seq == self.co.completed_seq && (^self).co.in_progress
        && (^self).ts@ == self.ts@.set(t@, Th::Running(needed)))]
    #[ensures(self.co.completed_seq@ < needed@ && self.co.in_progress ==>
        (^self).co == self.co && (^self).ts@ == self.ts@.set(t@, Th::Waiting(needed)))]
    pub fn check6(&mut self, t: usize, needed: u64) -> bool {
        if self.co.completed_seq >= needed {
            self.ts[t] = Th::Idle;
            return true;
        }
        if !self.co.in_progress {
            self.co.in_progress = true;
            self.ts[t] = Th::Running(needed);
            return false;
        }
        self.ts[t] = Th::Waiting(needed);
        false
    }

    /// `checkpoint()` called by idle thread `t` (lib.rs:730-735, first loop iteration).
    #[requires(t@ < self.ts@.len() && self.co.completed_seq@ + 2 <= u64::MAX@)]
    #[ensures(result == (self.co.completed_seq@ >= need_of(self.co)))]
    #[ensures((^self).co.completed_seq == self.co.completed_seq && (^self).ts@.len() == self.ts@.len())]
    #[ensures(forall<j: Int> 0 <= j && j < self.ts@.len() && j != t@ ==> (^self).ts@[j] == self.ts@[j])]
    #[ensures(!self.co.in_progress ==> (^self).co.in_progress && match (^self).ts@[t@] { Th::Running(n) => n@ == need_of(self.co), _ => false })]
    #[ensures(self.co.in_progress ==> (^self).co == self.co && match (^self).ts@[t@] { Th::Waiting(n) => n@ == need_of(self.co), _ => false })]
    pub fn enter6(&mut self, t: usize) -> bool {
        let needed = if self.co.in_progress { self.co.completed_seq + 2 } else { self.co.completed_seq + 1 };
        self.check6(t, needed)
    }

    /// Waiting thread `t` re-acquires the lock after `wait` returns (lib.rs:744).
    #[requires(t@ < self.ts@.len())]
    #[ensures(match self.ts@[t@] {
        Th::Waiting(n) => result == (self.co.completed_seq@ >= n@)
            && (self.co.completed_seq@ >= n@ ==> (^self).co == self.co && (^self).ts@ == self.ts@.set(t@, Th::Idle))
            && (self.co.completed_seq@ < n@ && !self.co.in_progress ==> (^self).co.completed_seq == self.co.completed_seq
                && (^self).co.in_progress && (^self).ts@ == self.ts@.set(t@, Th::Running(n)))
            && (self.co.completed_seq@ < n@ && self.co.in_progress ==> (^self).co == self.co && (^self).ts@ == self.ts@.set(t@, Th::Waiting(n))),
        _ => !result && ^self == *self,
    })]
    pub fn wake6(&mut self, t: usize) -> bool {
        match self.ts[t] {
            Th::Waiting(n) => self.check6(t, n),
            _ => false,
        }
    }

    /// `run_checkpoint()` returned in running thread `t` (`ok` = success): lib.rs:752-758.
    #[requires(t@ < self.ts@.len())]
    #[ensures(match self.ts@[t@] {
        Th::Running(n) => (^self).co.completed_seq == (if ok { n } else { self.co.completed_seq })
            && !(^self).co.in_progress && (^self).ts@ == self.ts@.set(t@, Th::Idle),
        _ => ^self == *self,
    })]
    pub fn finish6(&mut self, t: usize, ok: bool) {
        match self.ts[t] {
            Th::Running(n) => {
                if ok {
                    self.co.completed_seq = n;
                }
                self.co.in_progress = false;
                self.ts[t] = Th::Idle;
            }
            _ => {}
        }
    }
}

// =============================================================================
// format(): checks, device queries and instance id (lib.rs:383-481)
// =============================================================================

/// The FR-002 parameter checks of lib.rs:384-401 all pass.
#[logic(open)]
pub fn fr002(p: FormatParams) -> bool {
    pearlite! {
        p.sector_size@ > 0 && p.slab_size@ % p.sector_size@ == 0 && p.max_extent_size@ <= p.slab_size@
        && p.region_count != 0u32 && (p.region_count & (p.region_count - 1u32)) == 0u32
    }
}

/// `format` (lib.rs:383-481) up to the instance id: parameter checks, receptacle, device
/// queries `qn` / `qs` (the device's answers to `num_sectors` / `sector_size`, `None` = the call
/// failed), checkpoint layout, data space, instance id (`rnd` = the 8 bytes read from
/// /dev/urandom, `None` = open/read failed). Region construction (lib.rs:457-468) is pure and
/// local and is not repeated here (component.rs `format_build`). Takes `&` — nothing changes.
/// LEVEL-3 (phase D-B2): the two format ranges are required only on the path that reaches their
/// arithmetic (lib.rs:410-424: every parameter check passed, the device is connected and answered).
#[requires(match (qn, qs) { (Some(n), Some(s)) => fr002(params) && dev.connected ==>
    a_67ea4f(n@, s@) && a_2ee144(params.metadata_alignment@), _ => true })]
#[ensures(!fr002(params) ==> result == Err(EmError::CorruptMetadata))]
#[ensures(fr002(params) && !dev.connected ==> result == Err(EmError::NotInitialized))]
#[ensures(fr002(params) && dev.connected && (qn == None || qs == None) ==> result == Err(EmError::IoError))]
#[ensures(forall<n: u64, s: u32> fr002(params) && dev.connected && qn == Some(n) && qs == Some(s)
    && cksize_l(n@ * s@, params.metadata_region_size@, params.metadata_alignment@, params.sector_size@) == 0
    ==> result == Err(EmError::CorruptMetadata))]
#[ensures(forall<n: u64, s: u32> fr002(params) && dev.connected && qn == Some(n) && qs == Some(s)
    && cksize_l(n@ * s@, params.metadata_region_size@, params.metadata_alignment@, params.sector_size@) > 0
    && params.data_disk_size@ <= ds_l(n@ * s@, params.metadata_region_size@, params.metadata_alignment@, params.sector_size@)
    ==> result == Err(EmError::CorruptMetadata))]
#[ensures(forall<n: u64, s: u32> fr002(params) && dev.connected && qn == Some(n) && qs == Some(s)
    && cksize_l(n@ * s@, params.metadata_region_size@, params.metadata_alignment@, params.sector_size@) > 0
    && params.data_disk_size@ > ds_l(n@ * s@, params.metadata_region_size@, params.metadata_alignment@, params.sector_size@)
    ==> match params.instance_id {
        Some(id) => result == Ok(id),
        None => match rnd { Some(v) => result == Ok(v), None => result == Err(EmError::IoError) },
    })]
pub fn format_pre6(dev: &MetaDevice, params: FormatParams, qn: Option<u64>, qs: Option<u32>, rnd: Option<u64>) -> Result<u64, EmError> {
    if params.sector_size == 0 {
        return Err(EmError::CorruptMetadata);
    }
    if params.slab_size % params.sector_size as u64 != 0 {
        return Err(EmError::CorruptMetadata);
    }
    if params.max_extent_size as u64 > params.slab_size {
        return Err(EmError::CorruptMetadata);
    }
    if params.region_count == 0 || !params.region_count.is_power_of_two() {
        return Err(EmError::CorruptMetadata);
    }
    let data_disk_size = params.data_disk_size;
    // lib.rs:406-409: self.metadata_device.get().map_err(.. not_initialized ..)?
    if !dev.connected {
        return Err(EmError::NotInitialized);
    }
    // lib.rs:410-415: num_sectors(..).map_err(nvme_to_em)? * sector_size(..).map_err(nvme_to_em)?
    let n = match qn {
        Some(n) => n,
        None => return Err(EmError::IoError),
    };
    let s = match qs {
        Some(s) => s,
        None => return Err(EmError::IoError),
    };
    let metadata_disk_size = n * s as u64;
    // lib.rs:418-438
    let alignment = params.metadata_alignment;
    let sb_size = SUPERBLOCK_SIZE as u64;
    let checkpoint_region_offset = if alignment == 0 { sb_size } else { (sb_size + alignment - 1) / alignment * alignment /* lib.rs:423, same evaluation order (debug overflow semantics; 2ee144) */ };
    let effective_metadata_size = if params.metadata_region_size > 0 {
        if metadata_disk_size <= params.metadata_region_size { metadata_disk_size } else { params.metadata_region_size }
    } else {
        metadata_disk_size
    };
    let remaining = if effective_metadata_size >= checkpoint_region_offset {
        effective_metadata_size - checkpoint_region_offset
    } else {
        0
    }; // saturating_sub (lib.rs:430)
    let sector_size_u64 = params.sector_size as u64;
    let checkpoint_region_size = (remaining / 2) / sector_size_u64 * sector_size_u64;
    proof_assert! { lemma_divmod(remaining@ / 2, sector_size_u64@); checkpoint_region_size@ <= remaining@ / 2 };
    proof_assert! { checkpoint_region_offset@ == ckoff_l(params.metadata_alignment@) };
    proof_assert! { effective_metadata_size@ == eff_l(metadata_disk_size@, params.metadata_region_size@) };
    proof_assert! { checkpoint_region_size@ == cksize_l(metadata_disk_size@, params.metadata_region_size@, params.metadata_alignment@, params.sector_size@) };
    proof_assert! { checkpoint_region_size@ > 0 ==> checkpoint_region_offset@ + 2 * checkpoint_region_size@ <= effective_metadata_size@ };
    if checkpoint_region_size == 0 {
        return Err(EmError::CorruptMetadata);
    }
    // lib.rs:442-452
    let data_start_offset = if params.metadata_region_size > 0 { checkpoint_region_offset + 2 * checkpoint_region_size } else { 0 };
    proof_assert! { data_start_offset@ == ds_l(metadata_disk_size@, params.metadata_region_size@, params.metadata_alignment@, params.sector_size@) };
    let usable_data_size = if data_disk_size >= data_start_offset { data_disk_size - data_start_offset } else { 0 };
    if usable_data_size == 0 {
        return Err(EmError::CorruptMetadata);
    }
    // lib.rs:471-481
    match params.instance_id {
        Some(id) => Ok(id),
        None => match rnd {
            Some(v) => Ok(v),
            None => Err(EmError::IoError),
        },
    }
}

/// LEVEL-3: every check of lib.rs:383-452 passes (FR-002, device bound and answering, a
/// non-empty checkpoint region, usable data space) — the inputs on which format goes on to
/// build regions (where `sane_params` is needed).
#[logic(open)]
pub fn fmt6_passes(dev: MetaDevice, p: FormatParams, qn: Option<u64>, qs: Option<u32>) -> bool {
    pearlite! {
        fr002(p) && dev.connected && match (qn, qs) {
            (Some(n), Some(s)) =>
                cksize_l(n@ * s@, p.metadata_region_size@, p.metadata_alignment@, p.sector_size@) > 0
                && p.data_disk_size@ > ds_l(n@ * s@, p.metadata_region_size@, p.metadata_alignment@, p.sector_size@),
            _ => false,
        }
    }
}

impl ExtentManager {
    /// Mirror of `IExtentManager::format` (lib.rs:383-511): `format_pre6` (lib.rs:383-481,
    /// which changes nothing), then — with the instance id fixed — component.rs's `format`
    /// mirror for region construction, the superblock write and the two publishes.
    /// LEVEL-3: `sane_params` is required only on inputs that pass every early check
    /// (`fmt6_passes`) — the error paths of lib.rs:383-452 never read the slab geometry.
    /// LEVEL-3 (phase D-B2): every range is required only on the path that reaches it — the
    /// device-size / alignment ranges once the parameter checks pass and the device answered, the
    /// format-parameter ranges ([`fmt_sane`], no 7cb7c3) and the receptacle's own size only on inputs
    /// that pass every early check.
    #[requires(em_regions_wf(*self))]
    #[requires(match (qn, qs) { (Some(n), Some(s)) => fr002(params) && self.dev.connected ==>
        a_67ea4f(n@, s@) && a_2ee144(params.metadata_alignment@), _ => true })]
    #[requires(fmt6_passes(self.dev, params, qn, qs) ==> fmt_sane(params) && a_67ea4f(self.dev.num_sectors@, self.dev.sector_size@))]
    #[ensures(!fr002(params) ==> result == Err(EmError::CorruptMetadata) && ^self == *self)]
    #[ensures(fr002(params) && !self.dev.connected ==> result == Err(EmError::NotInitialized) && ^self == *self)]
    #[ensures(fr002(params) && self.dev.connected && (qn == None || qs == None) ==> result == Err(EmError::IoError) && ^self == *self)]
    #[ensures(forall<n: u64, s: u32> fr002(params) && self.dev.connected && qn == Some(n) && qs == Some(s)
        && cksize_l(n@ * s@, params.metadata_region_size@, params.metadata_alignment@, params.sector_size@) == 0
        ==> result == Err(EmError::CorruptMetadata) && ^self == *self)]
    #[ensures(forall<n: u64, s: u32> fr002(params) && self.dev.connected && qn == Some(n) && qs == Some(s)
        && cksize_l(n@ * s@, params.metadata_region_size@, params.metadata_alignment@, params.sector_size@) > 0
        && params.data_disk_size@ <= ds_l(n@ * s@, params.metadata_region_size@, params.metadata_alignment@, params.sector_size@)
        ==> result == Err(EmError::CorruptMetadata) && ^self == *self)]
    #[ensures(forall<n: u64, s: u32> fr002(params) && self.dev.connected && qn == Some(n) && qs == Some(s)
        && cksize_l(n@ * s@, params.metadata_region_size@, params.metadata_alignment@, params.sector_size@) > 0
        && params.data_disk_size@ > ds_l(n@ * s@, params.metadata_region_size@, params.metadata_alignment@, params.sector_size@)
        && params.instance_id == None && rnd == None
        ==> result == Err(EmError::IoError) && ^self == *self)]
    pub fn format6(&mut self, params: FormatParams, qn: Option<u64>, qs: Option<u32>, rnd: Option<u64>) -> Result<(), EmError> {
        let id = match format_pre6(&self.dev, params, qn, qs, rnd) {
            Ok(id) => id,
            Err(e) => return Err(e),
        };
        proof_assert! { fmt6_passes(self.dev, params, qn, qs) && fmt_sane(params) };
        proof_assert! { a_2ee144(params.metadata_alignment@) };
        let p2 = FormatParams { instance_id: Some(id), ..params };
        self.format(p2)
    }
}

/// lib.rs:420-424 in a RELEASE build (overflow checks off: `sb_size + alignment - 1` wraps).
/// Returns (the wrapped sum, checkpoint_region_offset).
#[requires(alignment@ > 0)]
#[ensures(result.0 == 4096u64 + (alignment - 1u64))]
#[ensures(result.1@ == result.0@ / alignment@ * alignment@)]
pub fn ckoff_release(alignment: u64) -> (u64, u64) {
    let sb_size = SUPERBLOCK_SIZE as u64;
    let w = sb_size.wrapping_add(alignment - 1);
    proof_assert! { lemma_divmod(w@, alignment@); w@ / alignment@ * alignment@ <= w@ };
    (w, w / alignment * alignment)
}

// =============================================================================
// Exact listing: get_extents / for_each_extent produce the extents in iteration order
// =============================================================================

/// The iteration order of a `BTreeMap<u64, Slab>` (ascending keys in std): a function of the
/// map's contents only.
#[logic(opaque)]
pub fn key_seq(m: FMap<Int, Slab>) -> Seq<u64> {
    dead
}

impl SlabMap {
    /// TRUSTED std leaf: `self.slabs.values()` / `.keys()` visit the keys in the order
    /// `key_seq(self@)` — the same order on every call for the same contents (std: ascending).
    #[trusted]
    #[ensures(result@ == key_seq(self@))]
    pub fn keys_det(&self) -> Vec<u64> {
        self.inner.keys().copied().collect()
    }
}

/// An extent as (key, offset, size).
#[logic(open)]
pub fn tri(e: Extent) -> (u64, Int, u32) {
    pearlite! { (e.key, e.offset@, e.size) }
}

/// The extents of slots `0..n` of slab `sl`, in slot order (lib.rs:641-654 inner loop).
#[logic(open)]
#[variant(n)]
pub fn slab_tris(sl: Slab, n: Int) -> Seq<(u64, Int, u32)> {
    pearlite! {
        if n <= 0 { Seq::empty() } else {
            if sl.keys@[n - 1] != FREE_KEY { slab_tris(sl, n - 1).push_back((sl.keys@[n - 1], slot_off(sl, n - 1), sl.element_size)) }
            else { slab_tris(sl, n - 1) }
        }
    }
}

/// The extents of the slabs `ks[0..a]` of map `m`.
#[logic(open)]
#[variant(a)]
pub fn keys_tris(m: FMap<Int, Slab>, ks: Seq<u64>, a: Int) -> Seq<(u64, Int, u32)> {
    pearlite! {
        if a <= 0 { Seq::empty() } else {
            keys_tris(m, ks, a - 1).concat(
                if m.contains(ks[a - 1]@) { slab_tris(m.lookup(ks[a - 1]@), m.lookup(ks[a - 1]@).bitmap.num_slots@) } else { Seq::empty() })
        }
    }
}

/// All extents of region `r`, in iteration order.
#[logic(open)]
pub fn region_tris(r: RegionState) -> Seq<(u64, Int, u32)> {
    pearlite! { keys_tris(r.slabs@, key_seq(r.slabs@), key_seq(r.slabs@).len()) }
}

/// All extents of the current regions `rv[0..n]`, in iteration order.
#[logic(open)]
#[variant(n)]
pub fn em_tris(arena: Seq<RegionState>, rv: Seq<usize>, n: Int) -> Seq<(u64, Int, u32)> {
    pearlite! { if n <= 0 { Seq::empty() } else { em_tris(arena, rv, n - 1).concat(region_tris(arena[rv[n - 1]@])) } }
}

/// `v[lo..]` is exactly the sequence `s`.
#[logic(open)]
pub fn matches(v: Seq<Extent>, lo: Int, s: Seq<(u64, Int, u32)>) -> bool {
    pearlite! { v.len() == lo + s.len() && forall<q: Int> lo <= q && q < v.len() ==> tri(v[q]) == s[q - lo] }
}

/// Two exact listings of the same sequence are equal element by element.
#[logic]
#[requires(matches(a, la, s) && matches(b, lb, s))]
#[ensures(a.len() - la == b.len() - lb)]
#[ensures(forall<p: Int> 0 <= p && p < s.len() ==> a[la + p] == b[lb + p])]
pub fn lemma_matches_eq(a: Seq<Extent>, la: Int, b: Seq<Extent>, lb: Int, s: Seq<(u64, Int, u32)>) {}

/// Extending a match by a concatenated tail.
#[logic]
#[requires(0 <= lo && matches(a, lo, s) && matches(b, a.len(), t))]
#[requires(forall<p: Int> 0 <= p && p < a.len() ==> b[p] == a[p])]
#[ensures(matches(b, lo, s.concat(t)))]
pub fn lemma_matches_concat(a: Seq<Extent>, b: Seq<Extent>, lo: Int, s: Seq<(u64, Int, u32)>, t: Seq<(u64, Int, u32)>) {}

/// The listing loops over one region's slabs, exact version of listing.rs `region_extents`.
#[requires(rg_list_ok(*r))]
#[ensures(forall<p: Int> 0 <= p && p < out@.len() ==> (^out)@[p] == out@[p])]
#[ensures(matches((^out)@, out@.len(), region_tris(*r)))]
pub fn region_extents6(r: &RegionState, out: &mut Vec<Extent>) {
    let ks = r.slabs.keys_det();
    let o0 = snapshot! { *out };
    let mut a: usize = 0;
    #[invariant(a@ <= ks@.len())]
    #[invariant(forall<p: Int> 0 <= p && p < o0@.len() ==> out@[p] == o0@[p])]
    #[invariant(matches(out@, o0@.len(), keys_tris(r.slabs@, ks@, a@)))]
    while a < ks.len() {
        let k = ks[a];
        let out_a = snapshot! { *out };
        match r.slabs.get(k) {
            Some(slab) => {
                proof_assert! { r.slabs@.contains(k@) && r.slabs@.lookup(k@) == *slab };
                proof_assert! { slab_inv(*slab) };
                let n = slab.num_slots() as usize;
                let mut i: usize = 0;
                #[invariant(i@ <= n@ && n@ == slab.bitmap.num_slots@)]
                #[invariant(forall<p: Int> 0 <= p && p < out_a@.len() ==> out@[p] == out_a@[p])]
                #[invariant(matches(out@, out_a@.len(), slab_tris(*slab, i@)))]
                while i < n {
                    let key = slab.get_key(i);
                    if key != FREE_KEY {
                        proof_assert! { lemma_slot_inside(*slab, i@); slot_off(*slab, i@) <= u64::MAX@ };
                        let e = Extent { key, offset: slab.slot_offset(i), size: slab.element_size };
                        let before = snapshot! { *out };
                        out.push(e);
                        proof_assert! { tri(e) == (slab.keys@[i@], slot_off(*slab, i@), slab.element_size) };
                        proof_assert! { slab_tris(*slab, i@ + 1) == slab_tris(*slab, i@).push_back(tri(e)) };
                        proof_assert! { forall<p: Int> 0 <= p && p < before@.len() ==> out@[p] == before@[p] };
                    } else {
                        proof_assert! { slab_tris(*slab, i@ + 1) == slab_tris(*slab, i@) };
                    }
                    i += 1;
                }
                proof_assert! { keys_tris(r.slabs@, ks@, a@ + 1) == keys_tris(r.slabs@, ks@, a@).concat(slab_tris(*slab, n@)) };
                proof_assert! { lemma_matches_concat(out_a@, out@, o0@.len(), keys_tris(r.slabs@, ks@, a@), slab_tris(*slab, n@));
                    matches(out@, o0@.len(), keys_tris(r.slabs@, ks@, a@ + 1)) };
            }
            None => {
                proof_assert! { !r.slabs@.contains(k@) };
                proof_assert! { keys_tris(r.slabs@, ks@, a@ + 1).ext_eq(keys_tris(r.slabs@, ks@, a@)) };
            }
        }
        a += 1;
    }
}

impl ExtentManager {
    /// `get_extents` (lib.rs:632-656), exact version: the result is the extents of every
    /// current region in iteration order.
    #[requires(em_regions_wf(*self) && em_list_ok(*self))]
    #[ensures(match self.regions { None => result@.len() == 0, Some(rv) => matches(result@, 0, em_tris(self.arena@, rv@, rv@.len())) })]
    pub fn get6(&self) -> Vec<Extent> {
        let mut result: Vec<Extent> = Vec::new();
        match &self.regions {
            Some(rv) => {
                let mut i: usize = 0;
                #[invariant(i@ <= rv@.len())]
                #[invariant(matches(result@, 0, em_tris(self.arena@, rv@, i@)))]
                while i < rv.len() {
                    let before = snapshot! { result };
                    proof_assert! { rg_list_ok(self.arena@[rv@[i@]@]) };
                    region_extents6(&self.arena[rv[i]], &mut result);
                    proof_assert! { lemma_matches_concat(before@, result@, 0, em_tris(self.arena@, rv@, i@), region_tris(self.arena@[rv@[i@]@]));
                        matches(result@, 0, em_tris(self.arena@, rv@, i@ + 1)) };
                    i += 1;
                }
                result
            }
            None => result,
        }
    }

    /// `for_each_extent` (lib.rs:658-678), exact version; `cb` records what the callback is
    /// passed, in call order.
    #[requires(em_regions_wf(*self) && em_list_ok(*self))]
    #[ensures(forall<p: Int> 0 <= p && p < cb@.len() ==> (^cb)@[p] == cb@[p])]
    #[ensures(match self.regions { None => (^cb)@ == cb@, Some(rv) => matches((^cb)@, cb@.len(), em_tris(self.arena@, rv@, rv@.len())) })]
    pub fn for_each6(&self, cb: &mut Vec<Extent>) {
        let c0 = snapshot! { *cb };
        match &self.regions {
            Some(rv) => {
                let mut i: usize = 0;
                #[invariant(i@ <= rv@.len())]
                #[invariant(forall<p: Int> 0 <= p && p < c0@.len() ==> cb@[p] == c0@[p])]
                #[invariant(matches(cb@, c0@.len(), em_tris(self.arena@, rv@, i@)))]
                while i < rv.len() {
                    let before = snapshot! { *cb };
                    proof_assert! { rg_list_ok(self.arena@[rv@[i@]@]) };
                    region_extents6(&self.arena[rv[i]], cb);
                    proof_assert! { lemma_matches_concat(before@, cb@, c0@.len(), em_tris(self.arena@, rv@, i@), region_tris(self.arena@[rv@[i@]@]));
                        matches(cb@, c0@.len(), em_tris(self.arena@, rv@, i@ + 1)) };
                    i += 1;
                }
            }
            None => {}
        }
    }
}
