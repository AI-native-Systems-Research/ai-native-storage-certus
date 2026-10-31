//! Repair test for finding `transient-map-error-becomes-keynotfound`
//! (DIS-BATCH-LOOKUP-TRANSIENT-RETRY, FR-039 step 1).
//!
//! `batch_lookup`'s classification loop retries a `dm.lookup` that answers
//! `Err` (a writer holds the entry's write reference past the map's 2 s
//! `DEFAULT_TIMEOUT`) up to five more times. But when every retry also times
//! out, the classification arm collapses the remaining error into
//! `Err(DispatcherError::KeyNotFound(key))` (lib.rs:2667-2669). A KeyNotFound
//! result is then forwarded to remote lookup (lib.rs:3025-3032) and attributed
//! `ServedBy::Miss` (lib.rs:3239) — for a key that HAS a cache entry, held by
//! a concurrent store-commit / promote. The spec (FR-039 step 1) forbids
//! exactly that: "retried locally a few times (up to 5) rather than collapsed
//! into `KeyNotFound`; collapsing it would forward a live key to
//! remote-lookup".
//!
//! This test models the writer hold deterministically, the same way
//! `repair_test_check_present_map_timeout` does for `check`: the mock map's
//! `lookup` answers `Err(Timeout)` for the first `timeouts_left` calls on a
//! writer-held entry (each an expired 2 s wait), and the writer settles within
//! the next attempt's wait when `timeouts_left` reaches 0.
//!
//! - KEY_SETTLES: the writer outlasts three lookup deadlines, then settles
//!   inside the retry budget → the key must be served warm (Ok, DRAM), never
//!   reported KeyNotFound and never forwarded to remote lookup.
//! - KEY_HELD: the writer outlasts every retry (all six attempts time out) →
//!   the key must be reported as the map's own error (a timeout to wait out),
//!   never as KeyNotFound / a Miss, and never forwarded to remote lookup.
//! - KEY_ABSENT: no cache entry at all → KeyNotFound, Miss, and forwarded.
//!
//! Self-contained (its own mocks, the public `IDispatcher` API only), so it
//! builds against the dispatcher as it was before the repair as well as
//! after it.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use component_core::query_interface;
use interfaces::{
    CacheKey, DispatchMapError, DispatcherConfig, DispatcherError, DmaBuffer, GpuDeviceInfo,
    GpuDmaBuffer, GpuIpcHandle, GpuStream, IDispatchMap, IDispatcher, IGpuServices, ILogger,
    IMemoryTier, IRemoteLookup, IpcHandle, LookupConfig, LookupResult, MemoryTierError,
    MemoryTierTelemetrySnapshot, RemoteLookupError, RemoteServeStats, ServedBy,
};

use crate::DispatcherComponent;

const BLOCK: u32 = 4096;

/// A memory-tier entry whose writer outlasts three lookup deadlines, then
/// settles inside the retry budget (a concurrent populate that finishes).
const KEY_SETTLES: CacheKey = 1;
/// A memory-tier entry whose writer outlasts every retry budget.
const KEY_HELD: CacheKey = 2;
/// A key with no cache entry at all.
const KEY_ABSENT: CacheKey = 3;

// --- dispatch map: entries with reference counts and a modelled lookup
//     deadline, as the real one enforces them ---

#[derive(Clone, Copy, Debug, PartialEq)]
enum Loc {
    Block(u64),
    // Pointer stored as usize so the mock stays Send + Sync without an
    // unsafe impl.
    Memory { ptr: usize, size: u32 },
}

struct Ent {
    loc: Loc,
    read_refs: u32,
    /// A writer holds the entry (`write_ref`), as `create_memory_tier_entry`
    /// leaves it until the writer settles.
    write_ref: bool,
    /// How many more `lookup`s answer `Err(Timeout)` while the writer holds:
    /// each is one expired 2 s wait (compressed to nothing here — only the
    /// outcome is modelled). When it reaches 0 the writer settles inside the
    /// next attempt's wait and that attempt succeeds.
    timeouts_left: u32,
    /// How many `lookup`s were issued on this key (to observe the retry
    /// count: one attempt plus up to five retries).
    lookups: u32,
}

