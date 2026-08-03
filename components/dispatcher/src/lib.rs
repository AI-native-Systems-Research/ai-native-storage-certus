//! Dispatcher component — the central data-plane orchestrator for Certus.
//!
//! # Architecture
//!
//! The dispatcher sits between gRPC clients and the storage/GPU subsystems,
//! implementing all cache operations: populate, lookup, check, remove, touch.
//!
//! ```text
//! ┌──────────┐     gRPC      ┌────────────┐
//! │ Client   │──────────────▶│ Dispatcher │
//! │ (GPU app)│◀──────────────│            │
//! └──────────┘               └─────┬──────┘
//!                                  │
//!                 ┌────────────────┼──────────────┐
//!                 │                │              │
//!          ┌──────▼──────┐  ┌─────▼─────┐  ┌──────▼──────┐
//!          │ DispatchMap │  │ MemoryTier│  │ BlockDevice │
//!          │ (key→loc)   │  │(DRAM tier)│  │ (NVMe SSD)  │
//!          └─────────────┘  └───────────┘  └─────────────┘
//! ```
//!
//! # Data paths
//!
//! **Populate** (GPU → DRAM → SSD):
//! 1. Open client GPU IPC handle
//! 2. Allocate memory-tier slot - evict existing entries to make space if needed.
//! 3. `cudaMemcpy` D2H from GPU into memory-tier
//! 4. Background writer asynchronously flushes to SSD via extent manager
//!
//! **Hot Lookup** (DRAM → GPU):
//! 1. `DispatchMap::lookup(key)` → `MemoryTier { pointer }`
//! 2. `cudaMemcpyAsync` H2D from memory-tier to client GPU (multi-stream)
//! 3. Single `stream_synchronize` after all copies in the batch
//!
//! **Cold Lookup** (SSD → DRAM → GPU):
//! 1. `DispatchMap::lookup(key)` → `BlockDevice { offset }`
//! 2. Evict entries from memory-tier to make space
//! 3. Insert new slot in memory-tier
//! 4. Pipelined NVMe reads directly into memory-tier (zero-copy)
//! 5. Async H2D DMA to client GPU (overlapped with reads)
//!
//! # Threading model
//!
//! - gRPC requests arrive on tokio async runtime → `spawn_blocking`
//! - Hot path: runs on the blocking thread, multi-stream GPU DMA
//! - Cold path: `std::thread::scope` spawns per-drive queue threads
//!   (up to 2 per NVMe drive) for parallel SSD reads
//! - Background writer: separate thread pool for staging → SSD flush
//!
//! # Key design decisions
//!
//! - **Zero-copy cold path**: memory-tier pool is co-registered with SPDK
//!   (`spdk_mem_register`) and CUDA (`cudaHostRegister`), so NVMe DMA and
//!   GPU DMA both operate on the same pinned memory without CPU copies.
//! - **Multi-stream hot path**: 4 CUDA streams distribute H2D copies
//!   round-robin so the GPU can overlap transfers on its copy engines.
//! - **Configurable eviction**: `max_eviction_attempts` controls how hard
//!   the dispatcher tries to free memory-tier space before giving up.

#![allow(clippy::too_many_arguments)]

mod background;
pub mod cold_pool;
pub mod io_segmenter;
pub mod metrics;
mod pins;
pub mod pipeline;

use parking_lot::RwLock;
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::sync::Arc;
use std::sync::Mutex;

use component_framework::define_component;
use interfaces::{
    CacheKey, ClientChannels, Command, Completion, DispatcherConfig, DispatcherError, DmaAllocFn,
    DmaBuffer, FormatParams, GpuStream, IBlockDevice, IBlockDeviceAdmin, IDispatchMap, IDispatcher,
    IExtentManager, IGpuServices, ILogger, IMemoryTier, IRemoteLookup, IpcHandle, LookupResult,
    PciAddress,
};

use component_core::binding::bind;
use spdk_env::ISPDKEnv;

use crate::background::{
    BackgroundEvictor, EvictorConfig, MemoryTierEvictor, MemoryTierEvictorConfig,
    ParallelBackgroundWriter, WriteJob,
};
pub use crate::metrics::PipelineMetrics;

#[derive(Clone, Debug)]
pub enum EvictionReason {
    Demoted,
    Removed,
}

#[derive(Clone, Debug)]
pub struct EvictionEvent {
    pub key: CacheKey,
    pub reason: EvictionReason,
}

/// Holds one (block-device, extent-manager) pair for a data drive.
#[allow(dead_code)]
struct DataDrive {
    _block_dev: Arc<dyn component_core::IUnknown + Send + Sync>,
    block_dev_admin: Option<Arc<dyn IBlockDeviceAdmin + Send + Sync>>,
    block_dev_iface: Arc<dyn IBlockDevice + Send + Sync>,
    _extent_mgr_component: Arc<dyn component_core::IUnknown + Send + Sync>,
    extent_mgr: Arc<dyn IExtentManager + Send + Sync>,
    channel_pool: ChannelPool,
}

/// A pool of per-drive client channels for the read pipeline.
///
/// Each [`ClientChannels`] is an SPSC pair (single producer / single consumer)
/// and must be used by exactly one thread at a time. `batch_lookup` and the
/// prefetch path run concurrently on separate `spawn_blocking` threads, so a
/// single shared channel would let one thread's completions be dequeued by
/// another (completion theft → the victim waits on `completion_rx` forever).
/// Each concurrent reader therefore checks out its own channel; channels are
/// reused across checkouts and the pool grows on demand up to peak concurrency.
struct ChannelPool {
    block_dev: Arc<dyn IBlockDevice + Send + Sync>,
    free: Mutex<Vec<ClientChannels>>,
}

impl ChannelPool {
    fn new(block_dev: Arc<dyn IBlockDevice + Send + Sync>) -> Self {
        Self {
            block_dev,
            free: Mutex::new(Vec::new()),
        }
    }

    /// Check out an exclusive channel, connecting a fresh one if the pool is
    /// empty. The returned lease returns the channel to the pool on drop.
    fn checkout(&self) -> Result<ChannelLease<'_>, DispatcherError> {
        let reused = self.free.lock().unwrap_or_else(|e| e.into_inner()).pop();
        let channels = match reused {
            Some(ch) => ch,
            None => self.block_dev.connect_client().map_err(|e| {
                DispatcherError::IoError(format!("channel pool connect_client failed: {e}"))
            })?,
        };
        // Discard any late completions left by a prior user that errored out, so
        // the next pipeline starts from a clean completion stream (tags from an
        // old op must never be matched against a new op's segments).
        while channels.completion_rx.try_recv().is_ok() {}
        Ok(ChannelLease {
            pool: self,
            channels: Some(channels),
        })
    }
}

/// RAII guard for a checked-out [`ClientChannels`]; returns it to the pool on drop.
struct ChannelLease<'a> {
    pool: &'a ChannelPool,
    channels: Option<ClientChannels>,
}

impl ChannelLease<'_> {
    fn channels(&self) -> &ClientChannels {
        self.channels
            .as_ref()
            .expect("channel lease used after channel taken")
    }
}

impl Drop for ChannelLease<'_> {
    fn drop(&mut self) {
        if let Some(ch) = self.channels.take() {
            self.pool
                .free
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .push(ch);
        }
    }
}

fn format_bytes(bytes: usize) -> String {
    const KIB: usize = 1024;
    const MIB: usize = 1024 * 1024;
    const GIB: usize = 1024 * 1024 * 1024;
    if bytes >= GIB {
        format!("{:.2} GiB", bytes as f64 / GIB as f64)
    } else if bytes >= MIB {
        format!("{:.2} MiB", bytes as f64 / MIB as f64)
    } else if bytes >= KIB {
        format!("{:.2} KiB", bytes as f64 / KIB as f64)
    } else {
        format!("{bytes} B")
    }
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
    pub DispatcherComponent {
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
            bg_mt_evictor: Mutex<Option<MemoryTierEvictor>>,
            cold_pool: Mutex<Option<cold_pool::ColdReadPool>>,
            data_drives: RwLock<Vec<DataDrive>>,
            pipeline_ring: RwLock<Option<pipeline::PipelineRing>>,
            warm_stream: AtomicU64,
            block_device_factory: Mutex<Option<BlockDeviceFactory>>,
            extent_manager_factory: Mutex<Option<ExtentManagerFactory>>,
            max_eviction_attempts: AtomicUsize,
            pipeline_metrics: RwLock<Option<Arc<dyn PipelineMetrics>>>,
            eviction_tx: Arc<Mutex<Option<crossbeam_channel::Sender<EvictionEvent>>>>,
            eviction_dropped: AtomicU64,
        },
    }
}

/// No-op free function for temporary DmaBuffer wrappers around memory-tier pointers.
/// The memory-tier component owns the memory; this wrapper must not free it.
unsafe extern "C" fn noop_free(_ptr: *mut std::ffi::c_void) {}

/// Per-GPU-device CUDA streams for the load/store DMA paths.
///
/// A CUDA stream is bound to the device that was current when it was created,
/// and `cudaMemcpyAsync` rejects a source/destination pointer that lives on a
/// different device than the stream. Under tensor parallelism the server DMAs
/// to more than one GPU, so a single shared stream fails ("invalid argument")
/// for every transfer whose peer GPU isn't the stream's device. We therefore
/// keep one warm stream and one pair of pipeline streams per device.
///
/// Process-global by design: there is a single dispatcher instance serving all
/// local GPUs, and CUDA streams are process-wide resources. Entries are created
/// lazily on first use for each device and live until the process exits.
pub(crate) struct DeviceStreams {
    /// Warm-path stream for memory-tier <-> GPU async copies.
    warm: u64,
    /// Dual streams for the pipelined SSD -> GPU cold path.
    pub(crate) pipe: [u64; 2],
}

static DEVICE_STREAMS: std::sync::OnceLock<
    std::sync::Mutex<std::collections::HashMap<i32, DeviceStreams>>,
> = std::sync::OnceLock::new();

/// Ensure per-device streams exist for `device`, creating them on that device.
/// Returns `None` if `device` is negative (no GPU association) or stream setup
/// fails — callers then fall back to the synchronous `DmaBuffer` path.
pub(crate) fn device_streams_for(gpu: &dyn IGpuServices, device: i32) -> Option<DeviceStreams> {
    if device < 0 {
        return None;
    }
    let map =
        DEVICE_STREAMS.get_or_init(|| std::sync::Mutex::new(std::collections::HashMap::new()));
    let mut guard = map.lock().unwrap();
    if let Some(s) = guard.get(&device) {
        return Some(DeviceStreams {
            warm: s.warm,
            pipe: s.pipe,
        });
    }
    // Create all streams on the target device (streams inherit the current device).
    gpu.set_device(device).ok()?;
    let warm = gpu.create_stream().ok()?;
    let pipe_a = gpu.create_stream().ok()?;
    let pipe_b = gpu.create_stream().ok()?;
    let entry = DeviceStreams {
        warm: warm.0 as u64,
        pipe: [pipe_a.0 as u64, pipe_b.0 as u64],
    };
    let ret = DeviceStreams {
        warm: entry.warm,
        pipe: entry.pipe,
    };
    guard.insert(device, entry);
    Some(ret)
}

/// Resolve the GPU device a batch's blocks live on from the first block's IPC
/// address, and make it the calling thread's current device (CUDA tracks the
/// current device per thread). Returns the device ordinal, or -1 if unknown.
/// All entries in one RPC come from a single rank, hence a single device.
fn set_batch_device(gpu: &dyn IGpuServices, addr: *mut u8) -> i32 {
    match gpu.device_of_ptr(addr as *const std::ffi::c_void) {
        Ok(dev) if dev >= 0 => {
            let _ = gpu.set_device(dev);
            dev
        }
        _ => -1,
    }
}

impl DispatcherComponent {
    fn log_info(&self, msg: &str) {
        if let Ok(logger) = self.logger.get() {
            logger.info(msg);
        }
    }

