//! Mirror of `RegionState` — `components/extent-manager/src/region.rs:9-161` — plus the
//! per-region rebuild `initialize()` performs (lib.rs:552-565) and
//! `recovery::slab_from_descriptor` (recovery.rs:76-85).
//!
//! `slabs: BTreeMap<u64, Slab>` (region.rs:10) is wrapped in [`SlabMap`], the real
//! std `BTreeMap` behind contract-carrying `#[trusted]` leaves whose contracts are the
//! documented std semantics over a ghost `FMap<Int, Slab>` view (`get`, `get_mut`,
//! `insert`, `remove`, `range(..=k).next_back()`). Iteration order is not modelled
//! (no property of this batch depends on it; later batches may add an ordered
//! iteration leaf).
//!
//! `Result<_, ExtentManagerError>` uses [`EmError`] (variant only).
use crate::model::bitmap::*;
use crate::model::buddy::*;
use crate::model::l2::*;
use crate::model::l2r::*;
use crate::model::params::*;
use crate::model::slab::*;
use creusot_std::logic::FMap;
use crate::model::assume::*;
use creusot_std::prelude::*;
use std::collections::BTreeMap;

// ------------------------------------------------------------------ SlabMap

/// `BTreeMap<u64, Slab>` (region.rs:10) with a ghost `FMap` view.
pub struct SlabMap {
    pub inner: BTreeMap<u64, Slab>,
}

impl View for SlabMap {
    type ViewTy = FMap<Int, Slab>;
    #[logic(opaque)]
    fn view(self) -> FMap<Int, Slab> {
        dead
    }
}

impl SlabMap {
    /// TRUSTED std leaf: `BTreeMap::new()`.
    #[trusted]
    #[ensures(result@ == FMap::empty())]
    pub fn new() -> Self {
        SlabMap { inner: BTreeMap::new() }
    }

    /// TRUSTED std leaf: `BTreeMap::get(&k)`.
    #[trusted]
    #[ensures(match result { Some(v) => self@.get(k@) == Some(*v), None => self@.get(k@) == None })]
    pub fn get(&self, k: u64) -> Option<&Slab> {
        self.inner.get(&k)
    }

    /// TRUSTED std leaf: `BTreeMap::get_mut(&k)`.
    #[trusted]
    #[ensures(match result {
        Some(v) => (*self)@.get(k@) == Some(*v) && (^self)@ == (*self)@.insert(k@, ^v),
        None => (*self)@.get(k@) == None && ^self == *self,
    })]
    pub fn get_mut(&mut self, k: u64) -> Option<&mut Slab> {
        self.inner.get_mut(&k)
    }

    /// TRUSTED std leaf: `BTreeMap::insert(k, v)` (replaces an existing entry).
    #[trusted]
    #[ensures((^self)@ == (*self)@.insert(k@, v))]
    pub fn insert(&mut self, k: u64, v: Slab) {
        self.inner.insert(k, v);
    }

    /// TRUSTED std leaf: `BTreeMap::remove(&k)`.
    #[trusted]
    #[ensures((^self)@ == (*self)@.remove(k@))]
    pub fn remove(&mut self, k: u64) {
        self.inner.remove(&k);
    }

    /// TRUSTED std leaf: `self.range(..=k).next_back().map(|(&k, _)| k)` (region.rs:46-50).
    #[trusted]
    #[ensures(match result {
        Some(s) => self@.contains(s@) && s@ <= k@
            && forall<t: Int> self@.contains(t) && t <= k@ ==> t <= s@,
        None => forall<t: Int> self@.contains(t) ==> t > k@,
    })]
    pub fn floor_key(&self, k: u64) -> Option<u64> {
        self.inner.range(..=k).next_back().map(|(&k, _)| k)
    }
}

/// `SlabDescriptor` (checkpoint.rs:175-181).
pub struct SlabDescriptor {
    pub start_offset: u64,
    pub slab_size: u64,
    pub element_size: u32,
    pub keys: Vec<u64>,
}

pub struct RegionState {
    pub slabs: SlabMap,
    pub size_classes: SizeClassManager,
    pub buddy: BuddyAllocator,
    pub dirty: bool,
    pub format_params: FormatParams,
    pub pending_frees: Vec<(u64, usize)>,
}

// --------------------------------------------------------------- invariants

/// The buddy order of one slab: `ord(slab_size / sector_size)`.
#[logic(open)]
pub fn slab_k(r: RegionState) -> Int {
    pearlite! { ord(r.format_params.slab_size@ / r.format_params.sector_size@) }
}

/// Region parameters (an INVARIANT: `format_params` never changes, `rg_frame`). Level 3
/// (phase F): its headroom conjunct `slab + ss <= u64::MAX` is not an input restriction — every
/// construction site derives it from the declared ranges (format_build / format7 via
/// `lemma_sane_hd` from 7cb7c3 + e0fe79; initialize via `lemma_sb_hd`; the region-level
/// constructors via `lemma_rg_in`), see model/assume.rs.
#[logic(open)]
pub fn rg_params(r: RegionState) -> bool {
    pearlite! {
        r.format_params.sector_size == r.buddy.sector_size
        && r.format_params.sector_size@ > 0
        && r.format_params.slab_size@ > 0
        && r.format_params.slab_size@ % r.format_params.sector_size@ == 0
        && r.format_params.slab_size@ + r.format_params.sector_size@ <= u64::MAX@
    }
}

/// `slab_ok` only reads the region's geometry, which `rg_frame` preserves.
#[logic]
#[requires(rg_frame(a, b))]
#[ensures(slab_ok(a, s) == slab_ok(b, s))]
pub fn lemma_slab_ok_frame(a: RegionState, b: RegionState, s: Slab) {}

/// Buddy invariant plus the no-overflow headroom `free` needs (buddy.rs:98). Level 3 (phase
/// F): `total + span(max_order) <= u64::MAX` is established at construction from `2 * size + ss
/// <= u64::MAX`, itself DERIVED from D-RANGE-FR-002-3b58ea (`2 * data_disk_size + ss`) and the
/// region lying inside the data disk (`lemma_sane_hd` / `lemma_sb_hd` / `lemma_rg_in`); kept by
/// `rg_frame` afterwards.
#[logic(open)]
pub fn rg_buddy(r: RegionState) -> bool {
    pearlite! {
        bd_inv(r.buddy)
        && r.buddy.total_usable_size@ + span(r.buddy.sector_size@, r.buddy.max_order@) <= u64::MAX@
    }
}

/// A slab of region `r`: well-formed, of the configured slab size, occupying a
/// well-formed order-`K` buddy block of the region.
#[logic(open)]
pub fn slab_ok(r: RegionState, s: Slab) -> bool {
    pearlite! {
        slab_inv(s)
        && s.slab_size == r.format_params.slab_size
        && s.start_offset@ >= r.buddy.base_offset@
        && good_block(r.buddy, s.start_offset@ - r.buddy.base_offset@, slab_k(r))
        && slab_k(r) <= r.buddy.max_order@
    }
}

/// Every map entry is a good slab keyed by its own start offset.
#[logic(open)]
pub fn rg_slabs(r: RegionState) -> bool {
    pearlite! {
        forall<k: Int> r.slabs@.contains(k) ==>
            slab_ok(r, r.slabs@.lookup(k)) && r.slabs@.lookup(k).start_offset@ == k
    }
}

#[logic(open)]
pub fn rg_inv(r: RegionState) -> bool {
    pearlite! { rg_params(r) && rg_buddy(r) && rg_slabs(r) }
}

/// `free_slot(s, i)` does not panic: the slot it names (if its slab exists) is an
/// in-range, allocated slot (bitmap.rs:27-28 debug-asserts, slab.rs:39 index).
#[logic(open)]
pub fn free_ok(r: RegionState, s: u64, i: usize) -> bool {
    pearlite! {
        match r.slabs@.get(s@) {
            Some(sl) => i@ < sl.bitmap.num_slots@ && slot_bit(sl.bitmap, i@),
            None => true,
        }
    }
}

/// Every queued deferred free names a distinct, in-range, allocated slot.
#[logic(open)]
pub fn pending_ok(r: RegionState) -> bool {
    pearlite! {
        (forall<p: Int> 0 <= p && p < r.pending_frees@.len() ==>
            r.slabs@.contains(r.pending_frees@[p].0@)
            && free_ok(r, r.pending_frees@[p].0, r.pending_frees@[p].1))
        && (forall<p: Int, q: Int> 0 <= p && p < q && q < r.pending_frees@.len() ==>
            r.pending_frees@[p] != r.pending_frees@[q])
    }
}

/// Frame: the region's geometry and buddy identity never change after construction.
#[logic(open)]
pub fn rg_frame(a: RegionState, b: RegionState) -> bool {
    pearlite! {
        a.format_params == b.format_params
        && a.buddy.base_offset == b.buddy.base_offset
        && a.buddy.total_usable_size == b.buddy.total_usable_size
        && a.buddy.sector_size == b.buddy.sector_size
        && a.buddy.max_order == b.buddy.max_order
        && a.buddy.free_lists@.len() == b.buddy.free_lists@.len()
    }
}

/// A freshly created slab right after its first `alloc_slot` (region.rs:76-77).
#[logic(open)]
pub fn fresh_one(s: Slab, d: Int, slab_size: Int, es: Int) -> bool {
    pearlite! {
        s.start_offset@ == d && s.slab_size@ == slab_size && s.element_size@ == es
        && s.bitmap.num_slots@ == slots_of(slab_size, es)
        && slot_bit(s.bitmap, 0)
        && (forall<j: Int> 0 < j && j < s.bitmap.num_slots@ ==> !slot_bit(s.bitmap, j))
        && (forall<j: Int> 0 <= j && j < s.keys@.len() ==> s.keys@[j] == FREE_KEY)
        && s.bitmap.allocated_count@ == 1
        && slab_inv(s)
    }
}

/// `align_to_sector_size` as a logic function (region.rs:36-38).
#[logic(open)]
pub fn align_l(size: Int, ss: Int) -> Int {
    pearlite! { (size + ss - 1) / ss * ss }
}

impl RegionState {
    /// Mirror of `RegionState::new` (region.rs:25-34).
    #[ensures(result.slabs@ == FMap::empty())]
    #[ensures(forall<e: Int> sc_get(result.size_classes, e) == Seq::empty())]
    #[ensures(result.buddy == buddy && result.format_params == format_params)]
    #[ensures(result.dirty == false && result.pending_frees@.len() == 0)]
    pub fn new(buddy: BuddyAllocator, format_params: FormatParams) -> Self {
        Self {
            slabs: SlabMap::new(),
            size_classes: SizeClassManager::new(),
            buddy,
            dirty: false,
            format_params,
            pending_frees: Vec::new(),
        }
    }

    /// Mirror of `RegionState::align_to_sector_size` (region.rs:36-38).
    #[requires(sector_size@ > 0 && size@ > 0 && a_3783f8(size@, sector_size@))]
    #[ensures(result@ == align_l(size@, sector_size@))]
    #[ensures(size@ <= result@ && result@ < size@ + sector_size@)]
    pub fn align_to_sector_size(&self, size: u32, sector_size: u32) -> u32 {
        proof_assert! { lemma_divmod(size@ + sector_size@ - 1, sector_size@);
            size@ + sector_size@ - 1 == sector_size@ * ((size@ + sector_size@ - 1) / sector_size@) + (size@ + sector_size@ - 1) % sector_size@ };
        proof_assert! { (size@ + sector_size@ - 1) / sector_size@ * sector_size@ == sector_size@ * ((size@ + sector_size@ - 1) / sector_size@) };
        // region.rs:37 `(size + sector_size - 1)`, same evaluation order (3783f8: size + ss <= u32::MAX).
        (size + sector_size - 1) / sector_size * sector_size
    }

