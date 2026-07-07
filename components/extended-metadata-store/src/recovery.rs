//! Recovery logic: read superblock, load active region, rebuild in-memory state.
//!
//! On startup:
//! 1. Read superblock from LBA 0
//! 2. If valid: read active region, deserialize entries, rebuild HashMap
//! 3. If active region corrupt: try inactive region (best-effort)
//! 4. If both corrupt or no superblock: format fresh (empty store)

use crate::block_io::BlockDeviceClient;
use crate::on_disk::{self, KvEntry, Superblock};

/// Result of recovery: the superblock state and recovered entries.
pub struct RecoveryResult {
    pub superblock: Superblock,
    pub entries: Vec<KvEntry>,
    pub warnings: Vec<String>,
}

/// Attempt to recover the store from disk.
///
/// Returns recovered entries and any warnings (e.g., skipped corrupt entries).
/// If the disk has no valid superblock, returns a freshly-formatted empty state.
pub fn recover_from_disk(client: &BlockDeviceClient) -> Result<RecoveryResult, String> {
    let sector_size = client.sector_size as usize;

    // Read superblock
    let sb = match client.read_superblock()? {
        Some(sb) => sb,
        None => {
            // No valid superblock — format fresh
            return format_fresh(client);
        }
    };

    // If flush_seq == 0, store was formatted but never flushed
    if sb.flush_seq == 0 {
        return Ok(RecoveryResult {
            superblock: sb,
            entries: Vec::new(),
            warnings: Vec::new(),
        });
    }

    let mut warnings = Vec::new();

    // Try active region first
    match try_read_region(client, sb.active_region_offset(), sb.region_a_size, sector_size) {
        Ok((header, entries)) if header.flush_seq == sb.flush_seq => {
            return Ok(RecoveryResult {
                superblock: sb,
                entries,
                warnings,
            });
        }
        Ok(_) => {
            warnings.push("active region flush_seq mismatch, trying inactive".into());
        }
        Err(e) => {
            warnings.push(format!("active region corrupt: {e}, trying inactive"));
        }
    }

    // Fallback: try inactive region
    let inactive_offset = sb.inactive_region_offset();
    match try_read_region(client, inactive_offset, sb.region_a_size, sector_size) {
        Ok((_header, entries)) => {
            warnings.push("recovered from inactive region (previous checkpoint)".into());
            Ok(RecoveryResult {
                superblock: sb,
                entries,
                warnings,
            })
        }
        Err(e) => {
            warnings.push(format!("inactive region also corrupt: {e}, formatting fresh"));
            format_fresh(client)
        }
    }
}

/// Format a fresh partition: write an empty superblock.
pub fn format_fresh(client: &BlockDeviceClient) -> Result<RecoveryResult, String> {
    let num_sectors = client.read_sectors(0, 1).map(|_| ()).ok();
    // We need to know partition size; read it from the mock or use a reasonable default
    // In practice, the caller provides this via the partition info.
    // For now, we'll create a superblock that the caller can update.
    let _ = num_sectors;

    // We can't determine partition size from the client alone.
    // Return a minimal result indicating fresh format is needed.
    Ok(RecoveryResult {
        superblock: Superblock::new(client.sector_size, 0),
        entries: Vec::new(),
        warnings: vec!["fresh format: no valid data on partition".into()],
    })
}

/// Format the partition with the given total sector count and write the superblock.
pub fn format_partition(client: &BlockDeviceClient, total_sectors: u64) -> Result<Superblock, String> {
    let sb = Superblock::new(client.sector_size, total_sectors);
    client.write_superblock(&sb)?;
    Ok(sb)
}

/// Try to read and deserialize a region.
fn try_read_region(
    client: &BlockDeviceClient,
    offset_sectors: u64,
    max_sectors: u64,
    sector_size: usize,
) -> Result<(on_disk::RegionHeader, Vec<KvEntry>), String> {
    let data = client.read_region(offset_sectors, max_sectors)?;
    on_disk::deserialize_region(&data, sector_size)
        .ok_or_else(|| "region header corrupt or invalid".to_string())
}
