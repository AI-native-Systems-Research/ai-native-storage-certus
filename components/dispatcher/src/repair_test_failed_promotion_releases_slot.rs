//! Repair test for finding `failed-promotion-releases-slot`
//! (DIS-LOOKUP-COLD-FAILURE-RELEASES-SLOT, DIS-PROMOTE-READ-FAILURE-RELEASES,
//! DIS-BATCH-LOOKUP-COLD-FAILURE-RELEASES-SLOT).
//!
//! A cold (SSD-resident) key obtains a memory-tier slot while it is being
//! promoted. If the NVMe read or the dispatch-map promotion then fails, the
//! slot must be released: the dispatch map still points the key at the SSD,
//! so a slot the map does not reference is dead capacity no lookup can use.
//!
//! The dispatcher gets one mock data drive whose NVMe reads fail for chosen
//! keys, and (per test) a dispatch map whose `promote_block_to_memory_tier`
//! fails for a chosen key. The drive is placed in `data_drives` before
//! `initialize`, which keeps it in memory-tier-only mode (no SPDK
//! environment, no factory). The cold key is seeded the way a demotion or
//! recovery leaves it: a BlockDevice entry at a known offset.
//!
//! Self-contained: it drives the public `IDispatcher` API through its own
//! mocks and touches only items that exist in the unfixed code as well.

use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Mutex};

use component_core::channel::SpscChannel;
use component_core::query_interface;
use interfaces::{
    BlockDeviceError, CacheKey, ClientChannels, Command, Completion, DispatchMapError,
    DispatcherConfig, DmaBuffer, Extent, ExtentManagerError, FormatParams, GpuDeviceInfo,
    GpuDmaBuffer, GpuIpcHandle, GpuStream, IBlockDevice, IDispatchMap, IDispatcher, IExtentManager,
    IGpuServices, IMemoryTier, IpcHandle, LookupOutcome, LookupResult, MemoryTierError,
    MemoryTierTelemetrySnapshot, NvmeBlockError, OpHandle, TelemetrySnapshot, WriteHandle,
};

use crate::{ChannelPool, DataDrive, DispatcherComponent};

const BLOCK: u32 = 4096;
/// Bytes of disk per key: key `k` lives at byte offset `k * EXTENT`.
const EXTENT: u64 = 16 * BLOCK as u64;

// --- dispatch map: entries with reference counts, as the real one enforces them ---

#[derive(Clone, Copy, Debug, PartialEq)]
enum Loc {
    Block(u64),
    // Pointer stored as usize so the mock stays Send + Sync without an unsafe impl.
    Memory { ptr: usize, size: u32 },
}

struct Ent {
    loc: Loc,
    read_refs: u32,
    write_ref: bool,
}

#[derive(Default)]
struct MapDm {
    entries: Mutex<HashMap<CacheKey, Ent>>,
    /// `promote_block_to_memory_tier` fails for this key (stands for a
    /// dispatch-map failure after the slot was read into).
    fail_promote: Mutex<Option<CacheKey>>,
}

