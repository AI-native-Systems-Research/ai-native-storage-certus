//! Background write worker for memory-tier-to-SSD persistence and SSD eviction.

use crossbeam_channel::{Receiver, Sender};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::{self, JoinHandle};
use std::time::Duration;

use interfaces::{
    CacheKey, IDispatchMap, IExtentManager, ILogger, IMemoryTier, LookupResult, WriteHandle,
};

use crate::{EvictionEvent, EvictionReason, TierEventCounters};

/// A job for the background writer to persist a memory-tier entry to SSD.
#[derive(Debug)]
pub struct WriteJob {
    /// Cache key identifying the entry.
    pub key: u64,
    /// Size of the data in bytes.
    pub size: u32,
    /// Index of the data block device to write to (single-drive layout only;
    /// a striped object's slices are routed by `StripeSlice::drive`).
    pub device_index: usize,
    /// The one stripe slice this job writes when the object is striped across
    /// several drives; `None` writes the whole object as one job.
    pub stripe: Option<StripeSlice>,
}

/// One drive's slice of a striped write-through. Slice `drive` is written
/// only by writer `drive`, so per-drive write concurrency is unchanged.
#[derive(Debug)]
pub struct StripeSlice {
    /// Drive (and writer) this slice belongs to.
    pub drive: usize,
    /// State shared by every slice of the object.
    pub state: Arc<StripeWrite>,
}

/// Extent reservation shared by the slices of one striped object.
#[derive(Debug)]
pub enum StripeReservation {
    /// No slice has run yet.
    Pending,
    /// The first slice to run peeked the slot and reserved `offset` on the
    /// stripe allocator; every slice writes its stripe at that offset.
    Reserved {
        handle: WriteHandle,
        offset: u64,
        mem_ptr: *mut u8,
    },
    /// The slot was gone or the reservation failed.
    Failed,
}

// SAFETY: `mem_ptr` is the object's memory-tier slot, kept pinned by the
// write-through read ref until the last slice finishes; it is only read from
// the writer threads, and access to the reservation is serialized by the
// `StripeWrite` mutex.
unsafe impl Send for StripeReservation {}

/// Shared state of one striped write-through: the reservation, the number of
/// slices still to finish and whether any slice failed.
#[derive(Debug)]
pub struct StripeWrite {
    pub reservation: Mutex<StripeReservation>,
    remaining: AtomicUsize,
    failed: AtomicBool,
}

impl StripeWrite {
    fn new(slices: usize) -> Self {
        Self {
            reservation: Mutex::new(StripeReservation::Pending),
            remaining: AtomicUsize::new(slices),
            failed: AtomicBool::new(false),
        }
    }

    /// Record that a slice failed; the object will be aborted.
    pub fn mark_failed(&self) {
        self.failed.store(true, Ordering::Release);
    }

    /// Whether any slice has failed so far.
    pub fn has_failed(&self) -> bool {
        self.failed.load(Ordering::Acquire)
    }

    /// Count one slice as finished. For the last slice only, returns the
    /// reservation together with whether every slice succeeded; the caller
    /// then publishes it or drops it (abort), exactly once per object.
    pub fn finish_slice(&self) -> Option<(StripeReservation, bool)> {
        if self.remaining.fetch_sub(1, Ordering::AcqRel) != 1 {
            return None;
        }
        let reservation = std::mem::replace(
            &mut *self.reservation.lock().unwrap_or_else(|e| e.into_inner()),
            StripeReservation::Failed,
        );
        Some((reservation, !self.has_failed()))
    }
}

/// Handle to the background writer thread.
pub struct BackgroundWriter {
    shutdown: Arc<AtomicBool>,
    handle: Option<JoinHandle<()>>,
    sender: Sender<WriteJob>,
    in_flight: Arc<std::sync::atomic::AtomicUsize>,
}

impl BackgroundWriter {
    /// Start the background writer thread.
    ///
    /// The thread drains `WriteJob`s from the channel until the shutdown
    /// flag is set and the channel is empty.
    #[cfg(test)]
    pub fn start<F>(process_job: F) -> Self
    where
        F: FnMut(WriteJob) + Send + 'static,
    {
        Self::start_named(0, process_job)
    }

