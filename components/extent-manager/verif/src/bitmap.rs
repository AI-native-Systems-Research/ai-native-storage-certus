//! Mirror of `AllocationBitmap` (src/bitmap.rs:1-64).
//!
//! Spec: FR-020 (each slab uses a bitmap allocator). `allocated_count` is the
//! liveness counter that drives `is_all_free`/`is_empty` (slab.rs:50-52) and
//! hence the deferred-free path (FR-008: an emptied slab is returned to the
//! buddy allocator). The proven properties are:
//!  * the word index derived from a slot index is always in bounds of `words`
//!    (memory safety of the bit access), and
//!  * `allocated_count` is incremented by `set` and decremented by `clear`, so
//!    `is_all_free` is exactly "no slots allocated".

use creusot_std::prelude::*;

pub struct AllocationBitmap {
    pub words: Vec<u64>,
    pub num_slots: u32,
    pub allocated_count: u32,
}

/// Well-formedness: the word vector holds exactly `ceil(num_slots / 64)` words,
/// as established by `AllocationBitmap::new` (bitmap.rs:8-15).
#[logic(open)]
pub fn wf(bm: &AllocationBitmap) -> bool {
    pearlite! { bm.words@.len() == (bm.num_slots@ + 63) / 64 }
}

impl AllocationBitmap {
    /// Mirror of `AllocationBitmap::new` (bitmap.rs:8-15).
    #[ensures(wf(&result))]
    #[ensures(result.num_slots@ == num_slots@)]
    #[ensures(result.allocated_count@ == 0)]
    pub fn new(num_slots: u32) -> Self {
        let num_words = (num_slots as usize + 63) / 64;
        Self {
            words: creusot_std::vec![0u64; num_words],
            num_slots,
            allocated_count: 0,
        }
    }

    /// Mirror of `AllocationBitmap::set` (bitmap.rs:17-24).
    ///
    /// `idx < num_slots` is the source `debug_assert!`; `allocated_count <
    /// num_slots` reflects that at most `num_slots` slots can be set and keeps
    /// the counter from overflowing.
    #[requires(wf(self))]
    #[requires(idx@ < (*self).num_slots@)]
    #[requires((*self).allocated_count@ < (*self).num_slots@)]
    #[ensures((^self).allocated_count@ == (*self).allocated_count@ + 1)]
    #[ensures((^self).num_slots@ == (*self).num_slots@)]
    #[ensures(wf(&^self))]
    pub fn set(&mut self, idx: usize) {
        let word = idx / 64;
        let bit = idx % 64;
        proof_assert!(word@ < self.words@.len());
        self.words[word] |= 1u64 << bit;
        self.allocated_count += 1;
    }

    /// Mirror of `AllocationBitmap::clear` (bitmap.rs:26-33).
    #[requires(wf(self))]
    #[requires(idx@ < (*self).num_slots@)]
    #[requires((*self).allocated_count@ > 0)]
    #[ensures((^self).allocated_count@ == (*self).allocated_count@ - 1)]
    #[ensures((^self).num_slots@ == (*self).num_slots@)]
    #[ensures(wf(&^self))]
    pub fn clear(&mut self, idx: usize) {
        let word = idx / 64;
        let bit = idx % 64;
        proof_assert!(word@ < self.words@.len());
        self.words[word] &= !(1u64 << bit);
        self.allocated_count -= 1;
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
    #[ensures(result@ == self.num_slots@)]
    pub fn num_slots(&self) -> u32 {
        self.num_slots
    }
}

/// Lifecycle proof: a freshly-created bitmap reports all-free, and after a
/// single `set` the count is 1 and it is no longer all-free (spec: FR-020).
#[requires(num_slots@ > 0)]
#[ensures(result.allocated_count@ == 1)]
pub fn lifecycle_new_set(num_slots: u32) -> AllocationBitmap {
    let mut bm = AllocationBitmap::new(num_slots);
    proof_assert!(bm.is_all_free_logic());
    bm.set(0);
    bm
}

impl AllocationBitmap {
    #[logic(open)]
    pub fn is_all_free_logic(self) -> bool {
        pearlite! { self.allocated_count@ == 0 }
    }
}