    /// Mirror of `RegionState::alloc_extent` (region.rs:40-91).
    ///
    /// Besides the invariant, two EXACT cases are specified (used by refutations):
    /// (B) the size class is empty: a fresh slab is carved from the buddy allocator;
    /// (C) the size class's first slab has a free slot: that slab is used.
    #[requires(rg_inv(*self))]
    #[requires(size@ > 0)]
    #[requires(a_3783f8(size@, self.format_params.sector_size@))]
    #[ensures(rg_inv(^self))]
    #[ensures(rg_frame(*self, ^self))]
    #[ensures((^self).pending_frees == self.pending_frees && (^self).dirty == self.dirty)]
    #[ensures(forall<s: u64, i: usize, off: u64> result == Ok((s, i, off)) ==>
        (^self).slabs@.contains(s@)
        && i@ < (^self).slabs@.lookup(s@).bitmap.num_slots@
        && slot_bit((^self).slabs@.lookup(s@).bitmap, i@)
        && off@ == slot_off((^self).slabs@.lookup(s@), i@))]
    #[ensures(sc_get(self.size_classes, align_l(size@, self.format_params.sector_size@)).len() == 0
        && align_l(size@, self.format_params.sector_size@) <= self.format_params.slab_size@
        && slots_of(self.format_params.slab_size@, align_l(size@, self.format_params.sector_size@)) >= 1 ==>
        fresh_case(*self, ^self, align_l(size@, self.format_params.sector_size@), result))]
    #[ensures(nonfull_listed(*self) ==> nonfull_listed(^self))]
    #[ensures(sc_get(self.size_classes, align_l(size@, self.format_params.sector_size@)).len() > 0 ==>
        exist_case(*self, ^self, align_l(size@, self.format_params.sector_size@),
                   sc_get(self.size_classes, align_l(size@, self.format_params.sector_size@))[0], result))]
    #[ensures(no_new_keys(*self, ^self))]
    #[ensures(match result { Ok(_) => true, Err(e) => e == EmError::OutOfSpace && err_frame(*self, ^self) })]
    #[ensures(sc_get(self.size_classes, align_l(size@, self.format_params.sector_size@)).len() == 0
        && slots_of(self.format_params.slab_size@, align_l(size@, self.format_params.sector_size@)) == 0 ==>
        match result { Ok(_) => false, Err(_) => true })]
    #[ensures(forall<k: Int> nonfull_listed(*self) && free_at(*self, k, align_l(size@, self.format_params.sector_size@)) ==>
        match result { Ok(_) => true, Err(_) => false })]
    #[ensures(alloc_any(*self, ^self, align_l(size@, self.format_params.sector_size@), result))]
    pub fn alloc_extent(&mut self, size: u32) -> Result<(u64, usize, u64), EmError> {
        let element_size = self.align_to_sector_size(size, self.format_params.sector_size);
        let old = snapshot! { *self };
        let mut first = snapshot! { true };
        g_sc_refl(snapshot! { old.size_classes }, snapshot! { element_size@ }); // PROOF-ONLY (ghost)

        #[invariant(rg_inv(*self) && rg_frame(*old, *self))]
        #[invariant(self.buddy == old.buddy && self.pending_frees == old.pending_frees && self.dirty == old.dirty)]
        #[invariant(*first ==> *self == *old)]
        #[invariant(!*first ==> !exist_cond(*old, element_size@, sc_get(old.size_classes, element_size@)[0]))]
        #[invariant(!*first ==> sc_get(old.size_classes, element_size@).len() > 0)]
        #[invariant(nonfull_listed(*old) ==> nonfull_listed(*self))]
        #[invariant(no_new_keys(*old, *self))]
        #[invariant(self.slabs@ == old.slabs@)]
        #[invariant(forall<k: Int> nonfull_listed(*old) && free_at(*old, k, element_size@) ==>
            listed(sc_get(self.size_classes, element_size@), k))]
        #[invariant(sc_sub(old.size_classes, self.size_classes, element_size@, -1))]
        loop {
            let it = snapshot! { *self };
            let slab_start = match self.size_classes.get_slabs_first(element_size) {
                Some(s) => s,
                None => {
                    proof_assert! { sc_get(self.size_classes, element_size@).len() == 0 };
                    break;
                }
            };
            proof_assert! { sc_get(old.size_classes, element_size@).len() > 0 };
            match self.slabs.get_mut(slab_start) {
                Some(slab) => {
                    proof_assert! { slab_inv(*slab) };
                    let pre_slab = snapshot! { *slab };
                    match slab.alloc_slot() {
                        Some((slot_idx, offset)) => {
                            // Remove from the non-full list if the slab just became full.
                            let now_full = slab.is_full();
                            let post_slab = snapshot! { *slab };
                            proof_assert! { nonfull_listed(*it) ==> {
                                lemma_nf_update(*it, *self, slab_start, *pre_slab, *post_slab); nonfull_listed(*self) } };
                            if now_full {
                                self.sc_remove(element_size, slab_start);
                                g_sc_remove(snapshot! { old.size_classes }, snapshot! { it.size_classes }, snapshot! { self.size_classes },
                                    snapshot! { element_size@ }, slab_start); // PROOF-ONLY (ghost)
                            }
                            proof_assert! { self.slabs@ == old.slabs@.insert(slab_start@, *post_slab) || !*first };
                            proof_assert! { self.slabs@.get(slab_start@) == Some(*post_slab) };
                            proof_assert! { self.slabs@.lookup(slab_start@) == *post_slab };
                            proof_assert! { offset@ == slot_off(*pre_slab, slot_idx@) };
                            proof_assert! { pre_slab.start_offset == post_slab.start_offset && pre_slab.element_size == post_slab.element_size };
                            proof_assert! { offset@ == slot_off(*post_slab, slot_idx@) };
                            proof_assert! { self.slabs@ == it.slabs@.insert(slab_start@, *post_slab) };
                            proof_assert! { lemma_slab_ok_frame(*it, *self, *post_slab); slab_ok(*it, *pre_slab) && slab_ok(*self, *post_slab) };
                            proof_assert! { forall<k: Int> self.slabs@.contains(k) && k != slab_start@ ==>
                                it.slabs@.contains(k) && self.slabs@.lookup(k) == it.slabs@.lookup(k) };
                            proof_assert! { forall<k: Int> self.slabs@.contains(k) ==> {
                                lemma_slab_ok_frame(*it, *self, self.slabs@.lookup(k));
                                slab_ok(*self, self.slabs@.lookup(k)) && self.slabs@.lookup(k).start_offset@ == k } };
                            proof_assert! { it.slabs@.get(slab_start@) == Some(*pre_slab) };
                            proof_assert! { pre_slab.keys == post_slab.keys && pre_slab.bitmap.num_slots == post_slab.bitmap.num_slots };
                            proof_assert! { no_new_keys(*it, *self) };
                            proof_assert! { lemma_nnk_trans(*old, *it, *self); no_new_keys(*old, *self) };
                            g_sc_first(snapshot! { old.size_classes }, snapshot! { it.size_classes }, snapshot! { element_size@ }, slab_start); // PROOF-ONLY
                            g_alloc_exist(old, snapshot! { *self }, snapshot! { element_size@ }, slab_start, slot_idx, offset); // PROOF-ONLY
                            return Ok((slab_start, slot_idx, offset));
                        }
                        None => {
                            proof_assert! { lemma_cnt_all_set(slab.bitmap, slab.bitmap.num_slots@); true };
                            proof_assert! { slab.bitmap.allocated_count@ == slab.bitmap.num_slots@ };
                        }
                    }
                    proof_assert! { self.slabs@ == it.slabs@.insert(slab_start@, *pre_slab) };
                    proof_assert! { it.slabs@.get(slab_start@) == Some(*pre_slab) };
                    proof_assert! { self.slabs@.ext_eq(it.slabs@) };
                    proof_assert! { no_new_keys(*it, *self) };
                    proof_assert! { lemma_nnk_trans(*old, *it, *self); no_new_keys(*old, *self) };
                    proof_assert! { nonfull_listed(*it) ==> {
                        lemma_nf_update(*it, *self, slab_start, *pre_slab, *pre_slab); nonfull_listed(*self) } };
                }
                None => {}
            }
            // Stale entry: slab was full or missing — remove it and try the next.
            proof_assert! { !self.slabs@.contains(slab_start@)
                || self.slabs@.lookup(slab_start@).bitmap.allocated_count@ == self.slabs@.lookup(slab_start@).bitmap.num_slots@ };
            proof_assert! { forall<k: Int> nonfull_listed(*old) && free_at(*old, k, element_size@) ==> slab_start@ != k };
            let pre_sc = snapshot! { *self };
            self.sc_remove(element_size, slab_start);
            g_sc_remove(snapshot! { old.size_classes }, snapshot! { pre_sc.size_classes }, snapshot! { self.size_classes },
                snapshot! { element_size@ }, slab_start); // PROOF-ONLY (ghost)
            proof_assert! { forall<k: Int> nonfull_listed(*old) && free_at(*old, k, element_size@) ==> {
                lemma_listed_filter(sc_get(pre_sc.size_classes, element_size@), slab_start, k);
                listed(sc_get(self.size_classes, element_size@), k) } };
            first = snapshot! { false };
        }

        proof_assert! { forall<k: Int> nonfull_listed(*old) && free_at(*old, k, element_size@) ==> false };
        let exitst = snapshot! { *self };
        let slab_size = self.format_params.slab_size;

        if element_size as u64 > slab_size {
            g_alloc_err(old, snapshot! { *self }, snapshot! { element_size@ }, EmError::OutOfSpace); // PROOF-ONLY
            return Err(EmError::OutOfSpace);
        }

        proof_assert! { lemma_div_exact(slab_size@, self.format_params.sector_size@); true };
        let disk_offset = match self.buddy.alloc(slab_size) {
            Some(d) => d,
            None => {
                g_alloc_err(old, snapshot! { *self }, snapshot! { element_size@ }, EmError::OutOfSpace); // PROOF-ONLY
                return Err(EmError::OutOfSpace);
            }
        };
        proof_assert! { lemma_slab_fits(*self); slab_size@ <= span(self.buddy.sector_size@, slab_k(*self)) };

        let mut slab = Slab::new(disk_offset, slab_size, element_size);
        let fresh = snapshot! { slab };
        proof_assert! { fresh.rover@ == 0 };
        proof_assert! { self.buddy.sector_size == self.format_params.sector_size };
        proof_assert! { slab_size@ % self.format_params.sector_size@ == 0 && self.format_params.sector_size@ > 0 && self.buddy.sector_size == self.format_params.sector_size };
        proof_assert! { lemma_div_exact(slab_size@, self.format_params.sector_size@);
            (slab_size@ + self.buddy.sector_size@ - 1) / self.buddy.sector_size@ == slab_size@ / self.format_params.sector_size@ };
        proof_assert! { ord((slab_size@ + self.buddy.sector_size@ - 1) / self.buddy.sector_size@) == slab_k(*self) };
        proof_assert! { exitst.buddy == old.buddy && exitst.slabs@ == old.slabs@ };
        let (slot_idx, offset) = match slab.alloc_slot() {
            Some(result) => result,
            None => {
                let pre_free = snapshot! { *self };
                proof_assert! { alloc_post(exitst.buddy, self.buddy, ord((slab_size@ + self.buddy.sector_size@ - 1) / self.buddy.sector_size@),
                    first_ne(exitst.buddy.free_lists@, ord((slab_size@ + self.buddy.sector_size@ - 1) / self.buddy.sector_size@), exitst.buddy.max_order@), disk_offset@) };
                proof_assert! { slab_k(*self) <= self.buddy.max_order@ };
                proof_assert! { ord(slab_size@ / self.buddy.sector_size@) == slab_k(*self) };
                proof_assert! { rg_tf(*pre_free) == rg_tf(*old) - span(self.buddy.sector_size@, slab_k(*self)) };
                self.buddy.free(disk_offset, slab_size);
                proof_assert! { rg_tf(*self) == rg_tf(*pre_free) + span(self.buddy.sector_size@, slab_k(*self)) };
                proof_assert! { rg_tf(*self) == rg_tf(*old) };
                g_alloc_rollback(old, snapshot! { *self }, snapshot! { pre_free.buddy }, snapshot! { element_size@ },
                    snapshot! { first_ne(old.buddy.free_lists@, slab_k(*old), old.buddy.max_order@) }, snapshot! { disk_offset@ },
                    EmError::OutOfSpace); // PROOF-ONLY
                return Err(EmError::OutOfSpace);
            }
        };
        proof_assert! { slot_idx@ < fresh.bitmap.num_slots@ && fresh.bitmap.num_slots@ == slots_of(slab_size@, element_size@) };
        proof_assert! { slab_size@ % self.format_params.sector_size@ == 0 && self.format_params.sector_size@ > 0 && self.buddy.sector_size == self.format_params.sector_size };
        proof_assert! { lemma_div_exact(slab_size@, self.format_params.sector_size@);
            (slab_size@ + self.buddy.sector_size@ - 1) / self.buddy.sector_size@ == slab_size@ / self.format_params.sector_size@ };
        proof_assert! { ord((slab_size@ + self.buddy.sector_size@ - 1) / self.buddy.sector_size@) == slab_k(*self) };
        proof_assert! { slot_idx@ > 0 ==> slot_bit(fresh.bitmap, 0) };
        proof_assert! { !slot_bit(fresh.bitmap, 0) };
        proof_assert! { slot_idx@ == 0 };
        proof_assert! { offset == disk_offset };
        proof_assert! { slab_ok(*self, slab) };
        proof_assert! { self.slabs == exitst.slabs && self.size_classes == exitst.size_classes };
        proof_assert! { nonfull_listed(*old) ==> { lemma_nf_frame(*exitst, *self); nonfull_listed(*self) } };
        let pre_ins = snapshot! { *self };
        let new_slab = snapshot! { slab };
        // Only add to the non-full list if the slab still has capacity after this first alloc;
        // then insert it (region.rs:84-89).
        self.insert_fresh(disk_offset, slab, element_size);
        proof_assert! { self.slabs@.lookup(disk_offset@) == *new_slab };
        proof_assert! { forall<j: Int> 0 <= j && j < new_slab.keys@.len() ==> new_slab.keys@[j] == FREE_KEY };
        proof_assert! { no_new_keys(*exitst, *self) };
        proof_assert! { lemma_nnk_trans(*old, *exitst, *self); no_new_keys(*old, *self) };
        proof_assert! { offset@ == slot_off(*new_slab, slot_idx@) };
        proof_assert! { lemma_slab_ok_frame(*pre_ins, *self, *new_slab); slab_ok(*self, *new_slab) && new_slab.start_offset == disk_offset };
        proof_assert! { forall<k: Int> self.slabs@.contains(k) && k != disk_offset@ ==> old.slabs@.contains(k) && self.slabs@.lookup(k) == old.slabs@.lookup(k) };
        proof_assert! { forall<k: Int> old.slabs@.contains(k) ==> {
            lemma_slab_ok_frame(*old, *self, old.slabs@.lookup(k));
            slab_ok(*self, old.slabs@.lookup(k)) && old.slabs@.lookup(k).start_offset@ == k } };
        proof_assert! { forall<k: Int> self.slabs@.contains(k) ==> {
            lemma_slab_ok_frame(*pre_ins, *self, self.slabs@.lookup(k));
            lemma_slab_ok_frame(*old, *self, self.slabs@.lookup(k));
            slab_ok(*self, self.slabs@.lookup(k)) && self.slabs@.lookup(k).start_offset@ == k } };
        proof_assert! { rg_inv(*self) };
        let fo = snapshot! { first_ne(old.buddy.free_lists@, slab_k(*old), old.buddy.max_order@) };
        proof_assert! { *first ==> *fo != -1 };
        proof_assert! { *first ==> alloc_post(old.buddy, self.buddy, slab_k(*old), *fo, disk_offset@) };
        proof_assert! { *first ==> self.slabs@.ext_eq(old.slabs@.insert(disk_offset@, self.slabs@.lookup(disk_offset@))) };
        proof_assert! { *first ==> fresh_one(self.slabs@.lookup(disk_offset@), disk_offset@, old.format_params.slab_size@, element_size@) };
        proof_assert! { *first ==> self.slabs@.lookup(disk_offset@).rover@ == 1 % slots_of(old.format_params.slab_size@, element_size@) };
        proof_assert! { *first ==> forall<e: Int> sc_get(self.size_classes, e) ==
                if e == element_size@ && slots_of(old.format_params.slab_size@, element_size@) > 1 { sc_get(old.size_classes, e).push_back(disk_offset) }
                else { sc_get(old.size_classes, e) } };
        proof_assert! { *first ==> fresh_case(*old, *self, element_size@, Ok((disk_offset, slot_idx, offset))) };
        g_sc_push(snapshot! { old.size_classes }, snapshot! { pre_ins.size_classes }, snapshot! { self.size_classes }, snapshot! { element_size@ },
            disk_offset, snapshot! { new_slab.bitmap.allocated_count@ != new_slab.bitmap.num_slots@ }); // PROOF-ONLY
        g_alloc_fresh(old, snapshot! { *self }, snapshot! { element_size@ },
            snapshot! { first_ne(old.buddy.free_lists@, slab_k(*old), old.buddy.max_order@) }, disk_offset, slot_idx, offset); // PROOF-ONLY

        Ok((disk_offset, slot_idx, offset))
    }

    /// `self.size_classes.remove_slab(es, s)` as alloc_extent uses it (region.rs:56, 61): the
    /// slab at `s` is missing or full, so no free slot is unlisted by the removal.
    #[requires(!self.slabs@.contains(s@)
        || self.slabs@.lookup(s@).bitmap.allocated_count@ == self.slabs@.lookup(s@).bitmap.num_slots@)]
    #[ensures(forall<e: Int> sc_get((^self).size_classes, e) == if e == es@ { filter_ne(sc_get(self.size_classes, e), s) } else { sc_get(self.size_classes, e) })]
    #[ensures((^self).slabs == self.slabs && (^self).buddy == self.buddy && (^self).dirty == self.dirty)]
    #[ensures((^self).format_params == self.format_params && (^self).pending_frees == self.pending_frees)]
    #[ensures(nonfull_listed(*self) ==> nonfull_listed(^self))]
    #[ensures(rg_frame(*self, ^self))]
    #[ensures(rg_inv(*self) ==> rg_inv(^self))]
    pub fn sc_remove(&mut self, es: u32, s: u64) {
        let it = snapshot! { *self };
        self.size_classes.remove_slab(es, s);
        proof_assert! { rg_inv(*it) ==> forall<k: Int> self.slabs@.contains(k) ==> {
            lemma_slab_ok_frame(*it, *self, self.slabs@.lookup(k));
            slab_ok(*self, self.slabs@.lookup(k)) && self.slabs@.lookup(k).start_offset@ == k } };
        proof_assert! { forall<k: Int> self.slabs@.contains(k)
            && self.slabs@.lookup(k).bitmap.allocated_count@ < self.slabs@.lookup(k).bitmap.num_slots@ ==> k != s@ };
        proof_assert! { nonfull_listed(*it) ==> forall<k: Int> self.slabs@.contains(k)
            && self.slabs@.lookup(k).bitmap.allocated_count@ < self.slabs@.lookup(k).bitmap.num_slots@ ==>
            listed(sc_get(it.size_classes, self.slabs@.lookup(k).element_size@), k) };
        proof_assert! { nonfull_listed(*it) ==> forall<k: Int> self.slabs@.contains(k)
            && self.slabs@.lookup(k).bitmap.allocated_count@ < self.slabs@.lookup(k).bitmap.num_slots@ ==> {
                lemma_listed_filter(sc_get(it.size_classes, self.slabs@.lookup(k).element_size@), s, k);
                listed(sc_get(self.size_classes, self.slabs@.lookup(k).element_size@), k) } };
    }

    /// region.rs:84-89: `if !slab.is_full() { size_classes.add_slab(es, d) }` then
    /// `slabs.insert(d, slab)`.
    #[requires(slab_inv(slab) && slab.element_size == es)]
    #[ensures((^self).slabs@ == self.slabs@.insert(d@, slab))]
    #[ensures(forall<e: Int> sc_get((^self).size_classes, e) ==
        if e == es@ && slab.bitmap.allocated_count@ != slab.bitmap.num_slots@ { sc_get(self.size_classes, e).push_back(d) } else { sc_get(self.size_classes, e) })]
    #[ensures((^self).buddy == self.buddy && (^self).dirty == self.dirty)]
    #[ensures((^self).format_params == self.format_params && (^self).pending_frees == self.pending_frees)]
    #[ensures(nonfull_listed(*self) ==> nonfull_listed(^self))]
    pub fn insert_fresh(&mut self, d: u64, slab: Slab, es: u32) {
        let it = snapshot! { *self };
        let sl = snapshot! { slab };
        proof_assert! { lemma_cnt_bounds(sl.bitmap, sl.bitmap.num_slots@); sl.bitmap.allocated_count@ <= sl.bitmap.num_slots@ };
        if !slab.is_full() {
            self.size_classes.add_slab(es, d);
        }
        self.slabs.insert(d, slab);
        proof_assert! { sl.bitmap.allocated_count@ < sl.bitmap.num_slots@ ==> {
            lemma_listed_push_self(sc_get(it.size_classes, es@), d); listed(sc_get(self.size_classes, es@), d@) } };
        proof_assert! { forall<k: Int, e: Int> listed(sc_get(it.size_classes, e), k) ==> {
            lemma_listed_push(sc_get(it.size_classes, e), d, k);
            listed(sc_get(self.size_classes, e), k) } };
    }

    /// Mirror of `RegionState::free_slot` (region.rs:93-111).
    #[requires(rg_inv(*self))]
    #[requires(free_ok(*self, slab_start, slot_idx))]
    #[ensures(rg_inv(^self))]
    #[ensures(rg_frame(*self, ^self))]
    #[ensures((^self).pending_frees == self.pending_frees && (^self).dirty == self.dirty)]
    #[ensures(self.slabs@.get(slab_start@) == None ==> ^self == *self)]
    #[ensures(forall<sl: Slab> self.slabs@.get(slab_start@) == Some(sl) ==> free_case(*self, ^self, slab_start, slot_idx, sl))]
    #[ensures(nonfull_listed(*self) ==> nonfull_listed(^self))]
    #[ensures(forall<sl: Slab> self.slabs@.get(slab_start@) == Some(sl) ==>
        tf((^self).buddy.free_lists@, self.buddy.sector_size@, (^self).buddy.free_lists@.len())
        == tf(self.buddy.free_lists@, self.buddy.sector_size@, self.buddy.free_lists@.len())
           + if sl.bitmap.allocated_count@ == 1 { span(self.buddy.sector_size@, slab_k(*self)) } else { 0 })]
    #[ensures(forall<sl: Slab> self.slabs@.get(slab_start@) == Some(sl) && sl.bitmap.allocated_count@ == 1 ==>
        free_e(self.buddy.free_lists@, (^self).buddy.free_lists@, self.buddy.sector_size@, slab_start@ - self.buddy.base_offset@, sk(*self)))]
    pub fn free_slot(&mut self, slab_start: u64, slot_idx: usize) {
        let old = snapshot! { *self };
        let (was_full, now_empty, element_size, slab_size) = match self.slabs.get_mut(slab_start) {
            Some(slab) => {
                let was_full = slab.is_full();
                slab.free_slot(slot_idx);
                (was_full, slab.is_empty(), slab.element_size, slab.slab_size)
            }
            None => return,
        };
        let sl = snapshot! { old.slabs@.lookup(slab_start@) };
        proof_assert! { old.slabs@.get(slab_start@) == Some(*sl) };
        proof_assert! { self.slabs@ == old.slabs@.insert(slab_start@, self.slabs@.lookup(slab_start@)) };
        proof_assert! { element_size == sl.element_size && slab_size == sl.slab_size };
        proof_assert! { now_empty == (sl.bitmap.allocated_count@ == 1) };
        proof_assert! { was_full == (sl.bitmap.allocated_count@ == sl.bitmap.num_slots@) };

        if now_empty {
            proof_assert! { slab_size == self.format_params.slab_size };
            proof_assert! { ord(slab_size@ / self.buddy.sector_size@) == slab_k(*self) };
            let b0 = snapshot! { self.buddy };
            self.buddy.free(slab_start, slab_size);
            proof_assert! { tf(self.buddy.free_lists@, old.buddy.sector_size@, self.buddy.free_lists@.len())
                == tf(old.buddy.free_lists@, old.buddy.sector_size@, old.buddy.free_lists@.len()) + span(old.buddy.sector_size@, slab_k(*old)) };
            proof_assert! { free_e(old.buddy.free_lists@, self.buddy.free_lists@, old.buddy.sector_size@, slab_start@ - old.buddy.base_offset@, sk(*old)) };
            self.size_classes.remove_slab(element_size, slab_start);
            self.slabs.remove(slab_start);
            proof_assert! { self.slabs@.ext_eq(old.slabs@.remove(slab_start@)) };
            proof_assert! { self.slabs@ == old.slabs@.remove(slab_start@) };
            proof_assert! { rg_params(*self) && rg_buddy(*self) };
            proof_assert! { forall<k: Int> self.slabs@.contains(k) ==> {
                lemma_slab_ok_frame(*old, *self, self.slabs@.lookup(k));
                slab_ok(*self, self.slabs@.lookup(k)) && self.slabs@.lookup(k).start_offset@ == k } };
            proof_assert! { rg_slabs(*self) };
            proof_assert! { free_case(*old, *self, slab_start, slot_idx, *sl) };
            proof_assert! { nonfull_listed(*old) ==> forall<k: Int> self.slabs@.contains(k)
                && self.slabs@.lookup(k).bitmap.allocated_count@ < self.slabs@.lookup(k).bitmap.num_slots@ ==> {
                    lemma_listed_filter(sc_get(old.size_classes, self.slabs@.lookup(k).element_size@), slab_start, k);
                    listed(sc_get(self.size_classes, self.slabs@.lookup(k).element_size@), k) } };
        } else if was_full {
            // Slab went from full -> partial: re-add to the non-full list.
            self.size_classes.add_slab(element_size, slab_start);
            proof_assert! { rg_params(*self) && rg_buddy(*self) };
            proof_assert! { forall<k: Int> self.slabs@.contains(k) ==> {
                lemma_slab_ok_frame(*old, *self, self.slabs@.lookup(k));
                slab_ok(*self, self.slabs@.lookup(k)) && self.slabs@.lookup(k).start_offset@ == k } };
            proof_assert! { rg_slabs(*self) };
            proof_assert! { free_case(*old, *self, slab_start, slot_idx, *sl) };
            proof_assert! { lemma_listed_push_self(sc_get(old.size_classes, element_size@), slab_start);
                listed(sc_get(self.size_classes, element_size@), slab_start@) };
            proof_assert! { forall<k: Int, e: Int> listed(sc_get(old.size_classes, e), k) ==> {
                lemma_listed_push(sc_get(old.size_classes, e), slab_start, k);
                listed(sc_get(self.size_classes, e), k) } };
        } else {
            proof_assert! { rg_params(*self) && rg_buddy(*self) };
            proof_assert! { forall<k: Int> self.slabs@.contains(k) ==> {
                lemma_slab_ok_frame(*old, *self, self.slabs@.lookup(k));
                slab_ok(*self, self.slabs@.lookup(k)) && self.slabs@.lookup(k).start_offset@ == k } };
            proof_assert! { rg_slabs(*self) };
            proof_assert! { free_case(*old, *self, slab_start, slot_idx, *sl) };
            proof_assert! { self.size_classes == old.size_classes };
        }
    }

    /// Mirror of `RegionState::publish_slot` (region.rs:113-118).
    #[requires(rg_inv(*self))]
    #[requires(match self.slabs@.get(slab_start@) { Some(sl) => slot_idx@ < sl.keys@.len(), None => true })]
    #[ensures(rg_inv(^self))]
    #[ensures(rg_frame(*self, ^self))]
    #[ensures((^self).buddy == self.buddy && (^self).pending_frees == self.pending_frees)]
    #[ensures((^self).size_classes == self.size_classes && (^self).dirty == true)]
    #[ensures(self.slabs@.get(slab_start@) == None ==> (^self).slabs == self.slabs)]
    #[ensures(forall<sl: Slab> self.slabs@.get(slab_start@) == Some(sl) ==>
        (^self).slabs@ == self.slabs@.insert(slab_start@, (^self).slabs@.lookup(slab_start@))
        && (^self).slabs@.lookup(slab_start@).keys@ == sl.keys@.set(slot_idx@, key)
        && (^self).slabs@.lookup(slab_start@).bitmap == sl.bitmap
        && (^self).slabs@.lookup(slab_start@).element_size == sl.element_size
        && (^self).slabs@.lookup(slab_start@).start_offset == sl.start_offset
        && (^self).slabs@.lookup(slab_start@).slab_size == sl.slab_size)]
    #[ensures(nonfull_listed(*self) ==> nonfull_listed(^self))]
    pub fn publish_slot(&mut self, slab_start: u64, slot_idx: usize, key: u64) {
        if let Some(slab) = self.slabs.get_mut(slab_start) {
            slab.set_key(slot_idx, key);
        }
        self.dirty = true;
    }

    /// Mirror of `RegionState::remove_extent_by_offset` (region.rs:120-149).
    #[requires(rg_inv(*self))]
    #[ensures(rg_inv(^self))]
    #[ensures(rg_frame(*self, ^self))]
    #[ensures((^self).buddy == self.buddy && (^self).size_classes == self.size_classes)]
    #[ensures(match result {
        Ok(()) => (^self).dirty == true && (^self).pending_frees@.len() == self.pending_frees@.len() + 1,
        Err(_) => ^self == *self,
    })]
    #[ensures(forall<s: u64, i: usize> rm_case(*self, offset, s, i) ==> rm_post(*self, ^self, s, i) && result == Ok(()))]
    #[ensures(nonfull_listed(*self) ==> nonfull_listed(^self))]
    #[ensures(result == Ok(()) ==> rm_step(*self, ^self, offset))]
    #[ensures(match result { Ok(()) => true, Err(e) => e == EmError::OffsetNotFound(offset) })]
    pub fn remove_extent_by_offset(&mut self, offset: u64) -> Result<(), EmError> {
        let slab_start = match self.slabs.floor_key(offset) {
            Some(k) => k,
            None => return Err(EmError::OffsetNotFound(offset)),
        };

        let slot_idx = {
            let slab = match self.slabs.get(slab_start) {
                Some(s) => s,
                None => return Err(EmError::OffsetNotFound(offset)), // unreachable: `.unwrap()` (region.rs:129)
            };
            proof_assert! { forall<s: u64, i: usize> rm_case(*self, offset, s, i) ==> slab_start == s };
            proof_assert! { forall<s: u64, i: usize> rm_case(*self, offset, s, i) ==> {
                lemma_slot_inside(self.slabs@.lookup(s@), i@); offset@ < s@ + self.slabs@.lookup(s@).slab_size@ } };
            if !slab.contains_offset(offset) {
                return Err(EmError::OffsetNotFound(offset));
            }
            let slot = match slab.slot_for_offset(offset) {
                Some(s) => s,
                None => return Err(EmError::OffsetNotFound(offset)),
            };
            proof_assert! { forall<s: u64, i: usize> rm_case(*self, offset, s, i) ==> {
                lemma_slot_off_inj(self.slabs@.lookup(s@), i@, slot@); slot == i } };
            if !slab.bitmap.is_set(slot) || slab.get_key(slot) == FREE_KEY {
                return Err(EmError::OffsetNotFound(offset));
            }
            slot
        };

        proof_assert! { forall<s: u64, i: usize> rm_case(*self, offset, s, i) ==> slab_start == s && slot_idx == i };
        let pre = snapshot! { *self };
        match self.slabs.get_mut(slab_start) {
            Some(sl) => sl.set_key(slot_idx, FREE_KEY),
            None => {} // unreachable: `.unwrap()` (region.rs:144)
        }
        self.pending_frees.push((slab_start, slot_idx));
        self.dirty = true;
        proof_assert! { forall<s: u64, i: usize> rm_case(*pre, offset, s, i) ==> rm_post(*pre, *self, s, i) };
        proof_assert! { rm_case(*pre, offset, slab_start, slot_idx) };
        proof_assert! { rm_post(*pre, *self, slab_start, slot_idx) };
        Ok(())
    }

    /// Mirror of `RegionState::flush_pending_frees` (region.rs:151-160).
    #[requires(rg_inv(*self))]
    #[requires(pending_ok(*self))]
    #[ensures(rg_inv(^self))]
    #[ensures(rg_frame(*self, ^self))]
    #[ensures((^self).pending_frees@.len() == 0 && (^self).dirty == self.dirty)]
    #[ensures(self.pending_frees@.len() == 0 ==> ^self == *self)]
    #[ensures(forall<s: u64, i: usize> self.pending_frees@ == Seq::singleton((s, i)) ==>
        free_case(*self, ^self, s, i, self.slabs@.lookup(s@)))]
    #[ensures(nonfull_listed(*self) ==> nonfull_listed(^self))]
    #[ensures(forall<s: u64, i: usize> self.pending_frees@ == Seq::singleton((s, i)) ==> flush1_tf(*self, ^self, s))]
    #[ensures(flushed_all(*self, ^self))]
    #[ensures(slabs_shrunk(*self, ^self))]
    #[ensures(fl_spec(*self, ^self))]
    pub fn flush_pending_frees(&mut self) {
        if self.pending_frees.len() == 0 {
            // `self.pending_frees.is_empty()` (region.rs:152)
            g_fl_init(snapshot! { *self }, snapshot! { *self }, snapshot! { self.pending_frees@ }); // PROOF-ONLY (ghost)
            return;
        }
        let pre = snapshot! { *self };
        let frees = std::mem::take(&mut self.pending_frees);
        let old = snapshot! { *self };
        proof_assert! { nonfull_listed(*pre) ==> nonfull_listed(*old) };
        proof_assert! { frees == pre.pending_frees && self.slabs == pre.slabs && self.size_classes == pre.size_classes && self.buddy == pre.buddy && self.format_params == pre.format_params };
        proof_assert! { forall<p: Int> 0 <= p && p < frees@.len() ==>
            pre.slabs@.contains(frees@[p].0@) && free_ok(*pre, frees@[p].0, frees@[p].1) };
        proof_assert! { forall<p: Int> 0 <= p && p < frees@.len() ==>
            self.slabs@.contains(frees@[p].0@) && free_ok(*self, frees@[p].0, frees@[p].1) };
        g_fl_init(pre, old, snapshot! { frees@ }); // PROOF-ONLY (ghost)
        let mut i: usize = 0;
        #[invariant(i@ <= frees@.len())]
        #[invariant(rg_inv(*self) && rg_frame(*old, *self))]
        #[invariant(self.pending_frees@.len() == 0 && self.dirty == old.dirty)]
        #[invariant(forall<p: Int> i@ <= p && p < frees@.len() ==>
            self.slabs@.contains(frees@[p].0@) && free_ok(*self, frees@[p].0, frees@[p].1))]
        #[invariant(forall<p: Int, q: Int> 0 <= p && p < q && q < frees@.len() ==> frees@[p] != frees@[q])]
        #[invariant(i@ == 0 ==> *self == *old)]
        #[invariant(nonfull_listed(*pre) ==> nonfull_listed(*self))]
        #[invariant(frees@.len() == 1 && i@ == 1 ==> free_case(*pre, *self, frees@[0].0, frees@[0].1, pre.slabs@.lookup(frees@[0].0@)))]
        #[invariant(frees@.len() == 1 && i@ == 1 ==> flush1_tf(*pre, *self, frees@[0].0))]
        #[invariant(slabs_shrunk(*pre, *self))]
        #[invariant(forall<p: Int> 0 <= p && p < i@ ==> slot_gone(*self, frees@[p].0, frees@[p].1))]
        #[invariant(fl_inv(*pre, *self, frees@, i@))]
        while i < frees.len() {
            let (slab_start, slot_idx) = frees[i];
            let cur = snapshot! { self.slabs@.lookup(slab_start@) };
            proof_assert! { forall<p: Int> i@ < p && p < frees@.len() && frees@[p].0 == slab_start ==> frees@[p].1 != slot_idx };
            proof_assert! { forall<p: Int> i@ < p && p < frees@.len() && frees@[p].0 == slab_start ==> {
                lemma_cnt_two(cur.bitmap, slot_idx@, frees@[p].1@, cur.bitmap.num_slots@);
                cur.bitmap.allocated_count@ >= 2 } };
            let before = snapshot! { *self };
            self.free_slot(slab_start, slot_idx);
            proof_assert! { free_case(*before, *self, slab_start, slot_idx, *cur) };
            proof_assert! { slot_gone(*self, slab_start, slot_idx) };
            proof_assert! { forall<p: Int> 0 <= p && p < i@ && before.slabs@.contains(frees@[p].0@) ==>
                pre.slabs@.contains(frees@[p].0@) && frees@[p].1@ < before.slabs@.lookup(frees@[p].0@).bitmap.num_slots@ };
            proof_assert! { forall<p: Int> 0 <= p && p < i@ ==> {
                lemma_gone_kept(*before, *self, slab_start, slot_idx, *cur, frees@[p].0, frees@[p].1);
                slot_gone(*self, frees@[p].0, frees@[p].1) } };
            proof_assert! { lemma_shrunk_step(*pre, *before, *self, slab_start, slot_idx, *cur); slabs_shrunk(*pre, *self) };
            proof_assert! { frees@.len() == 1 && i@ == 0 ==> before.slabs == pre.slabs && before.size_classes == pre.size_classes && before.buddy == pre.buddy && before.format_params == pre.format_params };
            proof_assert! { frees@.len() == 1 && i@ == 0 ==> {
                lemma_free_case_pending(*before, *pre, *self, slab_start, slot_idx, *cur);
                free_case(*pre, *self, frees@[0].0, frees@[0].1, pre.slabs@.lookup(frees@[0].0@)) } };
            proof_assert! { frees@.len() == 1 && i@ == 0 ==> flush1_tf(*pre, *self, frees@[0].0) };
            proof_assert! { forall<p: Int> i@ < p && p < frees@.len() ==>
                self.slabs@.contains(frees@[p].0@) && free_ok(*self, frees@[p].0, frees@[p].1) };
            proof_assert! { frees@[i@] == (slab_start, slot_idx) && before.slabs@.get(slab_start@) == Some(*cur) };
            g_fl_step(pre, before, snapshot! { *self }, snapshot! { frees@ }, snapshot! { i@ }, slab_start, slot_idx, cur); // PROOF-ONLY (ghost)
            i += 1;
        }
        g_fl_end(pre, snapshot! { *self }, snapshot! { frees@ }); // PROOF-ONLY (ghost)
    }
}

