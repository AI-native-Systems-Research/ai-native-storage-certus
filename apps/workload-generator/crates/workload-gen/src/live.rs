//! The live path: a producer building the plan ahead of lanes that consume it.
//!
//! # The queue is the architecture, not instrumentation
//!
//! An earlier version of this module built the **entire** plan before issuing anything
//! and then computed `depth = total - index`. That was a serious deviation from the
//! design, and it broke two requirements at once in ways that looked fine:
//!
//! - **FR-062 could never fire.** A countdown from `total` to 1 makes the minimum depth
//!   always 1 and the fraction at zero always 0, so `is_valid()` was a constant `true`.
//!   Two live runs at different seeds and lane counts reported byte-identical queue
//!   statistics, which is the signature of a value read off the loop index rather than
//!   off the system.
//! - **FR-059's unbounded run could not exist.** Pre-building allocates with the span, so
//!   an unbounded run was silently substituted with a 60-second one — a different
//!   experiment, reported as though it were the requested one.
//!
//! Both come from the same missing piece. A bounded queue between a producer and the
//! lanes is simultaneously what bounds memory, so an unbounded run is possible, and what
//! makes an underrun observable, so validity means something.
//!
//! # Underrun, not starvation
//!
//! The condition is a **buffer underrun**: a bounded buffer, a consumer that goes to read
//! and finds it empty, a stalled pipeline. "Starvation" is the wrong word for it —
//! in scheduling that names a *fairness* failure, a thread perpetually denied a resource
//! other threads keep winning. Nothing unfair happens here: no lane is denied by another
//! lane, the producer simply did not supply in time. The constitution uses "starving a
//! recency policy of `Touch`" for the plain-English sense of depriving something of what
//! it needs, and keeping the queue condition's own name distinct avoids that collision.
//!
//! # The underrun is counted, never sampled
//!
//! Sampling depth as a gauge can miss a brief exhaustion between samples — the same
//! failure as reporting an average, one level down. So a consumer that finds its queue
//! empty **is** the underrun, and it is counted directly. The minimum depth is
//! recorded at pop time beside it, because a consumer's view of the queue is the one that
//! matters.
//!
//! ## The initial fill is excluded, and that is necessary rather than convenient
//!
//! At the instant a run starts, every queue is empty because nothing has been produced
//! yet, so a consumer's first look always finds it so. Counting that made the first
//! streaming implementation report **every** lane underrunning exactly once — `[1, 1, 1, 1]`
//! — and every run invalid, which is as useless as a check that never fires. One vacuous
//! metric traded for another.
//!
//! ## A blocked producer is the healthy state, and is counted too
//!
//! Underruns alone cannot distinguish "the generator stayed comfortably ahead" from
//! "everything was marginal and nothing quite ran dry". The queue filling — the producer
//! blocking on a full channel — is the positive evidence, so [`LiveStats::producer_blocked`]
//! counts it. It is backpressure working as intended, never an error: a bounded blocking
//! channel cannot overwrite unconsumed work, so there is no "overrun" to pair it with.
//! A run with zero underruns **and** a producer that blocked is one where the queue
//! demonstrably did its job.
//!
//! So a lane's first batch is **primed**: it blocks for it without counting, and only
//! afterwards does an empty queue mean the producer fell behind. Starvation is therefore
//! "this lane had work and then ran out", which is the condition FR-062 is about. It is
//! the same principle as FR-046 excluding the startup cache clear from the timed window —
//! a run properly begins once its pipeline is full.
//!
//! # Lanes are sharded by session, which FR-035 requires
//!
//! A session's operations must stay strictly ordered. With one shared queue and several
//! consumers, two turns of one session could be issued out of order by different lanes.
//! So each lane owns a queue and the producer routes a turn to `session % lanes` —
//! per-session order then holds by construction rather than by coordination.
//!
//! The cost is head-of-line blocking: a full queue on one lane blocks the producer for
//! that lane while others drain, so a consumer can log an underrun the routing caused
//! rather than the producer's speed. [`QUEUE_CAPACITY`] absorbs ordinary imbalance, and
//! the report gives **per-lane** figures so the pattern is visible rather than averaged
//! away.
//!
//! # What still needs a GPU
//!
//! `LOOKUP` and `COPY_TO_STORE` take a handle batch — `(key, IpcHandle)` pairs — because
//! a load DMAs into a GPU buffer and a store copies out of one. Until the payload buffer
//! lands (T038, T039) they are counted and skipped, and the report says the run was
//! partial rather than quietly reporting a throughput for a stream that moved no data.

