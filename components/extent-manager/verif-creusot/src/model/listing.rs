//! Mirror of the two listing methods `IExtentManager::get_extents` (lib.rs:632-656 at the
//! pin) and `for_each_extent` (lib.rs:658-678), plus the `Extent` value type
//! (interfaces/src/iextent_manager.rs:10-15).
//!
//! `r.slabs.values()` (BTreeMap iteration) is the trusted std leaf [`SlabMap::keys_vec`]:
//! it yields every key of the map exactly once (ascending in std; the order is not used).
//! The `for_each_extent` callback is modelled as the observer that records every extent it
//! is passed, in order (`cb: &mut Vec<Extent>`), which is all a callback can see.
use crate::model::bitmap::*;
use crate::model::component::*;
use crate::model::params::*;
use crate::model::region::*;
use crate::model::slab::*;
use crate::model::assume::*;
use creusot_std::prelude::*;

/// Mirror of `interfaces::Extent` (iextent_manager.rs:10-15).
#[derive(::core::clone::Clone, ::core::marker::Copy)]
pub struct Extent {
    pub key: u64,
    pub size: u32,
    pub offset: u64,
}

impl SlabMap {
    /// TRUSTED std leaf: the keys `self.values()` visits (region.rs:10 BTreeMap iteration,
    /// lib.rs:639/663): every map key exactly once.
    #[trusted]
    #[ensures(forall<i: Int> 0 <= i && i < result@.len() ==> self@.contains(result@[i]@))]
    #[ensures(forall<k: Int> self@.contains(k) ==> exists<i: Int> 0 <= i && i < result@.len() && result@[i]@ == k)]
    #[ensures(forall<i: Int, j: Int> 0 <= i && i < j && j < result@.len() ==> result@[i] != result@[j])]
    pub fn keys_vec(&self) -> Vec<u64> {
        self.inner.keys().copied().collect()
    }
}

/// `e` is the extent get_extents builds from slot `j` of the slab keyed `k` of region `r`
/// (lib.rs:641-648): a non-FREE key, the slot's byte offset, the slab's element size.
#[logic(open)]
pub fn from_slot(r: RegionState, k: Int, j: Int, e: Extent) -> bool {
    pearlite! {
        r.slabs@.contains(k)
        && 0 <= j && j < r.slabs@.lookup(k).bitmap.num_slots@
        && r.slabs@.lookup(k).keys@[j] == e.key
        && e.key != FREE_KEY
        && e.offset@ == slot_off(r.slabs@.lookup(k), j)
        && e.size == r.slabs@.lookup(k).element_size
    }
}

/// `e` comes from some slot of region `r`.
#[logic(open)]
pub fn from_region(r: RegionState, e: Extent) -> bool {
    pearlite! { exists<k: Int, j: Int> from_slot(r, k, j, e) }
}

/// Every element of `s` from position `lo` on comes from region `r` and has a non-FREE key.
#[logic(open)]
pub fn all_from(s: Seq<Extent>, lo: Int, r: RegionState) -> bool {
    pearlite! { forall<p: Int> lo <= p && p < s.len() ==> from_region(r, s[p]) && s[p].key != FREE_KEY }
}

/// `e` occurs in `s` at or after position `lo`.
#[logic(open)]
pub fn in_from(s: Seq<Extent>, lo: Int, e: Extent) -> bool {
    pearlite! { exists<p: Int> lo <= p && p < s.len() && s[p] == e }
}

/// Every extent of an occupied slot of region `r` occurs in `s` from `lo` on (COMPLETENESS).
#[logic(open)]
pub fn all_listed(s: Seq<Extent>, lo: Int, r: RegionState) -> bool {
    pearlite! { forall<k: Int, j: Int, e: Extent> from_slot(r, k, j, e) ==> in_from(s, lo, e) }
}

/// An occurrence from `lo` is one from any `lo2 <= lo`.
#[logic]
#[requires(in_from(s, lo, e) && lo2 <= lo)]
#[ensures(in_from(s, lo2, e))]
pub fn lemma_in_from_lower(s: Seq<Extent>, lo: Int, lo2: Int, e: Extent) {}

/// `all_listed` for every current region of `em`.
#[logic(open)]
pub fn all_regions_listed(em: ExtentManager, rv: Seq<usize>, s: Seq<Extent>, lo: Int, n: Int) -> bool {
    pearlite! { forall<i: Int> 0 <= i && i < n ==> all_listed(s, lo, em.arena@[rv[i]@]) }
}

/// Appending (or any extension that keeps the prefix) keeps an occurrence.
#[logic]
#[requires(0 <= lo && in_from(a, lo, e))]
#[requires(a.len() <= b.len() && forall<p: Int> 0 <= p && p < a.len() ==> b[p] == a[p])]
#[ensures(in_from(b, lo, e))]
pub fn lemma_in_from_grow(a: Seq<Extent>, b: Seq<Extent>, lo: Int, e: Extent) {}

