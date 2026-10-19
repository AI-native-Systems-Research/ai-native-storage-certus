//! Repair test for finding `populate-leaks-slot`
//! (DIS-POPULATE-FAILURE-RELEASES-SLOT).
//!
//! When `populate` fails after it has reserved a memory-tier slot — the GPU
//! service is not bound, or the D2H copy's stream synchronization fails — the
//! reserved slot must be released and the key must not be cached, so a retry of
//! the same key can succeed.
//!
//! Self-contained: it drives only the public `IDispatcher` API through its own
//! minimal mocks, so it builds against the unfixed code as well.

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use component_core::query_interface;
use interfaces::{
    CacheKey, DispatchMapError, DispatcherConfig, DmaBuffer, GpuDeviceInfo, GpuDmaBuffer,
    GpuIpcHandle, GpuStream, IDispatchMap, IDispatcher, IGpuServices, ILogger, IMemoryTier,
    IpcHandle, LookupResult, MemoryTierError, MemoryTierTelemetrySnapshot,
};

use crate::DispatcherComponent;

// --- dispatch map: a plain map of entries with reference counts ---

#[derive(Clone, Copy)]
enum Loc {
    #[allow(dead_code)] // never constructed here; kept so lookup models both locations
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

// --- GPU services: host memcpy stands in for every DMA; sync can be made to fail ---

#[derive(Default)]
struct HostGpu {
    /// every `stream_synchronize` fails
    fail_sync: AtomicBool,
    /// every pointer belongs to no GPU, so populate falls back to the null stream
    no_device: AtomicBool,
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
        if self.no_device.load(Ordering::Relaxed) {
            return Ok(-1);
        }
        Ok(0)
    }
    fn stream_query(&self, _s: GpuStream) -> Result<bool, String> {
        Ok(true)
    }
    fn destroy_stream(&self, _s: GpuStream) -> Result<(), String> {
        Ok(())
    }
    fn stream_synchronize(&self, _s: GpuStream) -> Result<(), String> {
        if self.fail_sync.load(Ordering::Relaxed) {
            return Err("injected stream_synchronize failure".into());
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

/// A dispatcher wired to the mocks above; `gpu` is left unbound when `None`.
fn setup(
    gpu: Option<Arc<HostGpu>>,
) -> (
    Arc<DispatcherComponent>,
    Arc<dyn IDispatcher + Send + Sync>,
    Arc<MapDm>,
    Arc<PoolMt>,
) {
    let dm = Arc::new(MapDm::default());
    let mt = Arc::new(PoolMt::new(1024 * 1024));
    let c = DispatcherComponent::new_default();
    c.dispatch_map
        .connect(Arc::clone(&dm) as Arc<dyn IDispatchMap + Send + Sync>)
        .unwrap();
    c.logger.connect(Arc::new(QuietLogger)).unwrap();
    if let Some(g) = gpu {
        c.gpu_services
            .connect(g as Arc<dyn IGpuServices + Send + Sync>)
            .unwrap();
    }
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

fn assert_released(dm: &MapDm, mt: &PoolMt, key: CacheKey, what: &str) {
    assert!(
        !mt.contains(key),
        "{what}: populate failed but left the reserved memory-tier slot for key {key} allocated"
    );
    assert!(
        !dm.entries.lock().unwrap().contains_key(&key),
        "{what}: a failed populate must not cache key {key}"
    );
}

#[test]
fn populate_without_gpu_service_releases_the_slot() {
    let (_c, d, dm, mt) = setup(None);
    let mut src = vec![0x5Au8; 4096];
    let h = IpcHandle {
        address: src.as_mut_ptr(),
        size: src.len() as u32,
    };

    let r = d.populate(7, h);
    assert!(
        r.is_err(),
        "populate without a GPU service must fail, got {r:?}"
    );
    assert_released(&dm, &mt, 7, "gpu_services unbound");

    d.shutdown().unwrap();
}

#[test]
fn populate_sync_failure_releases_the_slot_and_allows_retry() {
    let gpu = Arc::new(HostGpu::default());
    let (_c, d, dm, mt) = setup(Some(Arc::clone(&gpu)));
    let mut src = vec![0x5Au8; 4096];
    let h = IpcHandle {
        address: src.as_mut_ptr(),
        size: src.len() as u32,
    };

    gpu.fail_sync.store(true, Ordering::Relaxed);
    let r = d.populate(7, h.clone());
    assert!(
        r.is_err(),
        "populate with a failing sync must fail, got {r:?}"
    );
    assert_released(&dm, &mt, 7, "store-stream sync failure");

    // Once the GPU recovers, storing the same key again must work.
    gpu.fail_sync.store(false, Ordering::Relaxed);
    let r = d.populate(7, h);
    assert!(
        r.is_ok(),
        "retry after a failed populate must succeed, got {r:?}"
    );
    assert!(dm.entries.lock().unwrap().contains_key(&7));

    d.shutdown().unwrap();
}

#[test]
fn populate_null_stream_sync_failure_releases_the_slot() {
    // No device for the pointer: populate passes the null stream, so the sync
    // that fails is the one inside copy_gpu_to_memory_async.
    let gpu = Arc::new(HostGpu::default());
    gpu.no_device.store(true, Ordering::Relaxed);
    let (_c, d, dm, mt) = setup(Some(Arc::clone(&gpu)));
    let mut src = vec![0x5Au8; 4096];
    let h = IpcHandle {
        address: src.as_mut_ptr(),
        size: src.len() as u32,
    };

    gpu.fail_sync.store(true, Ordering::Relaxed);
    let r = d.populate(9, h);
    assert!(
        r.is_err(),
        "populate with a failing sync must fail, got {r:?}"
    );
    assert_released(&dm, &mt, 9, "null-stream sync failure");

    d.shutdown().unwrap();
}
