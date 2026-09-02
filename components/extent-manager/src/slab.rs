use std::collections::HashMap;

use crate::bitmap::AllocationBitmap;

pub const FREE_KEY: u64 = u64::MAX;

pub(crate) struct Slab {
    pub start_offset: u64,
    pub slab_size: u64,
    pub element_size: u32,
    pub bitmap: AllocationBitmap,
    pub keys: Vec<u64>,
    rover: usize,
}

impl Slab {
    pub fn new(start_offset: u64, slab_size: u64, element_size: u32) -> Self {
        let num_slots = (slab_size / element_size as u64) as u32;
        Self {
            start_offset,
            slab_size,
            element_size,
            bitmap: AllocationBitmap::new(num_slots),
            keys: vec![FREE_KEY; num_slots as usize],
            rover: 0,
        }
    }

    pub fn alloc_slot(&mut self) -> Option<(usize, u64)> {
        let idx = self.bitmap.find_free_from(self.rover)?;
        self.bitmap.set(idx);
        self.rover = (idx + 1) % self.bitmap.num_slots() as usize;
        let offset = self.slot_offset(idx);
        Some((idx, offset))
    }

    pub fn free_slot(&mut self, slot_index: usize) {
        self.bitmap.clear(slot_index);
        self.keys[slot_index] = FREE_KEY;
    }

    pub fn set_key(&mut self, slot_index: usize, key: u64) {
        self.keys[slot_index] = key;
    }

    pub fn get_key(&self, slot_index: usize) -> u64 {
        self.keys[slot_index]
    }

    pub fn is_empty(&self) -> bool {
        self.bitmap.is_all_free()
    }

    pub fn is_full(&self) -> bool {
        self.bitmap.count_set() == self.bitmap.num_slots() as usize
    }

    pub fn slot_offset(&self, slot_index: usize) -> u64 {
        self.start_offset + slot_index as u64 * self.element_size as u64
    }

    pub fn slot_for_offset(&self, byte_offset: u64) -> Option<usize> {
        if byte_offset < self.start_offset {
            return None;
        }
        let relative = byte_offset - self.start_offset;
        if relative % self.element_size as u64 != 0 {
            return None;
        }
        let idx = (relative / self.element_size as u64) as usize;
        if idx < self.bitmap.num_slots() as usize {
            Some(idx)
        } else {
            None
        }
    }

    pub fn mark_slot_allocated(&mut self, slot_index: usize) {
        self.bitmap.set(slot_index);
    }

    pub fn num_slots(&self) -> u32 {
        self.bitmap.num_slots()
    }

    pub fn contains_offset(&self, byte_offset: u64) -> bool {
        byte_offset >= self.start_offset && byte_offset < self.start_offset + self.slab_size
    }
}

pub(crate) struct SizeClassManager {
    map: HashMap<u32, Vec<u64>>,
}

impl SizeClassManager {
    pub fn new() -> Self {
        Self {
            map: HashMap::new(),
        }
    }

    pub fn add_slab(&mut self, element_size: u32, start_offset: u64) {
        self.map.entry(element_size).or_default().push(start_offset);
    }

    pub fn remove_slab(&mut self, element_size: u32, start_offset: u64) {
        if let Some(offsets) = self.map.get_mut(&element_size) {
            offsets.retain(|&o| o != start_offset);
            if offsets.is_empty() {
                self.map.remove(&element_size);
            }
        }
    }

    pub fn get_slabs(&self, element_size: u32) -> &[u64] {
        self.map
            .get(&element_size)
            .map(|v| v.as_slice())
            .unwrap_or(&[])
    }
}

#[cfg(kani)]
mod verification {
    use super::*;

    // Small slab: 4 slots of 4096 bytes each, based at a non-zero start offset so the
    // offset arithmetic (start + idx*element_size) is genuinely exercised.
    const START: u64 = 8192;
    const ELEM: u32 = 4096;
    const SLAB_SIZE: u64 = 4096 * 4; // => 4 slots
    const NSLOTS: usize = 4;

    // FR-011: a freshly created slab has every key slot set to FREE_KEY.
    #[kani::proof]
    #[kani::unwind(6)]
    fn verify_new_keys_all_free() {
        let s = Slab::new(START, SLAB_SIZE, ELEM);
        assert!(s.num_slots() as usize == NSLOTS);
        let idx: usize = kani::any();
        kani::assume(idx < NSLOTS);
        assert!(s.get_key(idx) == FREE_KEY);
    }