/// The listing loops over one region's slabs (lib.rs:641-654 / 665-680): appends one
/// extent per occupied slot to `out` (the result vector, or the callback's observations).
#[requires(rg_list_ok(*r))]
#[ensures((^out)@.len() >= out@.len())]
#[ensures(forall<p: Int> 0 <= p && p < out@.len() ==> (^out)@[p] == out@[p])]
#[ensures(all_from((^out)@, out@.len(), *r))]
#[ensures(all_listed((^out)@, out@.len(), *r))]
pub fn region_extents(r: &RegionState, out: &mut Vec<Extent>) {
    let ks = r.slabs.keys_vec();
    let o0 = snapshot! { *out };
    let mut a: usize = 0;
    #[invariant(a@ <= ks@.len())]
    #[invariant(out@.len() >= o0@.len())]
    #[invariant(forall<p: Int> 0 <= p && p < o0@.len() ==> out@[p] == o0@[p])]
    #[invariant(all_from(out@, o0@.len(), *r))]
    #[invariant(forall<b: Int, j: Int, e: Extent> 0 <= b && b < a@ && from_slot(*r, ks@[b]@, j, e) ==> in_from(out@, o0@.len(), e))]
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
                #[invariant(out@.len() >= out_a@.len())]
                #[invariant(forall<p: Int> 0 <= p && p < out_a@.len() ==> out@[p] == out_a@[p])]
                #[invariant(all_from(out@, o0@.len(), *r))]
                #[invariant(forall<j: Int, e: Extent> 0 <= j && j < i@ && from_slot(*r, k@, j, e) ==> in_from(out@, o0@.len(), e))]
                while i < n {
                    let key = slab.get_key(i);
                    let before = snapshot! { *out };
                    if key != FREE_KEY {
                        proof_assert! { lemma_slot_inside(*slab, i@); slot_off(*slab, i@) <= u64::MAX@ };
                        let e = Extent { key, offset: slab.slot_offset(i), size: slab.element_size };
                        proof_assert! { from_slot(*r, k@, i@, e) };
                        out.push(e);
                        proof_assert! { forall<p: Int> 0 <= p && p < before@.len() ==> out@[p] == before@[p] };
                        proof_assert! { out@[before@.len()] == e && from_region(*r, e) };
                        proof_assert! { in_from(out@, o0@.len(), e) };
                        proof_assert! { forall<e2: Extent> from_slot(*r, k@, i@, e2) ==> e2 == e };
                        proof_assert! { forall<j: Int, e2: Extent> 0 <= j && j < i@ && from_slot(*r, k@, j, e2) ==> {
                            lemma_in_from_grow(before@, out@, o0@.len(), e2); in_from(out@, o0@.len(), e2) } };
                    } else {
                        proof_assert! { forall<e2: Extent> !from_slot(*r, k@, i@, e2) };
                    }
                    i += 1;
                }
                proof_assert! { forall<b: Int, j: Int, e: Extent> 0 <= b && b < a@ && from_slot(*r, ks@[b]@, j, e) ==> {
                    lemma_in_from_grow(out_a@, out@, o0@.len(), e); in_from(out@, o0@.len(), e) } };
            }
            None => {
                proof_assert! { forall<j: Int, e: Extent> !from_slot(*r, k@, j, e) };
            }
        }
        a += 1;
    }
    proof_assert! { forall<k: Int, j: Int, e: Extent> from_slot(*r, k, j, e) ==> r.slabs@.contains(k)
        && exists<b: Int> 0 <= b && b < ks@.len() && ks@[b]@ == k };
}

/// Every element of `s` comes from one of the current regions `rv` of `em`, with a
/// non-FREE key.
#[logic(open)]
pub fn listed_ok(em: ExtentManager, rv: Seq<usize>, s: Seq<Extent>) -> bool {
    pearlite! {
        forall<p: Int> 0 <= p && p < s.len() ==> s[p].key != FREE_KEY
            && exists<i: Int> 0 <= i && i < rv.len() && from_region(em.arena@[rv[i]@], s[p])
    }
}

/// `e` comes from one of the current regions `rv` of `em`.
#[logic(open)]
pub fn listed_one(em: ExtentManager, rv: Seq<usize>, e: Extent) -> bool {
    pearlite! { exists<i: Int> 0 <= i && i < rv.len() && from_region(em.arena@[rv[i]@], e) }
}

/// `listed_ok` for the prefix `0..lo`.
#[logic(open)]
pub fn listed_ok_upto(em: ExtentManager, rv: Seq<usize>, s: Seq<Extent>, lo: Int) -> bool {
    pearlite! {
        forall<p: Int> 0 <= p && p < lo ==> s[p].key != FREE_KEY
            && exists<i: Int> 0 <= i && i < rv.len() && from_region(em.arena@[rv[i]@], s[p])
    }
}

