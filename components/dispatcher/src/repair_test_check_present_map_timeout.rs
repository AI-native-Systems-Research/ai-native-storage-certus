//! Repair test for finding `check-returns-false-on-map-timeout`
//! (DIS-CHECK-PRESENT).
//!
//! `check` collapses every `Err` from `dm.lookup` into `Ok(false)`. But the
//! dispatch map's `lookup` answers `Err(DispatchMapError::Timeout)` when a
//! writer holds the entry's write reference past the map's 2 s
//! `DEFAULT_TIMEOUT` (dispatch-map src/lib.rs:128-140) — the entry EXISTS at
//! that moment, in memory-tier or block-device state, inside the window a
//! concurrent populate/promote opens with `create_memory_tier_entry`. A
//! timeout means "a writer is holding the entry", not "the key is absent".
//!
//! The spec (idispatcher.rs:604, DIS-CHECK-PRESENT) promises: "Returns `true`
//! if the key is present in the cache (any tier), `false` otherwise."
//!
//! This test models the timed-out wait deterministically: the mock map's
//! `lookup` answers `Err(Timeout)` for the first `timeouts_left` calls on a
//! writer-held entry (the deadline expired behind the writer, as in the real
//! map), and the writer settles within the next attempt's wait. `check` must
//! wait the writer out and answer `true` — never a false "not present".
//!
//! Self-contained (its own mocks, the public `IDispatcher` API only), so it
//! builds against the dispatcher as it was before the repair as well as
//! after it.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use component_core::query_interface;
use interfaces::{
    CacheKey, DispatchMapError, DispatcherConfig, IDispatchMap, IDispatcher, IMemoryTier,
    LookupResult, MemoryTierError, MemoryTierTelemetrySnapshot,
};

use crate::DispatcherComponent;

const BLOCK: u32 = 4096;

/// A memory-tier entry whose writer outlasts two lookup deadlines, then
/// settles (the concurrent populate finishes inside the retry window).
const KEY_MEM: CacheKey = 1;
/// A block-device entry whose writer outlasts two lookup deadlines.
const KEY_BLK: CacheKey = 2;
/// A memory-tier entry whose writer never settles within any retry budget.
const KEY_HELD: CacheKey = 3;
/// A key with no cache entry at all.
const KEY_ABSENT: CacheKey = 9;

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
            },
        );
    }

    /// Seed a writer-held entry that already carries its block-device copy.
    fn seed_block(&self, key: CacheKey, offset: u64, timeouts_left: u32) {
        self.entries.lock().unwrap().insert(
            key,
            Ent {
                loc: Loc::Block(offset),
                read_refs: 0,
                write_ref: true,
                timeouts_left,
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
        if e.write_ref {
            if e.timeouts_left > 0 {
                // The map's 2 s wait expired behind the writer, as the real
                // dispatch-map does (src/lib.rs:128-140).
                e.timeouts_left -= 1;
                return Err(DispatchMapError::Timeout(key));
            }
            // The writer settles inside this attempt's wait: the entry stays,
            // in one of the two states check must recognize.
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

// --- memory tier: a flat pool of 4 KiB slots (check never reads it; the
//     entry pointers must just be real pool slots) ---

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
}

// --- fixture ---

struct Fixture {
    d: Arc<dyn IDispatcher + Send + Sync>,
    dm: Arc<Dm>,
    mt: Arc<PoolMt>,
    _c: Arc<DispatcherComponent>,
}

/// A dispatcher in memory-tier-only mode (no SPDK environment, no drives),
/// with the mock dispatch map bound. `check` touches neither the GPU nor the
/// drives, so none are needed.
fn setup() -> Fixture {
    let dm = Arc::new(Dm::new());
    let mt = Arc::new(PoolMt::new(64 * 1024));

    let c = DispatcherComponent::new_default();
    c.dispatch_map
        .connect(Arc::clone(&dm) as Arc<dyn IDispatchMap + Send + Sync>)
        .unwrap();
    c.memory_tier
        .connect(Arc::clone(&mt) as Arc<dyn IMemoryTier + Send + Sync>)
        .unwrap();
    let d = query_interface!(c, IDispatcher).unwrap();
    d.initialize(DispatcherConfig {
        data_pci_addrs: vec!["0000:02:00.0".to_string()],
        ssd_eviction_threshold: 0.0,
        store_backpressure_ms: 0,
        ..Default::default()
    })
    .unwrap();
    Fixture { d, dm, mt, _c: c }
}

// --- tests ---

/// DIS-CHECK-PRESENT: check returns true for a key that has a cache entry in
/// either memory-tier or block-device state — including while a writer holds
/// the entry past the map's lookup deadline, and after the writer settles
/// inside the retry window. A writer that never settles is a timeout to
/// report, never a "not present"; an absent key stays false.
#[test]
fn check_present_despite_map_timeout() {
    let f = setup();

    // Memory-tier state as a concurrent populate leaves it: the slot holds
    // the data, the writer holds the entry; its hold outlasts two lookup
    // deadlines and then settles.
    let slot = f.mt.slot(KEY_MEM, BLOCK);
    f.dm.seed_memory(KEY_MEM, slot, BLOCK, 2);
    // Block-device state behind the same kind of writer hold.
    f.dm.seed_block(KEY_BLK, 0x2000, 2);
    // A writer that outlasts every retry budget.
    let held_slot = f.mt.slot(KEY_HELD, BLOCK);
    f.dm.seed_memory(KEY_HELD, held_slot, BLOCK, u32::MAX);

    // The writer-held memory-tier entry is present: check must answer true.
    let r_mem = f.d.check(KEY_MEM);
    assert!(
        matches!(r_mem, Ok(true)),
        "a memory-tier entry behind a writer that outlasts the lookup \
         deadline is present (DIS-CHECK-PRESENT) (got {r_mem:?})"
    );
    // ... and so is the block-device one.
    let r_blk = f.d.check(KEY_BLK);
    assert!(
        matches!(r_blk, Ok(true)),
        "a block-device entry behind a writer that outlasts the lookup \
         deadline is present (DIS-CHECK-PRESENT) (got {r_blk:?})"
    );
    // check took and released exactly one read reference on each.
    for key in [KEY_MEM, KEY_BLK] {
        assert_eq!(
            f.dm.read_refs(key),
            Some(0),
            "check leaked a read reference on key {key}"
        );
    }

    // A writer that never settles: a timeout to report — never Ok(false),
    // which would send the caller to re-populate a live key.
    let r_held = f.d.check(KEY_HELD);
    assert!(
        matches!(r_held, Err(interfaces::DispatcherError::Timeout(_))),
        "a key whose writer outlasts every retry must be reported as a \
         timeout, not as absent (got {r_held:?})"
    );
    assert!(
        f.dm.writer_holds(KEY_HELD),
        "check must not have disturbed the writer's hold"
    );

    // An absent key is still false (DIS-CHECK-ABSENT direction).
    assert!(
        matches!(f.d.check(KEY_ABSENT), Ok(false)),
        "absent key must answer false (got {:?})",
        f.d.check(KEY_ABSENT)
    );

    f.d.shutdown().unwrap();
}
