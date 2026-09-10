//! Global invariant **EM-DEFERRED-FREE** (rank 4, attachments 3) — crash-safety
//! core (FR-025). A slot freed by `remove_extent` is NOT reallocated until the
//! removal is persisted by a successful `checkpoint()`.
//!
//! The mechanism (region.rs:121-160): `remove_extent_by_offset` sets the slot
//! key to FREE_KEY and pushes `(slab_start, slot)` onto `pending_frees` — but
//! critically does **not** clear the allocation bitmap bit. Allocation
//! (`Slab::alloc_slot` -> `bitmap.find_free_from`, slab.rs:29-35, bitmap.rs:42-51)
//! only ever returns slots whose bitmap bit is clear, so a still-set bit makes
//! the slot un-reusable. Only `flush_pending_frees` — run by `run_checkpoint`
//! after the checkpoint + superblock writes succeed (lib.rs:312-321) — calls
//! `free_slot`, which clears the bitmap bit (region.rs:94-112, slab.rs:37-40)
//! and makes the slot reusable again.
//!
//! This module models one slot's (bitmap-bit, key, pending) triple and proves
//! the temporal invariant: reusable <-> bitmap bit clear; remove keeps the bit
//! set (=> not reusable) while clearing the key; flush clears the bit. Bundles:
//! remove_extent, checkpoint, reserve_extent. Re-authored from the inventory.

use creusot_std::prelude::*;

pub const FREE_KEY: u64 = u64::MAX;

/// One slot's deferred-free state: `allocated` is the bitmap bit,
/// `key` the dense key-vector entry, `pending_free` its membership in
/// `pending_frees`.
pub struct Slot {
    pub allocated: bool,
    pub key: u64,
    pub pending_free: bool,
}

/// A slot is reusable by the allocator iff its bitmap bit is clear — exactly the
/// `!is_set(idx)` test in `find_free_from` (bitmap.rs:46).
#[ensures(result == !s.allocated)]
pub fn is_reusable(s: &Slot) -> bool {
    !s.allocated
}

/// Mirror of `remove_extent_by_offset` (region.rs:143-149): set key to FREE_KEY,
/// push to pending — **bitmap bit unchanged (stays set)**. Therefore the slot is
/// NOT reusable after a remove, before checkpoint.
#[requires((*s).allocated)]
#[requires((*s).key@ != FREE_KEY@)]
#[ensures((^s).key@ == FREE_KEY@)]
// bitmap bit is NOT touched by remove — this is the crux of deferred free.
#[ensures((^s).allocated == (*s).allocated)]
#[ensures((^s).allocated == true)]
#[ensures((^s).pending_free == true)]
// crash-safety guarantee: still allocated => still not reusable.
#[ensures(!is_reusable_logic(&^s))]
pub fn deferred_remove(s: &mut Slot) {
    s.key = FREE_KEY;
    s.pending_free = true;
}

/// Mirror of `flush_pending_frees` -> `free_slot` (region.rs:152-160, 94-112):
/// run only on a successful checkpoint. Clears the bitmap bit and drops the
/// pending mark — the slot becomes reusable.
#[requires((*s).pending_free)]
#[ensures((^s).allocated == false)]
#[ensures((^s).pending_free == false)]
#[ensures(is_reusable_logic(&^s))]
pub fn flush_free(s: &mut Slot) {
    s.allocated = false;
    s.pending_free = false;
}

#[logic(open)]
pub fn is_reusable_logic(s: &Slot) -> bool {
    pearlite! { !s.allocated }
}

/// Lifecycle proof of the full FR-025 guarantee: reserve (allocated, key set) ->
/// remove -> the slot is NOT reusable (a concurrent/subsequent reserve cannot
/// pick it) -> checkpoint flush -> the slot IS reusable. The `proof_assert!` in
/// the middle is the crash-safety property: no reuse before checkpoint.
#[requires((*s).allocated)]
#[requires((*s).key@ != FREE_KEY@)]
#[ensures(is_reusable_logic(&^s))]
pub fn lifecycle_no_reuse_before_checkpoint(s: &mut Slot) {
    deferred_remove(s);
    // Between remove and checkpoint the slot must remain un-reusable.
    proof_assert!(!is_reusable_logic(s));
    flush_free(s);
}
