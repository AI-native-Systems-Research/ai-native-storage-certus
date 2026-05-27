//! Dispatcher component for the Certus storage system.
//!
//! Orchestrates cache operations (populate, lookup, check, remove) using
//! a DRAM memory-tier with LRU eviction and write-through to SSD.
//! Coordinates N data block devices with N extent managers for persistent storage.
//!
//! Provides the [`IDispatcher`] interface with receptacles for
//! [`ILogger`], [`IDispatchMap`], and [`IMemoryTier`].

mod background;
pub mod io_segmenter;
pub mod pipeline;

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use component_framework::define_component;
use interfaces::{
    CacheKey, ClientChannels, Command, Completion, DmaAllocFn, DmaBuffer,
    DispatcherConfig, DispatcherError, FormatParams, GpuStream, IBlockDevice,
    IBlockDeviceAdmin, IDispatchMap, IDispatcher, IExtentManager, IGpuServices, ILogger,
    IMemoryTier, IpcHandle, LookupResult, PciAddress, WriteHandle,
};

use block_device_spdk_nvme::BlockDeviceSpdkNvmeComponent;
use component_core::binding::bind;
use component_core::query_interface;
use extent_manager::ExtentManager;
use spdk_env::ISPDKEnv;

use crate::background::{BackgroundEvictor, BackgroundWriter, EvictorConfig, WriteJob};

/// A pending store awaiting commit or cancel.
///
/// Created by `prepare_store` and consumed by either `commit_store` (writes
/// the buffer to SSD and publishes the extent) or `cancel_store` (drops the
/// handle, which auto-aborts the reservation).
struct PendingWrite {
    /// Extent reservation handle; calling `publish()` commits, dropping aborts.
    write_handle: WriteHandle,
    /// DMA buffer the caller writes data into between prepare and commit.
    buffer: Arc<DmaBuffer>,
    /// Original (unaligned) data size in bytes.
    size: u32,
    /// Index into `data_drives` identifying the target SSD.
    drive_idx: usize,
}

/// Holds one (block-device, extent-manager) pair for a data drive.
#[allow(dead_code)]
struct DataDrive {
    _block_dev: Arc<dyn component_core::IUnknown + Send + Sync>,
    block_dev_admin: Arc<dyn IBlockDeviceAdmin + Send + Sync>,
    block_dev_iface: Arc<dyn IBlockDevice + Send + Sync>,
    extent_mgr: Arc<ExtentManager>,
    cached_channels: Option<ClientChannels>,
}

// ===== EVOLVE-BLOCK: COMPONENT_FIELDS =====
// The component fields define the concurrency model. Currently ALL mutable
// state is behind Mutex, meaning concurrent clients serialize on every operation.
// Evolution opportunities:
// - Replace Mutex<Vec<DataDrive>> with per-drive sharding (no contention across drives)
// - Replace Mutex<Option<PipelineRing>> with per-client or per-drive pipeline rings
// - Use RwLock for read-heavy paths (lookup is read-heavy, populate is write)
// - Use lock-free data structures (crossbeam, arc-swap) for hot-path fields
// - Shard pending_writes by key range
//
// CRITICAL CONSTRAINT: define_component! macro only supports Mutex<T> field syntax.
// To use RwLock/AtomicPtr/etc, wrap in a newtype or use Arc<RwLock<T>> inside Mutex<Option<...>>.
// Alternative: add auxiliary fields outside the macro using impl blocks.

define_component! {
    pub DispatcherComponent {
        version: "0.1.0",
        provides: [IDispatcher],
        receptacles: {
            logger: ILogger,
            dispatch_map: IDispatchMap,
            gpu_services: IGpuServices,
            spdk_env: ISPDKEnv,
            memory_tier: IMemoryTier,
        },
        fields: {
            initialized: AtomicBool,
            bg_writer: Mutex<Option<BackgroundWriter>>,
            bg_evictor: Mutex<Option<BackgroundEvictor>>,
            data_drives: Mutex<Vec<DataDrive>>,
            pending_writes: Mutex<HashMap<CacheKey, PendingWrite>>,
            pipeline_ring: Mutex<Option<pipeline::PipelineRing>>,
            warm_stream: AtomicU64,
        },
    }
}
// ===== END EVOLVE-BLOCK: COMPONENT_FIELDS =====

unsafe extern "C" fn libc_free(ptr: *mut std::ffi::c_void) {
    unsafe { libc::free(ptr) };
}

/// No-op free function for temporary DmaBuffer wrappers around memory-tier pointers.
/// The memory-tier component owns the memory; this wrapper must not free it.
unsafe extern "C" fn noop_free(_ptr: *mut std::ffi::c_void) {}

impl DispatcherComponent {
    fn log_info(&self, msg: &str) {
        if let Ok(logger) = self.logger.get() {
            logger.info(msg);
        }
    }

    #[allow(dead_code)]
    fn log_error(&self, msg: &str) {
        if let Ok(logger) = self.logger.get() {
            logger.error(msg);
        }
    }

    fn drive_index(key: CacheKey, num_drives: usize) -> usize {
        key as usize % num_drives
    }

    fn ensure_initialized(&self) -> Result<(), DispatcherError> {
        if !self.initialized.load(Ordering::Acquire) {
            return Err(DispatcherError::NotInitialized(
                "dispatcher not initialized".into(),
            ));
        }
        Ok(())
    }

