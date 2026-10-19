//! Repair test for finding `batch-lookup-served-by`
//! (DIS-BATCH-LOOKUP-SERVEDBY-CONCURRENT-DRAM, DIS-BATCH-LOOKUP-SERVEDBY-OVERWRITE).
//!
//! A key that loses the `mt.insert` race to a concurrent promotion is classified
//! `BlockDevice` (attributed `Ssd`), then rewritten by the recovery pass, which
//! serves it out of DRAM. The attribution must be rewritten with the result
//! (spec 002 FR-009) and must be `Dram` (spec 002 FR-010).
//!
//! Self-contained: it drives only the public `IDispatcher` API through its own
//! minimal mocks, so it builds against the unfixed code as well.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use component_core::query_interface;
use interfaces::{
    CacheKey, DispatchMapError, DispatcherConfig, DmaBuffer, GpuDeviceInfo, GpuDmaBuffer,
    GpuIpcHandle, GpuStream, IDispatchMap, IDispatcher, IGpuServices, ILogger, IMemoryTier,
    IpcHandle, LookupResult, MemoryTierError, MemoryTierTelemetrySnapshot, ServedBy,
};

use crate::DispatcherComponent;

// --- dispatch map: one key, BlockDevice until its first lookup, then MemoryTier ---

#[derive(Clone, Copy)]
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
struct RaceDm {
    entries: Mutex<HashMap<CacheKey, Ent>>,
    // One-shot: the lookup that sees `Block` installs this, as the concurrent
    // promotion winner finishing its SSD->DRAM read would.
    flip: Mutex<HashMap<CacheKey, (usize, u32)>>,
}

impl IDispatchMap for RaceDm {
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
            Loc::Block(offset) => {
                if let Some((p, s)) = self.flip.lock().unwrap().remove(&key) {
                    e.loc = Loc::Memory(p, s);
                }
                LookupResult::BlockDevice { offset }
            }
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
    fn is_evictable(&self, _key: CacheKey) -> bool {
        false
    }
    fn try_evict_to_block(&self, _key: CacheKey) -> Result<(), DispatchMapError> {
        Err(DispatchMapError::InvalidState("not evictable".into()))
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

// --- memory tier: a flat pool of 4 KiB-aligned slots ---

struct PoolMt {
    inner: Mutex<(Vec<u8>, HashMap<CacheKey, (usize, u32)>, usize)>,
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
}

// --- GPU services: host memcpy stands in for every DMA ---

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

#[test]
fn concurrent_promotion_loser_is_attributed_dram() {
    let dm = Arc::new(RaceDm::default());
    let mt = Arc::new(PoolMt::new(1024 * 1024));
    let c = DispatcherComponent::new_default();
    c.dispatch_map
        .connect(Arc::clone(&dm) as Arc<dyn IDispatchMap + Send + Sync>)
        .unwrap();
    c.logger.connect(Arc::new(QuietLogger)).unwrap();
    c.gpu_services.connect(Arc::new(HostGpu)).unwrap();
    c.memory_tier
        .connect(Arc::clone(&mt) as Arc<dyn IMemoryTier + Send + Sync>)
        .unwrap();
    let d = query_interface!(c, IDispatcher).unwrap();
    // No ISPDKEnv bound: no data drives, so cold keys take the no-drives promote.
    d.initialize(DispatcherConfig {
        data_pci_addrs: vec!["0000:02:00.0".to_string()],
        ..Default::default()
    })
    .unwrap();

    // The concurrent winner already holds key 1's DRAM slot (so this caller's
    // mt.insert -> AlreadyExists), filled with the block's bytes. The dispatch map
    // still says BlockDevice at classification, and MemoryTier on the recovery
    // lookup -- the winner having finished promoting in between.
    let slot = mt.insert(1, 4096).unwrap();
    // SAFETY: `slot` is a fresh 4096-byte slot inside the pool.
    unsafe { std::ptr::write_bytes(slot, 0xCD, 4096) };
    dm.entries.lock().unwrap().insert(
        1,
        Ent {
            loc: Loc::Block(0x1000),
            read_refs: 0,
            write_ref: false,
        },
    );
    dm.flip.lock().unwrap().insert(1, (slot as usize, 4096));

    let mut out = vec![0u8; 4096];
    let handle = IpcHandle {
        address: out.as_mut_ptr(),
        size: out.len() as u32,
    };
    let results = d.batch_lookup(&[(1, vec![handle])]);
    assert_eq!(results.len(), 1);
    assert!(
        results[0].result.is_ok(),
        "loser must be served: {:?}",
        results[0]
    );
    assert_eq!(out[0], 0xCD, "recovered block must carry the resident data");
    assert_eq!(
        results[0].served_by,
        ServedBy::Dram,
        "served from DRAM after waiting for a concurrent promotion, attributed {:?}",
        results[0].served_by
    );

    // The route counters are derived from the same attribution.
    let stats = d.tier_event_stats();
    assert_eq!(stats.lookup_hits_dram, 1, "lookup_hits_dram: {stats:?}");
    assert_eq!(stats.lookup_hits_ssd, 0, "lookup_hits_ssd: {stats:?}");

    d.shutdown().unwrap();
}
