//! Flush logic: serialize in-memory state to disk using the dual-region layout.
//!
//! Provides both a direct `flush_to_disk` function and a `FlushManager` that
//! runs a background thread with configurable timer + dirty-count triggers.

use crate::block_io::BlockDeviceClient;
use crate::on_disk::{self, Superblock};

use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Condvar, Mutex};
use std::thread;
use std::time::Duration;

/// Flush the current entries to disk using the dual-region ping-pong strategy.
///
/// 1. Serialize all entries to the INACTIVE region
/// 2. Write region data to disk
/// 3. Update superblock to point to the newly-written region
/// 4. Write superblock (atomic commit point)
pub fn flush_to_disk(
    client: &BlockDeviceClient,
    superblock: &mut Superblock,
    entries: &[(String, Vec<u8>)],
) -> Result<(), String> {
    let sector_size = client.sector_size as usize;
    let new_seq = superblock.flush_seq + 1;

    // Serialize all entries into the inactive region
    let region_data = on_disk::serialize_region(entries, new_seq, sector_size);
    let region_sectors = on_disk::bytes_to_sectors(region_data.len(), sector_size);

    // Check capacity
    let max_region_sectors = superblock.region_a_size;
    if region_sectors > max_region_sectors {
        return Err(format!(
            "region data ({region_sectors} sectors) exceeds region capacity ({max_region_sectors} sectors)"
        ));
    }

    // Write to the inactive region
    let inactive_offset = superblock.inactive_region_offset();
    client.write_region(inactive_offset, &region_data)?;

    // Update superblock: flip active region, bump sequence
    superblock.active_region = if superblock.active_region == 0 { 1 } else { 0 };
    superblock.flush_seq = new_seq;
    superblock.entry_count = entries.len() as u64;

    // Write superblock (the atomic commit point)
    client.write_superblock(superblock)?;

    Ok(())
}

/// Configuration for the background flush thread.
#[derive(Debug, Clone)]
pub struct FlushConfig {
    /// Interval between periodic flush attempts.
    pub interval: Duration,
    /// Number of mutations that trigger an immediate flush.
    pub dirty_threshold: u64,
}

impl Default for FlushConfig {
    fn default() -> Self {
        Self {
            interval: Duration::from_secs(5),
            dirty_threshold: 100,
        }
    }
}

/// Manages a background flush thread with coalescing.
///
/// Multiple concurrent `trigger_flush` calls share a single in-flight flush.
pub struct FlushManager {
    shutdown: Arc<AtomicBool>,
    notify: Arc<(Mutex<bool>, Condvar)>,
    completed_seq: Arc<AtomicU64>,
    flush_in_progress: Arc<(Mutex<bool>, Condvar)>,
    worker: Option<thread::JoinHandle<()>>,
}

/// Shared state passed to the flush worker thread.
struct FlushWorkerContext {
    shutdown: Arc<AtomicBool>,
    notify: Arc<(Mutex<bool>, Condvar)>,
    completed_seq: Arc<AtomicU64>,
    flush_in_progress: Arc<(Mutex<bool>, Condvar)>,
    config: FlushConfig,
    // Callbacks to interact with the component
    snapshot_fn: Box<dyn Fn() -> Vec<(String, Vec<u8>)> + Send>,
    dirty_count_fn: Box<dyn Fn() -> u64 + Send>,
    reset_dirty_fn: Box<dyn Fn(u64) + Send>,
    client: BlockDeviceClient,
    superblock: Arc<Mutex<Superblock>>,
}