impl ExtentManager {
    /// Mirror of `IExtentManager::get_extents` (lib.rs:632-656).
    #[requires(em_regions_wf(*self) && em_list_ok(*self))]
    #[ensures(match self.regions { None => result@.len() == 0, Some(rv) => listed_ok(*self, rv@, result@) })]
    #[ensures(match self.regions { None => true, Some(rv) => all_regions_listed(*self, rv@, result@, 0, rv@.len()) })]
    pub fn get_extents(&self) -> Vec<Extent> {
        let mut result: Vec<Extent> = Vec::new();
        match &self.regions {
            Some(rv) => {
                let mut i: usize = 0;
                #[invariant(i@ <= rv@.len())]
                #[invariant(listed_ok_upto(*self, rv@, result@, result@.len()))]
                #[invariant(all_regions_listed(*self, rv@, result@, 0, i@))]
                while i < rv.len() {
                    let before = snapshot! { result };
                    proof_assert! { rg_list_ok(self.arena@[rv@[i@]@]) };
                    region_extents(&self.arena[rv[i]], &mut result);
                    proof_assert! { forall<p: Int> 0 <= p && p < before@.len() ==> result@[p] == before@[p] };
                    proof_assert! { forall<p: Int> before@.len() <= p && p < result@.len() ==>
                        result@[p].key != FREE_KEY && from_region(self.arena@[rv@[i@]@], result@[p]) };
                    proof_assert! { listed_ok_upto(*self, rv@, result@, result@.len()) };
                    proof_assert! { forall<k: Int, j: Int, e: Extent> from_slot(self.arena@[rv@[i@]@], k, j, e) ==> {
                        lemma_in_from_lower(result@, before@.len(), 0, e); in_from(result@, 0, e) } };
                    proof_assert! { forall<i2: Int, k: Int, j: Int, e: Extent> 0 <= i2 && i2 < i@ && from_slot(self.arena@[rv@[i2]@], k, j, e) ==> {
                        lemma_in_from_grow(before@, result@, 0, e); in_from(result@, 0, e) } };
                    proof_assert! { all_regions_listed(*self, rv@, result@, 0, i@ + 1) };
                    i += 1;
                }
                result
            }
            None => result,
        }
    }

    /// Mirror of `IExtentManager::for_each_extent` (lib.rs:658-678); `cb` records what the
    /// callback is passed, in call order.
    #[requires(em_regions_wf(*self) && em_list_ok(*self))]
    #[ensures(forall<p: Int> 0 <= p && p < cb@.len() ==> (^cb)@[p] == cb@[p])]
    #[ensures((^cb)@.len() >= cb@.len())]
    #[ensures(forall<p: Int> cb@.len() <= p && p < (^cb)@.len() ==> (^cb)@[p].key != FREE_KEY)]
    #[ensures(match self.regions {
        None => (^cb)@.len() == cb@.len(),
        Some(rv) => forall<p: Int> cb@.len() <= p && p < (^cb)@.len() ==> listed_one(*self, rv@, (^cb)@[p]),
    })]
    #[ensures(match self.regions { None => true, Some(rv) => all_regions_listed(*self, rv@, (^cb)@, cb@.len(), rv@.len()) })]
    pub fn for_each_extent(&self, cb: &mut Vec<Extent>) {
        let c0 = snapshot! { *cb };
        match &self.regions {
            Some(rv) => {
                let mut i: usize = 0;
                #[invariant(i@ <= rv@.len())]
                #[invariant(cb@.len() >= c0@.len())]
                #[invariant(forall<p: Int> 0 <= p && p < c0@.len() ==> cb@[p] == c0@[p])]
                #[invariant(forall<p: Int> c0@.len() <= p && p < cb@.len() ==> cb@[p].key != FREE_KEY)]
                #[invariant(forall<p: Int> c0@.len() <= p && p < cb@.len() ==> listed_one(*self, rv@, cb@[p]))]
                #[invariant(all_regions_listed(*self, rv@, cb@, c0@.len(), i@))]
                while i < rv.len() {
                    let before = snapshot! { *cb };
                    proof_assert! { rg_list_ok(self.arena@[rv@[i@]@]) };
                    region_extents(&self.arena[rv[i]], cb);
                    proof_assert! { forall<p: Int> 0 <= p && p < before@.len() ==> cb@[p] == before@[p] };
                    proof_assert! { forall<p: Int> before@.len() <= p && p < cb@.len() ==>
                        cb@[p].key != FREE_KEY && from_region(self.arena@[rv@[i@]@], cb@[p]) };
                    proof_assert! { forall<p: Int> before@.len() <= p && p < cb@.len() ==> listed_one(*self, rv@, cb@[p]) };
                    proof_assert! { forall<k: Int, j: Int, e: Extent> from_slot(self.arena@[rv@[i@]@], k, j, e) ==> {
                        lemma_in_from_lower(cb@, before@.len(), c0@.len(), e); in_from(cb@, c0@.len(), e) } };
                    proof_assert! { forall<i2: Int, k: Int, j: Int, e: Extent> 0 <= i2 && i2 < i@ && from_slot(self.arena@[rv@[i2]@], k, j, e) ==> {
                        lemma_in_from_grow(before@, cb@, c0@.len(), e); in_from(cb@, c0@.len(), e) } };
                    proof_assert! { all_regions_listed(*self, rv@, cb@, c0@.len(), i@ + 1) };
                    i += 1;
                }
            }
            None => {}
        }
    }
}