use std::collections::BTreeMap;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::mpsc::{sync_channel, Receiver, SyncSender, TryRecvError, TrySendError};
use std::sync::Arc;
use std::thread;
use std::time::{Duration, Instant};

use hdrhistogram::Histogram;
use shm_queue::Client;
use workload_model::description::WorkloadDescription;
use workload_model::plan::{OpKind, OperationPlan};
use workload_model::sim::Simulation;
use workload_wire::frame::Counters;

use crate::exec::{clear_memory_tier, TurnExecutor};
use crate::payload::PayloadBuffer;

/// Turn batches a lane's queue holds before the producer blocks.
///
/// Each batch is one turn: its operations plus one copy of its prefix. Depth is therefore
/// in **turns**, and memory is bounded by `lanes * QUEUE_CAPACITY * prefix size` rather
/// than by the run's span — which is what makes an unbounded run possible at all.
///
/// 256 is deep enough to absorb the imbalance session sharding creates and shallow enough
/// that a long-session workload does not hold hundreds of megabytes.
pub const QUEUE_CAPACITY: usize = 256;

/// Virtual seconds the producer advances per step.
///
/// A window rather than the whole span: a bounded run must not build what it has not been
/// asked for, and an unbounded one has no whole span to build.
const PRODUCE_WINDOW: f64 = 5.0;

/// How long `attach` waits for the server to publish its ready flag.
const ATTACH_TIMEOUT: Duration = Duration::from_secs(10);

/// One lane's view of its own queue and its own traffic.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct LaneStats {
    /// Batches taken.
    pub pops: u64,
    /// Pops that found the queue **empty** — each one an underrun, meaning the
    /// generator was the constraint at that instant (FR-062).
    pub underruns: u64,
    /// Smallest depth seen at pop time.
    pub min_depth: usize,
    /// Operations skipped for want of a GPU payload buffer.
    pub skipped_needing_gpu: u64,
    /// Keys those skipped operations would have moved.
    pub skipped_keys: u64,
    /// What this lane's mailbox-facing code counted.
    ///
    /// Held rather than duplicated: [`crate::exec::TurnExecutor`] gathers these, and the node
    /// agent ships back the very same type, so a local figure and a remote figure are the
    /// same measurement rather than two that happen to be named alike.
    pub counters: Counters,
}

/// What a live run measured.
#[derive(Debug, Clone)]
pub struct LiveStats {
    /// Per-lane figures, so a routing imbalance is visible rather than averaged away.
    pub lanes: Vec<LaneStats>,
    /// Turn batches the producer built.
    pub batches_produced: u64,
    /// Whether the producer reached the end of its span rather than being stopped.
    pub producer_completed: bool,
    /// Times the producer blocked on a full queue.
    ///
    /// Backpressure working, and the positive counterpart to an underrun: it is the
    /// evidence that the generator was ahead rather than merely keeping up.
    pub producer_blocked: u64,
    /// Entries the startup memory-tier clear dropped, or `None` if no clear was asked for.
    pub cleared_entries: Option<u64>,
    /// Bytes per block, from the description's geometry.
    pub block_bytes: u64,
    /// Wallclock seconds of the timed window.
    pub elapsed: f64,
    /// Virtual seconds the plan advanced.
    pub virtual_span: f64,
    /// Per-request latency across every lane, every opcode together.
    pub latency: Histogram<u64>,
    /// Per-request latency **per opcode**.
    ///
    /// An aggregate mixes a `CHECK` — control only, microseconds — with a `LOOKUP` that
    /// DMAs a block per key, so its percentiles describe the *mix* of operations a
    /// description happens to produce rather than anything about Certus. Splitting them is
    /// what makes the number a statement about the system.
    pub latency_by_op: BTreeMap<u32, Histogram<u64>>,
}

impl LiveStats {
    /// Underruns across all lanes.
    pub fn underruns(&self) -> u64 {
        self.lanes.iter().map(|l| l.underruns).sum()
    }

    /// Pops across all lanes.
    pub fn pops(&self) -> u64 {
        self.lanes.iter().map(|l| l.pops).sum()
    }

    /// Smallest depth any lane saw at pop time.
    pub fn min_depth(&self) -> usize {
        self.lanes.iter().map(|l| l.min_depth).min().unwrap_or(0)
    }

    /// Fraction of pops that underran.
    pub fn fraction_underrun(&self) -> f64 {
        let pops = self.pops();
        if pops == 0 {
            0.0
        } else {
            self.underruns() as f64 / pops as f64
        }
    }

