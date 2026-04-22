mod bitmap;
mod block_io;
mod buddy;
pub(crate) mod checkpoint;
mod error;
mod recovery;
pub(crate) mod region;
mod slab;
mod write_handle;

#[cfg(any(test, feature = "testing"))]
pub mod superblock;
#[cfg(not(any(test, feature = "testing")))]
pub(crate) mod superblock;

#[cfg(any(test, feature = "testing"))]
pub mod test_support;

use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Condvar, Mutex};
use std::thread::JoinHandle;

use parking_lot::RwLock;

use interfaces::{
    DmaAllocFn, Extent, ExtentKey, ExtentManagerError, FormatParams, IBlockDevice,
    IExtentManagerV2, ILogger, WriteHandle,
};

use component_macros::define_component;

use crate::block_io::BlockDeviceClient;
use crate::buddy::BuddyAllocator;
use crate::region::{RegionState, SharedState};
use crate::superblock::Superblock;

#[derive(Default)]
struct CheckpointCoalesce {
    completed_seq: u64,
    in_progress: bool,
}

define_component! {
    pub MetadataManagerV2 {
        version: "0.2.0",
        provides: [IExtentManagerV2],
        receptacles: {
            block_device: IBlockDevice,
            logger: ILogger,
        },
        fields: {
            regions: RwLock<Option<Vec<Arc<RwLock<RegionState>>>>>,
            shared: Mutex<Option<SharedState>>,
            checkpoint_coalesce: Mutex<CheckpointCoalesce>,
            checkpoint_done: Condvar,
            dma_alloc: Mutex<Option<DmaAllocFn>>,
            checkpoint_interval_ms: AtomicU64,
            shutdown: Arc<AtomicBool>,
            checkpoint_thread: Mutex<Option<JoinHandle<()>>>,
        },
    }
}

impl MetadataManagerV2 {
    pub fn new_inner() -> Arc<Self> {
        let component = MetadataManagerV2::new_default();
        component
            .checkpoint_interval_ms
            .store(5000, Ordering::Relaxed);
        component
    }

    pub fn set_checkpoint_interval(&self, interval: std::time::Duration) {
        self.checkpoint_interval_ms
            .store(interval.as_millis() as u64, Ordering::Relaxed);
    }

    fn get_client(&self) -> Result<BlockDeviceClient, ExtentManagerError> {
        let bd = self
            .block_device
            .get()
            .map_err(|_| error::not_initialized("block device not connected"))?;

        let channels = bd
            .connect_client()
            .map_err(error::nvme_to_em)?;

        let alloc = self
            .dma_alloc
            .lock()
            .unwrap()
            .clone()
            .ok_or_else(|| error::not_initialized("DMA allocator not set"))?;

        let sector_size = {
            let shared = self.shared.lock().unwrap();
            match shared.as_ref() {
                Some(s) => s.format_params.sector_size,
                None => 4096,
            }
        };

        Ok(BlockDeviceClient::new(channels, alloc, sector_size))
    }

    fn region_for_key(&self, key: ExtentKey) -> Result<Arc<RwLock<RegionState>>, ExtentManagerError> {
        let regions = self.regions.read();
        let regions = regions
            .as_ref()
            .ok_or_else(|| error::not_initialized("component not initialized"))?;
        let idx = key as usize & (regions.len() - 1);
        Ok(Arc::clone(&regions[idx]))
    }

    fn log_info(&self, msg: &str) {
        if let Ok(logger) = self.logger.get() {
            logger.info(msg);
        }
    }

    fn log_error(&self, msg: &str) {
        if let Ok(logger) = self.logger.get() {
            logger.error(msg);
        }
    }

    fn log_warn(&self, msg: &str) {
        if let Ok(logger) = self.logger.get() {
            logger.warn(msg);
        }
    }