    /// Write `buffer` contents to SSD using MDTS-aware segmented I/O.
    ///
    /// Splits the write into segments that respect the drive's maximum transfer
    /// size, allocates per-segment DMA buffers, and issues synchronous writes.
    fn write_buffer_to_ssd(
        drive: &dyn IBlockDevice,
        buffer: &DmaBuffer,
        start_lba: u64,
        total_bytes: usize,
    ) -> Result<(), DispatcherError> {
        let block_size = drive.block_size() as usize;
        let max_transfer = drive.max_transfer_size();
        let numa_node = drive.numa_node();
        let aligned_bytes = total_bytes.next_multiple_of(block_size);

        let channels = drive.connect_client().map_err(|e| {
            DispatcherError::IoError(format!("connect_client failed: {e}"))
        })?;

        let segments =
            io_segmenter::segment_io(start_lba, aligned_bytes, max_transfer, block_size as u32);

        for seg in &segments {
            let seg_buf = DmaBuffer::new(seg.length, block_size, Some(numa_node)).map_err(
                |e| DispatcherError::AllocationFailed(format!("DMA segment buffer: {e}")),
            )?;

            let copy_len = seg.length.min(total_bytes.saturating_sub(seg.buffer_offset));
            if copy_len > 0 {
                unsafe {
                    std::ptr::copy_nonoverlapping(
                        (buffer.as_ptr() as *const u8).add(seg.buffer_offset),
                        seg_buf.as_ptr() as *mut u8,
                        copy_len,
                    );
                }
            }

            let seg_buf = Arc::new(seg_buf);
            channels
                .command_tx
                .send(Command::WriteSync {
                    ns_id: 1,
                    lba: seg.lba,
                    buf: seg_buf,
                })
                .map_err(|_| DispatcherError::IoError("send WriteSync failed".into()))?;

            match channels.completion_rx.recv() {
                Ok(Completion::WriteDone { result, .. }) => {
                    result.map_err(|e| {
                        DispatcherError::IoError(format!("SSD write failed: {e}"))
                    })?;
                }
                Ok(other) => {
                    return Err(DispatcherError::IoError(format!(
                        "unexpected completion: {other:?}"
                    )));
                }
                Err(_) => {
                    return Err(DispatcherError::IoError(
                        "completion channel disconnected".into(),
                    ));
                }
            }
        }
        Ok(())
    }


    // ===== EVOLVE-BLOCK: PROMOTE_AND_SERVE =====
    // This is the COLD LOOKUP path — the most performance-critical for multi-client.
    // BOTTLENECK: self.pipeline_ring.lock() serializes ALL concurrent promote calls.
    // With 8 clients, 7 block on the mutex while 1 runs the pipeline.
    //
    // Evolution opportunities:
    // - Per-drive pipeline rings (shard by drive_index, no cross-drive contention)
    // - Lock-free pipeline ring pool (try_lock + fallback to per-call allocation)
    // - Remove pipeline_ring dependency entirely (inline stream creation per call)
    // - Use data_drives read-lock (RwLock) since promote doesn't modify the vec
    // - Overlap: start GPU DMA before all NVMe reads complete (streaming pipeline)
    //
    // The drives lock and ring lock are held SIMULTANEOUSLY during the pipeline call,
    // creating a serial bottleneck proportional to object size.

    /// Promote an SSD-resident entry back into the memory-tier and serve to GPU.
    ///
    /// Uses pipelined chunked reads: SSD→DRAM (memory-tier) while streaming
    /// chunks from DRAM→GPU.
    fn promote_and_serve(
        &self,
        key: CacheKey,
        offset: u64,
        ipc_handle: &IpcHandle,
        gpu: &Arc<dyn IGpuServices + Send + Sync>,
        dm: &Arc<dyn IDispatchMap + Send + Sync>,
        mt: &Arc<dyn IMemoryTier + Send + Sync>,
    ) -> Result<(), DispatcherError> {
        let total_bytes = ipc_handle.size as usize;

        // Evict if needed to make space.
        Self::evict_for_space(dm, mt, ipc_handle.size)?;

        // Insert into memory-tier.
        let mem_ptr = mt.insert(key, ipc_handle.size).map_err(|e| {
            DispatcherError::AllocationFailed(format!("promote insert failed: {e}"))
        })?;

        // Read from SSD into memory-tier using pipelined reader.
        let drives = self.data_drives.lock().unwrap();
        if drives.is_empty() {
            // No hardware: just copy zeros to GPU (test/staging-only mode).
            let aligned = total_bytes.next_multiple_of(4096).max(4096);
            let temp_buf = unsafe {
                DmaBuffer::from_raw(mem_ptr as *mut std::ffi::c_void, aligned, noop_free, -1)
            }
            .map_err(|e| DispatcherError::IoError(format!("DmaBuffer wrap failed: {e}")))?;
            let result = gpu.dma_copy_to_device(
                &temp_buf,
                ipc_handle.address as *mut std::ffi::c_void,
                total_bytes,
            );
            std::mem::forget(temp_buf);
            // Register promoted entry in dispatch-map.
            let _ = dm.create_memory_tier_entry(key, mem_ptr, ipc_handle.size);
            let _ = dm.release_write(key);
            return result.map_err(|e| {
                DispatcherError::IoError(format!("GPU DMA copy (promote) failed: {e}"))
            });
        }

        let idx = Self::drive_index(key, drives.len());
        let drive = &drives[idx];
        let block_size = drive.block_dev_iface.block_size();
        let start_lba = offset / block_size as u64;
        let block_dev = Arc::clone(&drive.block_dev_iface);

        // Use cached channels if available, otherwise create new ones.
        let channels = match &drive.cached_channels {
            Some(ch) => ch,
            None => {
                drop(drives);
                return Err(DispatcherError::IoError(
                    "no cached channels for drive".into(),
                ));
            }
        };

        // Zero-copy pipelined reader: NVMe → memory-tier slot → GPU (no intermediate ring copy).
        // SAFETY: mem_ptr is a valid, CUDA-pinned, SPDK-registered memory-tier slot.
        // ipc_handle.address is a valid GPU destination pointer.
        let ring_guard = self.pipeline_ring.lock().unwrap();
        let ring_ref = ring_guard.as_ref().ok_or_else(|| {
            DispatcherError::NotInitialized("pipeline ring not allocated".into())
        })?;
        unsafe {
            pipeline::pipelined_ssd_to_gpu_zero_copy(
                &*block_dev,
                &**gpu,
                &ring_ref.streams,
                channels,
                mem_ptr,
                ipc_handle.address as *mut std::ffi::c_void,
                start_lba,
                total_bytes,
                ring_ref.chunk_size,
            )?;
        }
        drop(ring_guard);
        drop(drives);
    // ===== END EVOLVE-BLOCK: PROMOTE_AND_SERVE =====

        // Update dispatch-map: remove old BlockDevice entry and create fresh MemoryTier.
        // Since we released the read ref before calling this method, we can remove
        // and re-register.
        let _ = dm.remove(key);
        dm.create_memory_tier_entry(key, mem_ptr, ipc_handle.size)
            .map_err(|e| DispatcherError::IoError(format!("promote re-register failed: {e}")))?;
        // Set the ssd_offset since data is still on SSD.
        let _ = dm.convert_to_storage(key, offset);
        let _ = dm.release_write(key);

        Ok(())
    }