    /// Requests issued.
    pub fn requests(&self) -> u64 {
        self.lanes.iter().map(|l| l.counters.requests).sum()
    }

    /// Key references issued.
    pub fn key_references(&self) -> u64 {
        self.lanes.iter().map(|l| l.counters.key_references).sum()
    }

    /// Operations skipped for want of a GPU payload buffer.
    pub fn skipped_needing_gpu(&self) -> u64 {
        self.lanes.iter().map(|l| l.skipped_needing_gpu).sum()
    }

    /// Keys those skipped operations would have moved.
    pub fn skipped_keys(&self) -> u64 {
        self.lanes.iter().map(|l| l.skipped_keys).sum()
    }

    /// `CHECK` keys reported `RESIDENT`.
    pub fn check_resident(&self) -> u64 {
        self.lanes.iter().map(|l| l.counters.check_resident).sum()
    }

    /// `CHECK` keys reported `PENDING`.
    pub fn check_pending(&self) -> u64 {
        self.lanes.iter().map(|l| l.counters.check_pending).sum()
    }

    /// `CHECK` keys reported `MISS`.
    pub fn check_miss(&self) -> u64 {
        self.lanes.iter().map(|l| l.counters.check_miss).sum()
    }

    /// `LOOKUP` keys that returned data.
    pub fn lookup_hits(&self) -> u64 {
        self.lanes.iter().map(|l| l.counters.lookup_hits).sum()
    }

    /// `LOOKUP` keys that did not.
    pub fn lookup_misses(&self) -> u64 {
        self.lanes.iter().map(|l| l.counters.lookup_misses).sum()
    }

    /// Keys a `RESERVE` was asked for.
    pub fn reserves_attempted(&self) -> u64 {
        self.lanes
            .iter()
            .map(|l| l.counters.reserves_attempted)
            .sum()
    }

    /// Keys a `COMMIT_STORE` was asked for.
    pub fn commits_attempted(&self) -> u64 {
        self.lanes
            .iter()
            .map(|l| l.counters.commits_attempted)
            .sum()
    }

    /// Keys a `RESERVE` declined.
    pub fn reserves_declined(&self) -> u64 {
        self.lanes
            .iter()
            .map(|l| l.counters.reserves_declined)
            .sum()
    }

    /// Keys a `COPY_TO_STORE` declined.
    pub fn transfers_declined(&self) -> u64 {
        self.lanes
            .iter()
            .map(|l| l.counters.transfers_declined)
            .sum()
    }

    /// Keys a `COPY_TO_STORE` was asked for.
    pub fn transfers_attempted(&self) -> u64 {
        self.lanes
            .iter()
            .map(|l| l.counters.transfers_attempted)
            .sum()
    }

    /// Blocks whose payload Certus sent us: `LOOKUP` hits, and only those.
    ///
    /// A `LOOKUP` miss transfers nothing, so counting it would invent bandwidth.
    pub fn blocks_read(&self) -> u64 {
        self.lookup_hits()
    }

    /// Blocks whose payload we sent Certus: `COPY_TO_STORE` keys it accepted.
    ///
    /// `COPY_TO_STORE` is where the DMA happens (`copy_gpu_to_memory_async`); `RESERVE` and
    /// `COMMIT_STORE` are control. A declined transfer moved nothing.
    pub fn blocks_written(&self) -> u64 {
        self.transfers_attempted()
            .saturating_sub(self.transfers_declined())
    }

    /// Payload bytes read from Certus.
    pub fn read_bytes(&self) -> u64 {
        self.blocks_read() * self.block_bytes
    }

    /// Payload bytes written to Certus.
    pub fn write_bytes(&self) -> u64 {
        self.blocks_written() * self.block_bytes
    }

    /// Payload bytes read per wallclock second.
    pub fn read_bytes_per_second(&self) -> f64 {
        self.per_second(self.read_bytes())
    }

    /// Payload bytes written per wallclock second.
    pub fn write_bytes_per_second(&self) -> f64 {
        self.per_second(self.write_bytes())
    }

    fn per_second(&self, n: u64) -> f64 {
        if self.elapsed > 0.0 {
            n as f64 / self.elapsed
        } else {
            0.0
        }
    }

    /// Keys a `COMMIT_STORE` declined.
    pub fn commits_declined(&self) -> u64 {
        self.lanes.iter().map(|l| l.counters.commits_declined).sum()
    }

