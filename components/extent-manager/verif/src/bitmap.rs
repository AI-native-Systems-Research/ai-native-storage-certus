//! Faithful whole-function mirror of `AllocationBitmap` (src/bitmap.rs:1-64).
//!
//! Backing model for the per-slab allocation bitmap. Two families:
//!  * liveness counter `allocated_count` + word-index in-bounds (memory safety);
//!  * bit-level round-trip `set`/`clear`/`is_set` over `Vec<u64>` (`#[bitwise_proof]`).
//!
//! Inventory ids served (as an implementing obligation, per the inventory's
//! implementing-obligation map): backs EM-KEYVEC-MEMBERSHIP (slot allocation
//! record), EM-RESERVE-INVISIBLE-UNTIL-PUBLISH, EM-ABORT-RELEASES,
//! EM-DEFERRED-FREE (bitmap bit stays set across a deferred remove), and
//! EM-RECOVER-ROUNDTRIP (bitmap derived from keys). Re-authored from the
//! inventory in this worktree; not inherited from baseline.

use creusot_std::prelude::*;
use creusot_std::logic::ops::NthBitLogic;

pub struct AllocationBitmap {
    pub words: Vec<u64>,
    pub num_slots: u32,
    pub allocated_count: u32,
}

/// Well-formedness established by `new` (bitmap.rs:8-15): exactly
/// `ceil(num_slots/64)` words.
#[logic(open)]
pub fn wf(bm: &AllocationBitmap) -> bool {
    pearlite! { bm.words@.len() == (bm.num_slots@ + 63) / 64 }
}

/// The exact bit `is_set` reads (bitmap.rs:35-40): `(words[idx/64] >> idx%64) & 1`.
#[logic(open)]
pub fn slot_bit(bm: &AllocationBitmap, idx: Int) -> bool {
    pearlite! { bm.words@[idx / 64].nth_bit(idx % 64) }
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

    /// Mirror of `AllocationBitmap::set` (bitmap.rs:17-24): sets the bit,
    /// increments the counter; word index in bounds; wf + num_slots preserved.
    #[bitwise_proof]
    #[requires(wf(self))]
    #[requires(idx@ < (*self).num_slots@)]
    #[requires((*self).allocated_count@ < (*self).num_slots@)]
    #[ensures((^self).allocated_count@ == (*self).allocated_count@ + 1)]
    #[ensures((^self).num_slots@ == (*self).num_slots@)]
    #[ensures(wf(&^self))]
    #[ensures(slot_bit(&^self, idx@))]
    pub fn set(&mut self, idx: usize) {
        let word = idx / 64;
        let bit = idx % 64;
        proof_assert!(word@ < self.words@.len());
        self.words[word] |= 1u64 << bit;
        self.allocated_count += 1;
    }

    /// Mirror of `AllocationBitmap::clear` (bitmap.rs:26-33): clears the bit,
    /// decrements the counter.
    #[bitwise_proof]
    #[requires(wf(self))]
    #[requires(idx@ < (*self).num_slots@)]
    #[requires((*self).allocated_count@ > 0)]
    #[ensures((^self).allocated_count@ == (*self).allocated_count@ - 1)]
    #[ensures((^self).num_slots@ == (*self).num_slots@)]
    #[ensures(wf(&^self))]
    #[ensures(!slot_bit(&^self, idx@))]
    pub fn clear(&mut self, idx: usize) {
        let word = idx / 64;
        let bit = idx % 64;
        proof_assert!(word@ < self.words@.len());
        self.words[word] &= !(1u64 << bit);
        self.allocated_count -= 1;
    }

    /// Mirror of `AllocationBitmap::is_set` (bitmap.rs:35-40): the read equals
    /// the bit predicate — closes the round-trip.
    #[bitwise_proof]
    #[requires(wf(self))]
    #[requires(idx@ < self.num_slots@)]
    #[ensures(result == slot_bit(self, idx@))]
    pub fn is_set(&self, idx: usize) -> bool {
        let word = idx / 64;
        let bit = idx % 64;
        proof_assert!(word@ < self.words@.len());
        (self.words[word] >> bit) & 1 == 1
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

/// Bit-level round-trip: `is_set(idx)` is TRUE immediately after `set(idx)`.
/// Backs EM-KEYVEC-MEMBERSHIP's slot-allocation record.
#[requires(wf(&bm))]
#[requires(idx@ < bm.num_slots@)]
#[requires(bm.allocated_count@ < bm.num_slots@)]
#[ensures(result == true)]
pub fn roundtrip_set_then_is_set(mut bm: AllocationBitmap, idx: usize) -> bool {
    bm.set(idx);
    bm.is_set(idx)
}

/// Bit-level round-trip: `is_set(idx)` is FALSE immediately after `clear(idx)`.
#[requires(wf(&bm))]
#[requires(idx@ < bm.num_slots@)]
#[requires(bm.allocated_count@ > 0)]
#[ensures(result == false)]
pub fn roundtrip_clear_then_is_set(mut bm: AllocationBitmap, idx: usize) -> bool {
    bm.clear(idx);
    bm.is_set(idx)
}
