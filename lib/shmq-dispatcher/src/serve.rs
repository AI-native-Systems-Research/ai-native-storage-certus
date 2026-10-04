//! The shared serve loop: poller thread + blocking worker pool + reservation
//! reaper. Lifted verbatim (bar the generic log prefix) from the former
//! `certus-shmq-server` binary so both server front-ends share one
//! implementation. See the crate-level docs for the concurrency model.

use std::io;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::thread;
use std::time::Duration;

use interfaces::ILogger;

use crate::translate::Translator;
use crate::wire;

/// Tunables for [`serve`]. Populated from the server binary's CLI flags.
pub struct ServeConfig {
    /// Number of mailbox channels; also the worker-pool size.
    pub channels: usize,
    /// Reclaim reservations left uncommitted/unaborted for longer than this.
    pub reserve_timeout: Duration,
    /// Optional CPU core to pin the busy-poll thread to.
    pub poller_cpu: Option<usize>,
    /// Emit periodic poller fairness/backlog stats (per-channel serviced counts
    /// and worker-queue depth). Off by default; zero overhead when disabled.
    pub poller_stats: bool,
}

/// Pin the current thread to `cpu`. Best-effort; errors surface to the caller.
fn pin_current_thread(cpu: usize) -> io::Result<()> {
    // SAFETY: cpu_set_t is a plain bitset; sched_setaffinity(0, ...) targets the
    // calling thread. All arguments are sized correctly.
    unsafe {
        let mut set: libc::cpu_set_t = std::mem::zeroed();
        libc::CPU_ZERO(&mut set);
        libc::CPU_SET(cpu, &mut set);
        if libc::sched_setaffinity(0, std::mem::size_of::<libc::cpu_set_t>(), &set) != 0 {
            return Err(io::Error::last_os_error());
        }
    }
    Ok(())
}

/// Format a compact poller fairness/backlog summary line. Reports total
/// requests forwarded, the lower-half vs upper-half channel split (the
/// data-parallel fairness signal — replica 0 owns the low channels, replica 1
/// the high channels), per-channel min/max, current worker-queue depth, and the
/// running high-water mark. A balanced low/high split with a near-zero queue
/// means the poller is fair and workers keep up; a large queue means workers
/// (shared server-side state) are the bottleneck.
fn log_poller_stats(prefix: &str, serviced: &[u64], qdepth: usize, max_qdepth: usize) -> String {
    let n = serviced.len();
    let total: u64 = serviced.iter().sum();
    let half = n / 2;
    let low: u64 = serviced[..half].iter().sum();
    let high: u64 = serviced[half..].iter().sum();
    let min = serviced.iter().copied().min().unwrap_or(0);
    let max = serviced.iter().copied().max().unwrap_or(0);
    format!(
        "{prefix}: serviced total={total} low[0,{half})={low} high[{half},{n})={high} \
         per-ch min={min} max={max} qdepth={qdepth} qdepth_max={max_qdepth}"
    )
}

