//! Repair test for finding `remove-refused-leaves-entry-unchanged`
//! (DIS-REMOVE-REFUSED-LEAVES-ENTRY-UNCHANGED).
//!
//! `remove`'s location pre-pass flips a resident, written-through entry to its
//! block-device state (`convert_memory_tier_to_block`) to learn which SSD
//! extent it would have to free. When the removal itself is then refused —
//! a reader, a client `pin` or a concurrent lookup holds the entry, so
//! `dm.remove` answers `ActiveReferences` and the hold is not a write-through
//! pin `remove` could take over — the remove returns `KeyNotFound` but leaves
//! the entry flipped: it is BlockDevice instead of MemoryTier, so a later
//! lookup is served from the SSD instead of DRAM.
//!
//! The spec (spec.md:239, idispatcher.md:171 and proposal A-9) promises the
//! opposite: a refused remove frees *nothing* and changes *nothing* — the
//! entry is still MemoryTier with its memory-tier slot and its `ssd_offset`,
//! and once the hold is released a later remove succeeds and frees both the
//! slot and the SSD extent.
//!
//! Self-contained (its own mocks, the public `IDispatcher` API only), so it
//! builds against the dispatcher as it was before the repair as well as
//! after it.

use std::collections::{BTreeSet, HashMap};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use component_core::channel::SpscChannel;
use component_core::query_interface;
use interfaces::{
    CacheKey, ClientChannels, Command, Completion, DispatchMapError, DispatcherConfig, DmaBuffer,
    Extent, ExtentManagerError, FormatParams, GpuDeviceInfo, GpuDmaBuffer, GpuIpcHandle, GpuStream,
    IBlockDevice, IDispatchMap, IDispatcher, IExtentManager, IGpuServices, IMemoryTier,
    LookupResult, MemoryTierError, MemoryTierTelemetrySnapshot, NvmeBlockError, OpHandle,
    TelemetrySnapshot, WriteHandle,
};

use crate::{ChannelPool, DataDrive, DispatcherComponent};

const BLOCK: u32 = 4096;
/// The one extent offset the mock extent manager hands out.
const EXTENT: u64 = 16 * BLOCK as u64;
/// The key the scenario runs on.
const KEY: CacheKey = 1;

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

struct Dm {
    entries: Mutex<HashMap<CacheKey, Ent>>,
    /// (key, answered MemoryTier?) per `lookup`, to observe which tier served
    /// a later lookup.
    answers: Mutex<Vec<(CacheKey, bool)>>,
}

impl Dm {
    fn new() -> Self {
        Self {
            entries: Mutex::new(HashMap::new()),
            answers: Mutex::new(Vec::new()),
        }
    }

    fn read_refs(&self, key: CacheKey) -> Option<u32> {
        self.entries.lock().unwrap().get(&key).map(|e| e.read_refs)
    }

    fn loc(&self, key: CacheKey) -> Option<Loc> {
        self.entries.lock().unwrap().get(&key).map(|e| e.loc)
    }

    fn ssd_offset(&self, key: CacheKey) -> Option<u64> {
        match self.loc(key) {
            Some(Loc::Memory { ssd, .. }) => ssd,
            Some(Loc::Block(o)) => Some(o),
            None => None,
        }
    }

    /// Forget the recorded lookup answers, so the next observation window
    /// starts clean.
    fn clear_answers(&self) {
        self.answers.lock().unwrap().clear();
    }

    /// Whether any lookup since the last `clear_answers` answered `key` from
    /// its memory-tier (DRAM) state.
    fn answered_memory(&self, key: CacheKey) -> bool {
        self.answers
            .lock()
            .unwrap()
            .iter()
            .any(|&(k, memory)| k == key && memory)
    }
}