/// Slot `(s, i)` is released: its slab is gone or the slot's bit is clear (batch 5).
#[logic(open)]
pub fn slot_gone(r: RegionState, s: u64, i: usize) -> bool {
    pearlite! { !r.slabs@.contains(s@) || !slot_bit(r.slabs@.lookup(s@).bitmap, i@) }
}

/// Every deferred free queued in `a` is released in `b` (batch 5).
#[logic(open)]
pub fn flushed_all(a: RegionState, b: RegionState) -> bool {
    pearlite! { forall<p: Int> 0 <= p && p < a.pending_frees@.len() ==> slot_gone(b, a.pending_frees@[p].0, a.pending_frees@[p].1) }
}

/// No slab appears and every remaining slab keeps its geometry (batch 5).
#[logic(open)]
pub fn slabs_shrunk(a: RegionState, b: RegionState) -> bool {
    pearlite! {
        forall<k: Int> b.slabs@.contains(k) ==> a.slabs@.contains(k)
            && b.slabs@.lookup(k).start_offset == a.slabs@.lookup(k).start_offset
            && b.slabs@.lookup(k).element_size == a.slabs@.lookup(k).element_size
            && b.slabs@.lookup(k).slab_size == a.slabs@.lookup(k).slab_size
            && b.slabs@.lookup(k).bitmap.num_slots == a.slabs@.lookup(k).bitmap.num_slots
    }
}

