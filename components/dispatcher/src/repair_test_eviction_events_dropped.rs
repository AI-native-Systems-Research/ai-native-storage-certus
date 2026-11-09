//! DIS-INV-EVICTION-EVENTS (FR-042, FR-050): every entry the background
//! memory-tier evictor demotes must surface as a `Demoted` event or, when the
//! bounded event channel is full, be counted by `eviction_dropped_count()`.

use super::*;
use component_core::query_interface;
use interfaces::{DispatchMapError, LookupResult, MemoryTierError, MemoryTierTelemetrySnapshot};
use std::collections::HashSet;
use std::time::{Duration, Instant};

const SLOT: usize = 4096;

/// Memory tier holding a fixed set of resident keys (oldest first).
struct ResidentTier {
    keys: Mutex<Vec<CacheKey>>,
    capacity: usize,
}

impl IMemoryTier for ResidentTier {
    fn initialize(&self, _: usize, _: Option<i32>) -> Result<(), MemoryTierError> {
        Ok(())
    }
    fn insert(&self, _: CacheKey, _: u32) -> Result<*mut u8, MemoryTierError> {
        Err(MemoryTierError::PoolFull)
    }
    fn get(&self, _: CacheKey) -> Option<(*mut u8, u32)> {
        None
    }
    fn peek(&self, _: CacheKey) -> Option<(*mut u8, u32)> {
        None
    }
    fn evict_next(&self) -> Option<CacheKey> {
        None
    }
    fn evict_next_for_key(&self, _: CacheKey) -> Option<CacheKey> {
        None
    }
    fn oldest_keys(&self, n: usize) -> Vec<CacheKey> {
        self.keys.lock().unwrap().iter().take(n).copied().collect()
    }
    fn remove(&self, key: CacheKey) -> Result<(), MemoryTierError> {
        let mut keys = self.keys.lock().unwrap();
        let before = keys.len();
        keys.retain(|k| *k != key);
        if keys.len() == before {
            return Err(MemoryTierError::KeyNotFound(key));
        }
        Ok(())
    }
    fn touch(&self, _: CacheKey) {}
    fn batch_touch(&self, _: &[CacheKey]) {}
    fn contains(&self, key: CacheKey) -> bool {
        self.keys.lock().unwrap().contains(&key)
    }
    fn capacity(&self) -> usize {
        self.capacity
    }
    fn used(&self) -> usize {
        self.keys.lock().unwrap().len() * SLOT
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

/// Dispatch map in which every listed key is written through, so demotable.
struct DemotableMap {
    demotable: Mutex<HashSet<CacheKey>>,
}

impl IDispatchMap for DemotableMap {
    fn initialize(&self) -> Result<(), DispatchMapError> {
        Ok(())
    }
    fn lookup(&self, _: CacheKey) -> Result<LookupResult, DispatchMapError> {
        Ok(LookupResult::NotExist)
    }
    fn convert_to_storage(&self, k: CacheKey, _: u64) -> Result<(), DispatchMapError> {
        Err(DispatchMapError::KeyNotFound(k))
    }
    fn take_read(&self, k: CacheKey) -> Result<(), DispatchMapError> {
        Err(DispatchMapError::KeyNotFound(k))
    }
    fn take_write(&self, k: CacheKey) -> Result<(), DispatchMapError> {
        Err(DispatchMapError::KeyNotFound(k))
    }
    fn release_read(&self, k: CacheKey) -> Result<(), DispatchMapError> {
        Err(DispatchMapError::KeyNotFound(k))
    }
    fn release_write(&self, k: CacheKey) -> Result<(), DispatchMapError> {
        Err(DispatchMapError::KeyNotFound(k))
    }
    fn downgrade_reference(&self, k: CacheKey) -> Result<(), DispatchMapError> {
        Err(DispatchMapError::KeyNotFound(k))
    }
    fn remove(&self, k: CacheKey) -> Result<(), DispatchMapError> {
        Err(DispatchMapError::KeyNotFound(k))
    }
    fn touch(&self, k: CacheKey) -> Result<(), DispatchMapError> {
        Err(DispatchMapError::KeyNotFound(k))
    }
    fn entry_size(&self, k: CacheKey) -> Result<u32, DispatchMapError> {
        Err(DispatchMapError::KeyNotFound(k))
    }
    fn oldest_keys(&self, _: usize) -> Vec<CacheKey> {
        Vec::new()
    }
    fn create_memory_tier_entry(
        &self,
        k: CacheKey,
        _: *mut u8,
        _: u32,
    ) -> Result<(), DispatchMapError> {
        Err(DispatchMapError::KeyNotFound(k))
    }
    fn convert_memory_tier_to_block(&self, k: CacheKey) -> Result<(), DispatchMapError> {
        Err(DispatchMapError::KeyNotFound(k))
    }
    fn promote_block_to_memory_tier(
        &self,
        k: CacheKey,
        _: *mut u8,
        _: u32,
    ) -> Result<(), DispatchMapError> {
        Err(DispatchMapError::KeyNotFound(k))
    }
    fn is_evictable(&self, k: CacheKey) -> bool {
        self.demotable.lock().unwrap().contains(&k)
    }
    fn try_evict_to_block(&self, k: CacheKey) -> Result<(), DispatchMapError> {
        if self.demotable.lock().unwrap().remove(&k) {
            Ok(())
        } else {
            Err(DispatchMapError::KeyNotFound(k))
        }
    }
    fn recover_extent(&self, k: CacheKey, _: u64, _: u32) -> Result<(), DispatchMapError> {
        Err(DispatchMapError::KeyNotFound(k))
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

/// Logger that discards everything.
struct Quiet;

impl ILogger for Quiet {
    fn error(&self, _: &str) {}
    fn warn(&self, _: &str) {}
    fn info(&self, _: &str) {}
    fn debug(&self, _: &str) {}
}

#[test]
fn background_evictor_counts_demoted_events_that_do_not_fit() {
    let keys: Vec<CacheKey> = vec![1, 2, 3];
    let mt = Arc::new(ResidentTier {
        keys: Mutex::new(keys.clone()),
        capacity: 4 * SLOT,
    });
    let dm = Arc::new(DemotableMap {
        demotable: Mutex::new(keys.iter().copied().collect()),
    });

    let c = DispatcherComponent::new_default();
    c.dispatch_map
        .connect(Arc::clone(&dm) as Arc<dyn IDispatchMap + Send + Sync>)
        .unwrap();
    c.memory_tier
        .connect(Arc::clone(&mt) as Arc<dyn IMemoryTier + Send + Sync>)
        .unwrap();
    c.logger
        .connect(Arc::new(Quiet) as Arc<dyn ILogger + Send + Sync>)
        .unwrap();

    // Capacity 1 and nobody drains it while the evictor runs: of the three
    // Demoted events, one fits and two must be counted as dropped.
    let rx = c.create_eviction_channel(1);

    let d = query_interface!(c, IDispatcher).unwrap();
    d.initialize(DispatcherConfig {
        data_pci_addrs: vec!["0000:02:00.0".to_string()],
        memory_tier_eviction_threshold: 0.1,
        memory_tier_eviction_low_watermark: 0.0,
        memory_tier_eviction_batch_size: 4,
        memory_tier_eviction_interval_secs: 0,
        ..Default::default()
    })
    .unwrap();

    let deadline = Instant::now() + Duration::from_secs(10);
    while mt.used() > 0 {
        assert!(Instant::now() < deadline, "evictor did not demote the tier");
        std::thread::sleep(Duration::from_millis(1));
    }
    d.shutdown().unwrap();

    let delivered: Vec<EvictionEvent> = rx.try_iter().collect();
    for e in &delivered {
        assert_eq!(e.reason, EvictionReason::Demoted);
        assert!(keys.contains(&e.key));
    }
    let dropped = c.eviction_dropped_count();
    assert_eq!(
        delivered.len() as u64 + dropped,
        keys.len() as u64,
        "demoted {} entries, {} events delivered, {} counted dropped",
        keys.len(),
        delivered.len(),
        dropped
    );
}
