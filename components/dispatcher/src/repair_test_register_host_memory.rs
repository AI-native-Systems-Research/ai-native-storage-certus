//! Repair test for finding `host-memory-registered-only-with-spdk`
//! (DIS-INITIALIZE-REGISTER-HOST-MEMORY, FR-034).
//!
//! The pool registration was nested inside `if self.spdk_env.is_connected()`
//! (and the whole GPU block inside the drives branch), so a dispatcher whose GPU
//! and memory tier are ready but that has no SPDK environment connected —
//! factory-mode and memory-tier-only deployments — never pinned the pool for
//! zero-copy access; and on the SPDK path a registration failure was logged at
//! info level, not as the warning FR-034 demands.
//!
//! Self-contained: it drives only the public `IDispatcher` API through its own
//! minimal mocks, so it builds against the dispatcher as it was before the
//! repair as well as after it.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use component_core::query_interface;
use interfaces::{
    CacheKey, DispatchMapError, DispatcherConfig, DmaBuffer, GpuDeviceInfo, GpuDmaBuffer,
    GpuIpcHandle, GpuStream, IDispatchMap, IDispatcher, IGpuServices, ILogger, IMemoryTier,
    LookupResult, MemoryTierError, MemoryTierTelemetrySnapshot,
};

use crate::DispatcherComponent;

const POOL_BYTES: usize = 1024 * 1024;

// --- dispatch map: empty; initialize in memory-tier-only mode touches it not ---

struct EmptyDm;