    /// Whether the run is valid: **no lane ever underran** (FR-062).
    ///
    /// An underrun means the generator, not Certus, set the pace at that instant, so the
    /// throughput would describe the instrument rather than the system under test.
    pub fn is_valid(&self) -> bool {
        self.underruns() == 0
    }

    /// Keys per second over the timed window.
    pub fn keys_per_second(&self) -> f64 {
        if self.elapsed > 0.0 {
            self.key_references() as f64 / self.elapsed
        } else {
            0.0
        }
    }

    /// Bytes per second over the timed window.
    /// Payload bytes per wallclock second, read plus written (FR-066).
    ///
    /// # This was wrong, and wrong in the direction that flatters the system
    ///
    /// It used to be `key_references * block_bytes`, which charges a full block to every
    /// key of every request. But most requests move **no payload at all**: `CHECK`, `TOUCH`,
    /// `RESERVE` and `COMMIT_STORE` are control operations. Under the reactive rule a
    /// resident block produces three key references (check, touch, load) and transfers one
    /// block, and a missing one produces four (check, reserve, transfer, commit) and
    /// transfers one — so the old figure overstated bandwidth by three to four times and
    /// conflated reads with writes.
    ///
    /// Bandwidth now comes from the hit/miss results, which is the only place the
    /// information exists: bytes are read for a `LOOKUP` **hit** and written for an accepted
    /// `COPY_TO_STORE`, and nothing else moves a byte.
    ///
    /// Server-side write-through to SSD is additional traffic Certus generates on its own
    /// and is not counted here: this is what crossed the client boundary.
    pub fn bytes_per_second(&self) -> f64 {
        self.per_second(self.read_bytes() + self.write_bytes())
    }

    /// Virtual seconds advanced per wallclock second.
    ///
    /// A **speedup**, not a sustained-load figure: the run is closed-loop and issues as
    /// fast as the mailbox allows, because an open-loop paced mode is out of scope.
    pub fn virtual_to_wallclock(&self) -> f64 {
        if self.elapsed > 0.0 {
            self.virtual_span / self.elapsed
        } else {
            0.0
        }
    }
}

/// One turn's operations, owned so they can cross a thread boundary.
///
/// An [`OperationPlan`] holding a single turn is exactly the right container: it interns
/// the prefix **once** and shares that range between `Check`, `Touch` and `Load`, so a
/// batch costs one prefix copy rather than three. Reusing it also means the queued form
/// and the emitted form are the same type, so they cannot drift.
type Batch = OperationPlan;

/// A lane: one bounded queue and the depth counter its consumer reads.
struct Lane {
    tx: SyncSender<Batch>,
    depth: Arc<AtomicUsize>,
}

/// Attach to a node's mailbox and claim `lanes` channels.
///
/// # Errors
///
/// If the mailbox cannot be attached, or `lanes` exceeds the node's channel count —
/// refused rather than clamped, because the mailbox is depth-1 per channel and
/// over-subscription would serialise silently behind a claimed channel.
pub fn attach(shm_path: &str, lanes: usize) -> Result<(Arc<Client>, Vec<usize>), String> {
    let client = Client::attach(shm_path, ATTACH_TIMEOUT)
        .map_err(|e| format!("attach shmq mailbox {shm_path}: {e}"))?;
    let channels = client.channel_count();
    if lanes == 0 {
        return Err("--lanes must be at least 1".to_string());
    }
    if lanes > channels {
        return Err(format!(
            "refusing to run: {lanes} lanes against a node with {channels} channels. The \
             mailbox is depth-1 per channel, so the extra lanes would not add concurrency \
             — they would serialise behind a claimed channel and the run would report a \
             throughput for a concurrency it never had. Use --lanes {channels} or fewer"
        ));
    }
    let mut claimed = Vec::with_capacity(lanes);
    for _ in 0..lanes {
        match client.claim_channel() {
            Some(ch) => claimed.push(ch),
            None => {
                for ch in &claimed {
                    client.release_channel(*ch);
                }
                return Err(format!(
                    "could only claim {} of {lanes} channels; another client holds the rest",
                    claimed.len()
                ));
            }
        }
    }
    Ok((Arc::new(client), claimed))
}

