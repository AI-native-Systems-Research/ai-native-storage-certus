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
