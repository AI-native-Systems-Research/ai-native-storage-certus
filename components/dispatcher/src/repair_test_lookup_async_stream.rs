//! Repair test for finding `lookup-async-sync-null-stream`
//! (DIS-LOOKUP-ASYNC-WARM-STREAM, DIS-LOOKUP-ASYNC-DEVICE-STREAM).
//!
//! For a memory-tier (warm) hit, `lookup_async` must queue the H2D copy
//! asynchronously and return the CUDA stream it was queued on, so the caller
//! can `stream_synchronize` that stream before touching the destination
//! (spec 001 FR-036, `IDispatcher::lookup_async` doc). When the destination's
//! GPU device is known, the copy goes out on a stream bound to that device
//! (the per-device warm stream, FR-037/FR-052); only when the device is
//! unknown does it fall back to the shared warm stream.
//!
//! On the unfixed code the warm hit is copied synchronously inside the call
//! and a null stream is returned, and an unknown device degrades to the fully
//! synchronous `DmaBuffer` path instead of the shared warm stream.
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

// --- dispatch map: entries are fixed memory-tier locations for this test ---

#[derive(Clone, Copy)]
enum Loc {
    // Pointer stored as usize so the mock stays Send + Sync without an unsafe impl.
    Memory(usize, u32),
}

struct Ent {
    loc: Loc,
    read_refs: u32,
    write_ref: bool,
}

#[derive(Default)]
struct WarmDm {
    entries: Mutex<HashMap<CacheKey, Ent>>,
}

impl IDispatchMap for WarmDm {
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
            .ok_or(DispatchMapError::NoWriteReference(key))?;
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

// --- GPU services: host memcpy stands in for every DMA, with recording ---

/// Everything the mock observed, in order.
#[derive(Default, Debug, Clone)]
struct GpuObs {
    /// The device `set_device` made current.
    current_device: i32,
    next_stream: usize,
    /// `(stream, device current at creation)` per `create_stream`.
    created: Vec<(usize, i32)>,
    /// `(dst, stream)` per `memcpy_h2d_async`.
    h2d: Vec<(usize, usize)>,
    /// `(stream)` per `stream_synchronize`.
    syncs: Vec<usize>,
    /// Count of synchronous `dma_copy_to_device` calls.
    dma_to_device: usize,
}

struct RecGpu {
    obs: Mutex<GpuObs>,
    /// The `device_of_ptr` answer this instance is configured with.
    dev_answer: Result<i32, String>,
}

impl RecGpu {
    fn new(dev_answer: Result<i32, String>) -> Self {
        Self {
            obs: Mutex::new(GpuObs::default()),
            dev_answer,
        }
    }

