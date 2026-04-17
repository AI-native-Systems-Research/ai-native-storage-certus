mod bitmap;
mod block_device;
mod error;
mod metadata;
mod recovery;
mod state;
mod superblock;

#[cfg(any(test, feature = "testing"))]
pub mod test_support;

use std::sync::{Mutex, RwLock};

use interfaces::{
    DmaAllocFn, ExtentManagerError, IBlockDevice, IExtentManager, IExtentManagerAdmin, ILogger,
    NvmeBlockError, RecoveryResult,
};

use component_macros::define_component;

use crate::bitmap::AllocationBitmap;
use crate::block_device::BlockDeviceClient;
use crate::metadata::{ExtentMetadata, OnDiskExtentRecord, BLOCK_SIZE};
use crate::state::ExtentManagerState;
use crate::superblock::{Superblock, SUPERBLOCK_LBA};

define_component! {
    pub ExtentManagerComponentV1 {
        version: "0.1.0",
        provides: [IExtentManager, IExtentManagerAdmin],
        receptacles: {
            block_device: IBlockDevice,
            logger: ILogger,
        },
        fields: {
            state: RwLock<Option<ExtentManagerState>>,
            dma_alloc: Mutex<Option<DmaAllocFn>>,
        },
    }
}

impl ExtentManagerComponentV1 {
    #[allow(dead_code)]
    pub(crate) fn new_inner() -> std::sync::Arc<Self> {
        ExtentManagerComponentV1::new_default()
    }

    fn get_client(&self) -> Result<BlockDeviceClient, NvmeBlockError> {
        let bd = self
            .block_device
            .get()
            .map_err(|_| NvmeBlockError::NotInitialized("block device not connected".into()))?;
        let channels = bd.connect_client()?;
        let alloc = self
            .dma_alloc
            .lock()
            .unwrap()
            .clone()
            .ok_or_else(|| NvmeBlockError::NotInitialized("DMA allocator not set".into()))?;
        Ok(BlockDeviceClient::new(channels, alloc))
    }

    fn allocate_slab(
        &self,
        state: &mut ExtentManagerState,
        client: &BlockDeviceClient,
        size_class: u32,
    ) -> Result<usize, ExtentManagerError> {
        if !state.can_allocate_slab() {
            return Err(error::out_of_space(size_class));
        }

        let start_lba = state.next_free_lba;
        let slab_size = state.slab_size_blocks;
        let ns_id = state.namespace_id;

        let zero = metadata::zero_block();
        for offset in 0..slab_size as u64 {
            client
                .write_block(ns_id, start_lba + offset, &zero)
                .map_err(error::nvme_to_em)?;
        }

        let slab_idx = state.add_slab_descriptor(size_class, start_lba);

        let bitmap_blocks = state.slabs[slab_idx].bitmap.serialize_to_blocks();
        for (i, block) in bitmap_blocks.iter().enumerate() {
            client
                .write_block(
                    ns_id,
                    state.slabs[slab_idx].bitmap_start_lba + i as u64,
                    block,
                )
                .map_err(error::nvme_to_em)?;
        }

        let mut sb = Superblock::new(state.total_blocks, state.slab_size_blocks, ns_id);
        for s in &state.slabs {
            sb.add_slab(s.size_class, s.start_lba);
        }
        client
            .write_block(ns_id, SUPERBLOCK_LBA, &sb.serialize())
            .map_err(error::nvme_to_em)?;

        Ok(slab_idx)
    }
}

impl IExtentManagerAdmin for ExtentManagerComponentV1 {
    fn set_dma_alloc(&self, alloc: DmaAllocFn) {
        *self.dma_alloc.lock().unwrap() = Some(alloc);
    }

