//! DIS-REMOVE-NO-WAIT (FR-018): `remove` proceeds immediately, without waiting for
//! a pending background write-through of the key.
//!
//! Self-contained (its own mocks), so it builds against the dispatcher as it was
//! before the repair as well as after it.

use super::*;
use component_core::query_interface;
use interfaces::{
    DispatchMapError, GpuDeviceInfo, GpuDmaBuffer, GpuIpcHandle, MemoryTierError,
    MemoryTierTelemetrySnapshot,
};
use std::collections::HashMap;

// --- Mocks -----------------------------------------------------------------

struct NullLogger;

impl ILogger for NullLogger {
    fn error(&self, _msg: &str) {}
    fn warn(&self, _msg: &str) {}
    fn info(&self, _msg: &str) {}
    fn debug(&self, _msg: &str) {}
}

/// Pool bytes, `key -> (offset, size)` slots, and bytes used.
type PoolState = (Vec<u8>, HashMap<CacheKey, (usize, u32)>, usize);

/// A memory tier over a plain heap pool, one 4 KiB-aligned slot per key.
struct PoolTier {
    inner: Mutex<PoolState>,
}

impl PoolTier {
    fn new(capacity: usize) -> Self {
        Self {
            inner: Mutex::new((vec![0u8; capacity], HashMap::new(), 0)),
        }
    }
}

