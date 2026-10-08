//! Mirror of `AllocationBitmap` — `components/extent-manager/src/bitmap.rs:1-64`.
//!
//! Faithful whole-function mirror. Differences from the source, all disclosed:
//!  * the `debug_assert!`s at bitmap.rs:18,19,27,28,36 are rendered as `#[requires]`
//!    (debug-build semantics: violating them panics). This is deliberate: the
//!    preconditions ARE the inventory properties EM-BITMAP-INDEX-IN-RANGE /
//!    -NO-DOUBLE-ALLOC / -NO-DOUBLE-FREE, and every caller must discharge them.
//!  * the word-level bit operations are written inline exactly as in the source;
//!    the methods carry `#[bitwise_proof]` so `|`, `&`, `!`, `<<`, `>>` are reasoned
//!    about as 64-bit bit-vectors.
//!
//! Ghost view: `slot_bit(bm, i)` = bit `i % 64` of word `i / 64` — exactly the bit
//! `is_set` reads (`(words[i/64] >> i%64) & 1`). `cnt(bm, n)` = number of set slot
//! bits among the first `n` slots (recursive logic function).
use creusot_std::logic::ops::NthBitLogic;
use crate::model::assume::*;
use creusot_std::prelude::*;

pub struct AllocationBitmap {
    pub words: Vec<u64>,
    pub num_slots: u32,
    pub allocated_count: u32,
}

/// Word vector holds exactly `ceil(num_slots / 64)` words (bitmap.rs:9-11).
#[logic(open)]
pub fn bm_wf(bm: AllocationBitmap) -> bool {
    pearlite! { bm.words@.len() == (bm.num_slots@ + 63) / 64 }
}

/// The bit `is_set(i)` reads (bitmap.rs:35-40).
#[logic(open)]
pub fn slot_bit(bm: AllocationBitmap, i: Int) -> bool {
    pearlite! { bm.words@[i / 64].nth_bit(i % 64) }
}

/// Number of set slot bits among slots `0..n`.
#[logic(open)]
#[variant(n)]
pub fn cnt(bm: AllocationBitmap, n: Int) -> Int {
    pearlite! { if n <= 0 { 0 } else { cnt(bm, n - 1) + if slot_bit(bm, n - 1) { 1 } else { 0 } } }
}

/// EM-BITMAP-COUNT-MATCHES (bitmap half): the counter equals the number of set bits.
#[logic(open)]
pub fn bm_count_ok(bm: AllocationBitmap) -> bool {
    pearlite! { bm.allocated_count@ == cnt(bm, bm.num_slots@) }
}

/// The bitmap's maintained invariant: well-formed AND counter matches.
#[logic(open)]
pub fn bm_inv(bm: AllocationBitmap) -> bool {
    pearlite! { bm_wf(bm) && bm_count_ok(bm) }
}

/// Bits agree on the first `n` slots.
#[logic(open)]
pub fn same_bits(a: AllocationBitmap, b: AllocationBitmap, n: Int) -> bool {
    pearlite! { forall<j: Int> 0 <= j && j < n ==> slot_bit(a, j) == slot_bit(b, j) }
}

#[logic]
#[variant(n)]
#[requires(0 <= n)]
#[requires(same_bits(a, b, n))]
#[ensures(cnt(a, n) == cnt(b, n))]
pub fn lemma_cnt_frame(a: AllocationBitmap, b: AllocationBitmap, n: Int) {
    if n > 0 {
        lemma_cnt_frame(a, b, n - 1)
    }
}

#[logic]
#[variant(n)]
#[requires(0 <= n)]
#[ensures(0 <= cnt(a, n) && cnt(a, n) <= n)]
pub fn lemma_cnt_bounds(a: AllocationBitmap, n: Int) {
    if n > 0 {
        lemma_cnt_bounds(a, n - 1)
    }
}

/// A clear bit below `n` makes the count strictly smaller than `n`.
#[logic]
#[variant(n)]
#[requires(0 <= k && k < n)]
#[requires(!slot_bit(a, k))]
#[ensures(cnt(a, n) < n)]
pub fn lemma_cnt_lt(a: AllocationBitmap, k: Int, n: Int) {
    if n > k + 1 {
        lemma_cnt_lt(a, k, n - 1)
    } else {
        lemma_cnt_bounds(a, k)
    }
}

/// A set bit below `n` makes the count strictly positive.
#[logic]
#[variant(n)]
#[requires(0 <= k && k < n)]
#[requires(slot_bit(a, k))]
#[ensures(cnt(a, n) > 0)]
pub fn lemma_cnt_pos(a: AllocationBitmap, k: Int, n: Int) {
    if n > k + 1 {
        lemma_cnt_pos(a, k, n - 1)
    } else {
        lemma_cnt_bounds(a, k)
    }
}

