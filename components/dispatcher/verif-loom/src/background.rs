//! Mirror of `src/background.rs`: the per-drive background writer and one pass
//! of each evictor loop (the `sleep(interval)` pacing between passes and the
//! log lines are dropped; the shutdown flag is never set during a pass).

use crate::dmap::{DispatchMap, LookupResult};
use crate::stand::{Chan, ExtentManager, MemoryTier};
use crate::{CacheKey, Mutation};
use loom::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use loom::sync::{Arc, Condvar, Mutex};
use loom::thread::{self, JoinHandle};
use std::collections::{HashMap, HashSet};

/// Test rendezvous for the write-through job (DIS-EVICTOR-NO-FOREIGN-PIN-RELEASE):
/// `(arrived, released)`. A job that carries one parks after it has taken the
/// slot pointer but before it reads the slot (the window between
/// src/lib.rs:1474 and src/lib.rs:1503-1512), signalling `arrived`, and resumes
/// once `released` — forcing the model to explore a preemption exactly there.
pub type WriteGate = Arc<(Mutex<(bool, bool)>, Condvar)>;

/// mirrors src/background.rs:16 (WriteJob)
#[derive(Debug)]
pub struct WriteJob {
    pub key: CacheKey,
    pub size: u32,
    pub device_index: usize,
}

/// `crate::EvictionReason`
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EvictionReason {
    Demoted,
    Removed,
}

/// `crate::EvictionEvent`
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct EvictionEvent {
    pub key: CacheKey,
    pub reason: EvictionReason,
}

/// mirrors src/background.rs:231 (EvictionSender): the bounded event channel
/// plus the dispatcher's dropped-event counter.
#[derive(Clone)]
pub struct EvictionSender {
    pub tx: Arc<Chan<EvictionEvent>>,
    pub dropped: Arc<AtomicU64>,
}

impl EvictionSender {
    // mirrors src/background.rs:237 (emit)
    pub fn emit(&self, event: EvictionEvent) {
        if self.tx.try_send(event).is_err() {
            self.dropped.fetch_add(1, Ordering::Relaxed);
        }
    }
}

pub struct BackgroundWriter {
    shutdown: Arc<AtomicBool>,
    handle: Option<JoinHandle<()>>,
    sender: Arc<Chan<WriteJob>>,
    in_flight: Arc<AtomicUsize>,
}

impl BackgroundWriter {
    // mirrors src/background.rs:47 (start_named)
    pub fn start_named<F>(mut process_job: F, m: Mutation) -> Self
    where
        F: FnMut(WriteJob) + Send + 'static,
    {
        let chan = Arc::new(Chan::unbounded());
        let shutdown = Arc::new(AtomicBool::new(false));
        let in_flight = Arc::new(AtomicUsize::new(0));
        let (rx, sd, inf) = (chan.clone(), shutdown.clone(), in_flight.clone());
        let handle = thread::spawn(move || {
            Self::worker_loop(&sd, &rx, &inf, &mut process_job, m);
        });
        Self { shutdown, handle: Some(handle), sender: chan, in_flight }
    }

    // mirrors src/background.rs:79 (enqueue)
    pub fn enqueue(&self, job: WriteJob) -> Result<(), WriteJob> {
        self.in_flight.fetch_add(1, Ordering::Relaxed);
        self.sender.send(job).map_err(|j| {
            self.in_flight.fetch_sub(1, Ordering::Relaxed);
            j
        })
    }

    // mirrors src/background.rs:90 (in_flight)
    pub fn in_flight(&self) -> usize {
        self.in_flight.load(Ordering::Acquire)
    }

    // mirrors src/background.rs:109 (shutdown). Closing the channel stands for
    // the shutdown flag being observed on the next 50 ms recv timeout, after
    // which the worker drains the queue and returns.
    pub fn shutdown(&mut self) {
        self.shutdown.store(true, Ordering::Release);
        self.sender.close();
        if let Some(h) = self.handle.take() {
            let _ = h.join();
        }
    }

