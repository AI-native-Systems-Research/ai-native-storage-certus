//! Allocation bitmap for tracking free/used extent slots per size class.

use crate::block_device::BlockDevice;
use crate::metadata::BLOCK_SIZE;

/// Per-size-class allocation bitmap.
///
/// Tracks which extent slots are allocated (1) or free (0).
/// The bitmap is stored as a `Vec<u8>` in memory and persisted
/// to contiguous 4KiB blocks on the block device.
///
/// # Examples
///
/// ```ignore
/// use extent_manager::bitmap::AllocationBitmap;
///
/// let mut bm = AllocationBitmap::new(100);
/// assert_eq!(bm.find_first_free(), Some(0));
/// bm.set(0);
/// assert_eq!(bm.find_first_free(), Some(1));
/// bm.clear(0);
/// assert_eq!(bm.find_first_free(), Some(0));
/// ```
pub(crate) struct AllocationBitmap {
    /// Raw bitmap bytes. Bit i of byte i/8 is (bytes[i/8] >> (i%8)) & 1.
    bytes: Vec<u8>,
    /// Total number of slots tracked.
    num_slots: u32,
}

impl AllocationBitmap {
    /// Create a new bitmap with `num_slots` slots, all initially free.
    pub(crate) fn new(num_slots: u32) -> Self {
        let num_bytes = (num_slots as usize).div_ceil(8);
        // Round up to full blocks for persistence alignment.
        let aligned_bytes = num_bytes.div_ceil(BLOCK_SIZE) * BLOCK_SIZE;
        Self {
            bytes: vec![0u8; aligned_bytes],
            num_slots,
        }
    }

    /// Number of 4KiB blocks needed to store this bitmap on disk.
    pub(crate) fn block_count(&self) -> u64 {
        (self.bytes.len() / BLOCK_SIZE) as u64
    }

    /// Total number of slots tracked.
    pub(crate) fn num_slots(&self) -> u32 {
        self.num_slots
    }

    /// Check if a slot is allocated.
    pub(crate) fn is_set(&self, slot: u32) -> bool {
        if slot >= self.num_slots {
            return false;
        }
        let byte_idx = slot as usize / 8;
        let bit_idx = slot as usize % 8;
        (self.bytes[byte_idx] >> bit_idx) & 1 == 1
    }

    /// Mark a slot as allocated.
    pub(crate) fn set(&mut self, slot: u32) {
        if slot < self.num_slots {
            let byte_idx = slot as usize / 8;
            let bit_idx = slot as usize % 8;
            self.bytes[byte_idx] |= 1 << bit_idx;
        }
    }

    /// Mark a slot as free.
    pub(crate) fn clear(&mut self, slot: u32) {
        if slot < self.num_slots {
            let byte_idx = slot as usize / 8;
            let bit_idx = slot as usize % 8;
            self.bytes[byte_idx] &= !(1 << bit_idx);
        }
    }

    /// Find the first free (0) slot. Returns `None` if all slots are allocated.
    pub(crate) fn find_first_free(&self) -> Option<u32> {
        for (byte_idx, &byte) in self.bytes.iter().enumerate() {
            if byte != 0xFF {
                for bit in 0..8u32 {
                    let slot = byte_idx as u32 * 8 + bit;
                    if slot >= self.num_slots {
                        return None;
                    }
                    if (byte >> bit) & 1 == 0 {
                        return Some(slot);
                    }
                }
            }
        }
        None
    }

    /// Count the number of allocated slots.
    pub(crate) fn count_allocated(&self) -> u32 {
        let mut count = 0u32;
        for slot in 0..self.num_slots {
            if self.is_set(slot) {
                count += 1;
            }
        }
        count
    }

    /// Persist the bitmap to the block device starting at `start_lba`.
    ///
    /// Each 4KiB chunk of the bitmap is written as a separate atomic block.
    pub(crate) fn persist(&self, bd: &BlockDevice, start_lba: u64) -> Result<(), String> {
        let num_blocks = self.block_count();
        for i in 0..num_blocks {
            let offset = i as usize * BLOCK_SIZE;
            let block: [u8; BLOCK_SIZE] =
                self.bytes[offset..offset + BLOCK_SIZE].try_into().unwrap();
            bd.write_block(start_lba + i, &block)?;
        }
        Ok(())
    }

