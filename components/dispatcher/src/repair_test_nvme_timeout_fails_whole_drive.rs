//! Repair test for finding `nvme-timeout-fails-whole-drive`
//! (DIS-BATCH-LOOKUP-ERROR-ISOLATION, US-11 AS-4 "Edge case one fails").
//!
//! A read or DMA failure on one cold entry of a batch must be reported only
//! for that entry; the other entries are processed and reported independently.
//! A plain read error already fails only its own object; the defect is the
//! NVMe read **timeout**: `Completion::Timeout` carries only the actor-assigned
//! handle (not the caller tag), and the scatter-gather completion loop used
//! that as a reason to fail every entry on the drive.
//!
//! The dispatcher gets one mock data drive whose NVMe reads time out for
//! chosen keys (the mock actor delivers `Completion::Timeout` for their LBAs
//! and never completes them, exactly as the real actor forgets a timed-out
//! op). The other keys' reads succeed. The drive is placed in `data_drives`
//! before `initialize`, which keeps it in memory-tier-only mode (no SPDK
//! environment, no factory).
//!
//! Self-contained: it drives the public `IDispatcher` API through its own
//! mocks and touches only items that exist in the unfixed code as well.

use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Mutex};

use component_core::channel::SpscChannel;
use component_core::query_interface;
use interfaces::{
    CacheKey, ClientChannels, Command, Completion, DispatchMapError, DispatcherConfig, DmaBuffer,
    Extent, ExtentManagerError, FormatParams, GpuDeviceInfo, GpuDmaBuffer, GpuIpcHandle, GpuStream,
    IBlockDevice, IDispatchMap, IDispatcher, IExtentManager, IGpuServices, IMemoryTier, IpcHandle,
    LookupOutcome, LookupResult, MemoryTierError, MemoryTierTelemetrySnapshot, NvmeBlockError,
    OpHandle, TelemetrySnapshot, WriteHandle,
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

// --- GPU services: host memcpy, one (fake) device, working streams ---

#[derive(Default)]
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

// --- NVMe block device: reads of chosen LBAs time out ---

/// The byte a read returns at offset `i` of its buffer: position dependent,
/// so a piece copied from the wrong slot offset is detected.
fn disk_byte(i: usize) -> u8 {
    (i % 251) as u8
}

#[derive(Default)]
struct Disk {
    /// LBAs whose reads time out: the actor forgets the op and delivers
    /// `Completion::Timeout` — its `ReadDone` never arrives.
    timing_out: HashSet<u64>,
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
        let mut next_handle: u64 = 0;
        std::thread::spawn(move || {
            while let Ok(c) = cmd_rx.recv() {
                next_handle += 1;
                let done = match c {
                    Command::ReadAsync { lba, buf, tag, .. } => {
                        if disk.lock().unwrap().timing_out.contains(&lba) {
                            // Timed out: the op is forgotten, no ReadDone ever
                            // arrives for it, and the buffer is left untouched.
                            Completion::Timeout {
                                handle: OpHandle(next_handle),
                            }
                        } else {
                            // SAFETY: the buffer is at least one block long.
                            if let Ok(mut b) = buf.lock() {
                                for (i, byte) in b.as_mut_slice().iter_mut().enumerate() {
                                    *byte = disk_byte(i);
                                }
                            }
                            Completion::ReadDone {
                                handle: OpHandle(next_handle),
                                tag,
                                result: Ok(()),
                            }
                        }
                    }
                    Command::WriteSync { lba, .. } => Completion::WriteDone {
                        handle: OpHandle(next_handle),
                        tag: 0,
                        result: if disk.lock().unwrap().timing_out.contains(&lba) {
                            Err(NvmeBlockError::BlockDevice(
                                interfaces::BlockDeviceError::WriteFailed("timed out".into()),
                            ))
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

/// A dispatcher with one mock data drive. `timing_out_reads` lists the keys
/// whose NVMe reads time out (the actor forgets the op).
fn setup(timing_out_reads: &[CacheKey]) -> Fixture {
    let dm = Arc::new(MapDm::default());
    let mt = Arc::new(PoolMt::new(1024 * 1024));
    let nvme = Arc::new(MockNvme::default());
    {
        let mut disk = nvme.disk.lock().unwrap();
        for &k in timing_out_reads {
            disk.timing_out.insert(k * EXTENT / BLOCK as u64);
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
        .connect(Arc::new(HostGpu) as Arc<dyn IGpuServices + Send + Sync>)
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

/// The property's postcondition for the failed entry: its result is an error
/// (the read failure is reported), its reserved slot is released, and the key
/// stays SSD-resident.
fn assert_timed_out_entry(f: &Fixture, out: &LookupOutcome, key: CacheKey, off: u64) {
    assert!(
        out.result.is_err(),
        "the timed-out entry {key} must report the read failure, got {:?}",
        out.result
    );
    assert_eq!(
        out.served_by,
        interfaces::ServedBy::Error,
        "a failed entry is attributed Error"
    );
    assert!(
        !f.mt.contains(key),
        "the timed-out entry {key} must not keep a memory-tier slot"
    );
    assert_eq!(
        f.dm.loc(key),
        Some(Loc::Block(off)),
        "key {key} must stay SSD resident (BlockDevice) at {off}"
    );
}

/// The property's postcondition for an unaffected entry: it is served —
/// promoted and delivered — exactly as it would be without the sibling
/// failure.
fn assert_served_entry(f: &Fixture, out: &LookupOutcome, key: CacheKey, buf: &[u8]) {
    assert_eq!(
        out.served_by,
        interfaces::ServedBy::Ssd,
        "the unaffected cold entry {key} keeps its Ssd attribution"
    );
    assert!(
        out.result.is_ok(),
        "the unaffected entry {key} must be served independently of the sibling \
         timeout, got {:?}",
        out.result
    );
    for (i, &b) in buf.iter().enumerate() {
        assert_eq!(
            b,
            disk_byte(i),
            "entry {key} byte {i} must come from its own SSD read"
        );
    }
    assert!(
        f.mt.contains(key),
        "the unaffected entry {key} must be promoted to the memory tier"
    );
}

/// Control: nothing fails. Every cold entry of the batch is promoted and
/// served — proves the fixture actually exercises the cold batch path.
#[test]
fn batch_lookup_succeeds_when_nothing_times_out() {
    let f = setup(&[]);
    seed_cold(&f, 1);
    seed_cold(&f, 2);
    seed_cold(&f, 3);
    let (mut b1, h1) = gpu_buf();
    let (mut b2, h2) = gpu_buf();
    let (mut b3, h3) = gpu_buf();

    let out =
        f.d.batch_lookup(&[(1, vec![h1]), (2, vec![h2]), (3, vec![h3])]);

    assert_eq!(out.len(), 3);
    assert_served_entry(&f, &out[0], 1, &b1);
    assert_served_entry(&f, &out[1], 2, &b2);
    assert_served_entry(&f, &out[2], 3, &b3);
    f.d.shutdown().unwrap();
}

/// DIS-BATCH-LOOKUP-ERROR-ISOLATION: one cold entry's NVMe read times out;
/// the error is reported for that entry only and the other entries on the
/// same drive are processed and reported independently.
#[test]
fn batch_lookup_times_out_one_entry_only() {
    let f = setup(&[1]);
    let off1 = seed_cold(&f, 1);
    seed_cold(&f, 2);
    seed_cold(&f, 3);
    let (mut b1, h1) = gpu_buf();
    let (mut b2, h2) = gpu_buf();
    let (mut b3, h3) = gpu_buf();

    let out =
        f.d.batch_lookup(&[(1, vec![h1]), (2, vec![h2]), (3, vec![h3])]);

    assert_eq!(out.len(), 3);
    assert_timed_out_entry(&f, &out[0], 1, off1);
    assert_served_entry(&f, &out[1], 2, &b2);
    assert_served_entry(&f, &out[2], 3, &b3);
    f.d.shutdown().unwrap();
}

/// DIS-BATCH-LOOKUP-ERROR-ISOLATION: two cold entries time out and one
/// succeeds — each timed-out entry is reported for itself, and the entry
/// whose reads completed is still served.
#[test]
fn batch_lookup_times_out_two_entries_independently() {
    let f = setup(&[1, 3]);
    let off1 = seed_cold(&f, 1);
    let off3 = seed_cold(&f, 3);
    seed_cold(&f, 2);
    let (mut b1, h1) = gpu_buf();
    let (mut b2, h2) = gpu_buf();
    let (mut b3, h3) = gpu_buf();

    let out =
        f.d.batch_lookup(&[(1, vec![h1]), (2, vec![h2]), (3, vec![h3])]);

    assert_eq!(out.len(), 3);
    assert_timed_out_entry(&f, &out[0], 1, off1);
    assert_served_entry(&f, &out[1], 2, &b2);
    assert_timed_out_entry(&f, &out[2], 3, off3);
    f.d.shutdown().unwrap();
}

/// DIS-BATCH-LOOKUP-ERROR-ISOLATION: a lone cold entry whose read times out
/// is reported as failed (single-entry batch, still per-entry).
#[test]
fn batch_lookup_single_entry_timeout_fails_that_entry() {
    let f = setup(&[1]);
    let off1 = seed_cold(&f, 1);
    let (_b1, h1) = gpu_buf();

    let out = f.d.batch_lookup(&[(1, vec![h1])]);

    assert_eq!(out.len(), 1);
    assert_timed_out_entry(&f, &out[0], 1, off1);
    f.d.shutdown().unwrap();
}
