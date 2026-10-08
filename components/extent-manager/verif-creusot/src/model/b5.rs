//! Batch-5 model additions (all line numbers cite the PIN 2cd35bac):
//! * [`TSys`] — the background checkpoint TIMER thread (lib.rs:120-163) against
//!   `CheckpointTimerState::set_interval` (lib.rs:70-73), one atomic step per critical
//!   section of `interval: Mutex<Option<Duration>>` plus the std `Condvar::wait_timeout` /
//!   `wait` events the std documentation allows (a timeout, a notification, a spurious
//!   wake-up; a `notify_one` with no thread blocked in the wait is lost). Durations are
//!   abstract ticks (u64); `now` is a ghost monotonic clock advanced by `tick`.
//! * [`serialize_region`] — `checkpoint::serialize_region` (checkpoint.rs:15-33).
//! * [`read_ckpt_raw`] — `read_checkpoint_region` (checkpoint.rs:117-173) with the two
//!   `read_blocks` results supplied as arbitrary byte vectors (any length), so the
//!   defensive length checks (checkpoint.rs:128-130, 158-160) are reachable in the model.
//! * `ExtentManager::set_metadata_ns_id` (lib.rs:181-183) and `init_ns`, initialize's
//!   namespace choice (lib.rs:517).
use crate::model::b4::*;
use crate::model::component::*;
use crate::model::listing::*;
use crate::model::params::*;
use crate::model::region::*;
use crate::model::sbbytes::*;
use crate::model::slab::*;
use crate::model::assume::*;
use creusot_std::prelude::*;

// =============================================================================
// Background checkpoint timer (lib.rs:120-163, 70-73)
// =============================================================================

/// Where the timer thread is (lib.rs:121-162).
#[derive(::core::clone::Clone, ::core::marker::Copy)]
pub enum TPh {
    /// Loop head: about to lock `interval` (lib.rs:128).
    Top,
    /// Blocked in `wait_timeout(guard, dur)` with deadline `start + dur` (lib.rs:134); level 3:
    /// an unbounded ghost Int (std computes the deadline as an `Instant`, no u64 tick bound).
    Wait(Snapshot<Int>),
    /// Blocked in `wait(guard)` (interval None, lib.rs:137-138).
    WaitInf,
    /// The wait has ended (`true` = by timeout); the thread is re-acquiring the mutex.
    Woken(bool),
    /// `do_checkpoint` was true: running `this.checkpoint()` (lib.rs:152-159).
    Work,
    /// Left the loop (shutdown / component dropped).
    Exited,
}

/// The timer state, the timer thread, and a ghost clock / checkpoint counter.
pub struct TSys {
    pub interval: Option<u64>,
    /// Ghost monotonic clock (unbounded Int; level 3 widened from a u64 tick counter).
    pub now: Snapshot<Int>,
    pub ph: TPh,
    pub shutdown: bool,
    pub ckpts: u64,
}