impl FlushManager {
    /// Start a background flush thread.
    pub fn start(
        config: FlushConfig,
        client: BlockDeviceClient,
        superblock: Superblock,
        snapshot_fn: Box<dyn Fn() -> Vec<(String, Vec<u8>)> + Send>,
        dirty_count_fn: Box<dyn Fn() -> u64 + Send>,
        reset_dirty_fn: Box<dyn Fn(u64) + Send>,
    ) -> Self {
        let shutdown = Arc::new(AtomicBool::new(false));
        let notify = Arc::new((Mutex::new(false), Condvar::new()));
        let completed_seq = Arc::new(AtomicU64::new(superblock.flush_seq));
        let flush_in_progress = Arc::new((Mutex::new(false), Condvar::new()));

        let ctx = FlushWorkerContext {
            shutdown: shutdown.clone(),
            notify: notify.clone(),
            completed_seq: completed_seq.clone(),
            flush_in_progress: flush_in_progress.clone(),
            config,
            snapshot_fn,
            dirty_count_fn,
            reset_dirty_fn,
            client,
            superblock: Arc::new(Mutex::new(superblock)),
        };

        let worker = thread::spawn(move || {
            Self::worker_loop(ctx);
        });

        Self {
            shutdown,
            notify,
            completed_seq,
            flush_in_progress,
            worker: Some(worker),
        }
    }

    /// Trigger an immediate flush and block until it completes.
    /// Returns immediately if there are no dirty entries (already durable).
    pub fn trigger_flush(&self) -> Result<(), String> {
        // Mark that a flush is requested (before signaling worker)
        {
            let (lock, _) = &*self.flush_in_progress;
            let mut in_progress = lock.lock().unwrap();
            *in_progress = true;
        }

        // Signal the worker to wake and flush
        {
            let (lock, cvar) = &*self.notify;
            let mut signaled = lock.lock().unwrap();
            *signaled = true;
            cvar.notify_one();
        }

        // Wait until the worker marks the flush complete
        let (lock, cvar) = &*self.flush_in_progress;
        let _guard = cvar
            .wait_while(lock.lock().unwrap(), |in_progress| *in_progress)
            .unwrap();

        Ok(())
    }

    /// Get the last completed flush sequence number.
    pub fn completed_seq(&self) -> u64 {
        self.completed_seq.load(Ordering::Acquire)
    }

    fn worker_loop(ctx: FlushWorkerContext) {
        loop {
            // Wait for signal or timeout
            let (lock, cvar) = &*ctx.notify;
            let mut signaled = lock.lock().unwrap();
            if !*signaled {
                let result = cvar
                    .wait_timeout(signaled, ctx.config.interval)
                    .unwrap();
                signaled = result.0;
            }
            let was_signaled = *signaled;
            *signaled = false;
            drop(signaled);

            if ctx.shutdown.load(Ordering::Acquire) {
                // Final flush before exit
                Self::do_flush(&ctx);
                return;
            }

            // Check if flush is needed
            let dirty = (ctx.dirty_count_fn)();
            if dirty == 0 {
                // If explicitly signaled (force_flush) but nothing dirty, just mark done
                if was_signaled {
                    let (lock, cvar) = &*ctx.flush_in_progress;
                    let mut in_progress = lock.lock().unwrap();
                    *in_progress = false;
                    cvar.notify_all();
                }
                continue;
            }

            Self::do_flush(&ctx);
        }
    }

    fn do_flush(ctx: &FlushWorkerContext) {
        // Mark in-progress
        {
            let (lock, _cvar) = &*ctx.flush_in_progress;
            let mut in_progress = lock.lock().unwrap();
            *in_progress = true;
        }

        let entries = (ctx.snapshot_fn)();
        let mut sb = ctx.superblock.lock().unwrap();
        let result = flush_to_disk(&ctx.client, &mut sb, &entries);
        let new_seq = sb.flush_seq;
        drop(sb);

        if result.is_ok() {
            (ctx.reset_dirty_fn)(new_seq);
            ctx.completed_seq.store(new_seq, Ordering::Release);
        }

        // Mark complete and notify waiters
        {
            let (lock, cvar) = &*ctx.flush_in_progress;
            let mut in_progress = lock.lock().unwrap();
            *in_progress = false;
            cvar.notify_all();
        }
    }
}

impl Drop for FlushManager {
    fn drop(&mut self) {
        self.shutdown.store(true, Ordering::Release);
        // Wake the worker so it can exit
        let (lock, cvar) = &*self.notify;
        let mut signaled = lock.lock().unwrap();
        *signaled = true;
        cvar.notify_one();
        drop(signaled);

        if let Some(handle) = self.worker.take() {
            let _ = handle.join();
        }
    }
}
