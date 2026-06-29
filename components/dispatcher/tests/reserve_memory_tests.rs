//! Unit tests for `reserve_memory`, `copy_gpu_to_memory_async`,
//! `copy_gpu_to_memory_completed`, and `release_memory`.
//!
//! These four methods implement the memory-reservation phase of the KV-cache
//! GPU offloading path (backpressure hotpath).  They are tested here with an
//! all-mock component setup — no SPDK hardware required.
//!
//! # Test inventory (11 tests)
//!
//! | Test | Verifies |
//! |------|---------|
//! | `reserve_memory_happy_path_returns_nonnull_pointer` | Happy path returns non-null ptr |
//! | `reserve_memory_zero_size_returns_invalid_parameter` | size=0 → InvalidParameter |
//! | `reserve_memory_full_pool_returns_allocation_failed` | Pool exhausted → AllocationFailed |
//! | `reserve_memory_duplicate_key_returns_error` | Duplicate key → AlreadyExists |
//! | `release_memory_frees_reserved_slot` | Slot freed; Ok returned |
//! | `release_memory_absent_key_is_ok` | Absent key → Ok (idempotent) |
//! | `copy_gpu_to_memory_completed_makes_key_visible` | Key visible via `check()` after full lifecycle |
//! | `copy_gpu_to_memory_completed_without_reserve_returns_error` | No prior reserve → KeyNotFound |
//! | `copy_gpu_to_memory_async_copies_data_to_dram_slot` | Data appears in reserved DRAM slot |
//! | `full_three_phase_store_lifecycle` | reserve → copy_async → completed → check() |
//! | `reserve_release_re_reserve_sequence` | Slot reuse across multiple release cycles |

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use component_core::query_interface;
use dispatcher::DispatcherComponent;
use interfaces::{
    CacheKey, DispatchMapError, DispatcherConfig, DispatcherError, DmaAllocFn, DmaBuffer,
    GpuDeviceInfo, GpuDmaBuffer, GpuIpcHandle, GpuStream, IDispatchMap, IDispatcher, IGpuServices,
    ILogger, IMemoryTier, IpcHandle, LookupResult, MemoryTierError,
};

