use std::sync::{Arc, Mutex};

use parking_lot::RwLock;

use interfaces::{Extent, ExtentKey, ExtentManagerError};

use crate::block_io::BlockDeviceClient;
use crate::error;
use crate::region::{RegionState, SharedState};

pub(crate) const CHUNK_MAGIC: u32 = 0x434B_4E4B; // "CKNK"
pub(crate) const CHUNK_HEADER_SIZE: usize = 36;

const INDEX_ENTRY_SIZE: usize = 20; // u64 key + u64 offset + u32 size
const SLAB_ENTRY_SIZE: usize = 20;  // u64 start_offset + u64 slab_size + u32 element_size

#[derive(Debug, Clone)]
pub(crate) struct ChunkHeader {
    pub magic: u32,
    pub seq: u64,
    pub prev_lba: u64,
    pub next_lba: u64,
    pub payload_len: u32,
    pub checksum: u32,
}

impl ChunkHeader {
    pub fn serialize(&self, payload: &[u8]) -> Vec<u8> {
        let mut buf = Vec::with_capacity(CHUNK_HEADER_SIZE + payload.len());

        buf.extend_from_slice(&self.magic.to_le_bytes());
        buf.extend_from_slice(&self.seq.to_le_bytes());
        buf.extend_from_slice(&self.prev_lba.to_le_bytes());
        buf.extend_from_slice(&self.next_lba.to_le_bytes());
        buf.extend_from_slice(&self.payload_len.to_le_bytes());

        let crc_placeholder = 0u32;
        buf.extend_from_slice(&crc_placeholder.to_le_bytes());

        buf.extend_from_slice(payload);

        let crc = crc32fast::hash(&buf);
        buf[32..36].copy_from_slice(&crc.to_le_bytes());

        buf
    }

    pub fn deserialize(buf: &[u8]) -> Result<(Self, &[u8]), ExtentManagerError> {
        if buf.len() < CHUNK_HEADER_SIZE {
            return Err(error::corrupt_metadata("chunk too short"));
        }

        let magic = u32::from_le_bytes(buf[0..4].try_into().unwrap());
        if magic != CHUNK_MAGIC {
            return Err(error::corrupt_metadata(&format!(
                "invalid chunk magic: {magic:#x}"
            )));
        }

        let seq = u64::from_le_bytes(buf[4..12].try_into().unwrap());
        let prev_lba = u64::from_le_bytes(buf[12..20].try_into().unwrap());
        let next_lba = u64::from_le_bytes(buf[20..28].try_into().unwrap());
        let payload_len = u32::from_le_bytes(buf[28..32].try_into().unwrap());
        let stored_crc = u32::from_le_bytes(buf[32..36].try_into().unwrap());

        let crc_end = CHUNK_HEADER_SIZE + payload_len as usize;
        let mut check_buf = buf[..crc_end].to_vec();
        check_buf[32..36].copy_from_slice(&0u32.to_le_bytes());
        let computed_crc = crc32fast::hash(&check_buf);

        if stored_crc != computed_crc {
            return Err(error::corrupt_metadata(&format!(
                "chunk CRC mismatch: stored={stored_crc:#x} computed={computed_crc:#x}"
            )));
        }

        let payload_end = CHUNK_HEADER_SIZE + payload_len as usize;
        if buf.len() < payload_end {
            return Err(error::corrupt_metadata("chunk payload truncated"));
        }

        let header = Self {
            magic,
            seq,
            prev_lba,
            next_lba,
            payload_len,
            checksum: stored_crc,
        };

        Ok((header, &buf[CHUNK_HEADER_SIZE..payload_end]))
    }
}

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

fn region_serialized_size(region: &RegionState) -> usize {
    4 + region.index.len() * INDEX_ENTRY_SIZE + 4 + region.slabs.len() * SLAB_ENTRY_SIZE
}

