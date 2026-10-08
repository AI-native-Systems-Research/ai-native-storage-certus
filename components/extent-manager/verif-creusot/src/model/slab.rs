//! Mirror of `Slab` and `SizeClassManager` — `components/extent-manager/src/slab.rs:1-91`.
//!
//! Faithful whole-function mirrors. Disclosed differences:
//!  * `(slab_size / element_size as u64) as u32` (slab.rs:18): Creusot leaves an
//!    out-of-range `as u32` UNSPECIFIED, so the cast goes through the one-line
//!    `#[trusted]` leaf [`as_u32`], whose contract is Rust's documented truncating
//!    cast (`x as u32 == x mod 2^32`, Rust Reference "Numeric cast").
//!  * `HashMap<u32, Vec<u64>>` (slab.rs:91-93) is the real std type; its four point
//!    operations go through `#[trusted]` leaves (`hm_*`) whose contracts are the std
//!    documented semantics over the `FMap` view creusot-std already gives HashMap.
//!    `entry(e).or_default().push(s)` (slab.rs:103) is rendered as the equivalent
//!    get_mut-or-insert; `Vec::retain(|&o| o != s)` (slab.rs:108) as an index loop.
use crate::model::bitmap::*;
use creusot_std::logic::FMap;
use crate::model::assume::*;
use creusot_std::prelude::*;
use std::collections::HashMap;

/// `slab.rs:5` — `pub const FREE_KEY: u64 = u64::MAX;`
pub const FREE_KEY: u64 = u64::MAX;

/// TRUSTED leaf: Rust's truncating integer cast `x as u32` (Rust Reference,
/// "Semantics: numeric cast — casting from a larger integer to a smaller integer
/// truncates"). Creusot models an out-of-range cast as an unspecified value.
#[trusted]
#[ensures(result@ == x@ % 4294967296)]
pub fn as_u32(x: u64) -> u32 {
    x as u32
}

pub struct Slab {
    pub start_offset: u64,
    pub slab_size: u64,
    pub element_size: u32,
    pub bitmap: AllocationBitmap,
    pub keys: Vec<u64>,
    pub rover: usize,
}

/// `slab_size / element_size` truncated to u32 — the slot count `Slab::new` computes.
#[logic(open)]
pub fn slots_of(slab_size: Int, element_size: Int) -> Int {
    pearlite! { (slab_size / element_size) % 4294967296 }
}

/// Slab geometry fixed by `Slab::new` (slab.rs:17-27).
#[logic(open)]
pub fn slab_geom(s: Slab) -> bool {
    pearlite! {
        s.element_size@ > 0
        && s.bitmap.num_slots@ == slots_of(s.slab_size@, s.element_size@)
        && s.start_offset@ + s.slab_size@ <= u64::MAX@
    }
}

/// One key entry per slot (slab.rs:24).
#[logic(open)]
pub fn slab_keys_len(s: Slab) -> bool {
    pearlite! { s.keys@.len() == s.bitmap.num_slots@ }
}

/// The rover always names a slot (slab.rs:31), or is 0 for a zero-slot slab.
#[logic(open)]
pub fn slab_rover(s: Slab) -> bool {
    pearlite! { s.rover@ < s.bitmap.num_slots@ || (s.bitmap.num_slots@ == 0 && s.rover@ == 0) }
}

/// The slab's maintained invariant, as the conjunction of its named halves.
#[logic(open)]
pub fn slab_inv(s: Slab) -> bool {
    pearlite! { slab_geom(s) && slab_keys_len(s) && slab_rover(s) && bm_inv(s.bitmap) }
}

/// EM-INV-KEY-IMPLIES-ALLOCATED at slab level.
#[logic(open)]
pub fn key_alloc(s: Slab) -> bool {
    pearlite! {
        forall<i: Int> 0 <= i && i < s.bitmap.num_slots@ && s.keys@[i] != FREE_KEY ==> slot_bit(s.bitmap, i)
    }
}

/// Byte offset of slot `i` (slab.rs:58-60).
#[logic(open)]
pub fn slot_off(s: Slab, i: Int) -> Int {
    pearlite! { s.start_offset@ + i * s.element_size@ }
}