/// A `free_slot(s, i)` keeps every already-released slot released.
#[logic]
#[requires(free_case(a, b, s, i, sl) && a.slabs@.get(s@) == Some(sl))]
#[requires(slot_gone(a, t, j))]
#[requires(a.slabs@.contains(t@) ==> j@ < a.slabs@.lookup(t@).bitmap.num_slots@)]
#[ensures(slot_gone(b, t, j))]
pub fn lemma_gone_kept(a: RegionState, b: RegionState, s: u64, i: usize, sl: Slab, t: u64, j: usize) {
    pearlite! {
        if sl.bitmap.allocated_count@ == 1 {
            proof_assert! { b.slabs@ == a.slabs@.remove(s@) }; ()
        } else {
            proof_assert! { b.slabs@ == a.slabs@.insert(s@, b.slabs@.lookup(s@)) };
            if t@ == s@ {
                if j@ == i@ { () } else { proof_assert! { a.slabs@.lookup(s@) == sl }; () }
            } else { () }
        }
    }
}

/// A `free_slot(s, i)` keeps `slabs_shrunk` from `p`.
#[logic]
#[requires(slabs_shrunk(p, a))]
#[requires(free_case(a, b, s, i, sl) && a.slabs@.get(s@) == Some(sl))]
#[requires(rg_inv(a) && rg_inv(b) && rg_frame(a, b))]
#[ensures(slabs_shrunk(p, b))]
pub fn lemma_shrunk_step(p: RegionState, a: RegionState, b: RegionState, s: u64, i: usize, sl: Slab) {
    pearlite! {
        if sl.bitmap.allocated_count@ == 1 {
            proof_assert! { b.slabs@ == a.slabs@.remove(s@) }; ()
        } else {
            proof_assert! { b.slabs@ == a.slabs@.insert(s@, b.slabs@.lookup(s@)) };
            proof_assert! { a.slabs@.lookup(s@) == sl };
            proof_assert! { b.slabs@.contains(s@) && slab_ok(b, b.slabs@.lookup(s@)) && slab_ok(a, sl) };
            proof_assert! { b.slabs@.lookup(s@).start_offset@ == s@ && sl.start_offset@ == s@ };
            proof_assert! { b.slabs@.lookup(s@).slab_size == b.format_params.slab_size && sl.slab_size == a.format_params.slab_size };
            ()
        }
    }
}