    /// Evict entries from the memory-tier until enough space is available.
    ///
    /// Each evicted entry must have completed write-through (ssd_offset set).
    /// The dispatch-map entry transitions from MemoryTier to BlockDevice.
    fn evict_for_space(
        dm: &Arc<dyn IDispatchMap + Send + Sync>,
        mt: &Arc<dyn IMemoryTier + Send + Sync>,
        needed: u32,
    ) -> Result<(), DispatcherError> {
        while mt.used() + needed as usize > mt.capacity() {
            let evicted_key = mt.evict_lru().ok_or_else(|| {
                DispatcherError::AllocationFailed(
                    "memory-tier full and nothing evictable".into(),
                )
            })?;
            // Transition dispatch-map entry to BlockDevice.
            // If write-through hasn't completed, this fails and we lose the entry
            // from the memory-tier (acceptable: it's still tracked in dispatch-map
            // as MemoryTier with no ssd_offset, meaning a re-read won't find it
            // in memory-tier and will go to SSD). In practice, heavy eviction
            // pressure with unfinished writes is unlikely.
            let _ = dm.convert_memory_tier_to_block(evicted_key);
        }
        Ok(())
    }

    fn process_write_job(
        dm: &Arc<dyn IDispatchMap + Send + Sync>,
        mt: &Arc<dyn IMemoryTier + Send + Sync>,
        drives: &[Arc<dyn IBlockDevice + Send + Sync>],
        extent_mgrs: &[Arc<ExtentManager>],
        job: WriteJob,
    ) {
        // Get the memory-tier pointer for this key.
        let (mem_ptr, _size) = match mt.get(job.key) {
            Some(v) => v,
            None => return, // entry was removed before write-through
        };

        if drives.is_empty() {
            // No block devices: mark as converted with a synthetic offset.
            let block_offset = job.key * 4096;
            let _ = dm.convert_to_storage(job.key, block_offset);
            let _ = dm.release_read(job.key);
            return;
        }

        let drive_idx = job.device_index % drives.len();
        let drive = &drives[drive_idx];
        let block_size = drive.block_size() as usize;
        let total_bytes = job.size as usize;
        let aligned_bytes = total_bytes.next_multiple_of(block_size);

        // Wrap memory-tier pointer as a temporary DmaBuffer (noop free).
        // SAFETY: mem_ptr is valid for at least `aligned_bytes` and owned by memory-tier.
        let temp_buf = match unsafe {
            DmaBuffer::from_raw(
                mem_ptr as *mut std::ffi::c_void,
                aligned_bytes,
                noop_free,
                -1,
            )
        } {
            Ok(buf) => buf,
            Err(_) => return,
        };

        // Allocate extent via the extent manager.
        let em = &extent_mgrs[drive_idx % extent_mgrs.len()];
        let iem = match query_interface!(em, IExtentManager) {
            Some(i) => i,
            None => return,
        };
        let write_handle = match iem.reserve_extent(job.key, aligned_bytes as u32) {
            Ok(wh) => wh,
            Err(_) => return,
        };

        let block_offset = write_handle.extent_offset();
        let start_lba = block_offset / block_size as u64;

        if Self::write_buffer_to_ssd(&**drive, &temp_buf, start_lba, total_bytes).is_err() {
            return; // write_handle drops → abort
        }

        // Prevent the noop-free DmaBuffer from being dropped normally.
        std::mem::forget(temp_buf);

        // Data written successfully — commit the extent metadata.
        let _ = write_handle.publish();
        let _ = dm.convert_to_storage(job.key, block_offset);
        let _ = dm.release_read(job.key);
    }
}

impl DispatcherComponent {
    fn parse_pci_addr(s: &str) -> Result<PciAddress, DispatcherError> {
        let parts: Vec<&str> = s.split(':').collect();
        if parts.len() != 3 {
            return Err(DispatcherError::InvalidParameter(format!(
                "invalid PCI address format: {s}"
            )));
        }
        let domain = u32::from_str_radix(parts[0], 16).map_err(|_| {
            DispatcherError::InvalidParameter(format!("invalid PCI domain: {}", parts[0]))
        })?;
        let bus = u8::from_str_radix(parts[1], 16).map_err(|_| {
            DispatcherError::InvalidParameter(format!("invalid PCI bus: {}", parts[1]))
        })?;
        let dev_func: Vec<&str> = parts[2].split('.').collect();
        if dev_func.len() != 2 {
            return Err(DispatcherError::InvalidParameter(format!(
                "invalid PCI dev.func: {}",
                parts[2]
            )));
        }
        let dev = u8::from_str_radix(dev_func[0], 16).map_err(|_| {
            DispatcherError::InvalidParameter(format!("invalid PCI dev: {}", dev_func[0]))
        })?;
        let func = u8::from_str_radix(dev_func[1], 16).map_err(|_| {
            DispatcherError::InvalidParameter(format!("invalid PCI func: {}", dev_func[1]))
        })?;
        Ok(PciAddress {
            domain,
            bus,
            dev,
            func,
        })
    }

