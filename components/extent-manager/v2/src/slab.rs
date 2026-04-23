use std::collections::HashMap;

use crate::bitmap::AllocationBitmap;

pub(crate) struct Slab {
    pub start_offset: u64,
    pub slab_size: u64,
    pub element_size: u32,
    pub bitmap: AllocationBitmap,
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
    }

    pub fn is_empty(&self) -> bool {
        self.bitmap.is_all_free()
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
        byte_offset >= self.start_offset
            && byte_offset < self.start_offset + self.slab_size
    }
}

pub(crate) struct SizeClassManager {
    map: HashMap<u32, Vec<usize>>,
}

impl SizeClassManager {
    pub fn new() -> Self {
        Self {
            map: HashMap::new(),
        }
    }

    pub fn add_slab(&mut self, element_size: u32, slab_index: usize) {
        self.map.entry(element_size).or_default().push(slab_index);
    }

    pub fn remove_slab(&mut self, element_size: u32, slab_index: usize) {
        if let Some(indices) = self.map.get_mut(&element_size) {
            indices.retain(|&i| i != slab_index);
            if indices.is_empty() {
                self.map.remove(&element_size);
            }
        }
    }

    pub fn update_slab_index(&mut self, element_size: u32, old_index: usize, new_index: usize) {
        if let Some(indices) = self.map.get_mut(&element_size) {
            if let Some(pos) = indices.iter().position(|&i| i == old_index) {
                indices[pos] = new_index;
            }
        }
    }

    pub fn get_slabs(&self, element_size: u32) -> &[usize] {
        self.map
            .get(&element_size)
            .map(|v| v.as_slice())
            .unwrap_or(&[])
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
        scm.add_slab(4096, 1);
        scm.add_slab(8192, 2);

        assert_eq!(scm.get_slabs(4096), &[0, 1]);
        assert_eq!(scm.get_slabs(8192), &[2]);
        assert_eq!(scm.get_slabs(16384), &[] as &[usize]);

        scm.remove_slab(4096, 0);
        assert_eq!(scm.get_slabs(4096), &[1]);
    }
}
