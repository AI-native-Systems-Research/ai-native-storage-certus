//! Tests for the early write-through: a block's SSD write-through starts once
//! its own copy has synchronized in `copy_gpu_to_memory_async` (the null-stream
//! path the shm-queue `CopyToStore` handler uses), not at the commit.
//!
//! Until `copy_gpu_to_memory_completed` the job's extent stays unpublished and
//! the dispatch map records nothing. The commit publishes a write that has
//! landed, or hands a queued or running job the write-through pin. An abort
//! (`release_memory`) cancels a queued job, waits for a running one, and aborts
//! the extent, then frees the slot exactly once.
//!
//! The dispatcher gets one mock data drive whose writes can be held, so a job
//! can be kept running (or the ones behind it queued) while the test acts.

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Condvar, Mutex};
use std::time::{Duration, Instant};

use component_core::channel::SpscChannel;
use component_core::query_interface;
use interfaces::{
    CacheKey, ClientChannels, Command, Completion, DispatchMapError, DispatcherConfig, DmaBuffer,
    Extent, ExtentManagerError, FormatParams, GpuDeviceInfo, GpuDmaBuffer, GpuIpcHandle, GpuStream,
    IBlockDevice, IDispatchMap, IDispatcher, IExtentManager, IGpuServices, IMemoryTier, IpcHandle,
    LookupResult, MemoryTierError, MemoryTierTelemetrySnapshot, NvmeBlockError, OpHandle,
    TelemetrySnapshot, WriteHandle,
};

use crate::{ChannelPool, DataDrive, DispatcherComponent};

const BLOCK: u32 = 4096;
/// Bytes of disk per key: key `k` is written at byte offset `k * EXTENT`.
const EXTENT: u64 = 16 * BLOCK as u64;
const WAIT: Duration = Duration::from_secs(10);

// --- dispatch map: entries with reference counts, as the real one enforces them ---

// --- dispatch map: entries with reference counts, as the real one enforces them ---

#[derive(Clone, Copy, Debug, PartialEq)]
enum Loc {
    Block(u64),
    // Pointer stored as usize so the mock stays Send + Sync without an unsafe impl.
    Memory {
        ptr: usize,
        size: u32,
        ssd: Option<u64>,
    },
}

struct Ent {
    loc: Loc,
    read_refs: u32,
    write_ref: bool,
}

#[derive(Default)]
struct MapDm {
    entries: Mutex<HashMap<CacheKey, Ent>>,
}

impl MapDm {
    fn loc(&self, key: CacheKey) -> Option<Loc> {
        self.entries.lock().unwrap().get(&key).map(|e| e.loc)
    }

    fn read_refs(&self, key: CacheKey) -> Option<u32> {
        self.entries.lock().unwrap().get(&key).map(|e| e.read_refs)
    }

    fn ssd_offset(&self, key: CacheKey) -> Option<u64> {
        match self.loc(key) {
            Some(Loc::Memory { ssd, .. }) => ssd,
            Some(Loc::Block(o)) => Some(o),
            None => None,
        }
    }
}

