use std::collections::BTreeMap;

use interfaces::{ExtentKey, ExtentManagerError, FormatParams};

use crate::buddy::BuddyAllocator;
use crate::error;
use crate::slab::{SizeClassManager, Slab, FREE_KEY};
use crate::superblock::Superblock;

pub(crate) struct RegionState {
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
    pub superblock: Superblock,
}

impl RegionState {
    pub fn new(buddy: BuddyAllocator, format_params: FormatParams) -> Self {
        Self {
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

    pub fn alloc_extent(&mut self, size: u32) -> Result<(u64, usize, u64), ExtentManagerError> {
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

        if element_size as u64 > slab_size {
            return Err(error::out_of_space());
        }

        let disk_offset = self
            .buddy
            .alloc(slab_size)
            .ok_or_else(error::out_of_space)?;

        let mut slab = Slab::new(disk_offset, slab_size, element_size);
        let (slot_idx, offset) = match slab.alloc_slot() {
            Some(result) => result,
            None => {
                self.buddy.free(disk_offset, slab_size);
                return Err(error::out_of_space());
            }
        };
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

    pub fn remove_extent_by_offset(&mut self, offset: u64) -> Result<(), ExtentManagerError> {
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

        self.slabs
            .get_mut(&slab_start)
            .unwrap()
            .set_key(slot_idx, FREE_KEY);
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

// ---------------------------------------------------------------------------
// Kani harnesses (re-authored from the property inventory, clean-slate re-run).
//
// RegionState is the composition layer: BTreeMap<u64, Slab> + a HashMap-backed
// SizeClassManager + the BuddyAllocator. Two very different tractability regimes:
//   * `align_to_sector_size` is PURE ARITHMETIC — the EM-SECTOR-ALIGN global proved
//     here over a symbolic size and sector size (tractable, green).
//   * `alloc_extent` / `remove_extent_by_offset` / `flush_pending_frees` walk the
//     std BTreeMap and HashMap — the same container-modeling SAT wall memory-tier hit.
//     One wall probe below is run once to capture the reproducible timeout signature
//     (EM-DEFERRED-FREE / region-level EM-KEYVEC-MEMBERSHIP) and then moved on.
// ---------------------------------------------------------------------------
#[cfg(kani)]
mod verification {
    use super::*;

    // Build a RegionState with a trivial buddy so we can call the real private
    // `align_to_sector_size`. The buddy geometry is irrelevant to the alignment math.
    fn tiny_region() -> RegionState {
        let buddy = BuddyAllocator::new(0, 8, 1);
        RegionState::new(buddy, FormatParams::default())
    }

    // [EM-SECTOR-ALIGN, global rank 3] every reserved extent's element_size is rounded
    // UP to a whole number of sectors: result is sector-aligned, >= size, within one
    // sector of size, and a no-op on an already-aligned size. Proved over a SYMBOLIC
    // size and sector_size (bounded so the u32 rounding `size + ss - 1` cannot
    // spuriously overflow — see the report's note on the unguarded production add).
    #[kani::proof]
    #[kani::unwind(3)]
    fn verify_align_to_sector_size() {
        let r = tiny_region();
        let size: u32 = kani::any();
        let ss: u32 = kani::any();
        // Mirror the real domain: sector_size > 0 (format rejects 0), and keep the
        // rounding sum within u32 so we test the ALIGNMENT property, not overflow.
        kani::assume(ss >= 1 && ss <= 4096);
        kani::assume(size <= 1u32 << 20);
        let aligned = r.align_to_sector_size(size, ss);
        assert!(aligned % ss == 0); // result is sector-aligned
        assert!(aligned >= size); // never shrinks below the request
        assert!(aligned - size < ss); // rounds up by strictly less than one sector
        if size % ss == 0 {
            assert!(aligned == size); // already-aligned => identity
        }
    }

    // [EM-DEFERRED-FREE / region-level EM-KEYVEC-MEMBERSHIP] WALL PROBE — run once.
    // alloc_extent composes BTreeMap<u64,Slab>::insert, HashMap entry/get, and
    // buddy.alloc. Expected: CBMC stalls unwinding std BTreeMap/HashMap navigation
    // (find_key_index / node links) or hits the getrandom/SipHash RandomState seed of
    // the HashMap — captured as rc=124 at the per-harness timeout. Documented ⊘.
    #[kani::proof]
    #[kani::unwind(4)]
    fn wall_region_alloc_extent() {
        let buddy = BuddyAllocator::new(0, 4096 * 4, 4096);
        let mut params = FormatParams::default();
        params.sector_size = 4096;
        params.slab_size = 4096 * 4;
        let mut r = RegionState::new(buddy, params);
        let size: u32 = kani::any();
        kani::assume(size >= 1 && size <= 4096);
        if let Ok((slab_start, slot_idx, offset)) = r.alloc_extent(size) {
            // If the composition IS tractable, this is the deferred-free core:
            // a freed-then-flushed slot returns the extent to reusable state.
            r.publish_slot(slab_start, slot_idx, 42);
            assert!(r.remove_extent_by_offset(offset).is_ok());
            r.flush_pending_frees();
        }
    }
}
