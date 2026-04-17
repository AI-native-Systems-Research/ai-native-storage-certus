use crate::metadata::BLOCK_SIZE;

const BITS_PER_WORD: usize = 64;
const BITS_PER_BLOCK: usize = BLOCK_SIZE * 8;

#[derive(Debug, Clone)]
pub(crate) struct AllocationBitmap {
    words: Vec<u64>,
    num_slots: u32,
}

impl AllocationBitmap {
    pub fn new(num_slots: u32) -> Self {
        let num_words = (num_slots as usize).div_ceil(BITS_PER_WORD);
        AllocationBitmap {
            words: vec![0u64; num_words],
            num_slots,
        }
    }

    pub fn set(&mut self, index: u32) {
        let (word_idx, bit_idx) = Self::position(index);
        if word_idx < self.words.len() {
            self.words[word_idx] |= 1u64 << bit_idx;
        }
    }

    pub fn clear(&mut self, index: u32) {
        let (word_idx, bit_idx) = Self::position(index);
        if word_idx < self.words.len() {
            self.words[word_idx] &= !(1u64 << bit_idx);
        }
    }

    pub fn is_set(&self, index: u32) -> bool {
        let (word_idx, bit_idx) = Self::position(index);
        if word_idx < self.words.len() {
            self.words[word_idx] & (1u64 << bit_idx) != 0
        } else {
            false
        }
    }

    pub fn find_free(&self) -> Option<u32> {
        for (word_idx, &word) in self.words.iter().enumerate() {
            if word != u64::MAX {
                let bit_idx = (!word).trailing_zeros() as usize;
                let slot = word_idx * BITS_PER_WORD + bit_idx;
                if (slot as u32) < self.num_slots {
                    return Some(slot as u32);
                }
            }
        }
        None
    }

    #[allow(dead_code)]
    pub fn count_allocated(&self) -> u32 {
        self.words.iter().map(|w| w.count_ones()).sum()
    }

    pub fn num_blocks(&self) -> u64 {
        let total_bits = self.num_slots as u64;
        total_bits.div_ceil(BITS_PER_BLOCK as u64)
    }

    pub fn serialize_to_blocks(&self) -> Vec<[u8; BLOCK_SIZE]> {
        let n = self.num_blocks() as usize;
        let mut blocks = vec![[0u8; BLOCK_SIZE]; n];
        let word_bytes: Vec<u8> = self.words.iter().flat_map(|w| w.to_le_bytes()).collect();

        for (i, block) in blocks.iter_mut().enumerate() {
            let start = i * BLOCK_SIZE;
            let end = (start + BLOCK_SIZE).min(word_bytes.len());
            if start < word_bytes.len() {
                block[..end - start].copy_from_slice(&word_bytes[start..end]);
            }
        }
        blocks
    }

    pub fn deserialize_from_blocks(blocks: &[[u8; BLOCK_SIZE]], num_slots: u32) -> Self {
        let num_words = (num_slots as usize).div_ceil(BITS_PER_WORD);
        let mut words = Vec::with_capacity(num_words);

        let all_bytes: Vec<u8> = blocks.iter().flat_map(|b| b.iter().copied()).collect();

        for i in 0..num_words {
            let offset = i * 8;
            if offset + 8 <= all_bytes.len() {
                let word = u64::from_le_bytes(all_bytes[offset..offset + 8].try_into().unwrap());
                words.push(word);
            } else {
                words.push(0);
            }
        }

        AllocationBitmap { words, num_slots }
    }

    #[allow(dead_code)]
    pub fn num_slots(&self) -> u32 {
        self.num_slots
    }

    fn position(index: u32) -> (usize, usize) {
        let idx = index as usize;
        (idx / BITS_PER_WORD, idx % BITS_PER_WORD)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn new_bitmap_all_free() {
        let bm = AllocationBitmap::new(100);
        for i in 0..100 {
            assert!(!bm.is_set(i));
        }
        assert_eq!(bm.count_allocated(), 0);
    }

    #[test]
    fn set_and_clear() {
        let mut bm = AllocationBitmap::new(128);
        bm.set(0);
        bm.set(63);
        bm.set(64);
        bm.set(127);
        assert!(bm.is_set(0));
        assert!(bm.is_set(63));
        assert!(bm.is_set(64));
        assert!(bm.is_set(127));
        assert!(!bm.is_set(1));
        assert_eq!(bm.count_allocated(), 4);

        bm.clear(63);
        assert!(!bm.is_set(63));
        assert_eq!(bm.count_allocated(), 3);
    }

    #[test]
    fn find_free_basic() {
        let mut bm = AllocationBitmap::new(64);
        assert_eq!(bm.find_free(), Some(0));
        bm.set(0);
        assert_eq!(bm.find_free(), Some(1));

        for i in 0..63 {
            bm.set(i);
        }
        assert_eq!(bm.find_free(), Some(63));
        bm.set(63);
        assert_eq!(bm.find_free(), None);
    }

    #[test]
    fn find_free_crosses_word_boundary() {
        let mut bm = AllocationBitmap::new(128);
        for i in 0..64 {
            bm.set(i);
        }
        assert_eq!(bm.find_free(), Some(64));
    }

    #[test]
    fn serialize_deserialize_roundtrip() {
        let mut bm = AllocationBitmap::new(200);
        bm.set(0);
        bm.set(99);
        bm.set(199);

        let blocks = bm.serialize_to_blocks();
        let restored = AllocationBitmap::deserialize_from_blocks(&blocks, 200);

        assert!(restored.is_set(0));
        assert!(restored.is_set(99));
        assert!(restored.is_set(199));
        assert!(!restored.is_set(1));
        assert_eq!(restored.count_allocated(), 3);
    }

    #[test]
    fn num_blocks_calculation() {
        assert_eq!(AllocationBitmap::new(1).num_blocks(), 1);
        assert_eq!(AllocationBitmap::new(32768).num_blocks(), 1);
        assert_eq!(AllocationBitmap::new(32769).num_blocks(), 2);
        assert_eq!(AllocationBitmap::new(10_000_000).num_blocks(), 306);
    }
}