impl IDispatchMap for MapDm {
    fn initialize(&self) -> Result<(), DispatchMapError> {
        Ok(())
    }
    fn lookup(&self, key: CacheKey) -> Result<LookupResult, DispatchMapError> {
        let mut entries = self.entries.lock().unwrap();
        let Some(e) = entries.get_mut(&key) else {
            return Ok(LookupResult::NotExist);
        };
        if e.write_ref {
            return Err(DispatchMapError::Timeout(key));
        }
        e.read_refs += 1;
        Ok(match e.loc {
            Loc::Block(offset) => LookupResult::BlockDevice { offset },
            Loc::Memory { ptr, size, .. } => LookupResult::MemoryTier {
                pointer: ptr as *mut u8,
                size,
            },
        })
    }
    /// As `dispatch-map`'s `convert_to_storage`: records the SSD copy and drops
    /// one read reference (the write-through pin).
    fn convert_to_storage(&self, key: CacheKey, offset: u64) -> Result<(), DispatchMapError> {
        let mut entries = self.entries.lock().unwrap();
        let e = entries
            .get_mut(&key)
            .ok_or(DispatchMapError::KeyNotFound(key))?;
        match &mut e.loc {
            Loc::Memory { ssd, .. } => *ssd = Some(offset),
            Loc::Block(_) => return Err(DispatchMapError::InvalidState("block".into())),
        }
        e.read_refs = e.read_refs.saturating_sub(1);
        Ok(())
    }
    fn take_read(&self, key: CacheKey) -> Result<(), DispatchMapError> {
        let mut entries = self.entries.lock().unwrap();
        let e = entries
            .get_mut(&key)
            .ok_or(DispatchMapError::KeyNotFound(key))?;
        e.read_refs += 1;
        Ok(())
    }
    fn take_write(&self, key: CacheKey) -> Result<(), DispatchMapError> {
        let mut entries = self.entries.lock().unwrap();
        let e = entries
            .get_mut(&key)
            .ok_or(DispatchMapError::KeyNotFound(key))?;
        e.write_ref = true;
        Ok(())
    }
    fn release_read(&self, key: CacheKey) -> Result<(), DispatchMapError> {
        let mut entries = self.entries.lock().unwrap();
        let e = entries
            .get_mut(&key)
            .ok_or(DispatchMapError::KeyNotFound(key))?;
        if e.read_refs == 0 {
            return Err(DispatchMapError::RefCountUnderflow(key));
        }
        e.read_refs -= 1;
        Ok(())
    }
    fn release_write(&self, key: CacheKey) -> Result<(), DispatchMapError> {
        let mut entries = self.entries.lock().unwrap();
        let e = entries
            .get_mut(&key)
            .ok_or(DispatchMapError::KeyNotFound(key))?;
        if !e.write_ref {
            return Err(DispatchMapError::RefCountUnderflow(key));
        }
        e.write_ref = false;
        Ok(())
    }
    fn downgrade_reference(&self, key: CacheKey) -> Result<(), DispatchMapError> {
        let mut entries = self.entries.lock().unwrap();
        let e = entries
            .get_mut(&key)
            .ok_or(DispatchMapError::NoWriteReference(key))?;
        if !e.write_ref {
            return Err(DispatchMapError::NoWriteReference(key));
        }
        e.write_ref = false;
        e.read_refs += 1;
        Ok(())
    }
    fn remove(&self, key: CacheKey) -> Result<(), DispatchMapError> {
        let mut entries = self.entries.lock().unwrap();
        let e = entries
            .get(&key)
            .ok_or(DispatchMapError::KeyNotFound(key))?;
        if e.read_refs > 0 || e.write_ref {
            return Err(DispatchMapError::ActiveReferences(key));
        }
        entries.remove(&key);
        Ok(())
    }
    fn touch(&self, key: CacheKey) -> Result<(), DispatchMapError> {
        if self.entries.lock().unwrap().contains_key(&key) {
            Ok(())
        } else {
            Err(DispatchMapError::KeyNotFound(key))
        }
    }
    fn entry_size(&self, key: CacheKey) -> Result<u32, DispatchMapError> {
        self.touch(key).map(|_| BLOCK)
    }
    fn oldest_keys(&self, n: usize) -> Vec<CacheKey> {
        let mut keys: Vec<CacheKey> = self.entries.lock().unwrap().keys().copied().collect();
        keys.sort_unstable();
        keys.truncate(n);
        keys
    }
    fn create_memory_tier_entry(
        &self,
        key: CacheKey,
        pointer: *mut u8,
        size: u32,
    ) -> Result<(), DispatchMapError> {
        let mut entries = self.entries.lock().unwrap();
        if entries.contains_key(&key) {
            return Err(DispatchMapError::AlreadyExists(key));
        }
        entries.insert(
            key,
            Ent {
                loc: Loc::Memory {
                    ptr: pointer as usize,
                    size,
                    ssd: None,
                },
                read_refs: 0,
                write_ref: true,
            },
        );
        Ok(())
    }
    fn convert_memory_tier_to_block(&self, _key: CacheKey) -> Result<(), DispatchMapError> {
        Err(DispatchMapError::InvalidState("not modelled".into()))
    }
    fn promote_block_to_memory_tier(
        &self,
        _key: CacheKey,
        _pointer: *mut u8,
        _size: u32,
    ) -> Result<(), DispatchMapError> {
        Err(DispatchMapError::InvalidState("not modelled".into()))
    }
    fn is_evictable(&self, key: CacheKey) -> bool {
        let entries = self.entries.lock().unwrap();
        entries.get(&key).is_some_and(|e| {
            e.read_refs == 0 && !e.write_ref && matches!(e.loc, Loc::Memory { ssd: Some(_), .. })
        })
    }
    fn try_evict_to_block(&self, key: CacheKey) -> Result<(), DispatchMapError> {
        let mut entries = self.entries.lock().unwrap();
        let e = entries
            .get_mut(&key)
            .ok_or(DispatchMapError::KeyNotFound(key))?;
        if e.read_refs != 0 || e.write_ref {
            return Err(DispatchMapError::ActiveReferences(key));
        }
        match e.loc {
            Loc::Memory {
                ssd: Some(offset), ..
            } => {
                e.loc = Loc::Block(offset);
                Ok(())
            }
            _ => Err(DispatchMapError::InvalidState("not evictable".into())),
        }
    }
    fn recover_extent(
        &self,
        _key: CacheKey,
        _offset: u64,
        _size_blocks: u32,
    ) -> Result<(), DispatchMapError> {
        Err(DispatchMapError::InvalidState("not modelled".into()))
    }
    #[cfg(feature = "integrity-check")]
    fn set_checksum(&self, _key: CacheKey, _checksum: u32) -> Result<(), DispatchMapError> {
        Ok(())
    }
    #[cfg(feature = "integrity-check")]
    fn get_checksum(&self, _key: CacheKey) -> Option<u32> {
        None
    }
}