impl TSys {
    /// lib.rs:128-142: lock `interval`, check shutdown, read the interval and enter the wait
    /// (the guard is released atomically by the wait).
    #[requires(self.ph == TPh::Top)]
    #[ensures((^self).interval == self.interval && (^self).now == self.now)]
    #[ensures((^self).shutdown == self.shutdown && (^self).ckpts == self.ckpts)]
    #[ensures(self.shutdown ==> (^self).ph == TPh::Exited)]
    #[ensures(!self.shutdown ==> match self.interval {
        Some(d) => match (^self).ph { TPh::Wait(dl) => *dl == *self.now + d@, _ => false },
        None => (^self).ph == TPh::WaitInf,
    })]
    pub fn th_enter(&mut self) {
        if self.shutdown {
            self.ph = TPh::Exited;
            return;
        }
        match self.interval {
            Some(d) => self.ph = TPh::Wait(snapshot! { *self.now + d@ }),
            None => self.ph = TPh::WaitInf,
        }
    }

    /// Time passes.
    #[ensures(*(^self).now == *self.now + dt@)]
    #[ensures((^self).interval == self.interval && (^self).ph == self.ph)]
    #[ensures((^self).shutdown == self.shutdown && (^self).ckpts == self.ckpts)]
    pub fn tick(&mut self, dt: u64) {
        self.now = snapshot! { *self.now + dt@ };
    }

    /// `wait_timeout` reaches its deadline: the wait ends with `timed_out() == true`.
    #[requires(match self.ph { TPh::Wait(dl) => *self.now >= *dl, _ => false })]
    #[ensures((^self).ph == TPh::Woken(true))]
    #[ensures((^self).interval == self.interval && (^self).now == self.now)]
    #[ensures((^self).shutdown == self.shutdown && (^self).ckpts == self.ckpts)]
    pub fn timeout(&mut self) {
        self.ph = TPh::Woken(true);
    }

    /// A spurious wake-up (permitted by the std `Condvar` documentation for `wait` and
    /// `wait_timeout`): the wait ends with `timed_out() == false`.
    #[requires(match self.ph { TPh::Wait(_) => true, TPh::WaitInf => true, _ => false })]
    #[ensures((^self).ph == TPh::Woken(false))]
    #[ensures((^self).interval == self.interval && (^self).now == self.now)]
    #[ensures((^self).shutdown == self.shutdown && (^self).ckpts == self.ckpts)]
    pub fn spurious(&mut self) {
        self.ph = TPh::Woken(false);
    }

    /// `set_checkpoint_interval(iv)` (lib.rs:694-696 -> 70-73): under the mutex store the
    /// interval, then `notify_one`: a thread blocked in the wait wakes (not timed out); a
    /// notification with no thread blocked in the wait is lost.
    #[ensures((^self).interval == iv && (^self).now == self.now)]
    #[ensures((^self).shutdown == self.shutdown && (^self).ckpts == self.ckpts)]
    #[ensures((^self).ph == match self.ph { TPh::Wait(_) => TPh::Woken(false), TPh::WaitInf => TPh::Woken(false), p => p })]
    pub fn set_interval(&mut self, iv: Option<u64>) {
        self.interval = iv;
        match self.ph {
            TPh::Wait(_) => self.ph = TPh::Woken(false),
            TPh::WaitInf => self.ph = TPh::Woken(false),
            _ => {}
        }
    }

    /// The wait returns with the mutex re-acquired; `do_checkpoint = result.timed_out()`
    /// (lib.rs:134-135) or `false` (lib.rs:138-139); the guard is dropped (lib.rs:142);
    /// shutdown check (lib.rs:144-146); `if !do_checkpoint { continue }` (lib.rs:147-149).
    /// The interval is NOT re-read.
    #[requires(match self.ph { TPh::Woken(_) => true, _ => false })]
    #[ensures((^self).interval == self.interval && (^self).now == self.now)]
    #[ensures((^self).shutdown == self.shutdown && (^self).ckpts == self.ckpts)]
    #[ensures((^self).ph == if self.shutdown { TPh::Exited } else {
        match self.ph { TPh::Woken(t) => if t { TPh::Work } else { TPh::Top }, p => p } })]
    pub fn relock(&mut self) {
        let t = match self.ph {
            TPh::Woken(t) => t,
            _ => false,
        };
        if self.shutdown {
            self.ph = TPh::Exited;
            return;
        }
        if t {
            self.ph = TPh::Work;
        } else {
            self.ph = TPh::Top;
        }
    }

    /// lib.rs:151-161: `weak.upgrade()` succeeds and `this.checkpoint()` runs once.
    #[requires(self.ph == TPh::Work && self.ckpts@ < u64::MAX@)]
    #[ensures((^self).ckpts@ == self.ckpts@ + 1 && (^self).ph == TPh::Top)]
    #[ensures((^self).interval == self.interval && (^self).now == self.now && (^self).shutdown == self.shutdown)]
    pub fn work(&mut self) {
        self.ckpts += 1;
        self.ph = TPh::Top;
    }
}

// =============================================================================
// checkpoint::serialize_region (checkpoint.rs:15-33)
// =============================================================================

/// Region `r` holds exactly one slab, keyed `k`.
#[logic(open)]
pub fn one_key(r: RegionState, k: Int) -> bool {
    pearlite! { r.slabs@.contains(k) && forall<t: Int> r.slabs@.contains(t) ==> t == k }
}

/// Mirror of `serialize_region` (checkpoint.rs:15-33). `region.slabs.len()` is the number of
/// keys the BTreeMap iteration visits (the trusted leaf `SlabMap::keys_vec`, each key exactly
/// once); `to_le_bytes` / `extend_from_slice` are the trusted `to_le_bytes64/32` and the native
/// `append`. Specified for a region with ONE slab: the exact byte layout of its header
/// (slab count, start, slab size, element size, slot count) and the total length.
#[requires(rg_inv(*region))]
#[requires(exists<k: Int> one_key(*region, k))]
#[ensures(forall<k: Int> one_key(*region, k) ==>
    result@.len() == 28 + 8 * region.slabs@.lookup(k).bitmap.num_slots@
    && (forall<j: Int> 0 <= j && j < 4 ==> result@[j] == le32(1u32, j))
    && (forall<j: Int> 0 <= j && j < 8 ==> result@[4 + j] == le64(region.slabs@.lookup(k).start_offset, j))
    && (forall<j: Int> 0 <= j && j < 8 ==> result@[12 + j] == le64(region.slabs@.lookup(k).slab_size, j))
    && (forall<j: Int> 0 <= j && j < 4 ==> result@[20 + j] == le32(region.slabs@.lookup(k).element_size, j))
    && (forall<j: Int> 0 <= j && j < 4 ==> result@[24 + j] == le32(region.slabs@.lookup(k).bitmap.num_slots, j)))]
