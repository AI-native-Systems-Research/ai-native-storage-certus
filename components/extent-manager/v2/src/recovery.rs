use std::collections::HashMap;

use interfaces::{Extent, ExtentKey, ExtentManagerError};

use crate::block_io::BlockDeviceClient;
use crate::checkpoint::{self, SlabDescriptor};
use crate::error;
use crate::superblock::{Superblock, SUPERBLOCK_SIZE};

pub(crate) type PerRegionData = Vec<(HashMap<ExtentKey, Extent>, Vec<SlabDescriptor>)>;

pub(crate) fn recover(
    client: &BlockDeviceClient,
    component: &crate::MetadataManagerV2,
) -> Result<(Superblock, PerRegionData), ExtentManagerError> {
    let sb_data = client.read_blocks(0, SUPERBLOCK_SIZE)?;
    let sb = Superblock::deserialize(&sb_data)?;

    if sb.current_index_lba == 0 && sb.previous_index_lba == 0 {
        let empty: PerRegionData = (0..sb.region_count as usize)
            .map(|_| (HashMap::new(), Vec::new()))
            .collect();
        return Ok((sb, empty));
    }

    if sb.current_index_lba != 0 {
        match checkpoint::read_chunk_chain(
            client,
            sb.current_index_lba,
            sb.checkpoint_seq,
            sb.metadata_block_size,
        ) {
            Ok(data) => {
                let regions = checkpoint::deserialize_index_and_slabs(&data)?;
                return Ok((sb, regions));
            }
            Err(e) => {
                component.log_warn(&format!(
                    "recovery_fallback: primary chain corrupt: {e}"
                ));
            }
        }
    }

    if sb.previous_index_lba != 0 {
        let prev_seq = sb.checkpoint_seq.saturating_sub(1);
        match checkpoint::read_chunk_chain(
            client,
            sb.previous_index_lba,
            prev_seq,
            sb.metadata_block_size,
        ) {
            Ok(data) => {
                let regions = checkpoint::deserialize_index_and_slabs(&data)?;
                return Ok((sb, regions));
            }
            Err(e) => {
                component.log_error(&format!(
                    "corruption_detected: both checkpoint chains corrupt: {e}"
                ));
            }
        }
    }

    Err(error::corrupt_metadata(
        "both primary and fallback checkpoint chains are corrupt",
    ))
}
