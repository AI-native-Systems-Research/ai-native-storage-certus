//! Creusot verification of the memory-tier `FreeList` allocator core.
//!
//! These are **standalone mirrors** of the arithmetic in
//! `components/memory-tier/src/allocator.rs`. The real `FreeList` keeps its free
//! regions in a `BTreeMap<usize, usize>`, which Creusot cannot model; the
//! container operations (first-fit iteration, `range(..offset).next_back()`,
//! `get(&next_offset)`) are therefore **trusted boundaries** — see
//! `verified_properties.md`. What is proved here is the *arithmetic and
//! accounting core* that runs once those container lookups have selected a
//! region: alignment, split, used-accounting, and coalescing offset math.
//!
//! Each mirror transcribes the corresponding statements of `allocate()` /
//! `deallocate()` byte-faithfully; the `verified_properties.md` file records the
//! source line correspondence for the drift check.

use creusot_std::prelude::*;

/// 4 KiB alignment, matching `allocator.rs::ALIGNMENT`.
pub const ALIGNMENT: usize = 4096;

// -------------------------------------------------------------------------
// P1 — FR-004 / SC-4: 4 KiB alignment of allocation sizes.
//
// Mirrors `let aligned_size = size.next_multiple_of(ALIGNMENT);`
// (allocator.rs:42). `next_multiple_of(4096)` == ceil(size / 4096) * 4096 for
// the `size > 0` domain guaranteed by the `size == 0` guard at allocator.rs:39.
// -------------------------------------------------------------------------

/// Round `size` up to the next multiple of 4096 (the pool alignment).
#[requires(size@ > 0)]
#[requires(size@ + 4095 <= usize::MAX@)] // overflow-free (add of ALIGNMENT-1)
#[ensures(result@ % 4096 == 0)] // aligned to 4 KiB
#[ensures(result@ >= size@)] // never returns fewer bytes than requested
#[ensures(result@ > 0)] // positive for positive request
#[ensures(result@ < size@ + 4096)] // minimal: internal waste is < 4096 bytes
pub fn align_up(size: usize) -> usize {
    let n = size + (ALIGNMENT - 1); // size + 4095, overflow-free by precondition
    let q = n / ALIGNMENT;
    let r = n % ALIGNMENT;
    // Division identity + remainder bounds, bridged for the SMT backend:
    //   n == q*4096 + r,  0 <= r < 4096   ⟹   size <= q*4096 <= size+4095
    // hence result = q*4096 is aligned, >= size, and < size + 4096.
    proof_assert!(n@ == q@ * 4096 + r@);
    proof_assert!(r@ < 4096);
    proof_assert!(q@ * 4096 >= size@);
    proof_assert!(q@ * 4096 <= size@ + 4095);
    let result = q * ALIGNMENT;
    // Divisibility of the product, bridged via Div_mult: (q*4096)/4096 == q,
    // hence (q*4096) mod 4096 == 0 by the division identity.
    proof_assert!(result@ == q@ * 4096);
    proof_assert!(result@ / 4096 == q@);
    proof_assert!(result@ % 4096 == 0);
    result
}

// -------------------------------------------------------------------------
// P2 — FR-008: zero-size allocation is rejected.
//
// Mirrors `if size == 0 { return None; }` (allocator.rs:39-41). `true` models
// "admit / proceed to search", `false` models the `None` early return.
// -------------------------------------------------------------------------

/// Decide whether an allocation request is admitted (non-zero size).
#[ensures(size@ == 0 ==> result == false)] // zero size is always rejected
#[ensures(size@ > 0 ==> result == true)] // any positive size is admitted
pub fn alloc_admits(size: usize) -> bool {
    if size == 0 {
        return false;
    }
    true
}

// -------------------------------------------------------------------------
// P3 — FR-010 / SC-5: split arithmetic + used accounting on allocate.
//
// Mirrors allocator.rs:44-54, the code that runs *after* the first-fit search
// has selected a free region `(offset, region_size)` with
// `region_size >= aligned_size`:
//
//     let remaining = region_size - aligned_size;   // :48
//     if remaining > 0 { free_regions.insert(offset + aligned_size, remaining); } // :49-51
//     self.used += aligned_size;                     // :53
//
// Returns (new_used, remaining, leftover_offset).
// -------------------------------------------------------------------------

/// Carve an aligned chunk out of a selected free region.
#[requires(aligned_size@ > 0)]
#[requires(aligned_size@ % 4096 == 0)]
#[requires(region_size@ >= aligned_size@)] // first-fit guarantee (allocator.rs:44)
#[requires(offset@ + region_size@ <= capacity@)] // region lies within the pool
#[requires(used@ + aligned_size@ <= capacity@)] // pool has room for the request
#[ensures(result.0@ == used@ + aligned_size@)] // used grows by exactly aligned_size
#[ensures(result.1@ == region_size@ - aligned_size@)] // remaining free bytes in region
#[ensures(result.2@ == offset@ + aligned_size@)] // leftover region starts here
#[ensures(result.0@ <= capacity@)] // used never exceeds capacity
#[ensures(result.2@ + result.1@ <= capacity@)] // leftover region stays within the pool
pub fn allocate_split(
    used: usize,
    capacity: usize,
    offset: usize,
    region_size: usize,
    aligned_size: usize,
) -> (usize, usize, usize) {
    let remaining = region_size - aligned_size;
    let leftover_offset = offset + aligned_size;
    let new_used = used + aligned_size;
    (new_used, remaining, leftover_offset)
}