    // mirrors src/background.rs:118 (worker_loop)
    fn worker_loop<F>(
        _shutdown: &AtomicBool,
        receiver: &Chan<WriteJob>,
        in_flight: &AtomicUsize,
        process_job: &mut F,
        m: Mutation,
    ) where
        F: FnMut(WriteJob),
    {
        // `recv_timeout(50ms)`: Ok(job) | (Timeout && shutdown => drain, return).
        while let Some(job) = receiver.recv() {
            if m == Mutation::FlushEarlyDecrement {
                // MUTANT(FlushEarlyDecrement): count the job done before it is.
                in_flight.fetch_sub(1, Ordering::Release);
                process_job(job);
            } else {
                process_job(job);
                in_flight.fetch_sub(1, Ordering::Release);
            }
        }
    }
}

/// mirrors src/background.rs:163 (ParallelBackgroundWriter)
pub struct ParallelBackgroundWriter {
    writers: Vec<BackgroundWriter>,
    num_drives: usize,
}

impl ParallelBackgroundWriter {
    // mirrors src/background.rs:173 (start)
    pub fn start<F>(num_drives: usize, make_processor: impl Fn(usize) -> F, m: Mutation) -> Self
    where
        F: FnMut(WriteJob) + Send + 'static,
    {
        let writers =
            (0..num_drives).map(|idx| BackgroundWriter::start_named(make_processor(idx), m)).collect();
        Self { writers, num_drives }
    }

    // mirrors src/background.rs:188 (enqueue)
    pub fn enqueue(&self, job: WriteJob) -> Result<(), WriteJob> {
        let idx = job.device_index % self.num_drives;
        self.writers[idx].enqueue(job)
    }

    // mirrors src/background.rs:194 (in_flight)
    pub fn in_flight(&self) -> usize {
        self.writers.iter().map(|w| w.in_flight()).sum()
    }

    // mirrors src/background.rs:199 (flush); sleep(5ms) -> yield_now.
    pub fn flush(&self) {
        loop {
            if self.writers.iter().all(|w| w.in_flight() == 0) {
                return;
            }
            thread::yield_now();
        }
    }

    // mirrors src/background.rs:209 (shutdown)
    pub fn shutdown(&mut self) {
        for w in &mut self.writers {
            w.shutdown();
        }
    }
}

/// mirrors src/background.rs:260 (WriteThroughPins): owner ledger of the read
/// pin `populate` leaves for its write-through job, and of the keys whose
/// latest write-through failed.
#[derive(Default)]
pub struct WriteThroughPins {
    keys: Mutex<HashMap<CacheKey, PinState>>,
    failed: Mutex<HashSet<CacheKey>>,
}

/// mirrors src/background.rs:267 (PinState)
#[derive(Default)]
struct PinState {
    pinned: bool,
    queued: bool,
    running: bool,
    removed: bool,
    stale: u32,
}

/// mirrors src/background.rs:281 (JobStart)
pub enum JobStart {
    Owned,
    Stale,
    Untracked,
}

impl WriteThroughPins {
    // mirrors src/background.rs:293 (pin)
    pub fn pin(&self, key: CacheKey, enqueue: impl FnOnce() -> bool) {
        let mut keys = self.keys.lock().unwrap();
        let s = keys.entry(key).or_default();
        s.pinned = true;
        s.queued = enqueue();
    }

    // mirrors src/background.rs:301 (start)
    pub fn start(&self, key: CacheKey) -> JobStart {
        let mut keys = self.keys.lock().unwrap();
        let Some(s) = keys.get_mut(&key) else {
            return JobStart::Untracked;
        };
        if s.stale > 0 {
            s.stale -= 1;
            Self::forget_if_idle(&mut keys, key);
            return JobStart::Stale;
        }
        if !s.queued {
            return JobStart::Untracked;
        }
        s.queued = false;
        s.running = true;
        JobStart::Owned
    }

    // mirrors src/background.rs:323 (finish)
    pub fn finish(&self, key: CacheKey, settle: impl FnOnce(bool)) {
        let mut keys = self.keys.lock().unwrap();
        let owned = keys.get_mut(&key).map_or(true, |s| {
            s.running = false;
            s.pinned = false;
            !std::mem::take(&mut s.removed)
        });
        settle(owned);
        Self::forget_if_idle(&mut keys, key);
    }