/// `x mod m <= x` for the truncating cast, and the slot area fits in the slab.
#[logic]
#[requires(slab_geom(s))]
#[ensures(s.bitmap.num_slots@ <= s.slab_size@ / s.element_size@)]
#[ensures(s.bitmap.num_slots@ * s.element_size@ <= s.slab_size@)]
pub fn lemma_slots_fit(s: Slab) {
}

/// Slot `i < num_slots` lies inside the slab: `[off_i, off_i + es)` within `[start, start+slab_size)`.
#[logic]
#[requires(slab_geom(s))]
#[requires(0 <= i && i < s.bitmap.num_slots@)]
#[ensures(s.start_offset@ <= slot_off(s, i))]
#[ensures(slot_off(s, i) + s.element_size@ <= s.start_offset@ + s.slab_size@)]
pub fn lemma_slot_inside(s: Slab, i: Int) {
    lemma_slots_fit(s);
}

/// Different slots are disjoint: `i < j ==> off_i + es <= off_j`.
#[logic]
#[requires(slab_geom(s))]
#[requires(0 <= i && i < j)]
#[ensures(slot_off(s, i) + s.element_size@ <= slot_off(s, j))]
pub fn lemma_slots_disjoint(s: Slab, i: Int, j: Int) {
}

impl Slab {
    /// Mirror of `Slab::new` (slab.rs:17-27).
    #[requires(element_size@ > 0)]
    #[requires(start_offset@ + slab_size@ <= u64::MAX@)]
    #[ensures(result.start_offset == start_offset && result.slab_size == slab_size)]
    #[ensures(result.element_size == element_size)]
    #[ensures(result.bitmap.num_slots@ == slots_of(slab_size@, element_size@))]
    #[ensures(result.bitmap.allocated_count@ == 0 && result.rover@ == 0)]
    #[ensures(forall<j: Int> 0 <= j && j < result.bitmap.num_slots@ ==> !slot_bit(result.bitmap, j))]
    #[ensures(forall<j: Int> 0 <= j && j < result.keys@.len() ==> result.keys@[j] == FREE_KEY)]
    #[ensures(slab_inv(result))]
    #[ensures(key_alloc(result))]
    pub fn new(start_offset: u64, slab_size: u64, element_size: u32) -> Self {
        let num_slots = as_u32(slab_size / element_size as u64);
        Self {
            start_offset,
            slab_size,
            element_size,
            bitmap: AllocationBitmap::new(num_slots),
            keys: creusot_std::vec![FREE_KEY; num_slots as usize],
            rover: 0,
        }
    }

