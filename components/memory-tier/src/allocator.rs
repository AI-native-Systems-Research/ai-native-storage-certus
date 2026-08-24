//! First-fit free-list allocator for the memory-tier pool.

use std::collections::BTreeMap;

const ALIGNMENT: usize = 4096;

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
        let aligned_size = size.next_multiple_of(ALIGNMENT);

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
        let aligned_size = size.next_multiple_of(ALIGNMENT);
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
// Kani formal-verification harnesses.
//
// These prove properties of the `FreeList` allocator — the pure, pointer-free
// core of the memory-tier — over the full symbolic input domain (bounded by the
// unwind depth). The pointer/mmap/SPDK-FFI and `RwLock` layers in `lib.rs` are
// out of scope for Kani (raw pointers and FFI are not modelled); see
// `verified_properties.md` for the bounded-scope statement.
//
// Each harness calls the REAL `FreeList` functions and mirrors the production
// preconditions with `kani::assume`:
//   * `size > 0`                 — `insert()` rejects zero size (InvalidSize, FR-008).
//   * `size <= u32::MAX as usize`— `IMemoryTier::insert(key, size: u32)` type bound;
//                                  `deallocate` likewise receives `slot.size as usize`.
//   * `cap % ALIGNMENT == 0`     — `initialize()` pool sizes are page multiples.
// -----------------------------------------------------------------------------
#[cfg(kani)]
mod verification {
    use super::*;

    /// Upper bound on the modelled pool capacity. Keeps solver arithmetic
    /// tractable while still covering the full u32 allocation-size domain.
    const MAX_CAP: usize = 1 << 40; // 1 TiB

    /// [FR-004 / FR-007] A successful allocation returns a 4 KiB-aligned offset
    /// that lies wholly within the pool, and never violates `used <= capacity`.
    #[kani::proof]
    #[kani::unwind(2)]
    fn verify_allocate_alignment_and_bounds() {
        let cap: usize = kani::any();
        kani::assume(cap > 0 && cap <= MAX_CAP && cap % ALIGNMENT == 0);
        let mut fl = FreeList::new(cap);

        let size: usize = kani::any();
        kani::assume(size > 0); // insert() rejects size == 0 (InvalidSize)
        kani::assume(size <= u32::MAX as usize); // insert(size: u32) type bound

        if let Some(offset) = fl.allocate(size) {
            assert!(offset % ALIGNMENT == 0, "FR-004: offset is 4 KiB-aligned");
            let aligned = size.next_multiple_of(ALIGNMENT);
            assert!(offset + aligned <= cap, "allocation lies within the pool");
            assert!(fl.used() <= fl.capacity(), "used never exceeds capacity");
        }
    }

    /// [FR-010] When the request (aligned up) exceeds the entire free pool,
    /// `allocate` returns `None` — the `PoolFull` path taken by `insert()`.
    #[kani::proof]
    #[kani::unwind(2)]
    fn verify_allocate_poolfull() {
        let cap: usize = kani::any();
        kani::assume(cap <= MAX_CAP && cap % ALIGNMENT == 0);
        let mut fl = FreeList::new(cap);

        let size: usize = kani::any();
        kani::assume(size > 0 && size <= u32::MAX as usize);
        let aligned = size.next_multiple_of(ALIGNMENT);
        kani::assume(aligned > cap);

        assert!(
            fl.allocate(size).is_none(),
            "FR-010: over-capacity request yields None"
        );
    }

    /// [Accounting invariant / symmetric op] `allocate` then `deallocate` of the
    /// same region restores `used` exactly and never underflows.
    #[kani::proof]
    #[kani::unwind(3)]
    fn verify_alloc_dealloc_roundtrip() {
        let cap: usize = kani::any();
        kani::assume(cap > 0 && cap <= MAX_CAP && cap % ALIGNMENT == 0);
        let mut fl = FreeList::new(cap);

        let size: usize = kani::any();
        kani::assume(size > 0 && size <= u32::MAX as usize);

        let before = fl.used(); // 0 on a fresh pool
        if let Some(off) = fl.allocate(size) {
            let aligned = size.next_multiple_of(ALIGNMENT);
            assert!(fl.used() == before + aligned, "used grows by aligned size");
            fl.deallocate(off, size);
            assert!(
                fl.used() == before,
                "round-trip restores used (no underflow)"
            );
            assert!(fl.capacity() == cap, "capacity is unchanged");
        }
    }

    /// [FR-026] Deallocating two adjacent regions coalesces them: the combined
    /// span is reallocatable as a single region at the lower offset.
    ///
    /// Uses a concrete capacity: coalescing is a structural property of the
    /// free-list (independent of the exact pool size), and a symbolic capacity
    /// makes the multi-step alloc/dealloc arithmetic intractable for CBMC.
    #[kani::proof]
    #[kani::unwind(5)]
    fn verify_coalesce_adjacent() {
        let cap: usize = 3 * ALIGNMENT; // 12 KiB — room for two page-blocks plus slack
        let mut fl = FreeList::new(cap);

        let a = fl.allocate(ALIGNMENT).unwrap(); // offset 0
        let b = fl.allocate(ALIGNMENT).unwrap(); // offset 4096
        assert!(a == 0 && b == ALIGNMENT, "sequential first-fit offsets");

        fl.deallocate(a, ALIGNMENT);
        fl.deallocate(b, ALIGNMENT);

        assert!(
            fl.allocate(2 * ALIGNMENT) == Some(0),
            "FR-026: adjacent frees coalesce into one region at offset 0"
        );
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