// --- memory tier: a flat, DMA-capable pool of 4 KiB slots ---

/// (pool bytes, key -> (offset, size), next free offset)
type PoolState = (Vec<u8>, HashMap<CacheKey, (usize, u32)>, usize);

struct PoolMt {
    inner: Mutex<PoolState>,
    /// Slots freed, per key.
    freed: Mutex<HashMap<CacheKey, u32>>,
}

impl PoolMt {
    fn new(capacity: usize) -> Self {
        Self {
            inner: Mutex::new((vec![0u8; capacity], HashMap::new(), 0)),
            freed: Mutex::new(HashMap::new()),
        }
    }

    fn freed(&self, key: CacheKey) -> u32 {
        self.freed.lock().unwrap().get(&key).copied().unwrap_or(0)
    }
}

impl IMemoryTier for PoolMt {
    fn initialize(&self, _pool_size: usize, _numa: Option<i32>) -> Result<(), MemoryTierError> {
        Ok(())
    }
    fn insert(&self, key: CacheKey, size: u32) -> Result<*mut u8, MemoryTierError> {
        let mut g = self.inner.lock().unwrap();
        let (pool, slots, used) = &mut *g;
        if slots.contains_key(&key) {
            return Err(MemoryTierError::AlreadyExists(key));
        }
        let aligned = (size as usize).next_multiple_of(BLOCK as usize);
        if *used + aligned > pool.len() {
            return Err(MemoryTierError::PoolFull);
        }
        let off = *used;
        *used += aligned;
        slots.insert(key, (off, size));
        // SAFETY: `off + aligned <= pool.len()` was checked above.
        Ok(unsafe { pool.as_mut_ptr().add(off) })
    }
    fn get(&self, key: CacheKey) -> Option<(*mut u8, u32)> {
        let g = self.inner.lock().unwrap();
        // SAFETY: every recorded slot offset lies inside the pool.
        g.1.get(&key)
            .map(|&(off, size)| (unsafe { (g.0.as_ptr() as *mut u8).add(off) }, size))
    }
    fn peek(&self, key: CacheKey) -> Option<(*mut u8, u32)> {
        self.get(key)
    }
    fn evict_next(&self) -> Option<CacheKey> {
        None
    }
    fn evict_next_for_key(&self, _key: CacheKey) -> Option<CacheKey> {
        None
    }
    fn oldest_keys(&self, n: usize) -> Vec<CacheKey> {
        let mut keys: Vec<CacheKey> = self.inner.lock().unwrap().1.keys().copied().collect();
        keys.sort_unstable();
        keys.truncate(n);
        keys
    }
    fn remove(&self, key: CacheKey) -> Result<(), MemoryTierError> {
        self.inner
            .lock()
            .unwrap()
            .1
            .remove(&key)
            .ok_or(MemoryTierError::KeyNotFound(key))?;
        *self.freed.lock().unwrap().entry(key).or_default() += 1;
        Ok(())
    }
    fn touch(&self, _key: CacheKey) {}
    fn batch_touch(&self, _keys: &[CacheKey]) {}
    fn contains(&self, key: CacheKey) -> bool {
        self.inner.lock().unwrap().1.contains_key(&key)
    }
    fn capacity(&self) -> usize {
        self.inner.lock().unwrap().0.len()
    }
    fn used(&self) -> usize {
        self.inner.lock().unwrap().2
    }
    fn pool_info(&self) -> Option<(*mut u8, usize)> {
        None
    }
    /// DMA-capable, so the write-through wraps the slot instead of allocating
    /// an SPDK staging buffer (there is no SPDK environment in a unit test).
    fn is_dma_capable(&self) -> bool {
        true
    }
    fn clear(&self) -> Result<usize, MemoryTierError> {
        let mut g = self.inner.lock().unwrap();
        let n = g.1.len();
        g.1.clear();
        g.2 = 0;
        Ok(n)
    }
    fn telemetry_snapshot(&self) -> MemoryTierTelemetrySnapshot {
        MemoryTierTelemetrySnapshot::default()
    }
    // A test double with no bounce reserve.
    fn set_bounce_reserve(&self, _: usize) -> Result<(), interfaces::MemoryTierError> {
        Ok(())
    }
    fn bounce_alloc(&self, _: u32) -> Result<interfaces::BounceBuffer, interfaces::BounceError> {
        Err(interfaces::BounceError::NeverFits(
            "no bounce reserve".into(),
        ))
    }
    fn bounce_free(&self, _: interfaces::BounceBuffer) {
        unreachable!("no buffer was ever handed out")
    }
    fn bounce_usage(&self) -> (usize, usize) {
        (0, 0)
    }
}
// --- NVMe block device: keeps written blocks; writes can be held ---

