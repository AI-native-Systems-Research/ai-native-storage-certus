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
