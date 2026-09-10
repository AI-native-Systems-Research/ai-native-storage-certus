//! Slot <-> byte-offset geometry (src/slab.rs:58-88). Backs two obligations:
//!  * **EM-SECTOR-ALIGN** (slot_offset = start + idx*element_size is aligned by
//!    construction) and the reserved-extent offset handed to the WriteHandle;
//!  * **EM-REMOVE-NOTFOUND** / **EM-REMOVE-OFFSET-ROUTING** — `slot_for_offset`
//!    rejects out-of-range/misaligned offsets (None) and, for a valid slot,
//!    round-trips: the offset produced by `slot_offset` maps back to that exact
//!    slot (so remove_extent(offset) locates the right slot). Source: FR-005,
//!    FR-008, FR-012. Re-authored from the inventory in this worktree.

use creusot_std::prelude::*;

/// Mirror of `Slab::slot_offset` (slab.rs:58-60).
#[requires(start_offset@ + slot_index@ * element_size@ <= u64::MAX@)]
#[ensures(result@ == start_offset@ + slot_index@ * element_size@)]
#[ensures(result@ >= start_offset@)]
pub fn slot_offset(start_offset: u64, element_size: u32, slot_index: usize) -> u64 {
    start_offset + slot_index as u64 * element_size as u64
}

/// Mirror of `Slab::slot_for_offset` (slab.rs:62-76). `num_slots` mirrors
/// `self.bitmap.num_slots()`; `element_size > 0` (a slab is never zero-element).
#[requires(element_size@ > 0)]
// Every Some answer refers to a real slot and round-trips through slot_offset.
#[ensures(forall<i: usize> result == Some(i) ==>
    i@ < num_slots@
    && byte_offset@ == start_offset@ + i@ * element_size@)]
// Anything below the slab base is rejected.
#[ensures(byte_offset@ < start_offset@ ==> result == None)]
// Positive case: an in-range, element-aligned offset maps to its exact slot.
#[ensures(
    (start_offset@ <= byte_offset@
     && (byte_offset@ - start_offset@) % element_size@ == 0
     && (byte_offset@ - start_offset@) / element_size@ < num_slots@)
    ==> exists<i: usize> result == Some(i)
        && i@ == (byte_offset@ - start_offset@) / element_size@)]
pub fn slot_for_offset(
    start_offset: u64,
    element_size: u32,
    num_slots: u32,
    byte_offset: u64,
) -> Option<usize> {
    if byte_offset < start_offset {
        return None;
    }
    let relative = byte_offset - start_offset;
    if relative % element_size as u64 != 0 {
        return None;
    }
    let idx = (relative / element_size as u64) as usize;
    if idx < num_slots as usize {
        Some(idx)
    } else {
        None
    }
}

/// Mirror of `Slab::contains_offset` (slab.rs:86-88).
#[requires(start_offset@ + slab_size@ <= u64::MAX@)]
#[ensures(result == (start_offset@ <= byte_offset@ && byte_offset@ < start_offset@ + slab_size@))]
pub fn contains_offset(start_offset: u64, slab_size: u64, byte_offset: u64) -> bool {
    byte_offset >= start_offset && byte_offset < start_offset + slab_size
}

/// **EM-REMOVE-OFFSET-ROUTING** round-trip (FR-008, FR-012): the disk offset
/// produced for a valid slot index maps back, through `slot_for_offset`, to
/// exactly that slot index — what makes remove_extent(offset) locate the slot.
#[requires(element_size@ > 0)]
#[requires(slot_index@ < num_slots@)]
#[requires(start_offset@ + slot_index@ * element_size@ <= u64::MAX@)]
#[ensures(result == Some(slot_index))]
pub fn offset_roundtrip(
    start_offset: u64,
    element_size: u32,
    num_slots: u32,
    slot_index: usize,
) -> Option<usize> {
    let off = slot_offset(start_offset, element_size, slot_index);
    proof_assert!(off@ - start_offset@ == slot_index@ * element_size@);
    proof_assert!((slot_index@ * element_size@) % element_size@ == 0);
    proof_assert!((slot_index@ * element_size@) / element_size@ == slot_index@);
    slot_for_offset(start_offset, element_size, num_slots, off)
}