impl IDispatchMap for EmptyDm {
    fn initialize(&self) -> Result<(), DispatchMapError> {
        Ok(())
    }
    fn lookup(&self, _key: CacheKey) -> Result<LookupResult, DispatchMapError> {
        Ok(LookupResult::NotExist)
    }
    fn convert_to_storage(&self, _key: CacheKey, _offset: u64) -> Result<(), DispatchMapError> {
        Ok(())
    }
    fn take_read(&self, _key: CacheKey) -> Result<(), DispatchMapError> {
        Ok(())
    }
    fn take_write(&self, _key: CacheKey) -> Result<(), DispatchMapError> {
        Ok(())
    }
    fn release_read(&self, _key: CacheKey) -> Result<(), DispatchMapError> {
        Ok(())
    }
    fn release_write(&self, _key: CacheKey) -> Result<(), DispatchMapError> {
        Ok(())
    }
    fn downgrade_reference(&self, _key: CacheKey) -> Result<(), DispatchMapError> {
        Ok(())
    }
    fn remove(&self, _key: CacheKey) -> Result<(), DispatchMapError> {
        Ok(())
    }
    fn touch(&self, _key: CacheKey) -> Result<(), DispatchMapError> {
        Ok(())
    }
    fn entry_size(&self, _key: CacheKey) -> Result<u32, DispatchMapError> {
        Ok(4096)
    }
    fn oldest_keys(&self, _n: usize) -> Vec<CacheKey> {
        Vec::new()
    }
    fn create_memory_tier_entry(
        &self,
        _key: CacheKey,
        _pointer: *mut u8,
        _size: u32,
    ) -> Result<(), DispatchMapError> {
        Ok(())
    }
    fn convert_memory_tier_to_block(&self, _key: CacheKey) -> Result<(), DispatchMapError> {
        Ok(())
    }
    fn promote_block_to_memory_tier(
        &self,
        _key: CacheKey,
        _pointer: *mut u8,
        _size: u32,
    ) -> Result<(), DispatchMapError> {
        Ok(())
    }
    fn is_evictable(&self, _key: CacheKey) -> bool {
        false
    }
    fn try_evict_to_block(&self, _key: CacheKey) -> Result<(), DispatchMapError> {
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

// --- memory tier: a flat pool whose base pointer and length are observable ---

struct PoolMt {
    inner: Mutex<(Vec<u8>, HashMap<CacheKey, (usize, u32)>)>,
}

impl PoolMt {
    fn new() -> Self {
        Self {
            inner: Mutex::new((vec![0u8; POOL_BYTES], HashMap::new())),
        }
    }
    /// The pool's base pointer and length (the trait's `pool_info`, under a
    /// distinct name so the trait impl can delegate without recursing).
    fn pool_range(&self) -> (*mut u8, usize) {
        let g = self.inner.lock().unwrap();
        (g.0.as_ptr() as *mut u8, g.0.len())
    }
}

impl IMemoryTier for PoolMt {
    fn initialize(&self, _pool_size: usize, _numa: Option<i32>) -> Result<(), MemoryTierError> {
        Ok(())
    }
    fn insert(&self, key: CacheKey, _size: u32) -> Result<*mut u8, MemoryTierError> {
        Err(MemoryTierError::AlreadyExists(key))
    }
    fn get(&self, _key: CacheKey) -> Option<(*mut u8, u32)> {
        None
    }
    fn peek(&self, _key: CacheKey) -> Option<(*mut u8, u32)> {
        None
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
        Err(MemoryTierError::KeyNotFound(key))
    }
    fn touch(&self, _key: CacheKey) {}
    fn batch_touch(&self, _keys: &[CacheKey]) {}
    fn contains(&self, _key: CacheKey) -> bool {
        false
    }
    fn capacity(&self) -> usize {
        POOL_BYTES
    }
    fn used(&self) -> usize {
        0
    }
    fn pool_info(&self) -> Option<(*mut u8, usize)> {
        Some(self.pool_range())
    }
    fn is_dma_capable(&self) -> bool {
        false
    }
    fn clear(&self) -> Result<usize, MemoryTierError> {
        Ok(0)
    }
    fn telemetry_snapshot(&self) -> MemoryTierTelemetrySnapshot {
        MemoryTierTelemetrySnapshot::default()
    }
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

// --- GPU services: records host-memory (un)registrations, can be told to fail ---

#[derive(Default)]
struct RegGpu {
    /// (ptr, size) of every `register_host_memory` call.
    registered: Mutex<Vec<(usize, usize)>>,
    /// Every `unregister_host_memory` call's (ptr, size).
    unregistered: Mutex<Vec<(usize, usize)>>,
    /// Make `register_host_memory` fail (CUDA/SPDK refusal).
    fail_registration: bool,
}

impl IGpuServices for RegGpu {
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
        _src: *const std::ffi::c_void,
        _dst: &DmaBuffer,
        _size: usize,
    ) -> Result<(), String> {
        Ok(())
    }
    fn dma_copy_to_device(
        &self,
        _src: &DmaBuffer,
        _dst: *mut std::ffi::c_void,
        _size: usize,
    ) -> Result<(), String> {
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
        _src: &DmaBuffer,
        _dst: *mut std::ffi::c_void,
        _size: usize,
        _stream: GpuStream,
    ) -> Result<(), String> {
        Ok(())
    }
    fn memcpy_h2d_async(
        &self,
        _src: *const std::ffi::c_void,
        _dst: *mut std::ffi::c_void,
        _size: usize,
        _stream: GpuStream,
    ) -> Result<(), String> {
        Ok(())
    }
    fn dma_copy_to_host_async(
        &self,
        _src: *const std::ffi::c_void,
        _dst: &DmaBuffer,
        _size: usize,
        _stream: GpuStream,
    ) -> Result<(), String> {
        Ok(())
    }
    fn memcpy_d2h_async(
        &self,
        _src: *const std::ffi::c_void,
        _dst: *mut std::ffi::c_void,
        _size: usize,
        _stream: GpuStream,
    ) -> Result<(), String> {
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
    fn allocate_pinned_dma_buffer(&self, _size: usize) -> Result<DmaBuffer, String> {
        Err("mock has no DMA buffers".into())
    }
    fn register_host_memory(&self, ptr: *mut std::ffi::c_void, size: usize) -> Result<(), String> {
        self.registered.lock().unwrap().push((ptr as usize, size));
        if self.fail_registration {
            Err("cudaHostRegister failed (mock)".into())
        } else {
            Ok(())
        }
    }
    fn unregister_host_memory(
        &self,
        ptr: *mut std::ffi::c_void,
        size: usize,
    ) -> Result<(), String> {
        self.unregistered.lock().unwrap().push((ptr as usize, size));
        Ok(())
    }
}

// --- logger: keeps the messages, so a test can tell warn from info ---

#[derive(Default)]
struct RecLogger {
    warns: Mutex<Vec<String>>,
    infos: Mutex<Vec<String>>,
}

impl ILogger for RecLogger {
    fn error(&self, _msg: &str) {}
    fn warn(&self, msg: &str) {
        self.warns.lock().unwrap().push(msg.to_string());
    }
    fn info(&self, msg: &str) {
        self.infos.lock().unwrap().push(msg.to_string());
    }
    fn debug(&self, _msg: &str) {}
}

/// A dispatcher whose GPU and memory tier are ready but that has no SPDK
/// environment and no block-device factory: memory-tier-only mode.
fn build(
    fail_registration: bool,
) -> (
    Arc<dyn IDispatcher + Send + Sync>,
    Arc<RegGpu>,
    Arc<RecLogger>,
    Arc<PoolMt>,
) {
    let dm = Arc::new(EmptyDm);
    let mt = Arc::new(PoolMt::new());
    let gpu = Arc::new(RegGpu {
        fail_registration,
        ..RegGpu::default()
    });
    let logger = Arc::new(RecLogger::default());
    let c = DispatcherComponent::new_default();
    c.dispatch_map
        .connect(Arc::clone(&dm) as Arc<dyn IDispatchMap + Send + Sync>)
        .unwrap();
    c.logger
        .connect(Arc::clone(&logger) as Arc<dyn ILogger + Send + Sync>)
        .unwrap();
    c.gpu_services
        .connect(Arc::clone(&gpu) as Arc<dyn IGpuServices + Send + Sync>)
        .unwrap();
    c.memory_tier
        .connect(Arc::clone(&mt) as Arc<dyn IMemoryTier + Send + Sync>)
        .unwrap();
    let d = query_interface!(c, IDispatcher).unwrap();
    (d, gpu, logger, mt)
}

fn config() -> DispatcherConfig {
    DispatcherConfig {
        data_pci_addrs: vec!["0000:02:00.0".to_string()],
        ..Default::default()
    }
}

/// FR-034: with the GPU and the memory tier ready, `initialize` registers the
/// memory-tier pool with the GPU services — with no SPDK environment connected,
/// which is exactly the deployment the old code skipped.
#[test]
fn initialize_registers_pool_when_gpu_and_tier_ready_without_spdk() {
    let (d, gpu, _logger, mt) = build(false);
    d.initialize(config()).unwrap();

    let (pool_ptr, pool_size) = mt.pool_range();
    assert_eq!(
        gpu.registered.lock().unwrap().as_slice(),
        &[(pool_ptr as usize, pool_size)],
        "initialize must register the memory-tier pool with the GPU services \
         once the GPU and the memory tier are ready (FR-034), SPDK or not"
    );

    // The matching teardown (FR-035) unregisters the same range.
    d.shutdown().unwrap();
    assert_eq!(
        gpu.unregistered.lock().unwrap().as_slice(),
        &[(pool_ptr as usize, pool_size)],
        "shutdown must unregister what initialize registered"
    );
}

/// FR-034: a registration failure is reported as a warning and does not fail
/// initialization.
#[test]
fn registration_failure_is_a_warning_and_initialization_succeeds() {
    let (d, gpu, logger, mt) = build(true);
    d.initialize(config())
        .expect("a registration failure must not fail initialization (FR-034)");

    let (pool_ptr, pool_size) = mt.pool_range();
    assert_eq!(
        gpu.registered.lock().unwrap().as_slice(),
        &[(pool_ptr as usize, pool_size)],
        "the pool registration must still have been attempted"
    );
    let warns = logger.warns.lock().unwrap();
    assert!(
        warns
            .iter()
            .any(|m| m.contains("registration") && m.contains("failed")),
        "a registration failure must be logged as a warning (FR-034); warns: {warns:?}"
    );

    d.shutdown().unwrap();
}
