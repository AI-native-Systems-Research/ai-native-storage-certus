pub(crate) struct BuddyAllocator {
    base_offset: u64,
    total_usable_size: u64,
    sector_size: u32,
    max_order: usize,
    free_lists: Vec<Vec<u64>>,
}

impl BuddyAllocator {
    pub fn new(base_offset: u64, total_usable_size: u64, sector_size: u32) -> Self {
        let usable_blocks = total_usable_size / sector_size as u64;
        let max_order = if usable_blocks > 1 {
            63 - usable_blocks.leading_zeros() as usize
        } else if usable_blocks == 1 {
            0
        } else {
            return Self {
                base_offset,
                total_usable_size,
                sector_size,
                max_order: 0,
                free_lists: vec![Vec::new()],
            };
        };

        let mut free_lists = vec![Vec::new(); max_order + 1];

        let mut offset: u64 = 0;
        let mut remaining = usable_blocks;
        while remaining > 0 {
            let order = 63 - remaining.leading_zeros() as usize;
            let blocks = 1u64 << order;
            let byte_offset = offset * sector_size as u64;
            free_lists[order].push(byte_offset);
            offset += blocks;
            remaining -= blocks;
        }

        Self {
            base_offset,
            total_usable_size,
            sector_size,
            max_order,
            free_lists,
        }
    }

    pub fn base_offset(&self) -> u64 {
        self.base_offset
    }

    pub fn alloc(&mut self, size: u64) -> Option<u64> {
        let blocks_needed =
            (size + self.sector_size as u64 - 1) / self.sector_size as u64;
        let order_needed = if blocks_needed <= 1 {
            0
        } else {
            64 - (blocks_needed - 1).leading_zeros() as usize
        };

        let mut found_order = None;
        for order in order_needed..=self.max_order {
            if !self.free_lists[order].is_empty() {
                found_order = Some(order);
                break;
            }
        }

        let found_order = found_order?;
        let local_offset = self.free_lists[found_order].pop().unwrap();

        for split_order in (order_needed..found_order).rev() {
            let buddy_offset =
                local_offset + ((1u64 << split_order) * self.sector_size as u64);
            self.free_lists[split_order].push(buddy_offset);
        }

        Some(self.base_offset + local_offset)
    }

    pub fn free(&mut self, abs_offset: u64, size: u64) {
        let offset = abs_offset - self.base_offset;
        let blocks = size / self.sector_size as u64;
        let mut order = if blocks <= 1 {
            0
        } else {
            64 - (blocks - 1).leading_zeros() as usize
        };
        let mut current_offset = offset;

        while order < self.max_order {
            let block_span = (1u64 << order) * self.sector_size as u64;
            let buddy_offset = current_offset ^ block_span;

            if buddy_offset + block_span > self.total_usable_size {
                break;
            }

            if let Some(pos) = self.free_lists[order]
                .iter()
                .position(|&o| o == buddy_offset)
            {
                self.free_lists[order].swap_remove(pos);
                current_offset = current_offset.min(buddy_offset);
                order += 1;
            } else {
                break;
            }
        }

        self.free_lists[order].push(current_offset);
    }

    pub fn mark_allocated(&mut self, abs_offset: u64, size: u64) {
        let offset = abs_offset - self.base_offset;
        let blocks = size / self.sector_size as u64;
        let target_order = if blocks <= 1 {
            0
        } else {
            64 - (blocks - 1).leading_zeros() as usize
        };

        if let Some(pos) = self.free_lists[target_order]
            .iter()
            .position(|&o| o == offset)
        {
            self.free_lists[target_order].swap_remove(pos);
            return;
        }

        for search_order in (target_order + 1)..=self.max_order {
            let block_span =
                (1u64 << search_order) * self.sector_size as u64;
            let aligned_start = offset & !(block_span - 1);

            if let Some(pos) = self.free_lists[search_order]
                .iter()
                .position(|&o| o == aligned_start)
            {
                self.free_lists[search_order].swap_remove(pos);

                let mut split_offset = aligned_start;
                for so in (target_order..search_order).rev() {
                    let half_span =
                        (1u64 << so) * self.sector_size as u64;
                    if offset >= split_offset + half_span {
                        self.free_lists[so].push(split_offset);
                        split_offset += half_span;
                    } else {
                        self.free_lists[so]
                            .push(split_offset + half_span);
                    }
                }
                return;
            }
        }
    }

    pub fn total_free(&self) -> u64 {
        let mut total = 0u64;
        for (order, list) in self.free_lists.iter().enumerate() {
            total +=
                list.len() as u64 * (1u64 << order) * self.sector_size as u64;
        }
        total
    }