    fn run_checkpoint(&self) -> Result<(), ExtentManagerError> {
        let any_dirty = {
            let regions = self.regions.read();
            let regions = regions
                .as_ref()
                .ok_or_else(|| error::not_initialized("component not initialized"))?;
            regions.iter().any(|r| r.read().dirty)
        };

        if !any_dirty {
            return Ok(());
        }

        self.log_info("checkpoint_start");

        let client = self.get_client()?;

        checkpoint::write_checkpoint(&client, &self.regions, &self.shared)?;

        {
            let shared = self.shared.lock().unwrap();
            let shared = shared.as_ref().unwrap();
            let sb_data = shared.superblock.serialize();
            client.write_blocks(0, &sb_data)?;
        }

        {
            let regions = self.regions.read();
            if let Some(regions) = regions.as_ref() {
                for region in regions {
                    region.write().dirty = false;
                }
            }
        }

        self.log_info("checkpoint_complete");

        Ok(())
    }

    fn start_background_checkpoint(self: &Arc<Self>) {
        let this = Arc::clone(self);
        let shutdown = Arc::clone(&self.shutdown);

        let handle = std::thread::spawn(move || {
            loop {
                let interval_ms = this.checkpoint_interval_ms.load(Ordering::Relaxed);
                let duration = std::time::Duration::from_millis(interval_ms);
                std::thread::sleep(duration);

                if shutdown.load(Ordering::Relaxed) {
                    break;
                }

                if let Err(e) = this.checkpoint() {
                    this.log_error(&format!("background checkpoint failed: {e}"));
                }
            }
        });

        *self.checkpoint_thread.lock().unwrap() = Some(handle);
    }
}

impl Drop for MetadataManagerV2 {
    fn drop(&mut self) {
        self.shutdown.store(true, Ordering::Relaxed);
        if let Some(handle) = self.checkpoint_thread.lock().unwrap().take() {
            let _ = handle.join();
        }
    }
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

fn mark_chain_allocated(
    client: &BlockDeviceClient,
    head_lba: u64,
    metadata_block_size: u32,
    sector_size: u32,
    regions: &[Arc<RwLock<RegionState>>],
) {
    let mut current_lba = head_lba;
    while current_lba != 0 {
        let byte_offset = current_lba * sector_size as u64;
        let region_idx = find_region_for_offset(regions, byte_offset);
        regions[region_idx].write().buddy.mark_allocated(byte_offset, metadata_block_size as u64);

        let next = client
            .read_blocks(current_lba, metadata_block_size as usize)
            .ok()
            .and_then(|raw| {
                let magic = u32::from_le_bytes(raw[0..4].try_into().ok()?);
                if magic != checkpoint::CHUNK_MAGIC {
                    return None;
                }
                Some(u64::from_le_bytes(raw[20..28].try_into().ok()?))
            })
            .unwrap_or(0);
        current_lba = next;
    }
}

impl IExtentManagerV2 for MetadataManagerV2 {
    fn set_dma_alloc(&self, alloc: DmaAllocFn) {
        *self.dma_alloc.lock().unwrap() = Some(alloc);
    }

