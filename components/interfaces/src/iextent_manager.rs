//! Interface for the extent-manager component and shared error type.

use component_macros::define_interface;
use std::fmt;

/// A storage extent returned by the extent manager.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Extent {
    pub key: u64,    // Key corresponding to cache block
    pub size: u32,   // Size of the extent in bytes
    pub offset: u64, // Offset in bytes in the logical space
}

/// Errors returned by `IExtentManager` operations.
#[derive(Debug, Clone)]
pub enum ExtentManagerError {
    CorruptMetadata(String),
    DuplicateKey(u64),
    IoError(String),
    KeyNotFound(u64),
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

#[cfg(feature = "spdk")]
define_interface! {
    pub IExtentManager {
        /// Set the DMA allocator used for block device I/O.
        fn set_dma_alloc(&self, alloc: crate::spdk_types::DmaAllocFn);

        /// Initialize the extent manager with the given capacity.
        fn initialize(
            &self,
            total_size_bytes: u64,
            slab_size_bytes: u32,
        ) -> Result<(), ExtentManagerError>;

        /// Allocate a new extent.
        fn create_extent(
            &self,
            key: u64,
            extent_size: u32,
        ) -> Result<Extent, ExtentManagerError>;

        /// Free an extent
        fn remove_extent(&self, key: u64) -> Result<(), ExtentManagerError>;

        /// Get info about an extent
        fn lookup_extent(&self, key: u64) -> Result<Extent, ExtentManagerError>;

        /// Return all currently allocated extents.
        fn get_extents(&self) -> Vec<Extent>;
    }
}