impl MapDm {
    fn loc(&self, key: CacheKey) -> Option<Loc> {
        self.entries.lock().unwrap().get(&key).map(|e| e.loc)
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
            Loc::Memory { ptr, size } => LookupResult::MemoryTier {
                pointer: ptr as *mut u8,
                size,
            },
        })
    }
    fn convert_to_storage(&self, _key: CacheKey, _offset: u64) -> Result<(), DispatchMapError> {
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
        key: CacheKey,
        pointer: *mut u8,
        size: u32,
    ) -> Result<(), DispatchMapError> {
        if *self.fail_promote.lock().unwrap() == Some(key) {
            return Err(DispatchMapError::InvalidState("injected".into()));
        }
        let mut entries = self.entries.lock().unwrap();
        let e = entries
            .get_mut(&key)
            .ok_or(DispatchMapError::KeyNotFound(key))?;
        e.loc = Loc::Memory {
            ptr: pointer as usize,
            size,
        };
        Ok(())
    }
    fn is_evictable(&self, _key: CacheKey) -> bool {
        false
    }
    fn try_evict_to_block(&self, _key: CacheKey) -> Result<(), DispatchMapError> {
        Err(DispatchMapError::InvalidState("not evictable".into()))
    }
    fn recover_extent(
        &self,
        key: CacheKey,
        offset: u64,
        _size_blocks: u32,
    ) -> Result<(), DispatchMapError> {
        let mut entries = self.entries.lock().unwrap();
        if entries.contains_key(&key) {
            return Err(DispatchMapError::AlreadyExists(key));
        }
        entries.insert(
            key,
            Ent {
                loc: Loc::Block(offset),
                read_refs: 0,
                write_ref: false,
            },
        );
        Ok(())
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

// --- memory tier: a flat pool of 4 KiB-aligned slots ---

/// (pool bytes, key -> (offset, size), bytes used)
type PoolState = (Vec<u8>, HashMap<CacheKey, (usize, u32)>, usize);

struct PoolMt {
    inner: Mutex<PoolState>,
}

impl PoolMt {
    fn new(capacity: usize) -> Self {
        Self {
            inner: Mutex::new((vec![0u8; capacity], HashMap::new(), 0)),
        }
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
            .map(|_| ())
            .ok_or(MemoryTierError::KeyNotFound(key))
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
        let g = self.inner.lock().unwrap();
        Some((g.0.as_ptr() as *mut u8, g.0.len()))
    }
    fn is_dma_capable(&self) -> bool {
        false
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
}

// --- GPU services: host memcpy, one (fake) device, working streams ---

#[derive(Default)]
struct HostGpu {
    /// `stream_synchronize` fails (stands for a CUDA error surfacing at the
    /// final pipe-stream sync).
    fail_sync: bool,
}

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
        Ok(GpuStream(0x1 as *mut std::ffi::c_void))
    }
    fn set_device(&self, _device: i32) -> Result<(), String> {
        Ok(())
    }
    fn device_of_ptr(&self, _ptr: *const std::ffi::c_void) -> Result<i32, String> {
        // The host buffers "live on" device 0 so the cold path binds device
        // streams instead of falling back to the (absent) pipeline ring.
        Ok(0)
    }
    fn stream_query(&self, _s: GpuStream) -> Result<bool, String> {
        Ok(true)
    }
    fn destroy_stream(&self, _s: GpuStream) -> Result<(), String> {
        Ok(())
    }
    fn stream_synchronize(&self, _s: GpuStream) -> Result<(), String> {
        if self.fail_sync {
            return Err("mock: injected stream_synchronize failure".into());
        }
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
            self.memcpy_h2d_async(src, dst, size, stream)?;
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

// --- NVMe block device: fails the reads of chosen LBAs ---

/// The byte a read returns at offset `i` of its buffer: position dependent,
/// so a piece copied from the wrong slot offset is detected.
fn disk_byte(i: usize) -> u8 {
    (i % 251) as u8
}

#[derive(Default)]
struct Disk {
    /// LBAs whose reads fail with an I/O error.
    failing: HashSet<u64>,
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
                    Command::ReadAsync { lba, buf, tag, .. } => {
                        let result = if disk.lock().unwrap().failing.contains(&lba) {
                            Err(NvmeBlockError::BlockDevice(BlockDeviceError::ReadFailed(
                                "injected media error".into(),
                            )))
                        } else {
                            // SAFETY: the buffer is at least one block long.
                            if let Ok(mut b) = buf.lock() {
                                for (i, byte) in b.as_mut_slice().iter_mut().enumerate() {
                                    *byte = disk_byte(i);
                                }
                            }
                            Ok(())
                        };
                        Completion::ReadDone {
                            handle: OpHandle(1),
                            tag,
                            result,
                        }
                    }
                    Command::WriteSync { lba, .. } => Completion::WriteDone {
                        handle: OpHandle(1),
                        tag: 0,
                        result: if disk.lock().unwrap().failing.contains(&lba) {
                            Err(NvmeBlockError::BlockDevice(BlockDeviceError::WriteFailed(
                                "injected media error".into(),
                            )))
                        } else {
                            Ok(())
                        },
                    },
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

// --- extent manager: key k gets the extent at k * EXTENT ---

#[derive(Default)]
struct MockEm {
    _published: Arc<Mutex<HashMap<u64, CacheKey>>>,
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
        Ok(WriteHandle::new(
            key,
            offset,
            size,
            Box::new(move || {
                Ok(Extent {
                    key,
                    size: size / BLOCK,
                    offset,
                })
            }),
            Box::new(|| {}),
        ))
    }
    fn get_extents(&self) -> Vec<Extent> {
        Vec::new()
    }
    fn for_each_extent(&self, _cb: &mut dyn FnMut(&Extent)) {}
    fn remove_extent(&self, _offset: u64) -> Result<(), ExtentManagerError> {
        Ok(())
    }
    fn checkpoint(&self) -> Result<(), ExtentManagerError> {
        Ok(())
    }
    fn get_instance_id(&self) -> Result<u64, ExtentManagerError> {
        Ok(1)
    }
    fn set_checkpoint_interval(&self, _interval: Option<std::time::Duration>) {}
    fn used_bytes(&self) -> u64 {
        0
    }
    fn capacity_bytes(&self) -> u64 {
        1 << 40
    }
    fn set_metadata_base_lba(&self, _base_lba: u64) {}
    fn set_data_base_lba(&self, _base_lba: u64) {}
    fn data_base_lba(&self) -> u64 {
        0
    }
}

// --- fixture ---

struct Fixture {
    d: Arc<dyn IDispatcher + Send + Sync>,
    dm: Arc<MapDm>,
    mt: Arc<PoolMt>,
    _c: Arc<DispatcherComponent>,
}

/// A dispatcher with one mock data drive. `failing_reads` lists the keys whose
/// NVMe reads fail; `fail_promote` makes the dispatch map refuse the in-place
/// promotion of that key.
fn setup(failing_reads: &[CacheKey], fail_promote: Option<CacheKey>) -> Fixture {
    setup_with_gpu(failing_reads, fail_promote, HostGpu::default())
}

fn setup_with_gpu(
    failing_reads: &[CacheKey],
    fail_promote: Option<CacheKey>,
    gpu: HostGpu,
) -> Fixture {
    let dm = Arc::new(MapDm {
        entries: Mutex::new(HashMap::new()),
        fail_promote: Mutex::new(fail_promote),
    });
    let mt = Arc::new(PoolMt::new(1024 * 1024));
    let nvme = Arc::new(MockNvme::default());
    {
        let mut disk = nvme.disk.lock().unwrap();
        for &k in failing_reads {
            disk.failing.insert(k * EXTENT / BLOCK as u64);
        }
    }
    let ibd: Arc<dyn IBlockDevice + Send + Sync> = nvme;
    let iem: Arc<dyn IExtentManager + Send + Sync> = Arc::new(MockEm::default());

    let c = DispatcherComponent::new_default();
    c.dispatch_map
        .connect(Arc::clone(&dm) as Arc<dyn IDispatchMap + Send + Sync>)
        .unwrap();
    c.memory_tier
        .connect(Arc::clone(&mt) as Arc<dyn IMemoryTier + Send + Sync>)
        .unwrap();
    c.gpu_services
        .connect(Arc::new(gpu) as Arc<dyn IGpuServices + Send + Sync>)
        .unwrap();
    // A placeholder IUnknown for the drive's component handles; never queried.
    let placeholder: Arc<dyn component_core::IUnknown + Send + Sync> =
        spdk_env::SPDKEnvComponent::new_default();
    // Memory-tier-only initialize (no SPDK environment, no factory) leaves
    // `data_drives` as it finds it, so the cold paths use this drive.
    c.data_drives.write().push(DataDrive {
        _block_dev: Arc::clone(&placeholder),
        block_dev_admin: None,
        block_dev_iface: Arc::clone(&ibd),
        _extent_mgr_component: placeholder,
        extent_mgr: iem,
        channel_pool: ChannelPool::new(ibd),
    });
    let d = query_interface!(c, IDispatcher).unwrap();
    d.initialize(DispatcherConfig {
        data_pci_addrs: vec!["0000:02:00.0".to_string()],
        ssd_eviction_threshold: 0.0,
        store_backpressure_ms: 0,
        ..Default::default()
    })
    .unwrap();
    Fixture { d, dm, mt, _c: c }
}

/// The cold key, seeded the way a demotion or recovery leaves it: a
/// BlockDevice dispatch-map entry at `key * EXTENT`.
fn seed_cold(f: &Fixture, key: CacheKey) -> u64 {
    let off = key * EXTENT;
    f.dm.recover_extent(key, off, 1).unwrap();
    off
}

/// A 4 KiB GPU destination buffer plus its IPC handle.
fn gpu_buf() -> (Vec<u8>, IpcHandle) {
    let mut buf = vec![0u8; BLOCK as usize];
    let h = IpcHandle {
        address: buf.as_mut_ptr(),
        size: BLOCK,
    };
    (buf, h)
}

/// The property's postcondition: no memory-tier slot for `key` that the
/// dispatch map does not point to, and the key stays SSD resident at `off`.
fn assert_released_and_ssd(f: &Fixture, key: CacheKey, off: u64) {
    assert!(
        !f.mt.contains(key),
        "memory tier still holds a slot for key {key} whose dispatch-map entry was not promoted \
         (entry {:?}) — the slot is dead capacity",
        f.dm.loc(key)
    );
    assert_eq!(
        f.dm.loc(key),
        Some(Loc::Block(off)),
        "key {key} must stay SSD resident (BlockDevice) at {off}"
    );
}

/// Control: nothing fails. The cold lookup promotes the key into the memory
/// tier and serves it — proves the fixture actually exercises the cold path.
#[test]
fn cold_lookup_succeeds_when_nothing_fails() {
    let f = setup(&[], None);
    let off = seed_cold(&f, 1);
    let (_buf, h) = gpu_buf();

    let r = f.d.lookup(1, h);

    assert!(r.is_ok(), "control cold lookup must succeed, got {r:?}");
    assert!(
        f.mt.contains(1),
        "the promoted key must be memory-tier resident"
    );
    assert_eq!(
        f.dm.loc(1),
        Some(Loc::Memory {
            ptr: f.mt.get(1).unwrap().0 as usize,
            size: BLOCK
        }),
        "the dispatch map must point at the promoted slot"
    );
    assert_ne!(f.dm.loc(1), Some(Loc::Block(off)));
    f.d.shutdown().unwrap();
}

/// DIS-LOOKUP-COLD-FAILURE-RELEASES-SLOT: the SSD read of a cold lookup fails
/// after `evict_and_insert` reserved the slot — the slot must be released.
#[test]
fn cold_lookup_releases_slot_when_ssd_read_fails() {
    let f = setup(&[1], None);
    let off = seed_cold(&f, 1);
    let (_buf, h) = gpu_buf();

    let r = f.d.lookup(1, h);

    assert!(
        r.is_err(),
        "the lookup must report the failed read, got {r:?}"
    );
    assert_released_and_ssd(&f, 1, off);
    f.d.shutdown().unwrap();
}

/// DIS-LOOKUP-COLD-FAILURE-RELEASES-SLOT / DIS-PROMOTE-READ-FAILURE-RELEASES:
/// the dispatch-map promotion of a cold lookup fails after the slot was read
/// into — the slot must be released and the key stays SSD resident.
#[test]
fn cold_lookup_releases_slot_when_promote_fails() {
    let f = setup(&[], Some(1));
    let off = seed_cold(&f, 1);
    let (_buf, h) = gpu_buf();

    let r = f.d.lookup(1, h);

    assert!(
        r.is_err(),
        "the lookup must report the failed promotion, got {r:?}"
    );
    assert_released_and_ssd(&f, 1, off);
    f.d.shutdown().unwrap();
}

/// DIS-BATCH-LOOKUP-COLD-FAILURE-RELEASES-SLOT: the NVMe read of a cold batch
/// entry fails after `evict_and_insert` reserved its slot.
#[test]
fn batch_lookup_releases_slot_when_ssd_read_fails() {
    let f = setup(&[1], None);
    let off = seed_cold(&f, 1);
    let (_buf, h) = gpu_buf();

    let out = f.d.batch_lookup(&[(1, vec![h])]);

    assert_eq!(out.len(), 1);
    assert!(
        out[0].result.is_err(),
        "the batch must report the failed read, got {:?}",
        out[0].result
    );
    assert_released_and_ssd(&f, 1, off);
    f.d.shutdown().unwrap();
}

/// DIS-BATCH-LOOKUP-COLD-FAILURE-RELEASES-SLOT: the dispatch-map promotion of
/// a cold batch entry fails after its slot was read into.
#[test]
fn batch_lookup_releases_slot_when_promote_fails() {
    let f = setup(&[], Some(1));
    let off = seed_cold(&f, 1);
    let (_buf, h) = gpu_buf();

    let out = f.d.batch_lookup(&[(1, vec![h])]);

    assert_eq!(out.len(), 1);
    assert!(
        out[0].result.is_err(),
        "the batch must report the failed promotion, got {:?}",
        out[0].result
    );
    assert_released_and_ssd(&f, 1, off);
    f.d.shutdown().unwrap();
}

/// DIS-PROMOTE-READ-FAILURE-RELEASES: `promote_to_memory_tier` — the
/// dispatch-map promotion fails after the slot was read into.
#[test]
fn promote_to_memory_tier_releases_slot_when_promote_fails() {
    let f = setup(&[], Some(1));
    let off = seed_cold(&f, 1);

    f.d.promote_to_memory_tier(&[1]);

    assert_released_and_ssd(&f, 1, off);
    f.d.shutdown().unwrap();
}

/// DIS-PROMOTE-READ-FAILURE-RELEASES: `promote_to_memory_tier` — the SSD read
/// fails. This is the one failure exit that already releases the slot; it
/// must keep doing so after the fix.
#[test]
fn promote_to_memory_tier_releases_slot_when_ssd_read_fails() {
    let f = setup(&[1], None);
    let off = seed_cold(&f, 1);

    f.d.promote_to_memory_tier(&[1]);

    assert_released_and_ssd(&f, 1, off);
    f.d.shutdown().unwrap();
}

/// Control for the batch path: nothing fails, the cold entry is promoted and
/// served.
#[test]
fn batch_lookup_succeeds_when_nothing_fails() {
    let f = setup(&[], None);
    seed_cold(&f, 1);
    let (_buf, h) = gpu_buf();

    let out: Vec<LookupOutcome> = f.d.batch_lookup(&[(1, vec![h])]);

    assert_eq!(out.len(), 1);
    assert!(out[0].result.is_ok(), "control batch must succeed");
    assert!(
        f.mt.contains(1),
        "the promoted key must be memory-tier resident"
    );
    f.d.shutdown().unwrap();
}

/// Four 1 KiB GPU pieces of one 4 KiB block, each in its own buffer.
const PIECE: u32 = BLOCK / 4;

fn gpu_pieces() -> (Vec<Vec<u8>>, Vec<IpcHandle>) {
    let mut bufs: Vec<Vec<u8>> = (0..4).map(|_| vec![0u8; PIECE as usize]).collect();
    let handles = bufs
        .iter_mut()
        .map(|b| IpcHandle {
            address: b.as_mut_ptr(),
            size: PIECE,
        })
        .collect();
    (bufs, handles)
}

/// FR-061: a multi-region cold block is delivered inside the scatter-gather
/// call — every piece gets the bytes at its own slot offset — and the key ends
/// memory-tier resident.
#[test]
fn batch_lookup_multi_region_cold_delivers_each_piece() {
    let f = setup(&[], None);
    seed_cold(&f, 1);
    let (bufs, handles) = gpu_pieces();

    let out = f.d.batch_lookup(&[(1, handles)]);

    assert_eq!(out.len(), 1);
    assert!(
        out[0].result.is_ok(),
        "multi-region cold batch must succeed, got {:?}",
        out[0].result
    );
    for (l, buf) in bufs.iter().enumerate() {
        let base = l * PIECE as usize;
        for (i, &b) in buf.iter().enumerate() {
            assert_eq!(
                b,
                disk_byte(base + i),
                "piece {l} byte {i} must come from slot offset {}",
                base + i
            );
        }
    }
    assert!(
        f.mt.contains(1),
        "the promoted key must be memory-tier resident"
    );
    assert_eq!(
        f.dm.loc(1),
        Some(Loc::Memory {
            ptr: f.mt.get(1).unwrap().0 as usize,
            size: BLOCK
        }),
        "the dispatch map must point at the promoted slot"
    );
    f.d.shutdown().unwrap();
}

/// FR-061: the final pipe-stream sync fails — the multi-region cold key must
/// not be reported served or promoted; its slot is released.
#[test]
fn batch_lookup_multi_region_cold_fails_when_final_sync_fails() {
    let f = setup_with_gpu(&[], None, HostGpu { fail_sync: true });
    let off = seed_cold(&f, 1);
    let (_bufs, handles) = gpu_pieces();

    let out = f.d.batch_lookup(&[(1, handles)]);

    assert_eq!(out.len(), 1);
    assert!(
        out[0].result.is_err(),
        "a failed final sync must fail the key, got {:?}",
        out[0].result
    );
    assert_released_and_ssd(&f, 1, off);
    f.d.shutdown().unwrap();
}