/// Setting one clear bit raises the count by exactly one.
#[logic]
#[variant(n)]
#[requires(0 <= k && k < n)]
#[requires(forall<j: Int> 0 <= j && j < n && j != k ==> slot_bit(a, j) == slot_bit(b, j))]
#[requires(!slot_bit(a, k) && slot_bit(b, k))]
#[ensures(cnt(b, n) == cnt(a, n) + 1)]
pub fn lemma_cnt_inc(a: AllocationBitmap, b: AllocationBitmap, k: Int, n: Int) {
    if n > k + 1 {
        lemma_cnt_inc(a, b, k, n - 1)
    } else {
        lemma_cnt_frame(a, b, k)
    }
}

/// Clearing one set bit lowers the count by exactly one.
#[logic]
#[variant(n)]
#[requires(0 <= k && k < n)]
#[requires(forall<j: Int> 0 <= j && j < n && j != k ==> slot_bit(a, j) == slot_bit(b, j))]
#[requires(slot_bit(a, k) && !slot_bit(b, k))]
#[ensures(cnt(b, n) == cnt(a, n) - 1)]
pub fn lemma_cnt_dec(a: AllocationBitmap, b: AllocationBitmap, k: Int, n: Int) {
    if n > k + 1 {
        lemma_cnt_dec(a, b, k, n - 1)
    } else {
        lemma_cnt_frame(a, b, k)
    }
}

impl AllocationBitmap {
    /// Mirror of `AllocationBitmap::new` (bitmap.rs:8-15).
    #[bitwise_proof]
    #[ensures(bm_wf(result))]
    #[ensures(result.num_slots == num_slots)]
    #[ensures(result.allocated_count@ == 0)]
    #[ensures(forall<j: Int> 0 <= j && j < num_slots@ ==> !slot_bit(result, j))]
    #[ensures(bm_inv(result))]
    pub fn new(num_slots: u32) -> Self {
        let num_words = (num_slots as usize + 63) / 64;
        let r = Self { words: creusot_std::vec![0u64; num_words], num_slots, allocated_count: 0 };
        proof_assert! { forall<w: Int> 0 <= w && w < r.words@.len() ==> r.words@[w] == 0u64 };
        proof_assert! { forall<j: Int> 0 <= j && j < num_slots@ ==> !slot_bit(r, j) };
        snapshot! { lemma_cnt_zero_all(r, num_slots@) };
        r
    }

    /// Mirror of `AllocationBitmap::set` (bitmap.rs:17-24).
    #[bitwise_proof]
    #[requires(bm_wf(*self))]
    #[requires(idx@ < self.num_slots@)]
    #[requires(!slot_bit(*self, idx@))]
    #[requires(self.allocated_count@ < u32::MAX@)]
    #[ensures((^self).num_slots == self.num_slots)]
    #[ensures((^self).allocated_count@ == self.allocated_count@ + 1)]
    #[ensures(bm_wf(^self))]
    #[ensures(slot_bit(^self, idx@))]
    #[ensures(forall<j: Int> 0 <= j && j < 64 * self.words@.len() && j != idx@ ==> slot_bit(^self, j) == slot_bit(*self, j))]
    pub fn set(&mut self, idx: usize) {
        let word = idx / 64;
        let bit = idx % 64;
        self.words[word] |= 1u64 << bit;
        self.allocated_count += 1;
    }

    /// Mirror of `AllocationBitmap::clear` (bitmap.rs:26-33).
    #[bitwise_proof]
    #[requires(bm_wf(*self))]
    #[requires(idx@ < self.num_slots@)]
    #[requires(slot_bit(*self, idx@))]
    #[requires(self.allocated_count@ > 0)]
    #[ensures((^self).num_slots == self.num_slots)]
    #[ensures((^self).allocated_count@ == self.allocated_count@ - 1)]
    #[ensures(bm_wf(^self))]
    #[ensures(!slot_bit(^self, idx@))]
    #[ensures(forall<j: Int> 0 <= j && j < 64 * self.words@.len() && j != idx@ ==> slot_bit(^self, j) == slot_bit(*self, j))]
    pub fn clear(&mut self, idx: usize) {
        let word = idx / 64;
        let bit = idx % 64;
        self.words[word] &= !(1u64 << bit);
        self.allocated_count -= 1;
    }