    fn format(&self, params: FormatParams) -> Result<(), ExtentManagerError> {
        if params.sector_size == 0 {
            return Err(error::corrupt_metadata("sector_size must be > 0"));
        }
        if params.slab_size % params.sector_size != 0 {
            return Err(error::corrupt_metadata(
                "slab_size must be a multiple of sector_size",
            ));
        }
        if params.max_element_size > params.slab_size {
            return Err(error::corrupt_metadata(
                "max_element_size must be <= slab_size",
            ));
        }
        if params.metadata_block_size % params.sector_size != 0 {
            return Err(error::corrupt_metadata(
                "metadata_block_size must be a multiple of sector_size",
            ));
        }
        if params.region_count == 0 || !params.region_count.is_power_of_two() {
            return Err(error::corrupt_metadata(
                "region_count must be a power of two",
            ));
        }

        let client = self.get_client()?;

        let bd = self
            .block_device
            .get()
            .map_err(|_| error::not_initialized("block device not connected"))?;
        let disk_size = bd.num_sectors(1).map_err(error::nvme_to_em)?
            * bd.sector_size(1).map_err(error::nvme_to_em)? as u64;

        let region_count = params.region_count as usize;
        let region_bytes = disk_size / region_count as u64;

        let mut region_vec = Vec::with_capacity(region_count);
        for i in 0..region_count {
            let base = i as u64 * region_bytes;
            let size = if i < region_count - 1 {
                region_bytes
            } else {
                disk_size - (region_count as u64 - 1) * region_bytes
            };
            let mut buddy = BuddyAllocator::new(base, size, params.sector_size);

            let metadata_offset = buddy
                .alloc(params.slab_size as u64)
                .ok_or_else(error::out_of_space)?;

            let mut region = RegionState::new(i, buddy, params.clone());
            let metadata_slab =
                crate::slab::Slab::new(metadata_offset, params.slab_size, params.metadata_block_size);
            let slab_idx = region.slabs.len();
            region.slabs.push(metadata_slab);
            region.size_classes.add_slab(params.metadata_block_size, slab_idx);

            if i == 0 {
                region.slabs[slab_idx].mark_slot_allocated(0);
            }

            region_vec.push(Arc::new(RwLock::new(region)));
        }

        let sb = Superblock::new(
            disk_size,
            params.sector_size,
            params.slab_size,
            params.max_element_size,
            params.metadata_block_size,
            params.region_count,
        );

        let sb_data = sb.serialize();
        client.write_blocks(0, &sb_data)?;

        let shared = SharedState {
            format_params: params,
            checkpoint_seq: 0,
            disk_size,
            superblock: sb,
        };

        *self.regions.write() = Some(region_vec);
        *self.shared.lock().unwrap() = Some(shared);

        self.log_info("format complete");

        Ok(())
    }

    fn initialize(&self) -> Result<(), ExtentManagerError> {
        self.log_info("recovery_start");

        let client = self.get_client()?;
        let (sb, per_region_data) = recovery::recover(&client, self)?;

        let format_params = FormatParams {
            slab_size: sb.slab_size,
            max_element_size: sb.max_element_size,
            metadata_block_size: sb.metadata_block_size,
            sector_size: sb.sector_size,
            region_count: sb.region_count,
        };

        let region_count = sb.region_count as usize;
        let region_bytes = sb.disk_size / region_count as u64;

        let mut region_vec = Vec::with_capacity(region_count);
        for i in 0..region_count {
            let base = i as u64 * region_bytes;
            let size = if i < region_count - 1 {
                region_bytes
            } else {
                sb.disk_size - (region_count as u64 - 1) * region_bytes
            };
            let mut buddy = BuddyAllocator::new(base, size, sb.sector_size);

            let (index, slab_descriptors) = if i < per_region_data.len() {
                per_region_data[i].clone()
            } else {
                (std::collections::HashMap::new(), Vec::new())
            };

            let mut slabs = Vec::new();
            let mut size_classes = crate::slab::SizeClassManager::new();

            for desc in &slab_descriptors {
                let slab = crate::slab::Slab::new(
                    desc.start_offset,
                    desc.slab_size,
                    desc.element_size,
                );
                let slab_idx = slabs.len();
                size_classes.add_slab(desc.element_size, slab_idx);
                buddy.mark_allocated(desc.start_offset, desc.slab_size as u64);
                slabs.push(slab);
            }

            for extent in index.values() {
                let aligned_size =
                    (extent.size + sb.sector_size - 1) / sb.sector_size * sb.sector_size;
                for slab in slabs.iter_mut() {
                    if slab.element_size == aligned_size {
                        if let Some(slot_idx) = slab.slot_for_offset(extent.offset) {
                            slab.mark_slot_allocated(slot_idx);
                            break;
                        }
                    }
                }
            }

            if i == 0 {
                for slab in slabs.iter_mut() {
                    if let Some(slot_idx) = slab.slot_for_offset(0) {
                        slab.mark_slot_allocated(slot_idx);
                        break;
                    }
                }
            }

            let mut region = RegionState::new(i, buddy, format_params.clone());
            region.index = index;
            region.slabs = slabs;
            region.size_classes = size_classes;

            region_vec.push(Arc::new(RwLock::new(region)));
        }

        for &chain_lba in &[sb.current_index_lba, sb.previous_index_lba] {
            if chain_lba != 0 {
                mark_chain_allocated(
                    &client,
                    chain_lba,
                    sb.metadata_block_size,
                    sb.sector_size,
                    &region_vec,
                );
            }
        }

        let shared = SharedState {
            format_params,
            checkpoint_seq: sb.checkpoint_seq,
            disk_size: sb.disk_size,
            superblock: sb,
        };

        *self.regions.write() = Some(region_vec);
        *self.shared.lock().unwrap() = Some(shared);

        self.log_info("recovery_complete");

        Ok(())
    }