#[derive(Default)]
struct Disk {
    /// lba -> first byte of the block written there
    blocks: HashMap<u64, u8>,
    /// `WriteSync` commands received (a job reached the drive).
    writes: u32,
    /// While set, a received write waits: its job stays running.
    hold: bool,
}

#[derive(Default)]
struct DiskState {
    disk: Mutex<Disk>,
    cv: Condvar,
}

#[derive(Default)]
struct MockNvme {
    state: Arc<DiskState>,
}

impl IBlockDevice for MockNvme {
    fn connect_client(&self) -> Result<ClientChannels, NvmeBlockError> {
        let cmd = SpscChannel::<Command>::new(64);
        let comp = SpscChannel::<Completion>::new(64);
        let (cmd_tx, cmd_rx) = (cmd.sender().unwrap(), cmd.receiver().unwrap());
        let (comp_tx, comp_rx) = (comp.sender().unwrap(), comp.receiver().unwrap());
        let state = Arc::clone(&self.state);
        std::thread::spawn(move || {
            while let Ok(c) = cmd_rx.recv() {
                let done = match c {
                    Command::WriteSync { lba, buf, .. } => {
                        let mut d = state.disk.lock().unwrap();
                        d.writes += 1;
                        state.cv.notify_all();
                        while d.hold {
                            d = state.cv.wait(d).unwrap();
                        }
                        d.blocks.insert(lba, buf.as_slice()[0]);
                        Completion::WriteDone {
                            handle: OpHandle(1),
                            tag: 0,
                            result: Ok(()),
                        }
                    }
                    _ => Completion::Error {
                        handle: None,
                        error: NvmeBlockError::NotSupported("mock".into()),
                    },
                };
                if comp_tx.send(done).is_err() {
                    break;
                }
            }
        });
        Ok(ClientChannels {
            command_tx: cmd_tx,
            completion_rx: comp_rx,
        })
    }
    fn sector_size(&self, _ns_id: u32) -> Result<u32, NvmeBlockError> {
        Ok(BLOCK)
    }
    fn num_sectors(&self, _ns_id: u32) -> Result<u64, NvmeBlockError> {
        Ok(1 << 20)
    }
    fn max_queue_depth(&self) -> u32 {
        64
    }
    fn num_io_queues(&self) -> u32 {
        1
    }
    fn max_transfer_size(&self) -> u32 {
        128 * 1024
    }
    fn block_size(&self) -> u32 {
        BLOCK
    }
    fn numa_node(&self) -> i32 {
        -1
    }
    fn nvme_version(&self) -> String {
        "mock".into()
    }
    fn telemetry(&self) -> Result<TelemetrySnapshot, NvmeBlockError> {
        Err(NvmeBlockError::NotSupported("mock".into()))
    }
    fn read_write_stats(&self) -> interfaces::ReadWriteStats {
        interfaces::ReadWriteStats::default()
    }
}

// --- extent manager: key k gets the extent at k * EXTENT; records publish and abort ---

#[derive(Default)]
struct MockEm {
    published: Arc<Mutex<HashMap<u64, CacheKey>>>,
    aborted: Arc<Mutex<Vec<u64>>>,
    base_lba: Mutex<u64>,
}

