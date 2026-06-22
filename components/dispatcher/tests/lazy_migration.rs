//! Integration tests for lazy migration of staging buffers to SSD/NVMe.
//!
//! Verifies that after `populate()`, the background writer migrates entries
//! from staging (DMA buffer) to block-device state, and that subsequent
//! lookups and checks still succeed on migrated entries.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::thread;

use component_core::query_interface;
use dispatcher::DispatcherComponent;
use interfaces::{
    CacheKey, DispatchMapError, DispatcherConfig, DmaAllocFn, DmaBuffer, GpuDeviceInfo,
    GpuDmaBuffer, GpuIpcHandle, GpuStream, IDispatchMap, IDispatcher, IGpuServices, ILogger,
    IMemoryTier, IpcHandle, LookupResult, MemoryTierError,
};

// ---------------------------------------------------------------------------
// Mock infrastructure
// ---------------------------------------------------------------------------

unsafe extern "C" fn dma_free(ptr: *mut std::ffi::c_void) {
    unsafe { libc::free(ptr) };
}

fn alloc_dma_buffer(size: usize) -> Arc<DmaBuffer> {
    let sz = size.max(4096);
    let aligned_sz = sz.next_multiple_of(4096);
    let ptr = unsafe { libc::aligned_alloc(4096, aligned_sz) };
    assert!(!ptr.is_null(), "aligned_alloc failed for {aligned_sz} bytes");
    unsafe { std::ptr::write_bytes(ptr as *mut u8, 0, aligned_sz) };
    let buf = unsafe { DmaBuffer::from_raw(ptr, aligned_sz, dma_free, -1) }.unwrap();
    Arc::new(buf)
}

struct MockEntry {
    buffer: Arc<DmaBuffer>,
    block_offset: Option<u64>,
    write_ref: bool,
    read_refs: u32,
}

struct MockDmInner {
    entries: HashMap<CacheKey, MockEntry>,
}

struct MockDispatchMap {
    inner: Mutex<MockDmInner>,
}

impl MockDispatchMap {
    fn new() -> Self {
        Self {
            inner: Mutex::new(MockDmInner {
                entries: HashMap::new(),
            }),
        }
    }

    fn migrated_count(&self) -> usize {
        self.inner
            .lock()
            .unwrap()
            .entries
            .values()
            .filter(|e| e.block_offset.is_some())
            .count()
    }

    fn entry_count(&self) -> usize {
        self.inner.lock().unwrap().entries.len()
    }
}

impl IDispatchMap for MockDispatchMap {
    fn set_dma_alloc(&self, _alloc: DmaAllocFn) {}

    fn initialize(&self) -> Result<(), DispatchMapError> {
        Ok(())
    }

    fn create_staging(&self, key: CacheKey, size: u32) -> Result<Arc<DmaBuffer>, DispatchMapError> {
        let mut inner = self.inner.lock().unwrap();
        if inner.entries.contains_key(&key) {
            return Err(DispatchMapError::AlreadyExists(key));
        }
        let buffer = alloc_dma_buffer(size as usize * 4096);
        inner.entries.insert(
            key,
            MockEntry {
                buffer: Arc::clone(&buffer),
                block_offset: None,
                write_ref: true,
                read_refs: 0,
            },
        );
        Ok(buffer)
    }

    fn lookup(&self, key: CacheKey) -> Result<LookupResult, DispatchMapError> {
        let inner = self.inner.lock().unwrap();
        match inner.entries.get(&key) {
            None => Ok(LookupResult::NotExist),
            Some(entry) => match entry.block_offset {
                Some(offset) => Ok(LookupResult::BlockDevice { offset }),
                None => Ok(LookupResult::Staging {
                    buffer: Arc::clone(&entry.buffer),
                }),
            },
        }
    }

    fn convert_to_storage(&self, key: CacheKey, offset: u64) -> Result<(), DispatchMapError> {
        let mut inner = self.inner.lock().unwrap();
        match inner.entries.get_mut(&key) {
            None => Err(DispatchMapError::KeyNotFound(key)),
            Some(entry) => {
                entry.block_offset = Some(offset);
                Ok(())
            }
        }
    }

    fn take_read(&self, key: CacheKey) -> Result<(), DispatchMapError> {
        let mut inner = self.inner.lock().unwrap();
        match inner.entries.get_mut(&key) {
            None => Err(DispatchMapError::KeyNotFound(key)),
            Some(entry) => {
                entry.read_refs += 1;
                Ok(())
            }
        }
    }

    fn take_write(&self, key: CacheKey) -> Result<(), DispatchMapError> {
        let mut inner = self.inner.lock().unwrap();
        match inner.entries.get_mut(&key) {
            None => Err(DispatchMapError::KeyNotFound(key)),
            Some(entry) => {
                entry.write_ref = true;
                Ok(())
            }
        }
    }

