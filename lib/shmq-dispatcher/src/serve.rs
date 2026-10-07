//! The shared serve loop: poller thread + blocking worker pool + reservation
//! reaper. Lifted verbatim (bar the generic log prefix) from the former
//! `certus-shmq-server` binary so both server front-ends share one
//! implementation. See the crate-level docs for the concurrency model.

use std::io;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
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
/// Where a request got to, counted at each hand-off in the serve path.
///
/// **Diagnostic, added to separate two indistinguishable failure shapes.** Under
/// `dispatcher-p2p` the server reaches a state where every worker is idle on an empty
/// queue while the client still believes it has requests outstanding — so a request or a
/// reply is being lost, and the backtraces cannot say which. These four counters can:
///
/// - `taken > enqueued`  — the poller read a request off the mailbox and failed to hand it
///   to a worker (the only path is a closed channel, which returns early).
/// - `enqueued > dequeued` — requests are sitting in the queue with no worker taking them,
///   which contradicts an idle worker pool and would mean a lost wakeup.
/// - `dequeued > replied` — a worker took a request and never wrote a reply: the dispatch
///   call did not return.
/// - all four equal, client still waiting — the reply was written to shared memory but the
///   client never saw it, moving the fault to the mailbox or the agent.
///
/// Relaxed ordering throughout: these are monotonic observability counters read by a
/// reporter thread, and no reader depends on seeing them mutually consistent.
#[derive(Debug, Default)]
pub struct ServeCounters {
    /// Requests the poller took off a mailbox channel.
    pub taken: AtomicU64,
    /// Requests handed to the worker queue.
    pub enqueued: AtomicU64,
    /// Requests a worker took off the queue.
    pub dequeued: AtomicU64,
    /// Replies written back to shared memory.
    pub replied: AtomicU64,
    /// Worker panics caught. **Nonzero means the process is aborting**: a panic is an
    /// invariant break, and this server is in the data path.
    pub worker_panics: AtomicU64,
}

/// Best-effort text from a caught panic payload.
///
/// `panic!` with a literal yields `&str` and with formatting yields `String`; anything
/// else is possible but vanishingly rare, so it is named rather than guessed at.
fn panic_text(payload: &Box<dyn std::any::Any + Send>) -> String {
    if let Some(s) = payload.downcast_ref::<&str>() {
        (*s).to_string()
    } else if let Some(s) = payload.downcast_ref::<String>() {
        s.clone()
    } else {
        "non-string panic payload".to_string()
    }
}

impl ServeCounters {
    fn line(&self) -> String {
        let (t, e, d, r) = (
            self.taken.load(Ordering::Relaxed),
            self.enqueued.load(Ordering::Relaxed),
            self.dequeued.load(Ordering::Relaxed),
            self.replied.load(Ordering::Relaxed),
        );
        // The gaps are the point, so they are computed here rather than left to a reader
        // subtracting four numbers under time pressure during a stall.
        format!(
            "shmq-flow taken {t} enqueued {e} dequeued {d} replied {r} \
             gaps[take->enq {}, enq->deq {}, deq->reply {}] panics {}",
            t - e,
            e - d,
            d - r,
            self.worker_panics.load(Ordering::Relaxed)
        )
    }
}

