//! Interface for the extent-manager component and shared types.
#[cfg(feature = "spdk")]
use component_macros::define_interface;
use std::fmt;

/// Opaque key identifying an extent.
pub type ExtentKey = u64;

/// A storage extent returned by the extent manager.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Extent {
    pub key: ExtentKey,
    pub size: u32, // size in blocks
    pub offset: u64,
}

/// Errors returned by `IExtentManager` operations.
#[derive(Debug, Clone)]
pub enum ExtentManagerError {
    CorruptMetadata(String),
    IoError(String),
    NotInitialized(String),
    OffsetNotFound(u64),
    OutOfSpace,
}

impl fmt::Display for ExtentManagerError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::CorruptMetadata(msg) => write!(f, "corrupt metadata: {msg}"),
            Self::IoError(msg) => write!(f, "I/O error: {msg}"),
            Self::NotInitialized(msg) => write!(f, "not initialized: {msg}"),
            Self::OffsetNotFound(off) => write!(f, "no extent at offset: {off}"),
            Self::OutOfSpace => write!(f, "out of space"),
        }
    }
}

impl std::error::Error for ExtentManagerError {}

#[derive(Debug, Clone)]
pub struct FormatParams {
    /// Total size of the data disk in bytes.
    pub data_disk_size: u64,
    /// Size of each slab in bytes. Must be a multiple of `sector_size`.
    pub slab_size: u64,
    /// Maximum extent size in bytes. Must be <= `slab_size`.
    pub max_extent_size: u32,
    /// Device sector size in bytes.
    pub sector_size: u32,
    /// Number of regions (must be a power of two).
    pub region_count: u32,
    /// Alignment of checkpoint regions on the metadata disk.
    /// The first checkpoint region starts at the first multiple of this
    /// value that is >= the superblock size.
    pub metadata_alignment: u64,
    /// Instance identifier stored in the superblock. If None, a random
    /// value is generated at format time.
    pub instance_id: Option<u64>,
    /// NVMe namespace identifier for the metadata disk.
    pub metadata_disk_ns_id: u32,
    /// Maximum bytes reserved for metadata (superblock + checkpoints) on the
    /// device. Caps the checkpoint region calculation so that metadata and data
    /// can coexist on the same SSD without the checkpoint area consuming the
    /// entire device. Default: 128 MiB.
    pub metadata_region_size: u64,
}

impl FormatParams {
    pub fn new(data_disk_size: u64, instance_id: Option<u64>) -> Self {
        Self {
            data_disk_size,
            instance_id,
            ..Default::default()
        }
    }
}

impl Default for FormatParams {
    fn default() -> Self {
        Self {
            data_disk_size: 0,
            slab_size: 1024 * 1024 * 1024,       // 1 GiB
            max_extent_size: 1024 * 1024 * 1024, // 1 GiB
            sector_size: 4096,                   // 4 KiB
            region_count: 16,
            metadata_alignment: 128 * 1024, // 128 KiB
            instance_id: None,
            metadata_disk_ns_id: 1,
            metadata_region_size: 128 * 1024 * 1024, // 128 MiB
        }
    }
}

pub struct WriteHandle {
    key: ExtentKey,
    offset: u64,
    size: u32,
    publish_fn: Option<Box<dyn FnOnce() -> Result<Extent, ExtentManagerError> + Send>>,
    abort_fn: Option<Box<dyn FnOnce() + Send>>,
}

impl WriteHandle {
    pub fn new(
        key: ExtentKey,
        offset: u64,
        size: u32,
        publish_fn: Box<dyn FnOnce() -> Result<Extent, ExtentManagerError> + Send>,
        abort_fn: Box<dyn FnOnce() + Send>,
    ) -> Self {
        Self {
            key,
            offset,
            size,
            publish_fn: Some(publish_fn),
            abort_fn: Some(abort_fn),
        }
    }

    pub fn key(&self) -> ExtentKey {
        self.key
    }

    pub fn extent_offset(&self) -> u64 {
        self.offset
    }

    pub fn extent_size(&self) -> u32 {
        self.size
    }

    pub fn publish(mut self) -> Result<Extent, ExtentManagerError> {
        let f = self
            .publish_fn
            .take()
            .expect("publish called on consumed handle");
        self.abort_fn.take();
        f()
    }

    pub fn abort(mut self) {
        self.publish_fn.take();
        if let Some(f) = self.abort_fn.take() {
            f();
        }
    }
}

impl Drop for WriteHandle {
    fn drop(&mut self) {
        if let Some(f) = self.abort_fn.take() {
            f();
        }
    }
}

