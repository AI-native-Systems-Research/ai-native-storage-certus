use std::collections::HashMap;

use interfaces::{Extent, ExtentKey, ExtentManagerError};

use crate::block_io::BlockDeviceClient;
use crate::checkpoint::{self, SlabDescriptor};
use crate::error;
use crate::superblock::{Superblock, SUPERBLOCK_SIZE};

pub(crate) type PerRegionData = Vec<(HashMap<ExtentKey, Extent>, Vec<SlabDescriptor>)>;

pub(crate) fn recover(
    metadata_client: &BlockDeviceClient,
    component: &crate::ExtentManagerV2,
) -> Result<(Superblock, PerRegionData), ExtentManagerError> {
    let sb_data = metadata_client.read_blocks(0, SUPERBLOCK_SIZE)?;
    let sb = Superblock::deserialize(&sb_data)?;

    if sb.checkpoint_seq == 0 {
        let empty: PerRegionData = (0..sb.region_count as usize)
            .map(|_| (HashMap::new(), Vec::new()))
            .collect();
        return Ok((sb, empty));
    }

    let active_offset = sb.checkpoint_region_offset
        + sb.active_copy as u64 * sb.checkpoint_region_size;
    let inactive_offset = sb.checkpoint_region_offset
        + (1 - sb.active_copy) as u64 * sb.checkpoint_region_size;

    // Try active copy first
    match checkpoint::read_checkpoint_region(
        metadata_client,
        active_offset,
        sb.checkpoint_region_size,
        sb.checkpoint_seq,
    ) {
        Ok(data) => {
            let regions = checkpoint::deserialize_index_and_slabs(&data)?;
            return Ok((sb, regions));
        }
        Err(e) => {
            component.log_warn(&format!(
                "recovery_fallback: active checkpoint (copy {}) corrupt: {e}",
                sb.active_copy
            ));
        }
    }

    // Fall back to inactive copy (previous checkpoint)
    let prev_seq = sb.checkpoint_seq.saturating_sub(1);
    if prev_seq > 0 {
        match checkpoint::read_checkpoint_region(
            metadata_client,
            inactive_offset,
            sb.checkpoint_region_size,
            prev_seq,
        ) {
            Ok(data) => {
                let regions = checkpoint::deserialize_index_and_slabs(&data)?;
                return Ok((sb, regions));
            }
            Err(e) => {
                component.log_error(&format!(
                    "corruption_detected: both checkpoint copies corrupt: {e}"
                ));
            }
        }
    }

    Err(error::corrupt_metadata(
        "both active and inactive checkpoint copies are corrupt",
    ))
}
