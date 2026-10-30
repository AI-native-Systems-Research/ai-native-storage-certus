//! Repair test for finding `clear-mt-frees-pinned-slot`
//! (DIS-SPIN-CLEAR-MT-KEEPS-PINNED).
//!
//! `clear_memory_tier` must not release the memory-tier slot of an entry that a
//! reader (a lookup, or the background writer's write-through pin) still holds:
//! no dispatch-map entry may ever point at a released slot. When the call
//! returns, every entry it reports as cleared is BlockDevice or removed, and
//! the count is the number of entries really cleared.
//!
//! The memory-tier mock checks the invariant at the moment a slot is freed: it
//! records a violation if the dispatch map still has a `MemoryTier` entry
//! pointing at that slot. Each scenario pins the key before the clear and
//! releases the pin from another thread a little later, as a lookup or the
//! writer would.
//!
//! Self-contained: it drives only the public `IDispatcher` API through its own
//! minimal mocks, so it builds against the unfixed code as well.

use std::collections::HashMap;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use component_core::query_interface;
use interfaces::{
    CacheKey, DispatchMapError, DispatcherConfig, DmaBuffer, GpuDeviceInfo, GpuDmaBuffer,
    GpuIpcHandle, GpuStream, IDispatchMap, IDispatcher, IGpuServices, ILogger, IMemoryTier,
    LookupResult, MemoryTierError, MemoryTierTelemetrySnapshot,
};

use crate::DispatcherComponent;

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
    fn convert_to_storage(&self, key: CacheKey, offset: u64) -> Result<(), DispatchMapError> {
        let mut entries = self.entries.lock().unwrap();
        let e = entries
            .get_mut(&key)
            .ok_or(DispatchMapError::KeyNotFound(key))?;
        match &mut e.loc {
            Loc::Memory { ssd, .. } => {
                *ssd = Some(offset);
                Ok(())
            }
            Loc::Block(_) => Err(DispatchMapError::InvalidState("block".into())),
        }
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
        e.write_ref = false;
        e.read_refs += 1;
        Ok(())
    }
    /// As `dispatch-map`'s `remove`: a pinned entry is refused.
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
        key: CacheKey,
        pointer: *mut u8,
        size: u32,
    ) -> Result<(), DispatchMapError> {
        let mut entries = self.entries.lock().unwrap();
        let e = entries
            .get_mut(&key)
            .ok_or(DispatchMapError::KeyNotFound(key))?;
        let ssd = match e.loc {
            Loc::Block(o) => Some(o),
            Loc::Memory { ssd, .. } => ssd,
        };
        e.loc = Loc::Memory {
            ptr: pointer as usize,
            size,
            ssd,
        };
        Ok(())
    }
    fn is_evictable(&self, key: CacheKey) -> bool {
        let entries = self.entries.lock().unwrap();
        entries.get(&key).is_some_and(|e| {
            e.read_refs == 0 && !e.write_ref && matches!(e.loc, Loc::Memory { ssd: Some(_), .. })
        })
    }
    /// As `dispatch-map`'s `try_evict_to_block`: a pin is `ActiveReferences`,
    /// a missing SSD copy is `InvalidState`.
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

// --- memory tier: a flat pool of 4 KiB slots that checks every free ---

/// (pool bytes, key -> (offset, size), next free offset)
type PoolState = (Vec<u8>, HashMap<CacheKey, (usize, u32)>, usize);

struct PoolMt {
    inner: Mutex<PoolState>,
    /// The dispatch map, consulted at every free.
    dm: Arc<MapDm>,
    /// Slots freed while a dispatch-map entry still pointed at them.
    freed_while_referenced: AtomicUsize,
}

