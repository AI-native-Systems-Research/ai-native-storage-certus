//! Repair test for finding `eviction-errors-counted-unpersisted`
//! (DIS-TIER-STATS-BLOCKED-CAUSES).
//!
//! FR-033 (specs/002-served-by-tier-attribution) reserves
//! `evictions_blocked_unpersisted` for the one refusal cause "the entry has no
//! `ssd_offset` yet" and `evictions_blocked_by_pin` for "a reader holds it".
//! Any third cause must increment NEITHER counter, so that the two counters'
//! sum against the scanned total exposes it instead of absorbing it.
//!
//! The dispatch-map refuses a candidate in a third way that is neither of
//! those: `KeyNotFound` for a memory-tier slot that exists in the tier but has
//! no dispatch-map entry yet (the window after `reserve_memory` inserted the
//! slot and before the entry is registered). `evict_one_clean`'s `Err(_)`
//! catch-all booked that refusal as `evictions_blocked_unpersisted`, so the
//! counter over-reported write-through lag by every unregistered slot the
//! scan touched.
//!
//! Three candidates, one of each refusal cause, are scanned in one
//! `evict_for_space` call. `evict_for_space` takes two passes over them (the
//! per-step scan, then the last-chance full-depth scan), so each candidate is
//! examined twice:
//!
//!   * the pinned one must add 2 to `evictions_blocked_by_pin` (1 per
//!     examination),
//!   * the unpersisted one must add 2 to `evictions_blocked_unpersisted`,
//!   * the unregistered one must add 0 to both.
//!
//! On the unfixed code the unregistered candidate is counted as unpersisted,
//! so `evictions_blocked_unpersisted` comes out as 4 and the assertion fails.

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize};
use std::sync::{Arc, Mutex};

use parking_lot::RwLock;

use interfaces::{
    CacheKey, DispatchMapError, IDispatchMap, IMemoryTier, LookupResult, MemoryTierError,
    MemoryTierTelemetrySnapshot,
};

use crate::{DispatcherComponent, TierEventCounters};

const SLOT: u32 = 4096;

/// In the dispatch map, pinned by an in-flight reader -> `ActiveReferences`.
const KEY_PINNED: CacheKey = 1;
/// In the dispatch map, memory-tier location, write-through not landed (no
/// `ssd_offset`) -> `InvalidState("memory-tier entry has no SSD offset...")`.
const KEY_UNPERSISTED: CacheKey = 2;
/// A memory-tier slot with no dispatch-map entry yet (the window after
/// `reserve_memory` inserted the slot, before the entry is registered) ->
/// `KeyNotFound`.
const KEY_UNREGISTERED: CacheKey = 3;

/// The candidates the memory tier reports, oldest first.
const CANDIDATES: [CacheKey; 3] = [KEY_PINNED, KEY_UNPERSISTED, KEY_UNREGISTERED];

// --- dispatch map: mirrors the real try_evict_to_block refusals ---

#[derive(Clone, Copy)]
enum Loc {
    /// MemoryTier with its write-through offset, once landed.
    Persisted(u64),
    /// MemoryTier with no offset yet.
    Unpersisted,
}

struct Ent {
    loc: Loc,
    read_refs: u32,
}

#[derive(Default)]
struct Dm {
    entries: Mutex<HashMap<CacheKey, Ent>>,
}