    fn release_read(&self, key: CacheKey) -> Result<(), DispatchMapError> {
        let mut inner = self.inner.lock().unwrap();
        match inner.entries.get_mut(&key) {
            None => Err(DispatchMapError::KeyNotFound(key)),
            Some(entry) => {
                entry.read_refs = entry.read_refs.saturating_sub(1);
                Ok(())
            }
        }
    }

    fn release_write(&self, key: CacheKey) -> Result<(), DispatchMapError> {
        let mut inner = self.inner.lock().unwrap();
        match inner.entries.get_mut(&key) {
            None => Err(DispatchMapError::KeyNotFound(key)),
            Some(entry) => {
                entry.write_ref = false;
                Ok(())
            }
        }
    }

    fn downgrade_reference(&self, key: CacheKey) -> Result<(), DispatchMapError> {
        let mut inner = self.inner.lock().unwrap();
        match inner.entries.get_mut(&key) {
            None => Err(DispatchMapError::NoWriteReference(key)),
            Some(entry) => {
                entry.write_ref = false;
                entry.read_refs += 1;
                Ok(())
            }
        }
    }

    fn remove(&self, key: CacheKey) -> Result<(), DispatchMapError> {
        let mut inner = self.inner.lock().unwrap();
        if inner.entries.remove(&key).is_some() {
            Ok(())
        } else {
            Err(DispatchMapError::KeyNotFound(key))
        }
    }

    fn touch(&self, key: CacheKey) -> Result<(), DispatchMapError> {
        let inner = self.inner.lock().unwrap();
        if inner.entries.contains_key(&key) {
            Ok(())
        } else {
            Err(DispatchMapError::KeyNotFound(key))
        }
    }

    fn entry_size(&self, key: CacheKey) -> Result<u32, DispatchMapError> {
        let inner = self.inner.lock().unwrap();
        if inner.entries.contains_key(&key) {
            Ok(4096)
        } else {
            Err(DispatchMapError::KeyNotFound(key))
        }
    }

    fn oldest_keys(&self, n: usize) -> Vec<CacheKey> {
        let inner = self.inner.lock().unwrap();
        inner.entries.keys().copied().take(n).collect()
    }

    fn create_memory_tier_entry(
        &self,
        key: CacheKey,
        _pointer: *mut u8,
        _size: u32,
    ) -> Result<(), DispatchMapError> {
        let mut inner = self.inner.lock().unwrap();
        if inner.entries.contains_key(&key) {
            return Err(DispatchMapError::AlreadyExists(key));
        }
        inner.entries.insert(
            key,
            MockEntry {
                buffer: alloc_dma_buffer(4096),
                block_offset: None,
                write_ref: true,
                read_refs: 0,
            },
        );
        Ok(())
    }

    fn convert_memory_tier_to_block(&self, key: CacheKey) -> Result<(), DispatchMapError> {
        let inner = self.inner.lock().unwrap();
        if inner.entries.contains_key(&key) {
            Ok(())
        } else {
            Err(DispatchMapError::KeyNotFound(key))
        }
    }

    fn is_evictable(&self, key: CacheKey) -> bool {
        let inner = self.inner.lock().unwrap();
        inner.entries.contains_key(&key)
    }

    fn recover_extent(
        &self,
        _key: CacheKey,
        _offset: u64,
        _size_blocks: u32,
    ) -> Result<(), DispatchMapError> {
        Ok(())
    }
}

struct MockLogger;

impl ILogger for MockLogger {
    fn error(&self, _msg: &str) {}
    fn warn(&self, _msg: &str) {}
    fn info(&self, _msg: &str) {}
    fn debug(&self, _msg: &str) {}
}

struct MockGpuServices;