// -------------------------------------------------------------------------
// P4 — accounting: used decreases by exactly aligned_size on deallocate.
//
// Mirrors `self.used -= aligned_size;` (allocator.rs:61) after
// `let aligned_size = size.next_multiple_of(ALIGNMENT);` (:60).
// -------------------------------------------------------------------------

/// Return `aligned_size` bytes to the pool's used accounting.
#[requires(used@ >= aligned_size@)] // can't free more than is in use
#[ensures(result@ == used@ - aligned_size@)]
pub fn deallocate_used(used: usize, aligned_size: usize) -> usize {
    used - aligned_size
}

// -------------------------------------------------------------------------
// P5 — FR-026: coalescing with the preceding free region.
//
// Mirrors allocator.rs:66-73:
//     if let Some((&prev_offset, &prev_size)) = range(..offset).next_back() {
//         if prev_offset + prev_size == offset {
//             new_offset = prev_offset;
//             new_size  += prev_size;
//             ...
//         }
//     }
// The BTreeMap lookup is trusted; the merge arithmetic is proved here.
// -------------------------------------------------------------------------

/// Coalesce a freed region `(offset, size)` with an adjacent preceding region.
#[requires(prev_offset@ + prev_size@ <= offset@)] // prev ends at or before offset (no overlap)
#[requires(prev_size@ + size@ <= usize::MAX@)] // merged size overflow-free
#[ensures(prev_offset@ + prev_size@ == offset@
    ==> result.0@ == prev_offset@ && result.1@ == prev_size@ + size@)] // adjacent → merged
#[ensures(prev_offset@ + prev_size@ < offset@
    ==> result.0@ == offset@ && result.1@ == size@)] // gap → unchanged
#[ensures(result.0@ + result.1@ == offset@ + size@)] // right endpoint is preserved
#[ensures(result.0@ <= offset@)] // coalescing only extends leftward
pub fn coalesce_prev(
    prev_offset: usize,
    prev_size: usize,
    offset: usize,
    size: usize,
) -> (usize, usize) {
    let mut new_offset = offset;
    let mut new_size = size;
    if prev_offset + prev_size == offset {
        new_offset = prev_offset;
        new_size += prev_size;
    }
    (new_offset, new_size)
}

// -------------------------------------------------------------------------
// P6 — FR-026: coalescing with the following free region.
//
// Mirrors allocator.rs:76-80:
//     let next_offset = new_offset + new_size;
//     if let Some(&next_size) = free_regions.get(&next_offset) {
//         new_size += next_size;
//         ...
//     }
// `get(&next_offset)` returning Some means a free region starts exactly at
// `new_offset + new_size`; the merge extends the region rightward.
// -------------------------------------------------------------------------

/// Coalesce the current region with an adjacent following region of `next_size`.
#[requires(new_size@ + next_size@ <= usize::MAX@)] // merged size overflow-free
#[ensures(result@ == new_size@ + next_size@)] // region grows by the following region's size
#[ensures(result@ >= new_size@)] // coalescing never shrinks the region
pub fn coalesce_next(new_size: usize, next_size: usize) -> usize {
    new_size + next_size
}

// -------------------------------------------------------------------------
// P7 — lifecycle: allocate then deallocate conserves `used` accounting.
//
// Composes allocate_split (P3) and deallocate_used (P4): carving a chunk and
// then freeing the same chunk restores `used` to its original value. This is
// the allocator's core accounting-conservation invariant.
// -------------------------------------------------------------------------

/// Prove that alloc-then-free round-trips the `used` counter to its start value.
#[requires(aligned_size@ > 0)]
#[requires(aligned_size@ % 4096 == 0)]
#[requires(region_size@ >= aligned_size@)]
#[requires(offset@ + region_size@ <= capacity@)]
#[requires(used@ + aligned_size@ <= capacity@)]
#[ensures(result@ == used@)] // used is exactly restored
pub fn lifecycle_alloc_free(
    used: usize,
    capacity: usize,
    offset: usize,
    region_size: usize,
    aligned_size: usize,
) -> usize {
    let (new_used, _remaining, _leftover) =
        allocate_split(used, capacity, offset, region_size, aligned_size);
    deallocate_used(new_used, aligned_size)
}

// -------------------------------------------------------------------------
// P8 — lifecycle: free-region offsets stay 4 KiB-aligned inductively.
//
// The pool starts as one region at offset 0 (aligned; allocator.rs:19). Every
// allocate carves an aligned chunk and the leftover region starts at
// `offset + aligned_size` (allocator.rs:50). This proves the inductive step:
// an aligned region start plus an aligned carve yields an aligned leftover
// start — hence every returned allocation offset is 4 KiB-aligned (FR-004).
// -------------------------------------------------------------------------

/// Given an aligned region start, the leftover region start is also aligned.
#[requires(offset@ % 4096 == 0)] // region starts aligned (base case: offset 0)
#[requires(size@ > 0)]
#[requires(offset@ + size@ + 4096 <= usize::MAX@)] // overflow-free through align_up
#[ensures(result@ % 4096 == 0)] // leftover region start is aligned
#[ensures(result@ >= offset@)] // leftover lies after the region start
pub fn leftover_offset_aligned(offset: usize, size: usize) -> usize {
    let aligned = align_up(size);
    offset + aligned
}
