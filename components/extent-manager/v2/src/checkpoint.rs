use std::sync::{Arc, Mutex};

use parking_lot::RwLock;

use interfaces::{Extent, ExtentKey, ExtentManagerError};

use crate::block_io::BlockDeviceClient;
use crate::error;
use crate::region::{RegionState, SharedState};

pub(crate) const CHECKPOINT_HEADER_SIZE: usize = 16; // u64 seq + u32 payload_len + u32 CRC

const INDEX_ENTRY_SIZE: usize = 20; // u64 key + u64 offset + u32 size
const SLAB_ENTRY_SIZE: usize = 20;  // u64 start_offset + u64 slab_size + u32 element_size

fn serialize_region(region: &RegionState) -> Vec<u8> {
    let mut data = Vec::new();

    let num_entries = region.index.len() as u32;
    data.extend_from_slice(&num_entries.to_le_bytes());

    for (key, extent) in &region.index {
        data.extend_from_slice(&key.to_le_bytes());
        data.extend_from_slice(&extent.offset.to_le_bytes());
        data.extend_from_slice(&extent.size.to_le_bytes());
    }

    let num_slabs = region.slabs.len() as u32;
    data.extend_from_slice(&num_slabs.to_le_bytes());

    for slab in &region.slabs {
        data.extend_from_slice(&slab.start_offset.to_le_bytes());
        data.extend_from_slice(&slab.slab_size.to_le_bytes());
        data.extend_from_slice(&slab.element_size.to_le_bytes());
    }

    data
}

pub(crate) fn write_checkpoint(
    metadata_client: &BlockDeviceClient,
    regions_lock: &RwLock<Option<Vec<Arc<RwLock<RegionState>>>>>,
    shared_mutex: &Mutex<Option<SharedState>>,
) -> Result<(), ExtentManagerError> {
    let regions = regions_lock.read();
    let regions = regions
        .as_ref()
        .ok_or_else(|| error::not_initialized("component not initialized"))?;

    let region_count = regions.len();

    // Phase 1: Serialize all regions
    let mut payload = Vec::new();
    payload.extend_from_slice(&(region_count as u32).to_le_bytes());

    for region_arc in regions.iter() {
        let region = region_arc.read();
        let region_data = serialize_region(&region);
        payload.extend_from_slice(&region_data);
    }

    // Phase 2: Build checkpoint region data with header
    let (inactive_copy, region_offset, region_size, new_seq) = {
        let shared = shared_mutex.lock().unwrap();
        let s = shared.as_ref().unwrap();
        let inactive = 1 - s.superblock.active_copy;
        let offset = s.superblock.checkpoint_region_offset
            + inactive as u64 * s.superblock.checkpoint_region_size;
        let size = s.superblock.checkpoint_region_size;
        let seq = s.checkpoint_seq + 1;
        (inactive, offset, size, seq)
    };

    let total_needed = CHECKPOINT_HEADER_SIZE + payload.len();
    if total_needed as u64 > region_size {
        return Err(error::corrupt_metadata(&format!(
            "checkpoint payload ({} bytes) exceeds region size ({} bytes)",
            total_needed, region_size
        )));
    }

    // Build the header + payload blob
    let mut blob = Vec::with_capacity(region_size as usize);
    blob.extend_from_slice(&new_seq.to_le_bytes());           // 8 bytes
    blob.extend_from_slice(&(payload.len() as u32).to_le_bytes()); // 4 bytes
    blob.extend_from_slice(&0u32.to_le_bytes());               // 4 bytes CRC placeholder
    blob.extend_from_slice(&payload);

    let crc = crc32fast::hash(&blob);
    blob[12..16].copy_from_slice(&crc.to_le_bytes());

    // Pad to region size
    blob.resize(region_size as usize, 0);

    // Phase 3: Write to metadata device
    let sector_size = metadata_client.sector_size();
    let lba = region_offset / sector_size as u64;
    metadata_client.write_blocks(lba, &blob)?;

    // Phase 4: Update superblock
    {
        let mut shared = shared_mutex.lock().unwrap();
        let s = shared.as_mut().unwrap();
        s.superblock.active_copy = inactive_copy;
        s.superblock.checkpoint_seq = new_seq;
        s.checkpoint_seq = new_seq;
    }

    Ok(())
}

pub(crate) fn read_checkpoint_region(
    metadata_client: &BlockDeviceClient,
    region_byte_offset: u64,
    region_size: u64,
    expected_seq: u64,
) -> Result<Vec<u8>, ExtentManagerError> {
    let sector_size = metadata_client.sector_size();
    let lba = region_byte_offset / sector_size as u64;
    let raw = metadata_client.read_blocks(lba, region_size as usize)?;

    if raw.len() < CHECKPOINT_HEADER_SIZE {
        return Err(error::corrupt_metadata("checkpoint region too short"));
    }

    let seq = u64::from_le_bytes(raw[0..8].try_into().unwrap());
    let payload_len = u32::from_le_bytes(raw[8..12].try_into().unwrap()) as usize;
    let stored_crc = u32::from_le_bytes(raw[12..16].try_into().unwrap());

    if seq != expected_seq {
        return Err(error::corrupt_metadata(&format!(
            "checkpoint seq mismatch: expected={expected_seq} got={seq}"
        )));
    }

    let total = CHECKPOINT_HEADER_SIZE + payload_len;
    if total > raw.len() {
        return Err(error::corrupt_metadata("checkpoint payload truncated"));
    }

    let mut check_buf = raw[..total].to_vec();
    check_buf[12..16].copy_from_slice(&0u32.to_le_bytes());
    let computed_crc = crc32fast::hash(&check_buf);

    if stored_crc != computed_crc {
        return Err(error::corrupt_metadata(&format!(
            "checkpoint CRC mismatch: stored={stored_crc:#x} computed={computed_crc:#x}"
        )));
    }

    Ok(raw[CHECKPOINT_HEADER_SIZE..total].to_vec())
}

