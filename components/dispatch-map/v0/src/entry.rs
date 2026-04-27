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
}

/// Per-key metadata stored in the dispatch map.
#[derive(Debug)]
#[allow(dead_code)]
pub(crate) struct DispatchEntry {
    pub location: Location,
    pub size_blocks: u32,
    pub read_ref: u32,
    pub write_ref: u32,
}