    fn reserve_extent(
        &self,
        key: ExtentKey,
        size: u32,
    ) -> Result<WriteHandle, ExtentManagerError> {
        let region = self.region_for_key(key)?;

        let (slab_idx, slot_idx, offset, aligned_size) = {
            let mut r = region.write();
            let (si, sli, off) = r.alloc_extent(size)?;
            let bs = r.format_params.sector_size;
            (si, sli, off, (size + bs - 1) / bs * bs)
        };

        let publish_region = Arc::clone(&region);
        let abort_region = Arc::clone(&region);

        let publish_fn = Box::new(move || {
            let mut r = publish_region.write();

            if r.index.contains_key(&key) {
                r.free_slot(slab_idx, slot_idx);
                return Err(error::duplicate_key(key));
            }

            let extent = Extent {
                key,
                offset,
                size: aligned_size,
            };

            r.index.insert(key, extent.clone());
            r.dirty = true;
            Ok(extent)
        });

        let abort_fn = Box::new(move || {
            let mut r = abort_region.write();
            r.free_slot(slab_idx, slot_idx);
        });

        Ok(WriteHandle::new(key, offset, aligned_size, publish_fn, abort_fn))
    }

    fn lookup_extent(&self, key: ExtentKey) -> Result<Extent, ExtentManagerError> {
        let region = self.region_for_key(key)?;
        let r = region.read();
        r.index
            .get(&key)
            .cloned()
            .ok_or_else(|| error::key_not_found(key))
    }

    fn get_extents(&self) -> Vec<Extent> {
        let regions = self.regions.read();
        match regions.as_ref() {
            Some(regions) => {
                let mut result = Vec::new();
                for region in regions {
                    let r = region.read();
                    result.extend(r.index.values().cloned());
                }
                result
            }
            None => Vec::new(),
        }
    }

    fn for_each_extent(&self, cb: &mut dyn FnMut(&Extent)) {
        let regions = self.regions.read();
        if let Some(regions) = regions.as_ref() {
            for region in regions {
                let r = region.read();
                for extent in r.index.values() {
                    cb(extent);
                }
            }
        }
    }

    fn remove_extent(&self, key: ExtentKey) -> Result<(), ExtentManagerError> {
        let region = self.region_for_key(key)?;
        let mut r = region.write();
        let (slab_idx, slot_idx) = r.remove_extent(key)?;
        r.free_slot(slab_idx, slot_idx);
        Ok(())
    }

    fn checkpoint(&self) -> Result<(), ExtentManagerError> {
        let mut state = self.checkpoint_coalesce.lock().unwrap();
        let needed = if state.in_progress {
            state.completed_seq + 2
        } else {
            state.completed_seq + 1
        };

        loop {
            if state.completed_seq >= needed {
                return Ok(());
            }
            if !state.in_progress {
                break;
            }
            state = self.checkpoint_done.wait(state).unwrap();
        }

        state.in_progress = true;
        drop(state);

        let result = self.run_checkpoint();

        let mut state = self.checkpoint_coalesce.lock().unwrap();
        if result.is_ok() {
            state.completed_seq = needed;
        }
        state.in_progress = false;
        self.checkpoint_done.notify_all();
        drop(state);

        result
    }
}