    fn snapshot(&self) -> GpuObs {
        self.obs.lock().unwrap().clone()
    }
}

fn host_copy(src: *const u8, dst: *mut u8, size: usize) -> Result<(), String> {
    // SAFETY: in this test every src/dst is a live host buffer of at least `size` bytes.
    unsafe { std::ptr::copy_nonoverlapping(src, dst, size) };
    Ok(())
}

impl IGpuServices for RecGpu {
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
        self.obs.lock().unwrap().dma_to_device += 1;
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
        let mut o = self.obs.lock().unwrap();
        o.next_stream += 1;
        let raw = 0x1000 + o.next_stream;
        let dev = o.current_device;
        o.created.push((raw, dev));
        Ok(GpuStream(raw as *mut std::ffi::c_void))
    }
    fn set_device(&self, device: i32) -> Result<(), String> {
        self.obs.lock().unwrap().current_device = device;
        Ok(())
    }
    fn device_of_ptr(&self, _ptr: *const std::ffi::c_void) -> Result<i32, String> {
        self.dev_answer.clone()
    }
    fn stream_query(&self, _s: GpuStream) -> Result<bool, String> {
        Ok(true)
    }
    fn destroy_stream(&self, _s: GpuStream) -> Result<(), String> {
        Ok(())
    }
    fn stream_synchronize(&self, s: GpuStream) -> Result<(), String> {
        self.obs.lock().unwrap().syncs.push(s.0 as usize);
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
        s: GpuStream,
    ) -> Result<(), String> {
        self.obs
            .lock()
            .unwrap()
            .h2d
            .push((dst as usize, s.0 as usize));
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

/// Wire a dispatcher over the mocks and return the `IDispatcher` plus the
/// maps and GPU mock backing it. `dev_answer` is what `device_of_ptr` answers
/// for the caller's destination pointer (the destination's device, or an
/// error meaning "unknown"). `shared_warm` optionally pre-seeds the shared
/// warm stream the way a hardware (SPDK) `initialize` would have created it —
/// the staging-only `initialize` of this harness creates none.
fn harness(
    dev_answer: Result<i32, String>,
    shared_warm: Option<usize>,
) -> (
    Arc<dyn IDispatcher + Send + Sync>,
    Arc<WarmDm>,
    Arc<PoolMt>,
    Arc<RecGpu>,
) {
    let dm = Arc::new(WarmDm::default());
    let mt = Arc::new(PoolMt::new(1024 * 1024));
    let gpu = Arc::new(RecGpu::new(dev_answer));
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
    if let Some(raw) = shared_warm {
        c.warm_stream
            .store(raw as u64, std::sync::atomic::Ordering::Release);
    }
    let d = query_interface!(c, IDispatcher).unwrap();
    // No ISPDKEnv bound: no data drives, irrelevant for a warm hit.
    d.initialize(DispatcherConfig {
        data_pci_addrs: vec!["0000:02:00.0".to_string()],
        ..Default::default()
    })
    .unwrap();
    (d, dm, mt, gpu)
}

/// Install `key` as a memory-tier hit whose slot holds `bytes`.
fn seed_warm(dm: &WarmDm, mt: &PoolMt, key: CacheKey, bytes: &[u8]) {
    let slot = mt.insert(key, bytes.len() as u32).unwrap();
    // SAFETY: `slot` is a fresh slot of exactly `bytes.len()` bytes.
    unsafe { std::ptr::copy_nonoverlapping(bytes.as_ptr(), slot, bytes.len()) };
    dm.entries.lock().unwrap().insert(
        key,
        Ent {
            loc: Loc::Memory(slot as usize, bytes.len() as u32),
            read_refs: 0,
            write_ref: false,
        },
    );
}

/// DIS-LOOKUP-ASYNC-WARM-STREAM + DIS-LOOKUP-ASYNC-DEVICE-STREAM, destination
/// device known: the copy is queued (not synchronized in-call) on the warm
/// stream bound to the destination's device, and that stream is returned.
#[test]
fn warm_hit_returns_the_device_stream_the_copy_was_queued_on() {
    // Device 5: no other test's mock resolves a pointer to it, so the
    // process-global DEVICE_STREAMS entry is created by this mock alone.
    let (d, dm, mt, gpu) = harness(Ok(5), None);
    seed_warm(&dm, &mt, 42, &[0xA0, 0xA1, 0xA2, 0xA3]);
    let mut dst = vec![0u8; 4];
    let handle = IpcHandle {
        address: dst.as_mut_ptr(),
        size: 4,
    };
    let stream = d.lookup_async(42, handle).expect("warm hit must be served");
    assert!(
        !stream.0.is_null(),
        "lookup_async must return the stream the copy was queued on, not the null stream"
    );

    let obs = gpu.snapshot();
    // Exactly one H2D copy, queued on the stream handed to the caller.
    assert_eq!(obs.h2d.len(), 1, "one region, one H2D copy: {obs:?}");
    assert_eq!(
        obs.h2d[0].1, stream.0 as usize,
        "the copy must be queued on the stream returned to the caller"
    );
    assert_eq!(
        obs.h2d[0].0,
        dst.as_ptr() as usize,
        "the copy must target the caller's destination"
    );
    // The stream is bound to the device that owns the destination pointer.
    let bound_dev = obs
        .created
        .iter()
        .find(|(s, _)| *s == stream.0 as usize)
        .map(|(_, dev)| *dev)
        .expect("the returned stream must be one this mock created");
    assert_eq!(
        bound_dev, 5,
        "the warm stream must be bound to the destination's device"
    );
    // The call must not block on the copy: no synchronize before the caller
    // synchronizes the returned stream.
    assert!(
        obs.syncs.is_empty(),
        "lookup_async must not synchronize internally; the caller synchronizes the returned stream: {obs:?}"
    );
    assert_eq!(
        obs.dma_to_device, 0,
        "a device stream exists, so the copy must not degrade to the synchronous DmaBuffer path"
    );

    // After the caller synchronizes the returned stream, the destination
    // holds the cached data.
    gpu.stream_synchronize(stream).unwrap();
    assert_eq!(dst, vec![0xA0, 0xA1, 0xA2, 0xA3]);

    d.shutdown().unwrap();
}

/// DIS-LOOKUP-ASYNC-DEVICE-STREAM, destination device unknown: the copy falls
/// back to the shared warm stream (the one a hardware initialize allocates;
/// pre-seeded here because the staging-only initialize allocates none),
/// queued asynchronously on it, and that stream is returned.
#[test]
fn warm_hit_unknown_device_falls_back_to_shared_warm_stream() {
    // A raw value no mock create_stream would ever hand out (they start at
    // 0x1001), so a confused assert cannot pass by coincidence.
    let shared = 0x2000;
    let (d, dm, mt, gpu) = harness(Err("mock: device of pointer unknown".into()), Some(shared));
    seed_warm(&dm, &mt, 7, &[0xB0, 0xB1]);
    let mut dst = vec![0u8; 2];
    let handle = IpcHandle {
        address: dst.as_mut_ptr(),
        size: 2,
    };
    let stream = d.lookup_async(7, handle).expect("warm hit must be served");

    let obs = gpu.snapshot();
    assert!(
        !stream.0.is_null(),
        "even with the device unknown, the shared warm stream must be used and returned"
    );
    assert_eq!(
        stream.0 as usize, shared,
        "lookup_async must return the shared warm stream it queued the copy on"
    );
    assert_eq!(
        obs.h2d.len(),
        1,
        "the copy must be issued as an async H2D on the shared warm stream: {obs:?}"
    );
    assert_eq!(
        obs.h2d[0].1, shared,
        "the copy must be queued on the shared warm stream"
    );
    assert_eq!(
        obs.dma_to_device, 0,
        "with the device unknown the path must fall back to the shared warm stream, not the synchronous DmaBuffer copy"
    );
    assert!(
        obs.syncs.is_empty(),
        "lookup_async must not synchronize internally: {obs:?}"
    );

    gpu.stream_synchronize(stream).unwrap();
    assert_eq!(dst, vec![0xB0, 0xB1]);

    d.shutdown().unwrap();
}

/// FR-036: the synchronous `lookup` delegates to `lookup_async` and
/// synchronizes the returned stream before returning, so its result is
/// unchanged by the async warm path.
#[test]
fn sync_lookup_still_delivers_after_delegation() {
    // Device 6: distinct from the device the first test uses, so the two
    // tests cannot race on the process-global DEVICE_STREAMS entry (whoever
    // creates it first owns the mock that recorded its creation).
    let (d, dm, mt, gpu) = harness(Ok(6), None);
    seed_warm(&dm, &mt, 9, &[0xC0, 0xC1]);
    let mut dst = vec![0u8; 2];
    let handle = IpcHandle {
        address: dst.as_mut_ptr(),
        size: 2,
    };
    d.lookup(9, handle).expect("warm hit must be served");
    let obs = gpu.snapshot();
    assert!(
        !obs.syncs.is_empty(),
        "lookup must synchronize the stream lookup_async returned"
    );
    assert_eq!(dst, vec![0xC0, 0xC1]);

    d.shutdown().unwrap();
}