/// Free-space change of a single deferred free of slab `s` (batch 4): the slab's block
/// returns to the buddy allocator iff that free empties it (region.rs:104-107).
#[logic(open)]
pub fn flush1_tf(a: RegionState, b: RegionState, s: u64) -> bool {
    pearlite! {
        tf(b.buddy.free_lists@, a.buddy.sector_size@, b.buddy.free_lists@.len())
        == tf(a.buddy.free_lists@, a.buddy.sector_size@, a.buddy.free_lists@.len())
           + if a.slabs@.lookup(s@).bitmap.allocated_count@ == 1 { span(a.buddy.sector_size@, slab_k(a)) } else { 0 }
    }
}

/// No slot of `b` holds a non-FREE key that the same slot of `a` (same slab start, element
/// size and slot count bound) did not hold: an operation from `a` to `b` publishes nothing.
#[logic(open)]
pub fn no_new_keys(a: RegionState, b: RegionState) -> bool {
    pearlite! {
        forall<k: Int, j: Int> b.slabs@.contains(k) && 0 <= j && j < b.slabs@.lookup(k).bitmap.num_slots@
            && b.slabs@.lookup(k).keys@[j] != FREE_KEY ==>
            a.slabs@.contains(k) && j < a.slabs@.lookup(k).bitmap.num_slots@
            && a.slabs@.lookup(k).keys@[j] == b.slabs@.lookup(k).keys@[j]
            && a.slabs@.lookup(k).start_offset == b.slabs@.lookup(k).start_offset
            && a.slabs@.lookup(k).element_size == b.slabs@.lookup(k).element_size
    }
}

#[logic]
#[requires(no_new_keys(a, b) && no_new_keys(b, c))]
#[ensures(no_new_keys(a, c))]
pub fn lemma_nnk_trans(a: RegionState, b: RegionState, c: RegionState) {}

/// Total free bytes of the region's buddy allocator (buddy.rs total_free).
#[logic(open)]
pub fn rg_tf(r: RegionState) -> Int {
    pearlite! { tf(r.buddy.free_lists@, r.buddy.sector_size@, r.buddy.free_lists@.len()) }
}

/// A failed `alloc_extent` (batch 5): the slab map, the free space, the deferred frees and the
/// dirty flag are as before (only stale size-class entries may have been dropped).
#[logic(open)]
pub fn err_frame(a: RegionState, b: RegionState) -> bool {
    pearlite! { b.slabs@ == a.slabs@ && rg_tf(b) == rg_tf(a) && b.pending_frees == a.pending_frees && b.dirty == a.dirty }
}

/// Slab `k` of element size `es` has a free slot (batch 5).
#[logic(open)]
pub fn free_at(r: RegionState, k: Int, es: Int) -> bool {
    pearlite! {
        r.slabs@.contains(k) && r.slabs@.lookup(k).element_size@ == es
        && r.slabs@.lookup(k).bitmap.allocated_count@ < r.slabs@.lookup(k).bitmap.num_slots@
    }
}

/// `k` occurs in the offset list `sq`.
#[logic(open)]
pub fn listed(sq: Seq<u64>, k: Int) -> bool {
    pearlite! { exists<p: Int> 0 <= p && p < sq.len() && sq[p]@ == k }
}

/// EM-SIZECLASS-NONFULL-LISTED: every slab with a free slot is listed under its own
/// element size.
#[logic(open)]
pub fn nonfull_listed(r: RegionState) -> bool {
    pearlite! {
        forall<k: Int> r.slabs@.contains(k)
            && r.slabs@.lookup(k).bitmap.allocated_count@ < r.slabs@.lookup(k).bitmap.num_slots@ ==>
            listed(sc_get(r.size_classes, r.slabs@.lookup(k).element_size@), k)
    }
}

#[logic]
#[requires(listed(sq, k))]
#[ensures(listed(sq.push_back(a), k))]
pub fn lemma_listed_push(sq: Seq<u64>, a: u64, k: Int) {}

#[logic]
#[ensures(listed(sq.push_back(a), a@))]
pub fn lemma_listed_push_self(sq: Seq<u64>, a: u64) {
    pearlite! { proof_assert! { sq.push_back(a)[sq.len()] == a }; () }
}

/// Removing every `x` keeps every other listed offset.
#[logic]
#[variant(sq.len())]
#[requires(listed(sq, k) && x@ != k)]
#[ensures(listed(filter_ne(sq, x), k))]
pub fn lemma_listed_filter(sq: Seq<u64>, x: u64, k: Int) {
    pearlite! {
        if sq.len() > 0 {
            let pre = sq.subsequence(0, sq.len() - 1);
            if sq[sq.len() - 1]@ == k {
                lemma_listed_push_self(filter_ne(pre, x), sq[sq.len() - 1])
            } else {
                proof_assert! { listed(pre, k) };
                lemma_listed_filter(pre, x, k);
                if sq[sq.len() - 1] == x { () } else { lemma_listed_push(filter_ne(pre, x), sq[sq.len() - 1], k) }
            }
        }
    }
}

/// nonfull_listed reads only the slab map and the size classes.
#[logic]
#[requires(nonfull_listed(a) && a.slabs == b.slabs && a.size_classes == b.size_classes)]
#[ensures(nonfull_listed(b))]
pub fn lemma_nf_frame(a: RegionState, b: RegionState) {}

/// nonfull_listed survives replacing slab `s` by `pb` that has no more free slots than
/// the old `pa` (same element size and slot count), size classes unchanged.
#[logic]
#[requires(nonfull_listed(it))]
#[requires(it.slabs@.contains(s@) && it.slabs@.lookup(s@) == pa)]
#[requires(new.slabs@ == it.slabs@.insert(s@, pb) && new.size_classes == it.size_classes)]
#[requires(pb.element_size == pa.element_size && pb.bitmap.num_slots == pa.bitmap.num_slots)]
#[requires(pa.bitmap.allocated_count@ <= pb.bitmap.allocated_count@)]
#[ensures(nonfull_listed(new))]
pub fn lemma_nf_update(it: RegionState, new: RegionState, s: u64, pa: Slab, pb: Slab) {}

/// nonfull_listed after a successful `alloc_slot` on the listed slab `s` (region.rs:51-58).
#[logic]
#[requires(nonfull_listed(it))]
#[requires(it.slabs@.contains(s@) && it.slabs@.lookup(s@) == pa)]
#[requires(new.slabs@ == it.slabs@.insert(s@, pb))]
#[requires(pb.element_size == pa.element_size && pb.bitmap.num_slots == pa.bitmap.num_slots)]
#[requires(pb.bitmap.allocated_count@ == pa.bitmap.allocated_count@ + 1 && pb.bitmap.allocated_count@ <= pb.bitmap.num_slots@)]
#[requires((pb.bitmap.allocated_count@ == pb.bitmap.num_slots@
        && forall<e: Int> sc_get(new.size_classes, e) == if e == es { filter_ne(sc_get(it.size_classes, e), s) } else { sc_get(it.size_classes, e) })
    || (pb.bitmap.allocated_count@ != pb.bitmap.num_slots@ && new.size_classes == it.size_classes))]
