//! Phase-split mirror of the checkpoint path — `ExtentManager::run_checkpoint`
//! (lib.rs:275-370 at the pin), `checkpoint::write_checkpoint` (checkpoint.rs:35-115) — and
//! of `recovery::recover` (recovery.rs:11-71), over a metadata device whose contents are
//! the DECODED superblock (seq, active copy) and the two checkpoint copies (seq, image).
//!
//! Why phase-split: the source takes NO lock across a checkpoint. `write_checkpoint` holds
//! `regions.read()` (shared with every reserve/publish/abort/remove) and each region's
//! READ lock only while serialising that region (checkpoint.rs:51-55); the dirty flags
//! are cleared and the deferred frees released in a LATER critical section
//! (lib.rs:312-321). So a call from another thread — or from the background checkpoint
//! thread's own caller — can run between the snapshot and the commit. The three phases
//! [`CkEm::ck_begin`] (any_dirty + serialise), [`CkEm::ck_write`] (copy write, in-memory
//! superblock update, superblock write) and [`CkEm::ck_finish`] (clear dirty + flush) are
//! exactly those critical sections; `run_checkpoint` runs them back to back.
//!
//! Device model (IBlockDevice contract: a write either completes or returns an error): the
//! outcome of each of the two writes is an explicit oracle argument (`ok_data`, `ok_sb`),
//! so a refutation can pick a device behaviour the contract allows. A FAILED write is
//! modelled as "nothing written"; a completed copy write is assumed to read back with a
//! valid CRC (byte layout, CRC32 and the serialise/deserialise round-trip are NOT modelled:
//! the persisted image is the snapshot of the regions' slab maps). Payload-too-large
//! (checkpoint.rs:69-75) is folded into `!ok_data`. `get_metadata_client` failures and
//! logging are not modelled (they fail before any state change).
use crate::model::bitmap::*;
use crate::model::buddy::*;
use crate::model::listing::*;
use crate::model::params::*;
use crate::model::region::*;
use crate::model::slab::*;
use crate::model::assume::*;
use creusot_std::prelude::*;

/// One checkpoint copy as persisted: header seq + the serialised regions.
pub struct CkCopy {
    pub seq: u64,
    pub img: Snapshot<Seq<RegionState>>,
}

/// Decoded metadata device: superblock (checkpoint_seq, active_copy) + two copies.
pub struct CkDev {
    pub sb_seq: u64,
    pub sb_active: u8,
    pub copy0: Option<CkCopy>,
    pub copy1: Option<CkCopy>,
}

/// The component state the checkpoint path touches: the regions, and the in-memory
/// superblock's active copy and checkpoint sequence (`shared.superblock.active_copy`,
/// `shared.checkpoint_seq == shared.superblock.checkpoint_seq`).
pub struct CkEm {
    pub regions: Vec<RegionState>,
    pub active: u8,
    pub seq: u64,
    pub dev: CkDev,
}

#[logic(open)]
pub fn regions_ok(rs: Seq<RegionState>) -> bool {
    pearlite! { forall<i: Int> 0 <= i && i < rs.len() ==> rg_inv(rs[i]) && pending_ok(rs[i]) }
}

#[logic(open)]
pub fn all_clean(rs: Seq<RegionState>) -> bool {
    pearlite! { forall<i: Int> 0 <= i && i < rs.len() ==> !rs[i].dirty }
}

/// `b` is region `a` after `r.dirty = false; r.flush_pending_frees()` (lib.rs:316-321).
#[logic(open)]
pub fn fin_rel(a: RegionState, b: RegionState) -> bool {
    pearlite! {
        rg_inv(b) && rg_frame(a, b) && !b.dirty && b.pending_frees@.len() == 0
        && (a.pending_frees@.len() == 0 ==> b.slabs == a.slabs && b.size_classes == a.size_classes && b.buddy == a.buddy)
        && (forall<s: u64, i: usize> a.pending_frees@ == Seq::singleton((s, i)) ==>
                free_case(a, b, s, i, a.slabs@.lookup(s@)))
    }
}

/// LEVEL-3 D-B1: what finishing a checkpoint also guarantees for ANY list of deferred frees:
/// every queued slot is released (flushed_all) and EM-SIZECLASS-NONFULL-LISTED is kept.
#[logic(open)]
pub fn fin_more(a: RegionState, b: RegionState) -> bool {
    pearlite! { flushed_all(a, b) && slabs_shrunk(a, b) && (nonfull_listed(a) ==> nonfull_listed(b)) }
}

