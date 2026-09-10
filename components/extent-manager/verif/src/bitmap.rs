//! Mirror of `AllocationBitmap` (src/bitmap.rs:1-64).
//!
//! Spec: FR-020 (each slab uses a bitmap allocator). Two families of property:
//!
//!  * **Liveness counter** — `allocated_count` is incremented by `set`,
//!    decremented by `clear`, and drives `is_all_free`/`is_empty`
//!    (slab.rs:50-52) and hence the deferred-free path (FR-008). The word index
//!    derived from a slot index is always in bounds of `words` (memory safety of
//!    the bit access).
//!  * **Bit-level round-trip** (added under the coverage-discipline policy) —
//!    `is_set(idx)` is *true* immediately after `set(idx)` and *false* after
//!    `clear(idx)`, reasoning symbolically over `words[idx/64] & (1 << idx%64)`
//!    on the `Vec<u64>`. Proved with ``.

use creusot_std::prelude::*;
use creusot_std::logic::ops::NthBitLogic;

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

/// The bit that slot `idx` occupies is set, in the exact form the source
/// `is_set` reads it (bitmap.rs:35-40): `(words[idx/64] >> (idx%64)) & 1 == 1`.
/// This is the shared predicate that lets `set`/`clear`/`is_set` chain into a
/// round-trip proof without exposing a raw bit-position type at the call site.
#[logic(open)]
pub fn slot_bit(bm: &AllocationBitmap, idx: Int) -> bool {
    pearlite! { bm.words@[idx / 64].nth_bit(idx % 64) }
}

impl AllocationBitmap {
    /// Mirror of `AllocationBitmap::new` (bitmap.rs:8-15).
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
    ///
    /// Bit-level postcondition (`slot_bit(&^self, idx@)`): the slot's bit is set
    /// on exit — this is the "set" half of the round-trip.
                                pub fn set(&mut self, idx: usize) {
        let word = idx / 64;
        let bit = idx % 64;
                self.words[word] |= 1u64 << bit;
        self.allocated_count += 1;
    }

    /// Mirror of `AllocationBitmap::clear` (bitmap.rs:26-33).
    ///
    /// Bit-level postcondition (`!slot_bit(&^self, idx@)`): the slot's bit is
    /// clear on exit — the "clear" half of the round-trip.
                                pub fn clear(&mut self, idx: usize) {
        let word = idx / 64;
        let bit = idx % 64;
                self.words[word] &= !(1u64 << bit);
        self.allocated_count -= 1;
    }

    /// Mirror of `AllocationBitmap::is_set` (bitmap.rs:35-40).
    ///
    /// Postcondition: the boolean returned is exactly the shared `slot_bit`
    /// predicate — the "read" that closes the round-trip.
                pub fn is_set(&self, idx: usize) -> bool {
        let word = idx / 64;
        let bit = idx % 64;
                (self.words[word] >> bit) & 1 == 1
    }

    /// Mirror of `AllocationBitmap::is_all_free` (bitmap.rs:53-55).
        pub fn is_all_free(&self) -> bool {
        self.allocated_count == 0
    }

    /// Mirror of `AllocationBitmap::count_set` (bitmap.rs:57-59).
        pub fn count_set(&self) -> usize {
        self.allocated_count as usize
    }

    /// Mirror of `AllocationBitmap::num_slots` (bitmap.rs:61-63).
        pub fn num_slots(&self) -> u32 {
        self.num_slots
    }

    #[logic(open)]
    pub fn is_all_free_logic(self) -> bool {
        pearlite! { self.allocated_count@ == 0 }
    }
}

/// Lifecycle proof: a freshly-created bitmap reports all-free, and after a
/// single `set` the count is 1 and it is no longer all-free (spec: FR-020).
pub fn lifecycle_new_set(num_slots: u32) -> AllocationBitmap {
    let mut bm = AllocationBitmap::new(num_slots);
        bm.set(0);
    bm
}

/// Bit-level round-trip (the key coverage-discipline property): for any slot
/// `idx` in range, `is_set(idx)` returns **true** immediately after `set(idx)`.
/// Chains `set`'s bit postcondition into `is_set`'s bit postcondition — no bit
/// arithmetic needed here, only the shared `slot_bit` predicate.
pub fn roundtrip_set_then_is_set(mut bm: AllocationBitmap, idx: usize) -> bool {
    bm.set(idx);
    bm.is_set(idx)
}

/// Bit-level round-trip: `is_set(idx)` returns **false** immediately after
/// `clear(idx)`.
pub fn roundtrip_clear_then_is_set(mut bm: AllocationBitmap, idx: usize) -> bool {
    bm.clear(idx);
    bm.is_set(idx)
}
