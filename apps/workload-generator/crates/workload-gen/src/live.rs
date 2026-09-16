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

use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::mpsc::{sync_channel, Receiver, SyncSender, TryRecvError, TrySendError};
use std::sync::Arc;
use std::thread;
use std::time::{Duration, Instant};

use hdrhistogram::Histogram;
use shm_queue::Client;
use shmq_dispatcher::wire;
use workload_model::description::WorkloadDescription;
use workload_model::plan::OperationPlan;
use workload_model::sim::Simulation;

use crate::opstream::{forbidden_opcode, Encoding, OpStream};
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

/// Spin iterations before a request parks. Matches `apps/remote-lookup-bench`, so the two
/// tools contend for the mailbox the same way.
const SPIN_ITERS: u32 = 20_000;

/// Per-attempt wait before retrying a response poll.
const ATTEMPT_TIMEOUT: Duration = Duration::from_millis(50);

/// Overall deadline for one request.
const REQUEST_DEADLINE: Duration = Duration::from_secs(30);

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
    /// Requests issued.
    pub requests: u64,
    /// Key references issued.
    pub key_references: u64,
    /// Operations skipped for want of a GPU payload buffer.
    pub skipped_needing_gpu: u64,
    /// Keys those skipped operations would have moved.
    pub skipped_keys: u64,
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
    /// Bytes per block, from the description's geometry.
    pub block_bytes: u64,
    /// Wallclock seconds of the timed window.
    pub elapsed: f64,
    /// Virtual seconds the plan advanced.
    pub virtual_span: f64,
    /// Per-request latency across every lane.
    pub latency: Histogram<u64>,
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
        self.lanes.iter().map(|l| l.requests).sum()
    }

    /// Key references issued.
    pub fn key_references(&self) -> u64 {
        self.lanes.iter().map(|l| l.key_references).sum()
    }

    /// Operations skipped for want of a GPU payload buffer.
    pub fn skipped_needing_gpu(&self) -> u64 {
        self.lanes.iter().map(|l| l.skipped_needing_gpu).sum()
    }

    /// Keys those skipped operations would have moved.
    pub fn skipped_keys(&self) -> u64 {
        self.lanes.iter().map(|l| l.skipped_keys).sum()
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
    pub fn bytes_per_second(&self) -> f64 {
        self.keys_per_second() * self.block_bytes as f64
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
    let mut first_error = None;
    for (i, handle) in consumers.into_iter().enumerate() {
        match handle.join() {
            Ok(Ok((stats, hist))) => {
                lanes.push(stats);
                latency
                    .add(hist)
                    .map_err(|e| format!("merging lane {i}'s latencies: {e}"))?;
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
        block_bytes: description.blocks.bytes,
        elapsed,
        virtual_span,
        latency,
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

fn consume(c: Consumer) -> Result<(LaneStats, Histogram<u64>), String> {
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
    let mut stream = OpStream::new(block_bytes, batch_keys);
    if let Some(buffer) = payload {
        stream = stream.with_payload(buffer, slot);
    }
    let mut stats = LaneStats {
        min_depth: usize::MAX,
        ..LaneStats::default()
    };
    let mut latency = Histogram::<u64>::new_with_bounds(1, 60_000_000, 3)
        .map_err(|e| format!("cannot build the latency histogram: {e}"))?;

    // Prime: block for the first batch without counting it. Every queue is empty before
    // the producer has pushed anything, so counting that would invalidate every run —
    // see the module docs.
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

        for op in batch.operations() {
            let all = batch.keys_of(op);
            // Split to --batch-keys (FR-069). An operation with no keys — a poll — still
            // issues exactly one request, which is why this is a loop over offsets rather
            // than `chunks()`: `[].chunks(n)` yields nothing.
            let mut start = 0usize;
            loop {
                let end = (start + batch_keys).min(all.len());
                let chunk = &all[start..end];
                let (opcode, payload) = match stream.encode_chunk(op.kind(), chunk, op.session())? {
                    Encoding::Ready { opcode, payload } => (opcode, payload),
                    Encoding::NotPlanned => break,
                    Encoding::NeedsGpuPayload { keys, .. } => {
                        stats.skipped_needing_gpu += 1;
                        stats.skipped_keys += keys as u64;
                        start = end;
                        if start >= all.len() {
                            break;
                        }
                        continue;
                    }
                };
                if let Some(why) = forbidden_opcode(opcode) {
                    return Err(format!("refusing to issue a forbidden operation: {why}"));
                }
                // One clock read per request, never per key: a batch of 64 keys is one
                // round trip, so per-key timing would measure the same interval 64 times
                // and put a clock on the per-key path (FR-038, FR-070).
                let started = Instant::now();
                let (status, body) = client
                    .request(
                        channel,
                        opcode,
                        payload,
                        SPIN_ITERS,
                        ATTEMPT_TIMEOUT,
                        REQUEST_DEADLINE,
                    )
                    .map_err(|e| format!("shmq request (op {opcode}) failed: {e}"))?;
                if status != wire::STATUS_OK {
                    return Err(format!(
                        "op {opcode} returned an error status: {}",
                        String::from_utf8_lossy(&body)
                    ));
                }
                let micros = started.elapsed().as_micros().clamp(1, 60_000_000) as u64;
                latency
                    .record(micros)
                    .map_err(|e| format!("latency out of histogram range: {e}"))?;
                stats.requests += 1;
                stats.key_references += chunk.len() as u64;
                start = end;
                if start >= all.len() {
                    break;
                }
            }
        }
    }

    if stats.min_depth == usize::MAX {
        stats.min_depth = 0;
    }
    Ok((stats, latency))
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
                    requests: 50,
                    key_references: 500,
                    ..Default::default()
                },
                LaneStats {
                    requests: 50,
                    key_references: 1_500,
                    ..Default::default()
                },
            ],
            2.0,
        );
        assert_eq!(s.requests(), 100);
        assert_eq!(s.key_references(), 2_000);
        assert_eq!(s.keys_per_second(), 1_000.0);
        assert_eq!(s.bytes_per_second(), 1_000.0 * 32_768.0);

        let empty = stats(vec![LaneStats::default()], 0.0);
        assert_eq!(empty.keys_per_second(), 0.0);
        assert_eq!(empty.virtual_to_wallclock(), 0.0);
        assert_eq!(empty.fraction_underrun(), 0.0);
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