/// The copy the superblock names, if it carries the superblock's sequence number.
#[logic(open)]
pub fn copy_at(d: CkDev, c: u8) -> Option<CkCopy> {
    pearlite! { if c@ == 0 { d.copy0 } else { d.copy1 } }
}

impl CkEm {
    /// `run_checkpoint`'s any_dirty test (lib.rs:276-290) and Phase 1 of `write_checkpoint`
    /// (checkpoint.rs:47-55): `None` = nothing dirty (return Ok without I/O); otherwise the
    /// serialised image of every region.
    #[ensures(match result {
        None => all_clean(self.regions@),
        Some(img) => !all_clean(self.regions@) && *img == self.regions@,
    })]
    pub fn ck_begin(&self) -> Option<Snapshot<Seq<RegionState>>> {
        let mut any_dirty = false;
        let mut i: usize = 0;
        #[invariant(i@ <= self.regions@.len())]
        #[invariant(!any_dirty ==> forall<j: Int> 0 <= j && j < i@ ==> !self.regions@[j].dirty)]
        #[invariant(any_dirty ==> !all_clean(self.regions@))]
        while i < self.regions.len() {
            if self.regions[i].dirty {
                any_dirty = true;
            }
            i += 1;
        }
        if !any_dirty {
            return None;
        }
        Some(snapshot! { self.regions@ })
    }

    /// Phases 2-4 of `write_checkpoint` (checkpoint.rs:58-112) and the superblock write of
    /// `run_checkpoint` (lib.rs:302-307). `ok_data` / `ok_sb`: the device outcome of the
    /// copy write (checkpoint.rs:97) and of the superblock write (lib.rs:306).
    #[ensures((^self).regions == self.regions)]
    #[ensures(self.active@ <= 1 && self.seq@ < u64::MAX@ ==> match result {
        Ok(()) => ok_data && ok_sb
            && (^self).active@ == 1 - self.active@ && (^self).seq@ == self.seq@ + 1
            && (^self).dev.sb_seq == (^self).seq && (^self).dev.sb_active == (^self).active
            && match copy_at((^self).dev, (^self).active) { Some(c) => c.seq == (^self).seq && c.img == img, None => false }
            && copy_at((^self).dev, self.active) == copy_at(self.dev, self.active),
        Err(_) => !(ok_data && ok_sb),
    })]
    #[ensures(self.active@ <= 1 && self.seq@ < u64::MAX@ && ok_data && !ok_sb ==>
        (^self).active@ == 1 - self.active@ && (^self).seq@ == self.seq@ + 1
        && (^self).dev.sb_seq == self.dev.sb_seq && (^self).dev.sb_active == self.dev.sb_active
        && match copy_at((^self).dev, (^self).active) { Some(c) => c.seq == (^self).seq && c.img == img, None => false }
        && copy_at((^self).dev, self.active) == copy_at(self.dev, self.active))]
    #[ensures(!ok_data ==> (^self).dev.sb_seq == self.dev.sb_seq && (^self).dev.sb_active == self.dev.sb_active
        && (^self).active == self.active && (^self).seq == self.seq)]
    #[ensures(self.active@ <= 1 ==> (^self).active@ <= 1)]
    #[ensures(self.seq@ <= (^self).seq@ && (^self).seq@ <= self.seq@ + 1)]
    #[ensures(self.seq@ < u64::MAX@ ==> match result { Err(e) => e == EmError::IoError, Ok(()) => true })]
    #[ensures(!ok_data ==> (^self).dev == self.dev)]
    pub fn ck_write(&mut self, img: Snapshot<Seq<RegionState>>, ok_data: bool, ok_sb: bool) -> Result<(), EmError> {
        // `1 - s.superblock.active_copy` (checkpoint.rs:61)
        let inactive: u8 = if self.active == 0 { 1 } else { 0 };
        if self.seq == u64::MAX {
            return Err(EmError::CorruptMetadata); // `checkpoint_seq + 1` overflows (checkpoint.rs:65): excluded
        }
        let new_seq = self.seq + 1;
        // Phase 3: `metadata_client.write_blocks(lba, &blob)?` (checkpoint.rs:96-97)
        if !ok_data {
            return Err(EmError::IoError);
        }
        if inactive == 0 {
            self.dev.copy0 = Some(CkCopy { seq: new_seq, img });
        } else {
            self.dev.copy1 = Some(CkCopy { seq: new_seq, img });
        }
        // Phase 4: in-memory superblock update (checkpoint.rs:105-112)
        self.active = inactive;
        self.seq = new_seq;
        // `metadata_client.write_blocks(0, &shared.superblock.serialize())?` (lib.rs:302-307)
        if !ok_sb {
            return Err(EmError::IoError);
        }
        self.dev.sb_seq = self.seq;
        self.dev.sb_active = self.active;
        Ok(())
    }

    /// lib.rs:312-321: for every region `r.dirty = false; r.flush_pending_frees()`.
    #[requires(regions_ok(self.regions@))]
    #[ensures((^self).regions@.len() == self.regions@.len())]
    #[ensures(forall<j: Int> 0 <= j && j < self.regions@.len() ==> fin_rel(self.regions@[j], (^self).regions@[j]))]
    #[ensures(forall<j: Int> 0 <= j && j < self.regions@.len() ==> fin_more(self.regions@[j], (^self).regions@[j]))]
    #[ensures(regions_ok((^self).regions@) && all_clean((^self).regions@))]
    #[ensures((^self).dev == self.dev && (^self).active == self.active && (^self).seq == self.seq)]
    pub fn ck_finish(&mut self) {
        let old = snapshot! { *self };
        let mut i: usize = 0;
        #[invariant(i@ <= self.regions@.len() && self.regions@.len() == old.regions@.len())]
        #[invariant(forall<j: Int> 0 <= j && j < i@ ==> fin_rel(old.regions@[j], self.regions@[j]))]
        #[invariant(forall<j: Int> 0 <= j && j < i@ ==> fin_more(old.regions@[j], self.regions@[j]))]
        #[invariant(forall<j: Int> i@ <= j && j < self.regions@.len() ==> self.regions@[j] == old.regions@[j])]
        #[invariant(self.dev == old.dev && self.active == old.active && self.seq == old.seq)]
        while i < self.regions.len() {
            let before = snapshot! { self.regions };
            proof_assert! { rg_inv(self.regions@[i@]) && pending_ok(self.regions@[i@]) };
            let r = &mut self.regions[i];
            let r0 = snapshot! { *r };
            r.dirty = false;
            proof_assert! { rg_inv(*r) && pending_ok(*r) };
            proof_assert! { r.slabs == r0.slabs && r.size_classes == r0.size_classes && r.buddy == r0.buddy && r.pending_frees == r0.pending_frees && r.format_params == r0.format_params };
            let r1 = snapshot! { *r };
            r.flush_pending_frees();
            proof_assert! { forall<s: u64, k: usize> r0.pending_frees@ == Seq::singleton((s, k)) ==> {
                lemma_free_case_pending(*r1, *r0, *r, s, k, r1.slabs@.lookup(s@));
                free_case(*r0, *r, s, k, r0.slabs@.lookup(s@)) } };
            proof_assert! { fin_rel(*r0, *r) };
            proof_assert! { nonfull_listed(*r0) ==> nonfull_listed(*r1) };
            proof_assert! { fin_more(*r0, *r) };
            proof_assert! { forall<j: Int> 0 <= j && j < before@.len() && j != i@ ==> self.regions@[j] == before@[j] };
            i += 1;
        }
    }

    /// Mirror of `run_checkpoint` (lib.rs:275-370) when no other call interleaves.
    #[requires(regions_ok(self.regions@))]
    #[ensures(match result {
        Err(_) => (^self).regions == self.regions,
        Ok(()) => regions_ok((^self).regions@) && all_clean((^self).regions@),
    })]
    #[ensures(all_clean(self.regions@) ==> ^self == *self && result == Ok(()))]
    #[ensures((^self).regions@.len() == self.regions@.len())]
    #[ensures(self.active@ <= 1 && self.seq@ < u64::MAX@ && ok_data && ok_sb ==> result == Ok(()))]
    #[ensures(self.active@ <= 1 && !all_clean(self.regions@) && self.seq@ < u64::MAX@ ==> match result {
        Ok(()) => (^self).seq@ == self.seq@ + 1 && (^self).active@ == 1 - self.active@
            && (^self).dev.sb_seq == (^self).seq && (^self).dev.sb_active == (^self).active
            && match copy_at((^self).dev, (^self).active) { Some(c) => c.seq == (^self).seq && *c.img == self.regions@, None => false }
            && (forall<j: Int> 0 <= j && j < self.regions@.len() ==> fin_rel(self.regions@[j], (^self).regions@[j])),
        Err(_) => true,
    })]
    #[ensures(self.active@ <= 1 ==> (^self).active@ <= 1)]
    #[ensures(self.seq@ <= (^self).seq@ && (^self).seq@ <= self.seq@ + 1)]
    #[ensures(self.active@ <= 1 && self.seq@ < u64::MAX@ && !all_clean(self.regions@) && ok_data && !ok_sb ==>
        result != Ok(()) && (^self).active@ == 1 - self.active@ && (^self).seq@ == self.seq@ + 1
        && (^self).dev.sb_seq == self.dev.sb_seq && (^self).dev.sb_active == self.dev.sb_active
        && match copy_at((^self).dev, (^self).active) { Some(c) => c.seq == (^self).seq && *c.img == self.regions@, None => false }
        && copy_at((^self).dev, self.active) == copy_at(self.dev, self.active))]
    #[ensures(self.seq@ < u64::MAX@ ==> match result { Err(e) => e == EmError::IoError, Ok(()) => true })]
    pub fn run_checkpoint(&mut self, ok_data: bool, ok_sb: bool) -> Result<(), EmError> {
        let img = match self.ck_begin() {
            None => return Ok(()),
            Some(img) => img,
        };
        self.ck_write(img, ok_data, ok_sb)?;
        self.ck_finish();
        Ok(())
    }
}

