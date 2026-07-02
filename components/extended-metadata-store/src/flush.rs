//! Flush logic: serialize in-memory state to disk using the dual-region layout.

use crate::block_io::BlockDeviceClient;
use crate::on_disk::{self, Superblock};

/// Flush the current entries to disk using the dual-region ping-pong strategy.
///
/// 1. Serialize all entries to the INACTIVE region
/// 2. Write region data to disk
/// 3. Update superblock to point to the newly-written region
/// 4. Write superblock (atomic commit point)
pub fn flush_to_disk(
    client: &BlockDeviceClient,
    superblock: &mut Superblock,
    entries: &[(String, Vec<u8>)],
) -> Result<(), String> {
    let sector_size = client.sector_size as usize;
    let new_seq = superblock.flush_seq + 1;

    // Serialize all entries into the inactive region
    let region_data = on_disk::serialize_region(entries, new_seq, sector_size);
    let region_sectors = on_disk::bytes_to_sectors(region_data.len(), sector_size);

    // Check capacity
    let max_region_sectors = superblock.region_a_size;
    if region_sectors > max_region_sectors {
        return Err(format!(
            "region data ({region_sectors} sectors) exceeds region capacity ({max_region_sectors} sectors)"
        ));
    }

    // Write to the inactive region
    let inactive_offset = superblock.inactive_region_offset();
    client.write_region(inactive_offset, &region_data)?;

    // Update superblock: flip active region, bump sequence
    superblock.active_region = if superblock.active_region == 0 { 1 } else { 0 };
    superblock.flush_seq = new_seq;
    superblock.entry_count = entries.len() as u64;

    // Write superblock (the atomic commit point)
    client.write_superblock(superblock)?;

    Ok(())
}