/// What a live run needs beyond its description and its mailbox.
///
/// A struct rather than seven positional arguments, because `run(client, ch, d, 7, None,
/// 64, Some(0), false, stop)` is a line nobody can read or safely reorder.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct RunOptions {
    /// Seed: with the description, this alone determines the operation stream (FR-072).
    pub seed: u64,
    /// Virtual seconds to run, or `None` for an unbounded run (FR-059).
    pub until: Option<f64>,
    /// Keys per request (FR-069), which also sizes the payload template.
    pub batch_keys: usize,
    /// GPU device for the payload buffer, or `None` for a control-path-only run.
    ///
    /// `None` is honest but partial: `LOOKUP` and `COPY_TO_STORE` are counted and declared
    /// rather than issued, so the throughput is not comparable with a complete run's.
    pub gpu_device: Option<i32>,
    /// Stamp each stored block with its key. Costs a CUDA call per key; see
    /// [`crate::payload`] on why it is off by default.
    pub stamp_keys: bool,
    /// Clear the memory tier once before the timed window opens (FR-046).
    ///
    /// `CLEAR_MEMORY_TIER` is otherwise forbidden — see [`crate::opstream`] — because
    /// clearing mid-run would be the generator evicting on the policy's behalf. Exactly
    /// once, at startup, outside the timed window, is the permitted use.
    ///
    /// It clears the **memory tier**; disk-backed entries survive it, so it does not
    /// guarantee a cold cache.
    pub clear_cache: bool,
}

/// Run the workload: one producer thread, one consumer per lane.
///
/// `until` of `None` is an **unbounded** run (FR-059), possible precisely because the
/// producer is throttled by the queue rather than by memory. It ends when `stop` is set,
/// and an interrupted run whose lanes never underran is **valid** (FR-074).
///
/// # Errors
///
/// If a lane's request fails or a lane thread panics.
pub fn run(
    client: Arc<Client>,
    channels: Vec<usize>,
    description: &WorkloadDescription,
    options: &RunOptions,
    stop: Arc<AtomicBool>,
) -> Result<LiveStats, String> {
    let RunOptions {
        seed,
        until,
        batch_keys,
        gpu_device,
        stamp_keys,
        clear_cache,
    } = *options;
    let block_bytes = u32::try_from(description.blocks.bytes)
        .map_err(|_| "blocks.bytes exceeds a 32-bit reservation".to_string())?;
    let lane_count = channels.len();

    // One allocation for the whole run, filled before the timed window (FR-038). Without
    // it the two data-moving operations cannot be issued and the report says the run was
    // partial — which is a legitimate mode on a node with no accelerator, but never a
    // silent fallback after a device was asked for.
    let payload = match gpu_device {
        Some(device) => Some(Arc::new(PayloadBuffer::new(
            lane_count,
            batch_keys,
            block_bytes,
            device,
            stamp_keys,
        )?)),
        None => None,
    };

    // Before the clock starts (FR-046): a clear is setup, and timing it would charge the
    // run for work no operation in the stream performed.
    let cleared = if clear_cache {
        Some(clear_memory_tier(&client, channels[0])?)
    } else {
        None
    };

    let mut senders: Vec<Lane> = Vec::with_capacity(lane_count);
    let mut consumers = Vec::with_capacity(lane_count);
    let started = Instant::now();

    for (i, channel) in channels.iter().copied().enumerate() {
        let (tx, rx) = sync_channel::<Batch>(QUEUE_CAPACITY);
        let depth = Arc::new(AtomicUsize::new(0));
        senders.push(Lane {
            tx,
            depth: Arc::clone(&depth),
        });
        let client = Arc::clone(&client);
        let stop = Arc::clone(&stop);
        let payload = payload.clone();
        consumers.push(thread::spawn(move || {
            consume(Consumer {
                client,
                channel,
                slot: i,
                block_bytes,
                batch_keys,
                payload,
                rx,
                depth,
                stop,
            })
        }));
    }

    // The producer runs on this thread. Single-threaded and deterministic, so what it
    // builds is a function of description and seed alone (FR-072) — lane count changes
    // only who issues an operation, never which operations exist.
    let produced = produce(description, seed, until, &senders, &stop);
    // Dropping the senders closes each queue, which is how a consumer tells "the run is
    // over" from "the queue is momentarily quiet" — the distinction the underrun count
    // depends on.
    drop(senders);

    let mut lanes = Vec::with_capacity(lane_count);
    let mut latency = Histogram::<u64>::new_with_bounds(1, 60_000_000, 3)
        .map_err(|e| format!("cannot build the latency histogram: {e}"))?;
    let mut latency_by_op: BTreeMap<u32, Histogram<u64>> = BTreeMap::new();
    let mut first_error = None;
    for (i, handle) in consumers.into_iter().enumerate() {
        match handle.join() {
            Ok(Ok((stats, hist, per_op))) => {
                lanes.push(stats);
                latency
                    .add(hist)
                    .map_err(|e| format!("merging lane {i}'s latencies: {e}"))?;
                for (opcode, h) in per_op {
                    match latency_by_op.entry(opcode) {
                        std::collections::btree_map::Entry::Occupied(mut e) => e.get_mut().add(h),
                        std::collections::btree_map::Entry::Vacant(e) => {
                            e.insert(h);
                            Ok(())
                        }
                    }
                    .map_err(|e| format!("merging lane {i}'s opcode {opcode} latencies: {e}"))?;
                }
            }
            Ok(Err(e)) => {
                lanes.push(LaneStats::default());
                first_error = first_error.or(Some(format!("lane {i}: {e}")));
            }
            Err(_) => {
                lanes.push(LaneStats::default());
                first_error = first_error.or(Some(format!("lane {i} panicked")));
            }
        }
    }
    let elapsed = started.elapsed().as_secs_f64();

    for ch in &channels {
        client.release_channel(*ch);
    }
    if let Some(e) = first_error {
        return Err(e);
    }
    let (batches_produced, virtual_span, producer_completed, producer_blocked) = produced?;

    Ok(LiveStats {
        lanes,
        batches_produced,
        producer_completed,
        producer_blocked,
        cleared_entries: cleared,
        block_bytes: description.blocks.bytes,
        elapsed,
        virtual_span,
        latency,
        latency_by_op,
    })
}