    #[allow(clippy::type_complexity)]
    fn create_block_device(
        &self,
        i: usize,
        spdk_env: &Arc<dyn ISPDKEnv + Send + Sync>,
        logger: &Arc<dyn ILogger + Send + Sync>,
        pci_addr: PciAddress,
        addr_str: &str,
    ) -> Result<
        (
            Arc<dyn component_core::IUnknown + Send + Sync>,
            Arc<dyn IBlockDeviceAdmin + Send + Sync>,
            Arc<dyn IBlockDevice + Send + Sync>,
        ),
        DispatcherError,
    > {
        let block_dev = BlockDeviceSpdkNvmeComponent::new_default();
        block_dev
            .spdk_env
            .connect(Arc::clone(spdk_env))
            .map_err(|e| {
                DispatcherError::IoError(format!(
                    "failed to wire spdk_env for data drive {i}: {e}"
                ))
            })?;
        block_dev
            .logger
            .connect(Arc::clone(logger))
            .map_err(|e| {
                DispatcherError::IoError(format!(
                    "failed to wire logger for data drive {i}: {e}"
                ))
            })?;
        let admin = query_interface!(block_dev, IBlockDeviceAdmin).ok_or_else(|| {
            DispatcherError::IoError(format!(
                "failed to query IBlockDeviceAdmin for data drive {i}"
            ))
        })?;
        admin.set_pci_address(pci_addr);
        admin.initialize().map_err(|e| {
            DispatcherError::IoError(format!(
                "failed to initialize block device at {addr_str}: {e}"
            ))
        })?;
        let ibd = query_interface!(block_dev, IBlockDevice).ok_or_else(|| {
            DispatcherError::IoError(format!(
                "failed to query IBlockDevice for data drive {i}"
            ))
        })?;
        Ok((block_dev as Arc<dyn component_core::IUnknown + Send + Sync>, admin, ibd))
    }

    fn create_data_drives(&self, config: &DispatcherConfig) -> Result<Vec<DataDrive>, DispatcherError> {
        let spdk_env = self
            .spdk_env
            .get()
            .map_err(|_| DispatcherError::NotInitialized("spdk_env not bound".into()))?;

        let logger = self
            .logger
            .get()
            .map_err(|_| DispatcherError::NotInitialized("logger not bound".into()))?;

        let mut drives = Vec::with_capacity(config.data_pci_addrs.len());

        for (i, addr_str) in config.data_pci_addrs.iter().enumerate() {
            let pci_addr = Self::parse_pci_addr(addr_str)?;

            let (block_dev_component, admin, ibd) = self.create_block_device(
                i,
                &spdk_env,
                &logger,
                pci_addr,
                addr_str,
            )?;

            let extent_mgr = ExtentManager::new_inner();

            let numa_node = ibd.numa_node();
            let dma_alloc: DmaAllocFn = Arc::new(move |size, align, _numa| {
                DmaBuffer::new(size, align, Some(numa_node)).map_err(|e| e.to_string())
            });
            extent_mgr.set_dma_alloc(dma_alloc);

            extent_mgr
                .logger
                .connect(Arc::clone(&logger) as Arc<dyn ILogger + Send + Sync>)
                .map_err(|e| {
                    DispatcherError::IoError(format!(
                        "failed to wire logger for extent manager {i}: {e}"
                    ))
                })?;

            bind(
                &*block_dev_component,
                "IBlockDevice",
                &*extent_mgr as &dyn component_core::IUnknown,
                "metadata_device",
            )
            .map_err(|e| {
                DispatcherError::IoError(format!(
                    "failed to bind block device to extent manager {i}: {e}"
                ))
            })?;

            let iem = query_interface!(extent_mgr, IExtentManager).ok_or_else(|| {
                DispatcherError::IoError(format!(
                    "failed to query IExtentManager for data drive {i}"
                ))
            })?;
            let sector_size = ibd.block_size();
            let num_sectors = ibd.num_sectors(1).unwrap_or(0);
            let data_disk_size = num_sectors * sector_size as u64;
            let defaults = FormatParams::default();
            let region_size = data_disk_size / defaults.region_count as u64;
            // Slab must fit within a buddy-allocated region. Use 1/16 of region
            // (rounded to a power-of-2 in blocks) to allow many size classes.
            let blocks_in_region = region_size / sector_size as u64;
            let target_slab_blocks = blocks_in_region / 16;
            let slab_size = if target_slab_blocks > 0 {
                let pow2 = 1u64 << (63 - target_slab_blocks.leading_zeros());
                (pow2 * sector_size as u64).min(defaults.slab_size)
            } else {
                defaults.slab_size
            };
            let max_extent_size = (slab_size.min(defaults.max_extent_size as u64)) as u32;
            if config.format_on_init {
                iem.format(FormatParams {
                    data_disk_size,
                    sector_size,
                    slab_size,
                    max_extent_size,
                    ..defaults
                })
                .map_err(|e| {
                    DispatcherError::IoError(format!(
                        "failed to format extent manager for data drive {i}: {e}"
                    ))
                })?;
            }

            self.log_info(&format!(
                "dispatcher: data drive {i} initialized at {addr_str}"
            ));

            let cached_channels = ibd.connect_client().ok();

            drives.push(DataDrive {
                _block_dev: block_dev_component,
                block_dev_admin: admin,
                block_dev_iface: ibd,
                extent_mgr,
                cached_channels,
            });
        }

        Ok(drives)
    }
}

