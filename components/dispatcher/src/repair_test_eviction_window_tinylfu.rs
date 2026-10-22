//! Repair test for finding `eviction-window-poisoned-by-admission-front`.
//!
//! `eviction-policy-optimized` admits a store whose sketch count is no higher
//! than the current victim's at the LRU **front**. Under a store burst that is
//! every fresh key, so the front of the eviction order fills with entries whose
//! write-through has not landed and which `try_evict_to_block` must refuse.
//! Persisted, demotable entries sit just behind them.
//!
//! `evict_for_space` scans `MAX_SCAN × attempts` candidates, but every scan that
//! frees nothing returns `AllocationFailed` at once, so the widening never
//! happens: the scan stays 4 deep, sees only the unpersisted front, and the
//! store is refused although demotable entries exist. The fix takes one
//! last-chance full-depth scan before giving up, as `dispatcher-p2p` does.
//!
//! Uses the real `MemoryTierComponent` and `EvictionPolicyOptimizedComponent`,
//! so the eviction order is the one the `full-optimized` profile produces.

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize};
use std::sync::{Arc, Mutex};

use parking_lot::RwLock;

use component_core::query_interface;
use eviction_policy_optimized::EvictionPolicyOptimizedComponent;
use interfaces::{
    CacheKey, DispatchMapError, IDispatchMap, IEvictionPolicy, IMemoryTier, LookupResult,
};
use memory_tier::MemoryTierComponent;

use crate::{DispatcherComponent, DispatcherError, TierEventCounters};

const SLOT: u32 = 4096;
/// Persisted entries, stored first. They age to the back of the eviction order.
const PERSISTED: u64 = 8;
/// Unpersisted entries, stored next. TinyLFU admits each at the LRU front.
const UNPERSISTED: u64 = 8;

// --- dispatch map: just enough state for `try_evict_to_block` ---

