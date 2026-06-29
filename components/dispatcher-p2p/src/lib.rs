//! Dispatcher-P2P — GPU-direct cold-path variant of the Certus dispatcher.
//!
//! # Difference from `dispatcher`
//!
//! This crate is a fork of the base dispatcher that adds a GPUDirect P2P cold
//! path: instead of reading SSD → DRAM memory-tier → GPU (two DMA hops), it
//! reads SSD → GPU BAR1 staging ring → GPU VRAM (single PCIe hop for the data,
//! with GDRCopy-mapped BAR1 buffers registered with SPDK for NVMe DMA).
//!
//! ```text
//! Base dispatcher cold path:      NVMe SSD ──DMA──▶ DRAM ──DMA──▶ GPU
//! P2P dispatcher cold path:       NVMe SSD ──DMA──▶ GPU BAR1 ──copy──▶ GPU VRAM
//! ```
//!
//! The hot path (DRAM → GPU) is identical to the base dispatcher.
//!
//! # Architecture
//!
//! Same component-framework shape as the base dispatcher:
//! - Implements `IDispatcher` interface
//! - Receptacles: `ILogger`, `IDispatchMap`, `IMemoryTier`, `IGpuServices`, `ISPDKEnv`
//! - Factory-based block device and extent manager creation
//!
//! Additional module: `p2p_ring` — manages GPU-resident BAR1 staging buffers
//! for NVMe-to-GPU direct DMA.
//!
//! # When to use
//!
//! Use this dispatcher when the system has:
//! - GPUs with accessible BAR1 memory (large-BAR enabled in BIOS)
//! - GDRCopy installed (`libgdrapi.so`)
//! - NVMe drives on the same PCIe root complex as the GPU (same-socket)
//!
//! The P2P path avoids the DRAM bounce for cold reads, reducing cold-path
//! latency and freeing memory bandwidth for other operations.

#![allow(clippy::too_many_arguments)]

mod background;
pub mod io_segmenter;
pub mod p2p_ring;
pub mod pipeline;

use parking_lot::RwLock;
use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;
use std::sync::Mutex;

use component_framework::define_component;
use interfaces::{
    CacheKey, ClientChannels, Command, Completion, DispatcherConfig, DispatcherError, DmaAllocFn,
    DmaBuffer, FormatParams, GpuStream, IBlockDevice, IBlockDeviceAdmin, IDispatchMap, IDispatcher,
    IExtentManager, IGpuServices, ILogger, IMemoryTier, IRemoteLookup, IpcHandle, LookupResult,
    PciAddress, WriteHandle,
};

use component_core::binding::bind;
use spdk_env::ISPDKEnv;

use crate::background::{
    BackgroundEvictor, DramBackfillJob, DramBackfillWorker, EvictorConfig,
    ParallelBackgroundWriter, WriteJob,
};

/// Maximum number of queue threads per drive for cold-path processing.
/// With 4 drives, using 1 thread per drive gives each thread the full
/// 16-slot partition (MAX_QD_PER_THREAD), maximizing per-drive queue depth.
/// Using 2 threads per drive halves the effective QD to 8, which is still
/// sufficient but adds thread coordination overhead.
/// Set to 1 to maximize per-thread queue depth and minimize ring fragmentation.
const MAX_QUEUES_PER_DRIVE: usize = 1;

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
    block_dev_admin: Option<Arc<dyn IBlockDeviceAdmin + Send + Sync>>,
    block_dev_iface: Arc<dyn IBlockDevice + Send + Sync>,
    _extent_mgr_component: Arc<dyn component_core::IUnknown + Send + Sync>,
    extent_mgr: Arc<dyn IExtentManager + Send + Sync>,
    cached_channels: Option<ClientChannels>,
}

/// Factory function type for creating block device components.
/// Receives SPDK env, logger, drive index, PCI address, and optional CPU pin.
/// Returns a fully-initialized block device: (IUnknown holder, IBlockDevice, IBlockDeviceAdmin).
/// The factory is responsible for calling initialize() internally.
pub type BlockDeviceFactory = Box<
    dyn Fn(
            &Arc<dyn ISPDKEnv + Send + Sync>,
            &Arc<dyn ILogger + Send + Sync>,
            usize,
            PciAddress,
            Option<usize>,
        ) -> Result<
            (
                Arc<dyn component_core::IUnknown + Send + Sync>,
                Arc<dyn IBlockDevice + Send + Sync>,
                Arc<dyn IBlockDeviceAdmin + Send + Sync>,
            ),
            String,
        > + Send
        + Sync,
>;

/// Factory function type for creating extent manager components.
/// Receives logger and DMA allocator so it can wire receptacles.
/// Returns an `Arc<dyn IUnknown>` that provides `IExtentManager`.
pub type ExtentManagerFactory = Box<
    dyn Fn(
            &Arc<dyn ILogger + Send + Sync>,
            DmaAllocFn,
        ) -> Arc<dyn component_core::IUnknown + Send + Sync>
        + Send
        + Sync,
>;

define_component! {
    pub DispatcherP2pComponent {
        version: "0.1.0",
        provides: [IDispatcher],
        receptacles: {
            logger: ILogger,
            dispatch_map: IDispatchMap,
            gpu_services: IGpuServices,
            spdk_env: ISPDKEnv,
            memory_tier: IMemoryTier,
            remote_lookup: IRemoteLookup,
        },
        fields: {
            initialized: AtomicBool,
            bg_writer: Mutex<Option<ParallelBackgroundWriter>>,
            bg_evictor: Mutex<Option<BackgroundEvictor>>,
            bg_backfill: Mutex<Option<DramBackfillWorker>>,
            data_drives: RwLock<Vec<DataDrive>>,
            pending_writes: Mutex<HashMap<CacheKey, PendingWrite>>,
            pipeline_ring: RwLock<Option<pipeline::PipelineRing>>,
            p2p_ring: RwLock<Option<p2p_ring::P2pRing>>,
            warm_stream: AtomicU64,
            block_device_factory: Mutex<Option<BlockDeviceFactory>>,
            extent_manager_factory: Mutex<Option<ExtentManagerFactory>>,
        },
    }
}

unsafe extern "C" fn libc_free(ptr: *mut std::ffi::c_void) {
    unsafe { libc::free(ptr) };
}

/// No-op free function for temporary DmaBuffer wrappers around memory-tier pointers.
/// The memory-tier component owns the memory; this wrapper must not free it.
unsafe extern "C" fn noop_free(_ptr: *mut std::ffi::c_void) {}

impl DispatcherP2pComponent {
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

    /// Set a factory function for creating block device components.
    /// When set, the dispatcher uses this instead of the hard-coded SPDK NVMe implementation.
    pub fn set_block_device_factory(&self, factory: BlockDeviceFactory) {
        *self.block_device_factory.lock().unwrap() = Some(factory);
    }

    /// Set a factory function for creating extent manager components.
    /// When set, the dispatcher uses this instead of the hard-coded default.
    pub fn set_extent_manager_factory(&self, factory: ExtentManagerFactory) {
        *self.extent_manager_factory.lock().unwrap() = Some(factory);
    }

    fn drive_index(key: CacheKey, num_drives: usize) -> usize {
        // splitmix64 finalizer: distributes sequential keys uniformly.
        let mut h = key;
        h ^= h >> 30;
        h = h.wrapping_mul(0xbf58476d1ce4e5b9);
        h ^= h >> 27;
        h = h.wrapping_mul(0x94d049bb133111eb);
        h ^= h >> 31;
        h as usize % num_drives
    }

    /// Compute per-drive CPU assignments based on NUMA topology.
    ///
    /// For each PCI address, looks up the device's NUMA node from SPDK's
    /// device list, then assigns CPUs round-robin from that node's available
    /// cores. Returns `None` for any drive whose NUMA node can't be resolved
    /// (the block device component will fall back to its own NUMA heuristic).
    fn compute_numa_cpu_assignments(
        spdk_env: &Arc<dyn ISPDKEnv + Send + Sync>,
        pci_addrs: &[String],
        logger: &Arc<dyn ILogger + Send + Sync>,
    ) -> Vec<Option<usize>> {
        use std::collections::HashMap;

        let topo = match component_core::numa::NumaTopology::discover() {
            Ok(t) => t,
            Err(_) => {
                logger
                    .warn("dispatcher: NUMA topology unavailable, poller CPUs will not be pinned");
                return vec![None; pci_addrs.len()];
            }
        };

        let devices = spdk_env.devices();
        let device_map: HashMap<String, i32> = devices
            .iter()
            .map(|d| (d.address.to_string(), d.numa_node))
            .collect();

        // Track next available CPU index per NUMA node for round-robin.
        let mut node_cpu_idx: HashMap<usize, usize> = HashMap::new();

        pci_addrs
            .iter()
            .map(|addr| {
                let numa_node = device_map.get(addr).copied().unwrap_or(-1);
                if numa_node < 0 {
                    return None;
                }
                let node_id = numa_node as usize;
                let node = match topo.node(node_id) {
                    Some(n) => n,
                    None => return None,
                };
                let cpus: Vec<usize> = node.cpus().iter().filter(|&c| c >= 2).collect();
                if cpus.is_empty() {
                    return None;
                }
                let idx = node_cpu_idx.entry(node_id).or_insert(0);
                let cpu = cpus[*idx % cpus.len()];
                *idx += 1;
                logger.info(&format!(
                    "dispatcher: auto-pinning poller for {addr} to CPU {cpu} (NUMA node {node_id})"
                ));
                Some(cpu)
            })
            .collect()
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
    /// size. When `source_is_dma` is true, wraps slices of the source buffer
    /// directly for NVMe writes (zero-copy). Otherwise allocates per-segment
    /// staging buffers and copies into them.
    fn write_buffer_to_ssd(
        drive: &dyn IBlockDevice,
        buffer: &DmaBuffer,
        start_lba: u64,
        total_bytes: usize,
        source_is_dma: bool,
    ) -> Result<(), DispatcherError> {
        let block_size = drive.block_size() as usize;
        let max_transfer = drive.max_transfer_size();
        let numa_node = drive.numa_node();
        let aligned_bytes = total_bytes.next_multiple_of(block_size);

        let channels = drive
            .connect_client()
            .map_err(|e| DispatcherError::IoError(format!("connect_client failed: {e}")))?;

        let segments =
            io_segmenter::segment_io(start_lba, aligned_bytes, max_transfer, block_size as u32);

        for seg in &segments {
            let seg_buf = if source_is_dma {
                let ptr = unsafe {
                    (buffer.as_ptr() as *mut u8).add(seg.buffer_offset) as *mut std::ffi::c_void
                };
                unsafe { DmaBuffer::from_raw(ptr, seg.length, noop_free, numa_node) }.map_err(
                    |e| DispatcherError::AllocationFailed(format!("DmaBuffer wrap segment: {e}")),
                )?
            } else {
                let staging = DmaBuffer::new(seg.length, block_size, Some(numa_node)).map_err(
                    |e| DispatcherError::AllocationFailed(format!("DMA segment buffer: {e}")),
                )?;

                let copy_len = seg
                    .length
                    .min(total_bytes.saturating_sub(seg.buffer_offset));
                if copy_len > 0 {
                    unsafe {
                        std::ptr::copy_nonoverlapping(
                            (buffer.as_ptr() as *const u8).add(seg.buffer_offset),
                            staging.as_ptr() as *mut u8,
                            copy_len,
                        );
                    }
                }
                staging
            };

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
                    result
                        .map_err(|e| DispatcherError::IoError(format!("SSD write failed: {e}")))?;
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

        // Read from SSD into memory-tier using pipelined reader.
        let drives = self.data_drives.read();
        if drives.is_empty() {
            // No hardware: just copy zeros to GPU (test/staging-only mode).
            Self::evict_for_space(dm, mt, ipc_handle.size, key)?;
            let mem_ptr = mt.insert(key, ipc_handle.size).map_err(|e| {
                DispatcherError::AllocationFailed(format!("promote insert failed: {e}"))
            })?;
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

        let channels = match &drive.cached_channels {
            Some(ch) => ch,
            None => {
                drop(drives);
                return Err(DispatcherError::IoError(
                    "no cached channels for drive".into(),
                ));
            }
        };

        // Choose P2P path (NVMe → GPU BAR1 → D2D) or DRAM path (NVMe → DRAM → H2D).
        let p2p_guard = self.p2p_ring.read();
        if let Some(ref p2p) = *p2p_guard {
            let partition = p2p_ring::ThreadPartition::new(0, 1);
            unsafe {
                pipeline::pipelined_ssd_to_gpu_p2p(
                    &*block_dev,
                    &**gpu,
                    p2p,
                    &partition,
                    channels,
                    ipc_handle.address as *mut std::ffi::c_void,
                    start_lba,
                    total_bytes,
                )?;
            }
            drop(p2p_guard);
            drop(drives);

            // Client is served. Release write lock — key stays as BlockDevice.
            // Enqueue async DRAM backfill so future hot lookups work.
            let _ = dm.release_write(key);
            if let Some(ref backfill) = *self.bg_backfill.lock().unwrap() {
                let _ = backfill.enqueue(DramBackfillJob {
                    key,
                    drive_index: idx,
                });
            }
        } else {
            drop(p2p_guard);
            // DRAM path: allocate memory-tier slot, read NVMe→DRAM, then H2D to GPU.
            Self::evict_for_space(dm, mt, ipc_handle.size, key)?;
            let mem_ptr = mt.insert(key, ipc_handle.size).map_err(|e| {
                DispatcherError::AllocationFailed(format!("promote insert failed: {e}"))
            })?;

            let ring_guard = self.pipeline_ring.read();
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
                    16,
                )?;
            }
            drop(ring_guard);
            drop(drives);

            // DRAM is filled — register as MemoryTier immediately.
            let _ = dm.remove(key);
            dm.create_memory_tier_entry(key, mem_ptr, ipc_handle.size)
                .map_err(|e| DispatcherError::IoError(format!("promote re-register failed: {e}")))?;
            let _ = dm.convert_to_storage(key, offset);
            let _ = dm.release_write(key);
        }

        Ok(())
    }

    /// Evict entries from the memory-tier until enough space is available.
    ///
    /// Each evicted entry transitions from MemoryTier to BlockDevice in the
    /// dispatch-map. If write-through hasn't completed (no ssd_offset), the
    /// dispatch-map entry is removed entirely so lookups get NotExist rather
    /// than a dangling memory-tier pointer.
    // NOTE: This only handles global capacity pressure. If keys are heavily skewed
    // to one shard (e.g., all keys ≡ 0 mod 16), the target shard can fill while
    // global used() < capacity(). In that case insert() will return PoolFull after
    // this function succeeds. Acceptable for now — real workloads distribute evenly.
    fn evict_for_space(
        dm: &Arc<dyn IDispatchMap + Send + Sync>,
        mt: &Arc<dyn IMemoryTier + Send + Sync>,
        needed: u32,
        target_key: CacheKey,
    ) -> Result<(), DispatcherError> {
        const MAX_SCAN: usize = 4;
        const MAX_ATTEMPTS: usize = 512;

        let mut attempts = 0usize;
        while mt.used() + needed as usize > mt.capacity() {
            attempts += 1;
            if attempts > MAX_ATTEMPTS {
                static WARNED: std::sync::atomic::AtomicBool =
                    std::sync::atomic::AtomicBool::new(false);
                if !WARNED.swap(true, Ordering::Relaxed) {
                    const GIB: f64 = (1024 * 1024 * 1024) as f64;
                    eprintln!(
                        "WARNING: memory-tier exhausted (used={:.2} GiB, capacity={:.2} GiB, needed={:.2} GiB). \
                         Increase --memory-tier-size or reduce concurrent load.",
                        mt.used() as f64 / GIB,
                        mt.capacity() as f64 / GIB,
                        needed as f64 / GIB,
                    );
                }
                return Err(DispatcherError::AllocationFailed(
                    "memory-tier pool full after eviction".into(),
                ));
            }

            let evict_key = if attempts % 8 == 0 {
                let candidates = mt.oldest_keys(MAX_SCAN);
                candidates.iter().find(|&&k| dm.is_evictable(k)).copied()
            } else {
                None
            };

            match evict_key {
                Some(key) => {
                    if mt.remove(key).is_ok() {
                        let _ = dm.convert_memory_tier_to_block(key);
                    }
                }
                None => {
                    if let Some(evicted_key) = mt.evict_lru_for_key(target_key) {
                        if dm.convert_memory_tier_to_block(evicted_key).is_err() {
                            let _ = dm.remove(evicted_key);
                        }
                    }
                }
            }
        }
        Ok(())
    }


    fn process_write_job(
        dm: &Arc<dyn IDispatchMap + Send + Sync>,
        mt: &Arc<dyn IMemoryTier + Send + Sync>,
        drives: &[Arc<dyn IBlockDevice + Send + Sync>],
        extent_mgrs: &[Arc<dyn IExtentManager + Send + Sync>],
        job: WriteJob,
    ) {
        // Get the memory-tier pointer without refreshing LRU — the write-through
        // must not prevent this entry from being evicted under memory pressure.
        let (mem_ptr, _size) = match mt.peek(job.key) {
            Some(v) => v,
            None => {
                let _ = dm.release_read(job.key);
                return;
            }
        };

        if drives.is_empty() {
            // No block devices: mark as converted with a synthetic offset.
            let block_offset = job.key * 4096;
            let _ = dm.convert_to_storage(job.key, block_offset);
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
            Err(_) => {
                let _ = dm.release_read(job.key);
                return;
            }
        };

        // Allocate extent via the extent manager.
        let iem = &extent_mgrs[drive_idx % extent_mgrs.len()];
        let write_handle = match iem.reserve_extent(job.key, aligned_bytes as u32) {
            Ok(wh) => wh,
            Err(_) => {
                let _ = dm.release_read(job.key);
                return;
            }
        };

        let block_offset = write_handle.extent_offset();
        let start_lba = block_offset / block_size as u64;

        let dma_capable = mt.is_dma_capable();
        if Self::write_buffer_to_ssd(&**drive, &temp_buf, start_lba, total_bytes, dma_capable)
            .is_err()
        {
            let _ = dm.release_read(job.key);
            return; // write_handle drops → abort
        }

        // Prevent the noop-free DmaBuffer from being dropped normally.
        std::mem::forget(temp_buf);

        // Data written successfully — commit the extent metadata.
        // convert_to_storage also decrements the read reference.
        let _ = write_handle.publish();
        let _ = dm.convert_to_storage(job.key, block_offset);
        let _ = dm.release_read(job.key);
    }
}

impl DispatcherP2pComponent {
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