    fn log_warn(&self, msg: &str) {
        if let Ok(logger) = self.logger.get() {
            logger.warn(msg);
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

    /// Attach a pipeline metrics reporter for observability.
    /// When set, internal data-path stages report timing through this trait.
    pub fn set_pipeline_metrics(&self, m: Arc<dyn PipelineMetrics>) {
        *self.pipeline_metrics.write() = Some(m);
    }

    /// Create a bounded eviction event channel and install the sender.
    /// Returns the receiver that the gRPC layer should drain via `TakeEvents`.
    pub fn create_eviction_channel(
        &self,
        capacity: usize,
    ) -> crossbeam_channel::Receiver<EvictionEvent> {
        let (tx, rx) = crossbeam_channel::bounded(capacity);
        *self.eviction_tx.lock().unwrap() = Some(tx);
        rx
    }

    /// Read and reset the count of eviction events dropped due to a full channel.
    pub fn eviction_dropped_count(&self) -> u64 {
        self.eviction_dropped.swap(0, Ordering::Relaxed)
    }

    fn emit_eviction(&self, key: CacheKey, reason: EvictionReason) {
        let guard = self.eviction_tx.lock().unwrap();
        if let Some(ref tx) = *guard {
            if tx.try_send(EvictionEvent { key, reason }).is_err() {
                self.eviction_dropped.fetch_add(1, Ordering::Relaxed);
            }
        }
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
                // Zero-copy: source buffer is SPDK-allocated, wrap the slice directly.
                // SAFETY: buffer pointer + offset is valid for seg.length bytes and DMA-capable.
                let ptr = unsafe {
                    (buffer.as_ptr() as *mut u8).add(seg.buffer_offset) as *mut std::ffi::c_void
                };
                unsafe { DmaBuffer::from_raw(ptr, seg.length, noop_free, numa_node) }.map_err(
                    |e| DispatcherError::AllocationFailed(format!("DmaBuffer wrap segment: {e}")),
                )?
            } else {
                let staging =
                    DmaBuffer::new(seg.length, block_size, Some(numa_node)).map_err(|e| {
                        DispatcherError::AllocationFailed(format!("DMA segment buffer: {e}"))
                    })?;

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
        device: i32,
    ) -> Result<(), DispatcherError> {
        let total_bytes = ipc_handle.size as usize;

        // Evict and insert into memory-tier (retries on shard fragmentation).
        // On tier saturation, fall back to a staging buffer so the load still
        // succeeds (served uncached) instead of failing — see StagingPool.
        let max_attempts = self.max_eviction_attempts.load(Ordering::Relaxed);
        let has_drives = !self.data_drives.read().is_empty();
        let (mem_ptr, staging_lease) =
            match self.evict_and_insert(dm, mt, key, ipc_handle.size, max_attempts) {
                Ok(p) => (p, None),
                Err(DispatcherError::AllocationFailed(_)) if has_drives => {
                    let sp = self
                        .pipeline_ring
                        .read()
                        .as_ref()
                        .and_then(|r| r.staging.clone());
                    match sp {
                        Some(sp) if total_bytes <= sp.buf_bytes() => {
                            let lease = sp.checkout();
                            (lease.ptr(), Some(lease))
                        }
                        _ => {
                            return Err(DispatcherError::AllocationFailed(
                                "memory-tier full and no staging buffer available for cold load"
                                    .into(),
                            ))
                        }
                    }
                }
                Err(e) => return Err(e),
            };
        let staged = staging_lease.is_some();

        // Read from SSD into memory-tier using pipelined reader.
        let drives = self.data_drives.read();
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

        // Check out an exclusive channel for this read. A single shared SPSC
        // channel across concurrent readers lets one thread dequeue another's
        // completions (completion theft → permanent hang); see ChannelPool.
        let lease = drive.channel_pool.checkout()?;

        // Zero-copy pipelined reader: NVMe → memory-tier slot → GPU (no intermediate ring copy).
        // SAFETY: mem_ptr is a valid, CUDA-pinned, SPDK-registered memory-tier slot.
        // ipc_handle.address is a valid GPU destination pointer.
        //
        // Use CUDA streams bound to the destination GPU's device (multi-GPU): the
        // shared init-device pipeline_ring streams would fail cudaMemcpyAsync for
        // a block on another GPU. Fall back to the ring's streams only when the
        // device is unknown.
        let chunk_size = {
            let ring_guard = self.pipeline_ring.read();
            ring_guard.as_ref().map_or(131072, |r| r.chunk_size)
        };
        let dev_streams = device_streams_for(&**gpu, device);
        let streams: [GpuStream; 2] = match &dev_streams {
            Some(s) => [
                GpuStream(s.pipe[0] as *mut std::ffi::c_void),
                GpuStream(s.pipe[1] as *mut std::ffi::c_void),
            ],
            None => {
                let ring_guard = self.pipeline_ring.read();
                let ring_ref = ring_guard.as_ref().ok_or_else(|| {
                    DispatcherError::NotInitialized("pipeline ring not allocated".into())
                })?;
                ring_ref.streams
            }
        };
        unsafe {
            pipeline::pipelined_ssd_to_gpu_zero_copy(
                &*block_dev,
                &**gpu,
                &streams,
                lease.channels(),
                mem_ptr,
                ipc_handle.address as *mut std::ffi::c_void,
                start_lba,
                total_bytes,
                chunk_size,
                16,
            )?;
        }
        drop(lease);
        drop(drives);

        // Staged (uncached) serve: the DMA is done and the data was delivered to
        // the GPU, but it lives in a transient staging buffer, not a tier slot —
        // leave the entry as BlockDevice and return the buffer to the pool (the
        // lease drops here). Do NOT promote.
        if staged {
            drop(staging_lease);
            return Ok(());
        }

        // In-place BlockDevice->MemoryTier transition (retains the SSD offset so
        // the entry stays demotable). Unlike the old remove+recreate, this works
        // when the entry is pinned (read_ref > 0) by an in-flight load — remove
        // rejects pinned entries, which crashed the load path.
        let _ = offset; // offset retained inside promote_block_to_memory_tier
        dm.promote_block_to_memory_tier(key, mem_ptr, ipc_handle.size)
            .map_err(|e| DispatcherError::IoError(format!("promote transition failed: {e}")))?;

        Ok(())
    }

    /// Serve one cold key through a transient staging buffer (no caching).
    ///
    /// Used when the memory tier is saturated and `evict_and_insert` can't get a
    /// slot: reads `SSD → staging buffer → GPU` and leaves the dispatch-map entry
    /// as `BlockDevice`. Holds exactly one staging lease, for the duration of the
    /// read only, and takes no other lock while blocked on `checkout` — so the
    /// blocking checkout is deadlock-free (buffers are released independently of
    /// any memory-tier pin).
    #[allow(clippy::too_many_arguments)]
    fn serve_cold_staged(
        &self,
        gpu: &Arc<dyn IGpuServices + Send + Sync>,
        key: CacheKey,
        offset: u64,
        regions: &[IpcHandle],
        total_size: u32,
        device: i32,
    ) -> Result<(), DispatcherError> {
        let sp = match self
            .pipeline_ring
            .read()
            .as_ref()
            .and_then(|r| r.staging.clone())
        {
            Some(s) if total_size as usize <= s.buf_bytes() => s,
            _ => {
                return Err(DispatcherError::AllocationFailed(
                    "no staging buffer available for cold load".into(),
                ))
            }
        };
        let lease = sp.checkout();
        let total_bytes = total_size as usize;

        let drives = self.data_drives.read();
        if drives.is_empty() {
            return Err(DispatcherError::IoError(
                "staged cold read requires a data drive".into(),
            ));
        }
        let idx = Self::drive_index(key, drives.len());
        let drive = &drives[idx];
        let block_size = drive.block_dev_iface.block_size();
        let start_lba = offset / block_size as u64;
        let block_dev = Arc::clone(&drive.block_dev_iface);
        // Exclusive channel for this read (see ChannelPool: sharing one SPSC
        // channel across concurrent readers causes completion theft).
        let chan_lease = drive.channel_pool.checkout()?;

        // Device-correct streams (multi-GPU), falling back to the ring's streams.
        let chunk_size = {
            let ring_guard = self.pipeline_ring.read();
            ring_guard.as_ref().map_or(131072, |r| r.chunk_size)
        };
        let dev_streams = device_streams_for(&**gpu, device);
        let streams: [GpuStream; 2] = match &dev_streams {
            Some(s) => [
                GpuStream(s.pipe[0] as *mut std::ffi::c_void),
                GpuStream(s.pipe[1] as *mut std::ffi::c_void),
            ],
            None => {
                let ring_guard = self.pipeline_ring.read();
                let ring_ref = ring_guard.as_ref().ok_or_else(|| {
                    DispatcherError::NotInitialized("pipeline ring not allocated".into())
                })?;
                ring_ref.streams
            }
        };

        // Single-region (N==1): fuse SSD->GPU (gpu_dst = the one region), as
        // before. Multi-region (N>1): read SSD->staging only (gpu_dst null) then
        // scatter the staging buffer to the N GPU allocations below — the N
        // regions are colocated in one contiguous slot.
        let gpu_dst = if regions.len() == 1 {
            regions[0].address as *mut std::ffi::c_void
        } else {
            std::ptr::null_mut()
        };

        // SAFETY: lease.ptr() is a valid CUDA-pinned + SPDK-registered buffer for
        // `total_bytes`; gpu_dst is the GPU destination (null for the scatter
        // case). The lease keeps the buffer alive until after the read completes
        // below (and, for N>1, until the scatter completes).
        let res = unsafe {
            pipeline::pipelined_ssd_to_gpu_zero_copy(
                &*block_dev,
                &**gpu,
                &streams,
                chan_lease.channels(),
                lease.ptr(),
                gpu_dst,
                start_lba,
                total_bytes,
                chunk_size,
                16,
            )
        };
        drop(chan_lease); // return the channel to the pool
        drop(drives);

        // Multi-region: the read only filled the staging buffer; scatter it to
        // the client's N GPU regions before releasing the lease. Synchronize so
        // the copies complete while lease.ptr() is still valid.
        let res = res.and_then(|()| {
            if regions.len() > 1 {
                let warm_raw = dev_streams.as_ref().map_or(0, |s| s.warm);
                self.serve_memory_tier_to_gpu(
                    gpu,
                    lease.ptr() as *mut u8,
                    total_size,
                    regions,
                    warm_raw,
                    true,
                )
            } else {
                Ok(())
            }
        });
        drop(lease); // return the staging buffer; do NOT promote (stays BlockDevice)
        res
    }

    /// Free one pin-safe eviction victim, returning `true` if a slot was freed.
    ///
    /// Scans the `scan` oldest keys. For each candidate, in order of preference:
    ///  1. **Demote** to BlockDevice via `dm.try_evict_to_block` (keeps the data
    ///     on SSD and the entry resolvable). Succeeds only when write-through is
    ///     complete and the entry is unpinned.
    ///  2. Otherwise **drop** it via `dm.remove` (write-through not yet done, so
    ///     the block is lost from cache and recomputed on next miss).
    ///
    /// Both dispatch-map operations reject entries with `read_ref > 0`, and in
    /// both branches the dispatch-map transition happens *before* the DRAM slot
    /// is freed. So this NEVER frees a slot that a pinned, in-flight load still
    /// points at — the bug that let the old blind-eviction path reclaim a
    /// pinned slot and corrupt the concurrent load (invalid H2D DMA / stale
    /// data). A pinned candidate is skipped; the next one is tried.
    ///
    /// Returns `false` only when every scanned candidate is pinned — the caller
    /// must surface pool-full rather than blind-free.
    fn evict_one_clean(
        &self,
        dm: &Arc<dyn IDispatchMap + Send + Sync>,
        mt: &Arc<dyn IMemoryTier + Send + Sync>,
        scan: usize,
    ) -> bool {
        for cand in mt.oldest_keys(scan) {
            // Preferred: demote to block (data preserved, entry stays resolvable).
            if dm.try_evict_to_block(cand).is_ok() {
                let _ = mt.remove(cand);
                self.emit_eviction(cand, EvictionReason::Demoted);
                return true;
            }
            // Fallback: write-through incomplete so it can't be demoted. Drop it
            // entirely — but only if unpinned. `dm.remove` returns an error for
            // pinned entries, so a success means no in-flight load points at it;
            // only then is it safe to free the DRAM slot.
            if dm.remove(cand).is_ok() {
                let _ = mt.remove(cand);
                self.emit_eviction(cand, EvictionReason::Removed);
                return true;
            }
            // Pinned by an in-flight load — leave it and try the next candidate.
        }
        false
    }

    /// Evict entries from the memory-tier until `needed` bytes are free.
    ///
    /// Every iteration frees one pin-safe eviction victim via [`Self::evict_one_clean`],
    /// widening the scan as pressure persists. Evicted entries transition
    /// MemoryTier → BlockDevice in the dispatch-map (data remains on SSD from the
    /// prior write-through). If no candidate in the widening scan is evictable —
    /// every oldest entry is pinned by an in-flight load or not yet written
    /// through — this returns `AllocationFailed` rather than corrupt a pinned
    /// slot; the caller leaves the block uncached.
    // NOTE: This only handles global capacity pressure. If keys are heavily skewed
    // to one shard (e.g., all keys ≡ 0 mod 16), the target shard can fill while
    // global used() < capacity(). In that case insert() will return PoolFull after
    // this function succeeds. Acceptable for now — real workloads distribute evenly.
    fn evict_for_space(
        &self,
        dm: &Arc<dyn IDispatchMap + Send + Sync>,
        mt: &Arc<dyn IMemoryTier + Send + Sync>,
        needed: u32,
        _target_key: CacheKey,
        max_attempts: usize,
    ) -> Result<(), DispatcherError> {
        const MAX_SCAN: usize = 4;

        let mut attempts = 0usize;
        while mt.used() + needed as usize > mt.capacity() {
            attempts += 1;
            if attempts > max_attempts {
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

            // Free one pin-safe victim, widening the scan as pressure persists so
            // we look deeper into the eviction order past pinned/unpersisted entries.
            let scan = (MAX_SCAN * attempts).min(1024);
            if !self.evict_one_clean(dm, mt, scan) {
                // Every scanned candidate is pinned by an in-flight load. Do NOT
                // blind-free — that would reclaim DRAM a load still points at.
                // Surface pool-full; the caller leaves the block uncached.
                return Err(DispatcherError::AllocationFailed(
                    "memory-tier full: all eviction candidates are pinned by in-flight loads"
                        .into(),
                ));
            }
        }
        Ok(())
    }

    /// Evict and insert into memory-tier, retrying on fragmentation-induced PoolFull.
    ///
    /// After `evict_for_space` ensures global capacity, `mt.insert()` can still
    /// fail if the freed space is non-contiguous (shard-level fragmentation).
    /// This helper retries by force-evicting from the same shard until a
    /// contiguous slot is available.
    fn evict_and_insert(
        &self,
        dm: &Arc<dyn IDispatchMap + Send + Sync>,
        mt: &Arc<dyn IMemoryTier + Send + Sync>,
        key: CacheKey,
        size: u32,
        max_attempts: usize,
    ) -> Result<*mut u8, DispatcherError> {
        const MAX_SCAN: usize = 4;
        let mut attempts = 0usize;
        loop {
            self.evict_for_space(dm, mt, size, key, max_attempts.saturating_sub(attempts))?;
            match mt.insert(key, size) {
                Ok(ptr) => return Ok(ptr),
                Err(interfaces::MemoryTierError::PoolFull) => {
                    attempts += 1;
                    if attempts >= max_attempts {
                        return Err(DispatcherError::AllocationFailed(
                            "memory-tier pool full (fragmentation after eviction)".into(),
                        ));
                    }
                    // Force-evict one more pin-safe victim to relieve shard
                    // fragmentation. If nothing is evictable (all pinned or
                    // unpersisted), fail rather than blind-free a slot an
                    // in-flight load still points at.
                    if !self.evict_one_clean(dm, mt, MAX_SCAN * attempts) {
                        return Err(DispatcherError::AllocationFailed(
                            "memory-tier full: no evictable (unpinned, persisted) entry to relieve fragmentation"
                                .into(),
                        ));
                    }
                }
                Err(interfaces::MemoryTierError::AlreadyExists(k)) => {
                    return Err(DispatcherError::AlreadyExists(k));
                }
                Err(e) => {
                    return Err(DispatcherError::AllocationFailed(e.to_string()));
                }
            }
        }
    }

    /// Verify a warm slot's CRC-32 before it is served to the GPU. `expected`
    /// is the checksum recorded on the store path (`None` = never recorded, so
    /// nothing to check). The slot is pinned by the caller's read-ref, so the
    /// `size` bytes at `pointer` are stable while we hash them on the CPU. This
    /// is the latest correct point to check — immediately before the H2D copy
    /// that feeds the GPU consumer. Returns `IoError` on mismatch.
    #[cfg(feature = "integrity-check")]
    fn verify_slot_crc(
        expected: Option<u32>,
        key: CacheKey,
        pointer: *mut u8,
        size: u32,
    ) -> Result<(), DispatcherError> {
        let Some(expected) = expected else {
            return Ok(());
        };
        // SAFETY: the warm hit is pinned by read_ref (no concurrent writer), so
        // `size` bytes at `pointer` are the resident, immutable slot contents.
        let bytes = unsafe { std::slice::from_raw_parts(pointer as *const u8, size as usize) };
        let mut hasher = crc32fast::Hasher::new();
        hasher.update(bytes);
        let actual = hasher.finalize();
        if actual != expected {
            return Err(DispatcherError::IoError(format!(
                "integrity: CRC-32 mismatch for key {key:#018x}: expected {expected:#010x}, got {actual:#010x}"
            )));
        }
        Ok(())
    }

    /// Copy a resident memory-tier slot to a GPU block via DMA. Shared by the
    /// `batch_lookup` warm fast-path and the concurrent-promotion recovery
    /// serve. Uses the warm CUDA stream when one is bound, else the synchronous
    /// `DmaBuffer` fallback (matching the no-warm-stream test/CPU path). When
    /// `synchronize` is set and the warm stream is used, blocks until the copy
    /// completes; the fast path passes `false` and defers to one batched sync.
    fn serve_memory_tier_to_gpu(
        &self,
        gpu: &Arc<dyn IGpuServices + Send + Sync>,
        pointer: *mut u8,
        size: u32,
        regions: &[IpcHandle],
        warm_raw: u64,
        synchronize: bool,
    ) -> Result<(), DispatcherError> {
        // Scatter the one contiguous DRAM slot back to the client's N GPU
        // regions: region L reads from `slot + sum(sizes[0..L])`. A single-region
        // block (legacy / populate) is just the L==0 case. `size` is the slot's
        // byte length; we never read past it even if the regions sum larger.
        // Use the caller-resolved warm stream for the destination's device; the
        // stream must be on the same GPU as the region address or the async copy
        // fails with "invalid argument" under multi-GPU.
        let raw = warm_raw;
        if raw != 0 {
            let s = GpuStream(raw as *mut std::ffi::c_void);
            let mut off: usize = 0;
            for region in regions {
                let copy_size = (region.size as usize).min((size as usize).saturating_sub(off));
                if copy_size == 0 {
                    break;
                }
                gpu.memcpy_h2d_async(
                    (pointer as usize + off) as *const std::ffi::c_void,
                    region.address as *mut std::ffi::c_void,
                    copy_size,
                    s,
                )
                .map_err(|e| {
                    DispatcherError::IoError(format!(
                        "GPU DMA copy (memory-tier→device) failed: {e}"
                    ))
                })?;
                off += region.size as usize;
            }
            if synchronize {
                gpu.stream_synchronize(s).map_err(|e| {
                    DispatcherError::IoError(format!("stream_synchronize failed: {e}"))
                })?;
            }
            Ok(())
        } else {
            let mut off: usize = 0;
            for region in regions {
                let copy_size = (region.size as usize).min((size as usize).saturating_sub(off));
                if copy_size == 0 {
                    break;
                }
                let aligned = copy_size.next_multiple_of(4096).max(4096);
                let buf = unsafe {
                    DmaBuffer::from_raw(
                        (pointer as usize + off) as *mut std::ffi::c_void,
                        aligned,
                        noop_free,
                        -1,
                    )
                }
                .map_err(|e| DispatcherError::IoError(format!("DmaBuffer wrap failed: {e}")))?;
                let r = gpu
                    .dma_copy_to_device(&buf, region.address as *mut std::ffi::c_void, copy_size)
                    .map_err(|e| {
                        DispatcherError::IoError(format!(
                            "GPU DMA copy (memory-tier→device) failed: {e}"
                        ))
                    });
                std::mem::forget(buf);
                r?;
                off += region.size as usize;
            }
            Ok(())
        }
    }

    /// Serve a key whose promotion lost the `mt.insert` race to a concurrent
    /// lookup — e.g. the other tensor-parallel rank requesting the same
    /// content-hash key at the same time. When two `batch_lookup` calls both
    /// classify a cold key as `BlockDevice`, both try to promote it; the loser
    /// gets `MemoryTierError::AlreadyExists`. That is a hit, not a failure: the
    /// winner flips the dispatch-map entry to `MemoryTier` only *after* its
    /// SSD→DRAM read completes, so observing `MemoryTier` means the data is
    /// resident. Wait (bounded) for that transition, then DMA to the GPU.
    ///
    /// The caller's load pin (`take_read` from `prepare_load`) keeps the entry
    /// from being evicted while we wait.
    fn serve_concurrently_promoted(
        &self,
        dm: &Arc<dyn IDispatchMap + Send + Sync>,
        gpu: &Arc<dyn IGpuServices + Send + Sync>,
        key: CacheKey,
        regions: &[IpcHandle],
        warm_raw: u64,
    ) -> Result<(), DispatcherError> {
        let start = std::time::Instant::now();
        let timeout = std::time::Duration::from_secs(5);
        loop {
            match dm.lookup(key) {
                Ok(LookupResult::MemoryTier { pointer, size }) => {
                    let res =
                        self.serve_memory_tier_to_gpu(gpu, pointer, size, regions, warm_raw, true);
                    let _ = dm.release_read(key);
                    let _ = dm.touch(key);
                    return res;
                }
                Ok(LookupResult::BlockDevice { .. }) => {
                    // Winner hasn't finished its SSD→DRAM read yet: release the
                    // read-ref this lookup took and back off briefly.
                    let _ = dm.release_read(key);
                    if start.elapsed() >= timeout {
                        return Err(DispatcherError::KeyNotFound(key));
                    }
                    std::thread::sleep(std::time::Duration::from_micros(50));
                }
                Ok(LookupResult::MismatchSize) => {
                    let _ = dm.release_read(key);
                    return Err(DispatcherError::InvalidParameter(
                        "size mismatch on concurrent-promotion recovery".into(),
                    ));
                }
                // NotExist does not take a read-ref, so nothing to release.
                Ok(LookupResult::NotExist) => return Err(DispatcherError::KeyNotFound(key)),
                Err(e) => return Err(DispatcherError::IoError(e.to_string())),
            }
        }
    }

    fn process_write_job(
        dm: &Arc<dyn IDispatchMap + Send + Sync>,
        mt: &Arc<dyn IMemoryTier + Send + Sync>,
        drives: &[Arc<dyn IBlockDevice + Send + Sync>],
        extent_mgrs: &[Arc<dyn IExtentManager + Send + Sync>],
        job: WriteJob,
    ) {
        // Get the memory-tier pointer without refreshing its eviction-order position —
        // the write-through must not prevent this entry from being evicted under memory pressure.
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
                    if let Ok(mt) = self.memory_tier.get() {
                        let mt_hook = Arc::clone(&mt);
                        let logger_hook = Arc::clone(&logger) as Arc<dyn ILogger + Send + Sync>;
                        em.set_post_checkpoint_hook(Arc::new(move || {
                            let used = mt_hook.used();
                            let capacity = mt_hook.capacity();
                            let pct = if capacity > 0 {
                                used as f64 / capacity as f64 * 100.0
                            } else {
                                0.0
                            };
                            logger_hook.info(&format!(
                                "memory_tier_pool ({} / {}, {pct:.1}% used)",
                                format_bytes(used),
                                format_bytes(capacity),
                            ));
                        }));
                    }
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

            // --- Partition table management ---
            let part_mgr = disk_partition_manager::DiskPartitionManager::new_default();
            part_mgr.set_ns_id(1);
            bind(
                &*block_dev_component,
                "IBlockDevice",
                &*part_mgr,
                "block_device",
            )
            .map_err(|e| {
                DispatcherError::IoError(format!(
                    "failed to bind block device to partition manager {i}: {e}"
                ))
            })?;

            let partition_config = interfaces::PartitionConfig {
                sector_size,
                total_sectors: num_sectors,
                ns_id: 1,
                partitions: vec![
                    interfaces::PartitionSpec {
                        type_guid: interfaces::type_guids::CERTUS_METADATA,
                        size_bytes: config.metadata_partition_size,
                        name: "certus-metadata".into(),
                    },
                    interfaces::PartitionSpec {
                        type_guid: interfaces::type_guids::CERTUS_EXTERNAL_META,
                        size_bytes: config.extended_metadata_partition_size,
                        name: "certus-extended-metadata".into(),
                    },
                    interfaces::PartitionSpec {
                        type_guid: interfaces::type_guids::CERTUS_DATA,
                        size_bytes: 0, // rest of disk
                        name: "certus-data".into(),
                    },
                ],
            };

            let (table, formatted) = part_mgr
                .initialize_or_format(config.format_on_init, partition_config)
                .map_err(|e| {
                    DispatcherError::IoError(format!(
                        "failed to initialize partition table for data drive {i}: {e}"
                    ))
                })?;

            // Configure extent-manager with partition offsets.
            // The Certus layout requires exactly three partitions:
            //   [0] metadata, [1] extended metadata (reserved), [2] data.
            // A disk with a valid-but-incompatible GPT (e.g. an empty or
            // non-Certus partition table) reads back with fewer entries, so
            // guard against out-of-bounds access and fail with a clear error
            // instead of panicking.
            const EXPECTED_PARTITIONS: usize = 3;
            if table.partitions.len() < EXPECTED_PARTITIONS {
                return Err(DispatcherError::IoError(format!(
                    "data drive {i} has an incompatible partition table: expected \
                     {EXPECTED_PARTITIONS} Certus partitions (metadata, extended \
                     metadata, data) but found {}. The disk has a valid GPT that is \
                     not a Certus layout; re-run with format_on_init to (destructively) \
                     format it.",
                    table.partitions.len()
                )));
            }

            iem.set_metadata_base_lba(table.partitions[0].start_lba);
            // partition[1] = extended metadata (reserved for future use)
            iem.set_data_base_lba(table.partitions[2].start_lba);

            if formatted {
                self.log_warn(&format!(
                    "dispatcher: formatting disk for data drive {i} at {addr_str} \
                     — all existing data will be destroyed"
                ));
            }

            // Log partition layout
            for p in &table.partitions {
                let size_mib = p.num_sectors * sector_size as u64 / (1024 * 1024);
                let size_str = if size_mib >= 1024 * 1024 {
                    format!("{:.2} TiB", size_mib as f64 / (1024.0 * 1024.0))
                } else if size_mib >= 1024 {
                    format!("{:.2} GiB", size_mib as f64 / 1024.0)
                } else {
                    format!("{} MiB", size_mib)
                };
                self.log_info(&format!(
                    "dispatcher: drive {i} partition {}: \"{}\" start_lba={} size={}",
                    p.index, p.name, p.start_lba, size_str
                ));
            }

            let data_disk_size = table.partitions[2].num_sectors * sector_size as u64;
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
            if formatted {
                iem.format(FormatParams {
                    data_disk_size,
                    sector_size,
                    slab_size,
                    max_extent_size,
                    metadata_region_size: 0,
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

            let channel_pool = ChannelPool::new(Arc::clone(&ibd));

            drives.push(DataDrive {
                _block_dev: block_dev_component,
                block_dev_admin: admin,
                block_dev_iface: ibd,
                _extent_mgr_component: extent_mgr
                    as Arc<dyn component_core::IUnknown + Send + Sync>,
                extent_mgr: iem,
                channel_pool,
            });
        }

        Ok(drives)
    }
}

impl IDispatcher for DispatcherComponent {
    /// Initialize the dispatcher with N drives, CUDA streams, and background workers.
    ///
    /// Sequence:
    /// 1. Create N block devices + N extent managers via factories
    /// 2. Recover dispatch-map from on-disk extents (if not formatting)
    /// 3. Allocate warm CUDA streams for hot-path DMA
    /// 4. Allocate PipelineRing streams for cold-path DMA
    /// 5. Register memory-tier pool with CUDA + SPDK for zero-copy DMA
    /// 6. Start background write-through workers (one per drive)
    /// 7. Start background SSD evictor (if threshold configured)
    fn initialize(&self, config: DispatcherConfig) -> Result<(), DispatcherError> {
        self.log_info("dispatcher: initializing");

        self.max_eviction_attempts
            .store(config.max_eviction_attempts, Ordering::Relaxed);

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
                    match pipeline::PipelineRing::new(
                        &*gpu,
                        chunk_size,
                        config.cold_staging_slots,
                        config.cold_staging_buf_bytes,
                    ) {
                        Ok(ring) => {
                            if let Some(ref s) = ring.staging {
                                self.log_info(&format!(
                                    "dispatcher: cold-load staging pool ready ({} slots × {} KiB)",
                                    config.cold_staging_slots,
                                    config.cold_staging_buf_bytes / 1024,
                                ));
                                let _ = s;
                            }
                            *self.pipeline_ring.write() = Some(ring);
                        }
                        Err(e) => {
                            self.log_info(&format!(
                                "pipeline ring allocation failed (non-fatal): {e:?}"
                            ));
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

        // Start persistent cold-path worker pool (pre-connected NVMe channels + CUDA streams).
        if let Ok(gpu) = self.gpu_services.get() {
            let pool_drives: Vec<Arc<dyn IBlockDevice + Send + Sync>> = {
                let dd = self.data_drives.read();
                dd.iter().map(|d| Arc::clone(&d.block_dev_iface)).collect()
            };
            if !pool_drives.is_empty() {
                const COLD_POOL_QUEUES_PER_DRIVE: usize = 2;
                match cold_pool::ColdReadPool::new(&pool_drives, &gpu, COLD_POOL_QUEUES_PER_DRIVE) {
                    Ok(pool) => {
                        self.log_info(&format!(
                            "dispatcher: cold pool started ({} drives × {} queues)",
                            pool_drives.len(),
                            COLD_POOL_QUEUES_PER_DRIVE,
                        ));
                        *self.cold_pool.lock().unwrap() = Some(pool);
                    }
                    Err(e) => {
                        self.log_info(&format!(
                            "cold pool creation failed (non-fatal, will use scoped threads): {e:?}"
                        ));
                    }
                }
            }
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
                let evictor_eviction_tx = self.eviction_tx.lock().unwrap().clone();
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
                    evictor_eviction_tx,
                );
                *self.bg_evictor.lock().unwrap() = Some(evictor);
            }
        }

        // Start background memory-tier evictor (DRAM → SSD demotion) if configured.
        if config.memory_tier_eviction_threshold > 0.0 {
            let dm_for_mt_evictor = self
                .dispatch_map
                .get()
                .map_err(|_| DispatcherError::NotInitialized("dispatch_map not bound".into()))?;
            let mt_for_mt_evictor = self
                .memory_tier
                .get()
                .map_err(|_| DispatcherError::NotInitialized("memory_tier not bound".into()))?;
            let mt_evictor_logger = self.logger.get().ok();
            let mt_evictor = MemoryTierEvictor::start(
                dm_for_mt_evictor,
                mt_for_mt_evictor,
                MemoryTierEvictorConfig {
                    threshold: config.memory_tier_eviction_threshold,
                    low_watermark: config.memory_tier_eviction_low_watermark,
                    batch_size: config.memory_tier_eviction_batch_size,
                    interval: std::time::Duration::from_secs(
                        config.memory_tier_eviction_interval_secs,
                    ),
                },
                mt_evictor_logger,
                Arc::clone(&self.eviction_tx),
            );
            *self.bg_mt_evictor.lock().unwrap() = Some(mt_evictor);
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

        if let Some(mut mt_evictor) = self.bg_mt_evictor.lock().unwrap().take() {
            mt_evictor.shutdown();
        }

        if let Some(mut writer) = self.bg_writer.lock().unwrap().take() {
            writer.shutdown();
        }

        // Shut down cold pool before block device teardown (workers hold ClientChannels).
        if let Some(pool) = self.cold_pool.lock().unwrap().take() {
            pool.shutdown();
        }

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

        // Destroy warm stream and pipeline ring.
        if let Ok(gpu) = self.gpu_services.get() {
            let raw = self.warm_stream.swap(0, Ordering::AcqRel);
            if raw != 0 {
                let _ = gpu.destroy_stream(GpuStream(raw as *mut std::ffi::c_void));
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

    /// Batch lookup — the primary benchmark path for multi-key retrieval.
    ///
    /// Two-phase design:
    /// 1. **Classification loop**: for each key, query dispatch-map:
    ///    - MemoryTier hit → issue async H2D DMA (round-robin across 4 streams)
    ///    - BlockDevice hit → collect into cold_entries for phase 2
    ///    - NotExist → handle inline
    ///      After the loop, synchronize all warm streams once.
    ///
    /// 2. **Cold promotion** (if any BlockDevice hits):
    ///    - Group cold entries by drive index
    ///    - Spawn per-drive queue threads (up to 2 per drive)
    ///    - Each thread: evict → insert memory-tier slot → pipelined NVMe reads
    ///      directly into memory-tier → async H2D DMA to GPU
    fn batch_lookup(
        &self,
        entries: &[(CacheKey, Vec<IpcHandle>)],
    ) -> Vec<Result<(), DispatcherError>> {
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

        // Resolve the GPU device this batch's blocks live on (all entries come
        // from a single rank → a single device) and make it current on this
        // thread. Pick the warm/pipeline streams bound to that device: a stream
        // on another GPU makes cudaMemcpyAsync fail with "invalid argument"
        // under tensor parallelism.
        let batch_device = entries
            .iter()
            .flat_map(|(_, regions)| regions.iter().map(|h| h.address))
            .find(|a| !a.is_null())
            .map_or(-1, |addr| set_batch_device(&*gpu, addr));
        let dev_streams = device_streams_for(&*gpu, batch_device);
        let warm_raw = dev_streams.as_ref().map_or(0, |s| s.warm);

        // Classify entries and handle fast paths inline.
        struct ColdEntry {
            idx: usize,
            key: CacheKey,
            offset: u64,
            // One block = N per-layer regions (0.23+); N==1 for a coalesced
            // block (0.20/0.22). They land colocated in one slot of `total_size`
            // bytes, then scatter to the N GPU allocations on load.
            regions: Vec<IpcHandle>,
            total_size: u32,
        }
        // SAFETY: ColdEntry contains raw pointers from IpcHandle (GPU device pointers).
        // These pointers are valid across threads — CUDA IPC handles are designed for
        // cross-process/thread use. We only read the pointer value to pass to CUDA APIs.
        unsafe impl Send for ColdEntry {}
        unsafe impl Sync for ColdEntry {}

        let mut cold_entries: Vec<ColdEntry> = Vec::new();
        let mut deferred_touch_keys: Vec<CacheKey> = Vec::new();
        // Read pins for the warm hits below. The H2D copies are submitted async and
        // synchronized once after the loop, so each pin must survive until *after*
        // that sync: releasing at submission would let the memory-tier evictor
        // reclaim the DRAM slot from under an in-flight DMA.
        let mut warm_pins = pins::PinnedKeys::new(Arc::clone(&dm));

        for (i, (key, regions)) in entries.iter().enumerate() {
            let key = *key;
            // Σ of the per-region sizes = the reserved slot's byte length.
            let total_size: u32 = regions.iter().map(|r| r.size).sum();
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
                        let t_hot = std::time::Instant::now();
                        // Integrity: verify the slot's CRC-32 on the CPU before
                        // the H2D copy feeds the GPU. A mismatch fails the load
                        // for this key instead of serving corrupt KV.
                        #[cfg(feature = "integrity-check")]
                        {
                            if let Err(e) =
                                Self::verify_slot_crc(dm.get_checksum(key), key, pointer, size)
                            {
                                let _ = dm.release_read(key);
                                results[i] = Some(Err(e));
                                continue;
                            }
                        }
                        // Warm hit: defer the stream sync to one batched sync
                        // after the classification loop (synchronize = false).
                        let res = self.serve_memory_tier_to_gpu(
                            &gpu, pointer, size, regions, warm_raw, false,
                        );
                        if let Some(ref m) = *self.pipeline_metrics.read() {
                            m.record_hot_gpu_dma(t_hot.elapsed().as_micros() as f64);
                        }
                        // The copy is only submitted, not complete: hand the pin to
                        // `warm_pins`, which releases it after the batched sync.
                        warm_pins.adopt(key);
                        deferred_touch_keys.push(key);
                        results[i] = Some(res);
                    }
                    LookupResult::BlockDevice { offset } => {
                        let _ = dm.release_read(key);
                        cold_entries.push(ColdEntry {
                            idx: i,
                            key,
                            offset,
                            regions: regions.clone(),
                            total_size,
                        });
                    }
                },
                Err(_) => {
                    results[i] = Some(Err(DispatcherError::KeyNotFound(key)));
                }
            }
        }

        // Batched stream sync: wait for all submitted async DMA copies at once.
        // Sync the same per-device warm stream the fast path issued them on.
        if !deferred_touch_keys.is_empty() {
            if warm_raw != 0 {
                let s = GpuStream(warm_raw as *mut std::ffi::c_void);
                if let Err(e) = gpu.stream_synchronize(s) {
                    self.log_info(&format!("batch stream_synchronize failed: {e}"));
                }
            }
            // Every warm copy has completed: only now is it safe to let the entries
            // become evictable again.
            drop(warm_pins);
            mt.batch_touch(&deferred_touch_keys);
        } else {
            // Nothing was submitted, so nothing is pinned; drop explicitly so the
            // pin lifetime is obvious on both paths rather than left to scope end.
            drop(warm_pins);
        }

        // Promote cold entries in parallel — multiple queue threads per drive.
        // Each thread gets its own NVMe queue pair and CUDA streams, enabling
        // concurrent reads on the same physical drive.
        if !cold_entries.is_empty() {
            const MAX_QUEUES_PER_DRIVE: usize = 2;

            let chunk_size = {
                let ring_guard = self.pipeline_ring.read();
                ring_guard.as_ref().map_or(131072, |r| r.chunk_size)
            };

            let drives = self.data_drives.read();
            let num_drives = drives.len();

            if num_drives == 0 {
                let max_attempts = self.max_eviction_attempts.load(Ordering::Relaxed);
                for entry in &cold_entries {
                    self.evict_for_space(&dm, &mt, entry.total_size, entry.key, max_attempts)
                        .ok();
                    let res = mt
                        .insert(entry.key, entry.total_size)
                        .map(|mem_ptr| {
                            // In-place, pin-safe promote (no remove/recreate).
                            let _ = dm.promote_block_to_memory_tier(
                                entry.key,
                                mem_ptr,
                                entry.total_size,
                            );
                        })
                        .map_err(|e| match e {
                            // Preserve AlreadyExists so the recovery pass below
                            // can re-serve the block warm (concurrent promotion).
                            interfaces::MemoryTierError::AlreadyExists(k) => {
                                DispatcherError::AlreadyExists(k)
                            }
                            other => DispatcherError::AllocationFailed(format!(
                                "promote insert failed: {other}"
                            )),
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

                let pool_guard = self.cold_pool.lock().unwrap();
                let pool = pool_guard.as_ref();
                let queues_per_drive = pool.map_or(MAX_QUEUES_PER_DRIVE, |p| p.queues_per_drive());

                let queue_depth = 128;
                let max_attempts = self.max_eviction_attempts.load(Ordering::Relaxed);
                let pm = self.pipeline_metrics.read();
                let pm_arc: Option<Arc<dyn PipelineMetrics>> = pm.as_ref().map(Arc::clone);
                drop(pm);

                // Cold-load staging: when the tier is saturated (all slots pinned
                // by in-flight loads) a cold load can't get a tier slot. Rather
                // than fail it (which crashes vLLM), such entries are deferred to
                // `staged` and served after the pooled tier jobs, one staging
                // buffer at a time — see the post-pass below. We do NOT check out
                // buffers here: blocking on a checkout while holding `pool_guard`
                // (and accumulating leases that only release at batch end)
                // deadlocks the server.
                let staging_available = self
                    .pipeline_ring
                    .read()
                    .as_ref()
                    .is_some_and(|r| r.staging.is_some());
                let mut staged: Vec<usize> = Vec::new();

                // For each drive, prepare ColdReadJobs and submit to pool (or fallback).
                #[allow(clippy::type_complexity)]
                let mut pending_results: Vec<(
                    Vec<usize>,   // job_ci mapping
                    Vec<*mut u8>, // mem_ptrs
                    crossbeam_channel::Receiver<Vec<Result<(), DispatcherError>>>,
                )> = Vec::new();
                let mut prep_failures: Vec<(usize, Result<(), DispatcherError>)> = Vec::new();

                for (drive_idx, entry_indices) in per_drive.iter().enumerate() {
                    if entry_indices.is_empty() {
                        continue;
                    }

                    let drive = &drives[drive_idx];
                    let block_size = drive.block_dev_iface.block_size();

                    // Split this drive's entries across queue slots.
                    let num_queues = queues_per_drive.min(entry_indices.len());
                    let chunks: Vec<&[usize]> = entry_indices
                        .chunks(entry_indices.len().div_ceil(num_queues))
                        .collect();

                    for (slot, chunk) in chunks.into_iter().enumerate() {
                        let mut jobs: Vec<pipeline::ColdReadJob> = Vec::with_capacity(chunk.len());
                        let mut job_ci: Vec<usize> = Vec::with_capacity(chunk.len());
                        let mut mem_ptrs: Vec<*mut u8> = Vec::with_capacity(chunk.len());

                        for &ci in chunk {
                            let entry = &cold_entries[ci];
                            let ipc_size = entry.total_size;

                            let prep =
                                self.evict_and_insert(&dm, &mt, entry.key, ipc_size, max_attempts);

                            match prep {
                                Ok(mem_ptr) => {
                                    // Single-region (N==1): fuse SSD->GPU as before
                                    // (gpu_dst = the one region). Multi-region (N>1):
                                    // read SSD->DRAM slot only (gpu_dst null), then
                                    // scatter the slot to the N GPU allocations after
                                    // the promote below.
                                    let gpu_dst = if entry.regions.len() == 1 {
                                        entry.regions[0].address as *mut std::ffi::c_void
                                    } else {
                                        std::ptr::null_mut()
                                    };
                                    jobs.push(pipeline::ColdReadJob {
                                        mem_ptr,
                                        gpu_dst,
                                        start_lba: entry.offset / block_size as u64,
                                        total_bytes: ipc_size as usize,
                                    });
                                    job_ci.push(ci);
                                    mem_ptrs.push(mem_ptr);
                                }
                                // Tier saturated: defer to the staging post-pass
                                // (served uncached, one buffer at a time) so the
                                // load still succeeds. Only for AllocationFailed —
                                // AlreadyExists is a concurrent promotion handled
                                // by the recovery pass. Do NOT check out a buffer
                                // here (would block under pool_guard → deadlock).
                                Err(DispatcherError::AllocationFailed(_)) if staging_available => {
                                    staged.push(ci);
                                }
                                Err(e) => {
                                    prep_failures.push((ci, Err(e)));
                                }
                            }
                        }

                        if jobs.is_empty() {
                            continue;
                        }

                        let (result_tx, result_rx) = crossbeam_channel::bounded(1);

                        let request = cold_pool::ColdReadRequest {
                            jobs,
                            chunk_size,
                            queue_depth,
                            metrics: pm_arc.clone(),
                            result_tx,
                            gpu_device: batch_device,
                        };

                        if let Some(p) = pool {
                            if let Err(e) = p.submit(drive_idx, slot, request) {
                                for &ci in &job_ci {
                                    prep_failures.push((ci, Err(e.clone())));
                                }
                                continue;
                            }
                        } else {
                            // Fallback: no pool available, run inline on current thread.
                            let drive_iface = &*drive.block_dev_iface;
                            let gpu_ref = &*gpu;
                            let channels = drive_iface.connect_client();
                            let streams_result = gpu_ref.create_stream().and_then(|a| {
                                gpu_ref.create_stream().map(|b| [a, b]).map_err(|e| {
                                    let _ = gpu_ref.destroy_stream(a);
                                    e
                                })
                            });
                            match (channels, streams_result) {
                                (Ok(ch), Ok(st)) => {
                                    let pipeline_results = unsafe {
                                        pipeline::pipelined_multi_object_zero_copy(
                                            drive_iface,
                                            gpu_ref,
                                            &st,
                                            &ch,
                                            &request.jobs,
                                            request.chunk_size,
                                            request.queue_depth,
                                            request.metrics.as_deref(),
                                        )
                                    };
                                    let _ = gpu_ref.destroy_stream(st[0]);
                                    let _ = gpu_ref.destroy_stream(st[1]);
                                    let _ = request.result_tx.send(pipeline_results);
                                }
                                (Err(e), _) => {
                                    let err = DispatcherError::IoError(format!(
                                        "connect_client failed: {e}"
                                    ));
                                    let _ = request.result_tx.send(
                                        (0..request.jobs.len()).map(|_| Err(err.clone())).collect(),
                                    );
                                }
                                (_, Err(e)) => {
                                    let err = DispatcherError::IoError(format!(
                                        "create_stream failed: {e}"
                                    ));
                                    let _ = request.result_tx.send(
                                        (0..request.jobs.len()).map(|_| Err(err.clone())).collect(),
                                    );
                                }
                            }
                        }

                        pending_results.push((job_ci, mem_ptrs, result_rx));
                    }
                }

                drop(pool_guard);

                // Record prep failures.
                for (ci, res) in prep_failures {
                    results[cold_entries[ci].idx] = Some(res);
                }

                // Collect pipeline results and finalize dispatch-map state.
                for (job_ci, mem_ptrs, result_rx) in pending_results {
                    let pipeline_results = result_rx.recv().unwrap_or_else(|_| {
                        (0..job_ci.len())
                            .map(|_| {
                                Err(DispatcherError::IoError("pool worker disconnected".into()))
                            })
                            .collect()
                    });

                    for (job_idx, result) in pipeline_results.into_iter().enumerate() {
                        let ci = job_ci[job_idx];
                        let entry = &cold_entries[ci];
                        let res = match result {
                            Ok(()) => {
                                // In-place BlockDevice->MemoryTier: preserves the
                                // load's pin (read_ref) and keeps the SSD offset,
                                // so it works on a pinned entry (unlike the old
                                // remove+recreate, whose remove failed on a pin).
                                dm.promote_block_to_memory_tier(
                                    entry.key,
                                    mem_ptrs[job_idx],
                                    entry.total_size,
                                )
                                .map_err(|e| {
                                    DispatcherError::IoError(format!(
                                        "promote transition failed: {e}"
                                    ))
                                })
                                .and_then(|()| {
                                    // Multi-region (N>1): the pipeline only filled
                                    // the DRAM slot (gpu_dst was null). Scatter the
                                    // now-resident slot to the N GPU allocations.
                                    // N==1 already landed on the GPU via the fused
                                    // SSD->GPU path, so skip it there.
                                    if entry.regions.len() > 1 {
                                        self.serve_memory_tier_to_gpu(
                                            &gpu,
                                            mem_ptrs[job_idx],
                                            entry.total_size,
                                            &entry.regions,
                                            warm_raw,
                                            true,
                                        )
                                    } else {
                                        Ok(())
                                    }
                                })
                            }
                            Err(e) => Err(e),
                        };
                        results[cold_entries[ci].idx] = Some(res);
                    }
                }

                // Staging post-pass (pool_guard released): serve tier-saturated
                // cold entries one buffer at a time. Each serve checks out a
                // single staging buffer, does SSD→staging→GPU, and releases it —
                // so at most one lease is held per call and never while holding a
                // lock, making the blocking checkout deadlock-free.
                if !staged.is_empty() {
                    for &ci in &staged {
                        let entry = &cold_entries[ci];
                        let res = self.serve_cold_staged(
                            &gpu,
                            entry.key,
                            entry.offset,
                            &entry.regions,
                            entry.total_size,
                            batch_device,
                        );
                        results[entry.idx] = Some(res);
                    }
                }
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
                // remote-lookup works in CPU/DRAM only: it takes (key, size) and,
                // on success, makes the value resident in the local memory tier.
                // The dispatcher owns the subsequent DRAM->GPU delivery.
                let remote_entries: Vec<(CacheKey, u32)> = not_found
                    .iter()
                    .map(|&i| {
                        let (key, regions) = &entries[i];
                        (*key, regions.iter().map(|r| r.size).sum())
                    })
                    .collect();

                let remote_results = rl.batch_lookup(&remote_entries);

                // On a successful remote fetch the value is now resident in the
                // local memory tier; deliver it to the caller's GPU buffer with the
                // same memory-tier->device scatter used for a local hot-path hit.
                //
                // Submit every copy first and synchronize the warm stream ONCE, the
                // way the local warm path does. Blocking per key instead — one round
                // trip to the GPU for every key in the batch — is what pinned remote
                // lookup to a fixed per-key cost that did not respond to batch size.
                //
                // The read pin each `lookup` takes must therefore outlive the batched
                // sync, not merely the submission; `remote_pins` owns them until then.
                let mut remote_pins = pins::PinnedKeys::new(Arc::clone(&dm));
                let mut submitted: Vec<usize> = Vec::with_capacity(not_found.len());

                for (&pos, remote_res) in not_found.iter().zip(remote_results.into_iter()) {
                    let (key, regions) = &entries[pos];
                    let key = *key;
                    if let Err(e) = remote_res {
                        results[pos] =
                            Some(Err(DispatcherError::IoError(format!("remote lookup: {e}"))));
                        continue;
                    }
                    match dm.lookup(key) {
                        Ok(LookupResult::MemoryTier { pointer, size }) => {
                            // `lookup` pinned the entry; the guard owns that pin now,
                            // including on the failure paths below.
                            remote_pins.adopt(key);
                            // Integrity: verify the slot's CRC-32 on the CPU before
                            // the H2D copy feeds the GPU, as the warm path does.
                            #[cfg(feature = "integrity-check")]
                            {
                                if let Err(e) =
                                    Self::verify_slot_crc(dm.get_checksum(key), key, pointer, size)
                                {
                                    results[pos] = Some(Err(e));
                                    continue;
                                }
                            }
                            let res = self.serve_memory_tier_to_gpu(
                                &gpu, pointer, size, regions, warm_raw, false,
                            );
                            if res.is_ok() {
                                submitted.push(pos);
                            }
                            results[pos] = Some(res);
                        }
                        // A block-tier answer still holds a pin — `lookup` increments
                        // read_ref before it inspects the location — but the value is
                        // not where the remote fetch promised it would be.
                        Ok(LookupResult::BlockDevice { .. }) => {
                            let _ = dm.release_read(key);
                            results[pos] = Some(Err(DispatcherError::KeyNotFound(key)));
                        }
                        // NotExist returns before the read_ref increment, and
                        // MismatchSize takes no pin either, so releasing here would
                        // decrement some *other* reader's pin.
                        Ok(_) => {
                            results[pos] = Some(Err(DispatcherError::KeyNotFound(key)));
                        }
                        Err(e) => {
                            results[pos] = Some(Err(DispatcherError::IoError(format!(
                                "post-remote lookup: {e}"
                            ))));
                        }
                    }
                }

                // One sync for the whole batch, on the same per-device warm stream
                // every submission above was issued to.
                if !submitted.is_empty() && warm_raw != 0 {
                    let s = GpuStream(warm_raw as *mut std::ffi::c_void);
                    if let Err(e) = gpu.stream_synchronize(s) {
                        // The destination buffers may not hold the data, so none of
                        // these keys can be reported as a hit.
                        let msg = format!("remote-delivery stream_synchronize failed: {e}");
                        self.log_warn(&msg);
                        for &pos in &submitted {
                            results[pos] = Some(Err(DispatcherError::IoError(msg.clone())));
                        }
                    }
                }
                // Every remote copy has completed: release the pins.
                drop(remote_pins);
            }
        }

        // Recover concurrent-promotion losers: an entry left as AlreadyExists
        // means a sibling lookup (e.g. the other TP rank) won the mt.insert
        // race for this key. The block is resident (or being read in); wait for
        // the MemoryTier transition and serve the DMA warm instead of failing.
        for (i, (key, regions)) in entries.iter().enumerate() {
            if matches!(results[i], Some(Err(DispatcherError::AlreadyExists(_)))) {
                results[i] =
                    Some(self.serve_concurrently_promoted(&dm, &gpu, *key, regions, warm_raw));
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

        // Make this block's GPU device current and use its warm stream — a
        // stream on another GPU fails cudaMemcpyAsync under multi-GPU.
        let device = set_batch_device(&*gpu, ipc_handle.address);
        let warm_raw = device_streams_for(&*gpu, device).map_or(0, |s| s.warm);

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
                        // Integrity: verify the slot CRC-32 before the H2D copy.
                        #[cfg(feature = "integrity-check")]
                        {
                            if let Err(e) =
                                Self::verify_slot_crc(dm.get_checksum(key), key, pointer, size)
                            {
                                let _ = dm.release_read(key);
                                return Err(e);
                            }
                        }
                        // Synchronous serve via the shared, device-correct helper.
                        let r = self.serve_memory_tier_to_gpu(
                            &gpu,
                            pointer,
                            size,
                            std::slice::from_ref(&ipc_handle),
                            warm_raw,
                            true,
                        );
                        let _ = dm.release_read(key);
                        mt.touch(key);
                        r.map(|()| null_stream)
                    }
                    LookupResult::BlockDevice { offset } => {
                        let _ = dm.release_read(key);
                        self.promote_and_serve(key, offset, &ipc_handle, &gpu, &dm, &mt, device)?;
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

        // Remove from dispatch-map first (prevents new lookups from obtaining
        // the memory-tier pointer while we free it).
        dm.remove(key)
            .map_err(|_| DispatcherError::KeyNotFound(key))?;

        // Now safe to free the DRAM slot — no new readers can find this entry.
        if let Ok(mt) = self.memory_tier.get() {
            let _ = mt.remove(key);
        }

        if let Some(offset) = block_offset {
            let drives = self.data_drives.read();
            let idx = Self::drive_index(key, drives.len().max(1));
            if let Some(drive) = drives.get(idx) {
                let _ = drive.extent_mgr.remove_extent(offset);
            }
        }

        Ok(())
    }

    /// Ingest a new cache entry from GPU memory.
    ///
    /// Flow: evict if needed → allocate memory-tier slot → cudaMemcpy D2H
    /// from client GPU → register in dispatch-map → enqueue background
    /// write-through to SSD.
    fn populate(&self, key: CacheKey, ipc_handle: IpcHandle) -> Result<(), DispatcherError> {
        self.ensure_initialized()?;
        let t_total = std::time::Instant::now();

        if ipc_handle.size == 0 {
            return Err(DispatcherError::InvalidParameter(
                "IPC handle size must be > 0".into(),
            ));
        }

        let size: u32 = ipc_handle.size;

        // Phase 1: Evict if needed and allocate memory-tier slot.
        let t_alloc = std::time::Instant::now();
        // Internal populate path has no client session context.
        let _mem_ptr = self.reserve_memory(key, size, 0)?;
        let alloc_us = t_alloc.elapsed().as_micros() as f64;

        // Phase 2: Async DMA copy from GPU into the reserved slot, then sync.
        let t_d2h = std::time::Instant::now();
        let gpu = self
            .gpu_services
            .get()
            .map_err(|_| DispatcherError::NotInitialized("gpu_services not bound".into()))?;
        // D2H source is on the GPU (ipc_handle.address); make that device current
        // and use its warm stream so cudaMemcpyAsync doesn't reject a cross-GPU
        // stream under tensor parallelism.
        let device = set_batch_device(&*gpu, ipc_handle.address);
        let warm_raw = device_streams_for(&*gpu, device).map_or(0, |s| s.warm);
        let stream = GpuStream(warm_raw as *mut std::ffi::c_void);
        self.copy_gpu_to_memory_async(key, std::slice::from_ref(&ipc_handle), stream)?;
        gpu.stream_synchronize(stream)
            .map_err(|e| DispatcherError::IoError(format!("stream_synchronize failed: {e}")))?;
        let d2h_us = t_d2h.elapsed().as_micros() as f64;

        // Phase 3: Register in dispatch-map and enqueue SSD write-through.
        self.copy_gpu_to_memory_completed(key, size)?;

        if let Some(ref m) = *self.pipeline_metrics.read() {
            m.record_populate_alloc(alloc_us);
            m.record_populate_gpu_d2h(d2h_us);
            m.record_populate_total(t_total.elapsed().as_micros() as f64);
        }

        Ok(())
    }

    fn reserve_memory(
        &self,
        key: CacheKey,
        size: u32,
        session_id: u64,
    ) -> Result<*mut u8, DispatcherError> {
        self.ensure_initialized()?;

        // Per-call reserve logging is opt-in: it is noisy under load and only
        // useful for session-id observability. Enable by setting
        // CERTUS_LOG_RESERVE to a truthy value (1/true/yes/on). Read once.
        fn reserve_logging_enabled() -> bool {
            static ENABLED: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
            *ENABLED.get_or_init(|| {
                std::env::var("CERTUS_LOG_RESERVE")
                    .map(|v| matches!(v.trim(), "1" | "true" | "TRUE" | "yes" | "on"))
                    .unwrap_or(false)
            })
        }
        if reserve_logging_enabled() {
            self.log_info(&format!(
                "reserve_memory: key={key} size={size} session_id={session_id}"
            ));
        }

        if size == 0 {
            return Err(DispatcherError::InvalidParameter("size must be > 0".into()));
        }

        let dm = self
            .dispatch_map
            .get()
            .map_err(|_| DispatcherError::NotInitialized("dispatch_map not bound".into()))?;

        let mt = self
            .memory_tier
            .get()
            .map_err(|_| DispatcherError::NotInitialized("memory_tier not bound".into()))?;

        let max_attempts = self.max_eviction_attempts.load(Ordering::Relaxed);
        let mem_ptr = self.evict_and_insert(&dm, &mt, key, size, max_attempts)?;

        Ok(mem_ptr)
    }

    fn copy_gpu_to_memory_async(
        &self,
        key: CacheKey,
        regions: &[IpcHandle],
        stream: GpuStream,
    ) -> Result<(), DispatcherError> {
        self.ensure_initialized()?;

        let mt = self
            .memory_tier
            .get()
            .map_err(|_| DispatcherError::NotInitialized("memory_tier not bound".into()))?;

        let (mem_ptr, slot_size) = mt.get(key).ok_or(DispatcherError::KeyNotFound(key))?;

        // Gather the client's N GPU regions contiguously into the one reserved
        // slot: region L lands at `slot + sum(sizes[0..L])`. A single-region
        // block (legacy / populate) is the L==0 case. Reject a region set that
        // overflows the slot rather than corrupt neighbouring DRAM.
        let total: usize = regions.iter().map(|r| r.size as usize).sum();
        if total > slot_size as usize {
            return Err(DispatcherError::InvalidParameter(format!(
                "regions total {total} bytes exceeds reserved slot {slot_size} for key {key}"
            )));
        }

        let gpu = self
            .gpu_services
            .get()
            .map_err(|_| DispatcherError::NotInitialized("gpu_services not bound".into()))?;

        let mut off: usize = 0;
        for region in regions {
            // Raw-dst D2H: source is the GPU region, destination is the slot at
            // the running offset. No DmaBuffer wrap needed (memcpy_d2h_async
            // takes a raw pinned host pointer, which the slot is).
            gpu.memcpy_d2h_async(
                region.address as *const std::ffi::c_void,
                (mem_ptr as usize + off) as *mut std::ffi::c_void,
                region.size as usize,
                stream,
            )
            .map_err(|e| {
                let _ = mt.remove(key);
                DispatcherError::IoError(format!("GPU async DMA copy failed: {e}"))
            })?;
            off += region.size as usize;
        }
        Ok(())
    }

    fn copy_gpu_to_memory_completed(
        &self,
        key: CacheKey,
        size: u32,
    ) -> Result<(), DispatcherError> {
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

        // Integrity: compute the block's CRC-32 as soon as it has landed. The
        // store D2H (issued on the null/default stream by copy_to_store) is
        // async, so we synchronize it first, then hash the just-created slot
        // while the write-ref still holds it exclusively, and record the
        // checksum on the index entry. It travels with the entry through
        // demote/promote and is verified before the load H2D.
        #[cfg(feature = "integrity-check")]
        {
            let gpu = self
                .gpu_services
                .get()
                .map_err(|_| DispatcherError::NotInitialized("gpu_services not bound".into()))?;
            gpu.stream_synchronize(GpuStream(std::ptr::null_mut()))
                .map_err(|e| {
                    let _ = mt.remove(key);
                    DispatcherError::IoError(format!(
                        "integrity: stream_synchronize before hash failed: {e}"
                    ))
                })?;
            // SAFETY: the entry was just created with write_ref==1 (exclusive),
            // so no reader/writer can touch the slot; `size` bytes at `mem_ptr`
            // are the reserved, now-populated slot.
            let bytes = unsafe { std::slice::from_raw_parts(mem_ptr as *const u8, size as usize) };
            let mut hasher = crc32fast::Hasher::new();
            hasher.update(bytes);
            let crc = hasher.finalize();
            dm.set_checksum(key, crc).map_err(|e| {
                let _ = mt.remove(key);
                DispatcherError::IoError(format!("integrity: set_checksum failed: {e}"))
            })?;
        }

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

    fn pin(&self, key: CacheKey) -> Result<(), DispatcherError> {
        self.ensure_initialized()?;

        let dm = self
            .dispatch_map
            .get()
            .map_err(|_| DispatcherError::NotInitialized("dispatch_map not bound".into()))?;

        dm.take_read(key).map_err(|e| match e {
            interfaces::DispatchMapError::KeyNotFound(k) => DispatcherError::KeyNotFound(k),
            interfaces::DispatchMapError::Timeout(k) => {
                DispatcherError::Timeout(format!("timeout waiting on key: {k}"))
            }
            other => DispatcherError::IoError(other.to_string()),
        })
    }

    fn unpin(&self, key: CacheKey) -> Result<(), DispatcherError> {
        self.ensure_initialized()?;

        let dm = self
            .dispatch_map
            .get()
            .map_err(|_| DispatcherError::NotInitialized("dispatch_map not bound".into()))?;

        dm.release_read(key).map_err(|e| match e {
            interfaces::DispatchMapError::KeyNotFound(k) => DispatcherError::KeyNotFound(k),
            interfaces::DispatchMapError::RefCountUnderflow(k) => DispatcherError::KeyNotFound(k),
            other => DispatcherError::IoError(other.to_string()),
        })
    }

    fn touch(&self, key: CacheKey) -> Result<(), DispatcherError> {
        self.ensure_initialized()?;

        let dm = self
            .dispatch_map
            .get()
            .map_err(|_| DispatcherError::NotInitialized("dispatch_map not bound".into()))?;

        dm.touch(key)
            .map_err(|_| DispatcherError::KeyNotFound(key))?;

        if let Ok(mt) = self.memory_tier.get() {
            mt.touch(key);
        }

        Ok(())
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
                    if let Ok(size) = dm.entry_size(key) {
                        cold_entries.push(ColdEntry { key, offset, size });
                    }
                }
                Ok(LookupResult::MemoryTier { .. }) => {
                    let _ = dm.release_read(key);
                    let _ = dm.touch(key);
                    mt.touch(key);
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
            let max_attempts = self.max_eviction_attempts.load(Ordering::Relaxed);
            for entry in &cold_entries {
                if self
                    .evict_for_space(&dm, &mt, entry.size, entry.key, max_attempts)
                    .is_err()
                {
                    continue;
                }
                if let Ok(mem_ptr) = mt.insert(entry.key, entry.size) {
                    // In-place promote (pin-safe): no remove/recreate.
                    let _ = dm.promote_block_to_memory_tier(entry.key, mem_ptr, entry.size);
                }
            }
            return;
        }

        let chunk_size = {
            let ring_guard = self.pipeline_ring.read();
            ring_guard.as_ref().map_or(131072, |r| r.chunk_size)
        };

        // Group cold entries by target drive.
        let mut per_drive: Vec<Vec<usize>> = vec![Vec::new(); num_drives];
        for (i, entry) in cold_entries.iter().enumerate() {
            let drive_idx = Self::drive_index(entry.key, num_drives);
            per_drive[drive_idx].push(i);
        }

        let max_attempts = self.max_eviction_attempts.load(Ordering::Relaxed);

        std::thread::scope(|s| {
            for (drive_idx, entry_indices) in per_drive.iter().enumerate() {
                if entry_indices.is_empty() {
                    continue;
                }

                let drive = &drives[drive_idx];
                let channel_pool = &drive.channel_pool;
                let block_dev = Arc::clone(&drive.block_dev_iface);
                let dm = &dm;
                let mt = &mt;
                let cold = &cold_entries;
                let logger = &logger;

                s.spawn(move || {
                    // Exclusive channel for this prefetch thread. This runs
                    // concurrently with batch_lookup on the same drive, so it
                    // must not share one SPSC channel (see ChannelPool).
                    let chan_lease = match channel_pool.checkout() {
                        Ok(l) => l,
                        Err(_) => return,
                    };
                    let channels = chan_lease.channels();
                    for &ci in entry_indices {
                        let entry = &cold[ci];
                        let block_size = block_dev.block_size() as u64;
                        let start_lba = entry.offset / block_size;

                        if self
                            .evict_for_space(dm, mt, entry.size, entry.key, max_attempts)
                            .is_err()
                        {
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

                        // In-place, pin-safe promote (no remove/recreate).
                        let _ = dm.promote_block_to_memory_tier(entry.key, mem_ptr, entry.size);
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
        loop {
            let candidates = mt.oldest_keys(64);
            if candidates.is_empty() {
                break;
            }
            for key in candidates {
                if dm.try_evict_to_block(key).is_ok() {
                    let _ = mt.remove(key);
                } else {
                    // Entry not evictable (no SSD copy) — force remove.
                    let _ = mt.remove(key);
                    let _ = dm.remove(key);
                }
                count += 1;
            }
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

    fn read_write_stats(&self) -> interfaces::ReadWriteStats {
        // Aggregate per-direction counters across every data drive. Each drive's
        // block device tracks its own SSD read/write bytes+ops (zeroed unless
        // built with the telemetry feature); sum for the dispatcher-wide total.
        let mut agg = interfaces::ReadWriteStats::default();
        for drive in self.data_drives.read().iter() {
            let s = drive.block_dev_iface.read_write_stats();
            agg.read_ops += s.read_ops;
            agg.read_bytes += s.read_bytes;
            agg.read_latency_ns_sum += s.read_latency_ns_sum;
            agg.write_ops += s.write_ops;
            agg.write_bytes += s.write_bytes;
            agg.write_latency_ns_sum += s.write_latency_ns_sum;
        }
        agg
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
        CacheKey, DispatchMapError, DmaBuffer, GpuDeviceInfo, GpuDmaBuffer, GpuIpcHandle,
        GpuStream, IMemoryTier, LookupConfig, LookupResult, MemoryTierError,
        MemoryTierTelemetrySnapshot, RemoteLookupError,
    };

    // -----------------------------------------------------------------------
    // Test infrastructure
    // -----------------------------------------------------------------------

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
        fn initialize(
            &self,
            _pool_size: usize,
            _numa_node: Option<i32>,
        ) -> Result<(), MemoryTierError> {
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

        fn evict_next(&self) -> Option<CacheKey> {
            let mut inner = self.inner.lock().unwrap();
            let key = inner.slots.keys().next().copied()?;
            let slot = inner.slots.remove(&key).unwrap();
            let aligned = (slot.size as usize).next_multiple_of(4096);
            inner.used = inner.used.saturating_sub(aligned);
            Some(key)
        }

        fn evict_next_for_key(&self, _key: CacheKey) -> Option<CacheKey> {
            self.evict_next()
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

        fn telemetry_snapshot(&self) -> MemoryTierTelemetrySnapshot {
            MemoryTierTelemetrySnapshot::default()
        }
    }

    // --- MockDispatchMap ---

    enum MockEntryLocation {
        MemoryTier {
            pointer: *mut u8,
            size: u32,
            ssd_offset: Option<u64>,
        },
        BlockDevice {
            offset: u64,
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
        // Keys that should flip BlockDevice -> MemoryTier the *next* time they
        // are looked up, simulating a concurrent promotion winner finishing its
        // SSD->DRAM read between our classification and recovery. Pointer stored
        // as usize to keep MockDmInner Send without an unsafe impl.
        flip_on_next_lookup: HashMap<CacheKey, (usize, u32)>,
        #[cfg(feature = "integrity-check")]
        checksums: HashMap<CacheKey, u32>,
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
                    flip_on_next_lookup: HashMap::new(),
                    #[cfg(feature = "integrity-check")]
                    checksums: HashMap::new(),
                }),
            }
        }

        fn entry_count(&self) -> usize {
            self.inner.lock().unwrap().entries.len()
        }

        fn set_mismatch_key(&self, key: CacheKey) {
            self.inner.lock().unwrap().mismatch_keys.insert(key);
        }

        /// Outstanding read pins on one key.
        fn read_refs(&self, key: CacheKey) -> u32 {
            self.inner
                .lock()
                .unwrap()
                .entries
                .get(&key)
                .map_or(0, |e| e.read_refs)
        }

        /// Outstanding read pins across every entry — what a pin probe samples at
        /// each GPU call to prove a pin was held at that instant.
        fn total_read_refs(&self) -> u32 {
            self.inner
                .lock()
                .unwrap()
                .entries
                .values()
                .map(|e| e.read_refs)
                .sum()
        }

        /// Arm a one-shot flip: the next `lookup(key)` still reports the current
        /// (BlockDevice) classification, but installs a MemoryTier pointer so the
        /// *following* lookup observes MemoryTier — mimicking the racing lookup
        /// that won the `mt.insert` and finished promoting.
        fn flip_to_memory_tier_on_next_lookup(&self, key: CacheKey, pointer: *mut u8, size: u32) {
            self.inner
                .lock()
                .unwrap()
                .flip_on_next_lookup
                .insert(key, (pointer as usize, size));
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
        fn initialize(&self) -> Result<(), DispatchMapError> {
            Ok(())
        }

        fn lookup(&self, key: CacheKey) -> Result<LookupResult, DispatchMapError> {
            let mut inner = self.inner.lock().unwrap();
            if inner.mismatch_keys.contains(&key) {
                return Ok(LookupResult::MismatchSize);
            }
            let result = match inner.entries.get(&key) {
                None => return Ok(LookupResult::NotExist),
                Some(entry) => match &entry.location {
                    MockEntryLocation::MemoryTier {
                        pointer,
                        size,
                        ssd_offset,
                    } => match ssd_offset {
                        Some(offset) if pointer.is_null() => {
                            LookupResult::BlockDevice { offset: *offset }
                        }
                        _ => LookupResult::MemoryTier {
                            pointer: *pointer,
                            size: *size,
                        },
                    },
                    MockEntryLocation::BlockDevice { offset } => {
                        LookupResult::BlockDevice { offset: *offset }
                    }
                },
            };
            // The real dispatch map increments read_ref *before* it inspects the
            // location, so MemoryTier and BlockDevice answers both come back pinned
            // while NotExist and MismatchSize return earlier, unpinned. Model that
            // faithfully: whether a pin outlives an async H2D copy depends on it.
            if matches!(
                result,
                LookupResult::MemoryTier { .. } | LookupResult::BlockDevice { .. }
            ) {
                if let Some(entry) = inner.entries.get_mut(&key) {
                    entry.read_refs += 1;
                }
            }
            // One-shot promotion race simulation: after classifying this key as
            // cold, install a MemoryTier pointer so the recovery lookup sees it.
            if matches!(result, LookupResult::BlockDevice { .. }) {
                if let Some((ptr, size)) = inner.flip_on_next_lookup.remove(&key) {
                    if let Some(entry) = inner.entries.get_mut(&key) {
                        entry.location = MockEntryLocation::MemoryTier {
                            pointer: ptr as *mut u8,
                            size,
                            ssd_offset: Some(0),
                        };
                    }
                }
            }
            Ok(result)
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
                        MockEntryLocation::BlockDevice { .. } => {}
                    }
                    // The real map consumes the read ref that `downgrade_reference`
                    // installed at populate time, pairing the two.
                    if entry.read_refs > 0 {
                        entry.read_refs -= 1;
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

        fn promote_block_to_memory_tier(
            &self,
            key: CacheKey,
            pointer: *mut u8,
            size: u32,
        ) -> Result<(), DispatchMapError> {
            if size == 0 {
                return Err(DispatchMapError::InvalidSize);
            }
            let mut inner = self.inner.lock().unwrap();
            match inner.entries.get_mut(&key) {
                None => Err(DispatchMapError::KeyNotFound(key)),
                Some(entry) => {
                    // Mock only models a MemoryTier location; flip pointer/size
                    // in place, preserving refs (no remove/recreate).
                    entry.location = MockEntryLocation::MemoryTier {
                        pointer,
                        size,
                        ssd_offset: Some(0),
                    };
                    Ok(())
                }
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

        fn try_evict_to_block(&self, key: CacheKey) -> Result<(), DispatchMapError> {
            let mut inner = self.inner.lock().unwrap();
            let entry = inner
                .entries
                .get_mut(&key)
                .ok_or(DispatchMapError::KeyNotFound(key))?;
            // Mirror the real dispatch-map: a pinned entry (read/write ref held
            // by an in-flight load or write-back) is not evictable.
            if entry.read_refs > 0 || entry.write_ref {
                return Err(DispatchMapError::ActiveReferences(key));
            }
            match &entry.location {
                MockEntryLocation::MemoryTier {
                    ssd_offset: Some(offset),
                    ..
                } => {
                    let offset = *offset;
                    entry.location = MockEntryLocation::BlockDevice { offset };
                    Ok(())
                }
                _ => Err(DispatchMapError::InvalidState("not evictable".into())),
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

        #[cfg(feature = "integrity-check")]
        fn set_checksum(&self, key: CacheKey, checksum: u32) -> Result<(), DispatchMapError> {
            let mut inner = self.inner.lock().unwrap();
            if !inner.entries.contains_key(&key) {
                return Err(DispatchMapError::KeyNotFound(key));
            }
            inner.checksums.insert(key, checksum);
            Ok(())
        }

        #[cfg(feature = "integrity-check")]
        fn get_checksum(&self, key: CacheKey) -> Option<u32> {
            self.inner
                .lock()
                .unwrap()
                .checksums
                .get(&key)
                .copied()
                .filter(|&c| c != 0)
        }
    }

    struct MockLogger;

    impl ILogger for MockLogger {
        fn error(&self, _msg: &str) {}
        fn warn(&self, _msg: &str) {}
        fn info(&self, _msg: &str) {}
        fn debug(&self, _msg: &str) {}
    }

    /// A GPU call the pin probe watches.
    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    enum GpuEvent {
        SubmitH2d,
        Sync,
    }

    /// Records the dispatch-map's total read-pin count at each GPU call.
    ///
    /// This is what makes the pin-lifetime invariant testable: a pin must still be
    /// held when a copy is *submitted* and when the batched sync runs, because that
    /// pin is the only thing keeping the memory-tier evictor off the DRAM slot the
    /// DMA is reading. The event order also pins down the batching shape — one sync
    /// per batch rather than one per key.
    struct PinProbe {
        dm: Arc<MockDispatchMap>,
        events: Mutex<Vec<(GpuEvent, u32)>>,
        submissions: AtomicUsize,
        /// 1-based index of the submission to fail; 0 fails none.
        fail_submit_on: usize,
        fail_sync: bool,
        /// Report every pointer as belonging to no GPU, so the dispatcher resolves
        /// no per-device streams and takes the synchronous fallback copy.
        no_device: bool,
    }

    impl PinProbe {
        fn new(dm: Arc<MockDispatchMap>) -> Self {
            Self {
                dm,
                events: Mutex::new(Vec::new()),
                submissions: AtomicUsize::new(0),
                fail_submit_on: 0,
                fail_sync: false,
                no_device: false,
            }
        }

        fn failing_submit_on(dm: Arc<MockDispatchMap>, nth: usize) -> Self {
            Self {
                fail_submit_on: nth,
                ..Self::new(dm)
            }
        }

        fn failing_sync(dm: Arc<MockDispatchMap>) -> Self {
            Self {
                fail_sync: true,
                ..Self::new(dm)
            }
        }

        fn without_gpu_device(dm: Arc<MockDispatchMap>) -> Self {
            Self {
                no_device: true,
                ..Self::new(dm)
            }
        }

        fn record(&self, event: GpuEvent) {
            let pins = self.dm.total_read_refs();
            self.events.lock().unwrap().push((event, pins));
        }

        fn events(&self) -> Vec<(GpuEvent, u32)> {
            self.events.lock().unwrap().clone()
        }

        fn count(&self, event: GpuEvent) -> usize {
            self.events().iter().filter(|(e, _)| *e == event).count()
        }
    }

    #[derive(Default)]
    struct MockGpuServices {
        probe: Option<Arc<PinProbe>>,
    }

    impl MockGpuServices {
        fn with_probe(probe: Arc<PinProbe>) -> Self {
            Self { probe: Some(probe) }
        }
    }

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
        fn set_device(&self, _device: i32) -> Result<(), String> {
            Ok(())
        }
        fn device_of_ptr(&self, _ptr: *const std::ffi::c_void) -> Result<i32, String> {
            if self.probe.as_ref().is_some_and(|p| p.no_device) {
                return Ok(-1);
            }
            Ok(0)
        }
        fn stream_query(&self, _stream: GpuStream) -> Result<bool, String> {
            Ok(true)
        }
        fn destroy_stream(&self, _stream: GpuStream) -> Result<(), String> {
            Ok(())
        }
        fn stream_synchronize(&self, _stream: GpuStream) -> Result<(), String> {
            if let Some(ref p) = self.probe {
                p.record(GpuEvent::Sync);
                if p.fail_sync {
                    return Err("mock: injected stream_synchronize failure".into());
                }
            }
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
            if let Some(ref p) = self.probe {
                p.record(GpuEvent::SubmitH2d);
                let nth = p.submissions.fetch_add(1, Ordering::Relaxed) + 1;
                if p.fail_submit_on == nth {
                    return Err("mock: injected H2D submission failure".into());
                }
            }
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

    fn setup_initialized() -> (Arc<DispatcherComponent>, Arc<MockDispatchMap>) {
        let dm = Arc::new(MockDispatchMap::new());
        let logger: Arc<dyn ILogger + Send + Sync> = Arc::new(MockLogger);
        let gpu: Arc<dyn IGpuServices + Send + Sync> = Arc::new(MockGpuServices::default());
        let mt: Arc<dyn IMemoryTier + Send + Sync> = Arc::new(MockMemoryTier::new(1024 * 1024));
        let c = DispatcherComponent::new(
            AtomicBool::new(false),
            Mutex::new(None),
            Mutex::new(None),
            Mutex::new(None),
            Mutex::new(None),
            RwLock::new(Vec::new()),
            RwLock::new(None),
            AtomicU64::new(0),
            Mutex::new(None),
            Mutex::new(None),
            AtomicUsize::new(2048),
            RwLock::new(None),
            Arc::new(Mutex::new(None)),
            AtomicU64::new(0),
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

    // --- MockRemoteLookup ---

    /// A remote peer holding a known set of keys.
    ///
    /// On a hit it does what the real component's publish-on-success path does: makes
    /// the value resident in the local memory tier, registers a MemoryTier entry, and
    /// releases the write ref — so the key is published **unpinned** and the
    /// dispatcher's delivery has to take its own read pin. Each slot is filled with a
    /// per-key byte pattern so a test can prove the right bytes reached the caller.
    struct MockRemoteLookup {
        dm: Arc<MockDispatchMap>,
        mt: Arc<MockMemoryTier>,
        held: HashSet<CacheKey>,
    }

    impl MockRemoteLookup {
        fn new(dm: Arc<MockDispatchMap>, mt: Arc<MockMemoryTier>, held: &[CacheKey]) -> Self {
            Self {
                dm,
                mt,
                held: held.iter().copied().collect(),
            }
        }

        /// The byte a remotely-fetched `key`'s slot is filled with.
        fn fill_byte(key: CacheKey) -> u8 {
            (key as u8).wrapping_add(0xA0)
        }
    }

    impl IRemoteLookup for MockRemoteLookup {
        fn initialize(&self, _config: LookupConfig) -> Result<(), RemoteLookupError> {
            Ok(())
        }

        fn batch_lookup(&self, entries: &[(CacheKey, u32)]) -> Vec<Result<(), RemoteLookupError>> {
            entries
                .iter()
                .map(|&(key, size)| {
                    if !self.held.contains(&key) {
                        return Err(RemoteLookupError::NotFound);
                    }
                    let ptr = self
                        .mt
                        .insert(key, size)
                        .map_err(|e| RemoteLookupError::TransportError(e.to_string()))?;
                    // SAFETY: `insert` returned a slot of at least `size` bytes.
                    unsafe {
                        std::ptr::write_bytes(ptr, Self::fill_byte(key), size as usize);
                    }
                    self.dm
                        .create_memory_tier_entry(key, ptr, size)
                        .map_err(|e| RemoteLookupError::TransportError(e.to_string()))?;
                    // The real actor releases the write ref before marking the
                    // operation satisfied, so the key is published unpinned.
                    self.dm
                        .release_write(key)
                        .map_err(|e| RemoteLookupError::TransportError(e.to_string()))?;
                    Ok(())
                })
                .collect()
        }

        fn join_cluster(&self, _endpoint: &str) -> Result<(), RemoteLookupError> {
            Ok(())
        }

        fn leave_cluster(&self) -> Result<(), RemoteLookupError> {
            Ok(())
        }
    }

    struct RemoteFixture {
        component: Arc<DispatcherComponent>,
        dm: Arc<MockDispatchMap>,
        mt: Arc<MockMemoryTier>,
        probe: Arc<PinProbe>,
    }

    impl RemoteFixture {
        /// Make `key` resident and registered as a warm entry, bypassing `populate`.
        ///
        /// `populate` enqueues a background write whose `convert_to_storage` releases
        /// the populate-time read pin at an arbitrary later moment, which would race
        /// any assertion about pin counts. Installing the entry directly — the same
        /// insert/register/release-write sequence the remote peer uses — keeps the
        /// accounting deterministic.
        fn install_warm_key(&self, key: CacheKey, fill: u8, size: u32) {
            let ptr = self.mt.insert(key, size).expect("mock tier insert");
            // SAFETY: `insert` returned a slot of at least `size` bytes.
            unsafe {
                std::ptr::write_bytes(ptr, fill, size as usize);
            }
            self.dm.create_memory_tier_entry(key, ptr, size).unwrap();
            self.dm.release_write(key).unwrap();
        }
    }

    /// Like [`setup_initialized`], but with the `remote_lookup` receptacle connected
    /// to a peer holding `held`, and a pin probe watching the GPU mock.
    ///
    /// `make_probe` is handed the dispatch map so a test can pick a failure-injecting
    /// probe. The receptacle is connected *before* `initialize()`, which calls
    /// `join_cluster` on it.
    fn setup_initialized_with_remote(
        held: &[CacheKey],
        make_probe: impl FnOnce(Arc<MockDispatchMap>) -> PinProbe,
    ) -> RemoteFixture {
        let dm = Arc::new(MockDispatchMap::new());
        let mt = Arc::new(MockMemoryTier::new(1024 * 1024));
        let probe = Arc::new(make_probe(Arc::clone(&dm)));
        let logger: Arc<dyn ILogger + Send + Sync> = Arc::new(MockLogger);
        let gpu: Arc<dyn IGpuServices + Send + Sync> =
            Arc::new(MockGpuServices::with_probe(Arc::clone(&probe)));
        let rl: Arc<dyn IRemoteLookup + Send + Sync> = Arc::new(MockRemoteLookup::new(
            Arc::clone(&dm),
            Arc::clone(&mt),
            held,
        ));
        let c = DispatcherComponent::new(
            AtomicBool::new(false),
            Mutex::new(None),
            Mutex::new(None),
            Mutex::new(None),
            Mutex::new(None),
            RwLock::new(Vec::new()),
            RwLock::new(None),
            AtomicU64::new(0),
            Mutex::new(None),
            Mutex::new(None),
            AtomicUsize::new(2048),
            RwLock::new(None),
            Arc::new(Mutex::new(None)),
            AtomicU64::new(0),
        );
        c.dispatch_map
            .connect(Arc::clone(&dm) as Arc<dyn IDispatchMap + Send + Sync>)
            .unwrap();
        c.logger.connect(logger).unwrap();
        c.gpu_services.connect(gpu).unwrap();
        c.memory_tier
            .connect(Arc::clone(&mt) as Arc<dyn IMemoryTier + Send + Sync>)
            .unwrap();
        c.remote_lookup.connect(rl).unwrap();

        let d = query_interface!(c, IDispatcher).unwrap();
        d.initialize(DispatcherConfig {
            data_pci_addrs: vec!["0000:02:00.0".to_string()],
            ..Default::default()
        })
        .unwrap();

        RemoteFixture {
            component: c,
            dm,
            mt,
            probe,
        }
    }

    // -----------------------------------------------------------------------
    // Pre-initialization tests (existing)
    // -----------------------------------------------------------------------

    #[test]
    fn component_creation() {
        let _c = DispatcherComponent::new(
            AtomicBool::new(false),
            Mutex::new(None),
            Mutex::new(None),
            Mutex::new(None),
            Mutex::new(None),
            RwLock::new(Vec::new()),
            RwLock::new(None),
            AtomicU64::new(0),
            Mutex::new(None),
            Mutex::new(None),
            AtomicUsize::new(2048),
            RwLock::new(None),
            Arc::new(Mutex::new(None)),
            AtomicU64::new(0),
        );
    }

    #[test]
    fn query_idispatcher() {
        let c = DispatcherComponent::new(
            AtomicBool::new(false),
            Mutex::new(None),
            Mutex::new(None),
            Mutex::new(None),
            Mutex::new(None),
            RwLock::new(Vec::new()),
            RwLock::new(None),
            AtomicU64::new(0),
            Mutex::new(None),
            Mutex::new(None),
            AtomicUsize::new(2048),
            RwLock::new(None),
            Arc::new(Mutex::new(None)),
            AtomicU64::new(0),
        );
        let d = query_interface!(c, IDispatcher);
        assert!(d.is_some());
    }

    #[test]
    fn initialize_without_receptacles_fails() {
        let c = DispatcherComponent::new(
            AtomicBool::new(false),
            Mutex::new(None),
            Mutex::new(None),
            Mutex::new(None),
            Mutex::new(None),
            RwLock::new(Vec::new()),
            RwLock::new(None),
            AtomicU64::new(0),
            Mutex::new(None),
            Mutex::new(None),
            AtomicUsize::new(2048),
            RwLock::new(None),
            Arc::new(Mutex::new(None)),
            AtomicU64::new(0),
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
        let c = DispatcherComponent::new(
            AtomicBool::new(false),
            Mutex::new(None),
            Mutex::new(None),
            Mutex::new(None),
            Mutex::new(None),
            RwLock::new(Vec::new()),
            RwLock::new(None),
            AtomicU64::new(0),
            Mutex::new(None),
            Mutex::new(None),
            AtomicUsize::new(2048),
            RwLock::new(None),
            Arc::new(Mutex::new(None)),
            AtomicU64::new(0),
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
        let c = DispatcherComponent::new(
            AtomicBool::new(false),
            Mutex::new(None),
            Mutex::new(None),
            Mutex::new(None),
            Mutex::new(None),
            RwLock::new(Vec::new()),
            RwLock::new(None),
            AtomicU64::new(0),
            Mutex::new(None),
            Mutex::new(None),
            AtomicUsize::new(2048),
            RwLock::new(None),
            Arc::new(Mutex::new(None)),
            AtomicU64::new(0),
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
        let c = DispatcherComponent::new(
            AtomicBool::new(false),
            Mutex::new(None),
            Mutex::new(None),
            Mutex::new(None),
            Mutex::new(None),
            RwLock::new(Vec::new()),
            RwLock::new(None),
            AtomicU64::new(0),
            Mutex::new(None),
            Mutex::new(None),
            AtomicUsize::new(2048),
            RwLock::new(None),
            Arc::new(Mutex::new(None)),
            AtomicU64::new(0),
        );
        let d = query_interface!(c, IDispatcher).unwrap();
        let err = d.check(42);
        assert!(matches!(err, Err(DispatcherError::NotInitialized(_))));
    }

    #[test]
    fn remove_before_initialize_fails() {
        let c = DispatcherComponent::new(
            AtomicBool::new(false),
            Mutex::new(None),
            Mutex::new(None),
            Mutex::new(None),
            Mutex::new(None),
            RwLock::new(Vec::new()),
            RwLock::new(None),
            AtomicU64::new(0),
            Mutex::new(None),
            Mutex::new(None),
            AtomicUsize::new(2048),
            RwLock::new(None),
            Arc::new(Mutex::new(None)),
            AtomicU64::new(0),
        );
        let d = query_interface!(c, IDispatcher).unwrap();
        let err = d.remove(42);
        assert!(matches!(err, Err(DispatcherError::NotInitialized(_))));
    }

    #[test]
    fn populate_before_initialize_fails() {
        let c = DispatcherComponent::new(
            AtomicBool::new(false),
            Mutex::new(None),
            Mutex::new(None),
            Mutex::new(None),
            Mutex::new(None),
            RwLock::new(Vec::new()),
            RwLock::new(None),
            AtomicU64::new(0),
            Mutex::new(None),
            Mutex::new(None),
            AtomicUsize::new(2048),
            RwLock::new(None),
            Arc::new(Mutex::new(None)),
            AtomicU64::new(0),
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
        let c = DispatcherComponent::new(
            AtomicBool::new(false),
            Mutex::new(None),
            Mutex::new(None),
            Mutex::new(None),
            Mutex::new(None),
            RwLock::new(Vec::new()),
            RwLock::new(None),
            AtomicU64::new(0),
            Mutex::new(None),
            Mutex::new(None),
            AtomicUsize::new(2048),
            RwLock::new(None),
            Arc::new(Mutex::new(None)),
            AtomicU64::new(0),
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
        let c = DispatcherComponent::new(
            AtomicBool::new(false),
            Mutex::new(None),
            Mutex::new(None),
            Mutex::new(None),
            Mutex::new(None),
            RwLock::new(Vec::new()),
            RwLock::new(None),
            AtomicU64::new(0),
            Mutex::new(None),
            Mutex::new(None),
            AtomicUsize::new(2048),
            RwLock::new(None),
            Arc::new(Mutex::new(None)),
            AtomicU64::new(0),
        );
        let d = query_interface!(c, IDispatcher).unwrap();
        assert!(d.shutdown().is_ok());
    }

    #[test]
    fn double_shutdown_succeeds() {
        let c = DispatcherComponent::new(
            AtomicBool::new(false),
            Mutex::new(None),
            Mutex::new(None),
            Mutex::new(None),
            Mutex::new(None),
            RwLock::new(Vec::new()),
            RwLock::new(None),
            AtomicU64::new(0),
            Mutex::new(None),
            Mutex::new(None),
            AtomicUsize::new(2048),
            RwLock::new(None),
            Arc::new(Mutex::new(None)),
            AtomicU64::new(0),
        );
        let d = query_interface!(c, IDispatcher).unwrap();
        assert!(d.shutdown().is_ok());
        assert!(d.shutdown().is_ok());
    }

    #[test]
    fn concurrent_pre_init_calls_from_multiple_threads() {
        let c = Arc::new(DispatcherComponent::new(
            AtomicBool::new(false),
            Mutex::new(None),
            Mutex::new(None),
            Mutex::new(None),
            Mutex::new(None),
            RwLock::new(Vec::new()),
            RwLock::new(None),
            AtomicU64::new(0),
            Mutex::new(None),
            Mutex::new(None),
            AtomicUsize::new(2048),
            RwLock::new(None),
            Arc::new(Mutex::new(None)),
            AtomicU64::new(0),
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
        let c = DispatcherComponent::new(
            AtomicBool::new(false),
            Mutex::new(None),
            Mutex::new(None),
            Mutex::new(None),
            Mutex::new(None),
            RwLock::new(Vec::new()),
            RwLock::new(None),
            AtomicU64::new(0),
            Mutex::new(None),
            Mutex::new(None),
            AtomicUsize::new(2048),
            RwLock::new(None),
            Arc::new(Mutex::new(None)),
            AtomicU64::new(0),
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
        let c = DispatcherComponent::new(
            AtomicBool::new(false),
            Mutex::new(None),
            Mutex::new(None),
            Mutex::new(None),
            Mutex::new(None),
            RwLock::new(Vec::new()),
            RwLock::new(None),
            AtomicU64::new(0),
            Mutex::new(None),
            Mutex::new(None),
            AtomicUsize::new(2048),
            RwLock::new(None),
            Arc::new(Mutex::new(None)),
            AtomicU64::new(0),
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
        let gpu: Arc<dyn IGpuServices + Send + Sync> = Arc::new(MockGpuServices::default());
        let mt: Arc<dyn IMemoryTier + Send + Sync> =
            Arc::new(MockMemoryTier::with_fail_insert(1024 * 1024));
        let c = DispatcherComponent::new(
            AtomicBool::new(false),
            Mutex::new(None),
            Mutex::new(None),
            Mutex::new(None),
            Mutex::new(None),
            RwLock::new(Vec::new()),
            RwLock::new(None),
            AtomicU64::new(0),
            Mutex::new(None),
            Mutex::new(None),
            AtomicUsize::new(2048),
            RwLock::new(None),
            Arc::new(Mutex::new(None)),
            AtomicU64::new(0),
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

    /// Regression: a `batch_lookup` whose promotion lost the `mt.insert` race to
    /// a concurrent lookup (the other TP rank) must recover and serve the block
    /// warm rather than failing the load with AlreadyExists (`error_code=3`).
    #[test]
    fn batch_lookup_recovers_from_concurrent_promotion_race() {
        let (c, dm) = setup_initialized();
        let d = query_interface!(c, IDispatcher).unwrap();

        // Populate key 1 — dispatch-map = MemoryTier, and the memory-tier slot
        // holds 0xCD. This slot stands in for the "winner's" resident promotion.
        let mut buf = vec![0xCDu8; 4096];
        d.populate(1, make_handle(&mut buf)).unwrap();
        let mt = c.memory_tier.get().unwrap();
        let (mem_ptr, size) = mt.peek(1).expect("key 1 should be resident");

        // Reproduce the divergence the race produces:
        //  - classify key 1 as cold (BlockDevice) so batch_lookup tries to promote,
        //  - leave the memory-tier slot in place so mt.insert() -> AlreadyExists,
        //  - arm a one-shot flip so the *recovery* lookup observes MemoryTier
        //    (the concurrent winner having finished promoting).
        dm.convert_entry_to_block(1, 0x1000);
        dm.flip_to_memory_tier_on_next_lookup(1, mem_ptr, size);

        let mut out = vec![0u8; 4096];
        let results = d.batch_lookup(&[(1, vec![make_handle(&mut out)])]);
        assert_eq!(results.len(), 1);
        assert!(
            results[0].is_ok(),
            "concurrent-promotion loser should be served warm, got: {:?}",
            results[0]
        );
        assert_eq!(out[0], 0xCD, "recovered block must carry the resident data");

        d.shutdown().unwrap();
    }

    // -----------------------------------------------------------------------
    // Remote-lookup delivery: batching shape and read-pin lifetime
    // -----------------------------------------------------------------------

    /// Build `batch_lookup` input for `keys`, each with one 4 KiB destination
    /// region, plus the backing buffers so the caller can inspect the bytes.
    fn remote_batch(keys: &[CacheKey]) -> (Vec<Vec<u8>>, Vec<(CacheKey, Vec<IpcHandle>)>) {
        let mut bufs: Vec<Vec<u8>> = keys.iter().map(|_| vec![0u8; 4096]).collect();
        let entries = keys
            .iter()
            .zip(bufs.iter_mut())
            .map(|(&k, buf)| (k, vec![make_handle(buf)]))
            .collect();
        (bufs, entries)
    }

    /// The load-bearing invariant: a read pin must still be held when the copy is
    /// *submitted* and when the batched sync runs, because that pin is the only
    /// thing keeping the memory-tier evictor off the DRAM slot the DMA is reading.
    ///
    /// Also pins down the batching shape. On a per-key-synchronize implementation
    /// the event sequence is `[Submit, Sync, Submit, Sync]` with one pin held at
    /// each sync; here it must be both submissions first, then a single sync, with
    /// both pins still outstanding at that point.
    #[test]
    fn remote_delivery_holds_pin_until_after_batched_sync() {
        let fx = setup_initialized_with_remote(&[1, 2], PinProbe::new);
        let d = query_interface!(fx.component, IDispatcher).unwrap();

        let (bufs, entries) = remote_batch(&[1, 2]);
        let results = d.batch_lookup(&entries);
        assert!(
            results.iter().all(|r| r.is_ok()),
            "both remote keys should be delivered, got: {results:?}"
        );

        assert_eq!(
            fx.probe.events(),
            vec![
                (GpuEvent::SubmitH2d, 1),
                (GpuEvent::SubmitH2d, 2),
                (GpuEvent::Sync, 2),
            ],
            "each copy must be submitted with its own pin held (pins accumulate as \
             the batch is walked), then ONE sync with both pins still outstanding"
        );

        // Bytes actually arrived, and every pin is released once the call returns.
        assert_eq!(bufs[0][0], MockRemoteLookup::fill_byte(1));
        assert_eq!(bufs[1][0], MockRemoteLookup::fill_byte(2));
        assert_eq!(fx.dm.total_read_refs(), 0, "pins must not leak");

        d.shutdown().unwrap();
    }

    /// The perf-shape lock: sync cost must amortize over the batch, not scale with
    /// it. One `stream_synchronize` for eight keys, not eight.
    #[test]
    fn remote_delivery_issues_one_sync_per_batch() {
        let keys: Vec<CacheKey> = (1..=8).collect();
        let fx = setup_initialized_with_remote(&keys, PinProbe::new);
        let d = query_interface!(fx.component, IDispatcher).unwrap();

        let (_bufs, entries) = remote_batch(&keys);
        let results = d.batch_lookup(&entries);
        assert!(results.iter().all(|r| r.is_ok()), "got: {results:?}");

        assert_eq!(fx.probe.count(GpuEvent::SubmitH2d), 8);
        assert_eq!(
            fx.probe.count(GpuEvent::Sync),
            1,
            "one sync per batch, not per key"
        );
        assert_eq!(fx.dm.total_read_refs(), 0);

        d.shutdown().unwrap();
    }

    /// A key the peer does not hold must fail on its own without disturbing its
    /// neighbours, and must not leave a pin behind.
    #[test]
    fn remote_delivery_isolates_per_key_failure() {
        let fx = setup_initialized_with_remote(&[1, 3], PinProbe::new);
        let d = query_interface!(fx.component, IDispatcher).unwrap();

        let (bufs, entries) = remote_batch(&[1, 2, 3]);
        let results = d.batch_lookup(&entries);

        assert!(results[0].is_ok(), "key 1: {:?}", results[0]);
        assert!(results[1].is_err(), "key 2 is not held remotely");
        assert!(results[2].is_ok(), "key 3: {:?}", results[2]);

        assert_eq!(bufs[0][0], MockRemoteLookup::fill_byte(1));
        assert_eq!(bufs[2][0], MockRemoteLookup::fill_byte(3));
        assert_eq!(fx.probe.count(GpuEvent::Sync), 1);
        assert_eq!(fx.dm.total_read_refs(), 0);

        d.shutdown().unwrap();
    }

    /// The adopt-then-fail path: the pin is taken before submission, so a failed
    /// submission must still release it, and the batch must carry on.
    #[test]
    fn remote_delivery_releases_pin_when_submission_fails() {
        let fx = setup_initialized_with_remote(&[1, 2, 3], |dm| PinProbe::failing_submit_on(dm, 2));
        let d = query_interface!(fx.component, IDispatcher).unwrap();

        let (_bufs, entries) = remote_batch(&[1, 2, 3]);
        let results = d.batch_lookup(&entries);

        assert!(results[0].is_ok(), "key 1: {:?}", results[0]);
        assert!(results[1].is_err(), "key 2's submission was made to fail");
        assert!(results[2].is_ok(), "key 3: {:?}", results[2]);

        assert_eq!(fx.probe.count(GpuEvent::SubmitH2d), 3);
        assert_eq!(fx.probe.count(GpuEvent::Sync), 1);
        assert_eq!(
            fx.dm.total_read_refs(),
            0,
            "the failed key's pin must be released too"
        );

        d.shutdown().unwrap();
    }

    /// A failed batched sync means the destination buffers may not hold the data,
    /// so every key that was submitted has to be reported as an error rather than
    /// a hit.
    #[test]
    fn remote_delivery_reports_sync_failure_per_key() {
        let fx = setup_initialized_with_remote(&[1, 2], PinProbe::failing_sync);
        let d = query_interface!(fx.component, IDispatcher).unwrap();

        let (_bufs, entries) = remote_batch(&[1, 2]);
        let results = d.batch_lookup(&entries);

        assert!(
            results.iter().all(|r| r.is_err()),
            "a failed sync must not be reported as a hit, got: {results:?}"
        );
        assert_eq!(fx.dm.total_read_refs(), 0);

        d.shutdown().unwrap();
    }

    /// Without a warm stream the helper falls back to the synchronous `DmaBuffer`
    /// copy, which has already completed on return. Results and pin release must be
    /// unchanged. Reaching that branch needs `device_of_ptr` to disown the pointers,
    /// since it otherwise resolves any address to device 0.
    #[test]
    fn remote_delivery_falls_back_without_warm_stream() {
        let fx = setup_initialized_with_remote(&[1, 2], PinProbe::without_gpu_device);
        let d = query_interface!(fx.component, IDispatcher).unwrap();

        let (bufs, entries) = remote_batch(&[1, 2]);
        let results = d.batch_lookup(&entries);

        assert!(results.iter().all(|r| r.is_ok()), "got: {results:?}");
        assert_eq!(bufs[0][0], MockRemoteLookup::fill_byte(1));
        assert_eq!(bufs[1][0], MockRemoteLookup::fill_byte(2));
        assert_eq!(
            fx.probe.count(GpuEvent::SubmitH2d),
            0,
            "the fallback path uses the synchronous copy, not memcpy_h2d_async"
        );
        assert_eq!(
            fx.probe.count(GpuEvent::Sync),
            0,
            "nothing to synchronize without a warm stream"
        );
        assert_eq!(fx.dm.total_read_refs(), 0);

        d.shutdown().unwrap();
    }

    /// The same invariant on the local warm path: its copies are also submitted
    /// async and synchronized once, so its pins must survive the sync too.
    #[test]
    fn warm_hit_holds_pin_until_batched_sync() {
        let fx = setup_initialized_with_remote(&[], PinProbe::new);
        let d = query_interface!(fx.component, IDispatcher).unwrap();

        // Resident locally, so both keys are warm hits and never reach remote lookup.
        fx.install_warm_key(1, 0x11, 4096);
        fx.install_warm_key(2, 0x22, 4096);
        assert_eq!(fx.dm.total_read_refs(), 0);

        let (bufs, entries) = remote_batch(&[1, 2]);
        let results = d.batch_lookup(&entries);
        assert!(results.iter().all(|r| r.is_ok()), "got: {results:?}");

        assert_eq!(
            fx.probe.events(),
            vec![
                (GpuEvent::SubmitH2d, 1),
                (GpuEvent::SubmitH2d, 2),
                (GpuEvent::Sync, 2),
            ],
            "each warm copy is submitted with its own pin held, and both pins must \
             still be outstanding at the single batched sync"
        );
        assert_eq!(bufs[0][0], 0x11);
        assert_eq!(bufs[1][0], 0x22);
        assert_eq!(fx.dm.total_read_refs(), 0);

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
        // Wait for the write-through: its `convert_to_storage` is what consumes the
        // read-pin `downgrade_reference` installs during populate. Until then the
        // entry is legitimately pinned and `remove` refuses it.
        d.flush_to_ssd().unwrap();
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

        // Let the write-through release the populate-time read-pin before removing.
        d.flush_to_ssd().unwrap();
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

        let c = DispatcherComponent::new(
            AtomicBool::new(false),
            Mutex::new(None),
            Mutex::new(None),
            Mutex::new(None),
            Mutex::new(None),
            RwLock::new(Vec::new()),
            RwLock::new(None),
            AtomicU64::new(0),
            Mutex::new(None),
            Mutex::new(None),
            AtomicUsize::new(2048),
            RwLock::new(None),
            Arc::new(Mutex::new(None)),
            AtomicU64::new(0),
        );

        // Pool is now full (16384 used). Trying to add 4096 more should evict.
        c.evict_for_space(&dm, &mt, 4096, 100, 512).unwrap();

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

        let c = DispatcherComponent::new(
            AtomicBool::new(false),
            Mutex::new(None),
            Mutex::new(None),
            Mutex::new(None),
            Mutex::new(None),
            RwLock::new(Vec::new()),
            RwLock::new(None),
            AtomicU64::new(0),
            Mutex::new(None),
            Mutex::new(None),
            AtomicUsize::new(2048),
            RwLock::new(None),
            Arc::new(Mutex::new(None)),
            AtomicU64::new(0),
        );

        // Plenty of space, no eviction needed.
        c.evict_for_space(&dm, &mt, 4096, 100, 512).unwrap();

        assert!(mt.contains(0), "entry should not be evicted");
    }

    #[test]
    fn populate_triggers_eviction_on_full_pool() {
        let dm = Arc::new(MockDispatchMap::new());
        let logger: Arc<dyn ILogger + Send + Sync> = Arc::new(MockLogger);
        let gpu: Arc<dyn IGpuServices + Send + Sync> = Arc::new(MockGpuServices::default());
        // Pool can hold exactly 2 × 4 KiB entries.
        let mt: Arc<dyn IMemoryTier + Send + Sync> = Arc::new(MockMemoryTier::new(8192));
        let c = DispatcherComponent::new(
            AtomicBool::new(false),
            Mutex::new(None),
            Mutex::new(None),
            Mutex::new(None),
            Mutex::new(None),
            RwLock::new(Vec::new()),
            RwLock::new(None),
            AtomicU64::new(0),
            Mutex::new(None),
            Mutex::new(None),
            AtomicUsize::new(2048),
            RwLock::new(None),
            Arc::new(Mutex::new(None)),
            AtomicU64::new(0),
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

        // Simulate completed write-through so the two entries are cleanly
        // evictable: persisted to SSD (has an offset) and the write-back's
        // read-pin (from downgrade_reference during populate) released.
        // Eviction must only reclaim entries that are neither read- nor
        // write-pinned — freeing an in-flight slot corrupts the concurrent
        // load/write-back — so without this the pool is legitimately stuck.
        dm.convert_to_storage(1, 0x1000).unwrap();
        dm.convert_to_storage(2, 0x2000).unwrap();

        // Third populate should now trigger eviction of one entry and succeed.
        let mut buf3 = vec![0u8; 4096];
        d.populate(3, make_handle(&mut buf3)).unwrap();

        // Total entries in dispatch-map: at most 3 (one demoted to block, not removed).
        assert!(dm.entry_count() <= 3);

        d.shutdown().unwrap();
    }

    /// Regression: eviction under memory pressure must never free the DRAM slot
    /// of a pinned entry. The old blind-eviction fallback freed the victim before
    /// checking the pin, leaving the dispatch-map pointing at reclaimed DRAM and
    /// corrupting a concurrent load (observed as cudaMemcpyAsync H2D
    /// "invalid argument" / key-not-found and an engine crash under TP).
    #[test]
    fn eviction_never_frees_pinned_slot() {
        let dm = Arc::new(MockDispatchMap::new());
        let logger: Arc<dyn ILogger + Send + Sync> = Arc::new(MockLogger);
        let gpu: Arc<dyn IGpuServices + Send + Sync> = Arc::new(MockGpuServices::default());
        // Pool holds exactly one 4 KiB entry, so the next insert must evict.
        let mt: Arc<dyn IMemoryTier + Send + Sync> = Arc::new(MockMemoryTier::new(4096));
        let mt_probe = Arc::clone(&mt);
        let c = DispatcherComponent::new(
            AtomicBool::new(false),
            Mutex::new(None),
            Mutex::new(None),
            Mutex::new(None),
            Mutex::new(None),
            RwLock::new(Vec::new()),
            RwLock::new(None),
            AtomicU64::new(0),
            Mutex::new(None),
            Mutex::new(None),
            AtomicUsize::new(2048),
            RwLock::new(None),
            Arc::new(Mutex::new(None)),
            AtomicU64::new(0),
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

        // Resident, fully persisted key 1 — normally the perfect eviction
        // victim (write-back complete: persisted and its read-pin released).
        let mut buf = vec![0u8; 4096];
        d.populate(1, make_handle(&mut buf)).unwrap();
        // Let the write-through complete: it persists the entry and releases the
        // populate-time read-pin. Doing this by hand instead would race the
        // background writer doing the same thing, leaving the entry unpinned.
        d.flush_to_ssd().unwrap();
        // ...but pin it, as an in-flight load would (prepare_load -> Pin).
        dm.take_read(1).unwrap();

        // Populating key 2 needs the only slot, held by pinned key 1. Eviction
        // must refuse and surface pool-full rather than free the pinned slot.
        let mut buf2 = vec![0u8; 4096];
        let res = d.populate(2, make_handle(&mut buf2));
        assert!(
            matches!(res, Err(DispatcherError::AllocationFailed(_))),
            "populate should fail (pool full of pinned data), got: {res:?}"
        );

        // The critical invariant: key 1's slot was NOT reclaimed.
        assert!(
            mt_probe.peek(1).is_some(),
            "pinned entry's DRAM slot must survive eviction pressure"
        );
        assert!(matches!(dm.lookup(1), Ok(LookupResult::MemoryTier { .. })));

        d.shutdown().unwrap();
    }

    // Note: StagingPool allocates SPDK-registered DMA buffers, which require an
    // initialized SPDK environment, so it isn't unit-testable in the no-hardware
    // CI env — it's exercised by the kv-offload hardware bench. Its checkout/RAII
    // logic is a plain free-list + condvar (see pipeline::StagingPool).

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

        // One read reference, simulating a concurrent reader. get_evictable_offset
        // is self-balancing: its own `lookup` pins and its `release_read` unpins.
        dm_iface.take_read(1).unwrap();

        // get_evictable_offset sees BlockDevice and returns Some(offset),
        // releasing the pin its own lookup took.
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
            None,
        );

        std::thread::sleep(std::time::Duration::from_millis(200));
        evictor.shutdown();
    }

    // -----------------------------------------------------------------------
    // promote_to_memory_tier tests
    // -----------------------------------------------------------------------

    #[test]
    fn promote_block_device_entry_to_memory_tier() {
        let (c, dm) = setup_initialized();
        let d = query_interface!(c, IDispatcher).unwrap();

        let mut buf = vec![0u8; 4096];
        d.populate(1, make_handle(&mut buf)).unwrap();

        // Simulate eviction: entry becomes BlockDevice.
        let mt = c.memory_tier.get().unwrap();
        let _ = mt.remove(1);
        dm.convert_entry_to_block(1, 0x1000);

        // Verify it's in BlockDevice state.
        let result = dm.lookup(1).unwrap();
        assert!(matches!(result, LookupResult::BlockDevice { .. }));
        let _ = dm.release_read(1);

        // Promote without hardware — enters the no-drives path.
        d.promote_to_memory_tier(&[1]);

        // After promote, entry should be in MemoryTier state.
        let result = dm.lookup(1).unwrap();
        assert!(
            matches!(result, LookupResult::MemoryTier { .. }),
            "expected MemoryTier after promote, got: {result:?}"
        );
        let _ = dm.release_read(1);
        d.shutdown().unwrap();
    }

    #[test]
    fn promote_already_in_memory_tier_is_noop() {
        let (c, _dm) = setup_initialized();
        let d = query_interface!(c, IDispatcher).unwrap();

        let mut buf = vec![0u8; 4096];
        d.populate(1, make_handle(&mut buf)).unwrap();

        // Entry is already in MemoryTier (via populate → staging → memory-tier).
        // promote_to_memory_tier should just refresh timestamp without error.
        d.promote_to_memory_tier(&[1]);
        d.shutdown().unwrap();
    }

    #[test]
    fn promote_nonexistent_key_is_silent() {
        let (c, _dm) = setup_initialized();
        let d = query_interface!(c, IDispatcher).unwrap();

        // Should not panic on missing keys.
        d.promote_to_memory_tier(&[999, 1000, 1001]);
        d.shutdown().unwrap();
    }

    #[test]
    fn promote_mixed_batch() {
        let (c, dm) = setup_initialized();
        let d = query_interface!(c, IDispatcher).unwrap();

        // Key 1: populate then evict to BlockDevice
        let mut buf1 = vec![0u8; 4096];
        d.populate(1, make_handle(&mut buf1)).unwrap();
        let mt = c.memory_tier.get().unwrap();
        let _ = mt.remove(1);
        dm.convert_entry_to_block(1, 0x1000);

        // Key 2: stays in MemoryTier
        let mut buf2 = vec![0u8; 4096];
        d.populate(2, make_handle(&mut buf2)).unwrap();

        // Key 3: does not exist
        // Promote all three — should not panic.
        d.promote_to_memory_tier(&[1, 2, 3]);

        // Key 1 should now be in MemoryTier.
        let result = dm.lookup(1).unwrap();
        assert!(
            matches!(result, LookupResult::MemoryTier { .. }),
            "key 1 should be MemoryTier after promote, got: {result:?}"
        );
        let _ = dm.release_read(1);

        // Key 2 should still be in MemoryTier (untouched).
        let result = dm.lookup(2).unwrap();
        assert!(matches!(result, LookupResult::MemoryTier { .. }));
        let _ = dm.release_read(2);

        d.shutdown().unwrap();
    }
}