    // FR-005 / FR-012: slot_offset and slot_for_offset are exact inverses over the slot
    // domain, and slot_offset is sector/element-aligned relative to start. This is the
    // core of remove_extent(offset) locating the right slot.
    #[kani::proof]
    #[kani::unwind(6)]
    fn verify_slot_offset_roundtrip() {
        let s = Slab::new(START, SLAB_SIZE, ELEM);
        let idx: usize = kani::any();
        kani::assume(idx < NSLOTS);
        let off = s.slot_offset(idx);
        assert!(off == START + idx as u64 * ELEM as u64);
        assert!(s.contains_offset(off));
        assert!(s.slot_for_offset(off) == Some(idx));
    }

    // FR-005: alloc_slot yields an in-bounds slot whose reported offset matches
    // slot_offset, and the byte offset lies within the slab's disk range.
    #[kani::proof]
    #[kani::unwind(6)]
    fn verify_alloc_slot_offset_valid() {
        let mut s = Slab::new(START, SLAB_SIZE, ELEM);
        if let Some((idx, off)) = s.alloc_slot() {
            assert!(idx < NSLOTS);
            assert!(off == s.slot_offset(idx));
            assert!(off >= START && off < START + SLAB_SIZE);
        }
    }

    // FR-006 / FR-011: publishing a key then reading it back is consistent, and
    // free_slot resets the key to the FREE_KEY sentinel.
    #[kani::proof]
    #[kani::unwind(6)]
    fn verify_set_key_then_free() {
        let mut s = Slab::new(START, SLAB_SIZE, ELEM);
        let (idx, _off) = match s.alloc_slot() {
            Some(v) => v,
            None => return,
        };
        let key: u64 = kani::any();
        kani::assume(key != FREE_KEY); // FREE_KEY is the reserved sentinel, never stored as a live key
        s.set_key(idx, key);
        assert!(s.get_key(idx) == key);
        s.free_slot(idx);
        assert!(s.get_key(idx) == FREE_KEY);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn alloc_free_round_trip() {
        let mut slab = Slab::new(8192, 4096 * 4, 4096);
        assert_eq!(slab.num_slots(), 4);

        let (idx, offset) = slab.alloc_slot().unwrap();
        assert_eq!(idx, 0);
        assert_eq!(offset, 8192);

        slab.free_slot(idx);
        assert!(slab.is_empty());
    }

    #[test]
    fn keys_start_free() {
        let slab = Slab::new(0, 4096 * 4, 4096);
        for i in 0..4 {
            assert_eq!(slab.get_key(i), FREE_KEY);
        }
    }

    #[test]
    fn set_and_get_key() {
        let mut slab = Slab::new(0, 4096 * 2, 4096);
        let (idx, _) = slab.alloc_slot().unwrap();
        slab.set_key(idx, 42);
        assert_eq!(slab.get_key(idx), 42);
        slab.free_slot(idx);
        assert_eq!(slab.get_key(idx), FREE_KEY);
    }

    #[test]
    fn exhaust_all_slots() {
        let mut slab = Slab::new(0, 4096 * 2, 4096);
        assert_eq!(slab.num_slots(), 2);

        let (i0, _) = slab.alloc_slot().unwrap();
        let (i1, _) = slab.alloc_slot().unwrap();
        assert!(slab.alloc_slot().is_none());

        slab.free_slot(i0);
        slab.free_slot(i1);
        assert!(slab.is_empty());
    }

    #[test]
    fn rover_wraps() {
        let mut slab = Slab::new(0, 4096 * 3, 4096);
        let (i0, _) = slab.alloc_slot().unwrap();
        let (i1, _) = slab.alloc_slot().unwrap();
        slab.free_slot(i0);
        let (i2, _) = slab.alloc_slot().unwrap();
        assert_eq!(i2, 2);

        slab.free_slot(i1);
        let (i3, _) = slab.alloc_slot().unwrap();
        assert_eq!(i3, 0);
    }

    #[test]
    fn is_empty_after_full_free() {
        let mut slab = Slab::new(0, 4096 * 4, 4096);
        let slots: Vec<_> = (0..4).map(|_| slab.alloc_slot().unwrap().0).collect();
        assert!(!slab.is_empty());
        for s in slots {
            slab.free_slot(s);
        }
        assert!(slab.is_empty());
    }

    #[test]
    fn size_class_manager() {
        let mut scm = SizeClassManager::new();
        scm.add_slab(4096, 0);
        scm.add_slab(4096, 4096);
        scm.add_slab(8192, 8192);

        assert_eq!(scm.get_slabs(4096), &[0u64, 4096u64]);
        assert_eq!(scm.get_slabs(8192), &[8192u64]);
        assert_eq!(scm.get_slabs(16384), &[] as &[u64]);

        scm.remove_slab(4096, 0);
        assert_eq!(scm.get_slabs(4096), &[4096u64]);
    }
}
