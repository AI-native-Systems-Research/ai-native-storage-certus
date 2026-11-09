//! Repair test for finding `route-partition-does-not-add-up`
//! (DIS-TIER-STATS-ROUTE-PARTITION).
//!
//! FR-058 / SC-017: `lookup_hits_dram + lookup_hits_ssd + remote_lookup_hits`
//! partitions every served key by route — over any interval the three MUST sum
//! to the number of keys the lookups served in that interval, matching the
//! per-key `ServedBy` attributions the same batches return.
//!
//! `batch_lookup` records its half of the partition (derived from the per-key
//! attributions), but the single-key path `lookup`/`lookup_async` serves keys
//! (it bumps `promotions_to_gpu`) without ever bumping `lookup_hits_dram` /
//! `lookup_hits_ssd`. On the unfixed code, any interval containing a
//! successful single-key lookup undercounts the partition, so the route
//! counters and the served count disagree.
//!
//! Self-contained: it drives only the public `IDispatcher` API through its own
//! minimal mocks, so it builds against the unfixed code as well.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use component_core::query_interface;
use interfaces::{
    CacheKey, DispatchMapError, DispatcherConfig, DmaBuffer, GpuDeviceInfo, GpuDmaBuffer,
    GpuIpcHandle, GpuStream, IDispatchMap, IDispatcher, IGpuServices, ILogger, IMemoryTier,
    IpcHandle, LookupOutcome, LookupResult, MemoryTierError, MemoryTierTelemetrySnapshot, ServedBy,
    TierEventStats,
};

use crate::DispatcherComponent;

// --- dispatch map: fixed locations, with an optional flip to a memory-tier
// --- location after N reads (the concurrent-promotion recovery scenario) ---

#[derive(Clone, Copy)]
enum Loc {
    // Pointer stored as usize so the mock stays Send + Sync without an unsafe impl.
    Memory(usize, u32),
    Block(u64),
}

struct Ent {
    loc: Loc,
    read_refs: u32,
    write_ref: bool,
    /// Number of `lookup` calls so far.
    reads: usize,
    /// From the (at+1)-th `lookup` on, answer `loc` instead of the stored one —
    /// models a concurrent promotion landing between two of this test's reads.
    flip_at: Option<usize>,
}

#[derive(Default)]
struct MockDm {
    entries: Mutex<HashMap<CacheKey, Ent>>,
}

impl MockDm {
    fn current_loc(e: &Ent) -> Loc {
        match e.flip_at {
            Some(at) if e.reads > at => e.loc,
            _ => e.loc,
        }
    }
}