impl IDispatcher for DispatcherComponent {
    fn initialize(&self, config: DispatcherConfig) -> Result<(), DispatcherError> {
        self.log_info("dispatcher: initializing");

        self.dispatch_map
            .get()
            .map_err(|_| DispatcherError::NotInitialized("dispatch_map not bound".into()))?;

        self.memory_tier
            .get()
            .map_err(|_| DispatcherError::NotInitialized("memory_tier not bound".into()))?;

        if config.data_pci_addrs.is_empty() {
            return Err(DispatcherError::InvalidParameter(
                "data_pci_addrs must not be empty".into(),
            ));
        }

        // Create N block devices and N extent managers from config.
        // If spdk_env is not connected, skip drive creation (memory-tier-only mode).
        if self.spdk_env.is_connected() {
            let drives = self.create_data_drives(&config)?;
            *self.data_drives.lock().unwrap() = drives;

            // ===== EVOLVE-BLOCK: PIPELINE_INIT =====
            // Currently allocates ONE shared pipeline ring for ALL clients.
            // This creates the serialization bottleneck in promote_and_serve.
            //
            // Evolution opportunities:
            // - Allocate N pipeline rings (one per drive, or one per expected client)
            // - Allocate a pool of rings that clients can check out lock-free
            // - Create per-client warm streams (currently one shared warm_stream)
            // - Pre-allocate more CUDA streams for parallel cold lookups

            // Pre-allocate pipeline ring for promote_and_serve (CUDA-pinned + SPDK-registered).
            if let Ok(gpu) = self.gpu_services.get() {
                let chunk_size = self
                    .data_drives
                    .lock()
                    .unwrap()
                    .first()
                    .map(|d| d.block_dev_iface.max_transfer_size() as usize)
                    .unwrap_or(131072);
                match pipeline::PipelineRing::new(&*gpu, chunk_size) {
                    Ok(ring) => {
                        *self.pipeline_ring.lock().unwrap() = Some(ring);
                    }
                    Err(e) => {
                        self.log_info(&format!(
                            "pipeline ring allocation failed (non-fatal): {e:?}"
                        ));
                    }
                }

                // Dedicated CUDA stream for warm-path DMA (avoids pipeline_ring lock).
                match gpu.create_stream() {
                    Ok(stream) => {
                        self.warm_stream.store(stream.0 as u64, Ordering::Release);
                    }
                    Err(e) => {
                        self.log_info(&format!(
                            "warm stream allocation failed (non-fatal): {e}"
                        ));
                    }
                }
            // ===== END EVOLVE-BLOCK: PIPELINE_INIT =====

                // Register memory-tier pool as CUDA-pinned + SPDK DMA-capable
                // for zero-copy NVMe reads and async GPU transfers.
                if let Ok(mt) = self.memory_tier.get() {
                    if let Some((pool_ptr, pool_size)) = mt.pool_info() {
                        match gpu.register_host_memory(
                            pool_ptr as *mut std::ffi::c_void,
                            pool_size,
                        ) {
                            Ok(()) => {
                                self.log_info(&format!(
                                    "dispatcher: registered memory-tier pool ({} MiB) for zero-copy DMA",
                                    pool_size / (1024 * 1024)
                                ));
                            }
                            Err(e) => {
                                self.log_info(&format!(
                                    "memory-tier pool registration failed (non-fatal): {e}"
                                ));
                            }
                        }
                    }
                }
            }
        }

        let dm_for_writer = self
            .dispatch_map
            .get()
            .map_err(|_| DispatcherError::NotInitialized("dispatch_map not bound".into()))?;

        let mt_for_writer = self
            .memory_tier
            .get()
            .map_err(|_| DispatcherError::NotInitialized("memory_tier not bound".into()))?;

        // Collect block device interfaces and extent managers for the background writer.
        let bg_drives: Vec<Arc<dyn IBlockDevice + Send + Sync>> = self
            .data_drives
            .lock()
            .unwrap()
            .iter()
            .map(|d| Arc::clone(&d.block_dev_iface))
            .collect();
        let bg_extent_mgrs: Vec<Arc<ExtentManager>> = self
            .data_drives
            .lock()
            .unwrap()
            .iter()
            .map(|d| Arc::clone(&d.extent_mgr))
            .collect();

        let writer = BackgroundWriter::start(move |job: WriteJob| {
            Self::process_write_job(&dm_for_writer, &mt_for_writer, &bg_drives, &bg_extent_mgrs, job);
        });

        *self.bg_writer.lock().unwrap() = Some(writer);

        // Start background SSD evictor if drives exist and threshold is configured.
        if config.ssd_eviction_threshold > 0.0 {
            let dm_for_evictor = self
                .dispatch_map
                .get()
                .map_err(|_| DispatcherError::NotInitialized("dispatch_map not bound".into()))?;
            let mt_for_evictor = self
                .memory_tier
                .get()
                .map_err(|_| DispatcherError::NotInitialized("memory_tier not bound".into()))?;
            let evictor_extent_mgrs: Vec<Arc<ExtentManager>> = self
                .data_drives
                .lock()
                .unwrap()
                .iter()
                .map(|d| Arc::clone(&d.extent_mgr))
                .collect();
            let evictor_logger = self.logger.get().ok();

            if !evictor_extent_mgrs.is_empty() {
                let evictor = BackgroundEvictor::start(
                    dm_for_evictor,
                    mt_for_evictor,
                    evictor_extent_mgrs,
                    EvictorConfig {
                        threshold: config.ssd_eviction_threshold,
                        low_watermark: config.ssd_eviction_low_watermark,
                        batch_size: config.ssd_eviction_batch_size,
                        interval: std::time::Duration::from_secs(config.ssd_eviction_interval_secs),
                    },
                    evictor_logger,
                );
                *self.bg_evictor.lock().unwrap() = Some(evictor);
            }
        }

        self.initialized.store(true, Ordering::Release);

        self.log_info("dispatcher: initialized");
        Ok(())
    }