impl IExtentManager for MockEm {
    fn format(&self, _params: FormatParams) -> Result<(), ExtentManagerError> {
        Ok(())
    }
    fn initialize(&self) -> Result<(), ExtentManagerError> {
        Ok(())
    }
    fn reserve_extent(&self, key: CacheKey, size: u32) -> Result<WriteHandle, ExtentManagerError> {
        let offset = key * EXTENT;
        let published = Arc::clone(&self.published);
        let aborted = Arc::clone(&self.aborted);
        Ok(WriteHandle::new(
            key,
            offset,
            size,
            Box::new(move || {
                published.lock().unwrap().insert(offset, key);
                Ok(Extent {
                    key,
                    size: size / BLOCK,
                    offset,
                })
            }),
            Box::new(move || aborted.lock().unwrap().push(offset)),
        ))
    }
    fn get_extents(&self) -> Vec<Extent> {
        Vec::new()
    }
    fn for_each_extent(&self, _cb: &mut dyn FnMut(&Extent)) {}
    fn remove_extent(&self, offset: u64) -> Result<(), ExtentManagerError> {
        self.published.lock().unwrap().remove(&offset);
        Ok(())
    }
    fn checkpoint(&self) -> Result<(), ExtentManagerError> {
        Ok(())
    }
    fn get_instance_id(&self) -> Result<u64, ExtentManagerError> {
        Ok(1)
    }
    fn set_checkpoint_interval(&self, _interval: Option<Duration>) {}
    fn used_bytes(&self) -> u64 {
        0
    }
    fn capacity_bytes(&self) -> u64 {
        1 << 40
    }
    fn set_metadata_base_lba(&self, _base_lba: u64) {}
    fn set_data_base_lba(&self, base_lba: u64) {
        *self.base_lba.lock().unwrap() = base_lba;
    }
    fn data_base_lba(&self) -> u64 {
        *self.base_lba.lock().unwrap()
    }
}

// --- GPU: host memcpy, no device association (the copy takes the null-stream path) ---

struct HostGpu;

fn host_copy(src: *const u8, dst: *mut u8, size: usize) -> Result<(), String> {
    // SAFETY: in this test every src/dst is a live host buffer of at least `size` bytes.
    unsafe { std::ptr::copy_nonoverlapping(src, dst, size) };
    Ok(())
}

impl IGpuServices for HostGpu {
    fn initialize(&self) -> Result<(), String> {
        Ok(())
    }
    fn shutdown(&self) -> Result<(), String> {
        Ok(())
    }
    fn get_devices(&self) -> Result<Vec<GpuDeviceInfo>, String> {
        Ok(vec![])
    }
    fn deserialize_ipc_handle(&self, _payload: &str) -> Result<GpuIpcHandle, String> {
        Err("mock".into())
    }
    fn verify_memory(&self, _h: &GpuIpcHandle) -> Result<(), String> {
        Ok(())
    }
    fn pin_memory(&self, _h: &GpuIpcHandle) -> Result<(), String> {
        Ok(())
    }
    fn unpin_memory(&self, _h: &GpuIpcHandle) -> Result<(), String> {
        Ok(())
    }
    fn create_dma_buffer(&self, _h: GpuIpcHandle) -> Result<GpuDmaBuffer, String> {
        Err("mock".into())
    }
    fn dma_copy_to_host(
        &self,
        src: *const std::ffi::c_void,
        dst: &DmaBuffer,
        size: usize,
    ) -> Result<(), String> {
        host_copy(src as *const u8, dst.as_ptr() as *mut u8, size)
    }
    fn dma_copy_to_device(
        &self,
        src: &DmaBuffer,
        dst: *mut std::ffi::c_void,
        size: usize,
    ) -> Result<(), String> {
        host_copy(src.as_ptr() as *const u8, dst as *mut u8, size)
    }
    fn prepare_memory_for_spdk(
        &self,
        _payload: &str,
        _device_index: Option<u32>,
    ) -> Result<DmaBuffer, String> {
        Err("mock".into())
    }
    fn create_stream(&self) -> Result<GpuStream, String> {
        Err("mock".into())
    }
    fn set_device(&self, _device: i32) -> Result<(), String> {
        Ok(())
    }
    fn device_of_ptr(&self, _ptr: *const std::ffi::c_void) -> Result<i32, String> {
        Err("host pointer".into())
    }
    fn stream_query(&self, _s: GpuStream) -> Result<bool, String> {
        Ok(true)
    }
    fn destroy_stream(&self, _s: GpuStream) -> Result<(), String> {
        Ok(())
    }
    fn stream_synchronize(&self, _s: GpuStream) -> Result<(), String> {
        Ok(())
    }
    fn dma_copy_to_device_async(
        &self,
        src: &DmaBuffer,
        dst: *mut std::ffi::c_void,
        size: usize,
        _s: GpuStream,
    ) -> Result<(), String> {
        host_copy(src.as_ptr() as *const u8, dst as *mut u8, size)
    }
    fn memcpy_h2d_async(
        &self,
        src: *const std::ffi::c_void,
        dst: *mut std::ffi::c_void,
        size: usize,
        _s: GpuStream,
    ) -> Result<(), String> {
        host_copy(src as *const u8, dst as *mut u8, size)
    }
    fn dma_copy_to_host_async(
        &self,
        src: *const std::ffi::c_void,
        dst: &DmaBuffer,
        size: usize,
        _s: GpuStream,
    ) -> Result<(), String> {
        host_copy(src as *const u8, dst.as_ptr() as *mut u8, size)
    }
    fn memcpy_d2h_async(
        &self,
        src: *const std::ffi::c_void,
        dst: *mut std::ffi::c_void,
        size: usize,
        _s: GpuStream,
    ) -> Result<(), String> {
        host_copy(src as *const u8, dst as *mut u8, size)
    }
    fn memcpy_batch_async(
        &self,
        dsts: &[*mut std::ffi::c_void],
        srcs: &[*const std::ffi::c_void],
        sizes: &[usize],
        stream: GpuStream,
    ) -> Result<(), String> {
        for ((&dst, &src), &size) in dsts.iter().zip(srcs).zip(sizes) {
            self.memcpy_d2h_async(src, dst, size, stream)?;
        }
        Ok(())
    }
    fn allocate_pinned_dma_buffer(&self, size: usize) -> Result<DmaBuffer, String> {
        DmaBuffer::new(size, 4096, None).map_err(|e| e.to_string())
    }
    fn register_host_memory(&self, _p: *mut std::ffi::c_void, _s: usize) -> Result<(), String> {
        Ok(())
    }
    fn unregister_host_memory(&self, _p: *mut std::ffi::c_void, _s: usize) -> Result<(), String> {
        Ok(())
    }
}