struct Dm {
    entries: Mutex<HashMap<CacheKey, Ent>>,
}

impl Dm {
    fn new() -> Self {
        Self {
            entries: Mutex::new(HashMap::new()),
        }
    }

    fn read_refs(&self, key: CacheKey) -> Option<u32> {
        self.entries.lock().unwrap().get(&key).map(|e| e.read_refs)
    }

    fn writer_holds(&self, key: CacheKey) -> bool {
        self.entries
            .lock()
            .unwrap()
            .get(&key)
            .is_some_and(|e| e.write_ref)
    }

    fn lookup_count(&self, key: CacheKey) -> u32 {
        self.entries
            .lock()
            .unwrap()
            .get(&key)
            .map_or(0, |e| e.lookups)
    }

    /// Seed a writer-held entry in the state a concurrent populate leaves
    /// between `create_memory_tier_entry` and its settlement.
    fn seed_memory(&self, key: CacheKey, ptr: *mut u8, size: u32, timeouts_left: u32) {
        self.entries.lock().unwrap().insert(
            key,
            Ent {
                loc: Loc::Memory {
                    ptr: ptr as usize,
                    size,
                },
                read_refs: 0,
                write_ref: true,
                timeouts_left,
                lookups: 0,
            },
        );
    }
}

impl IDispatchMap for Dm {
    fn initialize(&self) -> Result<(), DispatchMapError> {
        Ok(())
    }
    fn lookup(&self, key: CacheKey) -> Result<LookupResult, DispatchMapError> {
        let mut entries = self.entries.lock().unwrap();
        let Some(e) = entries.get_mut(&key) else {
            return Ok(LookupResult::NotExist);
        };
        e.lookups += 1;
        if e.write_ref {
            if e.timeouts_left > 0 {
                // The map's 2 s wait expired behind the writer, as the real
                // dispatch-map does (src/lib.rs:128-140).
                e.timeouts_left -= 1;
                return Err(DispatchMapError::Timeout(key));
            }
            // The writer settles inside this attempt's wait: the entry stays,
            // memory-tier resident, and this lookup takes its read pin.
            e.write_ref = false;
        }
        e.read_refs += 1;
        Ok(match e.loc {
            Loc::Block(offset) => LookupResult::BlockDevice { offset },
            Loc::Memory { ptr, size } => LookupResult::MemoryTier {
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
            Loc::Memory { .. } => {
                e.loc = Loc::Block(offset);
                e.read_refs = e.read_refs.saturating_sub(1);
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
            .ok_or(DispatchMapError::KeyNotFound(key))?;
        if !e.write_ref {
            return Err(DispatchMapError::NoWriteReference(key));
        }
        e.write_ref = false;
        e.read_refs += 1;
        Ok(())
    }
    fn remove(&self, key: CacheKey) -> Result<(), DispatchMapError> {
        let mut entries = self.entries.lock().unwrap();
        match entries.get(&key) {
            None => Err(DispatchMapError::KeyNotFound(key)),
            Some(e) if e.read_refs > 0 || e.write_ref => {
                Err(DispatchMapError::ActiveReferences(key))
            }
            Some(_) => {
                entries.remove(&key);
                Ok(())
            }
        }
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
        let mut keys: Vec<CacheKey> = self.entries.lock().unwrap().keys().copied().collect();
        keys.sort_unstable();
        keys.truncate(n);
        keys
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
                },
                read_refs: 0,
                write_ref: true,
                timeouts_left: 0,
                lookups: 0,
            },
        );
        Ok(())
    }
    fn convert_memory_tier_to_block(&self, key: CacheKey) -> Result<(), DispatchMapError> {
        let mut entries = self.entries.lock().unwrap();
        let e = entries
            .get_mut(&key)
            .ok_or(DispatchMapError::KeyNotFound(key))?;
        match e.loc {
            Loc::Memory { .. } => {
                e.loc = Loc::Block(0);
                Ok(())
            }
            Loc::Block(_) => Err(DispatchMapError::InvalidState("already block".into())),
        }
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
        match e.loc {
            Loc::Block(_) => {
                e.loc = Loc::Memory {
                    ptr: pointer as usize,
                    size,
                };
                Ok(())
            }
            Loc::Memory { .. } => Err(DispatchMapError::InvalidState("already memory".into())),
        }
    }
    fn is_evictable(&self, key: CacheKey) -> bool {
        let entries = self.entries.lock().unwrap();
        entries
            .get(&key)
            .is_some_and(|e| e.read_refs == 0 && !e.write_ref)
    }
    fn try_evict_to_block(&self, key: CacheKey) -> Result<(), DispatchMapError> {
        let mut entries = self.entries.lock().unwrap();
        let e = entries
            .get_mut(&key)
            .ok_or(DispatchMapError::KeyNotFound(key))?;
        if e.read_refs != 0 || e.write_ref {
            return Err(DispatchMapError::ActiveReferences(key));
        }
        match e.loc {
            Loc::Memory { .. } => {
                e.loc = Loc::Block(0);
                Ok(())
            }
            Loc::Block(_) => Err(DispatchMapError::InvalidState("not evictable".into())),
        }
    }
    fn recover_extent(
        &self,
        _key: CacheKey,
        _offset: u64,
        _size_blocks: u32,
    ) -> Result<(), DispatchMapError> {
        Err(DispatchMapError::InvalidState("not modelled".into()))
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

// --- memory tier: a flat pool of 4 KiB slots ---

struct PoolMt {
    pool: Mutex<Vec<u8>>,
}

impl PoolMt {
    fn new(capacity: usize) -> Self {
        Self {
            pool: Mutex::new(vec![0u8; capacity]),
        }
    }

    /// A deterministic per-key slot in the pool, as populate's reserve would
    /// hand out (inherent, so it shadows nothing on the trait).
    fn slot(&self, key: CacheKey, size: u32) -> *mut u8 {
        let mut pool = self.pool.lock().unwrap();
        let off = (key as usize - 1) * BLOCK as usize;
        assert!(
            off + size as usize <= pool.len(),
            "mock pool too small for key {key}"
        );
        // SAFETY: `off + size <= pool.len()` was checked above.
        unsafe { pool.as_mut_ptr().add(off) }
    }
}

impl IMemoryTier for PoolMt {
    fn initialize(&self, _pool_size: usize, _numa: Option<i32>) -> Result<(), MemoryTierError> {
        Ok(())
    }
    fn insert(&self, key: CacheKey, size: u32) -> Result<*mut u8, MemoryTierError> {
        Ok(self.slot(key, size))
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
    fn remove(&self, _key: CacheKey) -> Result<(), MemoryTierError> {
        Ok(())
    }
    fn touch(&self, _key: CacheKey) {}
    fn batch_touch(&self, _keys: &[CacheKey]) {}
    fn contains(&self, _key: CacheKey) -> bool {
        false
    }
    fn capacity(&self) -> usize {
        self.pool.lock().unwrap().len()
    }
    fn used(&self) -> usize {
        0
    }
    fn pool_info(&self) -> Option<(*mut u8, usize)> {
        None
    }
    fn is_dma_capable(&self) -> bool {
        true
    }
    fn clear(&self) -> Result<usize, MemoryTierError> {
        Ok(0)
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

// --- GPU services: host memcpy stands in for every DMA; streams synchronize
//     successfully (the warm path itself is not under test here) ---

struct HostCopyGpu;

fn host_copy(src: *const u8, dst: *mut u8, size: usize) -> Result<(), String> {
    // SAFETY: in this test every src/dst is a live host buffer of at least `size` bytes.
    unsafe { std::ptr::copy_nonoverlapping(src, dst, size) };
    Ok(())
}

impl IGpuServices for HostCopyGpu {
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

// --- remote lookup: records every key forwarded to it; no peer holds
//     anything, so every forward answers NotFound ---

struct RecordingRemote {
    seen: Mutex<Vec<CacheKey>>,
}

impl IRemoteLookup for RecordingRemote {
    fn initialize(&self, _config: LookupConfig) -> Result<(), RemoteLookupError> {
        Ok(())
    }
    fn batch_lookup(&self, entries: &[(CacheKey, u32)]) -> Vec<Result<(), RemoteLookupError>> {
        let mut seen = self.seen.lock().unwrap();
        entries
            .iter()
            .map(|(key, _)| {
                seen.push(*key);
                Err(RemoteLookupError::NotFound)
            })
            .collect()
    }
    fn join_cluster(&self, _endpoint: &str) -> Result<(), RemoteLookupError> {
        Ok(())
    }
    fn leave_cluster(&self) -> Result<(), RemoteLookupError> {
        Ok(())
    }
    fn serve_stats(&self) -> RemoteServeStats {
        RemoteServeStats::default()
    }
}

struct QuietLogger;

impl ILogger for QuietLogger {
    fn error(&self, _msg: &str) {}
    fn warn(&self, _msg: &str) {}
    fn info(&self, _msg: &str) {}
    fn debug(&self, _msg: &str) {}
}

// --- fixture ---

struct Fixture {
    d: Arc<dyn IDispatcher + Send + Sync>,
    dm: Arc<Dm>,
    mt: Arc<PoolMt>,
    rl: Arc<RecordingRemote>,
    _c: Arc<DispatcherComponent>,
}

/// A dispatcher in memory-tier-only mode (no SPDK environment, no drives),
/// with the mock dispatch map and a remote lookup bound. The batch has only
/// warm hits, writer-held entries and misses — no cold path runs.
fn setup() -> Fixture {
    let dm = Arc::new(Dm::new());
    let mt = Arc::new(PoolMt::new(64 * 1024));
    let rl = Arc::new(RecordingRemote {
        seen: Mutex::new(Vec::new()),
    });

    let c = DispatcherComponent::new_default();
    c.dispatch_map
        .connect(Arc::clone(&dm) as Arc<dyn IDispatchMap + Send + Sync>)
        .unwrap();
    c.memory_tier
        .connect(Arc::clone(&mt) as Arc<dyn IMemoryTier + Send + Sync>)
        .unwrap();
    c.gpu_services.connect(Arc::new(HostCopyGpu)).unwrap();
    c.remote_lookup
        .connect(Arc::clone(&rl) as Arc<dyn IRemoteLookup + Send + Sync>)
        .unwrap();
    c.logger.connect(Arc::new(QuietLogger)).unwrap();
    let d = query_interface!(c, IDispatcher).unwrap();
    d.initialize(DispatcherConfig {
        data_pci_addrs: vec!["0000:02:00.0".to_string()],
        ssd_eviction_threshold: 0.0,
        store_backpressure_ms: 0,
        ..Default::default()
    })
    .unwrap();
    Fixture {
        d,
        dm,
        mt,
        rl,
        _c: c,
    }
}

// --- tests ---

/// DIS-BATCH-LOOKUP-TRANSIENT-RETRY: a dispatch-map lookup error is a writer
/// holding the entry, not a missing key. The writer-held key that settles
/// inside the retry budget is served warm; the writer that outlasts every
/// retry is reported as the map's own error (a timeout), never as
/// KeyNotFound, never attributed Miss, and never forwarded to remote lookup.
/// A genuinely absent key stays a miss and IS forwarded.
#[test]
fn batch_lookup_transient_map_error_not_keynotfound() {
    let f = setup();

    // KEY_SETTLES: memory-tier resident behind a writer that outlasts three
    // lookup deadlines, then settles (attempt 4, inside the 5-retry budget).
    let slot = f.mt.slot(KEY_SETTLES, BLOCK);
    // SAFETY: `slot` is a fresh 4096-byte slot inside the pool.
    unsafe { std::ptr::write_bytes(slot, 0xAA, BLOCK as usize) };
    f.dm.seed_memory(KEY_SETTLES, slot, BLOCK, 3);
    // KEY_HELD: memory-tier resident behind a writer that never settles.
    let held_slot = f.mt.slot(KEY_HELD, BLOCK);
    // SAFETY: `held_slot` is a fresh 4096-byte slot inside the pool.
    unsafe { std::ptr::write_bytes(held_slot, 0xBB, BLOCK as usize) };
    f.dm.seed_memory(KEY_HELD, held_slot, BLOCK, u32::MAX);

    let mut out_settles = vec![0u8; BLOCK as usize];
    let mut out_held = vec![0u8; BLOCK as usize];
    let mut out_absent = vec![0u8; BLOCK as usize];
    let entries = vec![
        (
            KEY_SETTLES,
            vec![IpcHandle {
                address: out_settles.as_mut_ptr(),
                size: BLOCK,
            }],
        ),
        (
            KEY_HELD,
            vec![IpcHandle {
                address: out_held.as_mut_ptr(),
                size: BLOCK,
            }],
        ),
        (
            KEY_ABSENT,
            vec![IpcHandle {
                address: out_absent.as_mut_ptr(),
                size: BLOCK,
            }],
        ),
    ];
    let results = f.d.batch_lookup(&entries);
    assert_eq!(results.len(), 3);

    // KEY_SETTLES: the retry waited the writer out and served the warm hit.
    assert!(
        results[0].result.is_ok(),
        "a writer-held entry that settles inside the retry budget must be \
         served (got {:?})",
        results[0].result
    );
    assert_eq!(results[0].served_by, ServedBy::Dram);
    assert!(out_settles.iter().all(|&b| b == 0xAA));
    // One attempt plus three timed-out retries settled it on the fourth
    // lookup — the transient error was retried, not classified.
    assert_eq!(
        f.dm.lookup_count(KEY_SETTLES),
        4,
        "the transient lookup error must be retried until the writer settles"
    );
    // The warm pin was released after the batched sync.
    assert_eq!(
        f.dm.read_refs(KEY_SETTLES),
        Some(0),
        "batch_lookup leaked the warm hit's read reference"
    );

    // KEY_HELD: one attempt plus all five retries timed out (six lookups)...
    assert_eq!(
        f.dm.lookup_count(KEY_HELD),
        6,
        "the lookup must be retried up to five more times before giving up"
    );
    assert!(
        f.dm.writer_holds(KEY_HELD),
        "batch_lookup must not have disturbed the writer's hold"
    );
    // ... and the key is still not reported as absent: the map's error is
    // reported as the timeout it is, never collapsed into KeyNotFound.
    assert!(
        !matches!(results[1].result, Err(DispatcherError::KeyNotFound(_))),
        "a live key behind a writer must never be reported KeyNotFound \
         (DIS-BATCH-LOOKUP-TRANSIENT-RETRY) (got {:?})",
        results[1].result
    );
    assert!(
        matches!(results[1].result, Err(DispatcherError::Timeout(_))),
        "an exhausted retry behind a writer must surface the map timeout, as \
         check does (FR-008) (got {:?})",
        results[1].result
    );
    // Not a miss: a live key must not deflate the hit rate.
    assert_ne!(
        results[1].served_by,
        ServedBy::Miss,
        "a live key behind a writer must not be attributed Miss"
    );
    assert_eq!(results[1].served_by, ServedBy::Error);

    // KEY_ABSENT: a genuine miss stays a miss and is forwarded to remote
    // lookup — which must therefore have seen ONLY that key.
    assert!(
        matches!(results[2].result, Err(DispatcherError::KeyNotFound(k)) if k == KEY_ABSENT),
        "an absent key must stay a miss (got {:?})",
        results[2].result
    );
    assert_eq!(results[2].served_by, ServedBy::Miss);
    let seen = f.rl.seen.lock().unwrap().clone();
    assert_eq!(
        seen,
        vec![KEY_ABSENT],
        "remote lookup must be forwarded only the genuinely absent key — \
         never a writer-held live key (forwarded: {seen:?})"
    );

    f.d.shutdown().unwrap();
}