    #[allow(clippy::type_complexity, unused_variables)]
    fn create_block_device(
        &self,
        i: usize,
        poller_cpu: Option<usize>,
        spdk_env: &Arc<dyn ISPDKEnv + Send + Sync>,
        logger: &Arc<dyn ILogger + Send + Sync>,
        pci_addr: PciAddress,
        addr_str: &str,
    ) -> Result<
        (
            Arc<dyn component_core::IUnknown + Send + Sync>,
            Option<Arc<dyn IBlockDeviceAdmin + Send + Sync>>,
            Arc<dyn IBlockDevice + Send + Sync>,
        ),
        DispatcherError,
    > {
        let factory_guard = self.block_device_factory.lock().unwrap();
        if let Some(ref factory) = *factory_guard {
            // Factory still uses the base-cpu convention for backward compatibility.
            let base = poller_cpu.map(|c| c.saturating_sub(i));
            let (block_dev, ibd, admin) =
                factory(spdk_env, logger, i, pci_addr, base).map_err(|e| {
                    DispatcherError::IoError(format!(
                        "block device factory failed for drive {i}: {e}"
                    ))
                })?;
            Ok((block_dev, Some(admin), ibd))
        } else {
            drop(factory_guard);
            #[cfg(feature = "spdk-backend")]
            {
                let component = block_device_spdk_nvme::BlockDeviceSpdkNvmeComponent::new_default();
                component
                    .spdk_env
                    .connect(Arc::clone(spdk_env))
                    .map_err(|e| {
                        DispatcherError::IoError(format!(
                            "failed to wire spdk_env for data drive {i}: {e}"
                        ))
                    })?;
                component.logger.connect(Arc::clone(logger)).map_err(|e| {
                    DispatcherError::IoError(format!(
                        "failed to wire logger for data drive {i}: {e}"
                    ))
                })?;
                let block_dev = component as Arc<dyn component_core::IUnknown + Send + Sync>;
                let admin: Arc<dyn IBlockDeviceAdmin + Send + Sync> =
                    component_core::iunknown::query::<dyn IBlockDeviceAdmin + Send + Sync>(
                        &*block_dev,
                    )
                    .ok_or_else(|| {
                        DispatcherError::IoError(format!(
                            "failed to query IBlockDeviceAdmin for data drive {i}"
                        ))
                    })?;
                admin.set_pci_address(pci_addr);
                if let Some(cpu) = poller_cpu {
                    admin.set_actor_cpu(cpu);
                }
                admin.initialize().map_err(|e| {
                    DispatcherError::IoError(format!(
                        "failed to initialize block device at {addr_str}: {e}"
                    ))
                })?;
                let ibd: Arc<dyn IBlockDevice + Send + Sync> =
                    component_core::iunknown::query::<dyn IBlockDevice + Send + Sync>(&*block_dev)
                        .ok_or_else(|| {
                            DispatcherError::IoError(format!(
                                "failed to query IBlockDevice for data drive {i}"
                            ))
                        })?;
                Ok((block_dev, Some(admin), ibd))
            }
            #[cfg(not(feature = "spdk-backend"))]
            {
                Err(DispatcherError::IoError(
                    "no block device factory set and spdk-backend feature not enabled".into(),
                ))
            }
        }
    }

