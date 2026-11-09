/// LBA generation strategies for the benchmark.
use rand::rngs::StdRng;
use rand::{Rng, SeedableRng};

/// Trait for generating logical block addresses.
pub(crate) trait LbaGenerator {
    /// Return the next LBA to use for an IO operation.
    ///
    /// `blocks_per_io` is the number of sectors this particular IO will touch,
    /// allowing mixed block-size workloads.
    fn next_lba(&mut self, blocks_per_io: u64) -> u64;
}

/// Uniform random LBA generator.
pub(crate) struct RandomLba {
    max_lba: u64,
    rng: StdRng,
}

impl RandomLba {
    /// Create a new random LBA generator.
    ///
    /// `total_sectors` is the namespace size in sectors.
    /// `max_blocks_per_io` is the largest number of sectors any single IO may
    /// touch (derived from the largest configured block size).
    pub(crate) fn new(total_sectors: u64, max_blocks_per_io: u64) -> Self {
        assert!(
            total_sectors > max_blocks_per_io,
            "namespace too small for block size"
        );
        Self {
            max_lba: total_sectors - max_blocks_per_io,
            rng: StdRng::from_entropy(),
        }
    }
}

impl LbaGenerator for RandomLba {
    fn next_lba(&mut self, _blocks_per_io: u64) -> u64 {
        self.rng.gen_range(0..self.max_lba)
    }
}

/// Sequential LBA generator with per-thread non-overlapping regions.
pub(crate) struct SequentialLba {
    current: u64,
    start: u64,
    end: u64,
}

impl SequentialLba {
    /// Create a new sequential LBA generator.
    ///
    /// Each thread gets a contiguous region of the namespace. The region
    /// is `total_sectors / num_threads` sectors, starting at
    /// `thread_index * region_size`.
    ///
    /// `max_blocks_per_io` is used for region boundary calculation (ensures
    /// the largest IO never overflows).
    pub(crate) fn new(
        thread_index: u32,
        num_threads: u32,
        total_sectors: u64,
        max_blocks_per_io: u64,
    ) -> Self {
        let region_size = total_sectors / num_threads as u64;
        let start = thread_index as u64 * region_size;
        let end = if thread_index == num_threads - 1 {
            total_sectors - max_blocks_per_io
        } else {
            start + region_size - max_blocks_per_io
        };
        Self {
            current: start,
            start,
            end,
        }
    }
}

impl LbaGenerator for SequentialLba {
    fn next_lba(&mut self, blocks_per_io: u64) -> u64 {
        let lba = self.current;
        self.current += blocks_per_io;
        if self.current > self.end {
            self.current = self.start;
        }
        lba
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn random_lba_in_range() {
        let mut gen = RandomLba::new(1_000_000, 8);
        for _ in 0..1000 {
            let lba = gen.next_lba(8);
            assert!(lba < 1_000_000 - 8);
        }
    }

    #[test]
    fn sequential_lba_contiguous() {
        let mut gen = SequentialLba::new(0, 1, 1000, 8);
        let first = gen.next_lba(8);
        let second = gen.next_lba(8);
        assert_eq!(first, 0);
        assert_eq!(second, 8);
    }

    #[test]
    fn sequential_lba_wraps() {
        let mut gen = SequentialLba::new(0, 1, 100, 8);
        let mut last = 0;
        for _ in 0..20 {
            let lba = gen.next_lba(8);
            if lba < last {
                assert_eq!(lba, 0);
                return;
            }
            last = lba;
        }
        panic!("sequential LBA did not wrap within expected iterations");
    }

    #[test]
    fn sequential_lba_non_overlapping_threads() {
        let total_sectors = 10000;
        let blocks_per_io = 8;
        let num_threads = 4;

        let mut generators: Vec<SequentialLba> = (0..num_threads)
            .map(|i| SequentialLba::new(i, num_threads, total_sectors, blocks_per_io))
            .collect();

        let lbas: Vec<u64> = generators
            .iter_mut()
            .map(|g| g.next_lba(blocks_per_io))
            .collect();

        let region_size = total_sectors / num_threads as u64;
        for (i, &lba) in lbas.iter().enumerate() {
            assert_eq!(lba, i as u64 * region_size);
        }

        let lbas2: Vec<u64> = generators
            .iter_mut()
            .map(|g| g.next_lba(blocks_per_io))
            .collect();
        for (i, &lba) in lbas2.iter().enumerate() {
            let region_start = i as u64 * region_size;
            let region_end = if i as u32 == num_threads - 1 {
                total_sectors - blocks_per_io
            } else {
                region_start + region_size - blocks_per_io
            };
            assert!(
                lba >= region_start && lba <= region_end,
                "thread {} lba {} outside region {}..{}",
                i,
                lba,
                region_start,
                region_end
            );
        }
    }
}