pub fn serialize_region(region: &RegionState) -> Vec<u8> {
    let mut data: Vec<u8> = Vec::new();
    let ks = region.slabs.keys_vec();
    let num_slabs = as_u32(ks.len() as u64);
    let hdr = to_le_bytes32(num_slabs);
    append(&mut data, &hdr);
    let start = snapshot! { data@ };
    proof_assert! { forall<k: Int> one_key(*region, k) ==> forall<i: Int> 0 <= i && i < ks@.len() ==> ks@[i]@ == k };
    proof_assert! { forall<k: Int> one_key(*region, k) ==> ks@.len() >= 1 };
    proof_assert! { forall<k: Int> one_key(*region, k) ==> ks@.len() >= 2 ==> ks@[0]@ == ks@[1]@ && ks@[0] != ks@[1] };
    proof_assert! { forall<k: Int> one_key(*region, k) ==> ks@.len() == 1 && ks@[0]@ == k };
    let mut p: usize = 0;
    #[invariant(p@ <= ks@.len())]
    #[invariant(data@.len() >= 4 && forall<j: Int> 0 <= j && j < 4 ==> data@[j] == (*start)[j])]
    #[invariant(p@ == 0 ==> data@.len() == 4)]
    #[invariant(forall<k: Int> one_key(*region, k) && p@ == 1 ==>
        data@.len() == 28 + 8 * region.slabs@.lookup(k).bitmap.num_slots@
        && (forall<j: Int> 0 <= j && j < 8 ==> data@[4 + j] == le64(region.slabs@.lookup(k).start_offset, j))
        && (forall<j: Int> 0 <= j && j < 8 ==> data@[12 + j] == le64(region.slabs@.lookup(k).slab_size, j))
        && (forall<j: Int> 0 <= j && j < 4 ==> data@[20 + j] == le32(region.slabs@.lookup(k).element_size, j))
        && (forall<j: Int> 0 <= j && j < 4 ==> data@[24 + j] == le32(region.slabs@.lookup(k).bitmap.num_slots, j)))]
    #[invariant(data@.len() <= 4 + p@ * (24 + 8 * 4294967296))]
    while p < ks.len() {
        let slab = match region.slabs.get(ks[p]) {
            Some(sl) => sl,
            None => {
                p += 1;
                continue;
            } // unreachable: every visited key is in the map
        };
        let num_slots = slab.num_slots();
        proof_assert! { slab_inv(*slab) && slab.keys@.len() == num_slots@ };
        let d0 = snapshot! { data@ };
        let b1 = to_le_bytes64(slab.start_offset);
        append(&mut data, &b1);
        let da = snapshot! { data@ };
        proof_assert! { forall<j: Int> 0 <= j && j < 8 ==> (*da)[d0.len() + j] == le64(slab.start_offset, j) };
        let b2 = to_le_bytes64(slab.slab_size);
        append(&mut data, &b2);
        let db = snapshot! { data@ };
        proof_assert! { forall<j: Int> 0 <= j && j < 8 ==> (*db)[d0.len() + 8 + j] == le64(slab.slab_size, j) };
        proof_assert! { forall<j: Int> 0 <= j && j < da.len() ==> (*db)[j] == (*da)[j] };
        let b3 = to_le_bytes32(slab.element_size);
        append(&mut data, &b3);
        let dc = snapshot! { data@ };
        proof_assert! { forall<j: Int> 0 <= j && j < 4 ==> (*dc)[d0.len() + 16 + j] == le32(slab.element_size, j) };
        proof_assert! { forall<j: Int> 0 <= j && j < db.len() ==> (*dc)[j] == (*db)[j] };
        let b4 = to_le_bytes32(num_slots);
        append(&mut data, &b4);
        let d1 = snapshot! { data@ };
        proof_assert! { forall<j: Int> 0 <= j && j < 4 ==> (*d1)[d0.len() + 20 + j] == le32(num_slots, j) };
        proof_assert! { forall<j: Int> 0 <= j && j < dc.len() ==> (*d1)[j] == (*dc)[j] };
        proof_assert! { d1.len() == d0.len() + 24 };
        proof_assert! { forall<j: Int> 0 <= j && j < d0.len() ==> (*d1)[j] == (*d0)[j] };
        let mut i: usize = 0;
        #[invariant(i@ <= num_slots@)]
        #[invariant(data@.len() == d1.len() + 8 * i@)]
        #[invariant(forall<j: Int> 0 <= j && j < d1.len() ==> data@[j] == (*d1)[j])]
        while i < num_slots as usize {
            proof_assert! { slab_inv(*slab) && i@ < slab.keys@.len() };
            let kb = to_le_bytes64(slab.get_key(i));
            append(&mut data, &kb);
            i += 1;
        }
        p += 1;
    }
    data
}