    fn initialize(
        &self,
        total_size_bytes: u64,
        slab_size_bytes: u32,
        ns_id: u32,
    ) -> Result<(), NvmeBlockError> {
        if total_size_bytes == 0 {
            return Err(NvmeBlockError::BlockDevice(
                interfaces::BlockDeviceError::WriteFailed("total_size_bytes must be > 0".into()),
            ));
        }
        if slab_size_bytes < (BLOCK_SIZE as u32 * 2) {
            return Err(NvmeBlockError::BlockDevice(
                interfaces::BlockDeviceError::WriteFailed(format!(
                    "slab_size_bytes must be >= {} (2 blocks)",
                    BLOCK_SIZE * 2,
                )),
            ));
        }
        if slab_size_bytes as u64 % BLOCK_SIZE as u64 != 0 {
            return Err(NvmeBlockError::BlockDevice(
                interfaces::BlockDeviceError::WriteFailed(
                    "slab_size_bytes must be a multiple of block size (4096)".into(),
                ),
            ));
        }

        let total_blocks = total_size_bytes / BLOCK_SIZE as u64;
        let slab_size_blocks = slab_size_bytes / BLOCK_SIZE as u32;

        let client = self.get_client()?;
        let sb = Superblock::new(total_blocks, slab_size_blocks, ns_id);
        client.write_block(ns_id, SUPERBLOCK_LBA, &sb.serialize())?;

        let em_state = ExtentManagerState::new(total_blocks, slab_size_blocks, ns_id);
        *self.state.write().unwrap() = Some(em_state);
        Ok(())
    }

    fn open(&self, ns_id: u32) -> Result<RecoveryResult, NvmeBlockError> {
        let client = self.get_client()?;

        let sb_data = client.read_block(ns_id, SUPERBLOCK_LBA)?;
        let sb = Superblock::deserialize(&sb_data).map_err(|e| {
            NvmeBlockError::BlockDevice(interfaces::BlockDeviceError::ReadFailed(e))
        })?;

        let mut em_state = ExtentManagerState::new(sb.total_blocks, sb.slab_size_blocks, ns_id);
        em_state.next_free_lba = sb.next_free_lba;

        for entry in &sb.slab_table {
            em_state.add_slab_descriptor(entry.size_class, entry.start_lba);
        }

        for slab in &mut em_state.slabs {
            let num_bitmap_blocks = slab.bitmap_blocks as usize;
            let mut bitmap_blocks = Vec::with_capacity(num_bitmap_blocks);
            for j in 0..num_bitmap_blocks {
                let block = client.read_block(ns_id, slab.bitmap_start_lba + j as u64)?;
                bitmap_blocks.push(block);
            }
            slab.bitmap = AllocationBitmap::deserialize_from_blocks(&bitmap_blocks, slab.num_slots);
        }

        let result = recovery::recover(&client, &mut em_state.slabs, ns_id)?;

        for slab in &em_state.slabs {
            for slot in 0..slab.num_slots {
                if slab.bitmap.is_set(slot) {
                    let lba = slab.record_start_lba + slot as u64;
                    let block_data = client.read_block(ns_id, lba)?;
                    let record = OnDiskExtentRecord { data: block_data };
                    if let Some(mut meta) = record.to_metadata() {
                        meta.slab_index = slab.slab_index;
                        em_state.index.insert(meta.key, meta);
                    }
                }
            }
        }

        *self.state.write().unwrap() = Some(em_state);
        Ok(result)
    }
}

