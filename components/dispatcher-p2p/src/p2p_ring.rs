//! GPU BAR1 P2P staging ring for NVMe-to-GPU direct DMA.
//!
//! Allocates a fixed ring of 64 GPU-resident staging buffers that serve as
//! DMA targets for NVMe reads. Each slot is allocated via `cudaMalloc`,
//! mapped to BAR1 via GDRCopy, and registered with SPDK for direct NVMe DMA.
//!
//! The ring is partitioned across threads for lock-free concurrent access.

use interfaces::{DmaBuffer, GpuStream, IGpuServices};

/// Total number of staging slots in the P2P ring.
pub const P2P_RING_SLOTS: usize = 64;

/// Maximum effective queue depth per thread (prevents NVMe qpair saturation).
const MAX_QD_PER_THREAD: usize = 16;

/// Pre-allocated ring of GPU-resident staging buffers for P2P NVMe reads.
///
/// Each slot is a `cudaMalloc`'d buffer with a GDRCopy BAR1 mapping and
/// SPDK DMA registration, enabling NVMe controllers to write directly
/// into GPU memory without host DRAM bounce.
pub struct P2pRing {
    slots: Vec<DmaBuffer>,
    streams: [GpuStream; 2],
    pub slot_size: usize,
}

impl P2pRing {
    /// Attempt to allocate the P2P staging ring.
    ///
    /// Returns `None` if GDRCopy/BAR1 is unavailable or GPU memory is
    /// insufficient. Cleans up any partial allocations on failure.
    pub fn new(gpu: &dyn IGpuServices, slot_size: usize) -> Option<Self> {
        let mut slots: Vec<DmaBuffer> = Vec::with_capacity(P2P_RING_SLOTS);

        for _i in 0..P2P_RING_SLOTS {
            match gpu.allocate_pinned_dma_buffer(slot_size) {
                Ok(buf) => slots.push(buf),
                Err(_) => {
                    // Partial allocation — clean up already-allocated slots.
                    // DmaBuffer Drop handles deallocation.
                    drop(slots);
                    return None;
                }
            }
        }

        let stream_a = match gpu.create_stream() {
            Ok(s) => s,
            Err(_) => {
                drop(slots);
                return None;
            }
        };

        let stream_b = match gpu.create_stream() {
            Ok(s) => s,
            Err(_) => {
                let _ = gpu.destroy_stream(stream_a);
                drop(slots);
                return None;
            }
        };

        Some(Self {
            slots,
            streams: [stream_a, stream_b],
            slot_size,
        })
    }

    /// Destroy CUDA streams. Slot buffers are freed via DmaBuffer::drop.
    pub fn destroy(self, gpu: &dyn IGpuServices) {
        let _ = gpu.destroy_stream(self.streams[0]);
        let _ = gpu.destroy_stream(self.streams[1]);
        // slots are freed on drop
    }

    /// Get a reference to the slot at `index`.
    pub fn slot(&self, index: usize) -> &DmaBuffer {
        &self.slots[index]
    }

    /// Get the device pointer for a slot (for D2D copy source).
    pub fn slot_ptr(&self, index: usize) -> *const std::ffi::c_void {
        self.slots[index].as_ptr()
    }

    /// Get the alternating CUDA streams.
    pub fn streams(&self) -> &[GpuStream; 2] {
        &self.streams
    }

    /// Total number of slots.
    pub fn total_slots(&self) -> usize {
        self.slots.len()
    }
}

/// Per-thread partition of the P2P ring for lock-free concurrent access.
pub struct ThreadPartition {
    /// Starting slot index for this thread's partition.
    pub ring_offset: usize,
    /// Number of slots available to this thread.
    pub effective_qd: usize,
}

impl ThreadPartition {
    /// Compute a non-overlapping partition for the given thread.
    ///
    /// Divides the ring evenly across `num_threads`, capping each partition
    /// at `MAX_QD_PER_THREAD` to prevent NVMe qpair saturation.
    pub fn new(thread_index: usize, num_threads: usize) -> Self {
        let slots_per_thread = (P2P_RING_SLOTS / num_threads.max(1)).min(MAX_QD_PER_THREAD);
        let ring_offset = thread_index * slots_per_thread;
        Self {
            ring_offset,
            effective_qd: slots_per_thread,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn partition_single_thread_gets_max_qd() {
        let p = ThreadPartition::new(0, 1);
        assert_eq!(p.ring_offset, 0);
        assert_eq!(p.effective_qd, MAX_QD_PER_THREAD);
    }

    #[test]
    fn partition_four_threads_non_overlapping() {
        let partitions: Vec<_> = (0..4).map(|i| ThreadPartition::new(i, 4)).collect();
        for i in 0..4 {
            assert_eq!(partitions[i].ring_offset, i * 16);
            assert_eq!(partitions[i].effective_qd, 16);
        }
        // Verify non-overlapping
        for i in 0..4 {
            for j in (i + 1)..4 {
                let a_end = partitions[i].ring_offset + partitions[i].effective_qd;
                assert!(a_end <= partitions[j].ring_offset);
            }
        }
    }

    #[test]
    fn partition_eight_threads_capped() {
        let partitions: Vec<_> = (0..8).map(|i| ThreadPartition::new(i, 8)).collect();
        for p in &partitions {
            assert_eq!(p.effective_qd, 8);
        }
        assert_eq!(partitions[7].ring_offset, 56);
    }

    #[test]
    fn partition_bounds_within_ring() {
        for num_threads in 1..=8 {
            for t in 0..num_threads {
                let p = ThreadPartition::new(t, num_threads);
                assert!(
                    p.ring_offset + p.effective_qd <= P2P_RING_SLOTS,
                    "thread {t}/{num_threads} exceeds ring: offset={} qd={}",
                    p.ring_offset,
                    p.effective_qd,
                );
            }
        }
    }
}
