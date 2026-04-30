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

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Condvar, Mutex};
use std::thread::JoinHandle;

use parking_lot::RwLock;

use interfaces::{
    DmaAllocFn, DmaBuffer, Extent, ExtentKey, ExtentManagerError, FormatParams, IBlockDevice,
    IExtentManager, ILogger, WriteHandle,
};

use component_macros::define_component;

use crate::block_io::BlockDeviceClient;
use crate::buddy::BuddyAllocator;
use crate::region::{RegionState, SharedState};
use crate::slab::FREE_KEY;
use crate::superblock::Superblock;

#[derive(Default)]
struct CheckpointCoalesce {
    completed_seq: u64,
    in_progress: bool,
}

/// Shared state between `ExtentManagerV2` and its background checkpoint thread.
/// The thread holds `Arc<CheckpointTimerState>` (not `Arc<ExtentManagerV2>`),
/// so the component can be dropped independently of the thread's sleep cycle.
#[derive(Default)]
struct CheckpointTimerState {
    interval: Mutex<Option<std::time::Duration>>,
    wake: Condvar,
    shutdown: AtomicBool,
}

impl CheckpointTimerState {
    fn set_interval(&self, interval: Option<std::time::Duration>) {
        *self.interval.lock().unwrap() = interval;
        self.wake.notify_one();
    }

    fn stop(&self) {
        self.shutdown.store(true, Ordering::Relaxed);
        self.wake.notify_one();
    }
}

define_component! {
    pub ExtentManagerV2 {
        version: "0.3.0",
        provides: [IExtentManager],
        receptacles: {
            metadata_device: IBlockDevice,
            logger: ILogger,
        },
        fields: {
            regions: RwLock<Option<Vec<Arc<RwLock<RegionState>>>>>,
            shared: Mutex<Option<SharedState>>,
            checkpoint_coalesce: Mutex<CheckpointCoalesce>,
            checkpoint_done: Condvar,
            dma_alloc: Mutex<Option<DmaAllocFn>>,
            checkpoint_timer_state: Arc<CheckpointTimerState>,
            checkpoint_thread: Mutex<Option<JoinHandle<()>>>,
            metadata_ns_id: Mutex<Option<u32>>,
        },
    }
}

impl ExtentManagerV2 {
    pub fn new_inner() -> Arc<Self> {
        let component = ExtentManagerV2::new_default();

        // Default: automatic checkpoint every 5 minutes.
        component
            .checkpoint_timer_state
            .set_interval(Some(std::time::Duration::from_secs(300)));

        // Start background checkpoint thread.  The thread holds only
        // Arc<CheckpointTimerState> (for sleeping) and Weak<Self> (for
        // doing work), so the component can be dropped without waiting
        // for the thread's sleep to expire.
        let timer = Arc::clone(&component.checkpoint_timer_state);
        let weak = Arc::downgrade(&component);
        let handle = std::thread::spawn(move || {
            loop {
                // Sleep phase — hold only Arc<CheckpointTimerState>.
                // Only proceed to checkpoint if the timer actually expired;
                // a notify_one() from set_interval/stop wakes early and should
                // cause a re-read of the new interval, not a spurious checkpoint.
                let do_checkpoint;
                {
                    let guard = timer.interval.lock().unwrap();
                    if timer.shutdown.load(Ordering::Relaxed) {
                        break;
                    }
                    do_checkpoint = match *guard {
                        Some(dur) => {
                            let (_guard, result) = timer.wake.wait_timeout(guard, dur).unwrap();
                            result.timed_out()
                        }
                        None => {
                            let _guard = timer.wake.wait(guard);
                            false
                        }
                    };
                }

                if timer.shutdown.load(Ordering::Relaxed) {
                    break;
                }
                if !do_checkpoint {
                    continue;
                }

                // Work phase — upgrade weak ref to run the checkpoint.
                match weak.upgrade() {
                    Some(this) => {
                        if let Err(e) = this.checkpoint() {
                            if !matches!(e, ExtentManagerError::NotInitialized(_)) {
                                this.log_error(&format!("background checkpoint failed: {e}"));
                            }
                        }
                    }
                    None => break,
                }
            }
        });
        *component.checkpoint_thread.lock().unwrap() = Some(handle);

        component
    }

    pub fn set_dma_alloc(&self, alloc: DmaAllocFn) {
        *self.dma_alloc.lock().unwrap() = Some(alloc);
    }

    /// Configure which NVMe namespace ID to use for the metadata device.
    ///
    /// Must be called before [`IExtentManager::initialize`] when the metadata
    /// namespace is not namespace 1.  Defaults to 1 if not called.
    pub fn set_metadata_ns_id(&self, ns_id: u32) {
        *self.metadata_ns_id.lock().unwrap() = Some(ns_id);
    }