impl IDispatchMap for MockDm {
    fn initialize(&self) -> Result<(), DispatchMapError> {
        Ok(())
    }
    fn lookup(&self, key: CacheKey) -> Result<LookupResult, DispatchMapError> {
        let mut entries = self.entries.lock().unwrap();
        let Some(e) = entries.get_mut(&key) else {
            return Ok(LookupResult::NotExist);
        };
        e.reads += 1;
        Ok(match Self::current_loc(e) {
            Loc::Memory(p, size) => LookupResult::MemoryTier {
                pointer: p as *mut u8,
                size,
            },
            Loc::Block(off) => LookupResult::BlockDevice { offset: off },
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
                reads: 0,
                flip_at: None,
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
        Ok(GpuStream(std::ptr::null_mut()))
    }
    fn set_device(&self, _device: i32) -> Result<(), String> {
        Ok(())
    }
    fn device_of_ptr(&self, _ptr: *const std::ffi::c_void) -> Result<i32, String> {
        // Unknown device: the serve path degrades to the synchronous
        // DmaBuffer copy, which the host memcpy above stands in for.
        Err("mock: device of pointer unknown".into())
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

/// Wire a dispatcher over the mocks (staging-only: no data drives) and return
/// the `IDispatcher` plus the backing maps.
fn harness() -> (Arc<dyn IDispatcher + Send + Sync>, Arc<MockDm>, Arc<PoolMt>) {
    let dm = Arc::new(MockDm::default());
    let mt = Arc::new(PoolMt::new(1024 * 1024));
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
    // No ISPDKEnv bound: no data drives, so a block-device entry is served by
    // the memory-tier-only promote path.
    d.initialize(DispatcherConfig {
        data_pci_addrs: vec!["0000:02:00.0".to_string()],
        ..Default::default()
    })
    .unwrap();
    (d, dm, mt)
}

/// Install `key` as a memory-tier hit whose slot holds `bytes`.
fn seed_warm(dm: &MockDm, mt: &PoolMt, key: CacheKey, bytes: &[u8]) {
    let slot = mt.insert(key, bytes.len() as u32).unwrap();
    // SAFETY: `slot` is a fresh slot of exactly `bytes.len()` bytes.
    unsafe { std::ptr::copy_nonoverlapping(bytes.as_ptr(), slot, bytes.len()) };
    dm.entries.lock().unwrap().insert(
        key,
        Ent {
            loc: Loc::Memory(slot as usize, bytes.len() as u32),
            read_refs: 0,
            write_ref: false,
            reads: 0,
            flip_at: None,
        },
    );
}

/// Install `key` as a block-device (SSD-resident) entry.
fn seed_cold(dm: &MockDm, key: CacheKey) {
    dm.entries.lock().unwrap().insert(
        key,
        Ent {
            loc: Loc::Block(4096),
            read_refs: 0,
            write_ref: false,
            reads: 0,
            flip_at: None,
        },
    );
}

fn route_sum(st: &TierEventStats) -> u64 {
    st.lookup_hits_dram + st.lookup_hits_ssd + st.remote_lookup_hits
}

/// DIS-TIER-STATS-ROUTE-PARTITION, single-key warm hit: a key served by the
/// synchronous `lookup` (memory-tier route) must appear in the partition —
/// `lookup_hits_dram` up by one, the route sum up by one, tracking
/// `promotions_to_gpu` (the served count) exactly.
#[test]
fn single_key_warm_hit_is_counted_in_the_route_partition() {
    let (d, dm, mt) = harness();
    seed_warm(&dm, &mt, 42, &[0xA0, 0xA1, 0xA2, 0xA3]);
    let mut dst = vec![0u8; 4];
    let handle = IpcHandle {
        address: dst.as_mut_ptr(),
        size: 4,
    };

    let before = d.tier_event_stats();
    d.lookup(42, handle).expect("warm hit must be served");
    let after = d.tier_event_stats();

    assert_eq!(dst, vec![0xA0, 0xA1, 0xA2, 0xA3], "the data must arrive");
    assert_eq!(
        after.lookup_hits_dram - before.lookup_hits_dram,
        1,
        "a single-key memory-tier hit is a DRAM-route hit"
    );
    assert_eq!(
        after.lookup_hits_ssd - before.lookup_hits_ssd,
        0,
        "a memory-tier hit must not touch the SSD counter"
    );
    assert_eq!(
        after.remote_lookup_hits - before.remote_lookup_hits,
        0,
        "a local hit must not touch the remote counter"
    );
    assert_eq!(
        route_sum(&after) - route_sum(&before),
        1,
        "the route partition must count the served key"
    );
    assert_eq!(
        after.promotions_to_gpu - before.promotions_to_gpu,
        1,
        "the served key is counted here, so the partition must match it"
    );

    d.shutdown().unwrap();
}

/// DIS-TIER-STATS-ROUTE-PARTITION, single-key cold hit: a block-device entry
/// served by the synchronous `lookup` (promote-and-serve route, no drives) is
/// an SSD-route hit — `lookup_hits_ssd` up by one.
#[test]
fn single_key_cold_hit_is_counted_in_the_route_partition() {
    let (d, dm, _mt) = harness();
    seed_cold(&dm, 7);
    let mut dst = vec![0u8; 4];
    let handle = IpcHandle {
        address: dst.as_mut_ptr(),
        size: 4,
    };

    let before = d.tier_event_stats();
    d.lookup(7, handle).expect("cold hit must be served");
    let after = d.tier_event_stats();

    assert_eq!(
        after.lookup_hits_ssd - before.lookup_hits_ssd,
        1,
        "a single-key block-device serve is an SSD-route hit"
    );
    assert_eq!(
        after.lookup_hits_dram - before.lookup_hits_dram,
        0,
        "a cold serve must not touch the DRAM counter"
    );
    assert_eq!(
        route_sum(&after) - route_sum(&before),
        1,
        "the route partition must count the served key"
    );
    assert_eq!(
        after.promotions_to_gpu - before.promotions_to_gpu,
        1,
        "the served key is counted here, so the partition must match it"
    );

    d.shutdown().unwrap();
}

/// DIS-TIER-STATS-ROUTE-PARTITION, single-key concurrent-promotion recovery:
/// the cold lookup lost the `mt.insert` race, and the recovery served the key
/// out of DRAM — the DRAM-route counter, exactly the attribution
/// `batch_lookup` derives for the same situation (lib.rs recovery pass).
#[test]
fn single_key_recovery_hit_is_a_dram_route_hit() {
    let (d, dm, mt) = harness();
    // The memory tier already holds the key (the concurrent promotion that
    // won the race); the dispatch map still answers BlockDevice for the first
    // read and MemoryTier from the second on.
    let slot = mt.insert(9, 2).unwrap();
    // SAFETY: `slot` is a fresh slot of exactly 2 bytes.
    unsafe { std::ptr::copy_nonoverlapping([0xC0u8, 0xC1].as_ptr(), slot, 2) };
    dm.entries.lock().unwrap().insert(
        9,
        Ent {
            loc: Loc::Memory(slot as usize, 2),
            read_refs: 0,
            write_ref: false,
            reads: 0,
            flip_at: Some(1),
        },
    );
    let mut dst = vec![0u8; 2];
    let handle = IpcHandle {
        address: dst.as_mut_ptr(),
        size: 2,
    };

    let before = d.tier_event_stats();
    d.lookup(9, handle).expect("recovery hit must be served");
    let after = d.tier_event_stats();

    assert_eq!(dst, vec![0xC0, 0xC1], "the winner's slot must be served");
    assert_eq!(
        after.lookup_hits_dram - before.lookup_hits_dram,
        1,
        "the recovery serves out of DRAM, so it is a DRAM-route hit (as in batch_lookup)"
    );
    assert_eq!(
        after.lookup_hits_ssd - before.lookup_hits_ssd,
        0,
        "no drive was read, so the SSD counter must not move"
    );
    assert_eq!(
        route_sum(&after) - route_sum(&before),
        1,
        "the route partition must count the served key"
    );

    d.shutdown().unwrap();
}

/// DIS-TIER-STATS-ROUTE-PARTITION, miss: a key no tier holds moves none of the
/// three route counters (the partition must not overcount either).
#[test]
fn single_key_miss_moves_no_route_counter() {
    let (d, _dm, _mt) = harness();
    let mut dst = vec![0u8; 4];
    let handle = IpcHandle {
        address: dst.as_mut_ptr(),
        size: 4,
    };

    let before = d.tier_event_stats();
    let miss = d.lookup(404, handle);
    let after = d.tier_event_stats();

    assert!(miss.is_err(), "an absent key cannot be served");
    assert_eq!(
        route_sum(&after) - route_sum(&before),
        0,
        "no key was served, so no route counter may move"
    );
    assert_eq!(
        after.promotions_to_gpu - before.promotions_to_gpu,
        0,
        "no key was served"
    );

    d.shutdown().unwrap();
}

/// DIS-TIER-STATS-ROUTE-PARTITION over a mixed interval: one single-key warm
/// lookup plus one batch (a warm hit and a miss). The increase of the three
/// route counters over the whole interval must equal the number of keys served
/// (2), tracking `promotions_to_gpu`, and must match the per-key attributions
/// the batch returned.
#[test]
fn mixed_interval_partition_adds_up_and_matches_attributions() {
    let (d, dm, mt) = harness();
    seed_warm(&dm, &mt, 100, &[0xD0, 0xD1]);
    seed_warm(&dm, &mt, 200, &[0xE0, 0xE1]);
    let mut dst_a = vec![0u8; 2];
    let mut dst_b = vec![0u8; 2];
    let mut dst_c = vec![0u8; 2];

    let before = d.tier_event_stats();
    // One single-key lookup (warm).
    d.lookup(
        100,
        IpcHandle {
            address: dst_a.as_mut_ptr(),
            size: 2,
        },
    )
    .expect("warm hit must be served");
    // One batch: a warm hit and a miss.
    let batch = vec![
        (
            200,
            vec![IpcHandle {
                address: dst_b.as_mut_ptr(),
                size: 2,
            }],
        ),
        (
            404,
            vec![IpcHandle {
                address: dst_c.as_mut_ptr(),
                size: 2,
            }],
        ),
    ];
    let outcomes: Vec<LookupOutcome> = d.batch_lookup(&batch);
    let after = d.tier_event_stats();

    assert_eq!(outcomes.len(), 2);
    assert_eq!(outcomes[0].served_by, ServedBy::Dram);
    assert!(outcomes[0].result.is_ok());
    assert_eq!(outcomes[1].served_by, ServedBy::Miss);
    assert!(outcomes[1].result.is_err());

    let served = outcomes.iter().filter(|o| o.result.is_ok()).count() as u64 + 1;
    assert_eq!(served, 2);
    assert_eq!(
        after.promotions_to_gpu - before.promotions_to_gpu,
        served,
        "every served key counts here (single-key and batch alike)"
    );
    assert_eq!(
        route_sum(&after) - route_sum(&before),
        served,
        "the route partition must equal the served count over the interval"
    );
    // And the local half matches the batch's own per-key attributions.
    assert_eq!(
        after.lookup_hits_dram - before.lookup_hits_dram,
        (outcomes
            .iter()
            .filter(|o| o.served_by == ServedBy::Dram)
            .count() as u64)
            + 1,
        "DRAM-route hits: the batch's Dram attributions plus the single-key warm hit"
    );
    assert_eq!(
        after.lookup_hits_ssd - before.lookup_hits_ssd,
        outcomes
            .iter()
            .filter(|o| o.served_by == ServedBy::Ssd)
            .count() as u64,
        "SSD-route hits: the batch's Ssd attributions"
    );

    d.shutdown().unwrap();
}
