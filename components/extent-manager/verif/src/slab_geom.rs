//! Mirror of the slot<->byte-offset geometry in `src/slab.rs`.
//!
//! Source: `Slab::slot_offset` (slab.rs:58-60), `Slab::slot_for_offset`
//! (slab.rs:62-76), `Slab::contains_offset` (slab.rs:86-88).
//!
//! These functions are the sole mechanism by which `remove_extent(offset)`
//! (FR-008) locates the slot owning a caller-supplied disk byte offset, and by
//! which a reserved slot is turned into a disk offset for the WriteHandle
//! (FR-005). The key property is that the two directions are inverse: the
//! offset handed out by `slot_offset` maps back to the same slot index.

use creusot_std::prelude::*;

/// Mirror of `Slab::slot_offset` (slab.rs:58-60):
/// `self.start_offset + slot_index as u64 * self.element_size as u64`.
pub fn slot_offset(start_offset: u64, element_size: u32, slot_index: usize) -> u64 {
    start_offset + slot_index as u64 * element_size as u64
}

/// Mirror of `Slab::slot_for_offset` (slab.rs:62-76).
///
/// `num_slots` mirrors `self.bitmap.num_slots()`. `element_size` is `u32` in
/// the source and is required to be non-zero (a slab is never created with a
/// zero element size — `Slab::new` divides `slab_size / element_size`).
// Every valid answer refers to a real slot and round-trips through slot_offset.
// Anything below the slab base is rejected.
// Positive case: an in-range, element-aligned offset maps to its exact slot.
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

/// Mirror of `Slab::contains_offset` (slab.rs:86-88):
/// `byte_offset >= self.start_offset && byte_offset < self.start_offset + self.slab_size`.
pub fn contains_offset(start_offset: u64, slab_size: u64, byte_offset: u64) -> bool {
    byte_offset >= start_offset && byte_offset < start_offset + slab_size
}

/// Lifecycle / round-trip proof (spec: FR-008, FR-012).
///
/// The disk offset produced for a valid slot index maps back, through
/// `slot_for_offset`, to exactly that slot index. This is what makes
/// `remove_extent(offset)` locate the correct slot.
pub fn offset_roundtrip(
    start_offset: u64,
    element_size: u32,
    num_slots: u32,
    slot_index: usize,
) -> Option<usize> {
    let off = slot_offset(start_offset, element_size, slot_index);
    // off - start == slot_index * element_size, so it is element-aligned and
    // divides back to exactly slot_index.
                slot_for_offset(start_offset, element_size, num_slots, off)
}
