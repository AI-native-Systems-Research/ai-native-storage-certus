//! Test doubles for the receptacles: a dispatch map that keeps a per-key
//! `read_ref` ledger (T025/T026's instrument) and a memory tier that reports a
//! pool. Their semantics follow the real `dispatch-map` where it matters: a
//! `MemoryTier` or `BlockDevice` answer is pinned, `NotExist` is not.

use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Mutex;

use crate::connection::mock::Sequence;
use interfaces::{
    BounceBuffer, BounceError, CacheKey, DispatchMapError, IDispatchMap, IMemoryTier, LookupResult,
    MemoryTierError, MemoryTierTelemetrySnapshot,
};

#[derive(Clone, Copy)]
enum Answer {
    Memory { addr: u64, size: u32 },
    Block { offset: u64 },
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
        self.set(key, Answer::Block { offset: 0 });
    }

    pub(crate) fn block_at(&self, key: CacheKey, offset: u64) {
        self.set(key, Answer::Block { offset });
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
            Some(Answer::Block { offset }) => {
                pinned(&mut l);
                LookupResult::BlockDevice { offset }
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
    fn bounce_alloc(&self, _: u32) -> Result<interfaces::BounceBuffer, interfaces::BounceError> {
        unimplemented!("the endpoint buffers only through the dispatcher (D13)")
    }
    fn bounce_free(&self, _: interfaces::BounceBuffer) {
        unimplemented!()
    }
    fn bounce_usage(&self) -> (usize, usize) {
        unimplemented!()
    }
}

/// A block source over a "disk" in memory, reading into buffers carved from a
/// range the test gives it — the top of its node's pool, as the memory tier's
/// bounce reserve is. Fixed-size buffers, so a test can exhaust them exactly.
/// Reads can be held until released, failed, or refused for lack of space;
/// each read and each buffer free is stamped from the shared sequence.
pub(crate) struct DiskSource {
    disk: Mutex<HashMap<u64, Vec<u8>>>,
    buffers: Mutex<Vec<u64>>,
    buffer_bytes: u32,
    hold: Mutex<bool>,
    released: std::sync::Condvar,
    fail: Mutex<std::collections::HashSet<u64>>,
    never_fits: Mutex<std::collections::HashSet<u64>>,
    reads: AtomicU64,
    reading: AtomicU64,
    peak_reading: AtomicU64,
    seq: Mutex<Option<Sequence>>,
    stamps: Mutex<Vec<(u64, &'static str, u64)>>,
}

// SAFETY: buffer addresses are integers here; the bytes they name belong to the
// test's pool, which outlives the endpoint using this source.
unsafe impl Send for DiskSource {}
// SAFETY: as above; all state is behind mutexes or atomics.
unsafe impl Sync for DiskSource {}

impl DiskSource {
    /// Buffers of `buffer_bytes` at `base`, `count` of them.
    pub(crate) fn new(base: u64, count: usize, buffer_bytes: u32) -> Self {
        DiskSource {
            disk: Mutex::new(HashMap::new()),
            buffers: Mutex::new(
                (0..count as u64)
                    .rev()
                    .map(|i| base + i * buffer_bytes as u64)
                    .collect(),
            ),
            buffer_bytes,
            hold: Mutex::new(false),
            released: std::sync::Condvar::new(),
            fail: Mutex::default(),
            never_fits: Mutex::default(),
            reads: AtomicU64::new(0),
            reading: AtomicU64::new(0),
            peak_reading: AtomicU64::new(0),
            seq: Mutex::new(None),
            stamps: Mutex::new(Vec::new()),
        }
    }

    /// Put `bytes` on the disk at extent `offset`.
    pub(crate) fn put(&self, offset: u64, bytes: &[u8]) {
        self.disk.lock().unwrap().insert(offset, bytes.to_vec());
    }

    /// Hold every read until [`release_reads`](Self::release_reads).
    pub(crate) fn hold_reads(&self) {
        *self.hold.lock().unwrap() = true;
    }

    pub(crate) fn release_reads(&self) {
        *self.hold.lock().unwrap() = false;
        self.released.notify_all();
    }

    /// Fail reads of the extent at `offset` with `Io`.
    pub(crate) fn fail_at(&self, offset: u64) {
        self.fail.lock().unwrap().insert(offset);
    }

    /// Answer reads of the extent at `offset` with `NeverFits`.
    pub(crate) fn never_fits_at(&self, offset: u64) {
        self.never_fits.lock().unwrap().insert(offset);
    }

    /// Reads begun, ever.
    pub(crate) fn reads(&self) -> u64 {
        self.reads.load(Ordering::SeqCst)
    }

    /// Reads in progress now, and the most at once.
    pub(crate) fn reading(&self) -> (u64, u64) {
        (
            self.reading.load(Ordering::SeqCst),
            self.peak_reading.load(Ordering::SeqCst),
        )
    }

    /// Buffers free now.
    pub(crate) fn free_buffers(&self) -> usize {
        self.buffers.lock().unwrap().len()
    }

    /// Stamp each read's end ("read") and each buffer free ("free"), by offset
    /// or address, from `seq`.
    pub(crate) fn record(&self, seq: &Sequence) {
        *self.seq.lock().unwrap() = Some(seq.clone());
    }

    pub(crate) fn stamps(&self) -> Vec<(u64, &'static str, u64)> {
        self.stamps.lock().unwrap().clone()
    }

    fn stamp(&self, what: &'static str, of: u64) {
        if let Some(s) = self.seq.lock().unwrap().as_ref() {
            self.stamps.lock().unwrap().push((s.next(), what, of));
        }
    }
}

impl crate::bounce::BlockSource for DiskSource {
    fn read(&self, _key: CacheKey, offset: u64, len: u32) -> Result<BounceBuffer, BounceError> {
        if self.never_fits.lock().unwrap().contains(&offset) || len > self.buffer_bytes {
            return Err(BounceError::NeverFits(format!("{len} bytes")));
        }
        let addr = self
            .buffers
            .lock()
            .unwrap()
            .pop()
            .ok_or(BounceError::NoSpace)?;
        self.reads.fetch_add(1, Ordering::SeqCst);
        let now = self.reading.fetch_add(1, Ordering::SeqCst) + 1;
        self.peak_reading.fetch_max(now, Ordering::SeqCst);
        let mut held = self.hold.lock().unwrap();
        while *held {
            held = self.released.wait(held).unwrap();
        }
        drop(held);
        self.reading.fetch_sub(1, Ordering::SeqCst);
        if self.fail.lock().unwrap().contains(&offset) {
            self.buffers.lock().unwrap().push(addr);
            self.stamp("read", offset);
            return Err(BounceError::Io("injected".into()));
        }
        let bytes = self
            .disk
            .lock()
            .unwrap()
            .get(&offset)
            .cloned()
            .unwrap_or_default();
        let n = bytes.len().min(len as usize);
        // SAFETY: `addr` is a buffer of `buffer_bytes >= len` bytes in the
        // test's pool, owned by this read until the buffer is freed.
        unsafe { std::ptr::copy_nonoverlapping(bytes.as_ptr(), addr as *mut u8, n) };
        self.stamp("read", offset);
        Ok(BounceBuffer {
            ptr: addr as *mut u8,
            len,
        })
    }

    fn release(&self, buf: BounceBuffer) {
        let mut l = self.buffers.lock().unwrap();
        assert!(
            !l.contains(&(buf.ptr as u64)),
            "bounce buffer {:?} freed twice",
            buf.ptr
        );
        l.push(buf.ptr as u64);
        drop(l);
        self.stamp("free", buf.ptr as u64);
    }
}

/// A dispatcher that serves `read_to_bounce` from a [`DiskSource`] and
/// implements nothing else: for checking that an `spdk` endpoint binds its
/// `dispatcher` receptacle and reads through it.
#[cfg(feature = "spdk")]
pub(crate) struct DiskDispatcher(pub(crate) std::sync::Arc<DiskSource>);

#[cfg(feature = "spdk")]
impl interfaces::IDispatcher for DiskDispatcher {
    fn read_to_bounce(
        &self,
        key: CacheKey,
        offset: u64,
        len: u32,
    ) -> Result<BounceBuffer, BounceError> {
        crate::bounce::BlockSource::read(&*self.0, key, offset, len)
    }
    fn release_bounce(&self, buf: BounceBuffer) {
        crate::bounce::BlockSource::release(&*self.0, buf)
    }

    fn initialize(
        &self,
        _: interfaces::DispatcherConfig,
    ) -> Result<(), interfaces::DispatcherError> {
        unimplemented!()
    }
    fn shutdown(&self) -> Result<(), interfaces::DispatcherError> {
        unimplemented!()
    }
    fn lookup(
        &self,
        _: CacheKey,
        _: interfaces::IpcHandle,
    ) -> Result<(), interfaces::DispatcherError> {
        unimplemented!()
    }
    fn lookup_async(
        &self,
        _: CacheKey,
        _: interfaces::IpcHandle,
    ) -> Result<interfaces::GpuStream, interfaces::DispatcherError> {
        unimplemented!()
    }
    fn batch_lookup(
        &self,
        _: &[(CacheKey, Vec<interfaces::IpcHandle>)],
    ) -> Vec<interfaces::LookupOutcome> {
        unimplemented!()
    }
    fn check(&self, _: CacheKey) -> Result<bool, interfaces::DispatcherError> {
        unimplemented!()
    }
    fn remove(&self, _: CacheKey) -> Result<(), interfaces::DispatcherError> {
        unimplemented!()
    }
    fn populate(
        &self,
        _: CacheKey,
        _: interfaces::IpcHandle,
    ) -> Result<(), interfaces::DispatcherError> {
        unimplemented!()
    }
    fn batch_populate(
        &self,
        _: &[(CacheKey, interfaces::IpcHandle)],
    ) -> Vec<Result<(), interfaces::DispatcherError>> {
        unimplemented!()
    }
    fn reserve_memory(
        &self,
        _: CacheKey,
        _: u32,
        _: u64,
        _: Option<std::time::Instant>,
    ) -> Result<*mut u8, interfaces::DispatcherError> {
        unimplemented!()
    }
    fn copy_gpu_to_memory_async(
        &self,
        _: CacheKey,
        _: &[interfaces::IpcHandle],
        _: interfaces::GpuStream,
    ) -> Result<(), interfaces::DispatcherError> {
        unimplemented!()
    }
    fn copy_gpu_to_memory_completed(
        &self,
        _: CacheKey,
        _: u32,
    ) -> Result<(), interfaces::DispatcherError> {
        unimplemented!()
    }
    fn release_memory(&self, _: CacheKey) -> Result<(), interfaces::DispatcherError> {
        unimplemented!()
    }
    fn pin(&self, _: CacheKey) -> Result<(), interfaces::DispatcherError> {
        unimplemented!()
    }
    fn unpin(&self, _: CacheKey) -> Result<(), interfaces::DispatcherError> {
        unimplemented!()
    }
    fn touch(&self, _: CacheKey) -> Result<(), interfaces::DispatcherError> {
        unimplemented!()
    }
    fn promote_to_memory_tier(&self, _: &[CacheKey]) {
        unimplemented!("serving a peer never promotes (FR-012)")
    }
    fn clear_memory_tier(&self) -> Result<usize, interfaces::DispatcherError> {
        unimplemented!()
    }
    fn flush_to_ssd(&self) -> Result<usize, interfaces::DispatcherError> {
        unimplemented!()
    }
    fn schedule_write_through(
        &self,
        _: CacheKey,
        _: u32,
    ) -> Result<(), interfaces::DispatcherError> {
        unimplemented!()
    }
    fn read_write_stats(&self) -> interfaces::ReadWriteStats {
        unimplemented!()
    }
    fn tier_event_stats(&self) -> interfaces::TierEventStats {
        unimplemented!()
    }
}
