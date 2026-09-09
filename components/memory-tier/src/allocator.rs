//! First-fit free-list allocator for the memory-tier pool.

use std::collections::BTreeMap;

const ALIGNMENT: usize = 4096;

/// Round `size` up to the next 4 KiB (`ALIGNMENT`) boundary.
///
/// This is the single alignment primitive the allocator relies on (FR-004): every
/// `allocate` request and every `deallocate` accounting adjustment is aligned through
/// it, so a returned offset is 4 KiB-aligned and `used` is tracked in aligned units.
/// Factored out as a real production function so the alignment contract can be
/// verified with Kani independently of the (BTreeMap-backed, CBMC-intractable) container.
#[inline]
pub(crate) fn align_up(size: usize) -> usize {
    size.next_multiple_of(ALIGNMENT)
}

/// A first-fit free-list allocator over a contiguous byte region.
pub(crate) struct FreeList {
    /// Map of free region start offset → region size.
    free_regions: BTreeMap<usize, usize>,
    capacity: usize,
    used: usize,
}

impl FreeList {
    pub fn new(capacity: usize) -> Self {
        let mut free_regions = BTreeMap::new();
        if capacity > 0 {
            free_regions.insert(0, capacity);
        }
        Self {
            free_regions,
            capacity,
            used: 0,
        }
    }

    pub fn capacity(&self) -> usize {
        self.capacity
    }

    pub fn used(&self) -> usize {
        self.used
    }

    /// Allocate `size` bytes (rounded up to 4 KiB alignment).
    /// Returns the byte offset into the pool, or `None` if no space.
    pub fn allocate(&mut self, size: usize) -> Option<usize> {
        if size == 0 {
            return None;
        }
        let aligned_size = align_up(size);

        let (&offset, &region_size) = self.free_regions.iter().find(|(_, &s)| s >= aligned_size)?;

        self.free_regions.remove(&offset);

        let remaining = region_size - aligned_size;
        if remaining > 0 {
            self.free_regions.insert(offset + aligned_size, remaining);
        }

        self.used += aligned_size;
        Some(offset)
    }

    /// Return a previously allocated region to the free list.
    /// `size` must be the original requested size (will be aligned internally).
    pub fn deallocate(&mut self, offset: usize, size: usize) {
        let aligned_size = align_up(size);
        self.used -= aligned_size;

        let mut new_offset = offset;
        let mut new_size = aligned_size;

        // Coalesce with the preceding free region.
        if let Some((&prev_offset, &prev_size)) = self.free_regions.range(..offset).next_back() {
            if prev_offset + prev_size == offset {
                new_offset = prev_offset;
                new_size += prev_size;
                self.free_regions.remove(&prev_offset);
            }
        }

        // Coalesce with the following free region.
        let next_offset = new_offset + new_size;
        if let Some(&next_size) = self.free_regions.get(&next_offset) {
            new_size += next_size;
            self.free_regions.remove(&next_offset);
        }

        self.free_regions.insert(new_offset, new_size);
    }
}

// -----------------------------------------------------------------------------
// Kani formal-verification harnesses (clean-slate re-run, verif/kani/memory-tier-rerun).
// Authored fresh from verif_memory-tier_property_inventory.md. Two groups:
//   (1) NATIVE proofs of the alignment primitive `align_up` (ALIGN-4K) — pure
//       usize arithmetic, no container/pointers, proved over the full production
//       domain [1, u32::MAX].
//   (2) FreeList / BTreeMap allocator harnesses — authored and RUN to capture the
//       std-associative-container tool-boundary signature (USED-LE-CAP, USED-CONSERVE,
//       SPACE-REUSE, COALESCE), rather than inheriting the baseline's claim.
// -----------------------------------------------------------------------------
#[cfg(kani)]
mod verification {
    use super::*;

    // ---- (1) NATIVE: ALIGN-4K primitive (FR-004) ----------------------------

    /// ALIGN-ROUNDUP / ALIGN-4K. `align_up(size)` is the *tight* 4 KiB upper
    /// rounding: a multiple of ALIGNMENT, never below `size`, within one page of it;
    /// arithmetic never overflows/panics over the whole [1, u32::MAX] request domain.
    #[kani::proof]
    fn verify_align_up_contract() {
        let size: usize = kani::any();
        kani::assume(size > 0); // insert() rejects size == 0 (InvalidSize, FR-008)
        kani::assume(size <= u32::MAX as usize); // insert(key, size: u32) type bound

        let a = align_up(size);
        assert!(a % ALIGNMENT == 0, "FR-004: result is 4 KiB-aligned");
        assert!(a >= size, "aligned size never loses capacity");
        assert!(a - size < ALIGNMENT, "rounds up by strictly less than one page");
    }

    /// ALIGN-4K (accounting symmetry). `align_up` is idempotent, so re-aligning
    /// `slot.size` inside `deallocate` recovers exactly the block `allocate` charged
    /// to `used`. Underpins USED-CONSERVE's symmetric charge/discharge.
    #[kani::proof]
    fn verify_align_up_idempotent() {
        let size: usize = kani::any();
        kani::assume(size > 0 && size <= u32::MAX as usize);
        let a = align_up(size);
        assert!(align_up(a) == a, "aligning an aligned value is a no-op");
    }

