use std::collections::HashMap;

use crate::bitmap::AllocationBitmap;
use crate::metadata::ExtentMetadata;
use crate::superblock::compute_slab_layout;

#[derive(Debug)]
pub(crate) struct SlabDescriptor {
    pub slab_index: usize,
    pub size_class: u32,
    pub start_lba: u64,
    pub bitmap_start_lba: u64,
    pub record_start_lba: u64,
    pub bitmap_blocks: u32,
    pub num_slots: u32,
    pub bitmap: AllocationBitmap,
}

impl SlabDescriptor {
    pub fn new(slab_index: usize, size_class: u32, start_lba: u64, slab_size_blocks: u32) -> Self {
        let (bitmap_blocks, num_slots) = compute_slab_layout(slab_size_blocks);
        let bitmap_start_lba = start_lba;
        let record_start_lba = start_lba + bitmap_blocks as u64;
        SlabDescriptor {
            slab_index,
            size_class,
            start_lba,
            bitmap_start_lba,
            record_start_lba,
            bitmap_blocks,
            num_slots,
            bitmap: AllocationBitmap::new(num_slots),
        }
    }
}

#[derive(Debug)]
pub(crate) struct ExtentManagerState {
    pub index: HashMap<u64, ExtentMetadata>,
    pub slabs: Vec<SlabDescriptor>,
    pub size_class_slabs: HashMap<u32, Vec<usize>>,
    pub total_blocks: u64,
    pub slab_size_blocks: u32,
    pub next_free_lba: u64,
    pub namespace_id: u32,
}

impl ExtentManagerState {
    pub fn new(total_blocks: u64, slab_size_blocks: u32, namespace_id: u32) -> Self {
        ExtentManagerState {
            index: HashMap::new(),
            slabs: Vec::new(),
            size_class_slabs: HashMap::new(),
            total_blocks,
            slab_size_blocks,
            next_free_lba: 1, // LBA 0 = superblock
            namespace_id,
        }
    }

    pub fn find_free_slot(&self, size_class: u32) -> Option<(usize, u32)> {
        let slab_indices = self.size_class_slabs.get(&size_class)?;
        for &idx in slab_indices {
            let slab = &self.slabs[idx];
            if let Some(slot) = slab.bitmap.find_free() {
                return Some((idx, slot));
            }
        }
        None
    }

    pub fn can_allocate_slab(&self) -> bool {
        self.slabs.len() < crate::superblock::MAX_SLABS
            && self.next_free_lba + self.slab_size_blocks as u64 <= self.total_blocks
    }

    pub fn add_slab_descriptor(&mut self, size_class: u32, start_lba: u64) -> usize {
        let idx = self.slabs.len();
        let slab = SlabDescriptor::new(idx, size_class, start_lba, self.slab_size_blocks);
        self.slabs.push(slab);
        self.size_class_slabs
            .entry(size_class)
            .or_default()
            .push(idx);
        self.next_free_lba = start_lba + self.slab_size_blocks as u64;
        idx
    }

    pub fn total_extents(&self) -> u64 {
        self.index.len() as u64
    }
}