    fn shutdown(&self) -> Result<(), DispatcherError> {
        self.log_info("dispatcher: shutting down");

        if let Some(mut evictor) = self.bg_evictor.lock().unwrap().take() {
            evictor.shutdown();
        }

        if let Some(mut writer) = self.bg_writer.lock().unwrap().take() {
            writer.shutdown();
        }

        self.pending_writes.lock().unwrap().clear();

        // Unregister memory-tier pool from CUDA/SPDK before tearing down.
        if let (Ok(gpu), Ok(mt)) = (self.gpu_services.get(), self.memory_tier.get()) {
            if let Some((pool_ptr, pool_size)) = mt.pool_info() {
                let _ = gpu.unregister_host_memory(
                    pool_ptr as *mut std::ffi::c_void,
                    pool_size,
                );
            }
        }

        // Destroy warm stream and pipeline ring.
        if let Ok(gpu) = self.gpu_services.get() {
            let raw = self.warm_stream.swap(0, Ordering::AcqRel);
            if raw != 0 {
                let _ = gpu.destroy_stream(GpuStream(raw as *mut std::ffi::c_void));
            }
            if let Some(ring) = self.pipeline_ring.lock().unwrap().take() {
                ring.destroy(&*gpu);
            }
        }

        // Shut down block devices in reverse order
        let drives = std::mem::take(&mut *self.data_drives.lock().unwrap());
        for (i, drive) in drives.iter().enumerate().rev() {
            if let Err(e) = drive.block_dev_admin.shutdown() {
                self.log_error(&format!(
                    "dispatcher: failed to shut down data drive {i}: {e}"
                ));
            }
        }

        self.initialized.store(false, Ordering::Release);
        self.log_info("dispatcher: shut down");
        Ok(())
    }

    fn lookup(&self, key: CacheKey, ipc_handle: IpcHandle) -> Result<(), DispatcherError> {
        let stream = self.lookup_async(key, ipc_handle)?;
        if !stream.0.is_null() {
            let gpu = self
                .gpu_services
                .get()
                .map_err(|_| DispatcherError::NotInitialized("gpu_services not bound".into()))?;
            gpu.stream_synchronize(stream).map_err(|e| {
                DispatcherError::IoError(format!("stream_synchronize failed: {e}"))
            })?;
        }
        Ok(())
    }

    // ===== EVOLVE-BLOCK: LOOKUP_ASYNC =====
    // This is the HOT PATH for multi-client concurrent throughput.
    // Currently lock-free for warm hits (AtomicU64 warm_stream) but serialized
    // for cold hits (calls promote_and_serve which locks pipeline_ring + data_drives).
    //
    // Multi-client bottleneck analysis:
    // - Warm path: FAST (atomic load, async memcpy) — no contention
    // - Cold path: SERIALIZED (promote_and_serve holds pipeline_ring mutex)
    //   With 8 clients all doing cold lookups, only 1 runs at a time.
    //
    // Evolution opportunities:
    // - Per-client CUDA streams (warm_stream is shared across all clients!)
    // - Batched cold lookups (collect multiple cold misses, pipeline together)
    // - Speculative prefetch (start next cold read while current one DMA's to GPU)
    // - Drive-sharded pipeline rings (keys on different drives don't contend)

    fn lookup_async(&self, key: CacheKey, ipc_handle: IpcHandle) -> Result<GpuStream, DispatcherError> {
        self.ensure_initialized()?;

        let dm = self
            .dispatch_map
            .get()
            .map_err(|_| DispatcherError::NotInitialized("dispatch_map not bound".into()))?;

        let mt = self
            .memory_tier
            .get()
            .map_err(|_| DispatcherError::NotInitialized("memory_tier not bound".into()))?;

        let result = dm.lookup(key);

        let gpu = self
            .gpu_services
            .get()
            .map_err(|_| DispatcherError::NotInitialized("gpu_services not bound".into()))?;

        let null_stream = GpuStream(std::ptr::null_mut());

        match result {
            Ok(lookup_result) => {
                use interfaces::LookupResult;
                match lookup_result {
                    LookupResult::NotExist => Err(DispatcherError::KeyNotFound(key)),
                    LookupResult::MismatchSize => {
                        let _ = dm.release_read(key);
                        Err(DispatcherError::InvalidParameter(
                            "size mismatch on lookup".into(),
                        ))
                    }
                    LookupResult::MemoryTier { pointer, size } => {
                        let copy_size = (ipc_handle.size as usize).min(size as usize);

                        // Use dedicated warm stream (lock-free AtomicU64 load).
                        let raw = self.warm_stream.load(Ordering::Acquire);
                        if raw != 0 {
                            let s = GpuStream(raw as *mut std::ffi::c_void);
                            gpu.memcpy_h2d_async(
                                pointer as *const std::ffi::c_void,
                                ipc_handle.address as *mut std::ffi::c_void,
                                copy_size,
                                s,
                            )
                            .map_err(|e| {
                                let _ = dm.release_read(key);
                                DispatcherError::IoError(format!(
                                    "GPU DMA copy (memory-tier→device) failed: {e}"
                                ))
                            })?;
                            let _ = dm.release_read(key);
                            mt.touch(key);
                            Ok(s)
                        } else {
                            // Fallback: sync copy via DmaBuffer wrapper.
                            let aligned = copy_size.next_multiple_of(4096).max(4096);
                            let temp_buf = unsafe {
                                DmaBuffer::from_raw(
                                    pointer as *mut std::ffi::c_void,
                                    aligned,
                                    noop_free,
                                    -1,
                                )
                            }
                            .map_err(|e| {
                                let _ = dm.release_read(key);
                                DispatcherError::IoError(format!("DmaBuffer wrap failed: {e}"))
                            })?;
                            let copy_result = gpu.dma_copy_to_device(
                                &temp_buf,
                                ipc_handle.address as *mut std::ffi::c_void,
                                copy_size,
                            );
                            std::mem::forget(temp_buf);
                            let _ = dm.release_read(key);
                            mt.touch(key);
                            copy_result.map_err(|e| {
                                DispatcherError::IoError(format!(
                                    "GPU DMA copy (memory-tier→device) failed: {e}"
                                ))
                            })?;
                            Ok(null_stream)
                        }
                    }
                    LookupResult::Staging { buffer } => {
                        let copy_result = gpu.dma_copy_to_device(
                            &buffer,
                            ipc_handle.address as *mut std::ffi::c_void,
                            ipc_handle.size as usize,
                        );
                        let _ = dm.release_read(key);
                        copy_result.map_err(|e| {
                            DispatcherError::IoError(format!(
                                "GPU DMA copy (staging→device) failed: {e}"
                            ))
                        })?;
                        Ok(null_stream)
                    }
                    LookupResult::BlockDevice { offset } => {
                        let _ = dm.release_read(key);
                        self.promote_and_serve(key, offset, &ipc_handle, &gpu, &dm, &mt)?;
                        Ok(null_stream)
                    }
                }
            }
            Err(_) => Err(DispatcherError::KeyNotFound(key)),
        }
    }
    // ===== END EVOLVE-BLOCK: LOOKUP_ASYNC =====

