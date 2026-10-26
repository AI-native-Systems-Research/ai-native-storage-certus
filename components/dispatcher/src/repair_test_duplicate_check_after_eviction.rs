//! Repair test for finding `duplicate-check-after-eviction`
//! (DIS-POPULATE-DUPLICATE-ALREADY-EXISTS, DIS-RESERVE-MEMORY-ALREADY-EXISTS).
//!
//! Storing a key that is already cached must be refused with `AlreadyExists`
//! immediately, leaving the existing entry — its dispatch-map location, its
//! memory-tier slot, its data and its SSD offset — unchanged. The only
//! duplicate check on the store path was `mt.insert` inside `evict_and_insert`,
//! which runs *after* `evict_for_space`: on a full tier the duplicate's own
//! entry could be demoted to make room for its replacement (the insert then
//! succeeds and the entry is destroyed), or, when nothing is evictable, the
//! refusal surfaced as `AllocationFailed` after the full backpressure budget.
//!
//! Self-contained: it drives only the public `IDispatcher` API through its own
//! minimal mocks, so it builds against the unfixed code as well.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use component_core::query_interface;
use interfaces::{
    CacheKey, DispatchMapError, DispatcherConfig, DispatcherError, DmaBuffer, GpuDeviceInfo,
    GpuDmaBuffer, GpuIpcHandle, GpuStream, IDispatchMap, IDispatcher, IGpuServices, ILogger,
    IMemoryTier, IpcHandle, LookupResult, MemoryTierError, MemoryTierTelemetrySnapshot,
};

use crate::DispatcherComponent;

// --- dispatch map: a plain map of entries with reference counts ---

#[derive(Clone, Copy, Debug)]
enum Loc {
    Block(u64),
    // Pointer stored as usize so the mock stays Send + Sync without an unsafe impl.
    Memory(usize, u32),
}

#[derive(Clone, Copy)]
struct Ent {
    loc: Loc,
    read_refs: u32,
    write_ref: bool,
    /// The entry's write-through SSD offset. `Some` = persisted, so the entry
    /// is demotable (MemoryTier -> BlockDevice); `None` = write-through has
    /// not landed, so `try_evict_to_block` must refuse it.
    ssd: Option<u64>,
}

#[derive(Default)]
struct MapDm {
    entries: Mutex<HashMap<CacheKey, Ent>>,
}

impl MapDm {
    fn seed_memory(&self, key: CacheKey, pointer: usize, size: u32, ssd: Option<u64>) {
        self.entries.lock().unwrap().insert(
            key,
            Ent {
                loc: Loc::Memory(pointer, size),
                read_refs: 0,
                write_ref: false,
                ssd,
            },
        );
    }

