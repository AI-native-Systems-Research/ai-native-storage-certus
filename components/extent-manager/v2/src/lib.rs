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
    IExtentManager, ILogger, WriteHandle,
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
    pub ExtentManagerV2 {
        version: "0.3.0",
        provides: [IExtentManager],
        receptacles: {
            block_device: IBlockDevice,
            metadata_device: IBlockDevice,
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

impl ExtentManagerV2 {
    pub fn new_inner() -> Arc<Self> {
        let component = ExtentManagerV2::new_default();
        component
            .checkpoint_interval_ms
            .store(5000, Ordering::Relaxed);
        component
    }

    pub fn set_checkpoint_interval(&self, interval: std::time::Duration) {
        self.checkpoint_interval_ms
            .store(interval.as_millis() as u64, Ordering::Relaxed);
    }

    fn get_data_client(&self) -> Result<BlockDeviceClient, ExtentManagerError> {
        let bd = self
            .block_device
            .get()
            .map_err(|_| error::not_initialized("data block device not connected"))?;

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

    fn get_metadata_client(&self) -> Result<BlockDeviceClient, ExtentManagerError> {
        let bd = self
            .metadata_device
            .get()
            .map_err(|_| error::not_initialized("metadata block device not connected"))?;

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

        let metadata_client = self.get_metadata_client()?;

        checkpoint::write_checkpoint(&metadata_client, &self.regions, &self.shared)?;

        // Write updated superblock to metadata device
        {
            let shared = self.shared.lock().unwrap();
            let shared = shared.as_ref().unwrap();
            let sb_data = shared.superblock.serialize();
            metadata_client.write_blocks(0, &sb_data)?;
        }

        {
            let regions = self.regions.read();
            if let Some(regions) = regions.as_ref() {
                for region in regions {
                    let mut r = region.write();
                    r.dirty = false;
                    r.flush_pending_frees();
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

impl Drop for ExtentManagerV2 {
    fn drop(&mut self) {
        self.shutdown.store(true, Ordering::Relaxed);
        if let Some(handle) = self.checkpoint_thread.lock().unwrap().take() {
            let _ = handle.join();
        }
    }
}

impl IExtentManager for ExtentManagerV2 {
    fn set_dma_alloc(&self, alloc: DmaAllocFn) {
        *self.dma_alloc.lock().unwrap() = Some(alloc);
    }

    fn format(&self, params: FormatParams) -> Result<(), ExtentManagerError> {
        if params.sector_size == 0 {
            return Err(error::corrupt_metadata("sector_size must be > 0"));
        }
        if params.slab_size % params.sector_size as u64 != 0 {
            return Err(error::corrupt_metadata(
                "slab_size must be a multiple of sector_size",
            ));
        }
        if params.max_element_size as u64 > params.slab_size {
            return Err(error::corrupt_metadata(
                "max_element_size must be <= slab_size",
            ));
        }
        if params.region_count == 0 || !params.region_count.is_power_of_two() {
            return Err(error::corrupt_metadata(
                "region_count must be a power of two",
            ));
        }

        // Query data device size
        let data_bd = self
            .block_device
            .get()
            .map_err(|_| error::not_initialized("data block device not connected"))?;
        let data_disk_size = data_bd.num_sectors(1).map_err(error::nvme_to_em)?
            * data_bd.sector_size(1).map_err(error::nvme_to_em)? as u64;

        // Query metadata device size
        let metadata_bd = self
            .metadata_device
            .get()
            .map_err(|_| error::not_initialized("metadata block device not connected"))?;
        let metadata_disk_size = metadata_bd.num_sectors(1).map_err(error::nvme_to_em)?
            * metadata_bd.sector_size(1).map_err(error::nvme_to_em)? as u64;

        // Compute checkpoint region layout on metadata device
        let checkpoint_region_offset =
            superblock::SUPERBLOCK_SIZE as u64 + params.metadata_padding;
        let remaining = metadata_disk_size.saturating_sub(checkpoint_region_offset);
        let sector_size_u64 = params.sector_size as u64;
        let checkpoint_region_size = (remaining / 2) / sector_size_u64 * sector_size_u64;

        if checkpoint_region_size == 0 {
            return Err(error::corrupt_metadata(
                "metadata device too small for checkpoint regions",
            ));
        }

        // Set up data device regions — entire disk available for user extents
        let region_count = params.region_count as usize;
        let region_bytes = data_disk_size / region_count as u64;

        let mut region_vec = Vec::with_capacity(region_count);
        for i in 0..region_count {
            let base = i as u64 * region_bytes;
            let size = if i < region_count - 1 {
                region_bytes
            } else {
                data_disk_size - (region_count as u64 - 1) * region_bytes
            };
            let buddy = BuddyAllocator::new(base, size, params.sector_size);
            let region = RegionState::new(i, buddy, params.clone());
            region_vec.push(Arc::new(RwLock::new(region)));
        }

        // Write superblock to metadata device
        let sb = Superblock::new(
            data_disk_size,
            params.sector_size,
            params.slab_size,
            params.max_element_size,
            params.region_count,
            checkpoint_region_offset,
            checkpoint_region_size,
        );

        let metadata_client = self.get_metadata_client()?;
        let sb_data = sb.serialize();
        metadata_client.write_blocks(0, &sb_data)?;

        let shared = SharedState {
            format_params: params,
            checkpoint_seq: 0,
            disk_size: data_disk_size,
            superblock: sb,
        };

        *self.regions.write() = Some(region_vec);
        *self.shared.lock().unwrap() = Some(shared);

        self.log_info("format complete");

        Ok(())
    }

    fn initialize(&self) -> Result<(), ExtentManagerError> {
        self.log_info("recovery_start");

        let metadata_client = self.get_metadata_client()?;
        let (sb, per_region_data) = recovery::recover(&metadata_client, self)?;

        let format_params = FormatParams {
            slab_size: sb.slab_size,
            max_element_size: sb.max_element_size,
            sector_size: sb.sector_size,
            region_count: sb.region_count,
            metadata_padding: sb.checkpoint_region_offset
                .saturating_sub(superblock::SUPERBLOCK_SIZE as u64),
        };

        // Query data device size for buddy allocator setup
        let data_bd = self
            .block_device
            .get()
            .map_err(|_| error::not_initialized("data block device not connected"))?;
        let data_disk_size = data_bd.num_sectors(1).map_err(error::nvme_to_em)?
            * data_bd.sector_size(1).map_err(error::nvme_to_em)? as u64;

        let region_count = sb.region_count as usize;
        let region_bytes = data_disk_size / region_count as u64;

        let mut region_vec = Vec::with_capacity(region_count);
        for i in 0..region_count {
            let base = i as u64 * region_bytes;
            let size = if i < region_count - 1 {
                region_bytes
            } else {
                data_disk_size - (region_count as u64 - 1) * region_bytes
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
                buddy.mark_allocated(desc.start_offset, desc.slab_size);
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

            let mut region = RegionState::new(i, buddy, format_params.clone());
            region.index = index;
            region.slabs = slabs;
            region.size_classes = size_classes;

            region_vec.push(Arc::new(RwLock::new(region)));
        }

        let shared = SharedState {
            format_params,
            checkpoint_seq: sb.checkpoint_seq,
            disk_size: data_disk_size,
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
        r.remove_extent(key)
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