// --- fixture ---

struct Fixture {
    d: Arc<dyn IDispatcher + Send + Sync>,
    dm: Arc<MapDm>,
    mt: Arc<PoolMt>,
    disk: Arc<DiskState>,
    published: Arc<Mutex<HashMap<u64, CacheKey>>>,
    aborted: Arc<Mutex<Vec<u64>>>,
    _c: Arc<DispatcherComponent>,
}

fn setup() -> Fixture {
    let dm = Arc::new(MapDm::default());
    let mt = Arc::new(PoolMt::new(1024 * 1024));
    let nvme = Arc::new(MockNvme::default());
    let disk = Arc::clone(&nvme.state);
    let em = MockEm::default();
    let (published, aborted) = (Arc::clone(&em.published), Arc::clone(&em.aborted));
    let ibd: Arc<dyn IBlockDevice + Send + Sync> = nvme;
    let iem: Arc<dyn IExtentManager + Send + Sync> = Arc::new(em);

    let c = DispatcherComponent::new_default();
    c.dispatch_map
        .connect(Arc::clone(&dm) as Arc<dyn IDispatchMap + Send + Sync>)
        .unwrap();
    c.memory_tier
        .connect(Arc::clone(&mt) as Arc<dyn IMemoryTier + Send + Sync>)
        .unwrap();
    c.gpu_services.connect(Arc::new(HostGpu)).unwrap();
    // A placeholder IUnknown for the drive's component handles; never queried.
    let placeholder: Arc<dyn component_core::IUnknown + Send + Sync> =
        spdk_env::SPDKEnvComponent::new_default();
    // Memory-tier-only initialize (no SPDK environment, no factory) leaves
    // `data_drives` as it finds it, so the background writer gets this drive.
    c.data_drives.write().push(DataDrive {
        _block_dev: Arc::clone(&placeholder),
        block_dev_admin: None,
        block_dev_iface: Arc::clone(&ibd),
        _extent_mgr_component: placeholder,
        extent_mgr: iem,
        channel_pool: Arc::new(ChannelPool::new(ibd)),
    });
    let d = query_interface!(c, IDispatcher).unwrap();
    d.initialize(DispatcherConfig {
        data_pci_addrs: vec!["0000:02:00.0".to_string()],
        ssd_eviction_threshold: 0.0,
        store_backpressure_ms: 0,
        ..Default::default()
    })
    .unwrap();
    Fixture {
        d,
        dm,
        mt,
        disk,
        published,
        aborted,
        _c: c,
    }
}