    /// Start a named background writer thread (used by `ParallelBackgroundWriter`).
    pub(crate) fn start_named<F>(drive_idx: usize, mut process_job: F) -> Self
    where
        F: FnMut(WriteJob) + Send + 'static,
    {
        let (sender, receiver): (Sender<WriteJob>, Receiver<WriteJob>) =
            crossbeam_channel::unbounded();
        let shutdown = Arc::new(AtomicBool::new(false));
        let shutdown_clone = Arc::clone(&shutdown);
        let in_flight = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let in_flight_clone = Arc::clone(&in_flight);

        let handle = thread::Builder::new()
            .name(format!("dispatcher-bg-writer-{drive_idx}"))
            .spawn(move || {
                Self::worker_loop(
                    &shutdown_clone,
                    &receiver,
                    &in_flight_clone,
                    &mut process_job,
                );
            })
            .expect("failed to spawn background writer thread");

        Self {
            shutdown,
            handle: Some(handle),
            sender,
            in_flight,
        }
    }

    /// Enqueue a write job for background processing.
    pub fn enqueue(&self, job: WriteJob) -> Result<(), WriteJob> {
        self.in_flight
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        self.sender.send(job).map_err(|e| {
            self.in_flight
                .fetch_sub(1, std::sync::atomic::Ordering::Relaxed);
            e.0
        })
    }

    /// Return the number of jobs currently in-flight (enqueued but not yet processed).
    pub fn in_flight(&self) -> usize {
        self.in_flight.load(std::sync::atomic::Ordering::Acquire)
    }

    /// Block until all jobs enqueued before this call have been processed.
    ///
    /// Jobs enqueued concurrently by other threads after this call begins are
    /// not guaranteed to be complete when this returns.
    #[cfg(test)]
    #[allow(dead_code)]
    pub fn flush(&self) {
        while self.in_flight.load(std::sync::atomic::Ordering::Acquire) > 0 {
            std::thread::sleep(Duration::from_millis(5));
        }
    }

    /// Signal shutdown and wait for the background thread to finish.
    ///
    /// All jobs already in the channel are processed before the thread exits.
    pub fn shutdown(&mut self) {
        self.shutdown.store(true, Ordering::Release);
        // Drop a cloned sender to help close the channel for any receivers.
        drop(self.sender.clone());
        if let Some(handle) = self.handle.take() {
            let _ = handle.join();
        }
    }

    fn worker_loop<F>(
        shutdown: &AtomicBool,
        receiver: &Receiver<WriteJob>,
        in_flight: &std::sync::atomic::AtomicUsize,
        process_job: &mut F,
    ) where
        F: FnMut(WriteJob),
    {
        loop {
            match receiver.recv_timeout(std::time::Duration::from_millis(50)) {
                Ok(job) => {
                    process_job(job);
                    in_flight.fetch_sub(1, std::sync::atomic::Ordering::Release);
                }
                Err(crossbeam_channel::RecvTimeoutError::Timeout) => {
                    if shutdown.load(Ordering::Acquire) {
                        while let Ok(job) = receiver.try_recv() {
                            process_job(job);
                            in_flight.fetch_sub(1, std::sync::atomic::Ordering::Release);
                        }
                        return;
                    }
                }
                Err(crossbeam_channel::RecvTimeoutError::Disconnected) => return,
            }
        }
    }
}

impl Drop for BackgroundWriter {
    fn drop(&mut self) {
        if self.handle.is_some() {
            self.shutdown();
        }
    }
}

// ---------------------------------------------------------------------------
// Parallel Background Writer (one thread per drive)
// ---------------------------------------------------------------------------

/// A pool of per-drive `BackgroundWriter` threads.
///
/// With one drive, routes each `WriteJob` to that drive's writer. With
/// several, every object is striped across all drives: each job is fanned out
/// into one slice job per drive, and slice `d` goes to writer `d`, enabling
/// concurrent write-through across multiple NVMe devices.
pub struct ParallelBackgroundWriter {
    writers: Vec<BackgroundWriter>,
    num_drives: usize,
    block_size: usize,
}