impl IMemoryTier for PoolTier {
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
        // SAFETY: off + aligned <= pool.len().
        Ok(unsafe { pool.as_mut_ptr().add(off) })
    }
    fn get(&self, key: CacheKey) -> Option<(*mut u8, u32)> {
        let mut g = self.inner.lock().unwrap();
        let (pool, slots, _) = &mut *g;
        let &(off, size) = slots.get(&key)?;
        // SAFETY: off is within the pool.
        Some((unsafe { pool.as_mut_ptr().add(off) }, size))
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
    fn oldest_keys(&self, _n: usize) -> Vec<CacheKey> {
        Vec::new()
    }
    fn remove(&self, key: CacheKey) -> Result<(), MemoryTierError> {
        let mut g = self.inner.lock().unwrap();
        g.1.remove(&key)
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
        let mut g = self.inner.lock().unwrap();
        Some((g.0.as_mut_ptr(), g.0.len()))
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

/// One dispatch-map entry: where it lives and its reference counts.
struct Entry {
    pointer: usize,
    size: u32,
    ssd: Option<u64>,
    write_ref: bool,
    read_refs: u32,
}

/// A dispatch map with the real map's reference rules: `remove` refuses an entry
/// with any read or write reference, `convert_to_storage` consumes the read pin
/// `downgrade_reference` left.
#[derive(Default)]
struct RefMap {
    entries: Mutex<HashMap<CacheKey, Entry>>,
}

impl RefMap {
    fn len(&self) -> usize {
        self.entries.lock().unwrap().len()
    }
    fn with<R>(
        &self,
        key: CacheKey,
        f: impl FnOnce(&mut Entry) -> R,
    ) -> Result<R, DispatchMapError> {
        self.entries
            .lock()
            .unwrap()
            .get_mut(&key)
            .map(f)
            .ok_or(DispatchMapError::KeyNotFound(key))
    }
}

impl IDispatchMap for RefMap {
    fn initialize(&self) -> Result<(), DispatchMapError> {
        Ok(())
    }
    fn lookup(&self, key: CacheKey) -> Result<LookupResult, DispatchMapError> {
        let mut g = self.entries.lock().unwrap();
        let Some(e) = g.get_mut(&key) else {
            return Ok(LookupResult::NotExist);
        };
        e.read_refs += 1;
        Ok(match e.ssd {
            Some(offset) if e.pointer == 0 => LookupResult::BlockDevice { offset },
            _ => LookupResult::MemoryTier {
                pointer: e.pointer as *mut u8,
                size: e.size,
            },
        })
    }
    fn convert_to_storage(&self, key: CacheKey, offset: u64) -> Result<(), DispatchMapError> {
        self.with(key, |e| {
            e.ssd = Some(offset);
            e.read_refs = e.read_refs.saturating_sub(1);
        })
    }
    fn take_read(&self, key: CacheKey) -> Result<(), DispatchMapError> {
        self.with(key, |e| e.read_refs += 1)
    }
    fn take_write(&self, key: CacheKey) -> Result<(), DispatchMapError> {
        self.with(key, |e| e.write_ref = true)
    }
    fn release_read(&self, key: CacheKey) -> Result<(), DispatchMapError> {
        self.with(key, |e| e.read_refs = e.read_refs.saturating_sub(1))
    }
    fn release_write(&self, key: CacheKey) -> Result<(), DispatchMapError> {
        self.with(key, |e| e.write_ref = false)
    }
    fn downgrade_reference(&self, key: CacheKey) -> Result<(), DispatchMapError> {
        self.with(key, |e| {
            e.write_ref = false;
            e.read_refs += 1;
        })
    }
    fn remove(&self, key: CacheKey) -> Result<(), DispatchMapError> {
        let mut g = self.entries.lock().unwrap();
        match g.get(&key) {
            None => Err(DispatchMapError::KeyNotFound(key)),
            Some(e) if e.read_refs > 0 || e.write_ref => {
                Err(DispatchMapError::ActiveReferences(key))
            }
            Some(_) => {
                g.remove(&key);
                Ok(())
            }
        }
    }
    fn touch(&self, key: CacheKey) -> Result<(), DispatchMapError> {
        self.with(key, |_| ())
    }
    fn entry_size(&self, key: CacheKey) -> Result<u32, DispatchMapError> {
        self.with(key, |e| e.size)
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
        let mut g = self.entries.lock().unwrap();
        if g.contains_key(&key) {
            return Err(DispatchMapError::AlreadyExists(key));
        }
        g.insert(
            key,
            Entry {
                pointer: pointer as usize,
                size,
                ssd: None,
                write_ref: true,
                read_refs: 0,
            },
        );
        Ok(())
    }
    fn convert_memory_tier_to_block(&self, key: CacheKey) -> Result<(), DispatchMapError> {
        self.with(key, |e| e.pointer = 0)
    }
    fn promote_block_to_memory_tier(
        &self,
        key: CacheKey,
        pointer: *mut u8,
        size: u32,
    ) -> Result<(), DispatchMapError> {
        self.with(key, |e| {
            e.pointer = pointer as usize;
            e.size = size;
        })
    }
    fn is_evictable(&self, _key: CacheKey) -> bool {
        false
    }
    fn try_evict_to_block(&self, key: CacheKey) -> Result<(), DispatchMapError> {
        Err(DispatchMapError::ActiveReferences(key))
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

/// Host-memory "GPU": every copy is a plain memcpy, there is no device.
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

// --- Setup -----------------------------------------------------------------

struct Rig {
    c: Arc<DispatcherComponent>,
    dm: Arc<RefMap>,
    mt: Arc<PoolTier>,
    d: Arc<dyn IDispatcher + Send + Sync>,
}

/// An initialized dispatcher (no SPDK) whose background writer is taken out, so
/// every write-through `populate` queues stays pending: the read pin `populate`
/// leaves for the job stays held, exactly as while the job waits in the queue.
fn rig_with_write_through_pending() -> Rig {
    let dm = Arc::new(RefMap::default());
    let mt = Arc::new(PoolTier::new(1024 * 1024));
    let c = DispatcherComponent::new_default();
    c.dispatch_map
        .connect(Arc::clone(&dm) as Arc<dyn IDispatchMap + Send + Sync>)
        .unwrap();
    c.logger.connect(Arc::new(NullLogger)).unwrap();
    c.gpu_services.connect(Arc::new(HostGpu)).unwrap();
    c.memory_tier
        .connect(Arc::clone(&mt) as Arc<dyn IMemoryTier + Send + Sync>)
        .unwrap();
    let d = query_interface!(c, IDispatcher).unwrap();
    d.initialize(DispatcherConfig {
        data_pci_addrs: vec!["0000:02:00.0".to_string()],
        ..Default::default()
    })
    .unwrap();
    if let Some(mut writer) = c.bg_writer.lock().unwrap().take() {
        writer.shutdown();
    }
    Rig { c, dm, mt, d }
}

fn handle(buf: &mut [u8]) -> IpcHandle {
    IpcHandle {
        address: buf.as_mut_ptr(),
        size: buf.len() as u32,
    }
}

// --- Tests -----------------------------------------------------------------

#[test]
fn remove_proceeds_while_write_through_pending() {
    let r = rig_with_write_through_pending();
    let mut buf = vec![5u8; 4096];
    r.d.populate(7, handle(&mut buf)).unwrap();
    assert!(r.d.check(7).unwrap(), "populate cached the key");

    let res = r.d.remove(7);

    assert!(
        res.is_ok(),
        "remove refused a key whose write-through is still pending: {res:?}"
    );
    assert_eq!(r.dm.len(), 0, "the dispatch-map entry is gone");
    assert!(!r.d.check(7).unwrap(), "the key is no longer cached");
    assert!(!r.mt.contains(7), "the memory-tier slot was freed");
    r.d.shutdown().unwrap();
    drop(r.c);
}

#[test]
fn key_can_be_stored_again_after_remove_with_write_through_pending() {
    let r = rig_with_write_through_pending();
    let mut buf = vec![6u8; 4096];
    r.d.populate(8, handle(&mut buf)).unwrap();

    assert!(r.d.remove(8).is_ok(), "remove refused a pending key");
    r.d.populate(8, handle(&mut buf)).unwrap();

    assert_eq!(r.dm.len(), 1);
    assert!(r.d.check(8).unwrap());
    r.d.shutdown().unwrap();
}