// =============================================================================
// read_checkpoint_region with arbitrary read results (checkpoint.rs:117-173)
// =============================================================================

/// The bytes read_checkpoint_region works on after phase 2 (checkpoint.rs:150-156): the
/// first read when one sector holds header + payload, else the second read.
#[logic(open)]
pub fn raw_of(h: Seq<u8>, full: Seq<u8>, ss: Int) -> Seq<u8> {
    pearlite! { if alignup(16 + dec32(h.subsequence(8, 12))@, ss) <= ss { h } else { full } }
}

/// Mirror of `read_checkpoint_region` (checkpoint.rs:117-173) where `h` is the result of the
/// header read `read_blocks(lba, sector_size)` (checkpoint.rs:127) and `full` the result of
/// `read_blocks(lba, aligned_len)` (checkpoint.rs:155, issued only when aligned_len > sector
/// size) — both ARBITRARY byte vectors (any length, any content).
#[requires(ss@ > 0 && ss@ <= 4294967295)]
#[ensures(match result { Ok(_) => true, Err(e) => e == EmError::CorruptMetadata })]
#[ensures(h@.len() < 16 ==> result == Err(EmError::CorruptMetadata))]
#[ensures(h@.len() >= 16 && raw_of(h@, full@, ss@).len() < 16 + dec32(h@.subsequence(8, 12))@ ==>
    result == Err(EmError::CorruptMetadata))]
#[ensures(h@.len() >= 16 && dec64(h@.subsequence(0, 8)) != expected ==> result == Err(EmError::CorruptMetadata))]
#[ensures(h@.len() >= 16 && 16 + dec32(h@.subsequence(8, 12))@ > region_size@ ==> result == Err(EmError::CorruptMetadata))]
pub fn read_ckpt_raw(h: Vec<u8>, full: Vec<u8>, ss: usize, region_size: u64, expected: u64) -> Result<Vec<u8>, EmError> {
    if h.len() < 16 {
        return Err(EmError::CorruptMetadata); // checkpoint.rs:128-130
    }
    let seq = rd64(&h, 0);
    let payload_len = rd32(&h, 8) as usize;
    let stored_crc = rd32(&h, 12);
    if seq != expected {
        return Err(EmError::CorruptMetadata); // checkpoint.rs:136-140
    }
    let total = 16 + payload_len;
    if total as u64 > region_size {
        return Err(EmError::CorruptMetadata); // checkpoint.rs:142-147
    }
    let aligned_len = (total + ss - 1) / ss * ss;
    let raw = if aligned_len <= ss { h } else { full };
    if raw.len() < total {
        return Err(EmError::CorruptMetadata); // checkpoint.rs:158-160
    }
    let mut check_buf = slice_vec(&raw, 0, total);
    put32b(&mut check_buf, 12, 0u32);
    let computed = crc32_hash(&check_buf, total);
    if stored_crc != computed {
        return Err(EmError::CorruptMetadata); // checkpoint.rs:166-170
    }
    Ok(slice_vec(&raw, 16, total))
}

// =============================================================================
// set_metadata_ns_id / initialize's namespace (lib.rs:181-183, 517)
// =============================================================================

impl ExtentManager {
    /// Mirror of `ExtentManager::set_metadata_ns_id` (lib.rs:181-183).
    #[ensures((^self).metadata_ns_id == Some(ns_id))]
    #[ensures((^self).arena == self.arena && (^self).regions == self.regions && (^self).shared == self.shared)]
    #[ensures((^self).metadata_base_lba == self.metadata_base_lba && (^self).data_base_lba == self.data_base_lba)]
    #[ensures((^self).dev == self.dev)]
    pub fn set_metadata_ns_id(&mut self, ns_id: u32) {
        self.metadata_ns_id = Some(ns_id);
    }

    /// `self.metadata_ns_id.lock().unwrap().unwrap_or(1)` (initialize, lib.rs:517).
    #[ensures(result == match self.metadata_ns_id { Some(n) => n, None => 1u32 })]
    pub fn init_ns(&self) -> u32 {
        match self.metadata_ns_id {
            Some(n) => n,
            None => 1,
        }
    }
}