impl fmt::Debug for WriteHandle {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("WriteHandle")
            .field("key", &self.key)
            .field("offset", &self.offset)
            .field("size", &self.size)
            .field("has_publish_fn", &self.publish_fn.is_some())
            .field("has_abort_fn", &self.abort_fn.is_some())
            .finish()
    }
}

// # Verified Properties (see `components/extent-manager/verif/`)
//
// The following invariants are formally proved with Creusot:
//
// - P1 (sector-size-nonzero): format rejects sector_size == 0
// - P2 (slab-alignment): format rejects slab_size not aligned to sector_size
// - P3 (extent-size-bounded): format rejects max_extent_size > slab_size
// - P4 (region-count-nonzero): format rejects region_count == 0
// - P5 (size-aligned): reserve_extent aligns size up to sector_size boundary
// - P6 (publish-exactly-once): WriteHandle::publish consumes the handle
// - P7 (abort-on-drop): dropping uncommitted WriteHandle auto-aborts reservation
// - P8 (out-of-space): reserve_extent returns OutOfSpace when no capacity
// - P9 (offset-not-found): remove_extent returns OffsetNotFound for invalid offset
// - P10 (lifecycle-valid): reserve→publish produces Extent with aligned size and correct offset
//
// Total: 10 properties, 22 verification conditions discharged by SMT solvers.

#[cfg(feature = "spdk")]
define_interface! {
    pub IExtentManager {
        /// Format the extent manager, writing superblock and initializing regions.
        ///
        /// # Verified: P1 (sector-size-nonzero), P2 (slab-alignment), P3 (extent-size-bounded), P4 (region-count-nonzero)
        /// Validates all format parameters before writing to disk.
        ///
        /// # Unchecked: Superblock write atomicity
        /// Superblock is written as a single 4KiB block. Power loss during write
        /// could leave corrupt metadata. Suggested technique: crash-injection test.
        fn format(&self, params: FormatParams) -> Result<(), ExtentManagerError>;

        /// Recover state from persisted metadata (superblock + checkpoints).
        ///
        /// # Unchecked: Recovery correctness after crash
        /// Depends on dual-checkpoint consistency and monotonic sequence numbers.
        /// Suggested technique: crash-injection + recovery verification test.
        fn initialize(&self) -> Result<(), ExtentManagerError>;

        /// Reserve space for a new extent and return a WriteHandle.
        ///
        /// # Verified: P5 (size-aligned), P8 (out-of-space), P10 (lifecycle-valid)
        /// Returned WriteHandle has sector-aligned size >= requested size.
        /// Returns OutOfSpace when buddy allocator cannot satisfy request.
        fn reserve_extent(
            &self,
            key: ExtentKey,
            size: u32,
        ) -> Result<WriteHandle, ExtentManagerError>;

        /// Return all committed extents.
        fn get_extents(&self) -> Vec<Extent>;

        /// Iterate over all committed extents without collecting.
        fn for_each_extent(&self, cb: &mut dyn FnMut(&Extent));

        /// Remove an extent at the given block offset.
        ///
        /// # Verified: P9 (offset-not-found)
        /// Returns OffsetNotFound if no extent exists at that offset.
        fn remove_extent(&self, offset: u64) -> Result<(), ExtentManagerError>;

        /// Persist current state to the metadata device.
        ///
        /// # Unchecked: Checkpoint atomicity (dual-region alternation)
        /// Uses two checkpoint regions with sequence numbers. A crash during
        /// write should not corrupt both. Suggested technique: crash-injection.
        fn checkpoint(&self) -> Result<(), ExtentManagerError>;

        /// Return the instance identifier from the superblock.
        fn get_instance_id(&self) -> Result<u64, ExtentManagerError>;

        /// Set the automatic checkpoint interval.
        ///
        /// `Some(duration)` enables the background checkpoint thread to fire
        /// every `duration`. `None` disables automatic checkpoints entirely;
        /// callers must then invoke `checkpoint()` manually. The default is
        /// 30 seconds.
        fn set_checkpoint_interval(&self, interval: Option<std::time::Duration>);

        /// Return the number of bytes currently allocated across all regions.
        fn used_bytes(&self) -> u64;

        /// Return the total usable capacity in bytes across all regions.
        fn capacity_bytes(&self) -> u64;

        /// Set the base LBA offset for all metadata I/O (partition-relative).
        fn set_metadata_base_lba(&self, base_lba: u64);

        /// Set the base LBA offset for the data partition.
        fn set_data_base_lba(&self, base_lba: u64);

        /// Get the configured data base LBA offset.
        fn data_base_lba(&self) -> u64;
    }
}