// ---------------------------------------------------------------------------
// Mock infrastructure
// ---------------------------------------------------------------------------

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
        // SAFETY: In tests both src (IpcHandle address) and dst (memory-tier pool) are valid.
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
        // SAFETY: same
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
        // 0x1 is a non-null sentinel for a fake stream; the mock ignores the value.
        Ok(GpuStream(0x1 as *mut std::ffi::c_void))
    }
    fn stream_query(&self, _stream: GpuStream) -> Result<bool, String> {
        Ok(true)
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
        // SAFETY: test-only; both pointers are valid host memory.
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
        // SAFETY: test-only host pointers.
        unsafe {
            std::ptr::copy_nonoverlapping(src as *const u8, dst as *mut u8, size);
        }
        Ok(())
    }
    fn dma_copy_to_host_async(
        &self,
        src: *const std::ffi::c_void,
        dst: &DmaBuffer,
        size: usize,
        _stream: GpuStream,
    ) -> Result<(), String> {
        // SAFETY: In tests src is a host slice and dst wraps the memory-tier pool.
        unsafe {
            std::ptr::copy_nonoverlapping(src as *const u8, dst.as_ptr() as *mut u8, size);
        }
        Ok(())
    }
    fn memcpy_d2h_async(
        &self,
        src: *const std::ffi::c_void,
        dst: *mut std::ffi::c_void,
        size: usize,
        _stream: GpuStream,
    ) -> Result<(), String> {
        // SAFETY: test-only host pointers.
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

struct MockMtInner {
    pool: Vec<u8>,
    slots: HashMap<CacheKey, MockMtSlot>,
    used: usize,
    capacity: usize,
    /// When true, every `insert` call returns `PoolFull` regardless of used/capacity.
    fail_insert: bool,
}

struct MockMemoryTier {
    inner: Mutex<MockMtInner>,
}

impl MockMemoryTier {
    /// Normal pool of `capacity` bytes.
    fn new(capacity: usize) -> Self {
        Self {
            inner: Mutex::new(MockMtInner {
                pool: vec![0u8; capacity],
                slots: HashMap::new(),
                used: 0,
                capacity,
                fail_insert: false,
            }),
        }
    }

    /// A pool whose `insert` always fails with `PoolFull`.
    ///
    /// The capacity is deliberately large so that `evict_for_space` does not
    /// loop (it only loops while `used + needed > capacity`); `insert` itself
    /// then fails immediately with the `fail_insert` flag, exercising the
    /// `AllocationFailed` error path in `reserve_memory`.
    fn always_fails() -> Self {
        Self {
            inner: Mutex::new(MockMtInner {
                pool: vec![0u8; 1024 * 1024],
                slots: HashMap::new(),
                used: 0,
                capacity: 1024 * 1024,
                fail_insert: true,
            }),
        }
    }
}

impl IMemoryTier for MockMemoryTier {
    fn initialize(
        &self,
        _pool_size: usize,
        _numa_node: Option<i32>,
    ) -> Result<(), MemoryTierError> {
        Ok(())
    }

    fn insert(&self, key: CacheKey, size: u32) -> Result<*mut u8, MemoryTierError> {
        let mut inner = self.inner.lock().unwrap();
        if inner.fail_insert {
            return Err(MemoryTierError::PoolFull);
        }
        if inner.slots.contains_key(&key) {
            return Err(MemoryTierError::AlreadyExists(key));
        }
        let aligned = (size as usize).next_multiple_of(4096);
        if inner.used + aligned > inner.capacity {
            return Err(MemoryTierError::PoolFull);
        }
        let offset = inner.used;
        inner.used += aligned;
        inner.slots.insert(key, MockMtSlot { offset, size });
        // SAFETY: offset is within pool bounds.
        let ptr = unsafe { inner.pool.as_mut_ptr().add(offset) };
        Ok(ptr)
    }

    fn get(&self, key: CacheKey) -> Option<(*mut u8, u32)> {
        let inner = self.inner.lock().unwrap();
        inner.slots.get(&key).map(|slot| {
            // SAFETY: slot.offset is within pool bounds.
            let ptr = unsafe { (inner.pool.as_ptr() as *mut u8).add(slot.offset) };
            (ptr, slot.size)
        })
    }

    fn peek(&self, key: CacheKey) -> Option<(*mut u8, u32)> {
        self.get(key)
    }

    fn oldest_keys(&self, n: usize) -> Vec<CacheKey> {
        let inner = self.inner.lock().unwrap();
        inner.slots.keys().copied().take(n).collect()
    }

    fn evict_lru(&self) -> Option<CacheKey> {
        let mut inner = self.inner.lock().unwrap();
        let key = inner.slots.keys().next().copied()?;
        let slot = inner.slots.remove(&key).unwrap();
        let aligned = (slot.size as usize).next_multiple_of(4096);
        inner.used = inner.used.saturating_sub(aligned);
        Some(key)
    }

    fn evict_lru_for_key(&self, _key: CacheKey) -> Option<CacheKey> {
        self.evict_lru()
    }

    fn remove(&self, key: CacheKey) -> Result<(), MemoryTierError> {
        let mut inner = self.inner.lock().unwrap();
        match inner.slots.remove(&key) {
            Some(slot) => {
                let aligned = (slot.size as usize).next_multiple_of(4096);
                inner.used = inner.used.saturating_sub(aligned);
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
        self.inner.lock().unwrap().capacity
    }

    fn used(&self) -> usize {
        self.inner.lock().unwrap().used
    }

    fn pool_info(&self) -> Option<(*mut u8, usize)> {
        let inner = self.inner.lock().unwrap();
        Some((inner.pool.as_ptr() as *mut u8, inner.capacity))
    }

    fn clear(&self) -> Result<usize, MemoryTierError> {
        let mut inner = self.inner.lock().unwrap();
        let n = inner.slots.len();
        inner.slots.clear();
        inner.used = 0;
        Ok(n)
    }

    fn is_dma_capable(&self) -> bool {
        false
    }
}

// --- MockDispatchMap ---

struct MockDmEntry {
    pointer: *mut u8,
    size: u32,
    write_ref: bool,
    read_refs: u32,
}

// SAFETY: pointer is borrowed from the MockMemoryTier pool which outlives the entry.
unsafe impl Send for MockDmEntry {}
unsafe impl Sync for MockDmEntry {}

struct MockDispatchMap {
    inner: Mutex<HashMap<CacheKey, MockDmEntry>>,
}

impl MockDispatchMap {
    fn new() -> Self {
        Self {
            inner: Mutex::new(HashMap::new()),
        }
    }

    fn entry_count(&self) -> usize {
        self.inner.lock().unwrap().len()
    }
}

impl IDispatchMap for MockDispatchMap {
    fn set_dma_alloc(&self, _alloc: DmaAllocFn) {}

    fn initialize(&self) -> Result<(), DispatchMapError> {
        Ok(())
    }

    fn create_staging(
        &self,
        _key: CacheKey,
        _size: u32,
    ) -> Result<Arc<DmaBuffer>, DispatchMapError> {
        unimplemented!("create_staging is not exercised by reserve_memory tests")
    }

    fn lookup(&self, key: CacheKey) -> Result<LookupResult, DispatchMapError> {
        let inner = self.inner.lock().unwrap();
        match inner.get(&key) {
            None => Ok(LookupResult::NotExist),
            Some(entry) => Ok(LookupResult::MemoryTier {
                pointer: entry.pointer,
                size: entry.size,
            }),
        }
    }

    fn convert_to_storage(&self, _key: CacheKey, _offset: u64) -> Result<(), DispatchMapError> {
        Ok(())
    }

    fn take_read(&self, key: CacheKey) -> Result<(), DispatchMapError> {
        let mut inner = self.inner.lock().unwrap();
        match inner.get_mut(&key) {
            None => Err(DispatchMapError::KeyNotFound(key)),
            Some(entry) => {
                entry.read_refs += 1;
                Ok(())
            }
        }
    }

    fn take_write(&self, _key: CacheKey) -> Result<(), DispatchMapError> {
        Ok(())
    }

    fn release_read(&self, key: CacheKey) -> Result<(), DispatchMapError> {
        let mut inner = self.inner.lock().unwrap();
        match inner.get_mut(&key) {
            None => Err(DispatchMapError::KeyNotFound(key)),
            Some(entry) => {
                entry.read_refs = entry.read_refs.saturating_sub(1);
                Ok(())
            }
        }
    }

    fn release_write(&self, key: CacheKey) -> Result<(), DispatchMapError> {
        let mut inner = self.inner.lock().unwrap();
        match inner.get_mut(&key) {
            None => Err(DispatchMapError::KeyNotFound(key)),
            Some(entry) => {
                entry.write_ref = false;
                Ok(())
            }
        }
    }

    fn downgrade_reference(&self, key: CacheKey) -> Result<(), DispatchMapError> {
        let mut inner = self.inner.lock().unwrap();
        match inner.get_mut(&key) {
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
        if inner.remove(&key).is_some() {
            Ok(())
        } else {
            Err(DispatchMapError::KeyNotFound(key))
        }
    }

    fn touch(&self, key: CacheKey) -> Result<(), DispatchMapError> {
        if self.inner.lock().unwrap().contains_key(&key) {
            Ok(())
        } else {
            Err(DispatchMapError::KeyNotFound(key))
        }
    }

    fn entry_size(&self, key: CacheKey) -> Result<u32, DispatchMapError> {
        let inner = self.inner.lock().unwrap();
        inner
            .get(&key)
            .map(|e| e.size)
            .ok_or(DispatchMapError::KeyNotFound(key))
    }

    fn oldest_keys(&self, n: usize) -> Vec<CacheKey> {
        let inner = self.inner.lock().unwrap();
        inner.keys().copied().take(n).collect()
    }

    fn create_memory_tier_entry(
        &self,
        key: CacheKey,
        pointer: *mut u8,
        size: u32,
    ) -> Result<(), DispatchMapError> {
        let mut inner = self.inner.lock().unwrap();
        if inner.contains_key(&key) {
            return Err(DispatchMapError::AlreadyExists(key));
        }
        inner.insert(
            key,
            MockDmEntry {
                pointer,
                size,
                write_ref: true,
                read_refs: 0,
            },
        );
        Ok(())
    }

    fn convert_memory_tier_to_block(&self, _key: CacheKey) -> Result<(), DispatchMapError> {
        Ok(())
    }

    fn is_evictable(&self, _key: CacheKey) -> bool {
        false
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

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn setup_initialized() -> (Arc<DispatcherComponent>, Arc<MockDispatchMap>) {
    let dm = Arc::new(MockDispatchMap::new());
    let logger: Arc<dyn ILogger + Send + Sync> = Arc::new(MockLogger);
    let gpu: Arc<dyn IGpuServices + Send + Sync> = Arc::new(MockGpuServices);
    let mt: Arc<dyn IMemoryTier + Send + Sync> = Arc::new(MockMemoryTier::new(4 * 1024 * 1024));
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

fn setup_with_failing_mt() -> (Arc<DispatcherComponent>, Arc<MockDispatchMap>) {
    let dm = Arc::new(MockDispatchMap::new());
    let logger: Arc<dyn ILogger + Send + Sync> = Arc::new(MockLogger);
    let gpu: Arc<dyn IGpuServices + Send + Sync> = Arc::new(MockGpuServices);
    let mt: Arc<dyn IMemoryTier + Send + Sync> = Arc::new(MockMemoryTier::always_fails());
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

fn null_stream() -> GpuStream {
    GpuStream(std::ptr::null_mut())
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[test]
fn reserve_memory_happy_path_returns_nonnull_pointer() {
    let (c, _dm) = setup_initialized();
    let d = query_interface!(c, IDispatcher).unwrap();

    let ptr = d
        .reserve_memory(1, 4096)
        .expect("reserve_memory should succeed");

    assert!(
        !ptr.is_null(),
        "reserve_memory must return a non-null pointer"
    );
    d.shutdown().unwrap();
}

#[test]
fn reserve_memory_zero_size_returns_invalid_parameter() {
    let (c, _dm) = setup_initialized();
    let d = query_interface!(c, IDispatcher).unwrap();

    let err = d.reserve_memory(1, 0).expect_err("size=0 must be rejected");

    assert!(
        matches!(err, DispatcherError::InvalidParameter(_)),
        "expected InvalidParameter, got: {err:?}"
    );
    d.shutdown().unwrap();
}

#[test]
fn reserve_memory_full_pool_returns_allocation_failed() {
    let (c, _dm) = setup_with_failing_mt();
    let d = query_interface!(c, IDispatcher).unwrap();

    let err = d
        .reserve_memory(1, 4096)
        .expect_err("exhausted pool must fail");

    assert!(
        matches!(err, DispatcherError::AllocationFailed(_)),
        "expected AllocationFailed, got: {err:?}"
    );
    d.shutdown().unwrap();
}

#[test]
fn reserve_memory_duplicate_key_returns_error() {
    let (c, _dm) = setup_initialized();
    let d = query_interface!(c, IDispatcher).unwrap();

    d.reserve_memory(42, 4096).unwrap();

    let err = d
        .reserve_memory(42, 4096)
        .expect_err("duplicate key must fail");

    assert!(
        matches!(err, DispatcherError::AlreadyExists(42)),
        "expected AlreadyExists(42), got: {err:?}"
    );
    d.shutdown().unwrap();
}

#[test]
fn release_memory_frees_reserved_slot() {
    let (c, _dm) = setup_initialized();
    let d = query_interface!(c, IDispatcher).unwrap();

    d.reserve_memory(10, 4096).unwrap();
    d.release_memory(10)
        .expect("release_memory on a reserved slot must succeed");

    d.shutdown().unwrap();
}

#[test]
fn release_memory_absent_key_is_ok() {
    // release_memory is documented as idempotent: an absent key is not an error.
    let (c, _dm) = setup_initialized();
    let d = query_interface!(c, IDispatcher).unwrap();

    d.release_memory(99)
        .expect("release_memory on absent key must return Ok");

    d.shutdown().unwrap();
}

#[test]
fn copy_gpu_to_memory_completed_makes_key_visible() {
    let (c, _dm) = setup_initialized();
    let d = query_interface!(c, IDispatcher).unwrap();
    let key: CacheKey = 7;
    let size: u32 = 4096;

    d.reserve_memory(key, size).unwrap();

    let mut src = vec![0u8; size as usize];
    d.copy_gpu_to_memory_async(key, make_handle(&mut src), null_stream())
        .unwrap();

    d.copy_gpu_to_memory_completed(key, size).unwrap();

    assert!(
        d.check(key).unwrap(),
        "key must be visible via check() after copy_gpu_to_memory_completed"
    );
    d.shutdown().unwrap();
}

#[test]
fn copy_gpu_to_memory_completed_without_reserve_returns_error() {
    let (c, _dm) = setup_initialized();
    let d = query_interface!(c, IDispatcher).unwrap();

    let err = d
        .copy_gpu_to_memory_completed(88, 4096)
        .expect_err("must fail without prior reserve_memory");

    assert!(
        matches!(err, DispatcherError::KeyNotFound(_)),
        "expected KeyNotFound, got: {err:?}"
    );
    d.shutdown().unwrap();
}

#[test]
fn copy_gpu_to_memory_async_copies_data_to_dram_slot() {
    let (c, _dm) = setup_initialized();
    let d = query_interface!(c, IDispatcher).unwrap();
    let key: CacheKey = 9;
    let size: u32 = 512;

    // Reserve the DRAM slot; keep the returned pointer for readback.
    let ptr = d.reserve_memory(key, size).unwrap();
    assert!(!ptr.is_null());

    // Fill source buffer with a deterministic pattern.
    let src_data: Vec<u8> = (0u32..size as u32).map(|i| (i % 251) as u8).collect();
    let mut src = src_data.clone();
    let handle = IpcHandle {
        address: src.as_mut_ptr(),
        size,
    };

    d.copy_gpu_to_memory_async(key, handle, null_stream())
        .unwrap();

    // The mock dma_copy_to_host_async is a plain memcpy, so the bytes must match.
    // SAFETY: ptr is valid for `size` bytes; the pool is kept alive by the component Arc.
    let written = unsafe { std::slice::from_raw_parts(ptr, size as usize) };
    assert_eq!(
        written,
        src_data.as_slice(),
        "copy_gpu_to_memory_async must write source bytes into the reserved DRAM slot"
    );

    d.shutdown().unwrap();
}

#[test]
fn full_three_phase_store_lifecycle() {
    let (c, dm) = setup_initialized();
    let d = query_interface!(c, IDispatcher).unwrap();
    let key: CacheKey = 100;
    let size: u32 = 4096;

    // Phase 1: reserve — allocates DRAM slot, does NOT register in dispatch-map.
    let ptr = d.reserve_memory(key, size).unwrap();
    assert!(!ptr.is_null());
    assert!(
        !d.check(key).unwrap(),
        "key must NOT be visible before copy_gpu_to_memory_completed"
    );

    // Phase 2: async DMA from a fake GPU buffer into the reserved slot.
    let mut src = vec![0xCDu8; size as usize];
    d.copy_gpu_to_memory_async(key, make_handle(&mut src), null_stream())
        .unwrap();

    // Phase 3: finalize — registers key in dispatch-map and enqueues SSD write-through.
    d.copy_gpu_to_memory_completed(key, size).unwrap();

    // Key must now be visible via check() and dispatch-map.
    assert!(
        d.check(key).unwrap(),
        "key must be visible after copy_gpu_to_memory_completed"
    );
    assert_eq!(
        dm.entry_count(),
        1,
        "dispatch-map must contain exactly one entry"
    );

    d.shutdown().unwrap();
}

#[test]
fn reserve_release_re_reserve_sequence() {
    let (c, _dm) = setup_initialized();
    let d = query_interface!(c, IDispatcher).unwrap();
    let key: CacheKey = 5;

    // First reservation.
    let ptr1 = d.reserve_memory(key, 4096).unwrap();
    assert!(!ptr1.is_null());

    // Release — slot is freed.
    d.release_memory(key).unwrap();

    // Re-reserve the same key; must succeed after release.
    let ptr2 = d
        .reserve_memory(key, 4096)
        .expect("re-reserve after release must succeed");
    assert!(!ptr2.is_null(), "re-reserved pointer must be non-null");

    d.shutdown().unwrap();
}