    fn entry(&self, key: CacheKey) -> Option<Ent> {
        self.entries.lock().unwrap().get(&key).copied()
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
        e.read_refs += 1;
        Ok(match e.loc {
            Loc::Block(offset) => LookupResult::BlockDevice { offset },
            Loc::Memory(p, size) => LookupResult::MemoryTier {
                pointer: p as *mut u8,
                size,
            },
        })
    }
    fn convert_to_storage(&self, key: CacheKey, offset: u64) -> Result<(), DispatchMapError> {
        let mut entries = self.entries.lock().unwrap();
        let e = entries
            .get_mut(&key)
            .ok_or(DispatchMapError::KeyNotFound(key))?;
        e.ssd = Some(offset);
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
        e.read_refs = e.read_refs.saturating_sub(1);
        Ok(())
    }
    fn release_write(&self, key: CacheKey) -> Result<(), DispatchMapError> {
        let mut entries = self.entries.lock().unwrap();
        let e = entries
            .get_mut(&key)
            .ok_or(DispatchMapError::KeyNotFound(key))?;
        e.write_ref = false;
        Ok(())
    }
    fn downgrade_reference(&self, key: CacheKey) -> Result<(), DispatchMapError> {
        let mut entries = self.entries.lock().unwrap();
        let e = entries
            .get_mut(&key)
            .ok_or(DispatchMapError::NoWriteReference(key))?;
        e.write_ref = false;
        e.read_refs += 1;
        Ok(())
    }
    fn remove(&self, key: CacheKey) -> Result<(), DispatchMapError> {
        self.entries
            .lock()
            .unwrap()
            .remove(&key)
            .map(|_| ())
            .ok_or(DispatchMapError::KeyNotFound(key))
    }
    fn touch(&self, key: CacheKey) -> Result<(), DispatchMapError> {
        if self.entries.lock().unwrap().contains_key(&key) {
            Ok(())
        } else {
            Err(DispatchMapError::KeyNotFound(key))
        }
    }
    fn entry_size(&self, key: CacheKey) -> Result<u32, DispatchMapError> {
        self.touch(key).map(|_| 4096)
    }
    fn oldest_keys(&self, n: usize) -> Vec<CacheKey> {
        self.entries
            .lock()
            .unwrap()
            .keys()
            .copied()
            .take(n)
            .collect()
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
                loc: Loc::Memory(pointer as usize, size),
                read_refs: 0,
                write_ref: true,
                ssd: None,
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
        e.loc = Loc::Memory(pointer as usize, size);
        Ok(())
    }
    fn is_evictable(&self, key: CacheKey) -> bool {
        let entries = self.entries.lock().unwrap();
        matches!(
            entries.get(&key),
            Some(Ent {
                ssd: Some(_),
                read_refs: 0,
                ..
            })
        )
    }
    fn try_evict_to_block(&self, key: CacheKey) -> Result<(), DispatchMapError> {
        let mut entries = self.entries.lock().unwrap();
        let e = entries
            .get_mut(&key)
            .ok_or(DispatchMapError::KeyNotFound(key))?;
        // As in the real map: a resident entry demotes only once its
        // write-through has landed (ssd_offset recorded) and no reader holds it.
        let Some(offset) = e.ssd else {
            return Err(DispatchMapError::InvalidState("no ssd_offset".into()));
        };
        if e.read_refs > 0 {
            return Err(DispatchMapError::ActiveReferences(key));
        }
        e.loc = Loc::Block(offset);
        Ok(())
    }
    fn recover_extent(
        &self,
        _key: CacheKey,
        _offset: u64,
        _size_blocks: u32,
    ) -> Result<(), DispatchMapError> {
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

// --- memory tier: a flat pool of 4 KiB-aligned bump-allocated slots ---

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

    /// The bytes currently stored in `key`'s slot.
    fn slot_data(&self, key: CacheKey) -> Option<Vec<u8>> {
        let g = self.inner.lock().unwrap();
        g.1.get(&key)
            .map(|&(off, size)| g.0[off..off + size as usize].to_vec())
    }

    /// Fill `key`'s slot with a 0x11 pattern (SAFETY: the slot is `size` live
    /// bytes inside the pool, freshly inserted above).
    fn fill(&self, key: CacheKey) {
        let g = self.inner.lock().unwrap();
        let &(off, size) = g.1.get(&key).unwrap();
        // SAFETY: `off + size <= pool.len()` was checked at insert.
        unsafe {
            std::ptr::copy_nonoverlapping(
                vec![0x11u8; size as usize].as_ptr(),
                (g.0.as_ptr() as *mut u8).add(off),
                size as usize,
            )
        };
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
        let aligned = (size as usize).next_multiple_of(4096);
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
        self.inner
            .lock()
            .unwrap()
            .1
            .keys()
            .copied()
            .take(n)
            .collect()
    }
    fn remove(&self, key: CacheKey) -> Result<(), MemoryTierError> {
        // Frees the slot only when it is the top of the bump allocation (every
        // scenario here removes the most recently inserted slot); holes below
        // the top are leaked, which no scenario allocates across.
        let mut g = self.inner.lock().unwrap();
        let (pool, slots, used) = &mut *g;
        let (off, size) = slots
            .remove(&key)
            .ok_or(MemoryTierError::KeyNotFound(key))?;
        let aligned = (size as usize).next_multiple_of(4096);
        if *used == off + aligned {
            *used = off;
        }
        let _ = pool;
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

// --- GPU services: host memcpy stands in for every DMA ---

fn host_copy(src: *const u8, dst: *mut u8, size: usize) -> Result<(), String> {
    // SAFETY: in this test every src/dst is a live host buffer of at least `size` bytes.
    unsafe { std::ptr::copy_nonoverlapping(src, dst, size) };
    Ok(())
}

struct HostGpu;

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

struct QuietLogger;

impl ILogger for QuietLogger {
    fn error(&self, _msg: &str) {}
    fn warn(&self, _msg: &str) {}
    fn info(&self, _msg: &str) {}
    fn debug(&self, _msg: &str) {}
}

/// A dispatcher wired to the mocks above, with a `capacity`-byte memory tier
/// and the given `backpressure_ms` store budget.
fn setup(
    capacity: usize,
    backpressure_ms: u64,
) -> (Arc<dyn IDispatcher + Send + Sync>, Arc<MapDm>, Arc<PoolMt>) {
    let dm = Arc::new(MapDm::default());
    let mt = Arc::new(PoolMt::new(capacity));
    let c = DispatcherComponent::new_default();
    c.dispatch_map
        .connect(Arc::clone(&dm) as Arc<dyn IDispatchMap + Send + Sync>)
        .unwrap();
    c.logger.connect(Arc::new(QuietLogger)).unwrap();
    c.gpu_services
        .connect(Arc::new(HostGpu) as Arc<dyn IGpuServices + Send + Sync>)
        .unwrap();
    c.memory_tier
        .connect(Arc::clone(&mt) as Arc<dyn IMemoryTier + Send + Sync>)
        .unwrap();
    let d = query_interface!(c, IDispatcher).unwrap();
    d.initialize(DispatcherConfig {
        data_pci_addrs: vec!["0000:02:00.0".to_string()],
        store_backpressure_ms: backpressure_ms,
        ..Default::default()
    })
    .unwrap();
    (d, dm, mt)
}

/// Seed `key` as a resident entry with a 0x11 data pattern in its slot and
/// the given write-through state. Returns the slot pointer.
fn seed_resident(mt: &PoolMt, dm: &MapDm, key: CacheKey, ssd: Option<u64>) -> *mut u8 {
    let ptr = mt.insert(key, 4096).expect("seed insert");
    mt.fill(key);
    dm.seed_memory(key, ptr as usize, 4096, ssd);
    ptr
}

/// The existing entry must be exactly what `seed_resident` left: MemoryTier
/// at the same slot, same data, SSD offset intact, no references.
fn assert_entry_unchanged(dm: &MapDm, mt: &PoolMt, key: CacheKey, slot: *mut u8, what: &str) {
    let e = dm
        .entry(key)
        .unwrap_or_else(|| panic!("{what}: entry {key} vanished"));
    assert!(
        matches!(e.loc, Loc::Memory(p, 4096) if p == slot as usize),
        "{what}: entry {key} no longer MemoryTier at its slot: {:?}",
        e.loc
    );
    assert_eq!(e.read_refs, 0, "{what}: entry {key} references changed");
    assert_eq!(
        e.ssd,
        Some(0x1234),
        "{what}: entry {key} SSD offset changed"
    );
    assert!(
        mt.contains(key),
        "{what}: entry {key} lost its memory-tier slot"
    );
    assert_eq!(
        mt.slot_data(key),
        Some(vec![0x11u8; 4096]),
        "{what}: entry {key} slot data was overwritten"
    );
}

#[test]
fn reserve_memory_duplicate_on_full_tier_refuses_already_exists() {
    // The tier holds exactly the duplicate's own entry, and that entry is the
    // only — demotable — eviction candidate. The refusal must be
    // AlreadyExists, not a successful re-reservation bought by demoting the
    // entry out from underneath itself.
    let (d, dm, mt) = setup(4096, 5000);
    let slot = seed_resident(&mt, &dm, 7, Some(0x1234));

    let r = d.reserve_memory(7, 4096, 0, None);
    assert!(
        matches!(r, Err(DispatcherError::AlreadyExists(7))),
        "reserve_memory for a key that already has a slot must refuse with AlreadyExists, got {r:?}"
    );
    assert_entry_unchanged(&dm, &mt, 7, slot, "duplicate reserve");
    let stats = d.tier_event_stats();
    assert_eq!(
        stats.store_already_resident, 1,
        "the duplicate refusal must be counted as already-resident"
    );
    assert_eq!(
        stats.store_backpressure_events, 0,
        "the duplicate refusal must not go through backpressure retries"
    );

    d.shutdown().unwrap();
}

#[test]
fn populate_duplicate_on_full_tier_leaves_entry_unchanged() {
    // Same tier state, through populate: the duplicate may not be bought by
    // demoting the existing entry to BlockDevice and re-inserting a fresh
    // slot (which the dispatch-map registration then refuses, after the
    // damage is done).
    let (d, dm, mt) = setup(4096, 5000);
    let slot = seed_resident(&mt, &dm, 7, Some(0x1234));
    let mut src = vec![0xEEu8; 4096];
    let h = IpcHandle {
        address: src.as_mut_ptr(),
        size: 4096,
    };

    let r = d.populate(7, h);
    assert!(
        matches!(r, Err(DispatcherError::AlreadyExists(7))),
        "populate of an already-cached key must refuse with AlreadyExists, got {r:?}"
    );
    assert_entry_unchanged(&dm, &mt, 7, slot, "duplicate populate");

    d.shutdown().unwrap();
}

#[test]
fn reserve_memory_duplicate_when_nothing_evictable_is_immediate() {
    // The duplicate's entry has no SSD offset yet (write-through in flight),
    // so no victim is evictable and the tier is full. The refusal must still
    // be an immediate AlreadyExists — not AllocationFailed after burning the
    // whole backpressure budget retrying.
    let (d, dm, mt) = setup(4096, 300);
    let slot = seed_resident(&mt, &dm, 7, None);

    let t0 = std::time::Instant::now();
    let r = d.reserve_memory(7, 4096, 0, None);
    let elapsed = t0.elapsed();
    assert!(
        matches!(r, Err(DispatcherError::AlreadyExists(7))),
        "reserve_memory for a key that already has a slot must refuse with AlreadyExists, got {r:?}"
    );
    assert!(
        elapsed < std::time::Duration::from_millis(300),
        "the duplicate refusal must be immediate, took {elapsed:?}"
    );
    let stats = d.tier_event_stats();
    assert_eq!(
        stats.store_backpressure_events, 0,
        "the duplicate refusal must not go through backpressure retries"
    );
    let e = dm.entry(7).expect("entry");
    assert!(
        matches!(e.loc, Loc::Memory(p, 4096) if p == slot as usize),
        "the refusal changed the entry: {:?}",
        e.loc
    );
    assert!(mt.contains(7), "the refusal freed the memory-tier slot");

    d.shutdown().unwrap();
}

#[test]
fn reserve_memory_duplicate_with_room_to_spare_still_refuses() {
    // Regression guard for the case that already worked: with room in the
    // tier the refusal comes from mt.insert. It must stay an immediate
    // AlreadyExists and must not evict anyone else on the way.
    let (d, dm, mt) = setup(1024 * 1024, 5000);
    let slot = seed_resident(&mt, &dm, 7, Some(0x1234));
    let other = seed_resident(&mt, &dm, 8, Some(0x5678));

    let r = d.reserve_memory(7, 4096, 0, None);
    assert!(
        matches!(r, Err(DispatcherError::AlreadyExists(7))),
        "reserve_memory for a key that already has a slot must refuse with AlreadyExists, got {r:?}"
    );
    assert_entry_unchanged(&dm, &mt, 7, slot, "duplicate reserve, room to spare");
    assert!(
        matches!(dm.entry(8).unwrap().loc, Loc::Memory(p, 4096) if p == other as usize),
        "the duplicate refusal evicted another entry"
    );

    d.shutdown().unwrap();
}