impl ParallelBackgroundWriter {
    /// Start one writer thread per drive.
    ///
    /// `block_size` is the (common) data-drive block size the stripe layout is
    /// computed with. `make_processor(drive_idx)` is called once per drive to
    /// produce the job-processing closure for that drive's dedicated thread.
    pub fn start<F>(
        num_drives: usize,
        block_size: usize,
        make_processor: impl Fn(usize) -> F,
    ) -> Self
    where
        F: FnMut(WriteJob) + Send + 'static,
    {
        let writers = (0..num_drives)
            .map(|idx| BackgroundWriter::start_named(idx, make_processor(idx)))
            .collect();

        Self {
            writers,
            num_drives,
            block_size: block_size.max(1),
        }
    }

    /// Enqueue a write job.
    ///
    /// With one drive the job is routed whole to that drive's writer. With
    /// several, it is fanned out into one slice job per drive holding a
    /// non-empty stripe, slice `d` on writer `d`; all slices are enqueued
    /// before this returns, so `flush()` never observes idle writers while
    /// slices are pending. A slice refused by a writer (shutdown) counts as
    /// failed. `Err` is returned only when no writer will finish the object,
    /// in which case the caller owns its write-through read ref.
    pub fn enqueue(&self, job: WriteJob) -> Result<(), WriteJob> {
        let n = self.num_drives;
        let total_bytes = job.size as usize;
        let slices: Vec<usize> = if n > 1 {
            (0..n)
                .filter(|&d| crate::stripe_range(total_bytes, n, self.block_size, d).1 > 0)
                .collect()
        } else {
            Vec::new()
        };
        if slices.is_empty() {
            let idx = job.device_index % n;
            return self.writers[idx].enqueue(job);
        }

        let state = Arc::new(StripeWrite::new(slices.len()));
        let mut unowned: Option<WriteJob> = None;
        for d in slices {
            let slice_job = WriteJob {
                key: job.key,
                size: job.size,
                device_index: d,
                stripe: Some(StripeSlice {
                    drive: d,
                    state: Arc::clone(&state),
                }),
            };
            if let Err(rejected) = self.writers[d].enqueue(slice_job) {
                state.mark_failed();
                if let Some((reservation, _)) = state.finish_slice() {
                    drop(reservation); // abort any reserved extent
                    unowned = Some(rejected);
                }
            }
        }
        match unowned {
            Some(rejected) => Err(rejected),
            None => Ok(()),
        }
    }

    /// Total number of jobs in-flight across all drive writers.
    pub fn in_flight(&self) -> usize {
        self.writers.iter().map(|w| w.in_flight()).sum()
    }

    /// Block until all per-drive queues are drained.
    pub fn flush(&self) {
        loop {
            if self.writers.iter().all(|w| w.in_flight() == 0) {
                return;
            }
            std::thread::sleep(Duration::from_millis(5));
        }
    }

    /// Shutdown all per-drive writer threads, draining remaining jobs.
    pub fn shutdown(&mut self) {
        for writer in &mut self.writers {
            writer.shutdown();
        }
    }
}

impl Drop for ParallelBackgroundWriter {
    fn drop(&mut self) {
        self.shutdown();
    }
}

// ---------------------------------------------------------------------------
// Background SSD Evictor
// ---------------------------------------------------------------------------

pub struct EvictorConfig {
    pub threshold: f64,
    pub low_watermark: f64,
    pub batch_size: usize,
    pub interval: Duration,
}

pub struct BackgroundEvictor {
    shutdown: Arc<AtomicBool>,
    handle: Option<JoinHandle<()>>,
}

impl BackgroundEvictor {
    pub fn start(
        dm: Arc<dyn IDispatchMap + Send + Sync>,
        mt: Arc<dyn IMemoryTier + Send + Sync>,
        extent_mgrs: Vec<Arc<dyn IExtentManager + Send + Sync>>,
        config: EvictorConfig,
        logger: Option<Arc<dyn ILogger + Send + Sync>>,
        eviction_tx: Option<Sender<EvictionEvent>>,
        tier_counters: Arc<TierEventCounters>,
    ) -> Self {
        let shutdown = Arc::new(AtomicBool::new(false));
        let shutdown_clone = Arc::clone(&shutdown);

        let handle = thread::Builder::new()
            .name("dispatcher-ssd-evictor".into())
            .spawn(move || {
                Self::evictor_loop(
                    &shutdown_clone,
                    &dm,
                    &mt,
                    &extent_mgrs,
                    &config,
                    logger.as_deref(),
                    eviction_tx.as_ref(),
                    &tier_counters,
                );
            })
            .expect("failed to spawn SSD evictor thread");

        Self {
            shutdown,
            handle: Some(handle),
        }
    }