#[ensures(nonfull_listed(new))]
pub fn lemma_nf_success(it: RegionState, new: RegionState, s: u64, pa: Slab, pb: Slab, es: Int) {
    pearlite! {
        proof_assert! { forall<k: Int> new.slabs@.contains(k) && k != s@
            && new.slabs@.lookup(k).bitmap.allocated_count@ < new.slabs@.lookup(k).bitmap.num_slots@ ==> {
                lemma_listed_filter(sc_get(it.size_classes, new.slabs@.lookup(k).element_size@), s, k);
                listed(sc_get(new.size_classes, new.slabs@.lookup(k).element_size@), k) } };
        ()
    }
}

/// nonfull_listed after removing the stale entry `s` from size class `es` (region.rs:61):
/// the slab at `s` is missing or full.
#[logic]
#[requires(nonfull_listed(it))]
#[requires(forall<k: Int> new.slabs@.contains(k) == it.slabs@.contains(k))]
#[requires(forall<k: Int> new.slabs@.contains(k) ==> new.slabs@.lookup(k) == it.slabs@.lookup(k))]
#[requires(!it.slabs@.contains(s@) || it.slabs@.lookup(s@).bitmap.allocated_count@ == it.slabs@.lookup(s@).bitmap.num_slots@)]
#[requires(forall<e: Int> sc_get(new.size_classes, e) == if e == es { filter_ne(sc_get(it.size_classes, e), s) } else { sc_get(it.size_classes, e) })]
#[ensures(nonfull_listed(new))]
pub fn lemma_nf_stale(it: RegionState, new: RegionState, s: u64, es: Int) {
    pearlite! {
        proof_assert! { forall<k: Int> new.slabs@.contains(k)
            && new.slabs@.lookup(k).bitmap.allocated_count@ < new.slabs@.lookup(k).bitmap.num_slots@ ==> {
                lemma_listed_filter(sc_get(it.size_classes, new.slabs@.lookup(k).element_size@), s, k);
                listed(sc_get(new.size_classes, new.slabs@.lookup(k).element_size@), k) } };
        ()
    }
}

/// nonfull_listed after inserting the fresh slab `sl` at `d` (region.rs:84-89).
#[logic]
#[requires(nonfull_listed(it))]
#[requires(new.slabs@ == it.slabs@.insert(d@, sl))]
#[requires(sl.element_size@ == es)]
#[requires(forall<e: Int> sc_get(new.size_classes, e) ==
    if e == es && sl.bitmap.allocated_count@ < sl.bitmap.num_slots@ { sc_get(it.size_classes, e).push_back(d) } else { sc_get(it.size_classes, e) })]
#[ensures(nonfull_listed(new))]
pub fn lemma_nf_fresh(it: RegionState, new: RegionState, d: u64, sl: Slab, es: Int) {
    pearlite! {
        proof_assert! { sl.bitmap.allocated_count@ < sl.bitmap.num_slots@ ==> {
            lemma_listed_push_self(sc_get(it.size_classes, es), d); listed(sc_get(new.size_classes, es), d@) } };
        proof_assert! { forall<k: Int, e: Int> listed(sc_get(it.size_classes, e), k) ==> {
            lemma_listed_push(sc_get(it.size_classes, e), d, k);
            listed(sc_get(new.size_classes, e), k) } };
        ()
    }
}

/// `free_case` reads only slabs, size classes and the buddy allocator of its `old` state.
#[logic]
#[requires(a.slabs == b.slabs && a.size_classes == b.size_classes && a.buddy == b.buddy && a.format_params == b.format_params)]
#[requires(free_case(a, n, s, i, sl))]
#[ensures(free_case(b, n, s, i, sl))]
pub fn lemma_free_case_pending(a: RegionState, b: RegionState, n: RegionState, s: u64, i: usize, sl: Slab) {}

/// Distinct slots of a slab have distinct offsets.
#[logic]
#[requires(sl.element_size@ > 0 && slot_off(sl, i) == slot_off(sl, j))]
#[ensures(i == j)]
pub fn lemma_slot_off_inj(sl: Slab, i: Int, j: Int) {}

/// `remove_extent_by_offset(offset)` finds slot `i` of the slab at `s`: `s` is the greatest
/// slab start <= offset, the slot is allocated, holds a published key, and starts at offset.
#[logic(open)]
pub fn rm_case(r: RegionState, offset: u64, s: u64, i: usize) -> bool {
    pearlite! {
        r.slabs@.contains(s@) && s@ <= offset@
        && (forall<t: Int> r.slabs@.contains(t) && t <= offset@ ==> t <= s@)
        && i@ < r.slabs@.lookup(s@).bitmap.num_slots@
        && slot_off(r.slabs@.lookup(s@), i@) == offset@
        && slot_bit(r.slabs@.lookup(s@).bitmap, i@)
        && r.slabs@.lookup(s@).keys@[i@] != FREE_KEY
    }
}

/// A successful `remove_extent_by_offset(offset)` (batch 5): some slot `(s, i)` matched
/// `rm_case` and the region changed exactly as `rm_post` says.
#[logic(open)]
pub fn rm_step(a: RegionState, b: RegionState, offset: u64) -> bool {
    pearlite! { exists<s: u64, i: usize> rm_case(a, offset, s, i) && rm_post(a, b, s, i) }
}

/// The exact effect of that removal (region.rs:142-147): the key becomes FREE_KEY, the slot
/// stays allocated, and `(s, i)` is queued as a deferred free.
#[logic(open)]
pub fn rm_post(r: RegionState, n: RegionState, s: u64, i: usize) -> bool {
    pearlite! {
        n.slabs@ == r.slabs@.insert(s@, n.slabs@.lookup(s@))
        && n.slabs@.lookup(s@).keys@ == r.slabs@.lookup(s@).keys@.set(i@, FREE_KEY)
        && n.slabs@.lookup(s@).bitmap == r.slabs@.lookup(s@).bitmap
        && n.slabs@.lookup(s@).start_offset == r.slabs@.lookup(s@).start_offset
        && n.slabs@.lookup(s@).element_size == r.slabs@.lookup(s@).element_size
        && n.slabs@.lookup(s@).slab_size == r.slabs@.lookup(s@).slab_size
        && n.pending_frees@ == r.pending_frees@.push_back((s, i))
        && n.size_classes == r.size_classes && n.buddy == r.buddy && n.dirty == true
    }
}

/// `exist_case`'s condition: the first listed slab exists and has a free slot.
#[logic(open)]
pub fn exist_cond(old: RegionState, es: Int, s0: u64) -> bool {
    pearlite! {
        old.slabs@.contains(s0@)
        && old.slabs@.lookup(s0@).bitmap.allocated_count@ < old.slabs@.lookup(s0@).bitmap.num_slots@
    }
}

/// (C) the first slab of size class `es` has a free slot: it is used.
#[logic(open)]
pub fn exist_case(old: RegionState, new: RegionState, es: Int, s0: u64, result: Result<(u64, usize, u64), EmError>) -> bool {
    pearlite! {
        exist_cond(old, es, s0) ==>
        exists<i: usize, off: u64> result == Ok((s0, i, off))
            && new.buddy == old.buddy
            && new.slabs@ == old.slabs@.insert(s0@, new.slabs@.lookup(s0@))
            && {
                let a = old.slabs@.lookup(s0@);
                let b = new.slabs@.lookup(s0@);
                i@ < a.bitmap.num_slots@ && !slot_bit(a.bitmap, i@) && slot_bit(b.bitmap, i@)
                && (forall<j: Int> 0 <= j && j < a.bitmap.num_slots@ && j != i@ ==> slot_bit(b.bitmap, j) == slot_bit(a.bitmap, j))
                && b.keys == a.keys && b.start_offset == a.start_offset && b.slab_size == a.slab_size
                && b.element_size == a.element_size && b.bitmap.num_slots == a.bitmap.num_slots
                && b.bitmap.allocated_count@ == a.bitmap.allocated_count@ + 1
                && off@ == slot_off(a, i@)
                && (forall<e: Int> sc_get(new.size_classes, e) ==
                    if e == es && b.bitmap.allocated_count@ == b.bitmap.num_slots@ { filter_ne(sc_get(old.size_classes, e), s0) }
                    else { sc_get(old.size_classes, e) })
            }
    }
}

/// (B) the size class is empty: a fresh slab is carved from the buddy allocator.
#[logic(open)]
pub fn fresh_case(old: RegionState, new: RegionState, es: Int, result: Result<(u64, usize, u64), EmError>) -> bool {
    pearlite! {
        let fo = first_ne(old.buddy.free_lists@, slab_k(old), old.buddy.max_order@);
        (fo == -1 ==> result == Err(EmError::OutOfSpace) && new == old)
        && (fo != -1 ==> exists<d: u64> result == Ok((d, 0usize, d))
            && alloc_post(old.buddy, new.buddy, slab_k(old), fo, d@)
            && new.slabs@ == old.slabs@.insert(d@, new.slabs@.lookup(d@))
            && fresh_one(new.slabs@.lookup(d@), d@, old.format_params.slab_size@, es)
            && new.slabs@.lookup(d@).rover@ == 1 % slots_of(old.format_params.slab_size@, es)
            && (forall<e: Int> sc_get(new.size_classes, e) ==
                if e == es && slots_of(old.format_params.slab_size@, es) > 1 { sc_get(old.size_classes, e).push_back(d) }
                else { sc_get(old.size_classes, e) }))
    }
}

/// The exact effect of `free_slot` on an existing slab `sl` (region.rs:93-111).
#[logic(open)]
pub fn free_case(old: RegionState, new: RegionState, s: u64, i: usize, sl: Slab) -> bool {
    pearlite! {
        let es = sl.element_size@;
        let empty = sl.bitmap.allocated_count@ == 1;
        let was_full = sl.bitmap.allocated_count@ == sl.bitmap.num_slots@;
        (empty ==>
            new.slabs@ == old.slabs@.remove(s@)
            && (forall<e: Int> sc_get(new.size_classes, e) == if e == es { filter_ne(sc_get(old.size_classes, e), s) } else { sc_get(old.size_classes, e) })
            && (old.buddy.free_lists@[slab_k(old)]@.len() == 0 || slab_k(old) == old.buddy.max_order@ ==>
                same_except(new.buddy.free_lists@, old.buddy.free_lists@, slab_k(old))
                && pushed(new.buddy.free_lists@[slab_k(old)]@, old.buddy.free_lists@[slab_k(old)]@, s@ - old.buddy.base_offset@)))
        && (!empty ==>
            new.buddy == old.buddy
            && new.slabs@ == old.slabs@.insert(s@, new.slabs@.lookup(s@))
            && new.slabs@.lookup(s@).keys@ == sl.keys@.set(i@, FREE_KEY)
            && !slot_bit(new.slabs@.lookup(s@).bitmap, i@)
            && (forall<j: Int> 0 <= j && j < sl.bitmap.num_slots@ && j != i@ ==> slot_bit(new.slabs@.lookup(s@).bitmap, j) == slot_bit(sl.bitmap, j))
            && new.slabs@.lookup(s@).bitmap.allocated_count@ == sl.bitmap.allocated_count@ - 1
            && new.slabs@.lookup(s@).bitmap.num_slots == sl.bitmap.num_slots
            && new.slabs@.lookup(s@).element_size == sl.element_size
            && (forall<e: Int> sc_get(new.size_classes, e) == if e == es && was_full { sc_get(old.size_classes, e).push_back(s) } else { sc_get(old.size_classes, e) }))
    }
}

/// `(n + d - 1) / d == n / d` for an exact multiple.
#[logic]
#[requires(d > 0 && n >= 0 && n % d == 0)]
#[ensures((n + d - 1) / d == n / d)]
pub fn lemma_div_exact(n: Int, d: Int) {
    lemma_divmod(n, d);
    proof_assert! { n == d * (n / d) };
    lemma_divmod(n + d - 1, d)
}

/// `n <= 2^ord(n/d) * d` for an exact multiple `n` of `d`.
#[logic]
#[requires(d > 0 && n >= 0 && n % d == 0 && n <= 18446744073709551615)]
#[ensures(n <= span(d, ord(n / d)))]
pub fn lemma_fits(n: Int, d: Int) {
    lemma_ord_ge(n / d);
    lemma_divmod(n, d);
    lemma_mul_le(n / d, ord(n / d).pow2(), d)
}