pub(crate) fn write_checkpoint(
    client: &BlockDeviceClient,
    regions_lock: &RwLock<Option<Vec<Arc<RwLock<RegionState>>>>>,
    shared_mutex: &Mutex<Option<SharedState>>,
) -> Result<(), ExtentManagerError> {
    let regions = regions_lock.read();
    let regions = regions
        .as_ref()
        .ok_or_else(|| error::not_initialized("component not initialized"))?;

    let (metadata_block_size, sector_size) = {
        let r = regions[0].read();
        (r.format_params.metadata_block_size, r.format_params.sector_size)
    };

    let max_payload = metadata_block_size as usize - CHUNK_HEADER_SIZE;
    let region_count = regions.len();

    // Phase 1: Per-region exclusive lock → size + allocate → downgrade → serialize → release
    let mut all_data = Vec::new();
    all_data.extend_from_slice(&(region_count as u32).to_le_bytes());

    let mut chunk_lbas: Vec<u64> = Vec::new();

    for region_arc in regions.iter() {
        let mut region = region_arc.write();

        let needed_bytes = region_serialized_size(&region);
        let allocated_payload_space = chunk_lbas.len() * max_payload;
        let remaining_space = allocated_payload_space.saturating_sub(all_data.len());

        if needed_bytes > remaining_space {
            let deficit = needed_bytes - remaining_space;
            let new_chunks_needed = (deficit + max_payload - 1) / max_payload;

            for _ in 0..new_chunks_needed {
                let abs_offset = region
                    .buddy
                    .alloc(metadata_block_size as u64)
                    .ok_or_else(error::out_of_space)?;
                let lba = abs_offset / sector_size as u64;
                chunk_lbas.push(lba);
            }
        }

        let region = parking_lot::RwLockWriteGuard::downgrade(region);
        let region_data = serialize_region(&region);
        all_data.extend_from_slice(&region_data);
        drop(region);
    }

    // Ensure we have at least one chunk
    if chunk_lbas.is_empty() {
        let region_arc = &regions[regions.len() - 1];
        let mut region = region_arc.write();
        let abs_offset = region
            .buddy
            .alloc(metadata_block_size as u64)
            .ok_or_else(error::out_of_space)?;
        let lba = abs_offset / sector_size as u64;
        chunk_lbas.push(lba);
    }

    // Phase 2: Build chunks with headers and write to disk
    let new_seq = {
        let shared = shared_mutex.lock().unwrap();
        let s = shared.as_ref().unwrap();
        s.checkpoint_seq + 1
    };

    let mut payload_offset = 0;
    for (i, &lba) in chunk_lbas.iter().enumerate() {
        let payload_end = (payload_offset + max_payload).min(all_data.len());
        let payload = &all_data[payload_offset..payload_end];

        let prev_lba = if i == 0 { 0 } else { chunk_lbas[i - 1] };
        let next_lba = if i + 1 < chunk_lbas.len() {
            chunk_lbas[i + 1]
        } else {
            0
        };

        let header = ChunkHeader {
            magic: CHUNK_MAGIC,
            seq: new_seq,
            prev_lba,
            next_lba,
            payload_len: payload.len() as u32,
            checksum: 0,
        };

        let mut chunk_data = header.serialize(payload);
        chunk_data.resize(metadata_block_size as usize, 0);

        client.write_blocks(lba, &chunk_data)?;
        payload_offset = payload_end;
    }

    // Phase 3: Update superblock and free old chain
    let old_previous;
    {
        let mut shared = shared_mutex.lock().unwrap();
        let s = shared.as_mut().unwrap();

        old_previous = s.superblock.previous_index_lba;
        s.superblock.previous_index_lba = s.superblock.current_index_lba;
        s.superblock.current_index_lba = chunk_lbas[0];
        s.superblock.checkpoint_seq = new_seq;
        s.checkpoint_seq = new_seq;
    }

    if old_previous != 0 {
        free_chain_allocations(client, old_previous, metadata_block_size, sector_size, regions);
    }

    Ok(())
}

fn find_region_for_offset(regions: &[Arc<RwLock<RegionState>>], byte_offset: u64) -> usize {
    for (i, region) in regions.iter().enumerate() {
        let r = region.read();
        let base = r.buddy.base_offset();
        let end = base + r.buddy.total_usable_size();
        if byte_offset >= base && byte_offset < end {
            return i;
        }
    }
    0
}

