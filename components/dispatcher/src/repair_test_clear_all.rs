//! Tests for `IDispatcher::clear_all`: every entry leaves both tiers, its
//! extent is freed, a client hears of each removal, and a pinned entry is
//! kept and reported rather than freed under its reader.
//!
//! The fixture is `repair_test_flush_failed_write`'s -- one mock data drive
//! behind the real background writer, entries stored the way the shm-queue
//! store path does it -- with an extent manager that lists its extents, since
//! `clear_all` finds SSD-only entries through `for_each_extent`.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::ignored_unit_patterns,
    clippy::ptr_cast_constness,
    clippy::cast_lossless,
    clippy::cast_possible_truncation,
    reason = "test doubles copied from repair_test_flush_failed_write, kept as written there"
)]

use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use component_core::channel::SpscChannel;
use component_core::query_interface;
use interfaces::{
    BlockDeviceError, CacheKey, ClientChannels, Command, Completion, DispatchMapError,
    DispatcherConfig, Extent, ExtentManagerError, FormatParams, IBlockDevice, IDispatchMap,
    IDispatcher, IExtentManager, IMemoryTier, LookupResult, MemoryTierError,
    MemoryTierTelemetrySnapshot, NvmeBlockError, OpHandle, TelemetrySnapshot, WriteHandle,
};

use crate::{ChannelPool, DataDrive, DispatcherComponent};

const BLOCK: u32 = 4096;
/// Bytes of disk per key: key `k` is written at byte offset `k * EXTENT`.
const EXTENT: u64 = 16 * BLOCK as u64;

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

// --- NVMe block device: keeps written blocks, fails the writes of chosen LBAs ---

#[derive(Default)]
struct Disk {
    /// lba -> first byte of the block written there
    blocks: HashMap<u64, u8>,
    /// writes to these LBAs fail with an I/O error
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
                    Command::WriteSync { lba, buf, .. } => {
                        let mut d = disk.lock().unwrap();
                        let result = if d.failing.contains(&lba) {
                            Err(NvmeBlockError::BlockDevice(BlockDeviceError::WriteFailed(
                                "injected media error".into(),
                            )))
                        } else {
                            d.blocks.insert(lba, buf.as_slice()[0]);
                            Ok(())
                        };
                        Completion::WriteDone {
                            handle: OpHandle(1),
                            tag: 0,
                            result,
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

// --- extent manager: key k gets the extent at k * EXTENT ---

#[derive(Default)]
struct MockEm {
    published: Arc<Mutex<HashMap<u64, CacheKey>>>,
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
            Box::new(|| {}),
        ))
    }
    fn get_extents(&self) -> Vec<Extent> {
        Vec::new()
    }
    fn for_each_extent(&self, cb: &mut dyn FnMut(&Extent)) {
        for (&offset, &key) in self.published.lock().unwrap().iter() {
            cb(&Extent {
                key,
                size: 1,
                offset,
            });
        }
    }
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
    extents: Arc<Mutex<HashMap<u64, CacheKey>>>,
    events: crossbeam_channel::Receiver<interfaces::EvictionEvent>,
    _c: Arc<DispatcherComponent>,
}

/// A dispatcher with one mock data drive whose writes of `failing_keys` fail.
fn setup(failing_keys: &[CacheKey]) -> Fixture {
    let dm = Arc::new(MapDm::default());
    let mt = Arc::new(PoolMt::new(1024 * 1024));
    let nvme = Arc::new(MockNvme::default());
    {
        let mut disk = nvme.disk.lock().unwrap();
        for &k in failing_keys {
            disk.failing.insert(k * EXTENT / BLOCK as u64);
        }
    }
    let ibd: Arc<dyn IBlockDevice + Send + Sync> = nvme;
    let em = MockEm::default();
    let extents = Arc::clone(&em.published);
    let iem: Arc<dyn IExtentManager + Send + Sync> = Arc::new(em);

    let c = DispatcherComponent::new_default();
    c.dispatch_map
        .connect(Arc::clone(&dm) as Arc<dyn IDispatchMap + Send + Sync>)
        .unwrap();
    c.memory_tier
        .connect(Arc::clone(&mt) as Arc<dyn IMemoryTier + Send + Sync>)
        .unwrap();
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
    let events = c.create_eviction_channel(1024);
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
        extents,
        events,
        _c: c,
    }
}