    pub fn shutdown(&mut self) {
        self.shutdown.store(true, Ordering::Release);
        if let Some(handle) = self.handle.take() {
            let _ = handle.join();
        }
    }

    fn evictor_loop(
        shutdown: &AtomicBool,
        dm: &Arc<dyn IDispatchMap + Send + Sync>,
        mt: &Arc<dyn IMemoryTier + Send + Sync>,
        extent_mgrs: &[Arc<dyn IExtentManager + Send + Sync>],
        config: &EvictorConfig,
        logger: Option<&(dyn ILogger + Send + Sync)>,
        eviction_tx: Option<&Sender<EvictionEvent>>,
        tier_counters: &TierEventCounters,
    ) {
        loop {
            thread::sleep(config.interval);

            if shutdown.load(Ordering::Acquire) {
                return;
            }

            let (used, capacity) = Self::compute_utilization(extent_mgrs);
            if capacity == 0 {
                continue;
            }

            let utilization = used as f64 / capacity as f64;
            if utilization < config.threshold {
                continue;
            }

            if let Some(log) = logger {
                log.info(&format!(
                    "ssd-evictor: utilization {:.1}% exceeds threshold {:.1}%, evicting",
                    utilization * 100.0,
                    config.threshold * 100.0,
                ));
            }

            let candidates = dm.oldest_keys(config.batch_size);
            let mut evicted = 0u32;

            for key in candidates {
                if shutdown.load(Ordering::Acquire) {
                    return;
                }

                let offset = match Self::get_evictable_offset(dm, key) {
                    Some(o) => o,
                    None => continue,
                };

                // Remove from memory-tier (no-op if not present).
                let _ = mt.remove(key);

                // Remove from dispatch-map.
                if dm.remove(key).is_err() {
                    continue;
                }

                if let Some(tx) = eviction_tx {
                    let _ = tx.try_send(EvictionEvent {
                        key,
                        reason: EvictionReason::Removed,
                    });
                }

                // Free the extent on the stripe allocator. Objects are striped
                // across all drives at one offset reserved on drive 0's extent
                // manager, which is the only one this evictor is given, so
                // drive_index(key, 1) always selects it.
                let drive_idx =
                    crate::DispatcherComponent::drive_index(key, extent_mgrs.len().max(1));
                if let Some(em) = extent_mgrs.get(drive_idx) {
                    let _ = em.remove_extent(offset);
                }

                evicted += 1;
                tier_counters.record_eviction_from_ssd();

                // Re-check utilization after each removal.
                let (used_now, _) = Self::compute_utilization(extent_mgrs);
                let util_now = used_now as f64 / capacity as f64;
                if util_now < config.low_watermark {
                    break;
                }
            }

            if let Some(log) = logger {
                let (used_after, _) = Self::compute_utilization(extent_mgrs);
                log.info(&format!(
                    "ssd-evictor: evicted {evicted} extents, utilization now {:.1}%",
                    used_after as f64 / capacity as f64 * 100.0,
                ));
            }
        }
    }

    pub(crate) fn compute_utilization(
        extent_mgrs: &[Arc<dyn IExtentManager + Send + Sync>],
    ) -> (u64, u64) {
        let mut total_used = 0u64;
        let mut total_cap = 0u64;
        for em in extent_mgrs {
            total_used += em.used_bytes();
            total_cap += em.capacity_bytes();
        }
        (total_used, total_cap)
    }

    pub(crate) fn get_evictable_offset(
        dm: &Arc<dyn IDispatchMap + Send + Sync>,
        key: CacheKey,
    ) -> Option<u64> {
        match dm.lookup(key) {
            Ok(LookupResult::BlockDevice { offset }) => {
                let _ = dm.release_read(key);
                Some(offset)
            }
            Ok(LookupResult::MemoryTier { .. }) => {
                // Entry is still in memory-tier; skip — it's hot.
                let _ = dm.release_read(key);
                None
            }
            Ok(_) => {
                let _ = dm.release_read(key);
                None
            }
            Err(_) => None,
        }
    }
}

impl Drop for BackgroundEvictor {
    fn drop(&mut self) {
        if self.handle.is_some() {
            self.shutdown();
        }
    }
}