fn free_chain_allocations(
    client: &BlockDeviceClient,
    head_lba: u64,
    metadata_block_size: u32,
    sector_size: u32,
    regions: &[Arc<RwLock<RegionState>>],
) {
    let mut current_lba = head_lba;
    while current_lba != 0 {
        let byte_offset = current_lba * sector_size as u64;

        let next = client
            .read_blocks(current_lba, metadata_block_size as usize)
            .ok()
            .and_then(|raw| {
                let magic = u32::from_le_bytes(raw[0..4].try_into().ok()?);
                if magic != CHUNK_MAGIC {
                    return None;
                }
                Some(u64::from_le_bytes(raw[20..28].try_into().ok()?))
            })
            .unwrap_or(0);

        let region_idx = find_region_for_offset(regions, byte_offset);
        regions[region_idx].write().buddy.free(byte_offset, metadata_block_size as u64);

        current_lba = next;
    }
}

pub(crate) fn read_chunk_chain(
    client: &BlockDeviceClient,
    head_lba: u64,
    expected_seq: u64,
    metadata_block_size: u32,
) -> Result<Vec<u8>, ExtentManagerError> {
    let mut data = Vec::new();
    let mut current_lba = head_lba;

    while current_lba != 0 {
        let raw = client.read_blocks(current_lba, metadata_block_size as usize)?;
        let (header, payload) = ChunkHeader::deserialize(&raw)?;

        if header.seq != expected_seq {
            return Err(error::corrupt_metadata(&format!(
                "chunk seq mismatch: expected={expected_seq} got={}",
                header.seq
            )));
        }

        data.extend_from_slice(payload);
        current_lba = header.next_lba;
    }

    Ok(data)
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
    fn chunk_header_round_trip() {
        let payload = b"hello world";
        let header = ChunkHeader {
            magic: CHUNK_MAGIC,
            seq: 42,
            prev_lba: 0,
            next_lba: 100,
            payload_len: payload.len() as u32,
            checksum: 0,
        };

        let serialized = header.serialize(payload);
        let (recovered, recovered_payload) = ChunkHeader::deserialize(&serialized).unwrap();

        assert_eq!(recovered.magic, CHUNK_MAGIC);
        assert_eq!(recovered.seq, 42);
        assert_eq!(recovered.prev_lba, 0);
        assert_eq!(recovered.next_lba, 100);
        assert_eq!(recovered_payload, payload);
    }

    #[test]
    fn chunk_corrupt_crc() {
        let payload = b"data";
        let header = ChunkHeader {
            magic: CHUNK_MAGIC,
            seq: 1,
            prev_lba: 0,
            next_lba: 0,
            payload_len: payload.len() as u32,
            checksum: 0,
        };

        let mut serialized = header.serialize(payload);
        serialized[CHUNK_HEADER_SIZE] ^= 0xFF;

        let err = ChunkHeader::deserialize(&serialized).unwrap_err();
        assert!(err.to_string().contains("CRC mismatch"));
    }

    #[test]
    fn serialize_deserialize_regions() {
        use crate::buddy::BuddyAllocator;
        use crate::region::RegionState;
        use interfaces::FormatParams;

        let fp = FormatParams {
            slab_size: 1024 * 1024,
            max_element_size: 65536,
            metadata_block_size: 131072,
            sector_size: 4096,
            region_count: 2,
        };

        let mut r0 = RegionState::new(0, BuddyAllocator::new(4096, 1024 * 1024, 4096), fp.clone());
        r0.index.insert(
            42,
            Extent {
                key: 42,
                offset: 8192,
                size: 4096,
            },
        );

        let mut r1 = RegionState::new(1, BuddyAllocator::new(4096 + 1024 * 1024, 1024 * 1024, 4096), fp);
        r1.index.insert(
            99,
            Extent {
                key: 99,
                offset: 12288,
                size: 4096,
            },
        );

        let mut all_data = Vec::new();
        all_data.extend_from_slice(&2u32.to_le_bytes()); // region_count
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
