//! GPU BAR1 P2P staging ring for NVMe-to-GPU direct DMA.
//!
//! Allocates a fixed ring of 64 GPU-resident staging buffers that serve as
//! DMA targets for NVMe reads. Each slot is allocated via `cudaMalloc`,
//! mapped to BAR1 via GDRCopy, and registered with SPDK for direct NVMe DMA.
//!
//! The ring is partitioned across threads for lock-free concurrent access.
//!
//! With 4 drives and MAX_QUEUES_PER_DRIVE=1, each drive gets 16 slots and
//! 16 queue depth — sufficient to saturate PCIe bandwidth per drive.

use std::sync::{Arc, Mutex};

use gpu_services::cuda_ffi;
use gpu_services::dma::create_spdk_dma_buffer_from_gpu_bar;
use interfaces::{DmaBuffer, GpuStream, IGpuServices};

/// Total number of staging slots in the P2P ring.
pub const P2P_RING_SLOTS: usize = 64;

/// Maximum effective queue depth per thread.
/// With 4 drives × 1 thread each = 4 threads, each gets 16 slots.
/// 16 in-flight NVMe reads per drive is sufficient to hide NVMe latency
/// and saturate BAR1 bandwidth at full PCIe line rate.
const MAX_QD_PER_THREAD: usize = 16;

/// Number of CUDA streams per P2P ring.
/// More streams allow more D2D copies to be in-flight simultaneously,
/// hiding PCIe latency for D2D transfers. 4 streams matches 4-drive topology.
const NUM_STREAMS: usize = 4;

/// Pre-allocated ring of GPU-resident staging buffers for P2P NVMe reads.
///
/// Each slot is a `cudaMalloc`'d buffer with a GDRCopy BAR1 mapping and
/// SPDK DMA registration, enabling NVMe controllers to write directly
/// into GPU memory without host DRAM bounce.
pub struct P2pRing {
    pub ring_bufs: Vec<Arc<Mutex<DmaBuffer>>>,
    pub dev_ptrs: Vec<*mut std::ffi::c_void>,
    pub streams: Vec<GpuStream>,
    pub slot_size: usize,
}

unsafe impl Send for P2pRing {}
unsafe impl Sync for P2pRing {}

impl P2pRing {
    /// Allocate a P2P ring with GDRCopy BAR1-mapped GPU staging buffers.
    ///
    /// Returns `None` if GDRCopy/BAR1 is unavailable or GPU memory is
    /// insufficient. Cleans up any partial allocations on failure.
    pub fn new(gpu: &dyn IGpuServices, slot_size: usize) -> Option<Self> {
        let mut dev_ptrs: Vec<*mut std::ffi::c_void> = Vec::with_capacity(P2P_RING_SLOTS);
        let mut ring_bufs: Vec<Arc<Mutex<DmaBuffer>>> = Vec::with_capacity(P2P_RING_SLOTS);

        for _i in 0..P2P_RING_SLOTS {
            let mut dev_ptr: *mut std::ffi::c_void = std::ptr::null_mut();
            let err = unsafe { cuda_ffi::cudaMalloc(&mut dev_ptr, slot_size) };
            if err != cuda_ffi::CUDA_SUCCESS {
                drop(ring_bufs);
                for p in &dev_ptrs {
                    unsafe { cuda_ffi::cudaFree(*p) };
                }
                return None;
            }

            match create_spdk_dma_buffer_from_gpu_bar(dev_ptr, slot_size) {
                Ok(buf) => {
                    dev_ptrs.push(dev_ptr);
                    ring_bufs.push(Arc::new(Mutex::new(buf)));
                }
                Err(_) => {
                    unsafe { cuda_ffi::cudaFree(dev_ptr) };
                    drop(ring_bufs);
                    for p in &dev_ptrs {
                        unsafe { cuda_ffi::cudaFree(*p) };
                    }
                    return None;
                }
            }
        }

        // Allocate NUM_STREAMS CUDA streams for maximum D2D parallelism.
        let mut streams: Vec<GpuStream> = Vec::with_capacity(NUM_STREAMS);
        for i in 0..NUM_STREAMS {
            match gpu.create_stream() {
                Ok(s) => streams.push(s),
                Err(_) => {
                    // Clean up already-created streams.
                    for s in &streams {
                        let _ = gpu.destroy_stream(*s);
                    }
                    // Fall back to 2 streams if we can't get NUM_STREAMS.
                    // This is non-fatal but reduces D2D parallelism.
                    if i >= 2 {
                        // Keep what we have if we at least have 2.
                        break;
                    }
                    drop(ring_bufs);
                    for p in &dev_ptrs {
                        unsafe { cuda_ffi::cudaFree(*p) };
                    }
                    return None;
                }
            }
        }

        // Ensure we have at least 2 streams.
        if streams.len() < 2 {
            for s in &streams {
                let _ = gpu.destroy_stream(*s);
            }
            drop(ring_bufs);
            for p in &dev_ptrs {
                unsafe { cuda_ffi::cudaFree(*p) };
            }
            return None;
        }

        Some(Self {
            ring_bufs,
            dev_ptrs,
            streams,
            slot_size,
        })
    }

    /// Destroy CUDA streams and free GPU allocations.
    pub fn destroy(self, gpu: &dyn IGpuServices) {
        for s in &self.streams {
            let _ = gpu.destroy_stream(*s);
        }
        drop(self.ring_bufs);
        for p in &self.dev_ptrs {
            unsafe { cuda_ffi::cudaFree(*p) };
        }
    }

    /// Get a reference to the slot DmaBuffer at `index`.
    pub fn slot(&self, index: usize) -> &Arc<Mutex<DmaBuffer>> {
        &self.ring_bufs[index]
    }

    /// Get the GPU device pointer for a slot (for D2D copy source).
    pub fn slot_ptr(&self, index: usize) -> *const std::ffi::c_void {
        self.dev_ptrs[index]
    }

    /// Get all CUDA streams.
    pub fn streams(&self) -> &[GpuStream] {
        &self.streams
    }

    /// Total number of slots.
    pub fn total_slots(&self) -> usize {
        self.ring_bufs.len()
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