    /// ALIGN-4K (first-fit consistency). `align_up` is monotonic non-decreasing,
    /// so the first-fit `region_size >= aligned_size` test is order-consistent.
    #[kani::proof]
    fn verify_align_up_monotonic() {
        let s1: usize = kani::any();
        let s2: usize = kani::any();
        kani::assume(s1 > 0 && s1 <= u32::MAX as usize);
        kani::assume(s2 > 0 && s2 <= u32::MAX as usize);
        kani::assume(s1 <= s2);
        assert!(align_up(s1) <= align_up(s2), "align_up is monotonic");
    }

    // ---- (2) BTreeMap allocator harnesses — RUN to capture the wall -----------
    // These call the REAL FreeList (BTreeMap-backed). Run under an 8-min CBMC
    // timebox; the outcome (green or container-wall signature) is recorded in
    // memory-tier_properties.md. Tiny geometry (1-2 pages) is deliberately chosen
    // to give CBMC its best chance.

    /// FreeList::new establishment: INIT-EMPTY (used==0) + CAP-CONST (capacity stored).
    /// One BTreeMap insert only.
    #[kani::proof]
    #[kani::unwind(4)]
    fn wall_freelist_new_establishes() {
        let cap: usize = 2 * ALIGNMENT;
        let fl = FreeList::new(cap);
        assert!(fl.used() == 0, "INIT-EMPTY: fresh allocator has used==0");
        assert!(fl.capacity() == cap, "CAP-CONST: capacity stored at new");
    }

    /// USED-LE-CAP + USED-CONSERVE (single allocate): after one allocate, used ==
    /// align_up(size) and used <= capacity. Calls the real BTreeMap allocate.
    #[kani::proof]
    #[kani::unwind(4)]
    fn wall_freelist_used_le_cap() {
        let cap: usize = 2 * ALIGNMENT;
        let mut fl = FreeList::new(cap);
        let size: usize = kani::any();
        kani::assume(size > 0 && size <= ALIGNMENT);
        if let Some(_off) = fl.allocate(size) {
            assert!(fl.used() == align_up(size), "USED-CONSERVE: used == aligned size");
            assert!(fl.used() <= fl.capacity(), "USED-LE-CAP");
        }
    }

    /// SPACE-REUSE (allocate → deallocate → used back to 0). Real BTreeMap
    /// allocate + deallocate + coalesce.
    #[kani::proof]
    #[kani::unwind(4)]
    fn wall_freelist_space_reuse() {
        let cap: usize = 2 * ALIGNMENT;
        let mut fl = FreeList::new(cap);
        if let Some(off) = fl.allocate(ALIGNMENT) {
            fl.deallocate(off, ALIGNMENT);
            assert!(fl.used() == 0, "SPACE-REUSE: dealloc returns bytes; used back to 0");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn allocate_single() {
        let mut fl = FreeList::new(1024 * 1024);
        let offset = fl.allocate(4096).unwrap();
        assert_eq!(offset, 0);
        assert_eq!(fl.used(), 4096);
    }

    #[test]
    fn allocate_rounds_up() {
        let mut fl = FreeList::new(1024 * 1024);
        let offset = fl.allocate(100).unwrap();
        assert_eq!(offset, 0);
        assert_eq!(fl.used(), 4096);
    }

    #[test]
    fn allocate_sequential() {
        let mut fl = FreeList::new(1024 * 1024);
        let a = fl.allocate(4096).unwrap();
        let b = fl.allocate(8192).unwrap();
        assert_eq!(a, 0);
        assert_eq!(b, 4096);
        assert_eq!(fl.used(), 4096 + 8192);
    }

    #[test]
    fn allocate_fails_when_full() {
        let mut fl = FreeList::new(8192);
        fl.allocate(8192).unwrap();
        assert!(fl.allocate(4096).is_none());
    }

    #[test]
    fn deallocate_and_reuse() {
        let mut fl = FreeList::new(8192);
        let a = fl.allocate(4096).unwrap();
        let _b = fl.allocate(4096).unwrap();
        fl.deallocate(a, 4096);
        let c = fl.allocate(4096).unwrap();
        assert_eq!(c, 0);
    }

    #[test]
    fn coalesce_adjacent() {
        let mut fl = FreeList::new(12288);
        let a = fl.allocate(4096).unwrap();
        let b = fl.allocate(4096).unwrap();
        let _c = fl.allocate(4096).unwrap();
        fl.deallocate(a, 4096);
        fl.deallocate(b, 4096);
        // Should coalesce into one 8192-byte region
        let d = fl.allocate(8192).unwrap();
        assert_eq!(d, 0);
    }

    #[test]
    fn coalesce_with_following() {
        let mut fl = FreeList::new(12288);
        let a = fl.allocate(4096).unwrap();
        let b = fl.allocate(4096).unwrap();
        let _c = fl.allocate(4096).unwrap();
        fl.deallocate(b, 4096);
        fl.deallocate(a, 4096);
        let d = fl.allocate(8192).unwrap();
        assert_eq!(d, 0);
    }

    #[test]
    fn zero_size_returns_none() {
        let mut fl = FreeList::new(1024 * 1024);
        assert!(fl.allocate(0).is_none());
    }

    #[test]
    fn capacity_tracking() {
        let fl = FreeList::new(65536);
        assert_eq!(fl.capacity(), 65536);
        assert_eq!(fl.used(), 0);
    }
}
