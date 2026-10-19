//! Flush logic: serialize in-memory state to disk using the dual-region layout.
//!
//! Provides both a direct `flush_to_disk` function and a `FlushManager` that
//! runs a background thread with configurable timer + dirty-count triggers.

use crate::block_io::BlockDeviceClient;
use crate::on_disk::{self, Superblock};

use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Condvar, Mutex};
use std::thread;
use std::time::{Duration, Instant};

/// Longest sleep between dirty-count checks while waiting for the timer:
/// `put`/`delete` do not wake the worker, so it polls for the threshold.
const THRESHOLD_POLL_INTERVAL: Duration = Duration::from_millis(10);

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
    flush_state: Arc<(Mutex<FlushTickets>, Condvar)>,
    worker: Option<thread::JoinHandle<()>>,
}

/// `trigger_flush` requests: each caller takes ticket `requested + 1`; the
/// worker sets `completed` to the last ticket taken before its snapshot, and
/// `durable` to the same if that pass left everything on disk.
#[derive(Debug, Default)]
struct FlushTickets {
    requested: u64,
    completed: u64,
    durable: u64,
    last_error: Option<String>,
}

/// Shared state passed to the flush worker thread.
struct FlushWorkerContext {
    shutdown: Arc<AtomicBool>,
    notify: Arc<(Mutex<bool>, Condvar)>,
    completed_seq: Arc<AtomicU64>,
    flush_state: Arc<(Mutex<FlushTickets>, Condvar)>,
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
        let flush_state = Arc::new((Mutex::new(FlushTickets::default()), Condvar::new()));

        let ctx = FlushWorkerContext {
            shutdown: shutdown.clone(),
            notify: notify.clone(),
            completed_seq: completed_seq.clone(),
            flush_state: flush_state.clone(),
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
            flush_state,
            worker: Some(worker),
        }
    }

    /// Trigger an immediate flush and block until it completes.
    /// Returns immediately if there are no dirty entries (already durable).
    /// Returns `Err` if the flush serving this call failed (data not durable).
    pub fn trigger_flush(&self) -> Result<(), String> {
        // Take a ticket (before signaling worker)
        let ticket = {
            let (lock, _) = &*self.flush_state;
            let mut tickets = lock.lock().unwrap();
            tickets.requested += 1;
            tickets.requested
        };

        // Signal the worker to wake and flush
        {
            let (lock, cvar) = &*self.notify;
            let mut signaled = lock.lock().unwrap();
            *signaled = true;
            cvar.notify_one();
        }

        // Wait until a flush whose snapshot was taken after our ticket completes
        let (lock, cvar) = &*self.flush_state;
        let tickets = cvar
            .wait_while(lock.lock().unwrap(), |tickets| tickets.completed < ticket)
            .unwrap();

        if tickets.durable >= ticket {
            Ok(())
        } else {
            Err(tickets.last_error.clone().unwrap_or_default())
        }
    }

    /// Get the last completed flush sequence number.
    pub fn completed_seq(&self) -> u64 {
        self.completed_seq.load(Ordering::Acquire)
    }

    fn worker_loop(ctx: FlushWorkerContext) {
        // A threshold of 0 would wake the worker with nothing dirty.
        let threshold = ctx.config.dirty_threshold.max(1);
        // Cleared by a failed flush, so a failing device is retried on the
        // timer rather than in a hot loop; set again by a successful flush.
        let mut threshold_armed = true;
        loop {
            // Wait for signal, timeout, or the dirty count reaching the threshold
            // (`None`: the interval is too long to represent, the timer never fires)
            let deadline = Instant::now().checked_add(ctx.config.interval);
            let (lock, cvar) = &*ctx.notify;
            let mut signaled = lock.lock().unwrap();
            while !*signaled && !(threshold_armed && (ctx.dirty_count_fn)() >= threshold) {
                let now = Instant::now();
                let left = deadline.map_or(Duration::MAX, |d| d.saturating_duration_since(now));
                if left.is_zero() {
                    break;
                }
                let slice = left.min(THRESHOLD_POLL_INTERVAL);
                let result = cvar.wait_timeout(signaled, slice).unwrap();
                signaled = result.0;
            }
            let was_signaled = *signaled;
            *signaled = false;
            drop(signaled);

            // Tickets taken so far are served by this pass: their writes
            // precede the dirty check and snapshot below.
            let serving = ctx.flush_state.0.lock().unwrap().requested;

            if ctx.shutdown.load(Ordering::Acquire) {
                // Final flush before exit
                Self::do_flush(&ctx, serving);
                return;
            }

            // Check if flush is needed
            let dirty = (ctx.dirty_count_fn)();
            if dirty == 0 {
                // If explicitly signaled (force_flush) but nothing dirty, just mark done
                if was_signaled {
                    let (lock, cvar) = &*ctx.flush_state;
                    let mut tickets = lock.lock().unwrap();
                    tickets.completed = tickets.completed.max(serving);
                    tickets.durable = tickets.durable.max(serving);
                    cvar.notify_all();
                }
                continue;
            }

            threshold_armed = Self::do_flush(&ctx, serving);
        }
    }

    /// Returns whether the flush succeeded.
    fn do_flush(ctx: &FlushWorkerContext, serving: u64) -> bool {
        let entries = (ctx.snapshot_fn)();
        let mut sb = ctx.superblock.lock().unwrap();
        let result = flush_to_disk(&ctx.client, &mut sb, &entries);
        let new_seq = sb.flush_seq;
        drop(sb);

        if result.is_ok() {
            (ctx.reset_dirty_fn)(new_seq);
            ctx.completed_seq.store(new_seq, Ordering::Release);
        }

        // Mark the served tickets complete and notify waiters
        {
            let (lock, cvar) = &*ctx.flush_state;
            let mut tickets = lock.lock().unwrap();
            tickets.completed = tickets.completed.max(serving);
            match &result {
                Ok(()) => tickets.durable = tickets.durable.max(serving),
                Err(e) => tickets.last_error = Some(e.clone()),
            }
            cvar.notify_all();
        }

        result.is_ok()
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

#[cfg(test)]
#[path = "repair_test_flush_lost_dirty.rs"]
mod repair_test_flush_lost_dirty;

#[cfg(test)]
#[path = "repair_test_flush_threshold.rs"]
mod repair_test_flush_threshold;

#[cfg(test)]
#[path = "repair_test_force_flush_error.rs"]
mod repair_test_force_flush_error;