    pub fn total_usable_size(&self) -> u64 {
        self.total_usable_size
    }

    pub fn sector_size(&self) -> u32 {
        self.sector_size
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn power_of_two_size() {
        let buddy = BuddyAllocator::new(0,1024 * 1024, 4096);
        assert_eq!(buddy.total_free(), 1024 * 1024);
    }

    #[test]
    fn non_power_of_two_size() {
        let total = 5 * 4096u64;
        let buddy = BuddyAllocator::new(0,total, 4096);
        assert_eq!(buddy.total_free(), total);
    }

    #[test]
    fn alloc_and_free() {
        let mut buddy = BuddyAllocator::new(0,16 * 4096, 4096);
        let total = buddy.total_free();

        let a = buddy.alloc(4096).expect("alloc 1 block");
        assert_eq!(buddy.total_free(), total - 4096);

        buddy.free(a, 4096);
        assert_eq!(buddy.total_free(), total);
    }

    #[test]
    fn alloc_merges_on_free() {
        let mut buddy = BuddyAllocator::new(0,4 * 4096, 4096);
        let a = buddy.alloc(4096).unwrap();
        let b = buddy.alloc(4096).unwrap();
        buddy.free(a, 4096);
        buddy.free(b, 4096);
        assert_eq!(buddy.total_free(), 4 * 4096);
        let big = buddy.alloc(4 * 4096);
        assert!(big.is_some());
    }

    #[test]
    fn tail_block_no_merge() {
        let total = 3u64 * 4096;
        let mut buddy = BuddyAllocator::new(0,total, 4096);
        let a = buddy.alloc(4096).unwrap();
        let b = buddy.alloc(4096).unwrap();
        let c = buddy.alloc(4096).unwrap();
        assert_eq!(buddy.total_free(), 0);

        buddy.free(c, 4096);
        buddy.free(b, 4096);
        buddy.free(a, 4096);
        assert_eq!(buddy.total_free(), total);
    }

    #[test]
    fn exhaust_and_reclaim() {
        let mut buddy = BuddyAllocator::new(0,2 * 4096, 4096);
        let a = buddy.alloc(4096).unwrap();
        let b = buddy.alloc(4096).unwrap();
        assert!(buddy.alloc(4096).is_none());
        buddy.free(a, 4096);
        assert!(buddy.alloc(4096).is_some());
    }

    #[test]
    fn large_non_power_of_two() {
        let total = 100 * 4096u64;
        let buddy = BuddyAllocator::new(0,total, 4096);
        assert_eq!(buddy.total_free(), total);
    }

    #[test]
    fn mark_allocated_exact() {
        let mut buddy = BuddyAllocator::new(0,4 * 4096, 4096);
        let a = buddy.alloc(4096).unwrap();
        buddy.free(a, 4096);
        buddy.mark_allocated(a, 4096);
        assert_eq!(buddy.total_free(), 3 * 4096);
    }

    #[test]
    fn mark_allocated_split() {
        let mut buddy = BuddyAllocator::new(0,4 * 4096, 4096);
        buddy.mark_allocated(0, 4096);
        assert_eq!(buddy.total_free(), 3 * 4096);
        let a = buddy.alloc(4096).unwrap();
        assert_ne!(a, 0);
    }

    #[test]
    fn alloc_larger_block() {
        let mut buddy = BuddyAllocator::new(0, 8 * 4096, 4096);
        let a = buddy.alloc(2 * 4096).unwrap();
        assert_eq!(buddy.total_free(), 6 * 4096);
        buddy.free(a, 2 * 4096);
        assert_eq!(buddy.total_free(), 8 * 4096);
    }

    #[test]
    fn base_offset_applied() {
        let base = 1024 * 1024;
        let mut buddy = BuddyAllocator::new(base, 4 * 4096, 4096);
        assert_eq!(buddy.base_offset(), base);

        let a = buddy.alloc(4096).unwrap();
        assert!(a >= base);
        assert_eq!(buddy.total_free(), 3 * 4096);

        buddy.free(a, 4096);
        assert_eq!(buddy.total_free(), 4 * 4096);
    }

    #[test]
    fn base_offset_mark_allocated() {
        let base = 8 * 4096;
        let mut buddy = BuddyAllocator::new(base, 4 * 4096, 4096);
        buddy.mark_allocated(base, 4096);
        assert_eq!(buddy.total_free(), 3 * 4096);
        let a = buddy.alloc(4096).unwrap();
        assert_ne!(a, base);
        assert!(a >= base);
    }
}
