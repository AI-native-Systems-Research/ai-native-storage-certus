//! Repair test for finding `failed-initialize-changes-config`
//! (DIS-INITIALIZE-FAILURE-NO-CONFIG-CHANGE).
//!
//! `initialize` stored the config's `max_eviction_attempts` and
//! `store_backpressure_ms` into the component's live atomics as its very
//! first act (src/lib.rs:2093-2096), before any validation. Every later
//! validation failure — a receptacle not bound, an empty `data_pci_addrs`,
//! a drive-creation error — therefore returned `Err` with the component's
//! eviction-attempt limit and store-backpressure budget already overwritten
//! by the rejected config, even though the dispatcher reports itself as not
//! initialized (DIS-INITIALIZE-FAILURE-LEAVES-UNINITIALIZED) and the property
//! record (verif/unified_properties.yaml, DIS-INITIALIZE-FAILURE-NO-CONFIG-
//! CHANGE) promises that "a call to initialize that fails validation does not
//! change the dispatcher's eviction-attempt limit or store-backpressure
//! budget from what they were before the call".
//!
//! The user-visible shape of the bug: a re-initialize attempt with a bad
//! config silently retunes a running dispatcher (how hard eviction tries,
//! how long a store backpressures before failing) even though the caller
//! was told the call failed and nothing was applied.
//!
//! Self-contained: only the public `IDispatcher` API plus same-crate reads
//! of the two atomics, with its own no-op mocks, so it builds against the
//! unfixed code as well.

use std::sync::atomic::Ordering;
use std::sync::{Arc, Mutex};

use component_core::query_interface;
use interfaces::{
    BounceBuffer, BounceError, CacheKey, DispatchMapError, DispatcherConfig, DispatcherError,
    IDispatchMap, IDispatcher, IMemoryTier, LookupResult, MemoryTierError,
    MemoryTierTelemetrySnapshot,
};

use crate::DispatcherComponent;

// --- no-op mocks: nothing on the dispatch map or the memory tier is called
//     on the paths under test (initialize only checks that the receptacles
//     are bound); they exist so the receptacles can be connected. ---

struct NoopDm;

impl IDispatchMap for NoopDm {
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
        Ok(0)
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

struct NoopMt;

impl IMemoryTier for NoopMt {
    fn initialize(&self, _pool_size: usize, _numa: Option<i32>) -> Result<(), MemoryTierError> {
        Ok(())
    }
    fn insert(&self, _key: CacheKey, _size: u32) -> Result<*mut u8, MemoryTierError> {
        Err(MemoryTierError::NotInitialized("noop".into()))
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
        0
    }
    fn used(&self) -> usize {
        0
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
    fn set_bounce_reserve(&self, _bytes: usize) -> Result<(), MemoryTierError> {
        Ok(())
    }
    fn bounce_alloc(&self, _len: u32) -> Result<BounceBuffer, BounceError> {
        Err(BounceError::NeverFits("no bounce reserve".into()))
    }
    fn bounce_free(&self, _buf: BounceBuffer) {}
    fn bounce_usage(&self) -> (usize, usize) {
        (0, 0)
    }
}

// --- helpers ---

/// A config offering the two tunables under test, distinct from every value
/// the component could legitimately hold before the call.
fn offered_config(max_att: usize, bp_ms: u64, addrs: &[&str]) -> DispatcherConfig {
    DispatcherConfig {
        data_pci_addrs: addrs.iter().map(|s| s.to_string()).collect(),
        max_eviction_attempts: max_att,
        store_backpressure_ms: bp_ms,
        // No background evictors, no formatting surprises.
        ssd_eviction_threshold: 0.0,
        memory_tier_eviction_threshold: 0.0,
        ..Default::default()
    }
}

// --- tests ---

/// DIS-INITIALIZE-FAILURE-NO-CONFIG-CHANGE: initialize that fails because a
/// receptacle is not bound (here: nothing at all is bound) must leave the
/// eviction-attempt limit and the store-backpressure budget untouched.
#[test]
fn failed_initialize_unbound_receptacle_leaves_config_unchanged() {
    let c = DispatcherComponent::new_default();
    let d = query_interface!(c, IDispatcher).unwrap();

    let before_att = c.max_eviction_attempts.load(Ordering::Relaxed);
    let before_bp = c.store_backpressure_ms.load(Ordering::Relaxed);
    // Guard against a vacuous pass: the offered values must differ.
    assert_ne!(before_att, 777, "test needs a non-777 starting limit");
    assert_ne!(before_bp, 999, "test needs a non-999 starting budget");

    let r = d.initialize(offered_config(777, 999, &["0000:02:00.0"]));
    assert!(
        matches!(r, Err(DispatcherError::NotInitialized(_))),
        "unbound dispatch_map must fail initialize, got {r:?}"
    );

    assert_eq!(
        c.max_eviction_attempts.load(Ordering::Relaxed),
        before_att,
        "a failed initialize must not change the eviction-attempt limit"
    );
    assert_eq!(
        c.store_backpressure_ms.load(Ordering::Relaxed),
        before_bp,
        "a failed initialize must not change the store-backpressure budget"
    );
}

/// DIS-INITIALIZE-FAILURE-NO-CONFIG-CHANGE: initialize that fails validation
/// of the config itself (empty `data_pci_addrs` → `InvalidParameter`) must
/// leave the two tunables untouched — on a fresh component and, the
/// user-visible case, on a dispatcher that is already running with a live
/// config: a rejected re-initialize must not retune it.
#[test]
fn failed_initialize_invalid_config_leaves_running_config_unchanged() {
    let c = DispatcherComponent::new_default();
    c.dispatch_map
        .connect(Arc::new(NoopDm) as Arc<dyn IDispatchMap + Send + Sync>)
        .unwrap();
    c.memory_tier
        .connect(Arc::new(NoopMt) as Arc<dyn IMemoryTier + Send + Sync>)
        .unwrap();
    let d = query_interface!(c, IDispatcher).unwrap();

    // Memory-tier-only mode (no SPDK env, no block-device factory): this
    // initialize succeeds and applies the config it was given.
    d.initialize(offered_config(111, 222, &["0000:02:00.0"]))
        .expect("memory-tier-only initialize must succeed");
    assert_eq!(
        c.max_eviction_attempts.load(Ordering::Relaxed),
        111,
        "a successful initialize applies the config's eviction-attempt limit"
    );
    assert_eq!(
        c.store_backpressure_ms.load(Ordering::Relaxed),
        222,
        "a successful initialize applies the config's store-backpressure budget"
    );

    // The rejected re-initialize: empty PCI list is an InvalidParameter.
    let r = d.initialize(offered_config(777, 999, &[]));
    assert!(
        matches!(r, Err(DispatcherError::InvalidParameter(_))),
        "empty data_pci_addrs must fail initialize, got {r:?}"
    );

    assert_eq!(
        c.max_eviction_attempts.load(Ordering::Relaxed),
        111,
        "a failed re-initialize must not change the running eviction-attempt limit"
    );
    assert_eq!(
        c.store_backpressure_ms.load(Ordering::Relaxed),
        222,
        "a failed re-initialize must not change the running store-backpressure budget"
    );

    d.shutdown().unwrap();
}