impl Fixture {
    /// Reserve `key`'s slot and copy a block of `fill` into it the way the
    /// shm-queue `CopyToStore` handler does (null stream). Queues the early job.
    fn copy(&self, key: CacheKey, fill: u8) {
        if !self.mt.contains(key) {
            self.d.reserve_memory(key, BLOCK, 0, None).unwrap();
        }
        let mut src = vec![fill; BLOCK as usize];
        let region = IpcHandle {
            address: src.as_mut_ptr(),
            size: BLOCK,
        };
        self.d
            .copy_gpu_to_memory_async(key, &[region], GpuStream(std::ptr::null_mut()))
            .unwrap();
    }

    fn commit(&self, key: CacheKey) {
        self.d.copy_gpu_to_memory_completed(key, BLOCK).unwrap();
    }

    fn hold(&self, hold: bool) {
        self.disk.disk.lock().unwrap().hold = hold;
        self.disk.cv.notify_all();
    }

    fn writes(&self) -> u32 {
        self.disk.disk.lock().unwrap().writes
    }

    /// Wait until `n` writes have reached the drive.
    fn wait_writes(&self, n: u32) {
        let deadline = Instant::now() + WAIT;
        let mut d = self.disk.disk.lock().unwrap();
        while d.writes < n {
            assert!(
                Instant::now() < deadline,
                "write {n} did not reach the drive"
            );
            d = self.disk.cv.wait_timeout(d, WAIT).unwrap().0;
        }
    }

    fn on_disk(&self, key: CacheKey) -> Option<u8> {
        let lba = key * EXTENT / u64::from(BLOCK);
        self.disk.disk.lock().unwrap().blocks.get(&lba).copied()
    }

    fn published(&self, key: CacheKey) -> bool {
        self.published.lock().unwrap().contains_key(&(key * EXTENT))
    }

    fn aborted(&self, key: CacheKey) -> usize {
        let aborted = self.aborted.lock().unwrap();
        aborted.iter().filter(|&&o| o == key * EXTENT).count()
    }

    /// Wait for every queued write job (`flush_to_ssd` drains the writers).
    fn drain(&self) {
        self.d.flush_to_ssd().unwrap();
    }
}

/// Run `f` on a thread; returns its done flag and handle.
fn spawn(f: impl FnOnce() + Send + 'static) -> (Arc<AtomicBool>, std::thread::JoinHandle<()>) {
    let done = Arc::new(AtomicBool::new(false));
    let flag = Arc::clone(&done);
    let h = std::thread::spawn(move || {
        f();
        flag.store(true, Ordering::Release);
    });
    (done, h)
}

#[test]
fn early_write_lands_before_commit_and_commit_converts_without_a_new_job() {
    let f = setup();
    f.copy(1, 0xA1);
    f.drain();

    // Written, but invisible: no extent published, no dispatch-map entry.
    assert_eq!(f.on_disk(1), Some(0xA1));
    assert!(!f.published(1));
    assert_eq!(f.dm.loc(1), None);

    f.commit(1);
    // The commit itself publishes and records the SSD copy; the pin is released.
    assert!(f.published(1));
    assert_eq!(f.dm.ssd_offset(1), Some(EXTENT));
    assert_eq!(f.dm.read_refs(1), Some(0));
    f.drain();
    assert_eq!(f.writes(), 1, "the commit must not queue a second write");
    assert_eq!(f.aborted(1), 0);
    f.d.shutdown().unwrap();
}

#[test]
fn commit_while_the_early_job_runs_settles_the_key_once() {
    let f = setup();
    f.hold(true);
    f.copy(1, 0xB1);
    f.wait_writes(1);

    f.commit(1);
    assert_eq!(f.dm.ssd_offset(1), None, "not landed yet");
    assert!(!f.published(1));
    assert_eq!(f.dm.read_refs(1), Some(1), "the job holds the pin");

    f.hold(false);
    f.drain();
    assert!(f.published(1));
    assert_eq!(f.dm.ssd_offset(1), Some(EXTENT));
    assert_eq!(f.dm.read_refs(1), Some(0));
    assert_eq!(f.writes(), 1);
    f.d.shutdown().unwrap();
}

#[test]
fn commit_while_the_early_job_is_queued_hands_it_the_pin() {
    let f = setup();
    f.hold(true);
    f.copy(1, 0x11);
    f.wait_writes(1);
    // One drive: key 2's early job queues behind key 1's.
    f.copy(2, 0x22);
    f.commit(2);
    assert_eq!(f.dm.read_refs(2), Some(1));

    f.hold(false);
    f.drain();
    assert_eq!(f.dm.ssd_offset(2), Some(2 * EXTENT));
    assert_eq!(f.dm.read_refs(2), Some(0));
    assert_eq!(f.writes(), 2, "no second job for key 2");
    f.commit(1);
    assert_eq!(f.dm.ssd_offset(1), Some(EXTENT));
    f.d.shutdown().unwrap();
}

