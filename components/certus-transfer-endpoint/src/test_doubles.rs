//! Test doubles for the receptacles: a dispatch map that keeps a per-key
//! `read_ref` ledger (T025/T026's instrument) and a memory tier that reports a
//! pool. Their semantics follow the real `dispatch-map` where it matters: a
//! `MemoryTier` or `BlockDevice` answer is pinned, `NotExist` is not.

use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Mutex;

use crate::connection::mock::Sequence;
use interfaces::{
    CacheKey, DispatchMapError, IDispatchMap, IMemoryTier, LookupResult, MemoryTierError,
    MemoryTierTelemetrySnapshot,
};

#[derive(Clone, Copy)]
enum Answer {
    Memory { addr: u64, size: u32 },
    Block,
    Mismatch,
    Timeout,
}

#[derive(Default)]
struct Ledger {
    answers: HashMap<CacheKey, Answer>,
    read_ref: HashMap<CacheKey, i64>,
    peak_total: i64,
    /// Each release, stamped from the shared sequence when one is set.
    releases: Vec<(u64, CacheKey)>,
    seq: Option<Sequence>,
}

impl Ledger {
    fn total(&self) -> i64 {
        self.read_ref.values().sum()
    }
}

/// A dispatch map with a pin ledger. `read_ref` going negative panics: that is
/// a release of a pin nobody held, the hazard Principle IX names.
#[derive(Default)]
pub(crate) struct LedgerMap {
    ledger: Mutex<Ledger>,
    lookups: AtomicU64,
}

impl LedgerMap {
    pub(crate) fn new() -> Self {
        Self::default()
    }

    fn set(&self, key: CacheKey, a: Answer) {
        self.ledger.lock().unwrap().answers.insert(key, a);
    }

    pub(crate) fn memory(&self, key: CacheKey, addr: u64, size: u32) {
        self.set(key, Answer::Memory { addr, size });
    }

    pub(crate) fn block(&self, key: CacheKey) {
        self.set(key, Answer::Block);
    }

    pub(crate) fn mismatch(&self, key: CacheKey) {
        self.set(key, Answer::Mismatch);
    }

    pub(crate) fn timeout(&self, key: CacheKey) {
        self.set(key, Answer::Timeout);
    }

    /// A pin held by some other reader, so an over-release shows up.
    pub(crate) fn preload_reader(&self, key: CacheKey) {
        *self.ledger.lock().unwrap().read_ref.entry(key).or_default() += 1;
    }

    pub(crate) fn read_ref(&self, key: CacheKey) -> i64 {
        self.ledger
            .lock()
            .unwrap()
            .read_ref
            .get(&key)
            .copied()
            .unwrap_or(0)
    }

    /// Pins held across all keys right now.
    pub(crate) fn held(&self) -> i64 {
        self.ledger.lock().unwrap().total()
    }

    /// The most pins ever held at once, across all keys (SC-004).
    pub(crate) fn peak_held(&self) -> i64 {
        self.ledger.lock().unwrap().peak_total
    }

    pub(crate) fn lookups(&self) -> u64 {
        self.lookups.load(Ordering::Relaxed)
    }

    /// Stamp every release from `seq`, shared with the fabric's event log.
    pub(crate) fn record(&self, seq: &Sequence) {
        self.ledger.lock().unwrap().seq = Some(seq.clone());
    }

    /// Releases so far, stamped, in order.
    pub(crate) fn releases(&self) -> Vec<(u64, CacheKey)> {
        self.ledger.lock().unwrap().releases.clone()
    }
}

impl IDispatchMap for LedgerMap {
    fn lookup(&self, key: CacheKey) -> Result<LookupResult, DispatchMapError> {
        self.lookups.fetch_add(1, Ordering::Relaxed);
        let mut l = self.ledger.lock().unwrap();
        let answer = l.answers.get(&key).copied();
        let pinned = |l: &mut Ledger| {
            *l.read_ref.entry(key).or_default() += 1;
            l.peak_total = l.peak_total.max(l.total());
        };
        Ok(match answer {
            None => LookupResult::NotExist,
            Some(Answer::Mismatch) => LookupResult::MismatchSize,
            Some(Answer::Timeout) => return Err(DispatchMapError::Timeout(key)),
            Some(Answer::Block) => {
                pinned(&mut l);
                LookupResult::BlockDevice { offset: 0 }
            }
            Some(Answer::Memory { addr, size }) => {
                pinned(&mut l);
                LookupResult::MemoryTier {
                    pointer: addr as *mut u8,
                    size,
                }
            }
        })
    }

    fn release_read(&self, key: CacheKey) -> Result<(), DispatchMapError> {
        let mut l = self.ledger.lock().unwrap();
        let r = l.read_ref.entry(key).or_default();
        *r -= 1;
        assert!(
            *r >= 0,
            "release_read({key}) without a pin: another reader's pin was decremented"
        );
        if let Some(stamp) = l.seq.as_ref().map(Sequence::next) {
            l.releases.push((stamp, key));
        }
        Ok(())
    }

