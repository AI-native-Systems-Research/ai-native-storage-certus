//! Dispatch map entry types and location enum.

use std::sync::Arc;

use interfaces::DmaBuffer;

/// Represents where extent data currently resides.
#[derive(Debug)]
pub(crate) enum Location {
    /// Data is in an in-memory DMA staging buffer.
    Staging { buffer: Arc<DmaBuffer> },
    /// Data has been committed to a block device.
    BlockDevice { offset: u64 },
    /// Data is in the DRAM memory-tier pool.
    MemoryTier {
        pointer: *mut u8,
        size: u32,
        /// Set when write-through to SSD completes; enables eviction.
        ssd_offset: Option<u64>,
    },
}

// SAFETY: The pointer in MemoryTier refers to memory in the memory-tier pool,
// which is accessible from any thread. All access is serialized through the
// dispatch-map's Mutex.
unsafe impl Send for Location {}
unsafe impl Sync for Location {}

/// Read the CPU timestamp counter via RDTSC.
#[inline(always)]
pub(crate) fn rdtsc() -> u64 {
    #[cfg(target_arch = "x86_64")]
    unsafe {
        core::arch::x86_64::_rdtsc()
    }
    #[cfg(target_arch = "x86")]
    unsafe {
        core::arch::x86::_rdtsc()
    }
    #[cfg(not(any(target_arch = "x86_64", target_arch = "x86")))]
    {
        0
    }
}

/// Per-key metadata stored in the dispatch map.
#[derive(Debug)]
pub(crate) struct DispatchEntry {
    pub location: Location,
    #[allow(dead_code)]
    pub size_blocks: u32,
    pub read_ref: u32,
    pub write_ref: u32,
    /// Timestamp counter value — set on creation, updated on lookup.
    pub tsc: u64,
}
