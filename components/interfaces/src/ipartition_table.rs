//! IPartitionTable interface and associated types for GPT partition management.

use std::fmt;

#[cfg(feature = "spdk")]
use component_macros::define_interface;

/// Information about a single partition.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PartitionInfo {
    /// Partition index (0-based).
    pub index: u32,
    /// Starting LBA of the partition.
    pub start_lba: u64,
    /// Number of sectors in the partition.
    pub num_sectors: u64,
    /// Partition type GUID (as 16-byte mixed-endian array per UEFI spec).
    pub type_guid: [u8; 16],
    /// Partition unique GUID.
    pub unique_guid: [u8; 16],
    /// UTF-8 partition name (decoded from GPT's UTF-16LE).
    pub name: String,
}

/// Configuration for a single partition in a format request.
#[derive(Debug, Clone)]
pub struct PartitionSpec {
    /// Partition type GUID.
    pub type_guid: [u8; 16],
    /// Size in bytes. 0 means "use all remaining space."
    pub size_bytes: u64,
    /// Partition name (max 36 UTF-16LE characters).
    pub name: String,
}

/// Configuration for formatting a partition table.
#[derive(Debug, Clone)]
pub struct PartitionConfig {
    /// Sector size in bytes (512 or 4096).
    pub sector_size: u32,
    /// Total number of sectors on the device.
    pub total_sectors: u64,
    /// NVMe namespace ID.
    pub ns_id: u32,
    /// Partition specifications (ordered).
    pub partitions: Vec<PartitionSpec>,
}

/// Resolved partition table after initialize or format.
#[derive(Debug, Clone)]
pub struct PartitionTable {
    /// All partitions discovered/created.
    pub partitions: Vec<PartitionInfo>,
    /// Device sector size.
    pub sector_size: u32,
}

/// Errors from partition table operations.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PartitionTableError {
    /// No valid GPT found on disk.
    NoPartitionTable(String),
    /// GPT header or entries are corrupt.
    CorruptTable(String),
    /// Requested partition index does not exist.
    InvalidPartition(String),
    /// I/O error accessing the block device.
    IoError(String),
    /// Partition layout doesn't fit on the device.
    LayoutError(String),
    /// Component not initialized.
    NotInitialized(String),
}

impl fmt::Display for PartitionTableError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NoPartitionTable(msg) => write!(f, "no partition table: {msg}"),
            Self::CorruptTable(msg) => write!(f, "corrupt partition table: {msg}"),
            Self::InvalidPartition(msg) => write!(f, "invalid partition: {msg}"),
            Self::IoError(msg) => write!(f, "partition I/O error: {msg}"),
            Self::LayoutError(msg) => write!(f, "partition layout error: {msg}"),
            Self::NotInitialized(msg) => write!(f, "not initialized: {msg}"),
        }
    }
}

impl std::error::Error for PartitionTableError {}

/// Well-known partition type GUIDs for Certus.
pub mod type_guids {
    /// Certus extent-manager metadata partition.
    /// UUID: 7C3A8E01-1B4F-4A2D-9E6C-0D3F5A8B7C01
    pub const CERTUS_METADATA: [u8; 16] = [
        0x01, 0x8E, 0x3A, 0x7C, 0x4F, 0x1B, 0x2D, 0x4A, 0x9E, 0x6C, 0x0D, 0x3F, 0x5A, 0x8B, 0x7C,
        0x01,
    ];

    /// Certus extent-manager data partition.
    /// UUID: 7C3A8E02-1B4F-4A2D-9E6C-0D3F5A8B7C02
    pub const CERTUS_DATA: [u8; 16] = [
        0x02, 0x8E, 0x3A, 0x7C, 0x4F, 0x1B, 0x2D, 0x4A, 0x9E, 0x6C, 0x0D, 0x3F, 0x5A, 0x8B, 0x7C,
        0x02,
    ];

    /// Certus external metadata partition.
    /// UUID: 7C3A8E03-1B4F-4A2D-9E6C-0D3F5A8B7C03
    pub const CERTUS_EXTERNAL_META: [u8; 16] = [
        0x03, 0x8E, 0x3A, 0x7C, 0x4F, 0x1B, 0x2D, 0x4A, 0x9E, 0x6C, 0x0D, 0x3F, 0x5A, 0x8B, 0x7C,
        0x03,
    ];
}

#[cfg(feature = "spdk")]
define_interface! {
    pub IPartitionTable {
        /// Read and validate the GPT on the connected block device.
        fn initialize(&self) -> Result<PartitionTable, PartitionTableError>;

        /// Write a new GPT with the specified partition layout.
        fn format(&self, config: PartitionConfig) -> Result<PartitionTable, PartitionTableError>;

        /// Get info for a specific partition by index (0-based).
        fn partition_info(&self, index: u32) -> Result<PartitionInfo, PartitionTableError>;

        /// Return the number of partitions.
        fn num_partitions(&self) -> Result<u32, PartitionTableError>;
    }
}