    fn create_data_drives(
        &self,
        config: &DispatcherConfig,
    ) -> Result<Vec<DataDrive>, DispatcherError> {
        let has_factory = self.block_device_factory.lock().unwrap().is_some();
        let spdk_env: Arc<dyn ISPDKEnv + Send + Sync> = match self.spdk_env.get() {
            Ok(env) => env,
            Err(_) if has_factory => {
                // Factory handles device creation; use uninitialized stub as placeholder
                use component_core::query_interface;
                let stub = spdk_env::SPDKEnvComponent::new_default();
                query_interface!(stub, ISPDKEnv).expect("SPDKEnvComponent must provide ISPDKEnv")
            }
            Err(_) => {
                return Err(DispatcherError::NotInitialized("spdk_env not bound".into()));
            }
        };

        let logger = self
            .logger
            .get()
            .map_err(|_| DispatcherError::NotInitialized("logger not bound".into()))?;

        // Compute per-drive poller CPU assignments.
        // When poller_base_cpu is set, drive i gets base + i (existing behavior).
        // When None, assign CPUs round-robin from each drive's NUMA node.
        let per_drive_cpu: Vec<Option<usize>> = if config.poller_base_cpu.is_some() {
            config
                .data_pci_addrs
                .iter()
                .enumerate()
                .map(|(i, _)| config.poller_base_cpu.map(|base| base + i))
                .collect()
        } else {
            Self::compute_numa_cpu_assignments(&spdk_env, &config.data_pci_addrs, &logger)
        };

        let mut drives = Vec::with_capacity(config.data_pci_addrs.len());

        for (i, addr_str) in config.data_pci_addrs.iter().enumerate() {
            let pci_addr = Self::parse_pci_addr(addr_str)?;

            let poller_cpu = per_drive_cpu.get(i).copied().flatten();
            let (block_dev_component, admin, ibd) =
                self.create_block_device(i, poller_cpu, &spdk_env, &logger, pci_addr, addr_str)?;

            let numa_node = ibd.numa_node();
            let spdk_available = self.spdk_env.is_connected();
            let dma_alloc: DmaAllocFn = if spdk_available {
                // SPDK path: use hugepage-backed DMA buffers
                Arc::new(move |size, align, _numa| {
                    DmaBuffer::new(size, align, Some(numa_node)).map_err(|e| e.to_string())
                })
            } else {
                // Non-SPDK path: use posix_memalign for DMA buffers
                unsafe extern "C" fn libc_free(p: *mut std::ffi::c_void) {
                    libc::free(p);
                }
                Arc::new(move |size, align, _numa| {
                    let mut ptr: *mut std::ffi::c_void = std::ptr::null_mut();
                    // SAFETY: posix_memalign is safe with valid align (power of 2, multiple of sizeof(void*))
                    let ret = unsafe { libc::posix_memalign(&mut ptr, align, size) };
                    if ret != 0 || ptr.is_null() {
                        return Err(format!(
                            "posix_memalign failed ({size}, {align}): errno {ret}"
                        ));
                    }
                    unsafe { std::ptr::write_bytes(ptr as *mut u8, 0, size) };
                    // SAFETY: ptr is valid, non-null, allocated with posix_memalign
                    unsafe {
                        DmaBuffer::from_raw(ptr, size, libc_free, numa_node)
                            .map_err(|e| e.to_string())
                    }
                })
            };

            let extent_mgr: Arc<dyn component_core::IUnknown + Send + Sync> = {
                let factory_guard = self.extent_manager_factory.lock().unwrap();
                if let Some(ref factory) = *factory_guard {
                    factory(&logger, dma_alloc)
                } else {
                    let em = extent_manager::ExtentManager::new_inner();
                    em.set_dma_alloc(dma_alloc);
                    em.logger
                        .connect(Arc::clone(&logger) as Arc<dyn ILogger + Send + Sync>)
                        .map_err(|e| {
                            DispatcherError::IoError(format!(
                                "failed to wire logger for extent manager {i}: {e}"
                            ))
                        })?;
                    em as Arc<dyn component_core::IUnknown + Send + Sync>
                }
            };

            bind(
                &*block_dev_component,
                "IBlockDevice",
                &*extent_mgr,
                "metadata_device",
            )
            .map_err(|e| {
                DispatcherError::IoError(format!(
                    "failed to bind block device to extent manager {i}: {e}"
                ))
            })?;

            let iem: Arc<dyn IExtentManager + Send + Sync> =
                component_core::iunknown::query::<dyn IExtentManager + Send + Sync>(&*extent_mgr)
                    .ok_or_else(|| {
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
            } else {
                iem.initialize().map_err(|e| {
                    DispatcherError::IoError(format!(
                        "failed to recover extent manager for data drive {i}: {e}"
                    ))
                })?;
            }

            let cpu_msg = config
                .poller_base_cpu
                .map(|base| format!(", poller pinned to CPU {}", base + i))
                .unwrap_or_default();
            self.log_info(&format!(
                "dispatcher: data drive {i} initialized at {addr_str}{cpu_msg}"
            ));

            let cached_channels = ibd.connect_client().ok();

            drives.push(DataDrive {
                _block_dev: block_dev_component,
                block_dev_admin: admin,
                block_dev_iface: ibd,
                _extent_mgr_component: extent_mgr
                    as Arc<dyn component_core::IUnknown + Send + Sync>,
                extent_mgr: iem,
                cached_channels,
            });
        }

        Ok(drives)
    }
}

impl IDispatcher for DispatcherP2pComponent {
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
        // Skip drive creation only in memory-tier-only mode (no spdk_env AND no factory).
        let has_bd_factory = self.block_device_factory.lock().unwrap().is_some();
        if self.spdk_env.is_connected() || has_bd_factory {
            let drives = self.create_data_drives(&config)?;
            *self.data_drives.write() = drives;

            // Rebuild dispatch-map from recovered extents when not formatting.
            if !config.format_on_init {
                let t0 = std::time::Instant::now();
                let dm = self.dispatch_map.get().map_err(|_| {
                    DispatcherError::NotInitialized("dispatch_map not bound".into())
                })?;
                let mut recovered: u64 = 0;
                let drives_guard = self.data_drives.read();
                for drive in drives_guard.iter() {
                    drive.extent_mgr.for_each_extent(&mut |extent| {
                        let _ = dm.recover_extent(extent.key, extent.offset, extent.size);
                        recovered += 1;
                    });
                }
                drop(drives_guard);
                let elapsed = t0.elapsed();
                self.log_info(&format!(
                    "dispatcher: dispatch-map recovered {recovered} extents from disk ({elapsed:.2?})"
                ));
            }

            // Pre-allocate pipeline ring and register memory with SPDK.
            // Only when SPDK is available — spdk_mem_register segfaults otherwise.
            if let Ok(gpu) = self.gpu_services.get() {
                // Dedicated CUDA stream for warm-path DMA (avoids pipeline_ring lock).
                match gpu.create_stream() {
                    Ok(stream) => {
                        self.warm_stream.store(stream.0 as u64, Ordering::Release);
                    }
                    Err(e) => {
                        self.log_info(&format!("warm stream allocation failed (non-fatal): {e}"));
                    }
                }

                if self.spdk_env.is_connected() {
                    let chunk_size = {
                        let dd = self.data_drives.read();
                        dd.first()
                            .map(|d| d.block_dev_iface.max_transfer_size() as usize)
                            .unwrap_or(131072)
                    };
                    match pipeline::PipelineRing::new(&*gpu, chunk_size) {
                        Ok(ring) => {
                            *self.pipeline_ring.write() = Some(ring);
                        }
                        Err(e) => {
                            self.log_info(&format!(
                                "pipeline ring allocation failed (non-fatal): {e:?}"
                            ));
                        }
                    }

                    // Attempt P2P ring allocation (GPU BAR1 staging buffers).
                    match p2p_ring::P2pRing::new(&*gpu, chunk_size) {
                        Some(ring) => {
                            self.log_info(&format!(
                                "dispatcher: P2P ring initialized ({} slots, {} KiB each, {} streams)",
                                p2p_ring::P2P_RING_SLOTS,
                                chunk_size / 1024,
                                ring.streams().len(),
                            ));
                            *self.p2p_ring.write() = Some(ring);
                        }
                        None => {
                            self.log_info(
                                "dispatcher: P2P ring unavailable, cold reads use DRAM path",
                            );
                        }
                    }

                    // Register memory-tier pool as CUDA-pinned + SPDK DMA-capable
                    // for zero-copy NVMe reads and async GPU transfers.
                    if let Ok(mt) = self.memory_tier.get() {
                        if let Some((pool_ptr, pool_size)) = mt.pool_info() {
                            match gpu
                                .register_host_memory(pool_ptr as *mut std::ffi::c_void, pool_size)
                            {
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
        let bg_drives: Vec<Arc<dyn IBlockDevice + Send + Sync>> = {
            let dd = self.data_drives.read();
            dd.iter().map(|d| Arc::clone(&d.block_dev_iface)).collect()
        };
        let bg_extent_mgrs: Vec<Arc<dyn IExtentManager + Send + Sync>> = {
            let dd = self.data_drives.read();
            dd.iter().map(|d| Arc::clone(&d.extent_mgr)).collect()
        };

        let num_writer_drives = bg_drives.len().max(1);
        let writer = ParallelBackgroundWriter::start(num_writer_drives, |drive_idx| {
            let dm = Arc::clone(&dm_for_writer);
            let mt = Arc::clone(&mt_for_writer);
            let drives = bg_drives.clone();
            let extent_mgrs = bg_extent_mgrs.clone();
            let _ = drive_idx;
            move |job: WriteJob| {
                Self::process_write_job(&dm, &mt, &drives, &extent_mgrs, job);
            }
        });

        *self.bg_writer.lock().unwrap() = Some(writer);

        // Start background DRAM backfill worker (async NVMe→DRAM after P2P cold reads).
        let num_backfill_drives = bg_drives.len();
        let backfill_delay_ms = config.backfill_delay_ms;
        if num_backfill_drives > 0 && backfill_delay_ms > 0 {
            let dm_for_backfill = self
                .dispatch_map
                .get()
                .map_err(|_| DispatcherError::NotInitialized("dispatch_map not bound".into()))?;
            let mt_for_backfill = self
                .memory_tier
                .get()
                .map_err(|_| DispatcherError::NotInitialized("memory_tier not bound".into()))?;
            let backfill_drives = bg_drives.clone();

            let chunk_size = {
                let ring_guard = self.pipeline_ring.read();
                ring_guard.as_ref().map_or(131072, |r| r.chunk_size)
            };

            let backfill = DramBackfillWorker::start(num_backfill_drives, |drive_idx| {
                let dm = Arc::clone(&dm_for_backfill);
                let mt = Arc::clone(&mt_for_backfill);
                let drive = Arc::clone(&backfill_drives[drive_idx]);
                let channels = drive.connect_client().expect("backfill connect_client");
                let block_size = drive.block_size() as u64;
                move |job: DramBackfillJob| {
                    std::thread::sleep(std::time::Duration::from_millis(backfill_delay_ms));

                    let start_lba = {
                        let lookup = dm.lookup(job.key);
                        match lookup {
                            Ok(LookupResult::BlockDevice { offset }) => {
                                let _ = dm.release_read(job.key);
                                offset / block_size
                            }
                            Ok(LookupResult::MemoryTier { .. }) => {
                                let _ = dm.release_read(job.key);
                                return; // Already promoted, nothing to do
                            }
                            _ => return,
                        }
                    };

                    let size = match dm.entry_size(job.key) {
                        Ok(s) => s,
                        Err(_) => return,
                    };

                    if Self::evict_for_space(&dm, &mt, size, job.key).is_err() {
                        return;
                    }

                    let mem_ptr = match mt.insert(job.key, size) {
                        Ok(ptr) => ptr,
                        Err(_) => return,
                    };

                    let result = unsafe {
                        pipeline::pipelined_ssd_to_dram_only(
                            &*drive,
                            &channels,
                            mem_ptr,
                            start_lba,
                            size as usize,
                            chunk_size,
                            16,
                        )
                    };

                    if result.is_ok() {
                        let _ = dm.remove(job.key);
                        let _ = dm.create_memory_tier_entry(job.key, mem_ptr, size);
                        let _ = dm.convert_to_storage(job.key, start_lba * block_size);
                        let _ = dm.release_write(job.key);
                    } else {
                        let _ = mt.remove(job.key);
                    }
                }
            });
            *self.bg_backfill.lock().unwrap() = Some(backfill);
        }

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
            let evictor_extent_mgrs: Vec<Arc<dyn IExtentManager + Send + Sync>> = {
                let dd = self.data_drives.read();
                dd.iter().map(|d| Arc::clone(&d.extent_mgr)).collect()
            };
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

        if let Ok(rl) = self.remote_lookup.get() {
            let _ = rl.join_cluster("certus://local-cluster");
        }

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

        if let Some(mut backfill) = self.bg_backfill.lock().unwrap().take() {
            backfill.shutdown();
        }

        self.pending_writes.lock().unwrap().clear();

        // Checkpoint all extent managers to persist metadata before teardown.
        {
            let drives = self.data_drives.read();
            for (i, drive) in drives.iter().enumerate() {
                if let Err(e) = drive.extent_mgr.checkpoint() {
                    self.log_error(&format!(
                        "dispatcher: extent manager {i} checkpoint failed: {e}"
                    ));
                }
            }
        }

        // Unregister memory-tier pool from CUDA/SPDK before tearing down.
        if let (Ok(gpu), Ok(mt)) = (self.gpu_services.get(), self.memory_tier.get()) {
            if let Some((pool_ptr, pool_size)) = mt.pool_info() {
                let _ = gpu.unregister_host_memory(pool_ptr as *mut std::ffi::c_void, pool_size);
            }
        }

        // Destroy warm stream, P2P ring, and pipeline ring.
        if let Ok(gpu) = self.gpu_services.get() {
            let raw = self.warm_stream.swap(0, Ordering::AcqRel);
            if raw != 0 {
                let _ = gpu.destroy_stream(GpuStream(raw as *mut std::ffi::c_void));
            }
            let p2p_opt = self.p2p_ring.write().take();
            if let Some(ring) = p2p_opt {
                ring.destroy(&*gpu);
            }
            let ring_opt = self.pipeline_ring.write().take();
            if let Some(ring) = ring_opt {
                ring.destroy(&*gpu);
            }
        }

        // Two-phase block device shutdown: signal all actors to stop first,
        // then join threads. This prevents crashes from SPDK transport teardown
        // invalidating memory that other actors are still actively polling.
        let drives = {
            let mut g = self.data_drives.write();
            std::mem::take(&mut *g)
        };
        // Phase 1: Signal all actors to stop (closes channels, actors exit poll loops)
        for drive in &drives {
            if let Some(ref admin) = drive.block_dev_admin {
                admin.signal_stop();
            }
        }
        // Phase 2: Join actor threads (safe now that all actors have been signaled)
        for (i, drive) in drives.iter().enumerate().rev() {
            if let Some(ref admin) = drive.block_dev_admin {
                if let Err(e) = admin.shutdown() {
                    self.log_error(&format!(
                        "dispatcher: failed to shut down data drive {i}: {e}"
                    ));
                }
            }
        }
        // Phase 3: Detach controllers (safe now that ALL actor threads have exited)
        for drive in &drives {
            if let Some(ref admin) = drive.block_dev_admin {
                admin.detach_controller();
            }
        }

        if let Ok(rl) = self.remote_lookup.get() {
            let _ = rl.leave_cluster();
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
            gpu.stream_synchronize(stream)
                .map_err(|e| DispatcherError::IoError(format!("stream_synchronize failed: {e}")))?;
        }
        Ok(())
    }

    fn batch_lookup(&self, entries: &[(CacheKey, IpcHandle)]) -> Vec<Result<(), DispatcherError>> {
        if entries.is_empty() {
            return Vec::new();
        }

        let init_check = self.ensure_initialized();
        if let Err(e) = init_check {
            return entries.iter().map(|_| Err(e.clone())).collect();
        }

        let dm = match self.dispatch_map.get() {
            Ok(dm) => dm,
            Err(_) => {
                let e = DispatcherError::NotInitialized("dispatch_map not bound".into());
                return entries.iter().map(|_| Err(e.clone())).collect();
            }
        };
        let mt = match self.memory_tier.get() {
            Ok(mt) => mt,
            Err(_) => {
                let e = DispatcherError::NotInitialized("memory_tier not bound".into());
                return entries.iter().map(|_| Err(e.clone())).collect();
            }
        };
        let gpu = match self.gpu_services.get() {
            Ok(gpu) => gpu,
            Err(_) => {
                let e = DispatcherError::NotInitialized("gpu_services not bound".into());
                return entries.iter().map(|_| Err(e.clone())).collect();
            }
        };

        let mut results: Vec<Option<Result<(), DispatcherError>>> = vec![None; entries.len()];

        // Classify entries and handle fast paths inline.
        struct ColdEntry {
            idx: usize,
            key: CacheKey,
            offset: u64,
            ipc_handle_addr: *mut u8,
            ipc_handle_size: u32,
        }
        // SAFETY: ColdEntry contains a raw pointer from IpcHandle (GPU device pointer).
        // These pointers are valid across threads — CUDA IPC handles are designed for
        // cross-process/thread use. We only read the pointer value to pass to CUDA APIs.
        unsafe impl Send for ColdEntry {}
        unsafe impl Sync for ColdEntry {}

        let mut cold_entries: Vec<ColdEntry> = Vec::new();

        for (i, (key, ipc_handle)) in entries.iter().enumerate() {
            let key = *key;
            match dm.lookup(key) {
                Ok(lookup_result) => match lookup_result {
                    LookupResult::NotExist => {
                        results[i] = Some(Err(DispatcherError::KeyNotFound(key)));
                    }
                    LookupResult::MismatchSize => {
                        let _ = dm.release_read(key);
                        results[i] = Some(Err(DispatcherError::InvalidParameter(
                            "size mismatch on lookup".into(),
                        )));
                    }
                    LookupResult::MemoryTier { pointer, size } => {
                        let copy_size = (ipc_handle.size as usize).min(size as usize);
                        let raw = self.warm_stream.load(Ordering::Acquire);
                        let res = if raw != 0 {
                            let s = GpuStream(raw as *mut std::ffi::c_void);
                            gpu.memcpy_h2d_async(
                                pointer as *const std::ffi::c_void,
                                ipc_handle.address as *mut std::ffi::c_void,
                                copy_size,
                                s,
                            )
                            .map_err(|e| {
                                DispatcherError::IoError(format!(
                                    "GPU DMA copy (memory-tier→device) failed: {e}"
                                ))
                            })
                            .and_then(|_| {
                                gpu.stream_synchronize(s).map_err(|e| {
                                    DispatcherError::IoError(format!(
                                        "stream_synchronize failed: {e}"
                                    ))
                                })
                            })
                        } else {
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
                                DispatcherError::IoError(format!("DmaBuffer wrap failed: {e}"))
                            });
                            match temp_buf {
                                Ok(buf) => {
                                    let r = gpu
                                        .dma_copy_to_device(
                                            &buf,
                                            ipc_handle.address as *mut std::ffi::c_void,
                                            copy_size,
                                        )
                                        .map_err(|e| {
                                            DispatcherError::IoError(format!(
                                                "GPU DMA copy (memory-tier→device) failed: {e}"
                                            ))
                                        });
                                    std::mem::forget(buf);
                                    r
                                }
                                Err(e) => Err(e),
                            }
                        };
                        let _ = dm.release_read(key);
                        mt.touch(key);
                        results[i] = Some(res);
                    }
                    LookupResult::Staging { buffer } => {
                        let res = gpu
                            .dma_copy_to_device(
                                &buffer,
                                ipc_handle.address as *mut std::ffi::c_void,
                                ipc_handle.size as usize,
                            )
                            .map_err(|e| {
                                DispatcherError::IoError(format!(
                                    "GPU DMA copy (staging→device) failed: {e}"
                                ))
                            });
                        let _ = dm.release_read(key);
                        results[i] = Some(res);
                    }
                    LookupResult::BlockDevice { offset } => {
                        let _ = dm.release_read(key);
                        cold_entries.push(ColdEntry {
                            idx: i,
                            key,
                            offset,
                            ipc_handle_addr: ipc_handle.address,
                            ipc_handle_size: ipc_handle.size,
                        });
                    }
                },
                Err(_) => {
                    results[i] = Some(Err(DispatcherError::KeyNotFound(key)));
                }
            }
        }

        // Promote cold entries in parallel — one queue thread per drive.
        // With MAX_QUEUES_PER_DRIVE=1 and 4 drives, each thread gets the full
        // 16-slot ring partition, maximizing per-drive NVMe queue depth.
        if !cold_entries.is_empty() {
            let drives = self.data_drives.read();
            let num_drives = drives.len();

            if num_drives == 0 {
                for entry in &cold_entries {
                    Self::evict_for_space(&dm, &mt, entry.ipc_handle_size, entry.key).ok();
                    let res = mt
                        .insert(entry.key, entry.ipc_handle_size)
                        .map(|mem_ptr| {
                            let _ = dm.create_memory_tier_entry(
                                entry.key,
                                mem_ptr,
                                entry.ipc_handle_size,
                            );
                            let _ = dm.release_write(entry.key);
                        })
                        .map_err(|e| {
                            DispatcherError::AllocationFailed(format!("promote insert failed: {e}"))
                        });
                    results[entry.idx] = Some(res);
                }
            } else {
                // Group cold entries by target drive.
                let mut per_drive: Vec<Vec<usize>> = vec![Vec::new(); num_drives];
                for (ci, entry) in cold_entries.iter().enumerate() {
                    let drive_idx = Self::drive_index(entry.key, num_drives);
                    per_drive[drive_idx].push(ci);
                }

                let p2p_ring_guard = self.p2p_ring.read();
                let p2p_ref = p2p_ring_guard.as_ref();

                std::thread::scope(|s| {
                    #[allow(clippy::type_complexity)]
                    let mut thread_handles: Vec<
                        std::thread::ScopedJoinHandle<(Vec<(usize, Result<(), DispatcherError>)>, Vec<(CacheKey, usize)>)>,
                    > = Vec::new();

                    let mut thread_counter = 0usize;
                    // Total threads = one per non-empty drive (MAX_QUEUES_PER_DRIVE=1).
                    // This gives each thread the maximum ring partition size (16 slots).
                    let total_threads = per_drive
                        .iter()
                        .filter(|e| !e.is_empty())
                        .map(|_| MAX_QUEUES_PER_DRIVE)
                        .sum::<usize>()
                        .max(1);

                    for (drive_idx, entry_indices) in per_drive.iter().enumerate() {
                        if entry_indices.is_empty() {
                            continue;
                        }

                        // With MAX_QUEUES_PER_DRIVE=1, one thread handles all entries
                        // for this drive, getting the full ring partition.
                        let num_queues = MAX_QUEUES_PER_DRIVE.min(entry_indices.len());
                        let chunks: Vec<&[usize]> = entry_indices
                            .chunks(entry_indices.len().div_ceil(num_queues))
                            .collect();

                        for chunk in chunks {
                            let dm_ref = &dm;
                            let _mt_ref = &mt;
                            let _gpu_ref = &gpu;
                            let drives_ref = &drives;
                            let cold_ref = &cold_entries;
                            let indices = chunk.to_vec();
                            let my_thread_idx = thread_counter;
                            thread_counter += 1;

                            let handle = s.spawn(move || {
                                let drive = &drives_ref[drive_idx];
                                let block_size = drive.block_dev_iface.block_size();

                                let channels =
                                    drive.block_dev_iface.connect_client().map_err(|e| {
                                        DispatcherError::IoError(format!(
                                            "connect_client failed: {e}"
                                        ))
                                    });

                                let mut batch_results: Vec<(usize, Result<(), DispatcherError>)> =
                                    Vec::with_capacity(indices.len());

                                let channels = match channels {
                                    Ok(ch) => ch,
                                    Err(e) => {
                                        for &ci in &indices {
                                            batch_results.push((ci, Err(e.clone())));
                                        }
                                        return (batch_results, Vec::new());
                                    }
                                };

                                // P2P path: SSD → BAR1 ring → D2D → client GPU.
                                let p2p = p2p_ref.expect(
                                    "dispatcher-p2p requires P2P ring; use full.yaml profile for DRAM path"
                                );
                                // With total_threads = num_non_empty_drives, each thread
                                // gets a non-overlapping 16-slot partition of the 64-slot ring.
                                let partition = p2p_ring::ThreadPartition::new(
                                    my_thread_idx,
                                    total_threads,
                                );
                                let mut backfill_keys: Vec<(CacheKey, usize)> = Vec::new();

                                // Build multi-object job list for interleaved P2P pipeline.
                                let jobs: Vec<pipeline::P2pColdJob> = indices.iter().map(|&ci| {
                                    let entry = &cold_ref[ci];
                                    pipeline::P2pColdJob {
                                        gpu_dst: entry.ipc_handle_addr as *mut std::ffi::c_void,
                                        start_lba: entry.offset / block_size as u64,
                                        total_bytes: entry.ipc_handle_size as usize,
                                    }
                                }).collect();

                                let pipeline_results = unsafe {
                                    pipeline::pipelined_multi_object_p2p(
                                        &*drive.block_dev_iface,
                                        p2p,
                                        &partition,
                                        &channels,
                                        &jobs,
                                    )
                                };

                                for (job_idx, result) in pipeline_results.into_iter().enumerate() {
                                    let ci = indices[job_idx];
                                    let entry = &cold_ref[ci];
                                    if result.is_ok() {
                                        let _ = dm_ref.release_write(entry.key);
                                        backfill_keys.push((entry.key, drive_idx));
                                    }
                                    batch_results.push((ci, result));
                                }

                                (batch_results, backfill_keys)
                            });

                            thread_handles.push(handle);
                        }
                    }

                    // Collect results from all threads.
                    let mut all_backfill_keys: Vec<(CacheKey, usize)> = Vec::new();
                    for handle in thread_handles {
                        let (batch_results, backfill_keys) =
                            handle.join().unwrap_or_else(|_| (Vec::new(), Vec::new()));
                        for (ci, res) in batch_results {
                            results[cold_entries[ci].idx] = Some(res);
                        }
                        all_backfill_keys.extend(backfill_keys);
                    }

                    // Enqueue async DRAM backfill for all P2P-served keys.
                    if let Some(ref backfill) = *self.bg_backfill.lock().unwrap() {
                        for (key, drive_index) in all_backfill_keys {
                            let _ = backfill.enqueue(DramBackfillJob { key, drive_index });
                        }
                    }
                });
            }
        }

        // --- Remote lookup for entries not found locally ---
        if let Ok(rl) = self.remote_lookup.get() {
            let not_found: Vec<usize> = results
                .iter()
                .enumerate()
                .filter_map(|(i, r)| match r {
                    Some(Err(DispatcherError::KeyNotFound(_))) => Some(i),
                    _ => None,
                })
                .collect();

            if !not_found.is_empty() {
                let remote_entries: Vec<(CacheKey, IpcHandle)> = not_found
                    .iter()
                    .map(|&i| {
                        let (key, handle) = &entries[i];
                        (*key, IpcHandle {
                            address: handle.address,
                            size: handle.size,
                        })
                    })
                    .collect();

                let remote_results = rl.batch_lookup(&remote_entries);

                for (pos, remote_res) in not_found.iter().zip(remote_results.into_iter()) {
                    results[*pos] = Some(remote_res.map_err(|e| {
                        DispatcherError::IoError(format!("remote lookup: {e}"))
                    }));
                }
            }
        }

        results.into_iter().map(|r| r.unwrap()).collect()
    }

    fn lookup_async(
        &self,
        key: CacheKey,
        ipc_handle: IpcHandle,
    ) -> Result<GpuStream, DispatcherError> {
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
            let drives = self.data_drives.read();
            let idx = Self::drive_index(key, drives.len().max(1));
            if let Some(drive) = drives.get(idx) {
                let _ = drive.extent_mgr.remove_extent(offset);
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

        let size: u32 = ipc_handle.size;

        // Phase 1: Evict if needed and allocate memory-tier slot.
        let _mem_ptr = self.reserve_memory(key, size)?;

        // Phase 2: Async DMA into the reserved slot, then synchronize.
        let gpu = self
            .gpu_services
            .get()
            .map_err(|_| DispatcherError::NotInitialized("gpu_services not bound".into()))?;
        let stream = GpuStream(self.warm_stream.load(Ordering::Acquire) as *mut std::ffi::c_void);
        self.copy_gpu_to_memory_async(key, ipc_handle, stream)?;
        gpu.stream_synchronize(stream)
            .map_err(|e| DispatcherError::IoError(format!("stream_synchronize failed: {e}")))?;

        // Phase 3: Register in dispatch-map and enqueue SSD write-through.
        self.copy_gpu_to_memory_completed(key, size)?;

        Ok(())
    }

    fn reserve_memory(&self, key: CacheKey, size: u32) -> Result<*mut u8, DispatcherError> {
        self.ensure_initialized()?;

        if size == 0 {
            return Err(DispatcherError::InvalidParameter(
                "size must be > 0".into(),
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

        Self::evict_for_space(&dm, &mt, size, key)?;

        let mem_ptr = mt.insert(key, size).map_err(|e| match e {
            interfaces::MemoryTierError::AlreadyExists(k) => DispatcherError::AlreadyExists(k),
            interfaces::MemoryTierError::PoolFull => {
                DispatcherError::AllocationFailed("memory-tier pool full after eviction".into())
            }
            other => DispatcherError::AllocationFailed(other.to_string()),
        })?;

        Ok(mem_ptr)
    }

    fn copy_gpu_to_memory_async(&self, key: CacheKey, ipc_handle: IpcHandle, stream: GpuStream) -> Result<(), DispatcherError> {
        self.ensure_initialized()?;

        let mt = self
            .memory_tier
            .get()
            .map_err(|_| DispatcherError::NotInitialized("memory_tier not bound".into()))?;

        let (mem_ptr, slot_size) = mt.get(key).ok_or(DispatcherError::KeyNotFound(key))?;

        let aligned_size = (slot_size as usize).next_multiple_of(4096);
        // SAFETY: mem_ptr is valid for aligned_size bytes, owned by memory-tier.
        let temp_buf = unsafe {
            DmaBuffer::from_raw(
                mem_ptr as *mut std::ffi::c_void,
                aligned_size,
                noop_free,
                -1,
            )
        }
        .map_err(|e| {
            DispatcherError::IoError(format!("DmaBuffer wrap failed: {e}"))
        })?;

        let gpu = self
            .gpu_services
            .get()
            .map_err(|_| DispatcherError::NotInitialized("gpu_services not bound".into()))?;

        gpu.dma_copy_to_host_async(
            ipc_handle.address as *const std::ffi::c_void,
            &temp_buf,
            ipc_handle.size as usize,
            stream,
        )
        .map_err(|e| {
            let _ = mt.remove(key);
            DispatcherError::IoError(format!("GPU async DMA copy failed: {e}"))
        })?;

        // Don't let the noop-free wrapper be dropped (it would call noop_free, which is fine, but let's be explicit).
        std::mem::forget(temp_buf);
        Ok(())
    }

    fn copy_gpu_to_memory_completed(&self, key: CacheKey, size: u32) -> Result<(), DispatcherError> {
        self.ensure_initialized()?;

        let dm = self
            .dispatch_map
            .get()
            .map_err(|_| DispatcherError::NotInitialized("dispatch_map not bound".into()))?;

        let mt = self
            .memory_tier
            .get()
            .map_err(|_| DispatcherError::NotInitialized("memory_tier not bound".into()))?;

        let (mem_ptr, _) = mt.get(key).ok_or(DispatcherError::KeyNotFound(key))?;

        dm.create_memory_tier_entry(key, mem_ptr, size)
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

        dm.downgrade_reference(key)
            .map_err(|e| DispatcherError::IoError(e.to_string()))?;

        let num_drives = {
            let dd = self.data_drives.read();
            dd.len().max(1)
        };
        let guard = self.bg_writer.lock().unwrap();
        if let Some(ref writer) = *guard {
            let _ = writer.enqueue(WriteJob {
                key,
                size,
                device_index: Self::drive_index(key, num_drives),
            });
        }

        Ok(())
    }

    fn release_memory(&self, key: CacheKey) -> Result<(), DispatcherError> {
        self.ensure_initialized()?;

        let mt = self
            .memory_tier
            .get()
            .map_err(|_| DispatcherError::NotInitialized("memory_tier not bound".into()))?;

        // Idempotent — KeyNotFound is not an error.
        match mt.remove(key) {
            Ok(()) | Err(interfaces::MemoryTierError::KeyNotFound(_)) => Ok(()),
            Err(e) => Err(DispatcherError::IoError(e.to_string())),
        }
    }

    fn prepare_store(&self, key: CacheKey, size: u32) -> Result<Arc<DmaBuffer>, DispatcherError> {
        self.ensure_initialized()?;
        self.log_info(&format!("dispatcher: prepare_store key={key} size={size}"));

        if size == 0 {
            return Err(DispatcherError::InvalidParameter("size must be > 0".into()));
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
        let drives = self.data_drives.read();
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

        let extent_mgrs: Vec<Arc<dyn IExtentManager + Send + Sync>> =
            drives.iter().map(|d| Arc::clone(&d.extent_mgr)).collect();
        drop(drives);

        let aligned_size = (size as usize).next_multiple_of(block_size);

        // Reserve extent via extent manager (if available).
        let write_handle = if let Some(em) = extent_mgrs.get(drive_idx) {
            match em.reserve_extent(key, aligned_size as u32) {
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

        let drives = self.data_drives.read();
        let drive = drives.get(pending.drive_idx).ok_or_else(|| {
            DispatcherError::IoError("data drive not available for commit".into())
        })?;

        let block_size = drive.block_dev_iface.block_size() as usize;
        let block_dev_iface = Arc::clone(&drive.block_dev_iface);
        drop(drives);

        let block_offset = pending.write_handle.extent_offset();
        let start_lba = block_offset / block_size as u64;
        let total_bytes = pending.size as usize;

        Self::write_buffer_to_ssd(&*block_dev_iface, &pending.buffer, start_lba, total_bytes, true)?;

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

        dm.touch(key).map_err(|_| DispatcherError::KeyNotFound(key))
    }

    fn promote_to_memory_tier(&self, keys: &[CacheKey]) {
        let Ok(()) = self.ensure_initialized() else {
            return;
        };

        let Ok(dm) = self.dispatch_map.get() else {
            return;
        };
        let Ok(mt) = self.memory_tier.get() else {
            return;
        };
        let logger = self.logger.get().ok();

        struct ColdEntry {
            key: CacheKey,
            offset: u64,
            size: u32,
        }

        let mut cold_entries: Vec<ColdEntry> = Vec::new();

        for &key in keys {
            match dm.lookup(key) {
                Ok(LookupResult::BlockDevice { offset }) => {
                    let _ = dm.release_read(key);
                    match dm.entry_size(key) {
                        Ok(size) => cold_entries.push(ColdEntry { key, offset, size }),
                        Err(_) => {}
                    }
                }
                Ok(LookupResult::MemoryTier { .. }) => {
                    let _ = dm.release_read(key);
                    let _ = dm.touch(key);
                    mt.touch(key);
                }
                Ok(LookupResult::Staging { .. }) => {
                    let _ = dm.release_read(key);
                    let _ = dm.touch(key);
                }
                _ => {}
            }
        }

        if cold_entries.is_empty() {
            return;
        }

        let drives = self.data_drives.read();
        let num_drives = drives.len();

        if num_drives == 0 {
            for entry in &cold_entries {
                if Self::evict_for_space(&dm, &mt, entry.size, entry.key).is_err() {
                    continue;
                }
                match mt.insert(entry.key, entry.size) {
                    Ok(mem_ptr) => {
                        let _ = dm.remove(entry.key);
                        let _ = dm.create_memory_tier_entry(entry.key, mem_ptr, entry.size);
                        let _ = dm.convert_to_storage(entry.key, entry.offset);
                        let _ = dm.release_write(entry.key);
                    }
                    Err(_) => {}
                }
            }
            return;
        }

        let chunk_size = {
            let ring_guard = self.pipeline_ring.read();
            ring_guard.as_ref().map_or(131072, |r| r.chunk_size)
        };

        let mut per_drive: Vec<Vec<usize>> = vec![Vec::new(); num_drives];
        for (i, entry) in cold_entries.iter().enumerate() {
            let drive_idx = Self::drive_index(entry.key, num_drives);
            per_drive[drive_idx].push(i);
        }

        std::thread::scope(|s| {
            for (drive_idx, entry_indices) in per_drive.iter().enumerate() {
                if entry_indices.is_empty() {
                    continue;
                }

                let drive = &drives[drive_idx];
                let channels = match &drive.cached_channels {
                    Some(ch) => ch,
                    None => continue,
                };
                let block_dev = Arc::clone(&drive.block_dev_iface);
                let dm = &dm;
                let mt = &mt;
                let cold = &cold_entries;
                let logger = &logger;

                s.spawn(move || {
                    for &ci in entry_indices {
                        let entry = &cold[ci];
                        let block_size = block_dev.block_size() as u64;
                        let start_lba = entry.offset / block_size;

                        if Self::evict_for_space(dm, mt, entry.size, entry.key).is_err() {
                            continue;
                        }

                        let mem_ptr = match mt.insert(entry.key, entry.size) {
                            Ok(ptr) => ptr,
                            Err(_) => continue,
                        };

                        // SAFETY: mem_ptr is a valid SPDK-registered memory-tier slot.
                        let result = unsafe {
                            pipeline::pipelined_ssd_to_dram_only(
                                &*block_dev,
                                channels,
                                mem_ptr,
                                start_lba,
                                entry.size as usize,
                                chunk_size,
                                16,
                            )
                        };

                        if let Err(e) = result {
                            let _ = mt.remove(entry.key);
                            if let Some(ref log) = logger {
                                log.debug(&format!(
                                    "promote_to_memory_tier: SSD read failed for key {}: {e}",
                                    entry.key
                                ));
                            }
                            continue;
                        }

                        let _ = dm.remove(entry.key);
                        let _ = dm.create_memory_tier_entry(entry.key, mem_ptr, entry.size);
                        let _ = dm.convert_to_storage(entry.key, entry.offset);
                        let _ = dm.release_write(entry.key);
                    }
                });
            }
        });
    }

    fn clear_memory_tier(&self) -> Result<usize, DispatcherError> {
        self.ensure_initialized()?;

        let dm = self
            .dispatch_map
            .get()
            .map_err(|_| DispatcherError::NotInitialized("dispatch_map not bound".into()))?;

        let mt = self
            .memory_tier
            .get()
            .map_err(|_| DispatcherError::NotInitialized("memory_tier not bound".into()))?;

        let mut count = 0;
        while let Some(key) = mt.evict_lru() {
            if dm.convert_memory_tier_to_block(key).is_err() {
                let _ = dm.remove(key);
            }
            count += 1;
        }
        Ok(count)
    }

    fn flush_to_ssd(&self) -> Result<usize, DispatcherError> {
        self.ensure_initialized()?;

        // Block until the background writer has processed all enqueued jobs.
        let flushed = if let Some(ref writer) = *self.bg_writer.lock().unwrap() {
            let before = writer.in_flight();
            writer.flush();
            before
        } else {
            0
        };

        Ok(flushed)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use component_core::query_interface;
    use std::collections::{HashMap, HashSet};
    use std::sync::Arc;
    use std::thread;

    use interfaces::{
        CacheKey, DispatchMapError, DmaAllocFn, DmaBuffer, GpuDeviceInfo, GpuDmaBuffer,
        GpuIpcHandle, GpuStream, IMemoryTier, LookupResult, MemoryTierError,
    };

    // -----------------------------------------------------------------------
    // Test infrastructure
    // -----------------------------------------------------------------------

    unsafe extern "C" fn dma_free(ptr: *mut std::ffi::c_void) {
        // SAFETY: ptr was allocated with libc::aligned_alloc in alloc_dma_buffer.
        unsafe { libc::free(ptr) };
    }

    fn alloc_dma_buffer(size: usize) -> Arc<DmaBuffer> {
        let sz = size.max(4096);
        let aligned_sz = sz.next_multiple_of(4096);
        let ptr = unsafe { libc::aligned_alloc(4096, aligned_sz) };
        assert!(
            !ptr.is_null(),
            "aligned_alloc failed for {aligned_sz} bytes"
        );
        unsafe { std::ptr::write_bytes(ptr as *mut u8, 0, aligned_sz) };
        let buf = unsafe { DmaBuffer::from_raw(ptr, aligned_sz, dma_free, -1) }.unwrap();
        Arc::new(buf)
    }

    // --- MockMemoryTier ---

    struct MockMtSlot {
        offset: usize,
        size: u32,
    }

    struct MockMemoryTier {
        inner: Mutex<MockMtInner>,
    }

    struct MockMtInner {
        pool: Vec<u8>,
        slots: HashMap<CacheKey, MockMtSlot>,
        used: usize,
        capacity: usize,
        fail_insert: bool,
    }

    impl MockMemoryTier {
        fn new(capacity: usize) -> Self {
            Self {
                inner: Mutex::new(MockMtInner {
                    pool: vec![0u8; capacity],
                    slots: HashMap::new(),
                    used: 0,
                    capacity,
                    fail_insert: false,
                }),
            }
        }

        fn with_fail_insert(capacity: usize) -> Self {
            Self {
                inner: Mutex::new(MockMtInner {
                    pool: vec![0u8; capacity],
                    slots: HashMap::new(),
                    used: 0,
                    capacity,
                    fail_insert: true,
                }),
            }
        }
    }

    impl IMemoryTier for MockMemoryTier {
        fn initialize(&self, _pool_size: usize, _numa_node: Option<i32>) -> Result<(), MemoryTierError> {
            Ok(())
        }

        fn insert(&self, key: CacheKey, size: u32) -> Result<*mut u8, MemoryTierError> {
            let mut inner = self.inner.lock().unwrap();
            if inner.fail_insert {
                return Err(MemoryTierError::PoolFull);
            }
            if inner.slots.contains_key(&key) {
                return Err(MemoryTierError::AlreadyExists(key));
            }
            let aligned = (size as usize).next_multiple_of(4096);
            if inner.used + aligned > inner.capacity {
                return Err(MemoryTierError::PoolFull);
            }
            let offset = inner.used;
            inner.used += aligned;
            inner.slots.insert(key, MockMtSlot { offset, size });
            let ptr = unsafe { inner.pool.as_mut_ptr().add(offset) };
            Ok(ptr)
        }

        fn get(&self, key: CacheKey) -> Option<(*mut u8, u32)> {
            let inner = self.inner.lock().unwrap();
            inner.slots.get(&key).map(|slot| {
                let ptr = unsafe { (inner.pool.as_ptr() as *mut u8).add(slot.offset) };
                (ptr, slot.size)
            })
        }

        fn peek(&self, key: CacheKey) -> Option<(*mut u8, u32)> {
            self.get(key)
        }

        fn oldest_keys(&self, n: usize) -> Vec<CacheKey> {
            let inner = self.inner.lock().unwrap();
            inner.slots.keys().take(n).copied().collect()
        }

        fn evict_lru(&self) -> Option<CacheKey> {
            let mut inner = self.inner.lock().unwrap();
            let key = inner.slots.keys().next().copied()?;
            let slot = inner.slots.remove(&key).unwrap();
            let aligned = (slot.size as usize).next_multiple_of(4096);
            inner.used = inner.used.saturating_sub(aligned);
            Some(key)
        }

        fn evict_lru_for_key(&self, _key: CacheKey) -> Option<CacheKey> {
            self.evict_lru()
        }

        fn remove(&self, key: CacheKey) -> Result<(), MemoryTierError> {
            let mut inner = self.inner.lock().unwrap();
            match inner.slots.remove(&key) {
                Some(slot) => {
                    let aligned = (slot.size as usize).next_multiple_of(4096);
                    inner.used = inner.used.saturating_sub(aligned);
                    Ok(())
                }
                None => Err(MemoryTierError::KeyNotFound(key)),
            }
        }

        fn touch(&self, _key: CacheKey) {}
        fn batch_touch(&self, _keys: &[CacheKey]) {}

        fn contains(&self, key: CacheKey) -> bool {
            self.inner.lock().unwrap().slots.contains_key(&key)
        }

        fn capacity(&self) -> usize {
            self.inner.lock().unwrap().capacity
        }

        fn used(&self) -> usize {
            self.inner.lock().unwrap().used
        }

        fn pool_info(&self) -> Option<(*mut u8, usize)> {
            let inner = self.inner.lock().unwrap();
            Some((inner.pool.as_ptr() as *mut u8, inner.capacity))
        }

        fn clear(&self) -> Result<usize, MemoryTierError> {
            let mut inner = self.inner.lock().unwrap();
            let count = inner.slots.len();
            inner.slots.clear();
            inner.used = 0;
            Ok(count)
        }

        fn is_dma_capable(&self) -> bool {
            false
        }
    }

    // --- MockDispatchMap ---

    enum MockEntryLocation {
        Staging {
            buffer: Arc<DmaBuffer>,
        },
        MemoryTier {
            pointer: *mut u8,
            size: u32,
            ssd_offset: Option<u64>,
        },
    }

    // SAFETY: pointers in MemoryTier refer to MockMemoryTier pool (test-only).
    unsafe impl Send for MockEntryLocation {}
    unsafe impl Sync for MockEntryLocation {}

    struct MockEntry {
        location: MockEntryLocation,
        write_ref: bool,
        read_refs: u32,
    }

    struct MockDmInner {
        entries: HashMap<CacheKey, MockEntry>,
        mismatch_keys: HashSet<CacheKey>,
    }

    struct MockDispatchMap {
        inner: Mutex<MockDmInner>,
    }

    impl MockDispatchMap {
        fn new() -> Self {
            Self {
                inner: Mutex::new(MockDmInner {
                    entries: HashMap::new(),
                    mismatch_keys: HashSet::new(),
                }),
            }
        }

        fn entry_count(&self) -> usize {
            self.inner.lock().unwrap().entries.len()
        }

        fn set_mismatch_key(&self, key: CacheKey) {
            self.inner.lock().unwrap().mismatch_keys.insert(key);
        }

        fn convert_entry_to_block(&self, key: CacheKey, offset: u64) {
            let mut inner = self.inner.lock().unwrap();
            if let Some(entry) = inner.entries.get_mut(&key) {
                entry.location = MockEntryLocation::MemoryTier {
                    pointer: std::ptr::null_mut(),
                    size: 0,
                    ssd_offset: Some(offset),
                };
            }
        }
    }

    impl IDispatchMap for MockDispatchMap {
        fn set_dma_alloc(&self, _alloc: DmaAllocFn) {}

        fn initialize(&self) -> Result<(), DispatchMapError> {
            Ok(())
        }

        fn create_staging(
            &self,
            key: CacheKey,
            size: u32,
        ) -> Result<Arc<DmaBuffer>, DispatchMapError> {
            let mut inner = self.inner.lock().unwrap();
            if inner.entries.contains_key(&key) {
                return Err(DispatchMapError::AlreadyExists(key));
            }
            let buffer = alloc_dma_buffer(size as usize * 4096);
            inner.entries.insert(
                key,
                MockEntry {
                    location: MockEntryLocation::Staging {
                        buffer: Arc::clone(&buffer),
                    },
                    write_ref: true,
                    read_refs: 0,
                },
            );
            Ok(buffer)
        }

        fn lookup(&self, key: CacheKey) -> Result<LookupResult, DispatchMapError> {
            let inner = self.inner.lock().unwrap();
            if inner.mismatch_keys.contains(&key) {
                return Ok(LookupResult::MismatchSize);
            }
            match inner.entries.get(&key) {
                None => Ok(LookupResult::NotExist),
                Some(entry) => match &entry.location {
                    MockEntryLocation::Staging { buffer } => Ok(LookupResult::Staging {
                        buffer: Arc::clone(buffer),
                    }),
                    MockEntryLocation::MemoryTier {
                        pointer,
                        size,
                        ssd_offset,
                    } => match ssd_offset {
                        Some(offset) if pointer.is_null() => {
                            Ok(LookupResult::BlockDevice { offset: *offset })
                        }
                        _ => Ok(LookupResult::MemoryTier {
                            pointer: *pointer,
                            size: *size,
                        }),
                    },
                },
            }
        }

        fn convert_to_storage(&self, key: CacheKey, offset: u64) -> Result<(), DispatchMapError> {
            let mut inner = self.inner.lock().unwrap();
            match inner.entries.get_mut(&key) {
                None => Err(DispatchMapError::KeyNotFound(key)),
                Some(entry) => {
                    match &mut entry.location {
                        MockEntryLocation::MemoryTier { ssd_offset, .. } => {
                            *ssd_offset = Some(offset);
                        }
                        MockEntryLocation::Staging { .. } => {
                            entry.location = MockEntryLocation::MemoryTier {
                                pointer: std::ptr::null_mut(),
                                size: 0,
                                ssd_offset: Some(offset),
                            };
                        }
                    }
                    Ok(())
                }
            }
        }

        fn take_read(&self, key: CacheKey) -> Result<(), DispatchMapError> {
            let mut inner = self.inner.lock().unwrap();
            match inner.entries.get_mut(&key) {
                None => Err(DispatchMapError::KeyNotFound(key)),
                Some(entry) => {
                    entry.read_refs += 1;
                    Ok(())
                }
            }
        }

        fn take_write(&self, key: CacheKey) -> Result<(), DispatchMapError> {
            let mut inner = self.inner.lock().unwrap();
            match inner.entries.get_mut(&key) {
                None => Err(DispatchMapError::KeyNotFound(key)),
                Some(entry) => {
                    if entry.write_ref {
                        return Err(DispatchMapError::ActiveReferences(key));
                    }
                    entry.write_ref = true;
                    Ok(())
                }
            }
        }

        fn release_read(&self, key: CacheKey) -> Result<(), DispatchMapError> {
            let mut inner = self.inner.lock().unwrap();
            match inner.entries.get_mut(&key) {
                None => Err(DispatchMapError::KeyNotFound(key)),
                Some(entry) => {
                    entry.read_refs = entry.read_refs.saturating_sub(1);
                    Ok(())
                }
            }
        }

        fn release_write(&self, key: CacheKey) -> Result<(), DispatchMapError> {
            let mut inner = self.inner.lock().unwrap();
            match inner.entries.get_mut(&key) {
                None => Err(DispatchMapError::KeyNotFound(key)),
                Some(entry) => {
                    entry.write_ref = false;
                    Ok(())
                }
            }
        }

        fn downgrade_reference(&self, key: CacheKey) -> Result<(), DispatchMapError> {
            let mut inner = self.inner.lock().unwrap();
            match inner.entries.get_mut(&key) {
                None => Err(DispatchMapError::NoWriteReference(key)),
                Some(entry) => {
                    entry.write_ref = false;
                    entry.read_refs += 1;
                    Ok(())
                }
            }
        }

        fn remove(&self, key: CacheKey) -> Result<(), DispatchMapError> {
            let mut inner = self.inner.lock().unwrap();
            match inner.entries.get(&key) {
                None => Err(DispatchMapError::KeyNotFound(key)),
                Some(entry) => {
                    if entry.read_refs > 0 || entry.write_ref {
                        return Err(DispatchMapError::ActiveReferences(key));
                    }
                    inner.entries.remove(&key);
                    Ok(())
                }
            }
        }

        fn touch(&self, key: CacheKey) -> Result<(), DispatchMapError> {
            let inner = self.inner.lock().unwrap();
            if inner.entries.contains_key(&key) {
                Ok(())
            } else {
                Err(DispatchMapError::KeyNotFound(key))
            }
        }

        fn entry_size(&self, key: CacheKey) -> Result<u32, DispatchMapError> {
            let inner = self.inner.lock().unwrap();
            if inner.entries.contains_key(&key) {
                Ok(4096)
            } else {
                Err(DispatchMapError::KeyNotFound(key))
            }
        }

        fn oldest_keys(&self, n: usize) -> Vec<CacheKey> {
            let inner = self.inner.lock().unwrap();
            inner.entries.keys().copied().take(n).collect()
        }

        fn create_memory_tier_entry(
            &self,
            key: CacheKey,
            pointer: *mut u8,
            size: u32,
        ) -> Result<(), DispatchMapError> {
            let mut inner = self.inner.lock().unwrap();
            if inner.entries.contains_key(&key) {
                return Err(DispatchMapError::AlreadyExists(key));
            }
            inner.entries.insert(
                key,
                MockEntry {
                    location: MockEntryLocation::MemoryTier {
                        pointer,
                        size,
                        ssd_offset: None,
                    },
                    write_ref: true,
                    read_refs: 0,
                },
            );
            Ok(())
        }

        fn convert_memory_tier_to_block(&self, key: CacheKey) -> Result<(), DispatchMapError> {
            let mut inner = self.inner.lock().unwrap();
            match inner.entries.get_mut(&key) {
                None => Err(DispatchMapError::KeyNotFound(key)),
                Some(entry) => match &entry.location {
                    MockEntryLocation::MemoryTier {
                        ssd_offset: Some(offset),
                        ..
                    } => {
                        let off = *offset;
                        entry.location = MockEntryLocation::MemoryTier {
                            pointer: std::ptr::null_mut(),
                            size: 0,
                            ssd_offset: Some(off),
                        };
                        Ok(())
                    }
                    _ => Err(DispatchMapError::InvalidState("no ssd_offset set".into())),
                },
            }
        }

        fn is_evictable(&self, key: CacheKey) -> bool {
            let inner = self.inner.lock().unwrap();
            match inner.entries.get(&key) {
                Some(entry) => matches!(
                    entry.location,
                    MockEntryLocation::MemoryTier {
                        ssd_offset: Some(_),
                        ..
                    }
                ),
                None => false,
            }
        }

        fn recover_extent(
            &self,
            key: CacheKey,
            offset: u64,
            _size_blocks: u32,
        ) -> Result<(), DispatchMapError> {
            let mut inner = self.inner.lock().unwrap();
            if inner.entries.contains_key(&key) {
                return Err(DispatchMapError::AlreadyExists(key));
            }
            inner.entries.insert(
                key,
                MockEntry {
                    location: MockEntryLocation::MemoryTier {
                        pointer: std::ptr::null_mut(),
                        size: 0,
                        ssd_offset: Some(offset),
                    },
                    write_ref: false,
                    read_refs: 0,
                },
            );
            Ok(())
        }
    }

    struct MockLogger;

    impl ILogger for MockLogger {
        fn error(&self, _msg: &str) {}
        fn warn(&self, _msg: &str) {}
        fn info(&self, _msg: &str) {}
        fn debug(&self, _msg: &str) {}
    }

    struct MockGpuServices;

    impl IGpuServices for MockGpuServices {
        fn initialize(&self) -> Result<(), String> {
            Ok(())
        }
        fn shutdown(&self) -> Result<(), String> {
            Ok(())
        }
        fn get_devices(&self) -> Result<Vec<GpuDeviceInfo>, String> {
            Ok(vec![])
        }
        fn deserialize_ipc_handle(&self, _base64_payload: &str) -> Result<GpuIpcHandle, String> {
            Err("mock: not implemented".into())
        }
        fn verify_memory(&self, _handle: &GpuIpcHandle) -> Result<(), String> {
            Ok(())
        }
        fn pin_memory(&self, _handle: &GpuIpcHandle) -> Result<(), String> {
            Ok(())
        }
        fn unpin_memory(&self, _handle: &GpuIpcHandle) -> Result<(), String> {
            Ok(())
        }
        fn create_dma_buffer(&self, _handle: GpuIpcHandle) -> Result<GpuDmaBuffer, String> {
            Err("mock: not implemented".into())
        }
        fn dma_copy_to_host(
            &self,
            src: *const std::ffi::c_void,
            dst: &DmaBuffer,
            size: usize,
        ) -> Result<(), String> {
            // SAFETY: src is a valid host pointer (from IpcHandle) and dst is a valid DmaBuffer.
            unsafe {
                std::ptr::copy_nonoverlapping(src as *const u8, dst.as_ptr() as *mut u8, size);
            }
            Ok(())
        }
        fn dma_copy_to_device(
            &self,
            src: &DmaBuffer,
            dst: *mut std::ffi::c_void,
            size: usize,
        ) -> Result<(), String> {
            // SAFETY: src is a valid DmaBuffer and dst is a valid host pointer (from IpcHandle).
            unsafe {
                std::ptr::copy_nonoverlapping(src.as_ptr() as *const u8, dst as *mut u8, size);
            }
            Ok(())
        }
        fn prepare_memory_for_spdk(
            &self,
            _base64_payload: &str,
            _device_index: Option<u32>,
        ) -> Result<DmaBuffer, String> {
            Err("mock: not implemented".into())
        }
        fn create_stream(&self) -> Result<GpuStream, String> {
            Ok(GpuStream(0x1 as *mut std::ffi::c_void))
        }
        fn stream_query(&self, _stream: GpuStream) -> Result<bool, String> {
            Ok(true)
        }
        fn destroy_stream(&self, _stream: GpuStream) -> Result<(), String> {
            Ok(())
        }
        fn stream_synchronize(&self, _stream: GpuStream) -> Result<(), String> {
            Ok(())
        }
        fn dma_copy_to_device_async(
            &self,
            src: &DmaBuffer,
            dst: *mut std::ffi::c_void,
            size: usize,
            _stream: GpuStream,
        ) -> Result<(), String> {
            // SAFETY: In tests, both src and dst are valid host pointers.
            unsafe {
                std::ptr::copy_nonoverlapping(src.as_ptr() as *const u8, dst as *mut u8, size);
            }
            Ok(())
        }
        fn memcpy_h2d_async(
            &self,
            src: *const std::ffi::c_void,
            dst: *mut std::ffi::c_void,
            size: usize,
            _stream: GpuStream,
        ) -> Result<(), String> {
            unsafe {
                std::ptr::copy_nonoverlapping(src as *const u8, dst as *mut u8, size);
            }
            Ok(())
        }
        fn dma_copy_to_host_async(
            &self,
            src: *const std::ffi::c_void,
            dst: &DmaBuffer,
            size: usize,
            _stream: GpuStream,
        ) -> Result<(), String> {
            unsafe {
                std::ptr::copy_nonoverlapping(src as *const u8, dst.as_ptr() as *mut u8, size);
            }
            Ok(())
        }
        fn memcpy_d2h_async(
            &self,
            src: *const std::ffi::c_void,
            dst: *mut std::ffi::c_void,
            size: usize,
            _stream: GpuStream,
        ) -> Result<(), String> {
            unsafe {
                std::ptr::copy_nonoverlapping(src as *const u8, dst as *mut u8, size);
            }
            Ok(())
        }
        fn allocate_pinned_dma_buffer(&self, size: usize) -> Result<DmaBuffer, String> {
            DmaBuffer::new(size, 4096, None).map_err(|e| e.to_string())
        }
        fn register_host_memory(
            &self,
            _ptr: *mut std::ffi::c_void,
            _size: usize,
        ) -> Result<(), String> {
            Ok(())
        }
        fn unregister_host_memory(
            &self,
            _ptr: *mut std::ffi::c_void,
            _size: usize,
        ) -> Result<(), String> {
            Ok(())
        }
    }

    fn setup_initialized() -> (Arc<DispatcherP2pComponent>, Arc<MockDispatchMap>) {
        let dm = Arc::new(MockDispatchMap::new());
        let logger: Arc<dyn ILogger + Send + Sync> = Arc::new(MockLogger);
        let gpu: Arc<dyn IGpuServices + Send + Sync> = Arc::new(MockGpuServices);
        let mt: Arc<dyn IMemoryTier + Send + Sync> = Arc::new(MockMemoryTier::new(1024 * 1024));
        let c = DispatcherP2pComponent::new(
            AtomicBool::new(false),
            Mutex::new(None),
            Mutex::new(None),
            Mutex::new(None),
            RwLock::new(Vec::new()),
            Mutex::new(HashMap::new()),
            RwLock::new(None),
            RwLock::new(None),
            AtomicU64::new(0),
            Mutex::new(None),
            Mutex::new(None),
        );
        c.dispatch_map
            .connect(Arc::clone(&dm) as Arc<dyn IDispatchMap + Send + Sync>)
            .unwrap();
        c.logger.connect(logger).unwrap();
        c.gpu_services.connect(gpu).unwrap();
        c.memory_tier.connect(mt).unwrap();

        let d = query_interface!(c, IDispatcher).unwrap();
        d.initialize(DispatcherConfig {
            data_pci_addrs: vec!["0000:02:00.0".to_string()],
            ..Default::default()
        })
        .unwrap();

        (c, dm)
    }

    fn make_handle(buf: &mut [u8]) -> IpcHandle {
        IpcHandle {
            address: buf.as_mut_ptr(),
            size: buf.len() as u32,
        }
    }

    // -----------------------------------------------------------------------
    // Pre-initialization tests (existing)
    // -----------------------------------------------------------------------

    #[test]
    fn component_creation() {
        let _c = DispatcherP2pComponent::new(
            AtomicBool::new(false),
            Mutex::new(None),
            Mutex::new(None),
            Mutex::new(None),
            RwLock::new(Vec::new()),
            Mutex::new(HashMap::new()),
            RwLock::new(None),
            RwLock::new(None),
            AtomicU64::new(0),
            Mutex::new(None),
            Mutex::new(None),
        );
    }

    #[test]
    fn query_idispatcher() {
        let c = DispatcherP2pComponent::new(
            AtomicBool::new(false),
            Mutex::new(None),
            Mutex::new(None),
            Mutex::new(None),
            RwLock::new(Vec::new()),
            Mutex::new(HashMap::new()),
            RwLock::new(None),
            RwLock::new(None),
            AtomicU64::new(0),
            Mutex::new(None),
            Mutex::new(None),
        );
        let d = query_interface!(c, IDispatcher);
        assert!(d.is_some());
    }

    #[test]
    fn initialize_without_receptacles_fails() {
        let c = DispatcherP2pComponent::new(
            AtomicBool::new(false),
            Mutex::new(None),
            Mutex::new(None),
            Mutex::new(None),
            RwLock::new(Vec::new()),
            Mutex::new(HashMap::new()),
            RwLock::new(None),
            RwLock::new(None),
            AtomicU64::new(0),
            Mutex::new(None),
            Mutex::new(None),
        );
        let d = query_interface!(c, IDispatcher).unwrap();
        let config = DispatcherConfig {
            data_pci_addrs: vec!["0000:02:00.0".to_string()],
            ..Default::default()
        };
        let err = d.initialize(config);
        assert!(matches!(err, Err(DispatcherError::NotInitialized(_))));
    }

    #[test]
    fn initialize_with_empty_pci_addrs_fails() {
        let c = DispatcherP2pComponent::new(
            AtomicBool::new(false),
            Mutex::new(None),
            Mutex::new(None),
            Mutex::new(None),
            RwLock::new(Vec::new()),
            Mutex::new(HashMap::new()),
            RwLock::new(None),
            RwLock::new(None),
            AtomicU64::new(0),
            Mutex::new(None),
            Mutex::new(None),
        );
        let d = query_interface!(c, IDispatcher).unwrap();
        let config = DispatcherConfig {
            data_pci_addrs: vec![],
            ..Default::default()
        };
        // This will fail with NotInitialized since dispatch_map isn't bound
        let err = d.initialize(config);
        assert!(err.is_err());
    }

    #[test]
    fn lookup_before_initialize_fails() {
        let c = DispatcherP2pComponent::new(
            AtomicBool::new(false),
            Mutex::new(None),
            Mutex::new(None),
            Mutex::new(None),
            RwLock::new(Vec::new()),
            Mutex::new(HashMap::new()),
            RwLock::new(None),
            RwLock::new(None),
            AtomicU64::new(0),
            Mutex::new(None),
            Mutex::new(None),
        );
        let d = query_interface!(c, IDispatcher).unwrap();
        let mut buf = vec![0u8; 4096];
        let handle = IpcHandle {
            address: buf.as_mut_ptr(),
            size: 4096,
        };
        let err = d.lookup(42, handle);
        assert!(matches!(err, Err(DispatcherError::NotInitialized(_))));
    }

    #[test]
    fn check_before_initialize_fails() {
        let c = DispatcherP2pComponent::new(
            AtomicBool::new(false),
            Mutex::new(None),
            Mutex::new(None),
            Mutex::new(None),
            RwLock::new(Vec::new()),
            Mutex::new(HashMap::new()),
            RwLock::new(None),
            RwLock::new(None),
            AtomicU64::new(0),
            Mutex::new(None),
            Mutex::new(None),
        );
        let d = query_interface!(c, IDispatcher).unwrap();
        let err = d.check(42);
        assert!(matches!(err, Err(DispatcherError::NotInitialized(_))));
    }

    #[test]
    fn remove_before_initialize_fails() {
        let c = DispatcherP2pComponent::new(
            AtomicBool::new(false),
            Mutex::new(None),
            Mutex::new(None),
            Mutex::new(None),
            RwLock::new(Vec::new()),
            Mutex::new(HashMap::new()),
            RwLock::new(None),
            RwLock::new(None),
            AtomicU64::new(0),
            Mutex::new(None),
            Mutex::new(None),
        );
        let d = query_interface!(c, IDispatcher).unwrap();
        let err = d.remove(42);
        assert!(matches!(err, Err(DispatcherError::NotInitialized(_))));
    }

    #[test]
    fn populate_before_initialize_fails() {
        let c = DispatcherP2pComponent::new(
            AtomicBool::new(false),
            Mutex::new(None),
            Mutex::new(None),
            Mutex::new(None),
            RwLock::new(Vec::new()),
            Mutex::new(HashMap::new()),
            RwLock::new(None),
            RwLock::new(None),
            AtomicU64::new(0),
            Mutex::new(None),
            Mutex::new(None),
        );
        let d = query_interface!(c, IDispatcher).unwrap();
        let mut buf = vec![0u8; 4096];
        let handle = IpcHandle {
            address: buf.as_mut_ptr(),
            size: 4096,
        };
        let err = d.populate(42, handle);
        assert!(matches!(err, Err(DispatcherError::NotInitialized(_))));
    }

    #[test]
    fn populate_with_zero_size_fails() {
        let c = DispatcherP2pComponent::new(
            AtomicBool::new(false),
            Mutex::new(None),
            Mutex::new(None),
            Mutex::new(None),
            RwLock::new(Vec::new()),
            Mutex::new(HashMap::new()),
            RwLock::new(None),
            RwLock::new(None),
            AtomicU64::new(0),
            Mutex::new(None),
            Mutex::new(None),
        );
        let d = query_interface!(c, IDispatcher).unwrap();
        // Even though not initialized, zero-size check comes after init check.
        // This test verifies the parameter validation exists in the code path.
        let mut buf = vec![0u8; 0];
        let handle = IpcHandle {
            address: buf.as_mut_ptr(),
            size: 0,
        };
        let err = d.populate(42, handle);
        // Will fail with NotInitialized since that check comes first
        assert!(err.is_err());
    }

    #[test]
    fn shutdown_without_initialize_succeeds() {
        let c = DispatcherP2pComponent::new(
            AtomicBool::new(false),
            Mutex::new(None),
            Mutex::new(None),
            Mutex::new(None),
            RwLock::new(Vec::new()),
            Mutex::new(HashMap::new()),
            RwLock::new(None),
            RwLock::new(None),
            AtomicU64::new(0),
            Mutex::new(None),
            Mutex::new(None),
        );
        let d = query_interface!(c, IDispatcher).unwrap();
        assert!(d.shutdown().is_ok());
    }

    #[test]
    fn double_shutdown_succeeds() {
        let c = DispatcherP2pComponent::new(
            AtomicBool::new(false),
            Mutex::new(None),
            Mutex::new(None),
            Mutex::new(None),
            RwLock::new(Vec::new()),
            Mutex::new(HashMap::new()),
            RwLock::new(None),
            RwLock::new(None),
            AtomicU64::new(0),
            Mutex::new(None),
            Mutex::new(None),
        );
        let d = query_interface!(c, IDispatcher).unwrap();
        assert!(d.shutdown().is_ok());
        assert!(d.shutdown().is_ok());
    }

    #[test]
    fn concurrent_pre_init_calls_from_multiple_threads() {
        let c = Arc::new(DispatcherP2pComponent::new(
            AtomicBool::new(false),
            Mutex::new(None),
            Mutex::new(None),
            Mutex::new(None),
            RwLock::new(Vec::new()),
            Mutex::new(HashMap::new()),
            RwLock::new(None),
            RwLock::new(None),
            AtomicU64::new(0),
            Mutex::new(None),
            Mutex::new(None),
        ));

        let handles: Vec<_> = (0..4)
            .map(|_| {
                let comp = Arc::clone(&c);
                thread::spawn(move || {
                    let d = query_interface!(comp, IDispatcher).unwrap();
                    assert!(matches!(
                        d.check(1),
                        Err(DispatcherError::NotInitialized(_))
                    ));
                    assert!(matches!(
                        d.remove(1),
                        Err(DispatcherError::NotInitialized(_))
                    ));
                })
            })
            .collect();

        for h in handles {
            h.join().unwrap();
        }
    }

    // -----------------------------------------------------------------------
    // Initialized dispatcher tests (with mock dispatch map)
    // -----------------------------------------------------------------------

    #[test]
    fn initialize_with_dispatch_map_succeeds() {
        let (c, _dm) = setup_initialized();
        let d = query_interface!(c, IDispatcher).unwrap();
        assert!(d.shutdown().is_ok());
    }

    #[test]
    fn initialize_empty_addrs_with_dispatch_map() {
        let dm: Arc<dyn IDispatchMap + Send + Sync> = Arc::new(MockDispatchMap::new());
        let mt: Arc<dyn IMemoryTier + Send + Sync> = Arc::new(MockMemoryTier::new(1024 * 1024));
        let c = DispatcherP2pComponent::new(
            AtomicBool::new(false),
            Mutex::new(None),
            Mutex::new(None),
            Mutex::new(None),
            RwLock::new(Vec::new()),
            Mutex::new(HashMap::new()),
            RwLock::new(None),
            RwLock::new(None),
            AtomicU64::new(0),
            Mutex::new(None),
            Mutex::new(None),
        );
        c.dispatch_map.connect(dm).unwrap();
        c.memory_tier.connect(mt).unwrap();

        let d = query_interface!(c, IDispatcher).unwrap();
        let config = DispatcherConfig {
            data_pci_addrs: vec![],
            ..Default::default()
        };
        let err = d.initialize(config);
        assert!(matches!(err, Err(DispatcherError::InvalidParameter(_))));
    }

    #[test]
    fn initialize_multiple_pci_addrs() {
        let dm: Arc<dyn IDispatchMap + Send + Sync> = Arc::new(MockDispatchMap::new());
        let logger: Arc<dyn ILogger + Send + Sync> = Arc::new(MockLogger);
        let mt: Arc<dyn IMemoryTier + Send + Sync> = Arc::new(MockMemoryTier::new(1024 * 1024));
        let c = DispatcherP2pComponent::new(
            AtomicBool::new(false),
            Mutex::new(None),
            Mutex::new(None),
            Mutex::new(None),
            RwLock::new(Vec::new()),
            Mutex::new(HashMap::new()),
            RwLock::new(None),
            RwLock::new(None),
            AtomicU64::new(0),
            Mutex::new(None),
            Mutex::new(None),
        );
        c.dispatch_map.connect(dm).unwrap();
        c.logger.connect(logger).unwrap();
        c.memory_tier.connect(mt).unwrap();

        let d = query_interface!(c, IDispatcher).unwrap();
        d.initialize(DispatcherConfig {
            data_pci_addrs: vec![
                "0000:02:00.0".to_string(),
                "0000:03:00.0".to_string(),
                "0000:04:00.0".to_string(),
            ],
            ..Default::default()
        })
        .unwrap();
        d.shutdown().unwrap();
    }

    #[test]
    fn populate_succeeds_after_init() {
        let (c, dm) = setup_initialized();
        let d = query_interface!(c, IDispatcher).unwrap();
        let mut buf = vec![0u8; 4096];
        assert!(d.populate(1, make_handle(&mut buf)).is_ok());
        assert_eq!(dm.entry_count(), 1);
        d.shutdown().unwrap();
    }

    #[test]
    fn populate_zero_size_returns_invalid_parameter_after_init() {
        let (c, _dm) = setup_initialized();
        let d = query_interface!(c, IDispatcher).unwrap();
        let mut buf = vec![0u8; 0];
        let handle = IpcHandle {
            address: buf.as_mut_ptr(),
            size: 0,
        };
        let err = d.populate(1, handle);
        assert!(matches!(err, Err(DispatcherError::InvalidParameter(_))));
        d.shutdown().unwrap();
    }

    #[test]
    fn populate_duplicate_key_returns_already_exists() {
        let (c, _dm) = setup_initialized();
        let d = query_interface!(c, IDispatcher).unwrap();
        let mut buf1 = vec![0u8; 4096];
        d.populate(1, make_handle(&mut buf1)).unwrap();

        let mut buf2 = vec![0u8; 4096];
        let err = d.populate(1, make_handle(&mut buf2));
        assert!(matches!(err, Err(DispatcherError::AlreadyExists(1))));
        d.shutdown().unwrap();
    }

    #[test]
    fn populate_allocation_failure() {
        let dm: Arc<dyn IDispatchMap + Send + Sync> = Arc::new(MockDispatchMap::new());
        let logger: Arc<dyn ILogger + Send + Sync> = Arc::new(MockLogger);
        let gpu: Arc<dyn IGpuServices + Send + Sync> = Arc::new(MockGpuServices);
        let mt: Arc<dyn IMemoryTier + Send + Sync> =
            Arc::new(MockMemoryTier::with_fail_insert(1024 * 1024));
        let c = DispatcherP2pComponent::new(
            AtomicBool::new(false),
            Mutex::new(None),
            Mutex::new(None),
            Mutex::new(None),
            RwLock::new(Vec::new()),
            Mutex::new(HashMap::new()),
            RwLock::new(None),
            RwLock::new(None),
            AtomicU64::new(0),
            Mutex::new(None),
            Mutex::new(None),
        );
        c.dispatch_map.connect(dm).unwrap();
        c.logger.connect(logger).unwrap();
        c.gpu_services.connect(gpu).unwrap();
        c.memory_tier.connect(mt).unwrap();

        let d = query_interface!(c, IDispatcher).unwrap();
        d.initialize(DispatcherConfig {
            data_pci_addrs: vec!["0000:02:00.0".to_string()],
            ..Default::default()
        })
        .unwrap();

        let mut buf = vec![0u8; 4096];
        let err = d.populate(1, make_handle(&mut buf));
        assert!(matches!(err, Err(DispatcherError::AllocationFailed(_))));
        d.shutdown().unwrap();
    }

    #[test]
    fn populate_non_block_aligned_size() {
        let (c, _dm) = setup_initialized();
        let d = query_interface!(c, IDispatcher).unwrap();
        let mut buf = vec![0u8; 5000];
        let handle = IpcHandle {
            address: buf.as_mut_ptr(),
            size: 5000,
        };
        assert!(d.populate(1, handle).is_ok());
        d.shutdown().unwrap();
    }

    #[test]
    fn populate_enqueues_many_writes() {
        let (c, dm) = setup_initialized();
        let d = query_interface!(c, IDispatcher).unwrap();

        for i in 0..100 {
            let mut buf = vec![0u8; 4096];
            d.populate(i, make_handle(&mut buf)).unwrap();
        }
        assert_eq!(dm.entry_count(), 100);
        d.shutdown().unwrap();
    }

    #[test]
    fn lookup_memory_tier_hit() {
        let (c, _dm) = setup_initialized();
        let d = query_interface!(c, IDispatcher).unwrap();

        let mut buf = vec![0xABu8; 4096];
        d.populate(1, make_handle(&mut buf)).unwrap();

        let mut buf2 = vec![0u8; 4096];
        assert!(d.lookup(1, make_handle(&mut buf2)).is_ok());
        // Verify GPU received the data (mock copies bytes directly).
        assert_eq!(buf2[0], 0xAB);
        d.shutdown().unwrap();
    }

    #[test]
    fn lookup_block_device_promote_without_hardware() {
        let (c, dm) = setup_initialized();
        let d = query_interface!(c, IDispatcher).unwrap();

        let mut buf = vec![0u8; 4096];
        d.populate(1, make_handle(&mut buf)).unwrap();

        // Simulate eviction: remove from memory-tier and convert dispatch-map to BlockDevice.
        let mt = c.memory_tier.get().unwrap();
        let _ = mt.remove(1);
        dm.convert_entry_to_block(1, 0x1000);

        // Without hardware, promote_and_serve enters the no-drives path
        // which copies zeros to GPU and re-registers the entry.
        let mut buf2 = vec![0u8; 4096];
        let result = d.lookup(1, make_handle(&mut buf2));
        assert!(
            result.is_ok(),
            "promote without hardware should succeed, got: {result:?}"
        );
        d.shutdown().unwrap();
    }

    #[test]
    fn lookup_key_not_found() {
        let (c, _dm) = setup_initialized();
        let d = query_interface!(c, IDispatcher).unwrap();
        let mut buf = vec![0u8; 4096];
        let err = d.lookup(999, make_handle(&mut buf));
        assert!(matches!(err, Err(DispatcherError::KeyNotFound(999))));
        d.shutdown().unwrap();
    }

    #[test]
    fn lookup_mismatch_size_returns_invalid_parameter() {
        let (c, dm) = setup_initialized();
        let d = query_interface!(c, IDispatcher).unwrap();

        let mut buf = vec![0u8; 4096];
        d.populate(1, make_handle(&mut buf)).unwrap();

        dm.set_mismatch_key(1);

        let mut buf2 = vec![0u8; 4096];
        let err = d.lookup(1, make_handle(&mut buf2));
        assert!(matches!(err, Err(DispatcherError::InvalidParameter(_))));
        d.shutdown().unwrap();
    }

    #[test]
    fn check_existing_returns_true() {
        let (c, _dm) = setup_initialized();
        let d = query_interface!(c, IDispatcher).unwrap();
        let mut buf = vec![0u8; 4096];
        d.populate(1, make_handle(&mut buf)).unwrap();
        assert!(d.check(1).unwrap());
        d.shutdown().unwrap();
    }

    #[test]
    fn check_nonexistent_returns_false() {
        let (c, _dm) = setup_initialized();
        let d = query_interface!(c, IDispatcher).unwrap();
        assert!(!d.check(999).unwrap());
        d.shutdown().unwrap();
    }

    #[test]
    fn remove_existing_succeeds() {
        let (c, dm) = setup_initialized();
        let d = query_interface!(c, IDispatcher).unwrap();
        let mut buf = vec![0u8; 4096];
        d.populate(1, make_handle(&mut buf)).unwrap();
        assert_eq!(dm.entry_count(), 1);
        assert!(d.remove(1).is_ok());
        assert_eq!(dm.entry_count(), 0);
        d.shutdown().unwrap();
    }

    #[test]
    fn remove_nonexistent_returns_key_not_found() {
        let (c, _dm) = setup_initialized();
        let d = query_interface!(c, IDispatcher).unwrap();
        let err = d.remove(999);
        assert!(matches!(err, Err(DispatcherError::KeyNotFound(999))));
        d.shutdown().unwrap();
    }

    #[test]
    fn full_lifecycle_populate_check_remove() {
        let (c, dm) = setup_initialized();
        let d = query_interface!(c, IDispatcher).unwrap();

        let mut buf = vec![0u8; 8192];
        d.populate(42, make_handle(&mut buf)).unwrap();
        assert_eq!(dm.entry_count(), 1);

        assert!(d.check(42).unwrap());
        assert!(!d.check(99).unwrap());

        assert!(d.remove(42).is_ok());
        assert_eq!(dm.entry_count(), 0);

        assert!(!d.check(42).unwrap());

        d.shutdown().unwrap();
    }

    #[test]
    fn operations_after_shutdown_fail() {
        let (c, _dm) = setup_initialized();
        let d = query_interface!(c, IDispatcher).unwrap();
        d.shutdown().unwrap();

        let mut buf = vec![0u8; 4096];
        assert!(matches!(
            d.populate(1, make_handle(&mut buf)),
            Err(DispatcherError::NotInitialized(_))
        ));
        assert!(matches!(
            d.check(1),
            Err(DispatcherError::NotInitialized(_))
        ));
        let mut buf2 = vec![0u8; 4096];
        assert!(matches!(
            d.lookup(1, make_handle(&mut buf2)),
            Err(DispatcherError::NotInitialized(_))
        ));
        assert!(matches!(
            d.remove(1),
            Err(DispatcherError::NotInitialized(_))
        ));
    }

    #[test]
    fn reinitialize_after_shutdown() {
        let (c, _dm) = setup_initialized();
        let d = query_interface!(c, IDispatcher).unwrap();
        d.shutdown().unwrap();

        d.initialize(DispatcherConfig {
            data_pci_addrs: vec!["0000:02:00.0".to_string()],
            ..Default::default()
        })
        .unwrap();

        assert!(!d.check(1).unwrap());
        d.shutdown().unwrap();
    }

    #[test]
    fn concurrent_checks_on_initialized_dispatcher() {
        let (c, _dm) = setup_initialized();

        let handles: Vec<_> = (0..8)
            .map(|i| {
                let comp = Arc::clone(&c);
                thread::spawn(move || {
                    let d = query_interface!(comp, IDispatcher).unwrap();
                    for k in 0..10 {
                        let result = d.check(i * 100 + k);
                        assert!(result.is_ok());
                        assert!(!result.unwrap());
                    }
                })
            })
            .collect();

        for h in handles {
            h.join().unwrap();
        }

        let d = query_interface!(c, IDispatcher).unwrap();
        d.shutdown().unwrap();
    }

    #[test]
    fn concurrent_populate_different_keys() {
        let (c, dm) = setup_initialized();

        let handles: Vec<_> = (0..4)
            .map(|t| {
                let comp = Arc::clone(&c);
                thread::spawn(move || {
                    let d = query_interface!(comp, IDispatcher).unwrap();
                    for i in 0..5 {
                        let key = t * 100 + i;
                        let mut buf = vec![0u8; 4096];
                        d.populate(key, make_handle(&mut buf)).unwrap();
                    }
                })
            })
            .collect();

        for h in handles {
            h.join().unwrap();
        }

        assert_eq!(dm.entry_count(), 20);

        let d = query_interface!(c, IDispatcher).unwrap();
        d.shutdown().unwrap();
    }

    // -----------------------------------------------------------------------
    // Eviction tests (memory-tier pool pressure)
    // -----------------------------------------------------------------------

    #[test]
    fn evict_for_space_evicts_when_pool_full() {
        let dm: Arc<dyn IDispatchMap + Send + Sync> = Arc::new(MockDispatchMap::new());
        // Small pool: 16 KiB total (can hold 4 × 4 KiB entries).
        let mt: Arc<dyn IMemoryTier + Send + Sync> = Arc::new(MockMemoryTier::new(16384));

        // Insert 4 entries into the memory-tier directly.
        for key in 0..4u64 {
            mt.insert(key, 4096).unwrap();
            dm.create_memory_tier_entry(key, std::ptr::null_mut(), 4096)
                .unwrap();
            dm.release_write(key).unwrap();
            // Set ssd_offset so convert_memory_tier_to_block can succeed.
            dm.convert_to_storage(key, key * 4096).unwrap();
        }

        // Pool is now full (16384 used). Trying to add 4096 more should evict.
        DispatcherP2pComponent::evict_for_space(&dm, &mt, 4096, 100).unwrap();

        // At least one entry was evicted from memory-tier.
        assert!(mt.used() + 4096 <= mt.capacity());
    }

    #[test]
    fn evict_for_space_noop_when_space_available() {
        let dm: Arc<dyn IDispatchMap + Send + Sync> = Arc::new(MockDispatchMap::new());
        let mt: Arc<dyn IMemoryTier + Send + Sync> = Arc::new(MockMemoryTier::new(1024 * 1024));

        // Insert one 4 KiB entry.
        mt.insert(0, 4096).unwrap();
        dm.create_memory_tier_entry(0, std::ptr::null_mut(), 4096)
            .unwrap();
        dm.release_write(0).unwrap();

        // Plenty of space, no eviction needed.
        DispatcherP2pComponent::evict_for_space(&dm, &mt, 4096, 100).unwrap();

        assert!(mt.contains(0), "entry should not be evicted");
    }

    #[test]
    fn populate_triggers_eviction_on_full_pool() {
        let dm = Arc::new(MockDispatchMap::new());
        let logger: Arc<dyn ILogger + Send + Sync> = Arc::new(MockLogger);
        let gpu: Arc<dyn IGpuServices + Send + Sync> = Arc::new(MockGpuServices);
        // Pool can hold exactly 2 × 4 KiB entries.
        let mt: Arc<dyn IMemoryTier + Send + Sync> = Arc::new(MockMemoryTier::new(8192));
        let c = DispatcherP2pComponent::new(
            AtomicBool::new(false),
            Mutex::new(None),
            Mutex::new(None),
            Mutex::new(None),
            RwLock::new(Vec::new()),
            Mutex::new(HashMap::new()),
            RwLock::new(None),
            RwLock::new(None),
            AtomicU64::new(0),
            Mutex::new(None),
            Mutex::new(None),
        );
        c.dispatch_map
            .connect(Arc::clone(&dm) as Arc<dyn IDispatchMap + Send + Sync>)
            .unwrap();
        c.logger.connect(logger).unwrap();
        c.gpu_services.connect(gpu).unwrap();
        c.memory_tier.connect(mt).unwrap();

        let d = query_interface!(c, IDispatcher).unwrap();
        d.initialize(DispatcherConfig {
            data_pci_addrs: vec!["0000:02:00.0".to_string()],
            ..Default::default()
        })
        .unwrap();

        // Fill the pool with 2 entries.
        let mut buf = vec![0u8; 4096];
        d.populate(1, make_handle(&mut buf)).unwrap();
        let mut buf2 = vec![0u8; 4096];
        d.populate(2, make_handle(&mut buf2)).unwrap();

        // Third populate should trigger eviction of one entry and succeed.
        let mut buf3 = vec![0u8; 4096];
        d.populate(3, make_handle(&mut buf3)).unwrap();

        // Total entries in dispatch-map: at most 3 (one may have been converted to block).
        assert!(dm.entry_count() <= 3);

        d.shutdown().unwrap();
    }

    #[test]
    fn prepare_store_returns_dma_buffer() {
        let (c, _dm) = setup_initialized();
        let d = query_interface!(c, IDispatcher).unwrap();

        let dma_buf = d.prepare_store(99, 4096).unwrap();
        assert!(dma_buf.len() >= 4096);

        // The prepared key is visible via check().
        assert!(d.check(99).unwrap());

        // Duplicate prepare_store on the same key fails.
        let err = d.prepare_store(99, 4096);
        assert!(matches!(err, Err(DispatcherError::AlreadyExists(99))));

        d.shutdown().unwrap();
    }

    // -----------------------------------------------------------------------
    // Background SSD Evictor tests
    // -----------------------------------------------------------------------

    #[test]
    fn evictor_get_evictable_offset_block_device() {
        let dm = Arc::new(MockDispatchMap::new());
        let dm_iface: Arc<dyn IDispatchMap + Send + Sync> = Arc::clone(&dm) as _;

        // Insert an entry that looks like BlockDevice (null pointer + ssd_offset).
        dm.inner.lock().unwrap().entries.insert(
            1,
            MockEntry {
                location: MockEntryLocation::MemoryTier {
                    pointer: std::ptr::null_mut(),
                    size: 4096,
                    ssd_offset: Some(8192),
                },
                write_ref: false,
                read_refs: 0,
            },
        );

        let offset = crate::background::BackgroundEvictor::get_evictable_offset(&dm_iface, 1);
        assert_eq!(offset, Some(8192));
    }

    #[test]
    fn evictor_get_evictable_offset_skips_memory_tier() {
        let dm = Arc::new(MockDispatchMap::new());
        let dm_iface: Arc<dyn IDispatchMap + Send + Sync> = Arc::clone(&dm) as _;

        // Insert a MemoryTier entry (non-null pointer = still hot in DRAM).
        let mut buf = vec![0u8; 4096];
        dm.inner.lock().unwrap().entries.insert(
            2,
            MockEntry {
                location: MockEntryLocation::MemoryTier {
                    pointer: buf.as_mut_ptr(),
                    size: 4096,
                    ssd_offset: Some(16384),
                },
                write_ref: false,
                read_refs: 0,
            },
        );

        let offset = crate::background::BackgroundEvictor::get_evictable_offset(&dm_iface, 2);
        assert_eq!(offset, None, "memory-tier entries should not be evictable");
        std::mem::forget(buf);
    }

    #[test]
    fn evictor_get_evictable_offset_skips_nonexistent() {
        let dm = Arc::new(MockDispatchMap::new());
        let dm_iface: Arc<dyn IDispatchMap + Send + Sync> = Arc::clone(&dm) as _;

        let offset = crate::background::BackgroundEvictor::get_evictable_offset(&dm_iface, 99);
        assert_eq!(offset, None);
    }

    #[test]
    fn evictor_full_eviction_cycle() {
        let dm = Arc::new(MockDispatchMap::new());
        let dm_iface: Arc<dyn IDispatchMap + Send + Sync> = Arc::clone(&dm) as _;
        let mt = Arc::new(MockMemoryTier::new(1024 * 1024));
        let mt_iface: Arc<dyn IMemoryTier + Send + Sync> = Arc::clone(&mt) as _;

        // Insert 10 entries all in BlockDevice state (null pointer + ssd_offset).
        for key in 0..10u64 {
            dm.inner.lock().unwrap().entries.insert(
                key,
                MockEntry {
                    location: MockEntryLocation::MemoryTier {
                        pointer: std::ptr::null_mut(),
                        size: 4096,
                        ssd_offset: Some(key * 4096),
                    },
                    write_ref: false,
                    read_refs: 0,
                },
            );
        }

        assert_eq!(dm.entry_count(), 10);

        // Simulate evictor logic: get oldest keys, filter, remove.
        let candidates = dm_iface.oldest_keys(5);
        assert_eq!(candidates.len(), 5);

        for key in &candidates {
            let offset =
                crate::background::BackgroundEvictor::get_evictable_offset(&dm_iface, *key);
            assert!(offset.is_some(), "key {key} should be evictable");

            let _ = mt_iface.remove(*key);
            dm_iface.remove(*key).unwrap();
        }

        assert_eq!(
            dm.entry_count(),
            5,
            "5 entries should remain after evicting 5"
        );
    }

    #[test]
    fn evictor_skips_entries_with_active_references() {
        let dm = Arc::new(MockDispatchMap::new());
        let dm_iface: Arc<dyn IDispatchMap + Send + Sync> = Arc::clone(&dm) as _;

        // Insert a BlockDevice entry.
        dm.inner.lock().unwrap().entries.insert(
            1,
            MockEntry {
                location: MockEntryLocation::MemoryTier {
                    pointer: std::ptr::null_mut(),
                    size: 4096,
                    ssd_offset: Some(4096),
                },
                write_ref: false,
                read_refs: 0,
            },
        );

        // Take two read references — one simulates a concurrent reader,
        // the other will be consumed by get_evictable_offset's release_read.
        dm_iface.take_read(1).unwrap();
        dm_iface.take_read(1).unwrap();

        // get_evictable_offset sees BlockDevice and returns Some(offset),
        // releasing one read ref internally.
        let offset = crate::background::BackgroundEvictor::get_evictable_offset(&dm_iface, 1);
        assert_eq!(offset, Some(4096));

        // Remove fails because one read ref remains (the concurrent reader).
        let remove_result = dm_iface.remove(1);
        assert!(
            remove_result.is_err(),
            "remove should fail with active references"
        );

        // Entry still exists.
        assert!(dm.inner.lock().unwrap().entries.contains_key(&1));

        // Release the concurrent reader's ref and retry.
        dm_iface.release_read(1).unwrap();
        dm_iface.remove(1).unwrap();
        assert!(!dm.inner.lock().unwrap().entries.contains_key(&1));
    }

    #[test]
    fn evictor_start_and_shutdown() {
        let dm: Arc<dyn IDispatchMap + Send + Sync> = Arc::new(MockDispatchMap::new());
        let mt: Arc<dyn IMemoryTier + Send + Sync> = Arc::new(MockMemoryTier::new(1024 * 1024));

        let mut evictor = crate::background::BackgroundEvictor::start(
            dm,
            mt,
            vec![],
            crate::background::EvictorConfig {
                threshold: 0.9,
                low_watermark: 0.8,
                batch_size: 10,
                interval: std::time::Duration::from_millis(50),
            },
            None,
        );

        std::thread::sleep(std::time::Duration::from_millis(200));
        evictor.shutdown();
    }
}