    /// Mirror of `Slab::alloc_slot` (slab.rs:29-35).
    #[requires(slab_inv(*self))]
    #[ensures(slab_inv(^self))]
    #[ensures((^self).start_offset == self.start_offset && (^self).slab_size == self.slab_size)]
    #[ensures((^self).element_size == self.element_size && (^self).keys == self.keys)]
    #[ensures((^self).bitmap.num_slots == self.bitmap.num_slots)]
    #[ensures(match result {
        Some((idx, off)) =>
            idx@ < self.bitmap.num_slots@ && !slot_bit(self.bitmap, idx@) && slot_bit((^self).bitmap, idx@)
            && (self.rover@ <= idx@ ==> forall<j: Int> self.rover@ <= j && j < idx@ ==> slot_bit(self.bitmap, j))
            && off@ == slot_off(*self, idx@)
            && (^self).rover@ == (idx@ + 1) % self.bitmap.num_slots@
            && (^self).bitmap.allocated_count@ == self.bitmap.allocated_count@ + 1
            && (forall<j: Int> 0 <= j && j < self.bitmap.num_slots@ && j != idx@ ==>
                    slot_bit((^self).bitmap, j) == slot_bit(self.bitmap, j)),
        None => ^self == *self
            && forall<j: Int> 0 <= j && j < self.bitmap.num_slots@ ==> slot_bit(self.bitmap, j),
    })]
    pub fn alloc_slot(&mut self) -> Option<(usize, u64)> {
        let idx = self.bitmap.find_free_from(self.rover)?;
        snapshot! { lemma_cnt_lt(self.bitmap, idx@, self.bitmap.num_slots@) };
        let old = snapshot! { *self };
        self.bitmap.set(idx);
        snapshot! { lemma_cnt_inc(old.bitmap, self.bitmap, idx@, self.bitmap.num_slots@) };
        self.rover = (idx + 1) % self.bitmap.num_slots() as usize;
        snapshot! { lemma_slot_inside(*self, idx@) };
        let offset = self.slot_offset(idx);
        Some((idx, offset))
    }

    /// Mirror of `Slab::free_slot` (slab.rs:37-40).
    #[requires(slab_inv(*self))]
    #[requires(slot_index@ < self.bitmap.num_slots@)]
    #[requires(slot_bit(self.bitmap, slot_index@))]
    #[ensures(slab_inv(^self))]
    #[ensures((^self).start_offset == self.start_offset && (^self).slab_size == self.slab_size)]
    #[ensures((^self).element_size == self.element_size && (^self).rover == self.rover)]
    #[ensures((^self).bitmap.num_slots == self.bitmap.num_slots)]
    #[ensures(!slot_bit((^self).bitmap, slot_index@))]
    #[ensures((^self).keys@ == self.keys@.set(slot_index@, FREE_KEY))]
    #[ensures((^self).bitmap.allocated_count@ == self.bitmap.allocated_count@ - 1)]
    #[ensures(forall<j: Int> 0 <= j && j < self.bitmap.num_slots@ && j != slot_index@ ==>
        slot_bit((^self).bitmap, j) == slot_bit(self.bitmap, j))]
    #[ensures(key_alloc(*self) ==> key_alloc(^self))]
    pub fn free_slot(&mut self, slot_index: usize) {
        snapshot! { lemma_cnt_pos(self.bitmap, slot_index@, self.bitmap.num_slots@) };
        let old = snapshot! { *self };
        self.bitmap.clear(slot_index);
        snapshot! { lemma_cnt_dec(old.bitmap, self.bitmap, slot_index@, self.bitmap.num_slots@) };
        self.keys[slot_index] = FREE_KEY;
    }

    /// Mirror of `Slab::set_key` (slab.rs:42-44).
    #[requires(slot_index@ < self.keys@.len())]
    #[ensures((^self).keys@ == self.keys@.set(slot_index@, key))]
    #[ensures((^self).bitmap == self.bitmap && (^self).rover == self.rover)]
    #[ensures((^self).start_offset == self.start_offset && (^self).slab_size == self.slab_size)]
    #[ensures((^self).element_size == self.element_size)]
    #[ensures(slab_inv(*self) ==> slab_inv(^self))]
    #[ensures(slab_keys_len(*self) && key_alloc(*self) && (key == FREE_KEY || slot_bit(self.bitmap, slot_index@)) ==> key_alloc(^self))]
    pub fn set_key(&mut self, slot_index: usize, key: u64) {
        self.keys[slot_index] = key;
    }

    /// Mirror of `Slab::get_key` (slab.rs:46-48).
    #[requires(slot_index@ < self.keys@.len())]
    #[ensures(result == self.keys@[slot_index@])]
    pub fn get_key(&self, slot_index: usize) -> u64 {
        self.keys[slot_index]
    }

    /// Mirror of `Slab::is_empty` (slab.rs:50-52).
    #[ensures(result == (self.bitmap.allocated_count@ == 0))]
    pub fn is_empty(&self) -> bool {
        self.bitmap.is_all_free()
    }

    /// Mirror of `Slab::is_full` (slab.rs:54-56).
    #[ensures(result == (self.bitmap.allocated_count@ == self.bitmap.num_slots@))]
    pub fn is_full(&self) -> bool {
        self.bitmap.count_set() == self.bitmap.num_slots() as usize
    }

    /// Mirror of `Slab::slot_offset` (slab.rs:58-60).
    #[requires(slot_off(*self, slot_index@) <= u64::MAX@)]
    #[ensures(result@ == slot_off(*self, slot_index@))]
    pub fn slot_offset(&self, slot_index: usize) -> u64 {
        self.start_offset + slot_index as u64 * self.element_size as u64
    }

    /// Mirror of `Slab::slot_for_offset` (slab.rs:62-76).
    #[requires(self.element_size@ > 0)]
    #[ensures(match result {
        Some(i) => i@ < self.bitmap.num_slots@ && slot_off(*self, i@) == byte_offset@,
        None => forall<i: Int> 0 <= i && i < self.bitmap.num_slots@ ==> slot_off(*self, i) != byte_offset@,
    })]
    pub fn slot_for_offset(&self, byte_offset: u64) -> Option<usize> {
        if byte_offset < self.start_offset {
            return None;
        }
        let relative = byte_offset - self.start_offset;
        if relative % self.element_size as u64 != 0 {
            proof_assert! { forall<i: Int> 0 <= i ==> (i * self.element_size@) % self.element_size@ == 0 };
            return None;
        }
        let idx = (relative / self.element_size as u64) as usize;
        if idx < self.bitmap.num_slots() as usize {
            Some(idx)
        } else {
            proof_assert! { forall<i: Int> 0 <= i && i * self.element_size@ == relative@ ==> i == idx@ };
            None
        }
    }

    /// Mirror of `Slab::mark_slot_allocated` (slab.rs:78-80).
    #[requires(slab_inv(*self))]
    #[requires(slot_index@ < self.bitmap.num_slots@)]
    #[requires(!slot_bit(self.bitmap, slot_index@))]
    #[ensures(slab_inv(^self))]
    #[ensures((^self).start_offset == self.start_offset && (^self).slab_size == self.slab_size)]
    #[ensures((^self).element_size == self.element_size && (^self).keys == self.keys)]
    #[ensures((^self).rover == self.rover && (^self).bitmap.num_slots == self.bitmap.num_slots)]
    #[ensures(slot_bit((^self).bitmap, slot_index@))]
    #[ensures(forall<j: Int> 0 <= j && j < self.bitmap.num_slots@ && j != slot_index@ ==>
        slot_bit((^self).bitmap, j) == slot_bit(self.bitmap, j))]
    #[ensures((^self).bitmap.allocated_count@ == self.bitmap.allocated_count@ + 1)]
    pub fn mark_slot_allocated(&mut self, slot_index: usize) {
        snapshot! { lemma_cnt_lt(self.bitmap, slot_index@, self.bitmap.num_slots@) };
        let old = snapshot! { *self };
        self.bitmap.set(slot_index);
        snapshot! { lemma_cnt_inc(old.bitmap, self.bitmap, slot_index@, self.bitmap.num_slots@) };
    }

    /// Mirror of `Slab::num_slots` (slab.rs:82-84).
    #[ensures(result == self.bitmap.num_slots)]
    pub fn num_slots(&self) -> u32 {
        self.bitmap.num_slots()
    }

    /// Mirror of `Slab::contains_offset` (slab.rs:86-88).
    #[requires(self.start_offset@ + self.slab_size@ <= u64::MAX@)]
    #[ensures(result == (self.start_offset@ <= byte_offset@ && byte_offset@ < self.start_offset@ + self.slab_size@))]
    pub fn contains_offset(&self, byte_offset: u64) -> bool {
        byte_offset >= self.start_offset && byte_offset < self.start_offset + self.slab_size
    }
}