impl IDispatchMap for Dm {
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
        let memory = matches!(e.loc, Loc::Memory { .. });
        let result = match e.loc {
            Loc::Block(offset) => LookupResult::BlockDevice { offset },
            Loc::Memory { ptr, size, .. } => LookupResult::MemoryTier {
                pointer: ptr as *mut u8,
                size,
            },
        };
        drop(entries);
        self.answers.lock().unwrap().push((key, memory));
        Ok(result)
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
        match entries.get(&key) {
            None => Err(DispatchMapError::KeyNotFound(key)),
            Some(e) if e.read_refs > 0 || e.write_ref => {
                Err(DispatchMapError::ActiveReferences(key))
            }
            Some(_) => {
                entries.remove(&key);
                Ok(())
            }
        }
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
    /// As `dispatch-map`'s `convert_memory_tier_to_block`: flips a resident
    /// entry that carries its write-through copy to its block-device state,
    /// keeping every reference count.
    fn convert_memory_tier_to_block(&self, key: CacheKey) -> Result<(), DispatchMapError> {
        let mut entries = self.entries.lock().unwrap();
        let e = entries
            .get_mut(&key)
            .ok_or(DispatchMapError::KeyNotFound(key))?;
        match e.loc {
            Loc::Memory { ssd: Some(off), .. } => {
                e.loc = Loc::Block(off);
                Ok(())
            }
            Loc::Memory { ssd: None, .. } => {
                Err(DispatchMapError::InvalidState("no ssd_offset".into()))
            }
            Loc::Block(_) => Err(DispatchMapError::InvalidState("already block".into())),
        }
    }
    /// As `dispatch-map`'s `promote_block_to_memory_tier`: the in-place
    /// inverse of the convert above — flips a block-device entry back to its
    /// memory-tier state at the given pointer, keeping every reference count
    /// and retaining the SSD offset.
    fn promote_block_to_memory_tier(
        &self,
        key: CacheKey,
        pointer: *mut u8,
        size: u32,
    ) -> Result<(), DispatchMapError> {
        let mut entries = self.entries.lock().unwrap();
        let e = entries
            .get_mut(&key)
            .ok_or(DispatchMapError::KeyNotFound(key))?;
        match e.loc {
            Loc::Block(off) => {
                e.loc = Loc::Memory {
                    ptr: pointer as usize,
                    size,
                    ssd: Some(off),
                };
                Ok(())
            }
            Loc::Memory { .. } => Err(DispatchMapError::InvalidState("already memory".into())),
        }
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

// --- memory tier: a flat, DMA-capable pool of 4 KiB slots with slot reuse ---

struct PoolInner {
    pool: Vec<u8>,
    slots: HashMap<CacheKey, (usize, u32)>,
    /// Free slot offsets, lowest handed out first, so reuse is observable.
    free: BTreeSet<usize>,
    used: usize,
}

struct PoolMt {
    inner: Mutex<PoolInner>,
}

impl PoolMt {
    fn new(capacity: usize) -> Self {
        Self {
            inner: Mutex::new(PoolInner {
                pool: vec![0u8; capacity],
                slots: HashMap::new(),
                free: (0..capacity).step_by(BLOCK as usize).collect(),
                used: 0,
            }),
        }
    }

    fn contains(&self, key: CacheKey) -> bool {
        self.inner.lock().unwrap().slots.contains_key(&key)
    }

    fn used(&self) -> usize {
        self.inner.lock().unwrap().used
    }
}

impl IMemoryTier for PoolMt {
    fn initialize(&self, _pool_size: usize, _numa: Option<i32>) -> Result<(), MemoryTierError> {
        Ok(())
    }
    fn insert(&self, key: CacheKey, size: u32) -> Result<*mut u8, MemoryTierError> {
        let mut g = self.inner.lock().unwrap();
        if g.slots.contains_key(&key) {
            return Err(MemoryTierError::AlreadyExists(key));
        }
        let aligned = (size as usize).next_multiple_of(BLOCK as usize);
        let off = *g.free.iter().next().ok_or(MemoryTierError::PoolFull)?;
        if g.used + aligned > g.pool.len() {
            return Err(MemoryTierError::PoolFull);
        }
        g.free.remove(&off);
        g.slots.insert(key, (off, size));
        g.used += aligned;
        // SAFETY: `off + aligned <= pool.len()` was checked above.
        Ok(unsafe { g.pool.as_mut_ptr().add(off) })
    }
    fn get(&self, key: CacheKey) -> Option<(*mut u8, u32)> {
        let g = self.inner.lock().unwrap();
        // SAFETY: every recorded slot offset lies inside the pool.
        g.slots
            .get(&key)
            .map(|&(off, size)| unsafe { ((g.pool.as_ptr() as *mut u8).add(off), size) })
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
        let mut keys: Vec<CacheKey> = self.inner.lock().unwrap().slots.keys().copied().collect();
        keys.sort_unstable();
        keys.truncate(n);
        keys
    }
    fn remove(&self, key: CacheKey) -> Result<(), MemoryTierError> {
        let mut g = self.inner.lock().unwrap();
        match g.slots.remove(&key) {
            Some((off, size)) => {
                g.free.insert(off);
                g.used -= (size as usize).next_multiple_of(BLOCK as usize);
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
        self.inner.lock().unwrap().pool.len()
    }
    fn used(&self) -> usize {
        self.inner.lock().unwrap().used
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
        let n = g.slots.len();
        g.slots.clear();
        g.free = (0..g.pool.len()).step_by(BLOCK as usize).collect();
        g.used = 0;
        Ok(n)
    }
    fn telemetry_snapshot(&self) -> MemoryTierTelemetrySnapshot {
        MemoryTierTelemetrySnapshot::default()
    }
    // A test double with no bounce reserve.
    fn set_bounce_reserve(&self, _: usize) -> Result<(), MemoryTierError> {
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

// --- NVMe block device: keeps written blocks ---

#[derive(Default)]
struct Disk {
    /// lba -> first byte of the block written there
    blocks: HashMap<u64, u8>,
}

#[derive(Default)]
struct MockNvme {
    disk: Arc<Mutex<Disk>>,
}

impl IBlockDevice for MockNvme {
    fn connect_client(&self) -> Result<ClientChannels, NvmeBlockError> {
        let cmd = SpscChannel::<Command>::new(64);
        let comp = SpscChannel::<Completion>::new(64);
        let (cmd_tx, cmd_rx) = (cmd.sender().unwrap(), cmd.receiver().unwrap());
        let (comp_tx, comp_rx) = (comp.sender().unwrap(), comp.receiver().unwrap());
        let disk = Arc::clone(&self.disk);
        std::thread::spawn(move || {
            while let Ok(c) = cmd_rx.recv() {
                let done = match c {
                    Command::WriteSync { lba, buf, .. } => {
                        let mut d = disk.lock().unwrap();
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

// --- extent manager: one extent offset, handed out again once freed ---

#[derive(Default)]
struct EmInner {
    /// offset -> key of committed extents
    published: HashMap<u64, CacheKey>,
    /// free extent offsets, lowest handed out first
    free: BTreeSet<u64>,
}

#[derive(Default)]
struct MockEm {
    inner: Arc<Mutex<EmInner>>,
}

impl MockEm {
    /// An extent manager with exactly one extent offset to give.
    fn with_single_extent(offset: u64) -> Self {
        let mut free = BTreeSet::new();
        free.insert(offset);
        Self {
            inner: Arc::new(Mutex::new(EmInner {
                published: HashMap::new(),
                free,
            })),
        }
    }

    fn used_extents(&self) -> usize {
        self.inner.lock().unwrap().published.len()
    }
}

impl IExtentManager for MockEm {
    fn format(&self, _params: FormatParams) -> Result<(), ExtentManagerError> {
        Ok(())
    }
    fn initialize(&self) -> Result<(), ExtentManagerError> {
        Ok(())
    }
    fn reserve_extent(&self, key: CacheKey, size: u32) -> Result<WriteHandle, ExtentManagerError> {
        let mut inner = self.inner.lock().unwrap();
        let offset = *inner
            .free
            .iter()
            .next()
            .ok_or(ExtentManagerError::OutOfSpace)?;
        inner.free.remove(&offset);
        let inner = Arc::clone(&self.inner);
        let inner_abort = Arc::clone(&self.inner);
        Ok(WriteHandle::new(
            key,
            offset,
            size,
            Box::new(move || {
                inner.lock().unwrap().published.insert(offset, key);
                Ok(Extent {
                    key,
                    size: size / BLOCK,
                    offset,
                })
            }),
            Box::new(move || {
                inner_abort.lock().unwrap().free.insert(offset);
            }),
        ))
    }
    fn get_extents(&self) -> Vec<Extent> {
        Vec::new()
    }
    fn for_each_extent(&self, _cb: &mut dyn FnMut(&Extent)) {}
    fn remove_extent(&self, offset: u64) -> Result<(), ExtentManagerError> {
        let mut inner = self.inner.lock().unwrap();
        if inner.published.remove(&offset).is_some() {
            inner.free.insert(offset);
        }
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
        (self.inner.lock().unwrap().published.len() as u64) * BLOCK as u64
    }
    fn capacity_bytes(&self) -> u64 {
        1 << 40
    }
    fn set_metadata_base_lba(&self, _base: u64) {}
    fn set_data_base_lba(&self, _base: u64) {}
    fn data_base_lba(&self) -> u64 {
        0
    }
}

// --- host-memory "GPU": every copy is a plain memcpy, there is no device ---

struct HostGpu;

impl HostGpu {
    fn copy(src: *const std::ffi::c_void, dst: *mut std::ffi::c_void, size: usize) {
        // SAFETY: test-only; both sides are host buffers of at least `size` bytes.
        unsafe { std::ptr::copy_nonoverlapping(src as *const u8, dst as *mut u8, size) }
    }
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
        Err("mock".into())
    }
    fn dma_copy_to_host(
        &self,
        src: *const std::ffi::c_void,
        dst: &DmaBuffer,
        size: usize,
    ) -> Result<(), String> {
        Self::copy(src, dst.as_ptr() as *mut _, size);
        Ok(())
    }
    fn dma_copy_to_device(
        &self,
        src: &DmaBuffer,
        dst: *mut std::ffi::c_void,
        size: usize,
    ) -> Result<(), String> {
        Self::copy(src.as_ptr() as *const _, dst, size);
        Ok(())
    }
    fn prepare_memory_for_spdk(
        &self,
        _payload: &str,
        _device_index: Option<u32>,
    ) -> Result<DmaBuffer, String> {
        Err("mock".into())
    }
    fn create_stream(&self) -> Result<GpuStream, String> {
        Ok(GpuStream(0x1 as *mut std::ffi::c_void))
    }
    fn set_device(&self, _device: i32) -> Result<(), String> {
        Ok(())
    }
    fn device_of_ptr(&self, _ptr: *const std::ffi::c_void) -> Result<i32, String> {
        Ok(-1)
    }
    fn destroy_stream(&self, _stream: GpuStream) -> Result<(), String> {
        Ok(())
    }
    fn stream_query(&self, _stream: GpuStream) -> Result<bool, String> {
        Ok(true)
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
        Self::copy(src.as_ptr() as *const _, dst, size);
        Ok(())
    }
    fn memcpy_h2d_async(
        &self,
        src: *const std::ffi::c_void,
        dst: *mut std::ffi::c_void,
        size: usize,
        _stream: GpuStream,
    ) -> Result<(), String> {
        Self::copy(src, dst, size);
        Ok(())
    }
    fn dma_copy_to_host_async(
        &self,
        src: *const std::ffi::c_void,
        dst: &DmaBuffer,
        size: usize,
        _stream: GpuStream,
    ) -> Result<(), String> {
        Self::copy(src, dst.as_ptr() as *mut _, size);
        Ok(())
    }
    fn memcpy_d2h_async(
        &self,
        src: *const std::ffi::c_void,
        dst: *mut std::ffi::c_void,
        size: usize,
        _stream: GpuStream,
    ) -> Result<(), String> {
        Self::copy(src, dst, size);
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

// --- fixtures ---

struct DriveFixture {
    d: Arc<dyn IDispatcher + Send + Sync>,
    dm: Arc<Dm>,
    mt: Arc<PoolMt>,
    em: Arc<MockEm>,
    _c: Arc<DispatcherComponent>,
}

/// A dispatcher with one mock data drive (real background write-through path)
/// whose extent manager owns a single extent offset, and a host-memory GPU so
/// `lookup` can serve a resident entry to a client buffer.
fn setup_with_drive() -> DriveFixture {
    let dm = Arc::new(Dm::new());
    let mt = Arc::new(PoolMt::new(1024 * 1024));
    let nvme = Arc::new(MockNvme::default());
    let ibd: Arc<dyn IBlockDevice + Send + Sync> = nvme;
    let em = Arc::new(MockEm::with_single_extent(EXTENT));
    let iem: Arc<dyn IExtentManager + Send + Sync> = em.clone();

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
    DriveFixture {
        d,
        dm,
        mt,
        em,
        _c: c,
    }
}

impl DriveFixture {
    /// Store `key` the way the shm-queue store path does: reserve the slot,
    /// fill it (stands in for the GPU copy), complete the store. The
    /// completion queues the key's background write-through.
    fn store(&self, key: CacheKey) {
        let ptr = self.d.reserve_memory(key, BLOCK, 0, None).unwrap();
        // SAFETY: `reserve_memory` returned a slot of BLOCK bytes.
        unsafe { std::ptr::write_bytes(ptr, key as u8, BLOCK as usize) };
        self.d.copy_gpu_to_memory_completed(key, BLOCK).unwrap();
    }

    /// Wait until the background writer has recorded an SSD copy for `key`.
    fn wait_persisted(&self, key: CacheKey) {
        let deadline = Instant::now() + Duration::from_secs(10);
        while self.dm.ssd_offset(key).is_none() {
            assert!(
                Instant::now() < deadline,
                "write-through of key {key} did not land in 10 s"
            );
            std::thread::sleep(Duration::from_millis(2));
        }
    }
}

fn handle(buf: &mut [u8]) -> interfaces::IpcHandle {
    interfaces::IpcHandle {
        address: buf.as_mut_ptr(),
        size: buf.len() as u32,
    }
}

// --- tests ---

/// DIS-REMOVE-REFUSED-LEAVES-ENTRY-UNCHANGED: a remove refused because a
/// client `pin` holds a resident, written-through entry returns KeyNotFound
/// and leaves the entry exactly as it was — still MemoryTier with its
/// memory-tier slot and its ssd_offset, nothing freed, a later lookup served
/// from DRAM — and once the hold is released, a remove succeeds and frees
/// both the slot and the SSD extent.
#[test]
fn refused_remove_leaves_the_entry_unchanged() {
    let f = setup_with_drive();
    f.store(KEY);
    f.wait_persisted(KEY);

    // Precondition: key is resident, written through and unpinned.
    let before = f.dm.loc(KEY).expect("the stored key is in the map");
    assert!(
        matches!(
            before,
            Loc::Memory {
                ssd: Some(EXTENT),
                ..
            }
        ),
        "precondition: key {KEY} is resident and written through (got {before:?})"
    );
    assert_eq!(f.dm.read_refs(KEY), Some(0), "precondition: unpinned");
    let Loc::Memory {
        ptr: slot,
        size: slot_size,
        ..
    } = before
    else {
        unreachable!("checked above")
    };
    assert_eq!(f.em.used_extents(), 1, "the extent is in use");

    // A client pins the entry: the hold that must refuse the remove.
    f.d.pin(KEY).unwrap();
    assert_eq!(f.dm.read_refs(KEY), Some(1), "the pin holds a read ref");

    // The remove is refused with KeyNotFound: nothing was freed.
    let r = f.d.remove(KEY);
    assert!(
        matches!(r, Err(interfaces::DispatcherError::KeyNotFound(k)) if k == KEY),
        "remove must refuse with KeyNotFound while a client pin holds the entry (got {r:?})"
    );

    // The entry is unchanged: still MemoryTier, same slot, same ssd_offset.
    assert!(
        matches!(
            f.dm.loc(KEY),
            Some(Loc::Memory {
                ptr,
                size,
                ssd: Some(EXTENT),
            }) if ptr == slot && size == slot_size
        ),
        "a refused remove changed the entry: it must stay MemoryTier with its \
         slot and ssd_offset (before {before:?}, after {:?})",
        f.dm.loc(KEY)
    );
    // The pin is untouched: a refused remove stole no reference.
    assert_eq!(
        f.dm.read_refs(KEY),
        Some(1),
        "a refused remove dropped the client's pin"
    );
    // Nothing is freed: the slot, the SSD extent and the map entry survive.
    assert!(
        f.mt.contains(KEY),
        "a refused remove freed the memory-tier slot"
    );
    assert_eq!(
        f.mt.used(),
        BLOCK as usize,
        "a refused remove freed DRAM bytes"
    );
    assert_eq!(
        f.em.used_extents(),
        1,
        "a refused remove freed the SSD extent"
    );

    // A later lookup is served from DRAM (the memory-tier state above).
    f.dm.clear_answers();
    let mut buf = vec![0u8; BLOCK as usize];
    f.d.lookup(KEY, handle(&mut buf))
        .expect("the later lookup must succeed");
    assert!(
        f.dm.answered_memory(KEY),
        "a later lookup was not served from DRAM after a refused remove \
         (entry state: {:?})",
        f.dm.loc(KEY)
    );
    assert!(
        buf.iter().all(|&b| b == KEY as u8),
        "the later lookup delivered wrong data"
    );

    // After the hold is released, a remove succeeds and frees both.
    f.d.unpin(KEY).unwrap();
    assert_eq!(f.dm.read_refs(KEY), Some(0), "the unpin released the hold");
    f.d.remove(KEY)
        .expect("remove after the release must succeed");
    assert_eq!(
        f.dm.loc(KEY),
        None,
        "remove after the release left the entry"
    );
    assert!(
        !f.mt.contains(KEY),
        "remove after the release did not free the slot"
    );
    assert_eq!(
        f.mt.used(),
        0,
        "remove after the release did not free the DRAM bytes"
    );
    assert_eq!(
        f.em.used_extents(),
        0,
        "remove after the release did not free the SSD extent"
    );

    f.d.shutdown().unwrap();
}