impl Fixture {
    /// Store `key` the way the shm-queue store path does: reserve the slot, fill
    /// it (stands in for the GPU copy), complete the store. The completion
    /// queues the key's background write-through.
    fn store(&self, key: CacheKey) {
        let ptr = self.d.reserve_memory(key, BLOCK, 0, None).unwrap();
        // SAFETY: `reserve_memory` returned a slot of BLOCK bytes.
        unsafe { std::ptr::write_bytes(ptr, key as u8, BLOCK as usize) };
        self.d.copy_gpu_to_memory_completed(key, BLOCK).unwrap();
    }

    /// Wait until the background writer has recorded an SSD copy for every key.
    fn wait_persisted(&self, keys: &[CacheKey]) {
        let deadline = Instant::now() + Duration::from_secs(10);
        while !keys.iter().all(|&k| self.dm.ssd_offset(k).is_some()) {
            assert!(
                Instant::now() < deadline,
                "write-through did not land in 10 s"
            );
            std::thread::sleep(Duration::from_millis(2));
        }
    }
}

impl Fixture {
    fn removed_events(&self) -> Vec<CacheKey> {
        let mut v: Vec<CacheKey> = self
            .events
            .try_iter()
            .filter(|e| e.reason == interfaces::EvictionReason::Removed)
            .map(|e| e.key)
            .collect();
        v.sort_unstable();
        v
    }
}

/// Entries on SSD only and in DRAM with an SSD copy are all removed, their
/// extents freed, and each reported once as Removed.
#[test]
fn clear_all_empties_both_tiers() {
    let f = setup(&[]);
    // 1, 2: SSD only (stored, persisted, demoted); 3, 4: in DRAM with their
    // SSD copies (stored after the demotion, persisted). DRAM with no SSD copy
    // is the next test.
    f.store(1);
    f.store(2);
    f.wait_persisted(&[1, 2]);
    f.d.clear_memory_tier().unwrap();
    f.store(3);
    f.store(4);
    f.wait_persisted(&[3, 4]);
    assert_eq!(
        f.dm.loc(1),
        Some(Loc::Block(EXTENT)),
        "1 should be SSD-only"
    );
    assert!(f.mt.contains(3), "3 should be in DRAM");
    assert_eq!(f.extents.lock().unwrap().len(), 4, "four extents before");
    let _ = f.removed_events();

    let c = f.d.clear_all().unwrap();

    assert_eq!(
        c,
        interfaces::ClearAll {
            removed: 4,
            still_pinned: 0
        }
    );
    for k in 1..=4 {
        assert_eq!(f.dm.loc(k), None, "key {k} still in the dispatch map");
        assert!(!f.mt.contains(k), "key {k} still in the memory tier");
        assert!(!f.d.check(k).unwrap(), "key {k} still reads present");
    }
    assert!(f.extents.lock().unwrap().is_empty(), "extents not freed");
    assert_eq!(
        f.removed_events(),
        vec![1, 2, 3, 4],
        "one Removed event per entry"
    );
    f.d.shutdown().unwrap();
}

/// A DRAM-only entry (no SSD copy: its write-through failed) is removed too.
#[test]
fn clear_all_removes_an_entry_with_no_ssd_copy() {
    let f = setup(&[7]);
    f.store(7);
    let _ = f.d.flush_to_ssd(); // Err: the write failed; 7 stays DRAM-only
    assert_eq!(f.dm.ssd_offset(7), None);
    assert!(f.mt.contains(7));

    let c = f.d.clear_all().unwrap();

    assert_eq!(c.removed, 1);
    assert_eq!(f.dm.loc(7), None);
    assert!(!f.mt.contains(7));
    f.d.shutdown().unwrap();
}

/// A pinned entry stays, is reported, and goes with a second call once unpinned.
#[test]
fn clear_all_reports_a_pinned_entry_and_keeps_it() {
    let f = setup(&[]);
    f.store(1);
    f.store(2);
    f.wait_persisted(&[1, 2]);
    f.dm.take_read(2).unwrap(); // a reader holds 2 past the 5 s wait

    let c = f.d.clear_all().unwrap();

    assert_eq!(
        c,
        interfaces::ClearAll {
            removed: 1,
            still_pinned: 1
        }
    );
    assert!(
        f.mt.contains(2) || f.dm.loc(2).is_some(),
        "the pinned entry must stay"
    );
    f.dm.release_read(2).unwrap();
    assert_eq!(
        f.d.clear_all().unwrap(),
        interfaces::ClearAll {
            removed: 1,
            still_pinned: 0
        }
    );
    assert!(f.extents.lock().unwrap().is_empty());
    f.d.shutdown().unwrap();
}