    fn get_metadata_client(&self, ns_id: u32) -> Result<BlockDeviceClient, ExtentManagerError> {
        let bd = self
            .metadata_device
            .get()
            .map_err(|_| error::not_initialized("metadata block device not connected"))?;

        let channels = bd
            .connect_client()
            .map_err(error::nvme_to_em)?;

        let alloc = self.dma_alloc.lock().unwrap().clone().unwrap_or_else(|| {
            Arc::new(|size, align, numa| {
                DmaBuffer::new(size, align, numa).map_err(|e| e.to_string())
            })
        });

        let sector_size = bd.sector_size(ns_id).map_err(error::nvme_to_em)?;

        Ok(BlockDeviceClient::new(channels, alloc, sector_size, ns_id))
    }

    fn region_for_key(&self, key: ExtentKey) -> Result<Arc<RwLock<RegionState>>, ExtentManagerError> {
        let regions = self.regions.read();
        let regions = regions
            .as_ref()
            .ok_or_else(|| error::not_initialized("component not initialized"))?;
        let idx = key as usize & (regions.len() - 1);
        Ok(Arc::clone(&regions[idx]))
    }

    fn region_for_offset(&self, offset: u64) -> Result<Arc<RwLock<RegionState>>, ExtentManagerError> {
        let regions = self.regions.read();
        let regions = regions
            .as_ref()
            .ok_or_else(|| error::not_initialized("component not initialized"))?;

        let data_disk_size = {
            let shared = self.shared.lock().unwrap();
            shared
                .as_ref()
                .ok_or_else(|| error::not_initialized("component not initialized"))?
                .format_params
                .data_disk_size
        };

        let region_bytes = data_disk_size / regions.len() as u64;
        if region_bytes == 0 {
            return Err(error::not_initialized("region size is zero"));
        }

        let idx = (offset / region_bytes) as usize;
        if idx >= regions.len() {
            return Err(error::offset_not_found(offset));
        }
        Ok(Arc::clone(&regions[idx]))
    }

    pub(crate) fn log_info(&self, msg: &str) {
        if let Ok(logger) = self.logger.get() {
            logger.info(msg);
        }
    }

    pub(crate) fn log_error(&self, msg: &str) {
        if let Ok(logger) = self.logger.get() {
            logger.error(msg);
        }
    }