/// Mirror of `recovery::recover` (recovery.rs:11-71) over the decoded device: the image a
/// fresh component's `initialize()` rebuilds its regions from.
#[ensures(d.sb_seq@ == 0 ==> match result { Ok(img) => *img == Seq::empty(), Err(_) => false })]
#[ensures(d.sb_seq@ > 0 ==> match copy_at(*d, d.sb_active) {
    Some(c) => c.seq == d.sb_seq ==> result == Ok(c.img),
    None => true,
})]
#[ensures(match result {
    Ok(img) => d.sb_seq@ == 0
        || (match copy_at(*d, d.sb_active) { Some(c) => c.seq == d.sb_seq && c.img == img, None => false })
        || (d.sb_seq@ > 1 && match copy_at(*d, if d.sb_active@ == 0 { 1u8 } else { 0u8 }) {
                Some(c) => c.seq@ == d.sb_seq@ - 1 && c.img == img, None => false }),
    Err(_) => true,
})]
#[ensures(forall<c: CkCopy> d.sb_seq@ > 1
    && (match copy_at(*d, d.sb_active) { Some(a) => a.seq != d.sb_seq, None => true })
    && copy_at(*d, if d.sb_active@ == 0 { 1u8 } else { 0u8 }) == Some(c) && c.seq@ == d.sb_seq@ - 1
    ==> result == Ok(c.img))]