struct Ent {
    ssd_offset: Option<u64>,
    on_block: bool,
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
            Some(e) if e.on_block => LookupResult::BlockDevice {
                offset: e.ssd_offset.unwrap_or(0),
            },
            Some(_) => LookupResult::MemoryTier {
                pointer: std::ptr::null_mut(),
                size: SLOT,
            },
        })
    }
    fn convert_to_storage(&self, key: CacheKey, offset: u64) -> Result<(), DispatchMapError> {
        let mut entries = self.entries.lock().unwrap();
        let e = entries
            .get_mut(&key)
            .ok_or(DispatchMapError::KeyNotFound(key))?;
        e.ssd_offset = Some(offset);
        Ok(())
    }
    fn take_read(&self, _: CacheKey) -> Result<(), DispatchMapError> {
        Ok(())
    }
    fn take_write(&self, _: CacheKey) -> Result<(), DispatchMapError> {
        Ok(())
    }
    fn release_read(&self, _: CacheKey) -> Result<(), DispatchMapError> {
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
                ssd_offset: None,
                on_block: false,
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
        matches!(entries.get(&key), Some(e) if !e.on_block && e.ssd_offset.is_some())
    }
    fn try_evict_to_block(&self, key: CacheKey) -> Result<(), DispatchMapError> {
        let mut entries = self.entries.lock().unwrap();
        let e = entries
            .get_mut(&key)
            .ok_or(DispatchMapError::KeyNotFound(key))?;
        if e.on_block || e.ssd_offset.is_none() {
            return Err(DispatchMapError::InvalidState(format!(
                "key {key} not demotable"
            )));
        }
        e.on_block = true;
        Ok(())
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

fn dispatcher(counters: Arc<TierEventCounters>) -> Arc<DispatcherComponent> {
    DispatcherComponent::new(
        AtomicBool::new(false),
        Mutex::new(None),
        Mutex::new(None),
        Mutex::new(None),
        RwLock::new(Vec::new()),
        RwLock::new(None),
        AtomicU64::new(0),
        Mutex::new(None),
        Mutex::new(None),
        AtomicUsize::new(2048),
        AtomicU64::new(0),
        RwLock::new(None),
        Arc::new(Mutex::new(None)),
        Arc::new(AtomicU64::new(0)),
        counters,
        Arc::new(crate::background::WriteThroughPins::default()),
    )
}

#[test]
fn eviction_reaches_persisted_entries_behind_an_unpersisted_admission_front() {
    // Real memory tier on the optimized policy, sized to hold exactly the 16 entries.
    let policy = EvictionPolicyOptimizedComponent::new_default();
    let ep: Arc<dyn IEvictionPolicy + Send + Sync> = query_interface!(policy, IEvictionPolicy)
        .expect("optimized policy provides IEvictionPolicy");
    let mt_comp = MemoryTierComponent::new_default();
    mt_comp.eviction_policy.connect(ep).unwrap();
    let mt: Arc<dyn IMemoryTier + Send + Sync> =
        query_interface!(mt_comp, IMemoryTier).expect("memory tier provides IMemoryTier");
    mt.initialize(((PERSISTED + UNPERSISTED) * SLOT as u64) as usize, None)
        .unwrap();

    let dm_concrete = Arc::new(Dm::default());
    let dm: Arc<dyn IDispatchMap + Send + Sync> = dm_concrete.clone();

    // Older entries whose write-through has landed: demotable.
    for key in 0..PERSISTED {
        mt.insert(key, SLOT).unwrap();
        dm.create_memory_tier_entry(key, std::ptr::null_mut(), SLOT)
            .unwrap();
        dm.convert_to_storage(key, key * SLOT as u64).unwrap();
    }
    // A store burst: fresh keys, write-through still in flight.
    for key in PERSISTED..PERSISTED + UNPERSISTED {
        mt.insert(key, SLOT).unwrap();
        dm.create_memory_tier_entry(key, std::ptr::null_mut(), SLOT)
            .unwrap();
    }

    // The precondition the finding rests on: the admission gate put the burst at the
    // front, so the dispatcher's first-step window holds nothing demotable.
    let window = mt.oldest_keys(4);
    assert!(
        window.iter().all(|k| *k >= PERSISTED),
        "TinyLFU should have admitted the unpersisted burst at the LRU front, got {window:?}",
    );
    assert_eq!(mt.used(), mt.capacity(), "the tier must start full");

    let counters = Arc::new(TierEventCounters::default());
    let c = dispatcher(Arc::clone(&counters));

    let res = c.evict_for_space(&dm, &mt, SLOT, 1000, 512);
    assert!(
        res.is_ok(),
        "8 demotable entries are resident, so eviction must free a slot instead of \
         reporting the tier full: {res:?} (stats {:?})",
        counters.snapshot(),
    );
    assert!(mt.used() + SLOT as usize <= mt.capacity());

    // What was freed must be a persisted entry, demoted rather than dropped, and every
    // unpersisted entry must still be resident.
    let demoted: Vec<CacheKey> = (0..PERSISTED)
        .filter(|k| matches!(dm.lookup(*k), Ok(LookupResult::BlockDevice { .. })))
        .collect();
    assert_eq!(
        demoted.len(),
        1,
        "exactly one persisted entry should be demoted"
    );
    for key in PERSISTED..PERSISTED + UNPERSISTED {
        assert!(
            mt.contains(key),
            "unpersisted key {key} must never be evicted"
        );
    }
}

#[test]
fn eviction_still_fails_when_nothing_in_the_tier_is_demotable() {
    // Guard against a fix that "frees" an unpersisted entry to make progress.
    let policy = EvictionPolicyOptimizedComponent::new_default();
    let ep: Arc<dyn IEvictionPolicy + Send + Sync> =
        query_interface!(policy, IEvictionPolicy).unwrap();
    let mt_comp = MemoryTierComponent::new_default();
    mt_comp.eviction_policy.connect(ep).unwrap();
    let mt: Arc<dyn IMemoryTier + Send + Sync> = query_interface!(mt_comp, IMemoryTier).unwrap();
    mt.initialize((UNPERSISTED * SLOT as u64) as usize, None)
        .unwrap();

    let dm: Arc<dyn IDispatchMap + Send + Sync> = Arc::new(Dm::default());
    for key in 0..UNPERSISTED {
        mt.insert(key, SLOT).unwrap();
        dm.create_memory_tier_entry(key, std::ptr::null_mut(), SLOT)
            .unwrap();
    }

    let c = dispatcher(Arc::new(TierEventCounters::default()));
    let res = c.evict_for_space(&dm, &mt, SLOT, 1000, 512);
    assert!(
        matches!(res, Err(DispatcherError::AllocationFailed(_))),
        "no entry is demotable, so this must fail: {res:?}",
    );
    for key in 0..UNPERSISTED {
        assert!(mt.contains(key), "key {key} must not be dropped");
    }
}