impl IGpuServices for MockGpuServices {
    fn initialize(&self) -> Result<(), String> {
        Ok(())
    }
    fn shutdown(&self) -> Result<(), String> {
        Ok(())
    }
    fn get_devices(&self) -> Result<Vec<GpuDeviceInfo>, String> {
        Ok(vec![])
    }
    fn deserialize_ipc_handle(&self, _base64_payload: &str) -> Result<GpuIpcHandle, String> {
        Err("mock: not implemented".into())
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
        Err("mock: not implemented".into())
    }
    fn dma_copy_to_host(
        &self,
        src: *const std::ffi::c_void,
        dst: &DmaBuffer,
        size: usize,
    ) -> Result<(), String> {
        unsafe {
            std::ptr::copy_nonoverlapping(src as *const u8, dst.as_ptr() as *mut u8, size);
        }
        Ok(())
    }
    fn dma_copy_to_device(
        &self,
        src: &DmaBuffer,
        dst: *mut std::ffi::c_void,
        size: usize,
    ) -> Result<(), String> {
        unsafe {
            std::ptr::copy_nonoverlapping(src.as_ptr() as *const u8, dst as *mut u8, size);
        }
        Ok(())
    }
    fn prepare_memory_for_spdk(
        &self,
        _base64_payload: &str,
        _device_index: Option<u32>,
    ) -> Result<DmaBuffer, String> {
        Err("mock: not implemented".into())
    }
    fn create_stream(&self) -> Result<GpuStream, String> {
        Ok(GpuStream(0x1 as *mut std::ffi::c_void))
    }
    fn destroy_stream(&self, _stream: GpuStream) -> Result<(), String> {
        Ok(())
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
        unsafe {
            std::ptr::copy_nonoverlapping(src.as_ptr() as *const u8, dst as *mut u8, size);
        }
        Ok(())
    }
    fn memcpy_h2d_async(
        &self,
        src: *const std::ffi::c_void,
        dst: *mut std::ffi::c_void,
        size: usize,
        _stream: GpuStream,
    ) -> Result<(), String> {
        unsafe {
            std::ptr::copy_nonoverlapping(src as *const u8, dst as *mut u8, size);
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

// --- MockMemoryTier ---

struct MockMtSlot {
    offset: usize,
    size: u32,
}

struct MockMemoryTier {
    inner: Mutex<MockMtInner>,
}

struct MockMtInner {
    pool: Vec<u8>,
    slots: HashMap<CacheKey, MockMtSlot>,
    next_offset: usize,
    capacity: usize,
}

impl MockMemoryTier {
    fn new(capacity: usize) -> Self {
        Self {
            inner: Mutex::new(MockMtInner {
                pool: vec![0u8; capacity],
                slots: HashMap::new(),
                next_offset: 0,
                capacity,
            }),
        }
    }
}

impl IMemoryTier for MockMemoryTier {
    fn initialize(&self, _pool_size: usize, _numa_node: Option<i32>) -> Result<(), MemoryTierError> {
        Ok(())
    }

    fn insert(&self, key: CacheKey, size: u32) -> Result<*mut u8, MemoryTierError> {
        let mut inner = self.inner.lock().unwrap();
        if inner.slots.contains_key(&key) {
            return Err(MemoryTierError::AlreadyExists(key));
        }
        let aligned = (size as usize).next_multiple_of(4096);
        if inner.next_offset + aligned > inner.capacity {
            return Err(MemoryTierError::PoolFull);
        }
        let offset = inner.next_offset;
        inner.next_offset += aligned;
        inner.slots.insert(key, MockMtSlot { offset, size });
        let ptr = unsafe { inner.pool.as_mut_ptr().add(offset) };
        Ok(ptr)
    }

    fn get(&self, key: CacheKey) -> Option<(*mut u8, u32)> {
        let inner = self.inner.lock().unwrap();
        inner.slots.get(&key).map(|slot| {
            let ptr = unsafe { (inner.pool.as_ptr() as *mut u8).add(slot.offset) };
            (ptr, slot.size)
        })
    }

    fn peek(&self, key: CacheKey) -> Option<(*mut u8, u32)> {
        self.get(key)
    }

    fn oldest_keys(&self, n: usize) -> Vec<CacheKey> {
        let inner = self.inner.lock().unwrap();
        inner.slots.keys().take(n).copied().collect()
    }

    fn evict_lru(&self) -> Option<CacheKey> {
        let mut inner = self.inner.lock().unwrap();
        let key = inner.slots.keys().next().copied()?;
        inner.slots.remove(&key);
        Some(key)
    }

    fn evict_lru_for_key(&self, _key: CacheKey) -> Option<CacheKey> {
        self.evict_lru()
    }

    fn remove(&self, key: CacheKey) -> Result<(), MemoryTierError> {
        let mut inner = self.inner.lock().unwrap();
        inner
            .slots
            .remove(&key)
            .map(|_| ())
            .ok_or(MemoryTierError::KeyNotFound(key))
    }

    fn touch(&self, _key: CacheKey) {}
    fn batch_touch(&self, _keys: &[CacheKey]) {}

    fn contains(&self, key: CacheKey) -> bool {
        self.inner.lock().unwrap().slots.contains_key(&key)
    }

    fn capacity(&self) -> usize {
        self.inner.lock().unwrap().capacity
    }

    fn used(&self) -> usize {
        self.inner.lock().unwrap().next_offset
    }

    fn pool_info(&self) -> Option<(*mut u8, usize)> {
        let inner = self.inner.lock().unwrap();
        Some((inner.pool.as_ptr() as *mut u8, inner.capacity))
    }

    fn clear(&self) -> Result<usize, MemoryTierError> {
        let mut inner = self.inner.lock().unwrap();
        let count = inner.slots.len();
        inner.slots.clear();
        inner.next_offset = 0;
        Ok(count)
    }

    fn is_dma_capable(&self) -> bool {
        false
    }
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn setup() -> (Arc<DispatcherComponent>, Arc<MockDispatchMap>) {
    let dm = Arc::new(MockDispatchMap::new());
    let logger: Arc<dyn ILogger + Send + Sync> = Arc::new(MockLogger);
    let gpu: Arc<dyn IGpuServices + Send + Sync> = Arc::new(MockGpuServices);
    let mt: Arc<dyn IMemoryTier + Send + Sync> = Arc::new(MockMemoryTier::new(1024 * 1024));
    let c = DispatcherComponent::new_default();
    c.dispatch_map
        .connect(Arc::clone(&dm) as Arc<dyn IDispatchMap + Send + Sync>)
        .unwrap();
    c.logger.connect(logger).unwrap();
    c.gpu_services.connect(gpu).unwrap();
    c.memory_tier.connect(mt).unwrap();

    let d = query_interface!(c, IDispatcher).unwrap();
    d.initialize(DispatcherConfig {
        data_pci_addrs: vec!["0000:02:00.0".to_string()],
        ..Default::default()
    })
    .unwrap();

    (c, dm)
}

fn make_handle(buf: &mut [u8]) -> IpcHandle {
    IpcHandle {
        address: buf.as_mut_ptr(),
        size: buf.len() as u32,
    }
}

// ---------------------------------------------------------------------------
// Lazy migration tests
// ---------------------------------------------------------------------------

#[test]
fn staging_entry_migrates_to_block_device_on_drain() {
    let (c, dm) = setup();
    let d = query_interface!(c, IDispatcher).unwrap();

    let mut buf = vec![0u8; 4096];
    d.populate(1, make_handle(&mut buf)).unwrap();

    assert_eq!(dm.migrated_count(), 0, "should still be in staging before drain");

    d.shutdown().unwrap();

    assert_eq!(dm.migrated_count(), 1, "entry should be migrated after bg writer drains");
}

#[test]
fn multiple_entries_all_migrate() {
    let (c, dm) = setup();
    let d = query_interface!(c, IDispatcher).unwrap();

    for key in 0..10u64 {
        let mut buf = vec![0u8; 8192];
        d.populate(key, make_handle(&mut buf)).unwrap();
    }

    assert_eq!(dm.entry_count(), 10);

    d.shutdown().unwrap();

    assert_eq!(dm.migrated_count(), 10, "all entries should migrate");
}

#[test]
fn lookup_after_migration_requires_hardware() {
    let (c, _dm) = setup();
    let d = query_interface!(c, IDispatcher).unwrap();

    let mut buf = vec![0u8; 4096];
    d.populate(42, make_handle(&mut buf)).unwrap();

    // Drain bg writer — entry is now marked as BlockDevice
    d.shutdown().unwrap();

    // Re-initialize to allow lookups
    d.initialize(DispatcherConfig {
        data_pci_addrs: vec!["0000:02:00.0".to_string()],
        ..Default::default()
    })
    .unwrap();

    // Without real block devices, lookup on a migrated entry returns IoError
    let mut buf2 = vec![0u8; 4096];
    let result = d.lookup(42, make_handle(&mut buf2));
    assert!(
        result.is_err(),
        "lookup on migrated entry without hardware should fail"
    );

    d.shutdown().unwrap();
}

#[test]
fn check_finds_migrated_entry() {
    let (c, _dm) = setup();
    let d = query_interface!(c, IDispatcher).unwrap();

    let mut buf = vec![0u8; 4096];
    d.populate(7, make_handle(&mut buf)).unwrap();

    d.shutdown().unwrap();

    d.initialize(DispatcherConfig {
        data_pci_addrs: vec!["0000:02:00.0".to_string()],
        ..Default::default()
    })
    .unwrap();

    assert!(d.check(7).unwrap(), "migrated entry should be discoverable");
    d.shutdown().unwrap();
}

#[test]
fn concurrent_populates_all_migrate() {
    let (c, dm) = setup();

    let handles: Vec<_> = (0..4)
        .map(|t| {
            let comp = Arc::clone(&c);
            thread::spawn(move || {
                let d = query_interface!(comp, IDispatcher).unwrap();
                for i in 0..5 {
                    let key: u64 = t * 1000 + i;
                    let mut buf = vec![0u8; 4096];
                    d.populate(key, make_handle(&mut buf)).unwrap();
                }
            })
        })
        .collect();

    for h in handles {
        h.join().unwrap();
    }

    let d = query_interface!(c, IDispatcher).unwrap();
    d.shutdown().unwrap();

    assert_eq!(dm.entry_count(), 20);
    assert_eq!(
        dm.migrated_count(),
        20,
        "all concurrently populated entries should migrate"
    );
}