/// Build turn batches and push each to its lane, blocking when that lane is full.
///
/// Returns `(batches, virtual span reached, completed)`. Blocking on a full queue is the
/// point: it is the backpressure that bounds memory, and it is what leaves the producer's
/// speed observable at the consumer instead of absorbed by an ever-growing buffer.
fn produce(
    description: &WorkloadDescription,
    seed: u64,
    until: Option<f64>,
    lanes: &[Lane],
    stop: &Arc<AtomicBool>,
) -> Result<(u64, f64, bool, u64), String> {
    let mut sim = Simulation::new(description, seed)
        .map_err(|e| format!("cannot start the simulation: {e}"))?;
    let n = lanes.len();
    let mut batches = 0u64;
    let mut blocked = 0u64;
    // Set when a send fails, meaning every consumer has gone.
    let mut disconnected = false;
    let mut horizon = PRODUCE_WINDOW;

    loop {
        if stop.load(Ordering::Relaxed) || disconnected {
            return Ok((batches, sim.now(), false, blocked));
        }
        if let Some(cap) = until {
            horizon = horizon.min(cap);
        }

        let mut this_window = 0u64;
        sim.run_until(horizon, &mut |session, turn| {
            if disconnected {
                return;
            }
            let mut batch = Batch::default();
            batch.record_turn(session, turn);
            let lane = (session.id() as usize) % n;
            lanes[lane].depth.fetch_add(1, Ordering::Relaxed);
            // `try_send` first so that blocking is *observable*: a full queue is
            // backpressure working, and the positive counterpart to an underrun. Falling
            // straight into a blocking `send` would hide the healthy case.
            match lanes[lane].tx.try_send(batch) {
                Ok(()) => {}
                Err(TrySendError::Full(batch)) => {
                    blocked += 1;
                    if lanes[lane].tx.send(batch).is_err() {
                        lanes[lane].depth.fetch_sub(1, Ordering::Relaxed);
                        disconnected = true;
                        return;
                    }
                }
                Err(TrySendError::Disconnected(_)) => {
                    lanes[lane].depth.fetch_sub(1, Ordering::Relaxed);
                    disconnected = true;
                    return;
                }
            }
            this_window += 1;
        });
        batches += this_window;

        if let Some(cap) = until {
            if horizon >= cap {
                return Ok((batches, cap, true, blocked));
            }
        }
        // An unbounded run with nothing left to do — every population mint-once and every
        // session finished — would otherwise advance the horizon for ever.
        if this_window == 0 && sim.next_event_at().is_none() {
            return Ok((batches, sim.now(), true, blocked));
        }
        horizon += PRODUCE_WINDOW;
    }
}

/// Take batches from one lane's queue and issue them.
///
/// A `try_recv` that returns empty **is** the underrun and is counted before the thread
/// blocks, so the figure is a count of consumer stalls rather than of a sampled gauge and
/// a brief exhaustion cannot fall between samples.
struct Consumer {
    client: Arc<Client>,
    channel: usize,
    /// This lane's disjoint region of the payload buffer.
    slot: usize,
    block_bytes: u32,
    batch_keys: usize,
    payload: Option<Arc<PayloadBuffer>>,
    rx: Receiver<Batch>,
    depth: Arc<AtomicUsize>,
    stop: Arc<AtomicBool>,
}