// ---------------------------------------------------------------------------
// SizeClassManager (slab.rs:91-121) over the real std HashMap.
// ---------------------------------------------------------------------------

pub struct SizeClassManager {
    pub map: HashMap<u32, Vec<u64>>,
}

/// The offsets list the size class `e` currently holds (empty if absent).
#[logic(open)]
pub fn sc_get(m: SizeClassManager, e: Int) -> Seq<u64> {
    pearlite! { match m.map@.get(e) { Some(v) => v@, None => Seq::empty() } }
}

/// `s` with every occurrence of `x` removed — `Vec::retain(|&o| o != x)`.
#[logic(open)]
#[variant(s.len())]
pub fn filter_ne(s: Seq<u64>, x: u64) -> Seq<u64> {
    pearlite! {
        if s.len() == 0 { Seq::empty() }
        else if s[s.len() - 1] == x { filter_ne(s.subsequence(0, s.len() - 1), x) }
        else { filter_ne(s.subsequence(0, s.len() - 1), x).push_back(s[s.len() - 1]) }
    }
}

/// TRUSTED std leaf: `HashMap::new()` is empty.
#[trusted]
#[ensures(result@ == FMap::empty())]
pub fn hm_new() -> HashMap<u32, Vec<u64>> {
    HashMap::new()
}