    fn check(&self, key: CacheKey) -> Result<bool, DispatcherError> {
        self.ensure_initialized()?;

        let dm = self
            .dispatch_map
            .get()
            .map_err(|_| DispatcherError::NotInitialized("dispatch_map not bound".into()))?;

        match dm.lookup(key) {
            Ok(result) => {
                use interfaces::LookupResult;
                let exists = !matches!(result, LookupResult::NotExist);
                if exists {
                    let _ = dm.release_read(key);
                }
                Ok(exists)
            }
            Err(_) => Ok(false),
        }
    }

    fn remove(&self, key: CacheKey) -> Result<(), DispatcherError> {
        self.ensure_initialized()?;

        let dm = self
            .dispatch_map
            .get()
            .map_err(|_| DispatcherError::NotInitialized("dispatch_map not bound".into()))?;

        // Lookup the entry to determine its location (waits for any active writer).
        let block_offset = match dm.lookup(key) {
            Ok(LookupResult::BlockDevice { offset }) => {
                let _ = dm.release_read(key);
                Some(offset)
            }
            Ok(_) => {
                let _ = dm.release_read(key);
                None
            }
            Err(_) => return Err(DispatcherError::KeyNotFound(key)),
        };

        // Remove from memory-tier if present.
        if let Ok(mt) = self.memory_tier.get() {
            let _ = mt.remove(key);
        }

        // Remove from dispatch-map (fails if another reference was taken in the
        // window after we released ours — acceptable race, caller can retry).
        dm.remove(key)
            .map_err(|_| DispatcherError::KeyNotFound(key))?;

        if let Some(offset) = block_offset {
            let drives = self.data_drives.lock().unwrap();
            let idx = Self::drive_index(key, drives.len().max(1));
            if let Some(drive) = drives.get(idx) {
                if let Some(iem) = query_interface!(drive.extent_mgr, IExtentManager) {
                    let _ = iem.remove_extent(offset);
                }
            }
        }

        Ok(())
    }

    fn populate(&self, key: CacheKey, ipc_handle: IpcHandle) -> Result<(), DispatcherError> {
        self.ensure_initialized()?;

        if ipc_handle.size == 0 {
            return Err(DispatcherError::InvalidParameter(
                "IPC handle size must be > 0".into(),
            ));
        }

        let dm = self
            .dispatch_map
            .get()
            .map_err(|_| DispatcherError::NotInitialized("dispatch_map not bound".into()))?;

        let mt = self
            .memory_tier
            .get()
            .map_err(|_| DispatcherError::NotInitialized("memory_tier not bound".into()))?;

        // Evict from memory-tier if needed to make space.
        Self::evict_for_space(&dm, &mt, ipc_handle.size)?;

        // Allocate a slot in the memory-tier.
        let mem_ptr = mt.insert(key, ipc_handle.size).map_err(|e| match e {
            interfaces::MemoryTierError::AlreadyExists(k) => DispatcherError::AlreadyExists(k),
            interfaces::MemoryTierError::PoolFull => {
                DispatcherError::AllocationFailed("memory-tier pool full after eviction".into())
            }
            other => DispatcherError::AllocationFailed(other.to_string()),
        })?;

        // Create a temporary DmaBuffer wrapping the memory-tier slot for GPU DMA.
        let aligned_size = (ipc_handle.size as usize).next_multiple_of(4096);
        // SAFETY: mem_ptr is valid for aligned_size bytes, owned by memory-tier.
        let temp_buf = unsafe {
            DmaBuffer::from_raw(mem_ptr as *mut std::ffi::c_void, aligned_size, noop_free, -1)
        }
        .map_err(|e| {
            let _ = mt.remove(key);
            DispatcherError::AllocationFailed(format!("DmaBuffer wrap failed: {e}"))
        })?;

        let gpu = self
            .gpu_services
            .get()
            .map_err(|_| DispatcherError::NotInitialized("gpu_services not bound".into()))?;

        // DMA copy from GPU to memory-tier slot.
        gpu.dma_copy_to_host(
            ipc_handle.address as *const std::ffi::c_void,
            &temp_buf,
            ipc_handle.size as usize,
        )
        .map_err(|e| {
            let _ = mt.remove(key);
            DispatcherError::IoError(format!("GPU DMA copy failed: {e}"))
        })?;

        // Don't let the noop-free wrapper be dropped (it would call noop_free, which is fine,
        // but let's be explicit).
        std::mem::forget(temp_buf);

        // Register in dispatch-map as memory-tier entry.
        dm.create_memory_tier_entry(key, mem_ptr, ipc_handle.size)
            .map_err(|e| match e {
                interfaces::DispatchMapError::AlreadyExists(k) => {
                    let _ = mt.remove(key);
                    DispatcherError::AlreadyExists(k)
                }
                other => {
                    let _ = mt.remove(key);
                    DispatcherError::IoError(other.to_string())
                }
            })?;

        // Downgrade write ref to read ref for background writer.
        dm.downgrade_reference(key)
            .map_err(|e| DispatcherError::IoError(e.to_string()))?;

        // Enqueue background write-through to SSD.
        let num_drives = self.data_drives.lock().unwrap().len().max(1);
        let guard = self.bg_writer.lock().unwrap();
        if let Some(ref writer) = *guard {
            let _ = writer.enqueue(WriteJob {
                key,
                size: ipc_handle.size,
                device_index: Self::drive_index(key, num_drives),
            });
        }

        Ok(())
    }

