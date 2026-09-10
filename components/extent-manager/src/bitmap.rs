pub(crate) struct AllocationBitmap {
    words: Vec<u64>,
    num_slots: u32,
    allocated_count: u32,
}

impl AllocationBitmap {
    pub fn new(num_slots: u32) -> Self {
        let num_words = (num_slots as usize + 63) / 64;
        Self {
            words: vec![0u64; num_words],
            num_slots,
            allocated_count: 0,
        }
    }

    pub fn set(&mut self, idx: usize) {
        debug_assert!((idx as u32) < self.num_slots);
        debug_assert!(!self.is_set(idx), "double-set on slot {idx}");
        let word = idx / 64;
        let bit = idx % 64;
        self.words[word] |= 1u64 << bit;
        self.allocated_count += 1;
    }

    pub fn clear(&mut self, idx: usize) {
        debug_assert!((idx as u32) < self.num_slots);
        debug_assert!(self.is_set(idx), "double-clear on slot {idx}");
        let word = idx / 64;
        let bit = idx % 64;
        self.words[word] &= !(1u64 << bit);
        self.allocated_count -= 1;
    }

    pub fn is_set(&self, idx: usize) -> bool {
        debug_assert!((idx as u32) < self.num_slots);
        let word = idx / 64;
        let bit = idx % 64;
        (self.words[word] >> bit) & 1 == 1
    }

    pub fn find_free_from(&self, start: usize) -> Option<usize> {
        let n = self.num_slots as usize;
        for i in 0..n {
            let idx = (start + i) % n;
            if !self.is_set(idx) {
                return Some(idx);
            }
        }
        None
    }

    pub fn is_all_free(&self) -> bool {
        self.allocated_count == 0
    }

    pub fn count_set(&self) -> usize {
        self.allocated_count as usize
    }

    pub fn num_slots(&self) -> u32 {
        self.num_slots
    }
}

// ---------------------------------------------------------------------------
// Kani harnesses (re-authored from the property inventory, clean-slate re-run).
//
// The AllocationBitmap is the slot-occupancy record underneath every slab. In
// inventory terms it is the implementing machinery for:
//   * EM-KEYVEC-MEMBERSHIP  (a slot is allocated iff its bit is set; parallel to keys)
//   * EM-RESERVE-SIZECLASS  (alloc_slot finds a free bit; full slab => no free bit)
// The bitmap is a plain Vec<u64> with pure integer/bit arithmetic — the
// tractable core Kani proves exhaustively at a bounded slot count.
// ---------------------------------------------------------------------------
#[cfg(kani)]
mod verification {
    use super::*;

    // Bounded slot count. `find_free_from` scans 0..num_slots, so this bounds the
    // loop; 8 slots exercise the sub-word (< 64) packing path in one u64 word.
    const N: u32 = 8;

    // [EM-KEYVEC-MEMBERSHIP impl] set(idx) on a free slot marks exactly that slot
    // allocated and increments the allocated count by one; is_set agrees with set.
    #[kani::proof]
    #[kani::unwind(9)]
    fn verify_set_marks_slot() {
        let mut bm = AllocationBitmap::new(N);
        let idx: usize = kani::any();
        kani::assume(idx < N as usize); // production guard: idx < num_slots (debug_assert in set)
        assert!(!bm.is_set(idx));
        let before = bm.count_set();
        bm.set(idx);
        assert!(bm.is_set(idx));
        assert!(bm.count_set() == before + 1);
    }

    // [EM-KEYVEC-MEMBERSHIP impl] set then clear returns the bitmap to the original
    // (empty) state — slot free again, count restored to zero.
    #[kani::proof]
    #[kani::unwind(9)]
    fn verify_set_clear_round_trip() {
        let mut bm = AllocationBitmap::new(N);
        let idx: usize = kani::any();
        kani::assume(idx < N as usize);
        bm.set(idx);
        assert!(bm.is_set(idx));
        bm.clear(idx);
        assert!(!bm.is_set(idx));
        assert!(bm.is_all_free());
        assert!(bm.count_set() == 0);
    }

    // [EM-KEYVEC-MEMBERSHIP impl] distinct slots do not alias — setting one slot
    // never marks a different slot.
    #[kani::proof]
    #[kani::unwind(9)]
    fn verify_set_independent() {
        let mut bm = AllocationBitmap::new(N);
        let i: usize = kani::any();
        let j: usize = kani::any();
        kani::assume(i < N as usize && j < N as usize && i != j);
        bm.set(i);
        assert!(bm.is_set(i));
        assert!(!bm.is_set(j)); // j was never set; must remain free
        assert!(bm.count_set() == 1);
    }

    // [EM-RESERVE-SIZECLASS impl] find_free_from on a fresh (all-free) bitmap returns
    // a valid, in-bounds, genuinely-free slot for any start index.
    #[kani::proof]
    #[kani::unwind(9)]
    fn verify_find_free_from_valid() {
        let bm = AllocationBitmap::new(N);
        let start: usize = kani::any();
        kani::assume(start < N as usize);
        let found = bm.find_free_from(start);
        match found {
            Some(idx) => {
                assert!(idx < N as usize);
                assert!(!bm.is_set(idx));
            }
            None => assert!(false), // an all-free bitmap must always find a slot
        }
    }

    // [EM-RESERVE-SIZECLASS impl / EM-RESERVE-OUTOFSPACE] a fully-allocated bitmap
    // yields no free slot (find_free_from == None) and the count equals num_slots.
    #[kani::proof]
    #[kani::unwind(9)]
    fn verify_find_free_from_full() {
        let mut bm = AllocationBitmap::new(N);
        for i in 0..N as usize {
            bm.set(i);
        }
        assert!(bm.count_set() == N as usize);
        let start: usize = kani::any();
        kani::assume(start < N as usize);
        assert!(bm.find_free_from(start).is_none());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn set_clear_round_trip() {
        let mut bm = AllocationBitmap::new(128);
        assert!(!bm.is_set(0));
        bm.set(0);
        assert!(bm.is_set(0));
        bm.clear(0);
        assert!(!bm.is_set(0));
    }

    #[test]
    fn find_free_from_wraps() {
        let mut bm = AllocationBitmap::new(4);
        bm.set(1);
        bm.set(2);
        bm.set(3);
        assert_eq!(bm.find_free_from(1), Some(0));
    }

    #[test]
    fn find_free_returns_none_when_full() {
        let mut bm = AllocationBitmap::new(64);
        for i in 0..64 {
            bm.set(i);
        }
        assert_eq!(bm.find_free_from(0), None);
    }

    #[test]
    fn is_all_free() {
        let mut bm = AllocationBitmap::new(100);
        assert!(bm.is_all_free());
        bm.set(50);
        assert!(!bm.is_all_free());
        bm.clear(50);
        assert!(bm.is_all_free());
    }

    #[test]
    fn count_set_correct() {
        let mut bm = AllocationBitmap::new(130);
        bm.set(0);
        bm.set(63);
        bm.set(64);
        bm.set(129);
        assert_eq!(bm.count_set(), 4);
    }

    #[test]
    fn non_multiple_of_64() {
        let mut bm = AllocationBitmap::new(3);
        bm.set(0);
        bm.set(1);
        bm.set(2);
        assert_eq!(bm.count_set(), 3);
        assert_eq!(bm.find_free_from(0), None);
        bm.clear(1);
        assert_eq!(bm.find_free_from(0), Some(1));
        assert!(!bm.is_all_free());
    }
}