/// TRUSTED std leaf: `HashMap::get`.
#[trusted]
#[ensures(match result { Some(v) => m@.get(k@) == Some(*v), None => m@.get(k@) == None })]
pub fn hm_get(m: &HashMap<u32, Vec<u64>>, k: u32) -> Option<&Vec<u64>> {
    m.get(&k)
}

/// TRUSTED std leaf: `HashMap::get_mut`.
#[trusted]
#[ensures(match result {
    Some(v) => (*m)@.get(k@) == Some(*v) && (^m)@ == (*m)@.insert(k@, ^v),
    None => (*m)@.get(k@) == None && ^m == *m,
})]
pub fn hm_get_mut(m: &mut HashMap<u32, Vec<u64>>, k: u32) -> Option<&mut Vec<u64>> {
    m.get_mut(&k)
}

/// TRUSTED std leaf: `HashMap::insert`.
#[trusted]
#[ensures((^m)@ == (*m)@.insert(k@, v))]
pub fn hm_insert(m: &mut HashMap<u32, Vec<u64>>, k: u32, v: Vec<u64>) {
    m.insert(k, v);
}

/// TRUSTED std leaf: `HashMap::remove`.
#[trusted]
#[ensures((^m)@ == (*m)@.remove(k@))]
pub fn hm_remove(m: &mut HashMap<u32, Vec<u64>>, k: u32) {
    m.remove(&k);
}

/// Mirror of `offsets.retain(|&o| o != start_offset)` (slab.rs:108).
#[ensures((^v)@ == filter_ne(v@, x))]
pub fn retain_ne(v: &mut Vec<u64>, x: u64) {
    let mut out: Vec<u64> = Vec::new();
    let mut i: usize = 0;
    let n = v.len();
    #[invariant(i@ <= n@ && n@ == v@.len())]
    #[invariant(out@ == filter_ne(v@.subsequence(0, i@), x))]
    while i < n {
        let e = v[i];
        proof_assert! { v@.subsequence(0, i@ + 1).subsequence(0, i@) == v@.subsequence(0, i@) };
        if e != x {
            out.push(e);
        }
        i += 1;
    }
    proof_assert! { v@.subsequence(0, n@) == v@ };
    *v = out;
}

impl SizeClassManager {
    /// Mirror of `SizeClassManager::new` (slab.rs:96-100).
    #[ensures(forall<e: Int> sc_get(result, e) == Seq::empty())]
    pub fn new() -> Self {
        Self { map: hm_new() }
    }

    /// Mirror of `SizeClassManager::add_slab` (slab.rs:102-104).
    #[ensures(forall<e: Int> sc_get(^self, e) == if e == element_size@ { sc_get(*self, e).push_back(start_offset) } else { sc_get(*self, e) })]
    pub fn add_slab(&mut self, element_size: u32, start_offset: u64) {
        match hm_get_mut(&mut self.map, element_size) {
            Some(v) => v.push(start_offset),
            None => {
                let mut v = Vec::new();
                v.push(start_offset);
                hm_insert(&mut self.map, element_size, v);
            }
        }
    }

    /// Mirror of `SizeClassManager::remove_slab` (slab.rs:106-113).
    #[ensures(forall<e: Int> sc_get(^self, e) == if e == element_size@ { filter_ne(sc_get(*self, e), start_offset) } else { sc_get(*self, e) })]
    pub fn remove_slab(&mut self, element_size: u32, start_offset: u64) {
        if let Some(offsets) = hm_get_mut(&mut self.map, element_size) {
            retain_ne(offsets, start_offset);
            if offsets.len() == 0 {  // `offsets.is_empty()` (slab.rs:109)
                hm_remove(&mut self.map, element_size);
            }
        }
    }

    /// Mirror of `SizeClassManager::get_slabs(e).first()` (slab.rs:115-120 plus the
    /// `.first()` every caller applies, region.rs:47).
    #[ensures(result == if sc_get(*self, element_size@).len() > 0 { Some(sc_get(*self, element_size@)[0]) } else { None })]
    pub fn get_slabs_first(&self, element_size: u32) -> Option<u64> {
        match hm_get(&self.map, element_size) {
            Some(v) => {
                if v.len() > 0 {
                    Some(v[0])
                } else {
                    None
                }
            }
            None => None,
        }
    }
}
