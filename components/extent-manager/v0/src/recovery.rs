//! Crash recovery: orphan detection and cleanup.

use crate::bitmap::AllocationBitmap;
use crate::block_device::BlockDevice;
use crate::error::ExtentManagerError;
use crate::metadata::{ExtentMetadata, OnDiskExtentRecord, BLOCK_SIZE};
use crate::superblock::Superblock;
use std::collections::HashMap;

/// Result of a recovery scan.
///
/// # Examples
///
/// ```
/// use extent_manager::recovery::RecoveryResult;
///
/// let result = RecoveryResult {
///     extents_loaded: 100,
///     orphans_cleaned: 2,
///     corrupt_records: 0,
/// };
/// assert_eq!(result.extents_loaded, 100);
/// ```
#[derive(Debug, Clone)]
pub struct RecoveryResult {
    /// Number of valid extents loaded into the in-memory index.
    pub extents_loaded: u64,
    /// Number of orphan records detected and cleaned up.
    pub orphans_cleaned: u64,
    /// Number of corrupt records detected (CRC mismatch).
    pub corrupt_records: u64,
}

/// Perform crash recovery: load bitmaps, scan records, detect orphans.
///
/// Returns the rebuilt in-memory index, loaded bitmaps, and recovery stats.
///
/// An orphan is a record that exists on disk but whose bitmap bit is not set
/// (indicating the create was interrupted or the remove completed the bitmap
/// clear but not the record cleanup). Orphan records are zeroed out.
/// Recovery output: (index, bitmaps, stats).
pub type RecoverOutput = (
    HashMap<u64, ExtentMetadata>,
    Vec<AllocationBitmap>,
    RecoveryResult,
);

pub(crate) fn recover(
    bd: &BlockDevice,
    sb: &Superblock,
) -> Result<RecoverOutput, ExtentManagerError> {
    let num_classes = sb.num_size_classes();

    // Load bitmaps.
    let mut bitmaps = Vec::with_capacity(num_classes as usize);
    for class in 0..num_classes {
        let lba = sb.bitmap_lba_for_class(class).unwrap();
        let slots = sb.slots_for_class(class).unwrap();
        let bm = AllocationBitmap::load(bd, lba, slots).map_err(ExtentManagerError::IoError)?;
        bitmaps.push(bm);
    }

    let mut index = HashMap::new();
    let mut stats = RecoveryResult {
        extents_loaded: 0,
        orphans_cleaned: 0,
        corrupt_records: 0,
    };

    // Scan all allocated slots across all size classes.
    for class in 0..num_classes {
        let slots = sb.slots_for_class(class).unwrap();
        let extent_size = sb.size_for_class(class).unwrap();

        for slot in 0..slots {
            let global = sb.global_slot(class, slot);
            let record_lba = sb.record_lba(global);
            let bit_set = bitmaps[class as usize].is_set(slot);

            // Read the record block.
            let mut block = [0u8; BLOCK_SIZE];
            bd.read_block(record_lba, &mut block)
                .map_err(ExtentManagerError::IoError)?;

            let record = OnDiskExtentRecord::from_bytes(block);

            if record.is_empty() {
                // Empty slot — nothing to do.
                continue;
            }

            match record.deserialize() {
                Ok(mut meta) => {
                    meta.extent_size = extent_size;

                    if bit_set {
                        // Valid allocated extent.
                        index.insert(meta.key, meta);
                        stats.extents_loaded += 1;
                    } else {
                        // Orphan: record exists but bit is not set.
                        // Zero out the record block.
                        let zeroed = [0u8; BLOCK_SIZE];
                        bd.write_block(record_lba, &zeroed)
                            .map_err(ExtentManagerError::IoError)?;
                        stats.orphans_cleaned += 1;
                    }
                }
                Err(_) => {
                    // Corrupt record — clear the bitmap bit if set and zero the record.
                    if bit_set {
                        bitmaps[class as usize].clear(slot);
                        let bm_lba = sb.bitmap_lba_for_class(class).unwrap();
                        bitmaps[class as usize]
                            .persist_block_for_slot(bd, bm_lba, slot)
                            .map_err(ExtentManagerError::IoError)?;
                    }
                    let zeroed = [0u8; BLOCK_SIZE];
                    bd.write_block(record_lba, &zeroed)
                        .map_err(ExtentManagerError::IoError)?;
                    stats.corrupt_records += 1;
                }
            }
        }
    }

    Ok((index, bitmaps, stats))
}