/// A slab fits in its buddy block: `slab_size <= 2^K * ss`.
#[logic]
#[requires(rg_params(r))]
#[ensures(r.format_params.slab_size@ <= span(r.buddy.sector_size@, slab_k(r)))]
pub fn lemma_slab_fits(r: RegionState) {
    pearlite! {
        lemma_ord_ge(r.format_params.slab_size@ / r.format_params.sector_size@);
        lemma_divmod(r.format_params.slab_size@, r.format_params.sector_size@);
        lemma_mul_le(r.format_params.slab_size@ / r.format_params.sector_size@,
                     ord(r.format_params.slab_size@ / r.format_params.sector_size@).pow2(),
                     r.format_params.sector_size@)
    }
}

/// A checkpointed slab descriptor that is well-formed for a region of geometry
/// (`base`, `size`, `fp`), in the SPAN form the rebuild uses. Level 3 (phase F): this is a
/// DERIVED predicate, NOT an assumption — `lemma_desc_ok_a` (model/assume.rs) proves it from
/// `desc_ok_a` (declared 980f61 + 3b612e, plus the UNDECLARED slot-count residue
/// `u_desc_slots_u32`) under 7cb7c3 and `slab % ss == 0`. A driver premise `desc_ok(..)` is a
/// narrowing unless derived that way.
#[logic(open)]
pub fn desc_ok(d: SlabDescriptor, base: Int, size: Int, fp: FormatParams) -> bool {
    pearlite! {
        d.slab_size == fp.slab_size
        && d.element_size@ > 0
        && d.keys@.len() == slots_of(d.slab_size@, d.element_size@)
        && d.start_offset@ >= base
        && (d.start_offset@ - base) % span(fp.sector_size@, ord(fp.slab_size@ / fp.sector_size@)) == 0
        && d.start_offset@ - base + span(fp.sector_size@, ord(fp.slab_size@ / fp.sector_size@)) <= size
    }
}

/// Mirror of `recovery::slab_from_descriptor` (recovery.rs:76-85).
#[requires(desc.element_size@ > 0)]
#[requires(desc.keys@.len() == slots_of(desc.slab_size@, desc.element_size@))]
#[requires(desc.start_offset@ + desc.slab_size@ <= u64::MAX@)]
#[ensures(slab_inv(result) && key_alloc(result))]
#[ensures(result.start_offset == desc.start_offset && result.slab_size == desc.slab_size)]
#[ensures(result.element_size == desc.element_size && result.keys@ == desc.keys@ && result.rover@ == 0)]
#[ensures(forall<i: Int> 0 <= i && i < result.bitmap.num_slots@ ==> slot_bit(result.bitmap, i) == (desc.keys@[i] != FREE_KEY))]
pub fn slab_from_descriptor(desc: &SlabDescriptor) -> Slab {
    let mut slab = Slab::new(desc.start_offset, desc.slab_size, desc.element_size);
    let mut i: usize = 0;
    // `for (i, &key) in desc.keys.iter().enumerate()` (recovery.rs:78)
    #[invariant(i@ <= desc.keys@.len())]
    #[invariant(slab_inv(slab) && key_alloc(slab) && slab.rover@ == 0)]
    #[invariant(slab.start_offset == desc.start_offset && slab.slab_size == desc.slab_size && slab.element_size == desc.element_size)]
    #[invariant(slab.bitmap.num_slots@ == desc.keys@.len())]
    #[invariant(forall<j: Int> 0 <= j && j < i@ ==> slab.keys@[j] == desc.keys@[j])]
    #[invariant(forall<j: Int> i@ <= j && j < slab.keys@.len() ==> slab.keys@[j] == FREE_KEY)]
    #[invariant(forall<j: Int> 0 <= j && j < i@ ==> slot_bit(slab.bitmap, j) == (desc.keys@[j] != FREE_KEY))]
    #[invariant(forall<j: Int> i@ <= j && j < slab.bitmap.num_slots@ ==> !slot_bit(slab.bitmap, j))]
    while i < desc.keys.len() {
        let key = desc.keys[i];
        if key != FREE_KEY {
            slab.mark_slot_allocated(i);
            slab.set_key(i, key);
        }
        i += 1;
    }
    proof_assert! { slab.keys@.ext_eq(desc.keys@) };
    slab
}

/// Descriptor start offsets are pairwise distinct (true of the slabs of one BTreeMap that
/// serialize_region wrote, checkpoint.rs:21).
#[logic(open)]
pub fn starts_distinct(ds: Seq<SlabDescriptor>) -> bool {
    pearlite! { forall<p: Int, q: Int> 0 <= p && p < q && q < ds.len() ==> ds[p].start_offset != ds[q].start_offset }
}

/// Some descriptor of `ds` below `n` starts at `k`.
#[logic(open)]
pub fn has_desc(ds: Seq<SlabDescriptor>, n: Int, k: Int) -> bool {
    pearlite! { exists<p: Int> 0 <= p && p < n && ds[p].start_offset@ == k }
}

/// Slab `sl` is exactly what slab_from_descriptor builds from `d`.
#[logic(open)]
pub fn slab_is_desc(sl: Slab, d: SlabDescriptor) -> bool {
    pearlite! {
        sl.start_offset == d.start_offset && sl.slab_size == d.slab_size && sl.element_size == d.element_size
        && sl.keys@ == d.keys@ && sl.bitmap.num_slots@ == d.keys@.len()
        && forall<i: Int> 0 <= i && i < sl.bitmap.num_slots@ ==> slot_bit(sl.bitmap, i) == (d.keys@[i] != FREE_KEY)
    }
}

/// The slab map of `r` is exactly the descriptors `ds`, one slab per descriptor at its start.
#[logic(open)]
pub fn rebuilt_all(r: RegionState, ds: Seq<SlabDescriptor>) -> bool {
    pearlite! {
        (forall<k: Int> r.slabs@.contains(k) ==> has_desc(ds, ds.len(), k))
        && (forall<p: Int> 0 <= p && p < ds.len() ==>
            r.slabs@.contains(ds[p].start_offset@) && slab_is_desc(r.slabs@.lookup(ds[p].start_offset@), ds[p]))
    }
}

/// Mirror of the per-region rebuild in `initialize` (lib.rs:552-565) for region
/// `base`/`size` and its recovered descriptors `descs`.
#[requires(fp.sector_size@ > 0 && fp.slab_size@ > 0 && fp.slab_size@ % fp.sector_size@ == 0)]
#[requires(fp.slab_size@ + fp.sector_size@ <= u64::MAX@)]
#[requires(base@ + size@ <= u64::MAX@)]
#[requires(2 * size@ + fp.sector_size@ <= u64::MAX@)]
#[requires(forall<p: Int> 0 <= p && p < descs@.len() ==> desc_ok(descs@[p], base@, size@, fp))]
#[ensures(rg_inv(result))]
#[ensures(result.format_params == fp && result.dirty == false && result.pending_frees@.len() == 0)]
#[ensures(result.buddy.base_offset == base && result.buddy.total_usable_size == size)]
#[ensures(result.buddy.sector_size == fp.sector_size)]
#[ensures(descs@.len() == 1 ==> rebuilt_one(result, descs@[0], base@, size@, fp))]
#[ensures(nonfull_listed(result))]
#[ensures(descs@.len() == 1 ==> rebuilt_hit(result, descs@[0], base@, size@, fp))]
#[ensures(starts_distinct(descs@) ==> rebuilt_all(result, descs@))]
#[ensures(p2(fp.sector_size@) && starts_distinct(descs@) ==> rg_good_o(result))]
pub fn rebuild_region(base: u64, size: u64, fp: FormatParams, descs: &Vec<SlabDescriptor>) -> RegionState {
    let mut buddy = BuddyAllocator::new(base, size, fp.sector_size);
    let b0 = snapshot! { buddy };
    let g = snapshot! { p2(fp.sector_size@) && starts_distinct(descs@) };
    g_rb_init(b0, snapshot! { descs@ }, snapshot! { base@ }, snapshot! { size@ }, snapshot! { fp }); // PROOF-ONLY (ghost)
    let mut i: usize = 0;
    // `for desc in &slab_descs { buddy.mark_allocated(desc.start_offset, desc.slab_size) }` (lib.rs:553-555)
    #[invariant(i@ <= descs@.len())]
    #[invariant(bd_inv(buddy) && buddy.base_offset == base && buddy.total_usable_size == size)]
    #[invariant(buddy.sector_size == fp.sector_size && buddy.max_order == b0.max_order)]
    #[invariant(descs@.len() == 1 && i@ == 0 ==> buddy == *b0)]
    #[invariant(forall<m: Int> descs@.len() == 1 && i@ == 1 && mark_noop_p2(descs@[0], base@, size@, fp, m) ==> buddy.free_lists == b0.free_lists)]
    #[invariant(forall<m: Int> descs@.len() == 1 && i@ == 1 && mark_hit_p2(descs@[0], base@, size@, fp, m) ==> hit_lists(buddy, descs@[0], base@, fp, m))]
    #[invariant(*g ==> rb1(buddy, *b0, descs@, i@, base@, size@, fp))]
    while i < descs.len() {
        proof_assert! { lemma_desc_k(descs@[i@], *b0, base@, size@, fp); ord(descs@[i@].slab_size@ / buddy.sector_size@) <= buddy.max_order@ };
        proof_assert! { descs@.len() == 1 && i@ == 0 ==> buddy == *b0 };
        proof_assert! { forall<m: Int> descs@.len() == 1 && i@ == 0 && mark_noop_p2(descs@[0], base@, size@, fp, m) ==> {
            lemma_pow2(m); lemma_top_unique(size@ / fp.sector_size@, m, b0.max_order@);
            b0.max_order@ == m } };
        proof_assert! { forall<m: Int> descs@.len() == 1 && i@ == 0 && mark_noop_p2(descs@[0], base@, size@, fp, m) ==>
            buddy.free_lists@[ord(descs@[0].slab_size@ / buddy.sector_size@)]@.len() == 0 };
        proof_assert! { forall<m: Int> descs@.len() == 1 && i@ == 0 && mark_noop_p2(descs@[0], base@, size@, fp, m) ==>
            forall<q: Int> 0 <= q && q < buddy.free_lists@[ord(descs@[0].slab_size@ / buddy.sector_size@)]@.len() ==>
                buddy.free_lists@[ord(descs@[0].slab_size@ / buddy.sector_size@)]@[q]@ != descs@[0].start_offset@ - buddy.base_offset@ };
        proof_assert! { forall<m: Int> descs@.len() == 1 && i@ == 0 && mark_noop_p2(descs@[0], base@, size@, fp, m) ==>
            forall<sk: Int, sp: u64, x: u64> ord(descs@[0].slab_size@ / buddy.sector_size@) < sk && sk <= buddy.max_order@
                && sp@ == span(buddy.sector_size@, sk) && x@ == descs@[0].start_offset@ - buddy.base_offset@ ==>
                forall<q: Int> 0 <= q && q < buddy.free_lists@[sk]@.len() ==> buddy.free_lists@[sk]@[q] != (x & !(sp - 1u64)) };
        proof_assert! { forall<m: Int> descs@.len() == 1 && i@ == 0 && mark_hit_p2(descs@[0], base@, size@, fp, m) ==> {
            lemma_pow2(m); lemma_top_unique(size@ / fp.sector_size@, m, b0.max_order@);
            b0.max_order@ == m && b0.free_lists@[m]@ == Seq::singleton(0u64) && b0.free_lists@[m - 1]@.len() == 0 } };
        proof_assert! { forall<m: Int, x: u64> descs@.len() == 1 && i@ == 0 && mark_hit_p2(descs@[0], base@, size@, fp, m)
            && x@ == descs@[0].start_offset@ - buddy.base_offset@ ==>
            split1_pre(buddy, x, ord(descs@[0].slab_size@ / buddy.sector_size@), 0u64) };
        let bpre = snapshot! { buddy };
        // proof-only: the relative offset mark_allocated computes (buddy.rs:118), as a witness term
        let xo: u64 = descs[i].start_offset - base;
        buddy.mark_allocated(descs[i].start_offset, descs[i].slab_size);
        proof_assert! { xo@ == descs@[i@].start_offset@ - base@ };
        g_rb_step(bpre, snapshot! { buddy }, b0, snapshot! { descs@ }, snapshot! { i@ }, snapshot! { base@ }, snapshot! { size@ },
            snapshot! { fp }, g); // PROOF-ONLY (ghost)
        proof_assert! { forall<m: Int, x: u64> descs@.len() == 1 && i@ == 0 && mark_hit_p2(descs@[0], base@, size@, fp, m)
            && x@ == descs@[0].start_offset@ - base@ ==>
            split1_post(*bpre, buddy, x, m - 1, 0u64) && bpre.free_lists@.len() == m + 1 };
        proof_assert! { forall<m: Int> descs@.len() == 1 && i@ == 0 && mark_hit_p2(descs@[0], base@, size@, fp, m) ==>
            split1_post(*bpre, buddy, xo, m - 1, 0u64) && split1_val(*bpre, xo, m - 1, 0u64) == (if xo@ >= span(fp.sector_size@, m - 1) { 0 } else { span(fp.sector_size@, m - 1) }) };
        proof_assert! { forall<m: Int> descs@.len() == 1 && i@ == 0 && mark_hit_p2(descs@[0], base@, size@, fp, m) ==> hit_lists(buddy, descs@[0], base@, fp, m) };
        i += 1;
    }

    let b1 = snapshot! { buddy };
    proof_assert! { lemma_span_pos(fp.sector_size@, buddy.max_order@); span(fp.sector_size@, buddy.max_order@) > 0 };
    proof_assert! { buddy.max_order@ == 0 ==> span(fp.sector_size@, 0) == fp.sector_size@ };
    let mut region = RegionState::new(buddy, fp);
    g_rb2_init(snapshot! { region }, snapshot! { descs@ }); // PROOF-ONLY (ghost)

    let mut i: usize = 0;
    #[invariant(i@ <= descs@.len())]
    #[invariant(rg_inv(region))]
    #[invariant(region.format_params == fp && region.dirty == false && region.pending_frees@.len() == 0)]
    #[invariant(region.buddy == *b1)]
    #[invariant(descs@.len() == 1 && i@ == 0 ==> region.slabs@ == FMap::empty()
        && forall<e: Int> sc_get(region.size_classes, e) == Seq::empty())]
    #[invariant(descs@.len() == 1 && i@ == 1 ==> one_slab(region, descs@[0]))]
    #[invariant(nonfull_listed(region))]
    #[invariant(forall<k: Int> region.slabs@.contains(k) ==> has_desc(descs@, i@, k))]
    #[invariant(starts_distinct(descs@) ==> forall<p: Int> 0 <= p && p < i@ ==>
        region.slabs@.contains(descs@[p].start_offset@) && slab_is_desc(region.slabs@.lookup(descs@[p].start_offset@), descs@[p]))]
    #[invariant(*g ==> rb2(region, descs@, i@))]
    while i < descs.len() {
        proof_assert! { lemma_fits(fp.slab_size@, fp.sector_size@); fp.slab_size@ <= span(fp.sector_size@, ord(fp.slab_size@ / fp.sector_size@)) };
        proof_assert! { descs@[i@].start_offset@ + descs@[i@].slab_size@ <= u64::MAX@ };
        let slab = slab_from_descriptor(&descs[i]);
        let element_size = slab.element_size;
        let start_offset = slab.start_offset;
        proof_assert! { lemma_desc_k(descs@[i@], *b0, base@, size@, fp); slab_ok(region, slab) };
        let pre = snapshot! { region };
        let sl = snapshot! { slab };
        region.size_classes.add_slab(element_size, start_offset);
        proof_assert! { lemma_listed_push_self(sc_get(pre.size_classes, element_size@), start_offset);
            listed(sc_get(region.size_classes, element_size@), start_offset@) };
        proof_assert! { forall<k: Int, e: Int> listed(sc_get(pre.size_classes, e), k) ==> {
            lemma_listed_push(sc_get(pre.size_classes, e), start_offset, k);
            listed(sc_get(region.size_classes, e), k) } };
        let pre_ins = snapshot! { region };
        region.slabs.insert(start_offset, slab);
        proof_assert! { region.slabs@.lookup(start_offset@) == *sl && sl.element_size == element_size };
        proof_assert! { slab_is_desc(*sl, descs@[i@]) };
        proof_assert! { forall<k: Int> region.slabs@.contains(k) ==> k == start_offset@ || pre_ins.slabs@.contains(k) };
        proof_assert! { forall<k: Int> region.slabs@.contains(k) ==> has_desc(descs@, i@ + 1, k) };
        proof_assert! { starts_distinct(descs@) ==> forall<p: Int> 0 <= p && p < i@ ==>
            descs@[p].start_offset@ != start_offset@ && region.slabs@.lookup(descs@[p].start_offset@) == pre_ins.slabs@.lookup(descs@[p].start_offset@) };
        proof_assert! { nonfull_listed(region) };
        proof_assert! { descs@.len() == 1 && i@ == 0 ==> region.slabs@ == FMap::empty().insert(start_offset@, *sl) };
        proof_assert! { descs@.len() == 1 && i@ == 0 ==> region.slabs@.contains(start_offset@) && region.slabs@.lookup(start_offset@) == *sl };
        proof_assert! { descs@.len() == 1 && i@ == 0 ==> forall<k: Int> region.slabs@.contains(k) ==> k == start_offset@ };
        proof_assert! { Seq::<u64>::empty().push_back(start_offset).ext_eq(Seq::singleton(start_offset)) };
        proof_assert! { descs@.len() == 1 && i@ == 0 ==> forall<e: Int> sc_get(region.size_classes, e) ==
            if e == element_size@ { Seq::singleton(start_offset) } else { Seq::empty() } };
        proof_assert! { descs@.len() == 1 && i@ == 0 ==> one_slab(region, descs@[0]) };
        proof_assert! { forall<k: Int> region.slabs@.contains(k) && k != start_offset@ ==> pre_ins.slabs@.contains(k) && region.slabs@.lookup(k) == pre_ins.slabs@.lookup(k) };
        proof_assert! { slab_ok(region, *sl) && sl.start_offset == start_offset };
        proof_assert! { rg_params(region) && rg_buddy(region) && rg_slabs(region) && rg_inv(region) };
        g_rb2_step(pre, pre_ins, snapshot! { region }, snapshot! { descs@ }, snapshot! { i@ }, sl, g); // PROOF-ONLY (ghost)
        i += 1;
    }
    proof_assert! { descs@.len() == 1 ==> one_slab(region, descs@[0]) };
    proof_assert! { region.buddy == *b1 };
    proof_assert! { forall<m: Int> descs@.len() == 1 && mark_noop_p2(descs@[0], base@, size@, fp, m) ==>
        region.buddy.free_lists == b0.free_lists };
    proof_assert! { descs@.len() == 1 ==> rebuilt_one(region, descs@[0], base@, size@, fp) };
    g_rb_final(snapshot! { region }, b0, snapshot! { descs@ }, snapshot! { base@ }, snapshot! { size@ }, snapshot! { fp }, g); // PROOF-ONLY (ghost)
    region
}

