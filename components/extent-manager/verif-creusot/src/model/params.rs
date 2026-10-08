//! Plain-data mirrors: `FormatParams` (interfaces/src/iextent_manager.rs:43-67),
//! `ExtentManagerError` (iextent_manager.rs:18-25), `Superblock`
//! (extent-manager/src/superblock.rs:8-24, 27-56), `SharedState` (region.rs:18-22).
//!
//! `ExtentManagerError`'s `String` payloads are dropped: no property of this
//! component depends on message CONTENT (that is IX-CREUSOT-STRING-CONTENT anyway);
//! the variant is kept.
use crate::model::assume::*;
use creusot_std::prelude::*;

/// Mirror of `interfaces::FormatParams` (all fields; `Copy` replaces `Clone`).
#[derive(::core::clone::Clone, ::core::marker::Copy)]
pub struct FormatParams {
    pub data_disk_size: u64,
    pub slab_size: u64,
    pub max_extent_size: u32,
    pub sector_size: u32,
    pub region_count: u32,
    pub metadata_alignment: u64,
    pub instance_id: Option<u64>,
    pub metadata_disk_ns_id: u32,
    pub metadata_region_size: u64,
}

/// Mirror of `interfaces::ExtentManagerError` (variants only).
#[derive(::core::clone::Clone, ::core::marker::Copy)]
pub enum EmError {
    CorruptMetadata,
    IoError,
    NotInitialized,
    OffsetNotFound(u64),
    OutOfSpace,
}

/// `superblock.rs:4` / `:5` / `:6`.
pub const SUPERBLOCK_SIZE: usize = 4096;
pub const SUPERBLOCK_MAGIC: u64 = 0x4345_5254_5553_5634;
pub const FORMAT_VERSION: u32 = 6;

/// Mirror of `Superblock` (superblock.rs:8-24).
#[derive(::core::clone::Clone, ::core::marker::Copy)]
pub struct Superblock {
    pub magic: u64,
    pub version: u32,
    pub data_disk_size: u64,
    pub sector_size: u32,
    pub slab_size: u64,
    pub max_extent_size: u32,
    pub region_count: u32,
    pub checkpoint_seq: u64,
    pub active_copy: u8,
    pub checkpoint_region_offset: u64,
    pub checkpoint_region_size: u64,
    pub instance_id: u64,
    pub metadata_disk_ns_id: u32,
    pub data_start_offset: u64,
}

impl Superblock {
    /// Mirror of `Superblock::new` (superblock.rs:27-56).
    #[ensures(result.magic == SUPERBLOCK_MAGIC && result.version == FORMAT_VERSION)]
    #[ensures(result.data_disk_size == data_disk_size && result.sector_size == sector_size)]
    #[ensures(result.slab_size == slab_size && result.max_extent_size == max_extent_size)]
    #[ensures(result.region_count == region_count && result.checkpoint_seq@ == 0 && result.active_copy@ == 0)]
    #[ensures(result.checkpoint_region_offset == checkpoint_region_offset)]
    #[ensures(result.checkpoint_region_size == checkpoint_region_size)]
    #[ensures(result.instance_id == instance_id && result.metadata_disk_ns_id == metadata_disk_ns_id)]
    #[ensures(result.data_start_offset == data_start_offset)]
    pub fn new(
        data_disk_size: u64,
        sector_size: u32,
        slab_size: u64,
        max_extent_size: u32,
        region_count: u32,
        checkpoint_region_offset: u64,
        checkpoint_region_size: u64,
        instance_id: u64,
        metadata_disk_ns_id: u32,
        data_start_offset: u64,
    ) -> Self {
        Self {
            magic: SUPERBLOCK_MAGIC,
            version: FORMAT_VERSION,
            data_disk_size,
            sector_size,
            slab_size,
            max_extent_size,
            region_count,
            checkpoint_seq: 0,
            active_copy: 0,
            checkpoint_region_offset,
            checkpoint_region_size,
            instance_id,
            metadata_disk_ns_id,
            data_start_offset,
        }
    }
}

/// Mirror of `SharedState` (region.rs:18-22).
#[derive(::core::clone::Clone, ::core::marker::Copy)]
pub struct SharedState {
    pub format_params: FormatParams,
    pub checkpoint_seq: u64,
    pub superblock: Superblock,
}