/// Serve the shmq control plane until `shutdown` is set, then tear down in
/// order (poller → workers → reaper) and return.
///
/// `shutdown` is a `'static` atomic so the SIGINT/SIGTERM handler installed by
/// the binary can flip it; `serve` only reads it.
pub fn serve(
    server: Arc<shm_queue::Server>,
    translator: Translator,
    config: ServeConfig,
    shutdown: &'static AtomicBool,
    logger: Arc<dyn ILogger + Send + Sync>,
) -> io::Result<()> {
    // Worker pool: one worker per channel, blocking on the request queue.
    let (tx, rx) = crossbeam_channel::unbounded::<shm_queue::PolledRequest>();
    let mut workers = Vec::with_capacity(config.channels);
    for w in 0..config.channels {
        let rx = rx.clone();
        let server = Arc::clone(&server);
        let tr = translator.clone();
        workers.push(
            thread::Builder::new()
                .name(format!("shmq-worker-{w}"))
                .spawn(move || {
                    while let Ok(req) = rx.recv() {
                        match tr.dispatch(req.opcode, &req.payload) {
                            Ok(blob) => server.reply(req.channel, req.seq, wire::STATUS_OK, &blob),
                            Err(e) => {
                                let msg = e.to_string();
                                server.reply(
                                    req.channel,
                                    req.seq,
                                    wire::STATUS_ERROR,
                                    msg.as_bytes(),
                                );
                            }
                        }
                    }
                })
                .expect("spawn worker"),
        );
    }
    drop(rx); // only workers hold receivers now

    // Reservation-timeout reaper: reclaim Reserve-without-Commit leaks.
    let reserve_timeout = config.reserve_timeout;
    let reaper = {
        let tr = translator.clone();
        let logger = Arc::clone(&logger);
        thread::Builder::new()
            .name("shmq-reaper".into())
            .spawn(move || {
                // Poll shutdown every 500ms; sweep for stale reservations ~5s.
                let tick = Duration::from_millis(500);
                let mut since_sweep = Duration::ZERO;
                while !shutdown.load(Ordering::Relaxed) {
                    thread::sleep(tick);
                    since_sweep += tick;
                    if since_sweep >= Duration::from_secs(5) {
                        since_sweep = Duration::ZERO;
                        let n = tr.reap_stale_reservations(reserve_timeout);
                        if n > 0 {
                            logger.warn(&format!(
                                "shmq: reclaimed {n} stale reservation(s) \
                                 (uncommitted > {}s)",
                                reserve_timeout.as_secs()
                            ));
                        }
                    }
                }
            })
            .expect("spawn reaper")
    };

    // Poller thread: busy-scan every channel, hand ready requests to workers.
    let poller = {
        let server = Arc::clone(&server);
        let logger = Arc::clone(&logger);
        let poller_cpu = config.poller_cpu;
        let poller_stats = config.poller_stats;
        let stats_tx = tx.clone(); // for backlog sampling (tx.len()); does not extend worker life
        thread::Builder::new()
            .name("shmq-poller".into())
            .spawn(move || {
                if let Some(cpu) = poller_cpu {
                    match pin_current_thread(cpu) {
                        Ok(()) => logger.info(&format!("shmq: poller pinned to CPU {cpu}")),
                        Err(e) => {
                            logger.warn(&format!("shmq: poller pin to CPU {cpu} failed: {e}"))
                        }
                    }
                }
                let mut last_seen = server.seq_baseline();
                let n = last_seen.len();
                // Rotating first-pick offset: the channel serviced first advances
                // every sweep so no channel slice is permanently ahead in the FIFO
                // worker queue. Fixes systematic starvation of higher-numbered
                // channels (e.g. the second data-parallel replica's slice) under
                // worker backlog.
                let mut start = 0usize;
                let mut sweeps: u64 = 0;
                // Optional per-channel servicing counters + worker-queue high-water
                // mark. Single-threaded poller, so plain counters (no atomics).
                let mut serviced: Vec<u64> = if poller_stats { vec![0; n] } else { Vec::new() };
                let mut max_qdepth: usize = 0;
                // Throttle the stats line to wall-clock cadence: 4096 idle sweeps
                // elapse in microseconds, so gate on time, not sweep count.
                let mut last_stat_log = std::time::Instant::now();
                while !shutdown.load(Ordering::Relaxed) {
                    let mut idle = true;
                    for i in 0..n {
                        let ch = {
                            let c = start + i;
                            if c >= n {
                                c - n
                            } else {
                                c
                            }
                        };
                        if let Some(req) = server.take_request(ch, &mut last_seen[ch]) {
                            idle = false;
                            if poller_stats {
                                serviced[ch] += 1;
                            }
                            if tx.send(req).is_err() {
                                return; // workers gone
                            }
                        }
                    }
                    start += 1;
                    if start >= n {
                        start = 0;
                    }
                    sweeps = sweeps.wrapping_add(1);
                    if sweeps % 4096 == 0 {
                        server.heartbeat();
                        if poller_stats {
                            let q = stats_tx.len();
                            if q > max_qdepth {
                                max_qdepth = q;
                            }
                            if last_stat_log.elapsed() >= Duration::from_secs(5) {
                                logger.info(&log_poller_stats(
                                    "shmq poller",
                                    &serviced,
                                    q,
                                    max_qdepth,
                                ));
                                last_stat_log = std::time::Instant::now();
                            }
                        }
                    }
                    if idle {
                        std::hint::spin_loop();
                    }
                }
                if poller_stats {
                    logger.info(&log_poller_stats(
                        "shmq poller (final)",
                        &serviced,
                        stats_tx.len(),
                        max_qdepth,
                    ));
                }
            })
            .expect("spawn poller")
    };

    logger.info("shmq: serving (Ctrl-C to stop)");

    // Wait for shutdown, then tear down in order: poller stops sending, its `tx`
    // drops, workers drain and exit on the closed channel, reaper wakes and exits.
    poller.join().expect("join poller");
    for w in workers {
        let _ = w.join();
    }
    let _ = reaper.join();

    Ok(())
}