pub fn recover(d: &CkDev) -> Result<Snapshot<Seq<RegionState>>, EmError> {
    if d.sb_seq == 0 {
        // recovery.rs:18-21: no checkpoint yet -> every region empty
        return Ok(snapshot! { Seq::empty() });
    }
    let (act, inact) = if d.sb_active == 0 { (&d.copy0, &d.copy1) } else { (&d.copy1, &d.copy0) };
    // Try active copy first (recovery.rs:29-38)
    match act {
        Some(c) => {
            if c.seq == d.sb_seq {
                return Ok(snapshot! { *c.img });
            }
        }
        None => {}
    }
    // Fall back to inactive copy (recovery.rs:48-66)
    let prev_seq = d.sb_seq - 1; // `saturating_sub(1)`, sb_seq > 0 here
    if prev_seq > 0 {
        match inact {
            Some(c) => {
                if c.seq == prev_seq {
                    return Ok(snapshot! { *c.img });
                }
            }
            None => {}
        }
    }
    Err(EmError::CorruptMetadata)
}

/// Extent `e` is listed by a component rebuilt from image `img` (recovery.rs:76-85 +
/// lib.rs:563-568 rebuild every persisted slab with its key vector; get_extents lists every
/// non-FREE key, lib.rs:632-656).
#[logic(open)]
pub fn img_lists(img: Seq<RegionState>, e: Extent) -> bool {
    pearlite! { exists<i: Int> 0 <= i && i < img.len() && from_region(img[i], e) }
}

/// No slot of any region of `img` holds key `k`.
#[logic(open)]
pub fn img_lacks_key(img: Seq<RegionState>, k: u64) -> bool {
    pearlite! {
        forall<i: Int, s: Int, j: Int> 0 <= i && i < img.len() && img[i].slabs@.contains(s)
            && 0 <= j && j < img[i].slabs@.lookup(s).keys@.len() ==> img[i].slabs@.lookup(s).keys@[j] != k
    }
}