    // mirrors src/background.rs:340 (remove_entry)
    pub fn remove_entry(&self, dm: &DispatchMap, key: CacheKey) -> Option<bool> {
        let mut keys = self.keys.lock().unwrap();
        let Some(s) = keys.get_mut(&key).filter(|s| s.pinned) else {
            return dm.remove(key).ok().map(|()| false);
        };
        dm.release_read(key).ok()?;
        if dm.remove(key).is_err() {
            let _ = dm.take_read(key);
            return None;
        }
        s.pinned = false;
        if std::mem::take(&mut s.queued) {
            s.stale += 1;
        }
        s.removed = s.running;
        let job_frees_slot = s.running;
        Self::forget_if_idle(&mut keys, key);
        Some(job_frees_slot)
    }

    // mirrors src/background.rs:365 (record_write)
    pub fn record_write(&self, key: CacheKey, landed: bool) {
        let mut failed = self.failed.lock().unwrap();
        if landed {
            failed.remove(&key);
        } else {
            failed.insert(key);
        }
    }

    // mirrors src/background.rs:376 (failed_writes)
    pub fn failed_writes(&self, resident: impl Fn(CacheKey) -> bool) -> usize {
        let mut failed = self.failed.lock().unwrap();
        failed.retain(|&key| resident(key));
        failed.len()
    }

    // mirrors src/background.rs:382 (forget_if_idle)
    fn forget_if_idle(keys: &mut HashMap<CacheKey, PinState>, key: CacheKey) {
        if keys
            .get(&key)
            .is_some_and(|s| !s.pinned && !s.queued && !s.running && s.stale == 0)
        {
            keys.remove(&key);
        }
    }
}

/// What one evictor pass did (observation only).
#[derive(Debug, Default, Clone)]
pub struct PassReport {
    /// Utilization was at/over the threshold, so the pass scanned.
    pub scanned: bool,
    pub candidates: Vec<CacheKey>,
    pub evicted: Vec<CacheKey>,
    /// The loop ended by the low-watermark `break` (else: candidates exhausted).
    pub stopped_at_watermark: bool,
    /// Utilization the loop observed at its last re-check (or at entry).
    pub last_util: f64,
}

/// mirrors src/background.rs:569 (MemoryTierEvictorConfig) / :372 (EvictorConfig)
#[derive(Debug, Clone, Copy)]
pub struct EvictorConfig {
    pub threshold: f64,
    pub low_watermark: f64,
    pub batch_size: usize,
}

// mirrors src/background.rs:621 (MemoryTierEvictor::evictor_loop), one pass.
pub fn mt_evictor_pass(
    dm: &DispatchMap,
    mt: &MemoryTier,
    config: &EvictorConfig,
    eviction_tx: &Mutex<Option<EvictionSender>>,
    consecutive_dry_runs: u32,
    m: Mutation,
) -> PassReport {
    let mut rep = PassReport::default();
    let used = mt.used();
    let capacity = mt.capacity();
    if capacity == 0 {
        return rep;
    }
    let utilization = used as f64 / capacity as f64;
    rep.last_util = utilization;
    if utilization < config.threshold {
        return rep;
    }
    rep.scanned = true;

    let headroom = 1.0 - config.threshold;
    let pressure = if headroom > 0.0 {
        ((utilization - config.threshold) / headroom).min(1.0)
    } else {
        1.0
    };
    let multiplier = 1.0 + 7.0 * pressure * pressure;
    let effective_batch = ((config.batch_size as f64 * multiplier) as usize).max(1);

    let scan_size = effective_batch * (consecutive_dry_runs as usize + 1).min(4);
    let candidates = mt.oldest_keys(scan_size);
    rep.candidates = candidates.clone();

    for key in candidates {
        if dm.try_evict_to_block(key).is_err() {
            continue;
        }
        if mt.remove(key).is_err() {
            continue;
        }
        if let Some(ref tx) = *eviction_tx.lock().unwrap() {
            // MUTANT(EvictorWrongReason): wrong reason on the event.
            let reason = if m == Mutation::EvictorWrongReason {
                EvictionReason::Removed
            } else {
                EvictionReason::Demoted
            };
            tx.emit(EvictionEvent { key, reason });
        }
        rep.evicted.push(key);

        let used_now = mt.used();
        let util_now = used_now as f64 / capacity as f64;
        rep.last_util = util_now;
        if util_now < config.low_watermark || m == Mutation::MtEvictorStopAfterOne {
            // MUTANT(MtEvictorStopAfterOne): `|| true` on the stop condition.
            rep.stopped_at_watermark = true;
            break;
        }
    }
    rep
}