pub fn serve(
    server: Arc<shm_queue::Server>,
    translator: Translator,
    config: ServeConfig,
    shutdown: &'static AtomicBool,
    logger: Arc<dyn ILogger + Send + Sync>,
) -> io::Result<()> {
    let flow = Arc::new(ServeCounters::default());
    // A reporter for the flow counters, at the same 2 s cadence as the server's
    // tier-events line so the two can be read against each other during a stall. Cheap
    // enough to leave on: four relaxed loads and one log line.
    {
        let flow_r = Arc::clone(&flow);
        let log_r = Arc::clone(&logger);
        thread::Builder::new()
            .name("shmq-flow".into())
            .spawn(move || {
                while !shutdown.load(Ordering::Relaxed) {
                    thread::sleep(std::time::Duration::from_secs(2));
                    log_r.debug(&flow_r.line());
                }
            })
            .expect("spawn flow reporter");
    }

    // Worker pool: one worker per channel, blocking on the request queue.
    let (tx, rx) = crossbeam_channel::unbounded::<shm_queue::PolledRequest>();
    let mut workers = Vec::with_capacity(config.channels);
    for w in 0..config.channels {
        let rx = rx.clone();
        let server = Arc::clone(&server);
        let tr = translator.clone();
        let flow_w = Arc::clone(&flow);
        let log_w = Arc::clone(&logger);
        workers.push(
            thread::Builder::new()
                .name(format!("shmq-worker-{w}"))
                .spawn(move || {
                    while let Ok(req) = rx.recv() {
                        flow_w.dequeued.fetch_add(1, Ordering::Relaxed);
                        // `AssertUnwindSafe` because `tr` and `server` are captured by
                        // reference and neither is `UnwindSafe`. The assertion is doing
                        // less work here than it usually does: this handler does not
                        // resume on the caught panic -- it replies and aborts -- so no
                        // later code observes whatever state the panic left behind. The
                        // component framework makes the same assertion at its own message
                        // boundary (`component-core/src/actor.rs`) and does continue.
                        let outcome =
                            std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                                tr.dispatch(req.opcode, &req.payload)
                            }));
                        match outcome {
                            Ok(Ok(blob)) => {
                                server.reply(req.channel, req.seq, wire::STATUS_OK, &blob)
                            }
                            Ok(Err(e)) => {
                                let msg = e.to_string();
                                server.reply(
                                    req.channel,
                                    req.seq,
                                    wire::STATUS_ERROR,
                                    msg.as_bytes(),
                                );
                            }
                            Err(payload) => {
                                // A panic used to unwind past both replies and end this
                                // loop, retiring the thread in silence. With one worker per
                                // channel the pool then eroded to nothing while the server
                                // kept answering /metrics and kept checkpointing: a black
                                // hole that still looked healthy. One observed instance cost
                                // 16 workers and left every client blocked forever.
                                let what = panic_text(&payload);

                                // Reply FIRST. Whatever happens to this process, the client
                                // that sent this request must not be left waiting on a reply
                                // that can never come -- that is the half of the defect that
                                // is unambiguously ours to fix.
                                server.reply(
                                    req.channel,
                                    req.seq,
                                    wire::STATUS_ERROR,
                                    format!("server panic: {what}").as_bytes(),
                                );
                                flow_w.worker_panics.fetch_add(1, Ordering::Relaxed);
                                log_w.error(&format!(
                                    "shmq-worker panic, aborting: {what} (opcode {}, channel \
                                     {})",
                                    req.opcode, req.channel
                                ));

                                // Then abort, deliberately, rather than carry on like the
                                // framework's actor does. This server sits in the data path:
                                // a panic mid-dispatch can leave a dispatch-map entry
                                // pointing at a half-written extent, and serving from that
                                // state risks returning wrong data. An outage is recoverable;
                                // silently wrong reads are not. Abort rather than `panic!`
                                // so no further unwinding runs destructors over the same
                                // broken state.
                                std::process::abort();
                            }
                        }
                        // After the reply, so `dequeued > replied` means dispatch did not
                        // return -- which is the distinction the gap exists to draw.
                        flow_w.replied.fetch_add(1, Ordering::Relaxed);
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
        let flow_p = Arc::clone(&flow);
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
                            flow_p.taken.fetch_add(1, Ordering::Relaxed);
                            if tx.send(req).is_err() {
                                return; // workers gone
                            }
                            flow_p.enqueued.fetch_add(1, Ordering::Relaxed);
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

#[cfg(test)]
mod tests {
    use super::*;

    /// The panic payload must become readable text, because it is what the client is told
    /// and what the operator sees in the log.
    ///
    /// Both forms are covered because `panic!("literal")` yields `&str` while
    /// `panic!("{x}")` yields `String`, and the `expect` that prompted this work
    /// (`dispatcher-p2p` requiring its P2P ring) is the `&str` form — so getting only the
    /// `String` case right would have produced "non-string panic payload" for the exact
    /// panic this exists to report.
    #[test]
    fn a_panic_payload_becomes_readable_text() {
        let from_str = std::panic::catch_unwind(|| panic!("ring unavailable"))
            .expect_err("the closure panics");
        assert_eq!(panic_text(&from_str), "ring unavailable");

        let n = 16;
        let from_string = std::panic::catch_unwind(|| panic!("{n} workers died"))
            .expect_err("the closure panics");
        assert_eq!(panic_text(&from_string), "16 workers died");

        let odd = std::panic::catch_unwind(|| std::panic::panic_any(7u8))
            .expect_err("the closure panics");
        assert_eq!(
            panic_text(&odd),
            "non-string panic payload",
            "an unexpected payload must still yield something a reader can act on"
        );
    }

    /// The flow line must surface the panic count, or the condition stays invisible in the
    /// one place an operator is already looking.
    #[test]
    fn the_flow_line_reports_panics() {
        let c = ServeCounters::default();
        c.taken.store(10, Ordering::Relaxed);
        c.enqueued.store(10, Ordering::Relaxed);
        c.dequeued.store(10, Ordering::Relaxed);
        c.replied.store(9, Ordering::Relaxed);
        c.worker_panics.store(1, Ordering::Relaxed);
        let line = c.line();
        assert!(
            line.contains("panics 1"),
            "panic count must be on the line: {line}"
        );
        assert!(
            line.contains("deq->reply 1"),
            "and the gap it explains must be there too: {line}"
        );
    }
}
