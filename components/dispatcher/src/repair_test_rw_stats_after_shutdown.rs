//! Repair test for finding `rw-stats-decrease-after-shutdown`
//! (DIS-READ-WRITE-STATS-MONOTONIC, FR-056).
//!
//! `IDispatcher::read_write_stats` promises *cumulative* per-direction SSD
//! counters that are monotonic for the life of the component (spec: "take
//! deltas across two calls to measure a window"). But `shutdown()` tears the
//! data drives down (`std::mem::take` of `data_drives`), and `read_write_stats`
//! aggregates only over the drives still in that list — so the first call
//! after shutdown reports all zeros, a decrease from any pre-shutdown value.
//!
//! Self-contained (its own mocks, the public `IDispatcher` API plus the
//! crate-internal `DataDrive` slot, exactly like the other drive-fixture
//! repair tests), so it builds against the dispatcher as it was before the
//! repair as well as after it.

use std::collections::{BTreeSet, HashMap};
use std::sync::{Arc, Mutex};

use component_core::channel::SpscChannel;
use component_core::query_interface;
use interfaces::{
    CacheKey, ClientChannels, Command, Completion, DispatchMapError, DispatcherConfig, Extent,
    ExtentManagerError, FormatParams, IBlockDevice, IDispatchMap, IDispatcher, IExtentManager,
    IMemoryTier, LookupResult, MemoryTierError, MemoryTierTelemetrySnapshot, NvmeBlockError,
    OpHandle, TelemetrySnapshot, WriteHandle,
};

use crate::{ChannelPool, DataDrive, DispatcherComponent};

const BLOCK: u32 = 4096;

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
}