#[test]
fn abort_of_a_queued_early_job_frees_the_slot_and_skips_the_write() {
    let f = setup();
    f.hold(true);
    f.copy(1, 0x11);
    f.wait_writes(1);
    f.copy(2, 0x22);

    // Returns at once: the queued job goes stale.
    f.d.release_memory(2).unwrap();
    assert!(!f.mt.contains(2));
    assert_eq!(f.mt.freed(2), 1);

    f.hold(false);
    f.drain();
    assert_eq!(f.writes(), 1, "the stale job must not write");
    assert_eq!(f.on_disk(2), None);
    assert!(!f.published(2));
    assert_eq!(f.mt.freed(2), 1);
    assert_eq!(f.dm.loc(2), None);
    f.d.shutdown().unwrap();
}

#[test]
fn abort_of_a_running_early_job_waits_for_it_then_aborts_the_extent() {
    let f = setup();
    f.hold(true);
    f.copy(1, 0xC1);
    f.wait_writes(1);

    let d = Arc::clone(&f.d);
    let (done, h) = spawn(move || d.release_memory(1).unwrap());
    std::thread::sleep(Duration::from_millis(50));
    assert!(!done.load(Ordering::Acquire), "abort must wait for the job");
    assert!(
        f.mt.contains(1),
        "the slot is not freed while the job reads it"
    );

    f.hold(false);
    h.join().unwrap();
    assert!(!f.mt.contains(1));
    assert_eq!(f.mt.freed(1), 1);
    assert_eq!(f.aborted(1), 1);
    assert!(!f.published(1));
    assert_eq!(f.dm.loc(1), None);
    f.drain();
    assert_eq!(f.mt.freed(1), 1);
    f.d.shutdown().unwrap();
}

#[test]
fn abort_of_a_landed_early_write_aborts_the_extent() {
    let f = setup();
    f.copy(1, 0xD1);
    f.drain();

    f.d.release_memory(1).unwrap();
    assert_eq!(f.mt.freed(1), 1);
    assert_eq!(f.aborted(1), 1);
    assert!(!f.published(1));
    assert!(f.d.copy_gpu_to_memory_completed(1, BLOCK).is_err());
    assert_eq!(f.dm.loc(1), None);
    f.d.shutdown().unwrap();
}

#[test]
fn remove_after_commit_while_the_early_job_runs_lets_the_job_free_the_slot() {
    let f = setup();
    f.hold(true);
    f.copy(1, 0xE1);
    f.wait_writes(1);
    f.commit(1);

    // FR-018: remove takes over the pin without waiting for the job.
    f.d.remove(1).unwrap();
    assert_eq!(f.dm.loc(1), None);
    assert!(f.mt.contains(1), "the running job still reads the slot");

    f.hold(false);
    f.drain();
    assert!(!f.mt.contains(1));
    assert_eq!(f.mt.freed(1), 1);
    assert_eq!(f.aborted(1), 1);
    assert!(!f.published(1));
    f.d.shutdown().unwrap();
}

#[test]
fn a_repeated_copy_cancels_the_prior_early_job() {
    let f = setup();
    f.copy(1, 0x01);
    f.drain();
    assert_eq!(f.on_disk(1), Some(0x01));

    f.copy(1, 0x02);
    assert_eq!(f.aborted(1), 1, "the first copy's extent is aborted");
    f.drain();
    assert_eq!(f.on_disk(1), Some(0x02));

    f.commit(1);
    assert!(f.published(1));
    assert_eq!(f.dm.ssd_offset(1), Some(EXTENT));
    assert_eq!(f.writes(), 2);
    assert_eq!(f.aborted(1), 1);
    assert_eq!(f.mt.freed(1), 0);
    f.d.shutdown().unwrap();
}

#[test]
fn flush_to_ssd_waits_for_early_jobs() {
    let f = setup();
    f.hold(true);
    f.copy(1, 0xF1);
    f.wait_writes(1);
    f.commit(1);

    let d = Arc::clone(&f.d);
    let (done, h) = spawn(move || {
        d.flush_to_ssd().unwrap();
    });
    std::thread::sleep(Duration::from_millis(50));
    assert!(
        !done.load(Ordering::Acquire),
        "flush must wait for the early job"
    );

    f.hold(false);
    h.join().unwrap();
    assert_eq!(f.dm.ssd_offset(1), Some(EXTENT));
    f.d.shutdown().unwrap();
}