impl PoolMt {
    fn new(capacity: usize, dm: Arc<MapDm>) -> Self {
        Self {
            inner: Mutex::new((vec![0u8; capacity], HashMap::new(), 0)),
            dm,
            freed_while_referenced: AtomicUsize::new(0),
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
        let slot_ptr = self.get(key).map(|(p, _)| p as usize);
        if let (Some(p), Some(Loc::Memory { ptr, .. })) = (slot_ptr, self.dm.loc(key)) {
            if p == ptr {
                self.freed_while_referenced.fetch_add(1, Ordering::SeqCst);
            }
        }
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

// --- GPU services: never used by clear_memory_tier ---

struct NoGpu;

impl IGpuServices for NoGpu {
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
        _src: *const std::ffi::c_void,
        _dst: &DmaBuffer,
        _size: usize,
    ) -> Result<(), String> {
        Err("mock".into())
    }
    fn dma_copy_to_device(
        &self,
        _src: &DmaBuffer,
        _dst: *mut std::ffi::c_void,
        _size: usize,
    ) -> Result<(), String> {
        Err("mock".into())
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
        _src: &DmaBuffer,
        _dst: *mut std::ffi::c_void,
        _size: usize,
        _s: GpuStream,
    ) -> Result<(), String> {
        Err("mock".into())
    }
    fn memcpy_h2d_async(
        &self,
        _src: *const std::ffi::c_void,
        _dst: *mut std::ffi::c_void,
        _size: usize,
        _s: GpuStream,
    ) -> Result<(), String> {
        Err("mock".into())
    }
    fn dma_copy_to_host_async(
        &self,
        _src: *const std::ffi::c_void,
        _dst: &DmaBuffer,
        _size: usize,
        _s: GpuStream,
    ) -> Result<(), String> {
        Err("mock".into())
    }
    fn memcpy_d2h_async(
        &self,
        _src: *const std::ffi::c_void,
        _dst: *mut std::ffi::c_void,
        _size: usize,
        _s: GpuStream,
    ) -> Result<(), String> {
        Err("mock".into())
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

struct QuietLogger;

impl ILogger for QuietLogger {
    fn error(&self, _msg: &str) {}
    fn warn(&self, _msg: &str) {}
    fn info(&self, _msg: &str) {}
    fn debug(&self, _msg: &str) {}
}

/// The component, its `IDispatcher`, and the mocks it is wired to.
type Fixture = (
    Arc<DispatcherComponent>,
    Arc<dyn IDispatcher + Send + Sync>,
    Arc<MapDm>,
    Arc<PoolMt>,
);

/// A memory-tier-only dispatcher wired to the mocks above.
fn setup() -> Fixture {
    let dm = Arc::new(MapDm::default());
    let mt = Arc::new(PoolMt::new(1024 * 1024, Arc::clone(&dm)));
    let c = DispatcherComponent::new_default();
    c.dispatch_map
        .connect(Arc::clone(&dm) as Arc<dyn IDispatchMap + Send + Sync>)
        .unwrap();
    c.logger.connect(Arc::new(QuietLogger)).unwrap();
    c.gpu_services.connect(Arc::new(NoGpu)).unwrap();
    c.memory_tier
        .connect(Arc::clone(&mt) as Arc<dyn IMemoryTier + Send + Sync>)
        .unwrap();
    let d = query_interface!(c, IDispatcher).unwrap();
    d.initialize(DispatcherConfig {
        data_pci_addrs: vec!["0000:02:00.0".to_string()],
        ..Default::default()
    })
    .unwrap();
    (c, d, dm, mt)
}

/// A resident memory-tier entry for `key` (slot + dispatch-map entry, no pins),
/// with an SSD copy at `ssd` if given.
fn seed_resident(dm: &MapDm, mt: &PoolMt, key: CacheKey, ssd: Option<u64>) {
    let ptr = mt.insert(key, 4096).unwrap();
    dm.create_memory_tier_entry(key, ptr, 4096).unwrap();
    dm.release_write(key).unwrap();
    if let Some(offset) = ssd {
        dm.convert_to_storage(key, offset).unwrap();
    }
}

/// Pin `key` now; from another thread, after a short delay, run `before_release`
/// (what the pin holder does with the entry) and drop the pin.
fn hold_pin_then<F>(
    d: &Arc<dyn IDispatcher + Send + Sync>,
    dm: &Arc<MapDm>,
    key: CacheKey,
    before_release: F,
) -> std::thread::JoinHandle<()>
where
    F: FnOnce(&MapDm) + Send + 'static,
{
    d.pin(key).unwrap();
    let dm2 = Arc::clone(dm);
    std::thread::spawn(move || {
        std::thread::sleep(Duration::from_millis(200));
        before_release(&dm2);
        dm2.release_read(key).unwrap();
    })
}

fn assert_cleared(dm: &MapDm, mt: &PoolMt, key: CacheKey, count: usize, case: &str) {
    assert_eq!(
        mt.freed_while_referenced.load(Ordering::SeqCst),
        0,
        "{case}: clear_memory_tier released a memory-tier slot that a dispatch-map entry still \
         points at (entry now {:?})",
        dm.loc(key)
    );
    let loc = dm.loc(key);
    assert!(
        matches!(loc, None | Some(Loc::Block(_))),
        "{case}: entry still {loc:?} when clear_memory_tier returned"
    );
    assert!(!mt.contains(key), "{case}: slot still resident after clear");
    assert_eq!(count, 1, "{case}: returned count != entries really cleared");
}

/// Lookup pin on a persisted entry: the clear must not free the slot under the
/// reader, and must still demote the entry once the reader is done.
#[test]
fn clear_memory_tier_keeps_slot_of_lookup_pinned_entry() {
    const KEY: CacheKey = 1;
    let (_c, d, dm, mt) = setup();
    seed_resident(&dm, &mt, KEY, Some(0x1000));

    let reader = hold_pin_then(&d, &dm, KEY, |_| {});
    let count = d.clear_memory_tier().unwrap();
    reader.join().unwrap();

    assert_cleared(&dm, &mt, KEY, count, "lookup pin");
    assert_eq!(
        dm.loc(KEY),
        Some(Loc::Block(0x1000)),
        "persisted entry must be demoted"
    );
    d.shutdown().unwrap();
}

/// Writer pin on an entry whose write-through has not landed yet: the writer
/// still reads the slot, then records the SSD offset and drops its pin. The
/// clear must keep the slot until then, and the entry ends up demoted.
#[test]
fn clear_memory_tier_keeps_slot_of_writer_pinned_entry() {
    const KEY: CacheKey = 2;
    let (_c, d, dm, mt) = setup();
    seed_resident(&dm, &mt, KEY, None);

    let writer = hold_pin_then(&d, &dm, KEY, |dm| {
        dm.convert_to_storage(KEY, 0x2000).unwrap();
    });
    let count = d.clear_memory_tier().unwrap();
    writer.join().unwrap();

    assert_cleared(&dm, &mt, KEY, count, "writer pin");
    assert_eq!(
        dm.loc(KEY),
        Some(Loc::Block(0x2000)),
        "persisted entry must be demoted"
    );
    d.shutdown().unwrap();
}

/// Pin on an unpersisted entry that stays unpersisted: once the pin is gone it
/// is removed from both tiers, never before.
#[test]
fn clear_memory_tier_removes_unpersisted_entry_only_after_pin_drops() {
    const KEY: CacheKey = 3;
    let (_c, d, dm, mt) = setup();
    seed_resident(&dm, &mt, KEY, None);

    let reader = hold_pin_then(&d, &dm, KEY, |_| {});
    let count = d.clear_memory_tier().unwrap();
    reader.join().unwrap();

    assert_cleared(&dm, &mt, KEY, count, "unpersisted pin");
    assert_eq!(dm.loc(KEY), None, "unpersisted entry must be removed");
    d.shutdown().unwrap();
}