    fn prepare_store(&self, key: CacheKey, size: u32) -> Result<Arc<DmaBuffer>, DispatcherError> {
        self.ensure_initialized()?;
        self.log_info(&format!("dispatcher: prepare_store key={key} size={size}"));

        if size == 0 {
            return Err(DispatcherError::InvalidParameter(
                "size must be > 0".into(),
            ));
        }

        let dm = self
            .dispatch_map
            .get()
            .map_err(|_| DispatcherError::NotInitialized("dispatch_map not bound".into()))?;

        // Register the key in the dispatch map (prevents duplicates, makes check() visible).
        // Uses create_staging as a lightweight reservation for the direct-write path.
        let _staging = dm.create_staging(key, 1).map_err(|e| match e {
            interfaces::DispatchMapError::AlreadyExists(k) => DispatcherError::AlreadyExists(k),
            other => DispatcherError::IoError(other.to_string()),
        })?;

        // Determine target drive and allocate extent.
        let drives = self.data_drives.lock().unwrap();
        let num_drives = drives.len().max(1);
        let drive_idx = Self::drive_index(key, num_drives);

        let (block_size, numa_node) = if let Some(drive) = drives.get(drive_idx) {
            (
                drive.block_dev_iface.block_size() as usize,
                drive.block_dev_iface.numa_node(),
            )
        } else {
            (4096, -1)
        };

        let extent_mgrs: Vec<Arc<ExtentManager>> = drives
            .iter()
            .map(|d| Arc::clone(&d.extent_mgr))
            .collect();
        drop(drives);

        let aligned_size = (size as usize).next_multiple_of(block_size);

        // Reserve extent via extent manager (if available).
        let write_handle = if let Some(em) = extent_mgrs.get(drive_idx) {
            if let Some(iem) = query_interface!(em, IExtentManager) {
                match iem.reserve_extent(key, aligned_size as u32) {
                    Ok(wh) => Some(wh),
                    Err(e) => {
                        let _ = dm.remove(key);
                        return Err(DispatcherError::AllocationFailed(format!(
                            "reserve_extent failed: {e}"
                        )));
                    }
                }
            } else {
                None
            }
        } else {
            None
        };

        // Allocate DMA buffer for the caller to write into.
        let buf = match DmaBuffer::new(aligned_size, block_size, Some(numa_node)) {
            Ok(b) => b,
            Err(_) => {
                // Fallback for environments without SPDK DMA (e.g., staging-only mode).
                let ptr = unsafe { libc::aligned_alloc(block_size, aligned_size) };
                if ptr.is_null() {
                    let _ = dm.remove(key);
                    return Err(DispatcherError::AllocationFailed(
                        "aligned_alloc failed".into(),
                    ));
                }
                unsafe { std::ptr::write_bytes(ptr as *mut u8, 0, aligned_size) };
                unsafe {
                    DmaBuffer::from_raw(ptr, aligned_size, libc_free, -1).map_err(|e| {
                        let _ = dm.remove(key);
                        DispatcherError::AllocationFailed(format!(
                            "DMA buffer from_raw failed: {e}"
                        ))
                    })?
                }
            }
        };

        let buf = Arc::new(buf);

        // Store the pending write for later commit/cancel.
        if let Some(wh) = write_handle {
            self.pending_writes.lock().unwrap().insert(
                key,
                PendingWrite {
                    write_handle: wh,
                    buffer: Arc::clone(&buf),
                    size,
                    drive_idx,
                },
            );
        }

        Ok(buf)
    }

    fn commit_store(&self, key: CacheKey) -> Result<(), DispatcherError> {
        self.ensure_initialized()?;
        self.log_info(&format!("dispatcher: commit_store key={key}"));

        let pending = self
            .pending_writes
            .lock()
            .unwrap()
            .remove(&key)
            .ok_or(DispatcherError::KeyNotFound(key))?;

        let drives = self.data_drives.lock().unwrap();
        let drive = drives.get(pending.drive_idx).ok_or_else(|| {
            DispatcherError::IoError("data drive not available for commit".into())
        })?;

        let block_size = drive.block_dev_iface.block_size() as usize;
        let block_dev_iface = Arc::clone(&drive.block_dev_iface);
        drop(drives);

        let block_offset = pending.write_handle.extent_offset();
        let start_lba = block_offset / block_size as u64;
        let total_bytes = pending.size as usize;

        Self::write_buffer_to_ssd(&*block_dev_iface, &pending.buffer, start_lba, total_bytes)?;

        // Data written — publish extent and register in dispatch map.
        let _ = pending.write_handle.publish();

        let dm = self
            .dispatch_map
            .get()
            .map_err(|_| DispatcherError::NotInitialized("dispatch_map not bound".into()))?;

        dm.convert_to_storage(key, block_offset)
            .map_err(|e| DispatcherError::IoError(format!("convert_to_storage failed: {e}")))?;

        let _ = dm.release_write(key);

        Ok(())
    }

    fn cancel_store(&self, key: CacheKey) -> Result<(), DispatcherError> {
        self.ensure_initialized()?;
        self.log_info(&format!("dispatcher: cancel_store key={key}"));

        self.pending_writes
            .lock()
            .unwrap()
            .remove(&key)
            .ok_or(DispatcherError::KeyNotFound(key))?;

        // PendingWrite dropped here — WriteHandle::drop calls abort automatically.

        // Remove the dispatch map entry created by prepare_store.
        let dm = self
            .dispatch_map
            .get()
            .map_err(|_| DispatcherError::NotInitialized("dispatch_map not bound".into()))?;
        let _ = dm.remove(key);

        Ok(())
    }

    fn touch(&self, key: CacheKey) -> Result<(), DispatcherError> {
        self.ensure_initialized()?;

        let dm = self
            .dispatch_map
            .get()
            .map_err(|_| DispatcherError::NotInitialized("dispatch_map not bound".into()))?;

        dm.touch(key)
            .map_err(|_| DispatcherError::KeyNotFound(key))
    }
}