impl IExtentManager for ExtentManagerComponentV1 {
    fn create_extent(
        &self,
        key: u64,
        size_class: u32,
        filename: &str,
        data_crc: u32,
        has_crc: bool,
    ) -> Result<Vec<u8>, ExtentManagerError> {
        if !(131072..=5_242_880).contains(&size_class) || size_class % 4096 != 0 {
            return Err(error::invalid_size_class(size_class));
        }

        let mut state_guard = self.state.write().unwrap();
        let state = state_guard
            .as_mut()
            .ok_or_else(|| error::not_initialized("component not initialized"))?;

        if state.index.contains_key(&key) {
            return Err(error::duplicate_key(key));
        }

        let (slab_idx, slot) = match state.find_free_slot(size_class) {
            Some(result) => result,
            None => {
                let client = self.get_client().map_err(error::nvme_to_em)?;
                let new_slab_idx = self.allocate_slab(state, &client, size_class)?;
                let slot = state.slabs[new_slab_idx]
                    .bitmap
                    .find_free()
                    .ok_or_else(|| error::out_of_space(size_class))?;
                (new_slab_idx, slot)
            }
        };

        let slab = &state.slabs[slab_idx];
        let offset_lba = slab.record_start_lba + slot as u64;

        let fname = if filename.is_empty() {
            None
        } else {
            Some(filename.to_string())
        };
        let crc = if has_crc { Some(data_crc) } else { None };

        let meta = ExtentMetadata {
            key,
            size_class,
            namespace_id: state.namespace_id,
            offset_lba,
            filename: fname,
            data_crc: crc,
            slab_index: slab_idx,
        };

        let client = self.get_client().map_err(error::nvme_to_em)?;

        let record = OnDiskExtentRecord::from_metadata(&meta);
        client
            .write_block(state.namespace_id, offset_lba, &record.data)
            .map_err(error::nvme_to_em)?;

        state.slabs[slab_idx].bitmap.set(slot);
        let bitmap_blocks = state.slabs[slab_idx].bitmap.serialize_to_blocks();
        let bitmap_block_idx = (slot as usize) / (BLOCK_SIZE * 8);
        if bitmap_block_idx < bitmap_blocks.len() {
            let bitmap_lba = state.slabs[slab_idx].bitmap_start_lba + bitmap_block_idx as u64;
            client
                .write_block(
                    state.namespace_id,
                    bitmap_lba,
                    &bitmap_blocks[bitmap_block_idx],
                )
                .map_err(error::nvme_to_em)?;
        }

        let serialized = meta.serialize();
        state.index.insert(key, meta);

        Ok(serialized)
    }

    fn remove_extent(&self, key: u64) -> Result<(), ExtentManagerError> {
        let mut state_guard = self.state.write().unwrap();
        let state = state_guard
            .as_mut()
            .ok_or_else(|| error::not_initialized("component not initialized"))?;

        let meta = state
            .index
            .get(&key)
            .ok_or_else(|| error::key_not_found(key))?
            .clone();

        let slab_idx = meta.slab_index;
        let slab = &state.slabs[slab_idx];
        let slot = (meta.offset_lba - slab.record_start_lba) as u32;

        let client = self.get_client().map_err(error::nvme_to_em)?;

        state.slabs[slab_idx].bitmap.clear(slot);
        let bitmap_blocks = state.slabs[slab_idx].bitmap.serialize_to_blocks();
        let bitmap_block_idx = (slot as usize) / (BLOCK_SIZE * 8);
        if bitmap_block_idx < bitmap_blocks.len() {
            let bitmap_lba = state.slabs[slab_idx].bitmap_start_lba + bitmap_block_idx as u64;
            client
                .write_block(
                    state.namespace_id,
                    bitmap_lba,
                    &bitmap_blocks[bitmap_block_idx],
                )
                .map_err(error::nvme_to_em)?;
        }

        let zero = metadata::zero_block();
        client
            .write_block(state.namespace_id, meta.offset_lba, &zero)
            .map_err(error::nvme_to_em)?;

        state.index.remove(&key);
        Ok(())
    }

    fn lookup_extent(&self, key: u64) -> Result<Vec<u8>, ExtentManagerError> {
        let state_guard = self.state.read().unwrap();
        let state = state_guard
            .as_ref()
            .ok_or_else(|| error::not_initialized("component not initialized"))?;

        let meta = state
            .index
            .get(&key)
            .ok_or_else(|| error::key_not_found(key))?;

        Ok(meta.serialize())
    }

    fn extent_count(&self) -> u64 {
        let state_guard = self.state.read().unwrap();
        match state_guard.as_ref() {
            Some(state) => state.total_extents(),
            None => 0,
        }
    }
}