// ---------------------------------------------------------------------------
// Background Memory-Tier Evictor (DRAM → SSD demotion)
// ---------------------------------------------------------------------------

pub struct MemoryTierEvictorConfig {
    pub threshold: f64,
    pub low_watermark: f64,
    pub batch_size: usize,
    pub interval: Duration,
}

pub struct MemoryTierEvictor {
    shutdown: Arc<AtomicBool>,
    handle: Option<JoinHandle<()>>,
}

impl MemoryTierEvictor {
    pub fn start(
        dm: Arc<dyn IDispatchMap + Send + Sync>,
        mt: Arc<dyn IMemoryTier + Send + Sync>,
        config: MemoryTierEvictorConfig,
        logger: Option<Arc<dyn ILogger + Send + Sync>>,
        eviction_tx: Arc<Mutex<Option<Sender<EvictionEvent>>>>,
        tier_counters: Arc<TierEventCounters>,
    ) -> Self {
        let shutdown = Arc::new(AtomicBool::new(false));
        let shutdown_clone = Arc::clone(&shutdown);

        let handle = thread::Builder::new()
            .name("dispatcher-mt-evictor".into())
            .spawn(move || {
                Self::evictor_loop(
                    &shutdown_clone,
                    &dm,
                    &mt,
                    &config,
                    logger.as_deref(),
                    &eviction_tx,
                    &tier_counters,
                );
            })
            .expect("failed to spawn memory-tier evictor thread");

        Self {
            shutdown,
            handle: Some(handle),
        }
    }

    pub fn shutdown(&mut self) {
        self.shutdown.store(true, Ordering::Release);
        if let Some(handle) = self.handle.take() {
            let _ = handle.join();
        }
    }

    fn evictor_loop(
        shutdown: &AtomicBool,
        dm: &Arc<dyn IDispatchMap + Send + Sync>,
        mt: &Arc<dyn IMemoryTier + Send + Sync>,
        config: &MemoryTierEvictorConfig,
        logger: Option<&(dyn ILogger + Send + Sync)>,
        eviction_tx: &Mutex<Option<Sender<EvictionEvent>>>,
        tier_counters: &TierEventCounters,
    ) {
        let mut consecutive_dry_runs = 0u32;

        loop {
            if shutdown.load(Ordering::Acquire) {
                return;
            }

            let used = mt.used();
            let capacity = mt.capacity();
            if capacity == 0 {
                thread::sleep(config.interval);
                continue;
            }

            let utilization = used as f64 / capacity as f64;
            if utilization < config.threshold {
                consecutive_dry_runs = 0;
                thread::sleep(config.interval);
                continue;
            }

            // Exponential aggressiveness: scale batch size as utilization approaches 1.0.
            // At threshold → 1× batch_size; at 1.0 → 8× batch_size.
            let headroom = 1.0 - config.threshold;
            let pressure = if headroom > 0.0 {
                ((utilization - config.threshold) / headroom).min(1.0)
            } else {
                1.0
            };
            let multiplier = 1.0 + 7.0 * pressure * pressure;
            let effective_batch = ((config.batch_size as f64 * multiplier) as usize).max(1);

            if let Some(log) = logger {
                log.info(&format!(
                    "mt-evictor: utilization {:.1}% exceeds threshold {:.1}%, \
                     demoting (batch={effective_batch}, pressure={pressure:.2})",
                    utilization * 100.0,
                    config.threshold * 100.0,
                ));
            }

            // Widen the scan when most candidates are unevictable (held by write-through).
            let scan_size = effective_batch * (consecutive_dry_runs as usize + 1).min(4);
            let candidates = mt.oldest_keys(scan_size);
            let mut demoted = 0u32;

            for key in candidates {
                if shutdown.load(Ordering::Acquire) {
                    return;
                }

                if dm.try_evict_to_block(key).is_err() {
                    continue;
                }

                if mt.remove(key).is_err() {
                    continue;
                }

                if let Some(ref tx) = *eviction_tx.lock().unwrap() {
                    let _ = tx.try_send(EvictionEvent {
                        key,
                        reason: EvictionReason::Demoted,
                    });
                }

                demoted += 1;
                tier_counters.record_eviction_from_memory();

                let used_now = mt.used();
                let util_now = used_now as f64 / capacity as f64;
                if util_now < config.low_watermark {
                    break;
                }
            }

            if let Some(log) = logger {
                let used_after = mt.used();
                log.info(&format!(
                    "mt-evictor: demoted {demoted} entries, utilization now {:.1}%",
                    used_after as f64 / capacity as f64 * 100.0,
                ));
            }

            if demoted == 0 {
                // No evictable entries — back off to let write-through release refs.
                consecutive_dry_runs = consecutive_dry_runs.saturating_add(1);
                thread::sleep(Duration::from_millis(500));
            } else {
                consecutive_dry_runs = 0;
                // Pace between successful batches so we don't starve other work.
                thread::sleep(Duration::from_millis(200));
            }
        }
    }
}