#[derive(Debug, Clone)]
pub(crate) struct SlabDescriptor {
    pub start_offset: u64,
    pub slab_size: u64,
    pub element_size: u32,
}

pub(crate) fn deserialize_index_and_slabs(
    data: &[u8],
) -> Result<
    Vec<(
        std::collections::HashMap<ExtentKey, Extent>,
        Vec<SlabDescriptor>,
    )>,
    ExtentManagerError,
> {
    if data.len() < 4 {
        return Err(error::corrupt_metadata("checkpoint data too short"));
    }

    let mut pos = 0;

    let region_count = u32::from_le_bytes(data[pos..pos + 4].try_into().unwrap()) as usize;
    pos += 4;

    let mut result = Vec::with_capacity(region_count);

    for _ in 0..region_count {
        if pos + 4 > data.len() {
            return Err(error::corrupt_metadata("truncated region index count"));
        }
        let num_entries = u32::from_le_bytes(data[pos..pos + 4].try_into().unwrap()) as usize;
        pos += 4;

        let mut index = std::collections::HashMap::with_capacity(num_entries);
        for _ in 0..num_entries {
            if pos + INDEX_ENTRY_SIZE > data.len() {
                return Err(error::corrupt_metadata("truncated index entry"));
            }
            let key = u64::from_le_bytes(data[pos..pos + 8].try_into().unwrap());
            pos += 8;
            let offset = u64::from_le_bytes(data[pos..pos + 8].try_into().unwrap());
            pos += 8;
            let size = u32::from_le_bytes(data[pos..pos + 4].try_into().unwrap());
            pos += 4;

            index.insert(key, Extent { key, offset, size });
        }

        if pos + 4 > data.len() {
            return Err(error::corrupt_metadata("truncated slab count"));
        }
        let num_slabs = u32::from_le_bytes(data[pos..pos + 4].try_into().unwrap()) as usize;
        pos += 4;

        let mut slabs = Vec::with_capacity(num_slabs);
        for _ in 0..num_slabs {
            if pos + SLAB_ENTRY_SIZE > data.len() {
                return Err(error::corrupt_metadata("truncated slab entry"));
            }
            let start_offset = u64::from_le_bytes(data[pos..pos + 8].try_into().unwrap());
            pos += 8;
            let slab_size = u64::from_le_bytes(data[pos..pos + 8].try_into().unwrap());
            pos += 8;
            let element_size = u32::from_le_bytes(data[pos..pos + 4].try_into().unwrap());
            pos += 4;

            slabs.push(SlabDescriptor {
                start_offset,
                slab_size,
                element_size,
            });
        }

        result.push((index, slabs));
    }

    Ok(result)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn checkpoint_header_round_trip() {
        let payload = b"hello world";
        let seq = 42u64;
        let payload_len = payload.len() as u32;

        let mut blob = Vec::new();
        blob.extend_from_slice(&seq.to_le_bytes());
        blob.extend_from_slice(&payload_len.to_le_bytes());
        blob.extend_from_slice(&0u32.to_le_bytes());
        blob.extend_from_slice(payload);

        let crc = crc32fast::hash(&blob);
        blob[12..16].copy_from_slice(&crc.to_le_bytes());

        let recovered_seq = u64::from_le_bytes(blob[0..8].try_into().unwrap());
        let recovered_len = u32::from_le_bytes(blob[8..12].try_into().unwrap());
        let recovered_crc = u32::from_le_bytes(blob[12..16].try_into().unwrap());
        let recovered_payload = &blob[CHECKPOINT_HEADER_SIZE..CHECKPOINT_HEADER_SIZE + recovered_len as usize];

        assert_eq!(recovered_seq, 42);
        assert_eq!(recovered_len, payload.len() as u32);
        assert_eq!(recovered_payload, payload);

        let mut check = blob[..CHECKPOINT_HEADER_SIZE + recovered_len as usize].to_vec();
        check[12..16].copy_from_slice(&0u32.to_le_bytes());
        assert_eq!(recovered_crc, crc32fast::hash(&check));
    }

    #[test]
    fn serialize_deserialize_regions() {
        use crate::buddy::BuddyAllocator;
        use crate::region::RegionState;
        use interfaces::FormatParams;

        let fp = FormatParams {
            data_disk_size: 2 * 1024 * 1024,
            slab_size: 1024 * 1024,
            max_extent_size: 65536,
            sector_size: 4096,
            region_count: 2,
            metadata_alignment: 1048576,
        };

        let mut r0 = RegionState::new(0, BuddyAllocator::new(0, 1024 * 1024, 4096), fp.clone());
        r0.index.insert(
            42,
            Extent {
                key: 42,
                offset: 8192,
                size: 4096,
            },
        );

        let mut r1 = RegionState::new(1, BuddyAllocator::new(1024 * 1024, 1024 * 1024, 4096), fp);
        r1.index.insert(
            99,
            Extent {
                key: 99,
                offset: 12288,
                size: 4096,
            },
        );

        let mut all_data = Vec::new();
        all_data.extend_from_slice(&2u32.to_le_bytes());
        all_data.extend_from_slice(&serialize_region(&r0));
        all_data.extend_from_slice(&serialize_region(&r1));

        let regions = deserialize_index_and_slabs(&all_data).unwrap();
        assert_eq!(regions.len(), 2);
        assert_eq!(regions[0].0.len(), 1);
        assert_eq!(regions[0].0[&42].offset, 8192);
        assert_eq!(regions[1].0.len(), 1);
        assert_eq!(regions[1].0[&99].offset, 12288);
    }
}
