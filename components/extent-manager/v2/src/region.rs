use std::collections::BTreeMap;

use interfaces::{ExtentKey, ExtentManagerError, FormatParams};

use crate::buddy::BuddyAllocator;
use crate::error;
use crate::slab::{FREE_KEY, SizeClassManager, Slab};
use crate::superblock::Superblock;

pub(crate) struct RegionState {
    pub region_index: usize,
    pub slabs: BTreeMap<u64, Slab>,
    pub size_classes: SizeClassManager,
    pub buddy: BuddyAllocator,
    pub dirty: bool,
    pub format_params: FormatParams,
    pending_frees: Vec<(u64, usize)>, // (slab_start, slot_idx)
}

pub(crate) struct SharedState {
    pub format_params: FormatParams,
    pub checkpoint_seq: u64,
    pub disk_size: u64,
    pub superblock: Superblock,
}

impl RegionState {
    pub fn new(region_index: usize, buddy: BuddyAllocator, format_params: FormatParams) -> Self {
        Self {
            region_index,
            slabs: BTreeMap::new(),
            size_classes: SizeClassManager::new(),
            buddy,
            dirty: false,
            format_params,
            pending_frees: Vec::new(),
        }
    }

    fn align_to_sector_size(&self, size: u32, sector_size: u32) -> u32 {
        (size + sector_size - 1) / sector_size * sector_size
    }

    pub fn alloc_extent(
        &mut self,
        size: u32,
    ) -> Result<(u64, usize, u64), ExtentManagerError> {
        let element_size = self.align_to_sector_size(size, self.format_params.sector_size);

        // The SizeClassManager invariant: only non-full slabs appear in the list.
        // Iterate, removing any stale full entries we encounter (shouldn't happen
        // in steady state, but guards against any inconsistency).
        loop {
            let slab_start = match self.size_classes.get_slabs(element_size).first() {
                Some(&s) => s,
                None => break,
            };
            if let Some(slab) = self.slabs.get_mut(&slab_start) {
                if let Some((slot_idx, offset)) = slab.alloc_slot() {
                    // Remove from the non-full list if the slab just became full.
                    let now_full = slab.is_full();
                    if now_full {
                        self.size_classes.remove_slab(element_size, slab_start);
                    }
                    return Ok((slab_start, slot_idx, offset));
                }
            }
            // Stale entry: slab was full or missing — remove it and try the next.
            self.size_classes.remove_slab(element_size, slab_start);
        }

        let slab_size = self.format_params.slab_size;
        let disk_offset = self
            .buddy
            .alloc(slab_size)
            .ok_or_else(error::out_of_space)?;

        let mut slab = Slab::new(disk_offset, slab_size, element_size);
        let (slot_idx, offset) = slab
            .alloc_slot()
            .expect("freshly created slab must have free slot");
        // Only add to the non-full list if the slab still has capacity after this first alloc.
        if !slab.is_full() {
            self.size_classes.add_slab(element_size, disk_offset);
        }
        self.slabs.insert(disk_offset, slab);

        Ok((disk_offset, slot_idx, offset))
    }

    pub fn free_slot(&mut self, slab_start: u64, slot_idx: usize) {
        let (was_full, now_empty, element_size, slab_size) =
            if let Some(slab) = self.slabs.get_mut(&slab_start) {
                let was_full = slab.is_full();
                slab.free_slot(slot_idx);
                (was_full, slab.is_empty(), slab.element_size, slab.slab_size)
            } else {
                return;
            };

        if now_empty {
            self.buddy.free(slab_start, slab_size);
            self.size_classes.remove_slab(element_size, slab_start);
            self.slabs.remove(&slab_start);
        } else if was_full {
            // Slab went from full → partial: re-add to the non-full list.
            self.size_classes.add_slab(element_size, slab_start);
        }
    }

    pub fn publish_slot(&mut self, slab_start: u64, slot_idx: usize, key: ExtentKey) {
        if let Some(slab) = self.slabs.get_mut(&slab_start) {
            slab.set_key(slot_idx, key);
        }
        self.dirty = true;
    }

    pub fn remove_extent_by_offset(
        &mut self,
        offset: u64,
    ) -> Result<(), ExtentManagerError> {
        let slab_start = self
            .slabs
            .range(..=offset)
            .next_back()
            .map(|(&k, _)| k)
            .ok_or_else(|| error::offset_not_found(offset))?;

        let slot_idx = {
            let slab = self.slabs.get(&slab_start).unwrap();
            if !slab.contains_offset(offset) {
                return Err(error::offset_not_found(offset));
            }
            let slot = slab
                .slot_for_offset(offset)
                .ok_or_else(|| error::offset_not_found(offset))?;
            if !slab.bitmap.is_set(slot) || slab.get_key(slot) == FREE_KEY {
                return Err(error::offset_not_found(offset));
            }
            slot
        };

        self.slabs.get_mut(&slab_start).unwrap().set_key(slot_idx, FREE_KEY);
        self.pending_frees.push((slab_start, slot_idx));
        self.dirty = true;
        Ok(())
    }

    pub fn flush_pending_frees(&mut self) {
        if self.pending_frees.is_empty() {
            return;
        }
        let frees = std::mem::take(&mut self.pending_frees);
        for (slab_start, slot_idx) in frees {
            self.free_slot(slab_start, slot_idx);
        }
    }
}