    pub(crate) fn log_warn(&self, msg: &str) {
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

        let ns_id = {
            let shared = self.shared.lock().unwrap();
            shared.as_ref().map_or(1, |s| s.format_params.metadata_disk_ns_id)
        };
        let metadata_client = self.get_metadata_client(ns_id)?;

        checkpoint::write_checkpoint(&metadata_client, &self.regions, &self.shared)?;

        // Write updated superblock to metadata device
        {
            let shared = self.shared.lock().unwrap();
            let shared = shared.as_ref().unwrap();
            let sb_data = shared.superblock.serialize();
            metadata_client.write_blocks(0, &sb_data)?;
        }
        // Flush the superblock write when volatile write caching is enabled.
        #[cfg(feature = "volatile_write_cache")]
        metadata_client.flush()?;

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

}

impl Drop for ExtentManagerV2 {
    fn drop(&mut self) {
        self.checkpoint_timer_state.stop();
        if let Some(handle) = self.checkpoint_thread.lock().unwrap().take() {
            let _ = handle.join();
        }
    }
}

impl IExtentManager for ExtentManagerV2 {
    fn format(&self, params: FormatParams) -> Result<(), ExtentManagerError> {
        if params.sector_size == 0 {
            return Err(error::corrupt_metadata("sector_size must be > 0"));
        }
        if params.slab_size % params.sector_size as u64 != 0 {
            return Err(error::corrupt_metadata(
                "slab_size must be a multiple of sector_size",
            ));
        }
        if params.max_extent_size as u64 > params.slab_size {
            return Err(error::corrupt_metadata(
                "max_extent_size must be <= slab_size",
            ));
        }
        if params.region_count == 0 || !params.region_count.is_power_of_two() {
            return Err(error::corrupt_metadata(
                "region_count must be a power of two",
            ));
        }

        let data_disk_size = params.data_disk_size;

        // Query metadata device size
        let metadata_bd = self
            .metadata_device
            .get()
            .map_err(|_| error::not_initialized("metadata block device not connected"))?;
        let metadata_disk_size = metadata_bd.num_sectors(params.metadata_disk_ns_id).map_err(error::nvme_to_em)?
            * metadata_bd.sector_size(params.metadata_disk_ns_id).map_err(error::nvme_to_em)? as u64;

        // Compute checkpoint region layout on metadata device
        let alignment = params.metadata_alignment;
        let sb_size = superblock::SUPERBLOCK_SIZE as u64;
        let checkpoint_region_offset = if alignment == 0 {
            sb_size
        } else {
            (sb_size + alignment - 1) / alignment * alignment
        };
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
        let instance_id = match params.instance_id {
            Some(id) => id,
            None => {
                use std::io::Read;
                let mut buf = [0u8; 8];
                std::fs::File::open("/dev/urandom")
                    .and_then(|mut f| f.read_exact(&mut buf))
                    .map(|_| u64::from_le_bytes(buf))
                    .map_err(|e| error::io_error(&format!("failed to generate instance_id: {e}")))?
            }
        };

        let sb = Superblock::new(
            data_disk_size,
            params.sector_size,
            params.slab_size,
            params.max_extent_size,
            params.region_count,
            checkpoint_region_offset,
            checkpoint_region_size,
            instance_id,
            params.metadata_disk_ns_id,
        );

        let metadata_client = self.get_metadata_client(params.metadata_disk_ns_id)?;
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

        let ns_id = self.metadata_ns_id.lock().unwrap().unwrap_or(1);
        let metadata_client = self.get_metadata_client(ns_id)?;
        let (sb, per_region_data) = recovery::recover(&metadata_client, self)?;

        let format_params = FormatParams {
            data_disk_size: sb.data_disk_size,
            slab_size: sb.slab_size,
            max_extent_size: sb.max_extent_size,
            sector_size: sb.sector_size,
            region_count: sb.region_count,
            metadata_alignment: sb.checkpoint_region_offset,
            instance_id: Some(sb.instance_id),
            metadata_disk_ns_id: sb.metadata_disk_ns_id,
        };

        let data_disk_size = sb.data_disk_size;
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
            let slab_descs = if i < per_region_data.len() {
                per_region_data[i].clone()
            } else {
                Vec::new()
            };

            let mut buddy = BuddyAllocator::new(base, size, sb.sector_size);
            for desc in &slab_descs {
                buddy.mark_allocated(desc.start_offset, desc.slab_size);
            }

            let mut region = RegionState::new(i, buddy, format_params.clone());

            for desc in &slab_descs {
                let slab = recovery::slab_from_descriptor(desc);
                let element_size = slab.element_size;
                let start_offset = slab.start_offset;
                region.size_classes.add_slab(element_size, start_offset);
                region.slabs.insert(start_offset, slab);
            }

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

        let (slab_start, slot_idx, offset, aligned_size) = {
            let mut r = region.write();
            let (ss, sli, off) = r.alloc_extent(size)?;
            let bs = r.format_params.sector_size;
            (ss, sli, off, (size + bs - 1) / bs * bs)
        };

        let publish_region = Arc::clone(&region);
        let abort_region = Arc::clone(&region);

        let publish_fn = Box::new(move || {
            let mut r = publish_region.write();

            if key == FREE_KEY {
                // Special sentinel: silently discard — free the slot and return Ok.
                r.free_slot(slab_start, slot_idx);
                return Ok(Extent { key, offset, size: aligned_size });
            }

            r.publish_slot(slab_start, slot_idx, key);
            Ok(Extent { key, offset, size: aligned_size })
        });

        let abort_fn = Box::new(move || {
            let mut r = abort_region.write();
            r.free_slot(slab_start, slot_idx);
        });

        Ok(WriteHandle::new(key, offset, aligned_size, publish_fn, abort_fn))
    }

    fn get_extents(&self) -> Vec<Extent> {
        let regions = self.regions.read();
        match regions.as_ref() {
            Some(regions) => {
                let mut result = Vec::new();
                for region in regions {
                    let r = region.read();
                    for slab in r.slabs.values() {
                        for i in 0..slab.num_slots() as usize {
                            let key = slab.get_key(i);
                            if key != FREE_KEY {
                                result.push(Extent {
                                    key,
                                    offset: slab.slot_offset(i),
                                    size: slab.element_size,
                                });
                            }
                        }
                    }
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
                for slab in r.slabs.values() {
                    for i in 0..slab.num_slots() as usize {
                        let key = slab.get_key(i);
                        if key != FREE_KEY {
                            let extent = Extent {
                                key,
                                offset: slab.slot_offset(i),
                                size: slab.element_size,
                            };
                            cb(&extent);
                        }
                    }
                }
            }
        }
    }

    fn remove_extent(&self, offset: u64) -> Result<(), ExtentManagerError> {
        let region = self.region_for_offset(offset)?;
        let mut r = region.write();
        r.remove_extent_by_offset(offset)
    }

    fn get_instance_id(&self) -> Result<u64, ExtentManagerError> {
        let shared = self.shared.lock().unwrap();
        let shared = shared
            .as_ref()
            .ok_or_else(|| error::not_initialized("component not initialized"))?;
        Ok(shared.superblock.instance_id)
    }

    fn set_checkpoint_interval(&self, interval: Option<std::time::Duration>) {
        self.checkpoint_timer_state.set_interval(interval);
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
