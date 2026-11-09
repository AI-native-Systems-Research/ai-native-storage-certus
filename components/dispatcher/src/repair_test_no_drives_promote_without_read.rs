//! Repair test for finding `no-drives-promote-without-read`
//! (DIS-BATCH-LOOKUP-COLD-PROMOTES, DIS-LOOKUP-NO-HW-PROMOTE).
//!
//! In memory-tier-only mode (no ISPDKEnv bound, so `data_drives` is empty) a
//! cold serve must run the same serve-then-promote sequence the with-drives
//! cold path runs: reserve a memory-tier slot, deliver it to the entry's GPU
//! destination, and only then re-register the dispatch-map entry as a
//! memory-tier entry (the in-place `promote_block_to_memory_tier` flip).
//!
//! The unfixed code skips steps of that sequence: `batch_lookup`'s no-drives
//! arm never delivers to the GPU destination at all, and `promote_and_serve`'s
//! no-drives arm re-registers with `create_memory_tier_entry` — which can only
//! answer `AlreadyExists` for a key that already exists as BlockDevice — so the
//! entry is never re-registered, the fresh slot is stranded, and a foreign
//! `release_write` runs. On a failed GPU delivery neither path released the
//! reserved slot.
//!
//! Self-contained: it drives only the public `IDispatcher` API through its own
//! minimal mocks, so it builds against the unfixed code as well.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use component_core::query_interface;
use interfaces::{
    CacheKey, DispatchMapError, DispatcherConfig, DmaBuffer, GpuDeviceInfo, GpuDmaBuffer,
    GpuIpcHandle, GpuStream, IDispatchMap, IDispatcher, IGpuServices, ILogger, IMemoryTier,
    IpcHandle, LookupResult, MemoryTierError, MemoryTierTelemetrySnapshot,
};

use crate::DispatcherComponent;

const KEY: CacheKey = 1;
const BLOCK: u32 = 4096;

/// The byte a fresh pool slot holds at byte `off` of its 4 KiB extent: the
/// first half of every slot is `0x5A`, the second half `0xA5`, so a delivery
/// assertion also pins down *which* slot bytes each GPU region received.
fn slot_byte(off: usize) -> u8 {
    if off % 4096 < 2048 {
        0x5A
    } else {
        0xA5
    }
}

/// The bytes a correct delivery leaves in a destination that reads the slot
/// from `off` for `len` bytes.
fn expected(off: usize, len: usize) -> Vec<u8> {
    (off..off + len).map(slot_byte).collect()
}

// --- dispatch map: entries with reference counts, as the real one enforces them ---

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Loc {
    Block(u64),
    // Pointer stored as usize so the mock stays Send + Sync without an unsafe impl.
    Memory(usize, u32),
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
    /// The entry's current location, as the test observes it.
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
            Loc::Memory(p, size) => LookupResult::MemoryTier {
                pointer: p as *mut u8,
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
        self.touch(key).map(|_| BLOCK)
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
    /// As the real dispatch map: refuses an existing key, takes a write ref.
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
            },
        );
        Ok(())
    }
    fn convert_memory_tier_to_block(&self, _key: CacheKey) -> Result<(), DispatchMapError> {
        Err(DispatchMapError::InvalidState("not modelled".into()))
    }
    /// As the real dispatch map: in-place flip, preserving refs.
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

// --- memory tier: a flat pool of 4 KiB-aligned slots, pattern-filled ---

/// (pool bytes, key -> (offset, size), bytes used)
type PoolState = (Vec<u8>, HashMap<CacheKey, (usize, u32)>, usize);

struct PoolMt {
    inner: Mutex<PoolState>,
}