    /// Mirror of `AllocationBitmap::is_set` (bitmap.rs:35-40).
    #[bitwise_proof]
    #[requires(bm_wf(*self))]
    #[requires(idx@ < self.num_slots@)]
    #[ensures(result == slot_bit(*self, idx@))]
    pub fn is_set(&self, idx: usize) -> bool {
        let word = idx / 64;
        let bit = idx % 64;
        (self.words[word] >> bit) & 1 == 1
    }

    /// Mirror of `AllocationBitmap::find_free_from` (bitmap.rs:42-51).
    /// `start < num_slots` is what every caller passes (the rover, slab.rs:30).
    #[requires(bm_wf(*self))]
    #[requires(self.num_slots@ == 0 || start@ < self.num_slots@)]
    #[ensures(match result {
        Some(i) => i@ < self.num_slots@ && !slot_bit(*self, i@)
            && (start@ <= i@ ==> forall<j: Int> start@ <= j && j < i@ ==> slot_bit(*self, j)),
        None => forall<j: Int> 0 <= j && j < self.num_slots@ ==> slot_bit(*self, j),
    })]
    pub fn find_free_from(&self, start: usize) -> Option<usize> {
        let n = self.num_slots as usize;
        #[invariant(forall<j: Int> 0 <= j && j < n@ && start@ <= j && j < start@ + produced.len() ==> slot_bit(*self, j))]
        #[invariant(forall<j: Int> 0 <= j && j < n@ && j + n@ < start@ + produced.len() ==> slot_bit(*self, j))]
        for i in 0..n {
            let idx = (start + i) % n;
            proof_assert! { idx@ == if start@ + i@ < n@ { start@ + i@ } else { start@ + i@ - n@ } };
            if !self.is_set(idx) {
                return Some(idx);
            }
        }
        None
    }

    /// Mirror of `AllocationBitmap::is_all_free` (bitmap.rs:53-55).
    #[ensures(result == (self.allocated_count@ == 0))]
    pub fn is_all_free(&self) -> bool {
        self.allocated_count == 0
    }

    /// Mirror of `AllocationBitmap::count_set` (bitmap.rs:57-59).
    #[ensures(result@ == self.allocated_count@)]
    pub fn count_set(&self) -> usize {
        self.allocated_count as usize
    }

    /// Mirror of `AllocationBitmap::num_slots` (bitmap.rs:61-63).
    #[ensures(result == self.num_slots)]
    pub fn num_slots(&self) -> u32 {
        self.num_slots
    }
}

/// Two distinct set bits below `n` make the count at least two.
#[logic]
#[variant(n)]
#[requires(0 <= i && i < n && 0 <= j && j < n && i != j)]
#[requires(slot_bit(a, i) && slot_bit(a, j))]
#[ensures(cnt(a, n) >= 2)]
pub fn lemma_cnt_two(a: AllocationBitmap, i: Int, j: Int, n: Int) {
    if n > i + 1 && n > j + 1 {
        lemma_cnt_two(a, i, j, n - 1)
    } else if n == i + 1 {
        lemma_cnt_pos(a, j, n - 1)
    } else {
        lemma_cnt_pos(a, i, n - 1)
    }
}

/// All bits set below `n` gives a full count.
#[logic]
#[variant(n)]
#[requires(0 <= n)]
#[requires(forall<j: Int> 0 <= j && j < n ==> slot_bit(a, j))]
#[ensures(cnt(a, n) == n)]
pub fn lemma_cnt_all_set(a: AllocationBitmap, n: Int) {
    if n > 0 {
        lemma_cnt_all_set(a, n - 1)
    }
}

/// All bits clear below `n` gives a zero count.
#[logic]
#[variant(n)]
#[requires(0 <= n)]
#[requires(forall<j: Int> 0 <= j && j < n ==> !slot_bit(a, j))]
#[ensures(cnt(a, n) == 0)]
pub fn lemma_cnt_zero_all(a: AllocationBitmap, n: Int) {
    if n > 0 {
        lemma_cnt_zero_all(a, n - 1)
    }
}

/// Zero count means no bit is set below `n`; full count means every bit is set.
#[logic]
#[variant(n)]
#[requires(0 <= n)]
#[ensures(cnt(a, n) == 0 ==> forall<j: Int> 0 <= j && j < n ==> !slot_bit(a, j))]
#[ensures(cnt(a, n) == n ==> forall<j: Int> 0 <= j && j < n ==> slot_bit(a, j))]
pub fn lemma_cnt_extremes(a: AllocationBitmap, n: Int) {
    if n > 0 {
        lemma_cnt_bounds(a, n - 1);
        lemma_cnt_extremes(a, n - 1)
    }
}
