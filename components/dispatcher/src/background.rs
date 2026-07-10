//! Background write worker for memory-tier-to-SSD persistence and SSD eviction.

use crossbeam_channel::{Receiver, Sender};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::thread::{self, JoinHandle};
use std::time::Duration;

use interfaces::{CacheKey, IDispatchMap, IExtentManager, ILogger, IMemoryTier, LookupResult};

use crate::{EvictionEvent, EvictionReason};

/// A job for the background writer to persist a memory-tier entry to SSD.
#[derive(Debug)]
pub struct WriteJob {
    /// Cache key identifying the entry.
    pub key: u64,
    /// Size of the data in bytes.
    pub size: u32,
    /// Index of the data block device to write to.
    pub device_index: usize,
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
/// Routes each `WriteJob` to the writer responsible for its target drive,
/// enabling concurrent write-through across multiple NVMe devices.
pub struct ParallelBackgroundWriter {
    writers: Vec<BackgroundWriter>,
    num_drives: usize,
}

impl ParallelBackgroundWriter {
    /// Start one writer thread per drive.
    ///
    /// `make_processor(drive_idx)` is called once per drive to produce the
    /// job-processing closure for that drive's dedicated thread.
    pub fn start<F>(num_drives: usize, make_processor: impl Fn(usize) -> F) -> Self
    where
        F: FnMut(WriteJob) + Send + 'static,
    {
        let writers = (0..num_drives)
            .map(|idx| BackgroundWriter::start_named(idx, make_processor(idx)))
            .collect();

        Self {
            writers,
            num_drives,
        }
    }

    /// Enqueue a write job, routing to the writer for its target drive.
    pub fn enqueue(&self, job: WriteJob) -> Result<(), WriteJob> {
        let idx = job.device_index % self.num_drives;
        self.writers[idx].enqueue(job)
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

                // Free extent on the appropriate drive.
                let drive_idx = key as usize % extent_mgrs.len().max(1);
                if let Some(em) = extent_mgrs.get(drive_idx) {
                    let _ = em.remove_extent(offset);
                }

                evicted += 1;

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
            })
            .unwrap();
        writer
            .enqueue(WriteJob {
                key: 2,
                size: 8192,
                device_index: 1,
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
                })
                .unwrap();
        } // drop here

        assert!(flag.load(Ordering::Acquire));
    }
}
