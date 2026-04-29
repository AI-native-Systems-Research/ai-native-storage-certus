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
    pub size: u32,
    pub offset: u64,
}

/// Errors returned by `IExtentManager` operations.
#[derive(Debug, Clone)]
pub enum ExtentManagerError {
    CorruptMetadata(String),
    DuplicateKey(ExtentKey),
    IoError(String),
    KeyNotFound(ExtentKey),
    NotInitialized(String),
    OutOfSpace,
}

impl fmt::Display for ExtentManagerError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::CorruptMetadata(msg) => write!(f, "corrupt metadata: {msg}"),
            Self::DuplicateKey(k) => write!(f, "duplicate key: {k}"),
            Self::IoError(msg) => write!(f, "I/O error: {msg}"),
            Self::KeyNotFound(k) => write!(f, "key not found: {k}"),
            Self::NotInitialized(msg) => write!(f, "not initialized: {msg}"),
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
}

impl FormatParams {
    pub fn new(data_disk_size: u64) -> Self {
        Self {
            data_disk_size,
            ..Default::default()
        }
    }
}

impl Default for FormatParams {
    fn default() -> Self {
        Self {
            data_disk_size: 0,
            slab_size: 1024 * 1024 * 1024,         // 1 GiB
            max_extent_size: 1024 * 1024 * 1024,    // 1 GiB
            sector_size: 4096,                       // 4 KiB
            region_count: 16,
            metadata_alignment: 128 * 1024,          // 128 KiB
            instance_id: None,
            metadata_disk_ns_id: 1,
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

#[cfg(feature = "spdk")]
define_interface! {
    pub IExtentManager {
        fn format(&self, params: FormatParams) -> Result<(), ExtentManagerError>;

        fn initialize(&self) -> Result<(), ExtentManagerError>;

        fn reserve_extent(
            &self,
            key: ExtentKey,
            size: u32,
        ) -> Result<WriteHandle, ExtentManagerError>;

        fn lookup_extent(&self, key: ExtentKey) -> Result<Extent, ExtentManagerError>;

        fn get_extents(&self) -> Vec<Extent>;

        fn for_each_extent(&self, cb: &mut dyn FnMut(&Extent));

        fn remove_extent(&self, key: ExtentKey) -> Result<(), ExtentManagerError>;

        fn checkpoint(&self) -> Result<(), ExtentManagerError>;

        fn get_instance_id(&self) -> Result<u64, ExtentManagerError>;
    }
}