impl Drop for MemoryTierEvictor {
    fn drop(&mut self) {
        if self.handle.is_some() {
            self.shutdown();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    #[test]
    fn start_and_shutdown() {
        let mut writer = BackgroundWriter::start(|_job| {});
        writer.shutdown();
    }

    #[test]
    fn processes_enqueued_jobs() {
        let processed = Arc::new(Mutex::new(Vec::new()));
        let processed_clone = Arc::clone(&processed);

        let mut writer = BackgroundWriter::start(move |job| {
            processed_clone.lock().unwrap().push(job.key);
        });

        writer
            .enqueue(WriteJob {
                key: 1,
                size: 4096,
                device_index: 0,
                stripe: None,
            })
            .unwrap();
        writer
            .enqueue(WriteJob {
                key: 2,
                size: 8192,
                device_index: 1,
                stripe: None,
            })
            .unwrap();

        writer.shutdown();

        let keys = processed.lock().unwrap();
        assert!(keys.contains(&1));
        assert!(keys.contains(&2));
    }

    #[test]
    fn drain_on_shutdown() {
        let count = Arc::new(Mutex::new(0u32));
        let count_clone = Arc::clone(&count);

        let mut writer = BackgroundWriter::start(move |_job| {
            *count_clone.lock().unwrap() += 1;
        });

        for i in 0..10 {
            writer
                .enqueue(WriteJob {
                    key: i,
                    size: 4096,
                    device_index: 0,
                    stripe: None,
                })
                .unwrap();
        }

        writer.shutdown();
        assert_eq!(*count.lock().unwrap(), 10);
    }

    #[test]
    fn concurrent_enqueue_from_multiple_threads() {
        let processed = Arc::new(Mutex::new(Vec::new()));
        let processed_clone = Arc::clone(&processed);

        let mut writer = BackgroundWriter::start(move |job| {
            processed_clone.lock().unwrap().push(job.key);
        });

        let sender = writer.sender.clone();
        let handles: Vec<_> = (0..4)
            .map(|t| {
                let s = sender.clone();
                thread::spawn(move || {
                    for i in 0..25 {
                        s.send(WriteJob {
                            key: t * 100 + i,
                            size: 4096,
                            device_index: 0,
                            stripe: None,
                        })
                        .unwrap();
                    }
                })
            })
            .collect();

        for h in handles {
            h.join().unwrap();
        }

        writer.shutdown();

        let keys = processed.lock().unwrap();
        assert_eq!(keys.len(), 100);
    }

    #[test]
    fn concurrent_enqueue_during_slow_processing() {
        let count = Arc::new(Mutex::new(0u32));
        let count_clone = Arc::clone(&count);

        let mut writer = BackgroundWriter::start(move |_job| {
            std::thread::sleep(std::time::Duration::from_millis(1));
            *count_clone.lock().unwrap() += 1;
        });

        for i in 0..20 {
            writer
                .enqueue(WriteJob {
                    key: i,
                    size: 4096,
                    device_index: 0,
                    stripe: None,
                })
                .unwrap();
        }

        writer.shutdown();
        assert_eq!(*count.lock().unwrap(), 20);
    }

    #[test]
    fn drop_triggers_shutdown() {
        let flag = Arc::new(AtomicBool::new(false));
        let flag_clone = Arc::clone(&flag);

        {
            let writer = BackgroundWriter::start(move |_job| {
                flag_clone.store(true, Ordering::Release);
            });
            writer
                .enqueue(WriteJob {
                    key: 1,
                    size: 4096,
                    device_index: 0,
                    stripe: None,
                })
                .unwrap();
        } // drop here

        assert!(flag.load(Ordering::Acquire));
    }
}