type LaneLatency = (LaneStats, Histogram<u64>, BTreeMap<u32, Histogram<u64>>);

fn consume(c: Consumer) -> Result<LaneLatency, String> {
    let Consumer {
        client,
        channel,
        slot,
        block_bytes,
        batch_keys,
        payload,
        rx,
        depth,
        stop,
    } = c;
    // The shared executor, so the local path and the node agent apply one rule rather than
    // two that could drift apart (see `crate::exec`).
    let mut exec = TurnExecutor::new(block_bytes, batch_keys)?;
    if let Some(buffer) = payload {
        exec = exec.with_payload(buffer, slot);
    }
    let mut path: Vec<u64> = Vec::new();
    let mut stats = LaneStats {
        min_depth: usize::MAX,
        ..LaneStats::default()
    };

    // Prime: block for the first batch without counting it. Every queue is empty before the
    // producer has pushed anything, so counting that would invalidate every run — see the
    // module docs.
    let mut primed = false;

    loop {
        let batch = match rx.try_recv() {
            Ok(b) => b,
            Err(TryRecvError::Empty) => {
                if primed {
                    // This lane had work and ran out: the generator fell behind, which is
                    // FR-062's event.
                    stats.underruns += 1;
                    stats.min_depth = 0;
                }
                if stop.load(Ordering::Relaxed) {
                    break;
                }
                match rx.recv() {
                    Ok(b) => b,
                    // Closed while we waited, so the run is over rather than underrunning.
                    Err(_) => break,
                }
            }
            Err(TryRecvError::Disconnected) => break,
        };
        primed = true;
        let observed = depth.fetch_sub(1, Ordering::Relaxed).saturating_sub(1);
        stats.pops += 1;
        stats.min_depth = stats.min_depth.min(observed);

        // Everything about which keys, which session and when came from the producer; the
        // executor decides only load-versus-store, from what the cache reports.
        batch.key_path(&mut path);
        let session = batch
            .operations()
            .first()
            .map(|o| o.session())
            .unwrap_or_default();
        let polls = batch
            .operations()
            .iter()
            .any(|o| o.kind() == OpKind::PollEvents);
        exec.run_turn(&client, channel, session, &path, polls)?;
    }

    if stats.min_depth == usize::MAX {
        stats.min_depth = 0;
    }
    stats.counters = *exec.counters();
    let (skipped, skipped_keys) = exec.skipped();
    stats.skipped_needing_gpu = skipped;
    stats.skipped_keys = skipped_keys;
    Ok((stats, exec.latency().clone(), exec.latency_by_op().clone()))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn stats(lanes: Vec<LaneStats>, elapsed: f64) -> LiveStats {
        LiveStats {
            lanes,
            batches_produced: 10,
            producer_completed: true,
            producer_blocked: 0,
            cleared_entries: None,
            latency_by_op: BTreeMap::new(),
            block_bytes: 32_768,
            elapsed,
            virtual_span: 10.0,
            latency: Histogram::<u64>::new_with_bounds(1, 60_000_000, 3).unwrap(),
        }
    }

    fn lane(pops: u64, empty: u64, min_depth: usize) -> LaneStats {
        LaneStats {
            pops,
            underruns: empty,
            min_depth,
            ..Default::default()
        }
    }

    #[test]
    fn a_single_underrun_anywhere_invalidates_the_run() {
        // FR-062, and the property the old implementation could not express: validity now
        // depends on something the system does, not on a loop index.
        let healthy = stats(vec![lane(100, 0, 4), lane(100, 0, 9)], 1.0);
        assert!(healthy.is_valid());
        assert_eq!(healthy.min_depth(), 4);
        assert_eq!(healthy.fraction_underrun(), 0.0);

        let underran = stats(vec![lane(100, 0, 4), lane(100, 1, 0)], 1.0);
        assert!(
            !underran.is_valid(),
            "one underrun on one lane must invalidate the run"
        );
        assert_eq!(underran.underruns(), 1);
        assert_eq!(underran.min_depth(), 0);
    }

    #[test]
    fn per_lane_figures_are_kept_rather_than_averaged() {
        // Session sharding creates imbalance, and an aggregate would hide a lane that
        // underran on every pop behind another that never did.
        let s = stats(vec![lane(10, 10, 0), lane(1_000, 0, 12)], 1.0);
        assert_eq!(s.lanes[0].underruns, 10);
        assert_eq!(s.lanes[1].underruns, 0);
        assert!(!s.is_valid());
        // The overall fraction is under 1%, which is exactly why the per-lane figure is
        // reported beside it rather than instead of it.
        assert!(s.fraction_underrun() < 0.01);
    }

    #[test]
    fn rates_aggregate_across_lanes_and_never_produce_a_nan() {
        let s = stats(
            vec![
                LaneStats {
                    counters: Counters {
                        requests: 50,
                        key_references: 500,
                        ..Default::default()
                    },
                    ..Default::default()
                },
                LaneStats {
                    counters: Counters {
                        requests: 50,
                        key_references: 1_500,
                        ..Default::default()
                    },
                    ..Default::default()
                },
            ],
            2.0,
        );
        assert_eq!(s.requests(), 100);
        assert_eq!(s.key_references(), 2_000);
        assert_eq!(s.keys_per_second(), 1_000.0);
        // Key references are NOT bandwidth. These lanes issued 2000 key references and
        // moved **nothing**: no LOOKUP hit and no accepted transfer. The old formula was
        // `key_references * block_bytes`, which charged a full block to every CHECK and
        // TOUCH and so reported 32 MB/s for a run that transferred no payload at all.
        assert_eq!(
            s.bytes_per_second(),
            0.0,
            "control-only traffic must report zero bandwidth"
        );

        let empty = stats(vec![LaneStats::default()], 0.0);
        assert_eq!(empty.keys_per_second(), 0.0);
        assert_eq!(empty.bytes_per_second(), 0.0);
        assert_eq!(empty.virtual_to_wallclock(), 0.0);
        assert_eq!(empty.fraction_underrun(), 0.0);
    }

    #[test]
    fn bandwidth_counts_a_lookup_hit_as_read_and_an_accepted_transfer_as_write() {
        // The only two payload-moving operations. A LOOKUP miss brings no bytes, and a
        // declined COPY_TO_STORE sends none, so both are excluded — otherwise a run against
        // a cold cache would report reading data it never received.
        let s = stats(
            vec![LaneStats {
                counters: Counters {
                    lookup_hits: 10,
                    lookup_misses: 90,
                    transfers_attempted: 8,
                    transfers_declined: 3,
                    ..Default::default()
                },
                ..Default::default()
            }],
            2.0,
        );
        assert_eq!(s.blocks_read(), 10, "only hits carry a payload");
        assert_eq!(s.blocks_written(), 5, "8 attempted less 3 declined");
        assert_eq!(s.read_bytes(), 10 * 32_768);
        assert_eq!(s.write_bytes(), 5 * 32_768);
        assert_eq!(s.read_bytes_per_second(), 10.0 * 32_768.0 / 2.0);
        assert_eq!(s.write_bytes_per_second(), 5.0 * 32_768.0 / 2.0);
        assert_eq!(s.bytes_per_second(), 15.0 * 32_768.0 / 2.0);
    }

    #[test]
    fn a_run_that_only_missed_reports_no_read_bandwidth() {
        // The direction of the old error: a cold run issues many references and reads
        // nothing, and must not be credited with bandwidth for the misses.
        let s = stats(
            vec![LaneStats {
                counters: Counters {
                    key_references: 10_000,
                    lookup_hits: 0,
                    lookup_misses: 500,
                    ..Default::default()
                },
                ..Default::default()
            }],
            1.0,
        );
        assert_eq!(s.read_bytes(), 0);
        assert_eq!(s.blocks_read(), 0);
    }

    #[test]
    fn a_lane_that_never_received_anything_is_not_counted_as_underrun() {
        // The startup transient. A lane with no pops at all never got past priming, so it
        // contributes no underrun — otherwise a run that ended before a lane's first
        // batch would be invalid for a reason that says nothing about the generator.
        let s = stats(vec![lane(0, 0, 0), lane(50, 0, 7)], 1.0);
        assert!(s.is_valid());
        assert_eq!(s.underruns(), 0);
    }

    #[test]
    fn depth_is_in_turns_and_capped_so_memory_does_not_track_the_span() {
        // The property that makes an unbounded run possible. Stated as an assertion
        // because it is the reason the queue exists, and a later "let's make it bigger"
        // should have to argue with it.
        assert!(QUEUE_CAPACITY > 0);
        assert!(
            QUEUE_CAPACITY <= 1024,
            "a deeper queue holds more prefixes than a long-session workload can afford"
        );
    }
}