    /// Persist only the block containing the given slot.
    ///
    /// This is used after a single set/clear operation to minimize I/O.
    pub(crate) fn persist_block_for_slot(
        &self,
        bd: &BlockDevice,
        start_lba: u64,
        slot: u32,
    ) -> Result<(), String> {
        let byte_idx = slot as usize / 8;
        let block_idx = byte_idx / BLOCK_SIZE;
        let offset = block_idx * BLOCK_SIZE;
        let block: [u8; BLOCK_SIZE] = self.bytes[offset..offset + BLOCK_SIZE].try_into().unwrap();
        bd.write_block(start_lba + block_idx as u64, &block)
    }

    /// Load the bitmap from the block device starting at `start_lba`.
    pub(crate) fn load(bd: &BlockDevice, start_lba: u64, num_slots: u32) -> Result<Self, String> {
        let mut bm = Self::new(num_slots);
        let num_blocks = bm.block_count();
        for i in 0..num_blocks {
            let offset = i as usize * BLOCK_SIZE;
            let block: &mut [u8; BLOCK_SIZE] = (&mut bm.bytes[offset..offset + BLOCK_SIZE])
                .try_into()
                .unwrap();
            bd.read_block(start_lba + i, block)?;
        }
        Ok(bm)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn new_bitmap_all_free() {
        let bm = AllocationBitmap::new(100);
        for i in 0..100 {
            assert!(!bm.is_set(i), "slot {i} should be free");
        }
    }

    #[test]
    fn set_and_check() {
        let mut bm = AllocationBitmap::new(64);
        bm.set(0);
        bm.set(7);
        bm.set(63);
        assert!(bm.is_set(0));
        assert!(bm.is_set(7));
        assert!(bm.is_set(63));
        assert!(!bm.is_set(1));
    }

    #[test]
    fn clear_after_set() {
        let mut bm = AllocationBitmap::new(32);
        bm.set(5);
        assert!(bm.is_set(5));
        bm.clear(5);
        assert!(!bm.is_set(5));
    }

    #[test]
    fn find_first_free_empty() {
        let bm = AllocationBitmap::new(10);
        assert_eq!(bm.find_first_free(), Some(0));
    }

    #[test]
    fn find_first_free_partial() {
        let mut bm = AllocationBitmap::new(10);
        bm.set(0);
        bm.set(1);
        bm.set(2);
        assert_eq!(bm.find_first_free(), Some(3));
    }

    #[test]
    fn find_first_free_full() {
        let mut bm = AllocationBitmap::new(8);
        for i in 0..8 {
            bm.set(i);
        }
        assert_eq!(bm.find_first_free(), None);
    }

    #[test]
    fn count_allocated() {
        let mut bm = AllocationBitmap::new(100);
        assert_eq!(bm.count_allocated(), 0);
        bm.set(10);
        bm.set(20);
        bm.set(30);
        assert_eq!(bm.count_allocated(), 3);
    }

    #[test]
    fn block_count_small() {
        let bm = AllocationBitmap::new(100);
        assert_eq!(bm.block_count(), 1); // 100 bits < 32768 bits per block
    }

    #[test]
    fn block_count_large() {
        let bm = AllocationBitmap::new(100_000);
        // 100K bits / 32768 bits per block = 4 blocks (ceil)
        assert_eq!(bm.block_count(), 4);
    }

    #[test]
    fn is_set_out_of_range() {
        let bm = AllocationBitmap::new(10);
        assert!(!bm.is_set(10));
        assert!(!bm.is_set(100));
    }

    #[test]
    fn set_out_of_range_is_noop() {
        let mut bm = AllocationBitmap::new(10);
        bm.set(10); // should not panic
        assert!(!bm.is_set(10));
    }

    #[test]
    fn find_first_free_skips_padding_bits() {
        // 10 slots, but bitmap bytes cover 16 bits (2 bytes).
        // Slots 0-9 used. find_first_free should return None, not 10.
        let mut bm = AllocationBitmap::new(10);
        for i in 0..10 {
            bm.set(i);
        }
        assert_eq!(bm.find_first_free(), None);
    }
}