impl PoolMt {
    /// A pool whose every fresh slot already holds the `slot_byte` pattern.
    fn new(capacity: usize) -> Self {
        let pool = (0..capacity).map(slot_byte).collect();
        Self {
            inner: Mutex::new((pool, HashMap::new(), 0)),
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

// --- GPU services: host memcpy, with an injectable delivery failure ---

#[derive(Default)]
struct HookGpu {
    /// While set, every host -> device copy fails (the delivery error path).
    fail_copies: Mutex<bool>,
}

fn host_copy(src: *const u8, dst: *mut u8, size: usize) -> Result<(), String> {
    // SAFETY: in this test every src/dst is a live host buffer of at least `size` bytes.
    unsafe { std::ptr::copy_nonoverlapping(src, dst, size) };
    Ok(())
}

impl HookGpu {
    fn copying_fails(&self) -> bool {
        *self.fail_copies.lock().unwrap()
    }
}

impl IGpuServices for HookGpu {
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
        if self.copying_fails() {
            return Err("injected GPU copy failure".into());
        }
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
        // No GPU owns the host buffers: the synchronous DmaBuffer path is used.
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
        src: &DmaBuffer,
        dst: *mut std::ffi::c_void,
        size: usize,
        _s: GpuStream,
    ) -> Result<(), String> {
        if self.copying_fails() {
            return Err("injected GPU copy failure".into());
        }
        host_copy(src.as_ptr() as *const u8, dst as *mut u8, size)
    }
    fn memcpy_h2d_async(
        &self,
        src: *const std::ffi::c_void,
        dst: *mut std::ffi::c_void,
        size: usize,
        _s: GpuStream,
    ) -> Result<(), String> {
        if self.copying_fails() {
            return Err("injected GPU copy failure".into());
        }
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
    Arc<HookGpu>,
);

/// A memory-tier-only dispatcher wired to the mocks above.
fn setup() -> Fixture {
    let dm = Arc::new(MapDm::default());
    let mt = Arc::new(PoolMt::new(1024 * 1024));
    let gpu = Arc::new(HookGpu::default());
    let c = DispatcherComponent::new_default();
    c.dispatch_map
        .connect(Arc::clone(&dm) as Arc<dyn IDispatchMap + Send + Sync>)
        .unwrap();
    c.logger.connect(Arc::new(QuietLogger)).unwrap();
    c.gpu_services
        .connect(Arc::clone(&gpu) as Arc<dyn IGpuServices + Send + Sync>)
        .unwrap();
    c.memory_tier
        .connect(Arc::clone(&mt) as Arc<dyn IMemoryTier + Send + Sync>)
        .unwrap();
    let d = query_interface!(c, IDispatcher).unwrap();
    d.initialize(DispatcherConfig {
        data_pci_addrs: vec!["0000:02:00.0".to_string()],
        ..Default::default()
    })
    .unwrap();
    (c, d, dm, mt, gpu)
}

/// An SSD-only (BlockDevice) entry, as a demotion or recovery leaves it.
fn seed_cold(dm: &MapDm, key: CacheKey, offset: u64) {
    dm.recover_extent(key, offset, 1).unwrap();
    assert_eq!(dm.loc(key), Some(Loc::Block(offset)));
}

/// DIS-LOOKUP-NO-HW-PROMOTE: in memory-tier-only mode a cold `lookup` copies
/// the promoted slot to the GPU destination and re-registers the entry as a
/// memory-tier entry pointing at that slot.
#[test]
fn cold_lookup_copies_and_registers_without_hardware() {
    let (_c, d, dm, mt, _gpu) = setup();
    seed_cold(&dm, KEY, 0x1000);

    let mut dst = vec![0xEEu8; BLOCK as usize];
    let h = IpcHandle {
        address: dst.as_mut_ptr(),
        size: BLOCK,
    };
    let r = d.lookup(KEY, h);

    assert!(r.is_ok(), "the cold lookup must succeed, got {r:?}");
    let (slot_ptr, slot_size) = mt.get(KEY).expect("the promoted key must have a tier slot");
    assert_eq!(slot_size, BLOCK);
    assert_eq!(
        dst,
        expected(0, BLOCK as usize),
        "the GPU destination must hold the promoted slot's bytes"
    );
    assert_eq!(
        dm.loc(KEY),
        Some(Loc::Memory(slot_ptr as usize, BLOCK)),
        "the entry must be re-registered as a memory-tier entry pointing at the slot"
    );
    d.shutdown().unwrap();
}

/// DIS-LOOKUP-NO-HW-PROMOTE (failure exit): a GPU delivery that fails must
/// release the reserved slot and leave the key SSD resident, not strand the
/// slot as dead tier capacity.
#[test]
fn cold_lookup_releases_slot_when_gpu_copy_fails() {
    let (_c, d, dm, mt, gpu) = setup();
    seed_cold(&dm, KEY, 0x1000);
    *gpu.fail_copies.lock().unwrap() = true;

    let mut dst = vec![0u8; BLOCK as usize];
    let h = IpcHandle {
        address: dst.as_mut_ptr(),
        size: BLOCK,
    };
    let r = d.lookup(KEY, h);

    assert!(
        r.is_err(),
        "the failed GPU delivery must fail the lookup, got {r:?}"
    );
    assert!(
        !mt.contains(KEY),
        "the reserved slot must be released when the delivery fails"
    );
    assert_eq!(
        dm.loc(KEY),
        Some(Loc::Block(0x1000)),
        "the key must stay SSD resident (BlockDevice)"
    );
    d.shutdown().unwrap();
}

/// DIS-BATCH-LOOKUP-COLD-PROMOTES: a cold batch entry served in
/// memory-tier-only mode is delivered to its GPU destination and afterwards
/// registered as a memory-tier entry.
#[test]
fn batch_lookup_cold_copies_and_registers_without_hardware() {
    let (_c, d, dm, mt, _gpu) = setup();
    seed_cold(&dm, KEY, 0x2000);

    let mut dst = vec![0xEEu8; BLOCK as usize];
    let h = IpcHandle {
        address: dst.as_mut_ptr(),
        size: BLOCK,
    };
    let out = d.batch_lookup(&[(KEY, vec![h])]);

    assert_eq!(out.len(), 1);
    assert!(
        out[0].result.is_ok(),
        "the cold batch entry must be served, got {:?}",
        out[0].result
    );
    let (slot_ptr, slot_size) = mt.get(KEY).expect("the promoted key must have a tier slot");
    assert_eq!(slot_size, BLOCK);
    assert_eq!(
        dst,
        expected(0, BLOCK as usize),
        "the entry must be delivered to its GPU destination"
    );
    assert_eq!(
        dm.loc(KEY),
        Some(Loc::Memory(slot_ptr as usize, BLOCK)),
        "the entry must be registered as a memory-tier entry pointing at the slot"
    );
    d.shutdown().unwrap();
}

/// DIS-BATCH-LOOKUP-COLD-PROMOTES, multi-region shape: the slot is scattered
/// to the entry's N GPU regions, region L reading from `slot + sum(sizes[..L])`.
#[test]
fn batch_lookup_cold_scatters_regions_without_hardware() {
    let (_c, d, dm, _mt, _gpu) = setup();
    seed_cold(&dm, KEY, 0x3000);

    let mut dst1 = vec![0xEEu8; 2048];
    let mut dst2 = vec![0xDDu8; 2048];
    let entries = [(
        KEY,
        vec![
            IpcHandle {
                address: dst1.as_mut_ptr(),
                size: 2048,
            },
            IpcHandle {
                address: dst2.as_mut_ptr(),
                size: 2048,
            },
        ],
    )];
    let out = d.batch_lookup(&entries);

    assert_eq!(out.len(), 1);
    assert!(
        out[0].result.is_ok(),
        "the cold batch entry must be served, got {:?}",
        out[0].result
    );
    assert_eq!(
        dst1,
        expected(0, 2048),
        "region 0 must receive the first half of the slot"
    );
    assert_eq!(
        dst2,
        expected(2048, 2048),
        "region 1 must receive the second half of the slot"
    );
    d.shutdown().unwrap();
}

/// DIS-BATCH-LOOKUP-COLD-PROMOTES (failure exit): a GPU delivery that fails
/// must release the reserved slot, leave the key SSD resident, and be reported
/// as an error rather than a served hit.
#[test]
fn batch_lookup_cold_releases_slot_when_gpu_copy_fails() {
    let (_c, d, dm, mt, gpu) = setup();
    seed_cold(&dm, KEY, 0x1000);
    *gpu.fail_copies.lock().unwrap() = true;

    let mut dst = vec![0u8; BLOCK as usize];
    let h = IpcHandle {
        address: dst.as_mut_ptr(),
        size: BLOCK,
    };
    let out = d.batch_lookup(&[(KEY, vec![h])]);

    assert_eq!(out.len(), 1);
    assert!(
        out[0].result.is_err(),
        "the failed GPU delivery must fail the entry, got {:?}",
        out[0].result
    );
    assert!(
        !mt.contains(KEY),
        "the reserved slot must be released when the delivery fails"
    );
    assert_eq!(
        dm.loc(KEY),
        Some(Loc::Block(0x1000)),
        "the key must stay SSD resident (BlockDevice)"
    );
    d.shutdown().unwrap();
}