/// For a power-of-two region (`size/ss == 2^m`, one free block `[0]` at order `m`),
/// `mark_allocated` of descriptor `d` (block order `t < m`) finds nothing when the
/// mask `x & !(2^m*ss - 1)` of its relative offset `x` is not 0 — the silent no-op.
#[logic(open)]
pub fn mark_noop_p2(d: SlabDescriptor, base: Int, size: Int, fp: FormatParams, m: Int) -> bool {
    pearlite! {
        0 <= m && m.pow2() == size / fp.sector_size@
            && ord(d.slab_size@ / fp.sector_size@) < m
            && forall<sp: u64, x: u64> sp@ == span(fp.sector_size@, m) && x@ == d.start_offset@ - base ==>
                (x & !(sp - 1u64)) != 0u64
    }
}

/// For a power-of-two region (`size/ss == 2^m`) and a descriptor of block order `m - 1`
/// whose masked candidate `x & !(2^m*ss - 1)` IS the region's single free block 0 — the
/// case a power-of-two sector size always gives — mark_allocated splits that block.
#[logic(open)]
pub fn mark_hit_p2(d: SlabDescriptor, base: Int, size: Int, fp: FormatParams, m: Int) -> bool {
    pearlite! {
        1 <= m && m.pow2() == size / fp.sector_size@
            && ord(d.slab_size@ / fp.sector_size@) + 1 == m
            && forall<sp: u64, x: u64> sp@ == span(fp.sector_size@, m) && x@ == d.start_offset@ - base ==>
                (x & !(sp - 1u64)) == 0u64
    }
}

/// The buddy lists after that split: order `m` empty, order `m-1` holds the other half.
#[logic(open)]
pub fn hit_lists(b: BuddyAllocator, d: SlabDescriptor, base: Int, fp: FormatParams, m: Int) -> bool {
    pearlite! {
        b.max_order@ == m && b.free_lists@.len() == m + 1
        && b.free_lists@[m]@.len() == 0
        && b.free_lists@[m - 1]@.len() == 1
        && b.free_lists@[m - 1]@[0]@ == (if d.start_offset@ - base >= span(fp.sector_size@, m - 1) { 0 } else { span(fp.sector_size@, m - 1) })
        && forall<o: Int> 0 <= o && o < m - 1 ==> b.free_lists@[o]@.len() == 0
    }
}

/// `rebuild_region` on one descriptor in the split case: the buddy lists are `hit_lists`.
#[logic(open)]
pub fn rebuilt_hit(r: RegionState, d: SlabDescriptor, base: Int, size: Int, fp: FormatParams) -> bool {
    pearlite! { forall<m: Int> mark_hit_p2(d, base, size, fp, m) ==> hit_lists(r.buddy, d, base, fp, m) }
}

/// The region holds exactly the slab rebuilt from `d`, listed in its size class.
#[logic(open)]
pub fn one_slab(r: RegionState, d: SlabDescriptor) -> bool {
    pearlite! {
        r.slabs@.contains(d.start_offset@)
        && (forall<k: Int> r.slabs@.contains(k) ==> k == d.start_offset@)
        && {
            let sl = r.slabs@.lookup(d.start_offset@);
            sl.start_offset == d.start_offset && sl.slab_size == d.slab_size
            && sl.element_size == d.element_size && sl.keys@ == d.keys@ && sl.rover@ == 0
            && slab_inv(sl)
            && forall<i: Int> 0 <= i && i < sl.bitmap.num_slots@ ==> slot_bit(sl.bitmap, i) == (d.keys@[i] != FREE_KEY)
        }
        && (forall<e: Int> sc_get(r.size_classes, e) ==
            if e == d.element_size@ { Seq::singleton(d.start_offset) } else { Seq::empty() })
    }
}

/// `rebuild_region` on one descriptor: the slab is back, and when `mark_allocated`
/// was a no-op the free lists are exactly those of a fresh power-of-two region.
#[logic(open)]
pub fn rebuilt_one(r: RegionState, d: SlabDescriptor, base: Int, size: Int, fp: FormatParams) -> bool {
    pearlite! {
        one_slab(r, d)
        && (forall<m: Int> mark_noop_p2(d, base, size, fp, m) ==>
                r.buddy.max_order@ == m && r.buddy.free_lists@[m]@ == Seq::singleton(0u64)
                && forall<o: Int> 0 <= o && o < m ==> r.buddy.free_lists@[o]@.len() == 0)
    }
}

/// A well-formed descriptor's block order fits the fresh buddy allocator.
#[logic]
#[requires(desc_ok(d, base, size, fp))]
#[requires(fp.sector_size@ > 0 && b.sector_size == fp.sector_size && b.total_usable_size@ == size)]
#[requires(b.total_usable_size@ / b.sector_size@ >= 1 ==> b.total_usable_size@ / b.sector_size@ < (b.max_order@ + 1).pow2())]
#[requires(0 <= b.max_order@)]
#[ensures(ord(d.slab_size@ / fp.sector_size@) <= b.max_order@)]
pub fn lemma_desc_k(d: SlabDescriptor, b: BuddyAllocator, base: Int, size: Int, fp: FormatParams) {
    pearlite! {
        lemma_ord_ge(fp.slab_size@ / fp.sector_size@);
        lemma_span_pos(fp.sector_size@, ord(fp.slab_size@ / fp.sector_size@));
        lemma_pow2(ord(fp.slab_size@ / fp.sector_size@));
        lemma_div_le(ord(fp.slab_size@ / fp.sector_size@).pow2(), fp.sector_size@, size);
        lemma_pow2_strict(ord(fp.slab_size@ / fp.sector_size@), b.max_order@ + 1)
    }
}

/// `p * d <= n` gives `p <= n / d`.
#[logic]
#[requires(d > 0 && p >= 0 && n >= 0 && p * d <= n)]
#[ensures(p <= n / d)]
pub fn lemma_div_le(p: Int, d: Int, n: Int) {
    lemma_divmod(n, d)
}