// mirrors src/background.rs:522 (compute_utilization)
pub fn compute_utilization(extent_mgrs: &[Arc<ExtentManager>]) -> (u64, u64) {
    let mut total_used = 0u64;
    let mut total_cap = 0u64;
    for em in extent_mgrs {
        total_used += em.used_bytes();
        total_cap += em.capacity_bytes();
    }
    (total_used, total_cap)
}

// mirrors src/background.rs:558 (get_evictable_offset)
pub fn get_evictable_offset(dm: &DispatchMap, key: CacheKey, m: Mutation) -> Option<u64> {
    match dm.lookup(key) {
        Ok(LookupResult::BlockDevice { offset }) => {
            let _ = dm.release_read(key);
            Some(offset)
        }
        Ok(LookupResult::MemoryTier { .. }) => {
            let _ = dm.release_read(key);
            None
        }
        Ok(LookupResult::NotExist) => {
            // Mirrors the fixed catch-all Ok(_) arm (background.rs:572-578): a
            // NotExist candidate took no read reference — the map's lookup
            // only pins existing entries — so none is released. The release
            // this arm used to perform was the foreign release of spec-sync
            // A-11: in the race where populate registers the candidate between
            // this lookup and that release, it stole the write-through pin
            // populate's downgrade leaves, and a later remove freed the slot
            // under the entry's write-through job.
            if m == Mutation::EvictorReleasesForeign {
                // MUTANT(EvictorReleasesForeign): the foreign release the fix
                // removed, so the twin still catches the regression.
                let _ = dm.release_read(key);
            }
            None
        }
        Err(_) => None,
    }
}

// mirrors src/background.rs:426 (BackgroundEvictor::evictor_loop), one pass.
pub fn ssd_evictor_pass(
    dm: &DispatchMap,
    mt: &MemoryTier,
    extent_mgrs: &[Arc<ExtentManager>],
    config: &EvictorConfig,
    eviction_tx: Option<&EvictionSender>,
    m: Mutation,
) -> PassReport {
    let mut rep = PassReport::default();
    let (used, capacity) = compute_utilization(extent_mgrs);
    if capacity == 0 {
        return rep;
    }
    let utilization = used as f64 / capacity as f64;
    rep.last_util = utilization;
    if utilization < config.threshold {
        return rep;
    }
    rep.scanned = true;

    let candidates = dm.oldest_keys(config.batch_size);
    rep.candidates = candidates.clone();

    for key in candidates {
        let offset = match get_evictable_offset(dm, key, m) {
            Some(o) => o,
            None => continue,
        };
        // dispatch-map first: it refuses a pinned entry; then the slot.
        if dm.remove(key).is_err() {
            continue;
        }
        let _ = mt.remove(key);
        if let Some(tx) = eviction_tx {
            tx.emit(EvictionEvent { key, reason: EvictionReason::Removed });
        }
        let drive_idx = crate::dispatcher::drive_index(key, extent_mgrs.len().max(1));
        if let Some(em) = extent_mgrs.get(drive_idx) {
            em.remove_extent(offset);
        }
        rep.evicted.push(key);

        let (used_now, _) = compute_utilization(extent_mgrs);
        let util_now = used_now as f64 / capacity as f64;
        rep.last_util = util_now;
        if util_now < config.low_watermark || m == Mutation::SsdEvictorStopAfterOne {
            // MUTANT(SsdEvictorStopAfterOne): `|| true` on the stop condition.
            rep.stopped_at_watermark = true;
            break;
        }
    }
    rep
}