    fn initialize(&self) -> Result<(), DispatchMapError> {
        Ok(())
    }
    fn convert_to_storage(&self, _: CacheKey, _: u64) -> Result<(), DispatchMapError> {
        unimplemented!()
    }
    fn take_read(&self, _: CacheKey) -> Result<(), DispatchMapError> {
        unimplemented!("the endpoint pins only through lookup (D6)")
    }
    fn take_write(&self, _: CacheKey) -> Result<(), DispatchMapError> {
        unimplemented!()
    }
    fn release_write(&self, _: CacheKey) -> Result<(), DispatchMapError> {
        unimplemented!()
    }
    fn downgrade_reference(&self, _: CacheKey) -> Result<(), DispatchMapError> {
        unimplemented!()
    }
    fn remove(&self, _: CacheKey) -> Result<(), DispatchMapError> {
        unimplemented!()
    }
    fn touch(&self, _: CacheKey) -> Result<(), DispatchMapError> {
        unimplemented!()
    }
    fn entry_size(&self, _: CacheKey) -> Result<u32, DispatchMapError> {
        unimplemented!()
    }
    fn oldest_keys(&self, _: usize) -> Vec<CacheKey> {
        unimplemented!()
    }
    fn create_memory_tier_entry(
        &self,
        _: CacheKey,
        _: *mut u8,
        _: u32,
    ) -> Result<(), DispatchMapError> {
        unimplemented!()
    }
    fn convert_memory_tier_to_block(&self, _: CacheKey) -> Result<(), DispatchMapError> {
        unimplemented!()
    }
    fn promote_block_to_memory_tier(
        &self,
        _: CacheKey,
        _: *mut u8,
        _: u32,
    ) -> Result<(), DispatchMapError> {
        unimplemented!()
    }
    fn is_evictable(&self, _: CacheKey) -> bool {
        unimplemented!()
    }
    fn try_evict_to_block(&self, _: CacheKey) -> Result<(), DispatchMapError> {
        unimplemented!()
    }
    fn recover_extent(&self, _: CacheKey, _: u64, _: u32) -> Result<(), DispatchMapError> {
        unimplemented!()
    }
}

/// A memory tier that only reports its pool.
pub(crate) struct PoolTier {
    pub(crate) base: *mut u8,
    pub(crate) len: usize,
}

// SAFETY: only `pool_info` is called, which copies the two fields out; the
// pointer is never dereferenced through this type.
unsafe impl Send for PoolTier {}
// SAFETY: as above.
unsafe impl Sync for PoolTier {}

impl IMemoryTier for PoolTier {
    fn pool_info(&self) -> Option<(*mut u8, usize)> {
        (!self.base.is_null()).then_some((self.base, self.len))
    }

    fn initialize(&self, _: usize, _: Option<i32>) -> Result<(), MemoryTierError> {
        unimplemented!()
    }
    fn insert(&self, _: CacheKey, _: u32) -> Result<*mut u8, MemoryTierError> {
        unimplemented!()
    }
    fn get(&self, _: CacheKey) -> Option<(*mut u8, u32)> {
        unimplemented!()
    }
    fn peek(&self, _: CacheKey) -> Option<(*mut u8, u32)> {
        unimplemented!()
    }
    fn evict_next(&self) -> Option<CacheKey> {
        unimplemented!()
    }
    fn evict_next_for_key(&self, _: CacheKey) -> Option<CacheKey> {
        unimplemented!()
    }
    fn oldest_keys(&self, _: usize) -> Vec<CacheKey> {
        unimplemented!()
    }
    fn remove(&self, _: CacheKey) -> Result<(), MemoryTierError> {
        unimplemented!()
    }
    fn touch(&self, _: CacheKey) {
        unimplemented!()
    }
    fn batch_touch(&self, _: &[CacheKey]) {
        unimplemented!()
    }
    fn contains(&self, _: CacheKey) -> bool {
        unimplemented!()
    }
    fn capacity(&self) -> usize {
        self.len
    }
    fn used(&self) -> usize {
        unimplemented!()
    }
    fn is_dma_capable(&self) -> bool {
        unimplemented!()
    }
    fn clear(&self) -> Result<usize, MemoryTierError> {
        unimplemented!()
    }
    fn telemetry_snapshot(&self) -> MemoryTierTelemetrySnapshot {
        unimplemented!()
    }
    fn set_bounce_reserve(&self, _: usize) -> Result<(), MemoryTierError> {
        unimplemented!()
    }
    fn bounce_alloc(&self, _: u32) -> Result<interfaces::BounceLease, interfaces::BounceError> {
        unimplemented!("the endpoint leases only through the dispatcher (D13)")
    }
    fn bounce_free(&self, _: interfaces::BounceLease) {
        unimplemented!()
    }
    fn bounce_usage(&self) -> (usize, usize) {
        unimplemented!()
    }
}