impl Dm {
    fn new() -> Self {
        Self {
            entries: Mutex::new(HashMap::new()),
        }
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
        Ok(match e.loc {
            Loc::Block(offset) => LookupResult::BlockDevice { offset },
            Loc::Memory { ptr, size, .. } => LookupResult::MemoryTier {
                pointer: ptr as *mut u8,
                size,
            },
        })
    }
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
    fn promote_block_to_memory_tier(
        &self,
        _key: CacheKey,
        _pointer: *mut u8,
        _size: u32,
    ) -> Result<(), DispatchMapError> {
        Err(DispatchMapError::InvalidState("not modelled".into()))
    }
    fn is_evictable(&self, key: CacheKey) -> bool {
        self.entries
            .lock()
            .unwrap()
            .get(&key)
            .is_some_and(|e| e.read_refs == 0 && !e.write_ref)
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

// --- memory tier: a flat pool of 4 KiB slots (nothing is stored; shapes only) ---

struct PoolInner {
    pool: Vec<u8>,
    slots: HashMap<CacheKey, (usize, u32)>,
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
    fn telemetry_snapshot(&self) -> MemoryTierTelemetrySnapshot {
        MemoryTierTelemetrySnapshot::default()
    }
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
    fn set_bounce_reserve(&self, _: usize) -> Result<(), MemoryTierError> {
        Ok(())
    }
    fn bounce_alloc(&self, _: u32) -> Result<interfaces::BounceBuffer, interfaces::BounceError> {
        Err(interfaces::BounceError::NeverFits(
            "no bounce reserve".into(),
        ))
    }
    fn bounce_free(&self, _: interfaces::BounceBuffer) {}
    fn bounce_usage(&self) -> (usize, usize) {
        (0, 0)
    }
}

// --- NVMe block device: a telemetry-enabled drive that has already done I/O ---

#[derive(Default)]
struct MockNvme {
    _disk: Mutex<()>,
}

impl IBlockDevice for MockNvme {
    fn connect_client(&self) -> Result<ClientChannels, NvmeBlockError> {
        let cmd = SpscChannel::<Command>::new(64);
        let comp = SpscChannel::<Completion>::new(64);
        let (cmd_tx, cmd_rx) = (cmd.sender().unwrap(), cmd.receiver().unwrap());
        let (comp_tx, comp_rx) = (comp.sender().unwrap(), comp.receiver().unwrap());
        std::thread::spawn(move || {
            while let Ok(c) = cmd_rx.recv() {
                let done = match c {
                    Command::WriteSync { .. } => Completion::WriteDone {
                        handle: OpHandle(1),
                        tag: 0,
                        result: Ok(()),
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
    /// A drive built with telemetry enabled that has already served traffic:
    /// every counter and histogram bucket the dispatcher aggregates is
    /// non-zero, so any drop in the aggregate is observable.
    fn read_write_stats(&self) -> interfaces::ReadWriteStats {
        let mut s = interfaces::ReadWriteStats::default();
        s.read_ops = 2;
        s.read_bytes = 2 * BLOCK as u64;
        s.read_latency_ns_sum = 1_000;
        s.read_size_buckets[13] = 2;
        s.write_ops = 3;
        s.write_bytes = 3 * BLOCK as u64;
        s.write_latency_ns_sum = 2_000;
        s.write_size_buckets[13] = 3;
        s
    }
}

// --- extent manager: nothing is ever allocated ---

#[derive(Default)]
struct MockEm {
    _inner: Mutex<()>,
}

impl IExtentManager for MockEm {
    fn format(&self, _params: FormatParams) -> Result<(), ExtentManagerError> {
        Ok(())
    }
    fn initialize(&self) -> Result<(), ExtentManagerError> {
        Ok(())
    }
    fn reserve_extent(
        &self,
        _key: CacheKey,
        _size: u32,
    ) -> Result<WriteHandle, ExtentManagerError> {
        Err(ExtentManagerError::OutOfSpace)
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
    fn set_metadata_base_lba(&self, _base: u64) {}
    fn set_data_base_lba(&self, _base: u64) {}
    fn data_base_lba(&self) -> u64 {
        0
    }
}

// --- fixtures ---

/// A dispatcher with one mock data drive whose block device reports non-zero
/// read/write counters (a telemetry-enabled drive after some traffic).
fn setup_with_drive() -> Arc<dyn IDispatcher + Send + Sync> {
    let dm = Arc::new(Dm::new());
    let mt = Arc::new(PoolMt::new(1024 * 1024));
    let nvme = Arc::new(MockNvme::default());
    let ibd: Arc<dyn IBlockDevice + Send + Sync> = nvme;
    let em = Arc::new(MockEm::default());
    let iem: Arc<dyn IExtentManager + Send + Sync> = em;

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
    // `data_drives` as it finds it, so the dispatcher serves this drive.
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
    d
}

/// Every counter of `s` must be >= the same counter of `prev` (monotonic).
fn assert_monotonic(prev: &interfaces::ReadWriteStats, now: &interfaces::ReadWriteStats) {
    assert!(
        now.read_ops >= prev.read_ops,
        "read_ops decreased: {} -> {}",
        prev.read_ops,
        now.read_ops
    );
    assert!(
        now.read_bytes >= prev.read_bytes,
        "read_bytes decreased: {} -> {}",
        prev.read_bytes,
        now.read_bytes
    );
    assert!(
        now.read_latency_ns_sum >= prev.read_latency_ns_sum,
        "read_latency_ns_sum decreased: {} -> {}",
        prev.read_latency_ns_sum,
        now.read_latency_ns_sum
    );
    assert!(
        now.write_ops >= prev.write_ops,
        "write_ops decreased: {} -> {}",
        prev.write_ops,
        now.write_ops
    );
    assert!(
        now.write_bytes >= prev.write_bytes,
        "write_bytes decreased: {} -> {}",
        prev.write_bytes,
        now.write_bytes
    );
    assert!(
        now.write_latency_ns_sum >= prev.write_latency_ns_sum,
        "write_latency_ns_sum decreased: {} -> {}",
        prev.write_latency_ns_sum,
        now.write_latency_ns_sum
    );
    for i in 0..interfaces::IO_SIZE_BUCKETS {
        assert!(
            now.read_size_buckets[i] >= prev.read_size_buckets[i],
            "read_size_buckets[{i}] decreased: {} -> {}",
            prev.read_size_buckets[i],
            now.read_size_buckets[i]
        );
        assert!(
            now.write_size_buckets[i] >= prev.write_size_buckets[i],
            "write_size_buckets[{i}] decreased: {} -> {}",
            prev.write_size_buckets[i],
            now.write_size_buckets[i]
        );
    }
}

// --- tests ---

/// DIS-READ-WRITE-STATS-MONOTONIC: the read/write statistics never decrease
/// between two successive calls — in particular not across `shutdown`, which
/// tears the data drives down. The spec (FR-056) calls the counters cumulative
/// and monotonic for the life of the dispatcher, so a monitoring caller that
/// samples them before and after shutdown must never see them go down.
#[test]
fn rw_stats_never_decrease_across_shutdown() {
    let d = setup_with_drive();

    let first = d.read_write_stats();
    assert!(
        first.read_ops > 0 && first.write_ops > 0,
        "precondition: the drive reports traffic (got {first:?})"
    );

    // Two successive calls in normal operation agree (monotonic, nothing lost).
    let second = d.read_write_stats();
    assert_monotonic(&first, &second);

    d.shutdown().unwrap();

    // The drives are gone after shutdown, but the counters they had
    // accumulated must not be: the aggregate has to stay at least at its
    // pre-shutdown value, not fall back to zero.
    let after = d.read_write_stats();
    assert_monotonic(&second, &after);

    // And the post-shutdown value is stable, not a one-shot fluke.
    let again = d.read_write_stats();
    assert_monotonic(&after, &again);
    assert_eq!(after, again);
}
