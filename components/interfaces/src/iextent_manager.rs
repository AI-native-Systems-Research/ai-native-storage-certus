//! Interface for the extent-manager component and shared types.

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
    pub slab_size: u32,
    pub max_element_size: u32,
    pub metadata_block_size: u32,
    pub sector_size: u32,
    pub region_count: u32,
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

#[cfg(feature = "spdk")]
define_interface! {
    pub IExtentManager {
        fn set_dma_alloc(&self, alloc: crate::spdk_types::DmaAllocFn);

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
    }
}
