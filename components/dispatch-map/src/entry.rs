//! Dispatch map entry types and location enum.

use std::sync::atomic::AtomicU32;

use interfaces::EvictionHandle;

/// Represents where extent data currently resides.
#[derive(Debug)]
pub(crate) enum Location {
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

/// Per-key metadata stored in the dispatch map.
pub(crate) struct DispatchEntry {
    pub location: Location,
    #[allow(dead_code)]
    pub size_blocks: u32,
    pub read_ref: u32,
    pub write_ref: u64,
    /// Handle into the eviction policy's ordering.
    pub eviction_handle: EvictionHandle,
    /// Number of times this entry has been reused (read hits).
    pub reuse_count: AtomicU32,
    /// CRC-32 of the stored block data, set on the store path and verified on
    /// load. `0` means "not yet recorded". Only compiled under the
    /// `integrity-check` feature — when off, `DispatchEntry` stays 56 bytes.
    #[cfg(feature = "integrity-check")]
    pub checksum: u32,
}

impl std::fmt::Debug for DispatchEntry {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let mut dbg = f.debug_struct("DispatchEntry");
        dbg.field("location", &self.location)
            .field("size_blocks", &self.size_blocks)
            .field("read_ref", &self.read_ref)
            .field("write_ref", &self.write_ref)
            .field("eviction_handle", &self.eviction_handle)
            .field(
                "reuse_count",
                &self.reuse_count.load(std::sync::atomic::Ordering::Relaxed),
            );
        #[cfg(feature = "integrity-check")]
        dbg.field("checksum", &self.checksum);
        dbg.finish()
    }
}