impl IDispatchMap for Dm {
    fn initialize(&self) -> Result<(), DispatchMapError> {
        Ok(())
    }
    fn lookup(&self, key: CacheKey) -> Result<LookupResult, DispatchMapError> {
        let entries = self.entries.lock().unwrap();
        Ok(match entries.get(&key) {
            None => LookupResult::NotExist,
            Some(e) => LookupResult::MemoryTier {
                pointer: std::ptr::null_mut(),
                size: e.read_refs.max(1) * SLOT,
            },
        })
    }
    fn convert_to_storage(&self, key: CacheKey, offset: u64) -> Result<(), DispatchMapError> {
        let mut entries = self.entries.lock().unwrap();
        let e = entries
            .get_mut(&key)
            .ok_or(DispatchMapError::KeyNotFound(key))?;
        e.loc = Loc::Persisted(offset);
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
    fn take_write(&self, _: CacheKey) -> Result<(), DispatchMapError> {
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
    fn release_write(&self, _: CacheKey) -> Result<(), DispatchMapError> {
        Ok(())
    }
    fn downgrade_reference(&self, _: CacheKey) -> Result<(), DispatchMapError> {
        Ok(())
    }
    fn remove(&self, key: CacheKey) -> Result<(), DispatchMapError> {
        self.entries.lock().unwrap().remove(&key);
        Ok(())
    }
    fn touch(&self, _: CacheKey) -> Result<(), DispatchMapError> {
        Ok(())
    }
    fn entry_size(&self, _: CacheKey) -> Result<u32, DispatchMapError> {
        Ok(SLOT)
    }
    fn oldest_keys(&self, _: usize) -> Vec<CacheKey> {
        Vec::new()
    }
    fn create_memory_tier_entry(
        &self,
        key: CacheKey,
        _: *mut u8,
        _: u32,
    ) -> Result<(), DispatchMapError> {
        self.entries.lock().unwrap().insert(
            key,
            Ent {
                loc: Loc::Unpersisted,
                read_refs: 0,
            },
        );
        Ok(())
    }
    fn convert_memory_tier_to_block(&self, _: CacheKey) -> Result<(), DispatchMapError> {
        Ok(())
    }
    fn promote_block_to_memory_tier(
        &self,
        _: CacheKey,
        _: *mut u8,
        _: u32,
    ) -> Result<(), DispatchMapError> {
        Ok(())
    }
    fn is_evictable(&self, key: CacheKey) -> bool {
        let entries = self.entries.lock().unwrap();
        matches!(entries.get(&key), Some(e) if matches!(e.loc, Loc::Persisted(_)))
    }
    /// Mirrors dispatch-map `try_evict_to_block` (components/dispatch-map
    /// src/lib.rs): `KeyNotFound` when the map holds no entry, then
    /// `ActiveReferences` when a pin is held, then `InvalidState` when the
    /// memory-tier entry has no SSD offset to demote to.
    fn try_evict_to_block(&self, key: CacheKey) -> Result<(), DispatchMapError> {
        let mut entries = self.entries.lock().unwrap();
        let e = entries
            .get_mut(&key)
            .ok_or(DispatchMapError::KeyNotFound(key))?;
        if e.read_refs > 0 {
            return Err(DispatchMapError::ActiveReferences(key));
        }
        match e.loc {
            Loc::Persisted(offset) => {
                e.loc = Loc::Persisted(offset);
                Ok(())
            }
            Loc::Unpersisted => Err(DispatchMapError::InvalidState(
                "memory-tier entry has no SSD offset (write-through not complete)".into(),
            )),
        }
    }
    fn recover_extent(&self, _: CacheKey, _: u64, _: u32) -> Result<(), DispatchMapError> {
        Ok(())
    }
    #[cfg(feature = "integrity-check")]
    fn set_checksum(&self, _: CacheKey, _: u32) -> Result<(), DispatchMapError> {
        Ok(())
    }
    #[cfg(feature = "integrity-check")]
    fn get_checksum(&self, _: CacheKey) -> Option<u32> {
        None
    }
}

// --- memory tier: the three slots, always reported as the oldest keys ---

struct Mt;

impl IMemoryTier for Mt {
    fn initialize(&self, _: usize, _: Option<i32>) -> Result<(), MemoryTierError> {
        Ok(())
    }
    fn insert(&self, _: CacheKey, _: u32) -> Result<*mut u8, MemoryTierError> {
        Ok(std::ptr::null_mut())
    }
    fn get(&self, key: CacheKey) -> Option<(*mut u8, u32)> {
        self.peek(key)
    }
    fn peek(&self, key: CacheKey) -> Option<(*mut u8, u32)> {
        CANDIDATES
            .contains(&key)
            .then_some((std::ptr::null_mut(), SLOT))
    }
    fn evict_next(&self) -> Option<CacheKey> {
        None
    }
    fn evict_next_for_key(&self, _: CacheKey) -> Option<CacheKey> {
        None
    }
    fn oldest_keys(&self, n: usize) -> Vec<CacheKey> {
        CANDIDATES.iter().copied().take(n).collect()
    }
    fn remove(&self, _: CacheKey) -> Result<(), MemoryTierError> {
        Ok(())
    }
    fn touch(&self, _: CacheKey) {}
    fn batch_touch(&self, _: &[CacheKey]) {}
    fn contains(&self, key: CacheKey) -> bool {
        CANDIDATES.contains(&key)
    }
    fn capacity(&self) -> usize {
        3 * SLOT as usize
    }
    fn used(&self) -> usize {
        3 * SLOT as usize
    }
    fn pool_info(&self) -> Option<(*mut u8, usize)> {
        None
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
    fn bounce_free(&self, _: interfaces::BounceBuffer) {}
    fn bounce_usage(&self) -> (usize, usize) {
        (0, 0)
    }
}

#[test]
fn unregistered_slot_refusal_is_not_counted_as_unpersisted() {
    let dm: Arc<dyn IDispatchMap + Send + Sync> = Arc::new(Dm::default());
    // KEY_PINNED: resident, pinned by an in-flight reader.
    dm.create_memory_tier_entry(KEY_PINNED, std::ptr::null_mut(), SLOT)
        .unwrap();
    dm.take_read(KEY_PINNED).unwrap();
    // KEY_UNPERSISTED: resident, unpinned, write-through not landed.
    dm.create_memory_tier_entry(KEY_UNPERSISTED, std::ptr::null_mut(), SLOT)
        .unwrap();
    // KEY_UNREGISTERED: memory-tier slot with no dispatch-map entry. It is in
    // the tier (oldest_keys reports it) but absent from the map, so
    // try_evict_to_block refuses with KeyNotFound.

    let mt: Arc<dyn IMemoryTier + Send + Sync> = Arc::new(Mt);

    let counters = Arc::new(TierEventCounters::default());
    let c = DispatcherComponent::new(
        AtomicBool::new(false),
        Mutex::new(None),
        Mutex::new(None),
        Mutex::new(None),
        RwLock::new(Vec::new()),
        RwLock::new(interfaces::ReadWriteStats::default()),
        RwLock::new(None),
        AtomicU64::new(0),
        Mutex::new(None),
        Mutex::new(None),
        AtomicUsize::new(2048),
        AtomicU64::new(0),
        RwLock::new(None),
        Arc::new(Mutex::new(None)),
        Arc::new(AtomicU64::new(0)),
        Arc::clone(&counters),
        Arc::new(crate::background::WriteThroughPins::default()),
    );

    // The tier is full (used == capacity), so this scans for room. Nothing is
    // demotable (pinned / unpersisted / unregistered), so it must fail; the
    // two passes (per-step scan 4, then the last-chance full-depth scan)
    // examine each of the three candidates exactly twice.
    let res = c.evict_for_space(&dm, &mt, SLOT, 1000, 512);
    assert!(
        res.is_err(),
        "no candidate is demotable, so the eviction must fail"
    );

    let stats = counters.snapshot();

    // The pin refusal is counted per candidate examined: two examinations of
    // KEY_PINNED -> +2. (Holds before and after the fix.)
    assert_eq!(
        stats.evictions_blocked_by_pin, 2,
        "the pinned candidate must be counted once per examination"
    );

    // Only the missing-ssd_offset refusal may land in evictions_blocked_unpersisted:
    // two examinations of KEY_UNPERSISTED -> +2, and the KeyNotFound refusal of
    // KEY_UNREGISTERED must add NOTHING. The catch-all made it 4.
    assert_eq!(
        stats.evictions_blocked_unpersisted, 2,
        "evictions_blocked_unpersisted must count only candidates refused for want of an \
         ssd_offset (FR-033); a KeyNotFound refusal (a slot with no dispatch-map entry \
         yet) is a third cause and must land in neither counter, got {:?}",
        stats
    );

    // Both scans examined every candidate and freed nothing (FR-034; holds
    // before and after the fix).
    assert_eq!(
        stats.eviction_scans_exhausted, 2,
        "both passes (step scan + last-chance scan) must be counted as exhausted"
    );
}
