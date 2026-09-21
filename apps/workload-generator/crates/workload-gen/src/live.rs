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
//! # The underrun is counted, never sampled — but it is not what validity rests on
//!
//! Sampling depth as a gauge can miss a brief exhaustion between samples — the same
//! failure as reporting an average, one level down. So a consumer that finds its queue
//! empty **is** the underrun, and it is counted directly. The minimum depth is
//! recorded at pop time beside it, because a consumer's view of the queue is the one that
//! matters.
//!
//! ## Why the count cannot be the test: it weighs a microsecond like a minute
//!
//! At the instant a run starts every queue is empty, because nothing has been produced yet,
//! so a consumer's first looks always find it so. A **count** cannot tell those from a
//! mid-run stall, and validity required the count to be *zero*, so no work-conserving run
//! could ever be valid. That is not hypothetical: on the four-instance stress run the
//! producer finished the entire 10-second span in **0.32s of a 180s run** and never once
//! blocked on a full queue — it was demonstrably never the constraint — and the run was
//! still disqualified by 88 empty pops, each of them microseconds long.
//!
//! A `primed` flag used to suppress each lane's *first* look. It could not work: the fill
//! takes several looks, not one, so runs stayed invalid; and it left the reported count
//! neither the raw truth nor the thing being adjudicated. It is gone.
//!
//! What replaces it is [`LaneStats::producer_wait_us`] — the **time** a lane spent blocked
//! waiting for the producer — against the lane-time available, bounded by
//! [`DEFAULT_PRODUCER_WAIT_TOLERANCE`]. That reads directly as the share by which the
//! reported throughput understates Certus, which is the claim FR-062 has to settle, and it
//! separates the unavoidable from the disqualifying by six orders of magnitude rather than
//! by a flag. The empty-pop count is still reported; it is a symptom, not the verdict.
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
//! A run with little waiting **and** a producer that blocked is one where the queue
//! demonstrably did its job: the first says the lanes were never held up, the second says the
//! generator was ahead rather than merely keeping pace.
//!
//! # What is here, and what moved
//!
//! This module is the **producer** and the figures a run reports. It no longer executes anything:
//! FR-079 routes every node through an agent, so the mailbox-facing code — the executor, the
//! payload buffer, the opcode mapping — lives in `workload-node-agent`, and the submission side
//! lives in [`crate::drive`]. What is left here is what both need and neither owns: the bounded
//! queue, the routing seam, and [`LiveStats`].
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
//! # Waiting means something different under pacing (FR-080)
//!
//! Everything above describes a **work-conserving** run, which issues as fast as the mailbox
//! allows and measures the ceiling. Under [`Pacing::Real`] an empty queue is the normal, intended
//! state — nothing is due yet, and a paced producer emits only what is due, so its lanes idle by
//! design and measurably so: around **6% of lane-time** in `pacing.rs`'s slow-node run. Waiting
//! therefore stops meaning "the generator was the constraint"
//! and validity becomes **lateness** instead: how far past its due time each turn was submitted.
//! Adopting pacing therefore *replaced* a metric rather than adding a delay, which is why
//! [`LiveStats::is_valid`] asks which mode the run was in.

use std::collections::BTreeMap;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::mpsc::{sync_channel, Receiver, SyncSender, TrySendError};
use std::sync::Arc;

use hdrhistogram::Histogram;
use workload_model::description::WorkloadDescription;
use workload_model::plan::OperationPlan;
use workload_model::sim::Simulation;
use workload_wire::frame::Counters;

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

/// Whether a run holds each turn until its virtual time is due.
///
/// The two modes answer different questions and both are wanted: work-conserving measures *how
/// fast Certus can go*, paced measures *the latency Certus delivers under the load this workload
/// actually represents*. They are not comparable, which is why the report names the mode.
///
/// **Paced is the default**, and the argument is the constitution's own — each measurement
/// principle guards a failure mode that produces plausible numbers rather than an error, and the
/// two defaults are asymmetric under that test. Paced when a ceiling was wanted returns a
/// throughput capped at the rate that was asked for, which is conspicuous. Work-conserving when
/// the workload's latency was wanted returns percentiles from a **saturated** queue, which look
/// entirely plausible and describe a queue the workload would never form.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Pacing {
    /// Hold each turn until its virtual time is due.
    #[default]
    Real,
    /// Issue as fast as the transport allows, and measure the ceiling.
    None,
}

impl Pacing {
    /// The name the report prints, since a paced figure and a work-conserving one are not
    /// comparable and would otherwise be quoted side by side.
    pub fn name(self) -> &'static str {
        match self {
            Self::Real => "paced",
            Self::None => "work-conserving",
        }
    }
}

/// Lateness a paced run tolerates at the 99th percentile, in microseconds.
///
/// Coarse on purpose, and the two bounds are what set it. It has to be well **above** the jitter
/// of asking an OS to wake a thread at a time — a millisecond of sleep granularity must never
/// invalidate a run — and well **below** anything that would disturb the figures being reported,
/// which are per-operation latencies in the tens to hundreds of microseconds. 100 ms sits between
/// those by three orders of magnitude at each end.
pub const DEFAULT_LATENESS_TOLERANCE_US: u64 = 100_000;

/// The share of lane-time a work-conserving run may spend waiting for the producer before its
/// throughput stops describing Certus (FR-062).
///
/// Not zero, and that is the point. Every lane's queue is empty at `t = 0`, so a consumer
/// cannot help waiting while the producer gets ahead; requiring zero made **every**
/// work-conserving run invalid, which is how this tolerance came to exist. Measured on the
/// four-instance stress run, that startup wait is microseconds against a 180-second window —
/// six orders of magnitude below this bound — while a producer that genuinely could not keep
/// up would spend seconds per lane and exceed it by a wide margin. 1% therefore separates the
/// unavoidable from the disqualifying without sitting near either.
pub const DEFAULT_PRODUCER_WAIT_TOLERANCE: f64 = 0.01;

/// One lane's view of its own queue and its own traffic.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct LaneStats {
    /// Batches taken.
    pub pops: u64,
    /// Pops that found the queue **empty**.
    ///
    /// Reported, but **not** what validity rests on: a count weighs a startup wait of a few
    /// microseconds exactly as heavily as a mid-run stall of ten seconds. See
    /// [`LaneStats::producer_wait_us`], which is the figure FR-062 is actually about.
    pub underruns: u64,
    /// Microseconds this lane spent blocked waiting for the producer.
    ///
    /// The quantity FR-062 means by "the generator set the pace": time the lane could have
    /// spent driving Certus and could not, because the next turn did not exist yet. Summed
    /// across lanes and divided by the lane-time available, it *is* the fraction by which the
    /// reported throughput understates the system.
    pub producer_wait_us: u64,
    /// Smallest depth seen at pop time.
    pub min_depth: usize,
    /// Operations skipped for want of a GPU payload buffer.
    pub skipped_needing_gpu: u64,
    /// Keys those skipped operations would have moved.
    pub skipped_keys: u64,
    /// Turns this lane held for their due time, which is every turn under pacing and none
    /// otherwise. Reported so a lateness percentile always has its `n` beside it (FR-066a).
    pub paced_turns: u64,
    /// What the mailbox-facing code on this lane's node counted.
    ///
    /// The agent's `TurnExecutor` gathers these and ships back this very type, rather than the
    /// generator counting anything of its own — which is what makes one node's figure and
    /// another's the same measurement rather than two that happen to be named alike.
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
    /// Which mode the run was in, which decides what its validity means.
    pub pacing: Pacing,
    /// Virtual seconds per wallclock second the run was aimed at.
    pub rate: f64,
    /// How far past its due time each turn was submitted, in microseconds (FR-080).
    ///
    /// One-sided by construction: pacing never submits early, so there is nothing negative to
    /// record. Empty under [`Pacing::None`], which has no schedule to be late for.
    pub lateness: Histogram<u64>,
    /// The 99th-percentile lateness this run tolerated before calling itself invalid.
    pub lateness_tolerance_us: u64,
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

    /// Microseconds lanes spent waiting for the producer, summed across lanes.
    pub fn producer_wait_us(&self) -> u64 {
        self.lanes.iter().map(|l| l.producer_wait_us).sum()
    }

    /// The worst single lane's wait, in microseconds.
    ///
    /// Reported beside the aggregate because one starved lane among sixteen is invisible in a
    /// mean — the aggregate would read 1/16th of its true severity.
    pub fn worst_lane_producer_wait_us(&self) -> u64 {
        self.lanes
            .iter()
            .map(|l| l.producer_wait_us)
            .max()
            .unwrap_or(0)
    }

    /// Fraction of the available **lane-time** spent waiting for the producer.
    ///
    /// The denominator is `lanes x elapsed`, not `elapsed`: with sixteen lanes the run has
    /// sixteen lane-seconds per wallclock second to spend, and this is the share of them lost
    /// to an empty queue. That makes it read directly as the amount by which the reported
    /// throughput understates Certus — which is exactly the claim FR-062 has to adjudicate.
    pub fn producer_wait_fraction(&self) -> f64 {
        let lane_time_us = self.elapsed * 1e6 * self.lanes.len() as f64;
        if lane_time_us <= 0.0 {
            0.0
        } else {
            self.producer_wait_us() as f64 / lane_time_us
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

    /// Loaded blocks whose stamp did not match the key asked for.
    ///
    /// **Non-zero means Certus returned the wrong block.** Unlike every other figure here that
    /// is a correctness failure rather than a cache outcome, so [`LiveStats::is_valid`] treats
    /// it as invalidating: a throughput measured while the wrong data came back is not a
    /// result.
    pub fn payload_mismatches(&self) -> u64 {
        self.lanes
            .iter()
            .map(|l| l.counters.payload_mismatches)
            .sum()
    }

    /// Loaded blocks checked, so a mismatch count has a denominator.
    pub fn payloads_verified(&self) -> u64 {
        self.lanes
            .iter()
            .map(|l| l.counters.payloads_verified)
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

    /// Turns held for their due time — the `n` a lateness percentile needs (FR-066a).
    pub fn paced_turns(&self) -> u64 {
        self.lanes.iter().map(|l| l.paced_turns).sum()
    }

    /// Lateness at a quantile, in microseconds.
    pub fn lateness_us(&self, quantile: f64) -> u64 {
        self.lateness.value_at_quantile(quantile)
    }

    /// Worst lateness seen, in microseconds.
    pub fn max_lateness_us(&self) -> u64 {
        self.lateness.max()
    }

    /// Whether the run kept the schedule it set itself (FR-080).
    ///
    /// Only meaningful under [`Pacing::Real`]; a work-conserving run has no schedule, so this is
    /// vacuously true there and [`LiveStats::is_valid`] uses the underrun count instead.
    pub fn kept_the_schedule(&self) -> bool {
        if self.pacing == Pacing::None || self.lateness.is_empty() {
            return true;
        }
        self.lateness_us(0.99) <= self.lateness_tolerance_us
    }

    /// Whether the run is valid — and **which test that is depends on the mode**.
    ///
    /// Work-conserving: **lanes spent almost none of their time waiting for the producer**
    /// (FR-062), the bound being [`DEFAULT_PRODUCER_WAIT_TOLERANCE`] of lane-time. Time, not a
    /// count of empty pops: the count cannot tell a startup wait from a stall, and since every
    /// queue starts empty it can never reach zero, so requiring zero of it disqualified every
    /// work-conserving run — measured, on runs whose producer had in fact finished the entire
    /// span in 0.32s of 180s and never once blocked on a full queue.
    ///
    /// Paced: **the schedule was kept** (FR-080). An empty queue is the normal, intended state
    /// there — nothing is due yet — so the underrun count carries no information at all and
    /// applying FR-062 would invalidate every paced run. What replaces it is lateness, and it
    /// fails for the same underlying reason: the pace came from somewhere other than the
    /// workload's own timing.
    pub fn is_valid(&self) -> bool {
        // A wrong block invalidates as surely as either, and for the same reason: the
        // throughput would describe something other than the system serving this workload
        // correctly. It is the one cache-facing figure that is a failure rather than an
        // outcome, and it holds in both modes.
        if self.payload_mismatches() != 0 {
            return false;
        }
        match self.pacing {
            Pacing::Real => self.kept_the_schedule(),
            Pacing::None => self.producer_wait_fraction() <= DEFAULT_PRODUCER_WAIT_TOLERANCE,
        }
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
    /// Two different things depending on the mode. Work-conserving, it is a **speedup**: the run
    /// is closed-loop and issues as fast as the transport allows. Paced, it is a **cross-check on
    /// the requested rate** — it should come out at approximately [`LiveStats::rate`], and a
    /// shortfall is accumulated lateness arriving by a second route. The two must agree; if they
    /// do not, one of them is measuring something else.
    pub fn virtual_to_wallclock(&self) -> f64 {
        if self.elapsed > 0.0 {
            self.virtual_span / self.elapsed
        } else {
            0.0
        }
    }

    /// How far the achieved rate fell short of the requested one, as a fraction.
    ///
    /// `None` for a work-conserving run, which asked for no rate. Zero or negative means the
    /// schedule was kept; positive is the same information the lateness percentiles carry.
    pub fn rate_shortfall(&self) -> Option<f64> {
        if self.pacing == Pacing::None || self.rate <= 0.0 {
            return None;
        }
        Some((self.rate - self.virtual_to_wallclock()) / self.rate)
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
pub(crate) struct Lane {
    tx: SyncSender<Batch>,
    depth: Arc<AtomicUsize>,
}

impl Lane {
    /// A lane and the receiver its consumer takes from.
    pub(crate) fn pair(capacity: usize) -> (Self, Receiver<Batch>) {
        let (tx, rx) = sync_channel::<Batch>(capacity);
        (
            Self {
                tx,
                depth: Arc::new(AtomicUsize::new(0)),
            },
            rx,
        )
    }

    /// The shared depth counter, so a consumer can report what it saw at pop time.
    pub(crate) fn depth(&self) -> Arc<AtomicUsize> {
        Arc::clone(&self.depth)
    }
}

/// What a run needs beyond its description and its nodes.
///
/// A struct rather than a row of positional arguments, because `run(d, 7, None, 64, false,
/// Real, 1.0, stop)` is a line nobody can read or safely reorder.
///
/// Everything that describes the **payload** — the GPU device, key stamping, verification — used
/// to live here and is now an agent argument: the agent owns the device buffer, and under FR-079
/// the generator has none. Everything left is either the workload's identity or the tempo it is
/// played at.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct RunOptions {
    /// Seed: with the description, this alone determines the operation stream (FR-072).
    pub seed: u64,
    /// Virtual seconds to run, or `None` for an unbounded run (FR-059).
    pub until: Option<f64>,
    /// Keys per request (FR-069).
    pub batch_keys: usize,
    /// Clear every node's memory tier once before the timed window opens (FR-046).
    ///
    /// `CLEAR_MEMORY_TIER` is otherwise forbidden — clearing mid-run would be the generator
    /// evicting on the policy's behalf (FR-043). Exactly once, at startup, outside the timed
    /// window, is the permitted use.
    ///
    /// It clears the **memory tier**; disk-backed entries survive it, so it does not guarantee a
    /// cold cache.
    pub clear_cache: bool,
    /// Whether each turn waits for its virtual time to be due (FR-080).
    pub pacing: Pacing,
    /// Virtual seconds per wallclock second, under [`Pacing::Real`].
    ///
    /// A **calibration** control, not a convenience. A description's durations are arbitrary with
    /// respect to any particular machine — `think_time` and session lifetimes reflect whatever
    /// hardware the workload was observed on — so on faster hardware the same description
    /// under-drives the system and on slower hardware it over-drives it. This is how one
    /// description is aimed at different targets without being rewritten, which is also why it is
    /// a command-line option and never a field of the description (FR-069's reasoning, FR-005
    /// portability).
    ///
    /// It changes only the **tempo**: the same keys in the same order with the same virtual
    /// interleaving, submitted faster or slower. A run's plan fingerprint is independent of it.
    pub rate: f64,
    /// The 99th-percentile lateness a paced run tolerates before calling itself invalid.
    pub lateness_tolerance_us: u64,
}

impl Default for RunOptions {
    fn default() -> Self {
        Self {
            seed: 0,
            until: None,
            batch_keys: 64,
            clear_cache: false,
            pacing: Pacing::default(),
            rate: 1.0,
            lateness_tolerance_us: DEFAULT_LATENESS_TOLERANCE_US,
        }
    }
}

/// Build turns and route each to a lane, blocking when that lane's queue is full.
///
/// Returns `(batches, virtual span reached, completed, times blocked)`. Blocking on a full queue
/// is the point: it is the backpressure that bounds memory, and it is what leaves the producer's
/// speed observable at the consumer instead of absorbed by an ever-growing buffer.
///
/// What a production run got through.
///
/// A struct rather than a tuple because the tuple was already four wide and the fifth element
/// is exactly the one that went missing: `migrations` existed on `DriveStats`, documented as
/// being there "so a run that exercised none is visible", and was never filled in or printed.
/// A named field cannot be dropped on the floor by a destructuring that ignores it.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct Produced {
    /// Batches pushed to the lanes.
    pub batches: u64,
    /// Virtual seconds covered.
    pub virtual_span: f64,
    /// Whether the run reached its own end rather than being stopped.
    pub completed: bool,
    /// Times the producer blocked on a full queue.
    pub blocked: u64,
    /// Migrations the simulation performed (FR-048, FR-049).
    pub migrations: u64,
}

/// # Why the routing is a parameter
///
/// A target is "somewhere a turn can be sent", and one node's lane is not distinguishable here
/// from another's — the same simulation, the same turns, the same order. Keeping the routing out
/// of the producer is what let the local and the remote path share it while both existed, and
/// what now lets [`crate::drive`] compose placement with a within-node lane in one expression.
///
/// `route` MUST be a function of the session alone, and MUST NOT consult anything the cache
/// reported: routing decides *where* a turn goes, never *what* it is (FR-072).
pub(crate) fn produce<R>(
    description: &WorkloadDescription,
    seed: u64,
    until: Option<f64>,
    nodes: usize,
    lanes: &[Lane],
    route: R,
    stop: &Arc<AtomicBool>,
) -> Result<Produced, String>
where
    R: Fn(&workload_model::session::Session, usize) -> usize,
{
    // The node count belongs to the simulation, not to the routing: placement and migration are
    // decisions it makes (FR-048), and the router only reads the answer. It is passed to the
    // constructor because the population seeded at t=0 is placed there; supplying it afterwards
    // put every session alive at t=0 on node 0, which drove one instance hard and the rest barely
    // at all.
    let mut sim = Simulation::new(description, seed, nodes)
        .map_err(|e| format!("cannot start the simulation: {e}"))?;
    let n = lanes.len();
    let mut batches = 0u64;
    let mut blocked = 0u64;
    // Set when a send fails, meaning every consumer has gone.
    let mut disconnected = false;
    let mut horizon = PRODUCE_WINDOW;

    loop {
        if stop.load(Ordering::Relaxed) || disconnected {
            return Ok(Produced {
                batches,
                virtual_span: sim.now(),
                completed: false,
                blocked,
                migrations: sim.migrations(),
            });
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
            let lane = route(session, n).min(n - 1);
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
                return Ok(Produced {
                    batches,
                    virtual_span: cap,
                    completed: true,
                    blocked,
                    migrations: sim.migrations(),
                });
            }
        }
        // An unbounded run with nothing left to do — every population mint-once and every
        // session finished — would otherwise advance the horizon for ever.
        if this_window == 0 && sim.next_event_at().is_none() {
            return Ok(Produced {
                batches,
                virtual_span: sim.now(),
                completed: true,
                blocked,
                migrations: sim.migrations(),
            });
        }
        horizon += PRODUCE_WINDOW;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn hist() -> Histogram<u64> {
        Histogram::<u64>::new_with_bounds(1, 60_000_000, 3).unwrap()
    }

    /// A **work-conserving** run's figures, so the underrun tests read as they always did.
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
            latency: hist(),
            pacing: Pacing::None,
            rate: 1.0,
            lateness: hist(),
            lateness_tolerance_us: DEFAULT_LATENESS_TOLERANCE_US,
        }
    }

    /// The same run, paced, with `lateness` in microseconds.
    fn paced(lanes: Vec<LaneStats>, elapsed: f64, lateness_us: &[u64]) -> LiveStats {
        let mut s = stats(lanes, elapsed);
        s.pacing = Pacing::Real;
        for v in lateness_us {
            s.lateness.record(*v).unwrap();
        }
        s
    }

    fn lane(pops: u64, empty: u64, min_depth: usize) -> LaneStats {
        LaneStats {
            pops,
            underruns: empty,
            min_depth,
            ..Default::default()
        }
    }

    /// A lane that found the queue empty `empty` times and spent `wait_us` doing it.
    ///
    /// The two are independent on purpose: the whole point of the time-based test is that a
    /// count says nothing about severity.
    fn lane_waiting(pops: u64, empty: u64, min_depth: usize, wait_us: u64) -> LaneStats {
        LaneStats {
            pops,
            underruns: empty,
            min_depth,
            producer_wait_us: wait_us,
            ..Default::default()
        }
    }

    #[test]
    fn time_waiting_for_the_generator_decides_validity_not_the_count_of_empty_pops() {
        // FR-062. The count cannot express severity: every lane's queue is empty at t=0, so a
        // consumer always finds it empty a few times while the producer gets ahead, and
        // requiring zero of those made every work-conserving run invalid. Measured on the
        // stress run: the producer had finished the whole span in 0.32s of 180s and never
        // blocked on a full queue, yet 88 empty pops disqualified it.
        //
        // Two lanes, one wallclock second each, so lane-time is 2s.
        let brief = stats(
            vec![lane_waiting(100, 6, 0, 40), lane_waiting(100, 5, 0, 30)],
            1.0,
        );
        assert!(
            brief.is_valid(),
            "70us of waiting across 2s of lane-time is the startup fill, not the generator \
             setting the pace"
        );
        // ... and the empty pops are still counted and reported, just not adjudicated on.
        assert_eq!(brief.underruns(), 11);
        assert!(brief.producer_wait_fraction() < DEFAULT_PRODUCER_WAIT_TOLERANCE);

        // Fewer empty pops, but half a second of real starvation on one of them.
        let starved = stats(
            vec![lane_waiting(100, 2, 0, 500_000), lane_waiting(100, 0, 9, 0)],
            1.0,
        );
        assert!(
            !starved.is_valid(),
            "a lane idle for 0.5s of 2s lane-time was waiting on the generator, whatever the \
             count says"
        );
        assert!(
            starved.underruns() < brief.underruns(),
            "fewer events, worse run"
        );
    }

    #[test]
    fn per_lane_figures_are_kept_rather_than_averaged() {
        // Session sharding creates imbalance, and an aggregate would hide one starved lane
        // behind fifteen healthy ones — divided by sixteen, a lane idle for the whole run
        // reads as 6%.
        let mut lanes = vec![lane_waiting(10, 10, 0, 900_000)];
        lanes.extend(vec![lane_waiting(1_000, 0, 12, 0); 15]);
        let s = stats(lanes, 1.0);
        assert_eq!(s.lanes[0].underruns, 10);
        assert_eq!(s.lanes[1].underruns, 0);
        assert!(
            !s.is_valid(),
            "one lane idle for 90% of the run is not a valid run"
        );
        // The worst lane is reported precisely so this is visible: it spent 90% of the run
        // waiting, while the aggregate over sixteen lanes reads under 6%. The dilution is real
        // — spread thin enough (hundreds of lanes) one starved lane would fall under the
        // tolerance, and the worst-lane figure is what keeps it from being silent.
        assert_eq!(s.worst_lane_producer_wait_us(), 900_000);
        assert!(s.producer_wait_fraction() < 0.06);
        // The overall fraction of pops is likewise under 1%, which is why the per-lane
        // figures are reported beside the aggregates rather than instead of them.
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
    fn under_pacing_an_empty_queue_is_normal_and_lateness_is_the_test_instead() {
        // FR-080's substance. A paced run's queue is *supposed* to run dry — nothing is due yet —
        // so applying FR-062 there would invalidate every paced run for the thing it was designed
        // to do. The replacement fails for the same underlying reason: the pace came from
        // somewhere other than the workload's own timing.
        // A paced lane idle most of the run is doing exactly what pacing asks of it.
        let underran_but_on_time = paced(
            vec![lane_waiting(100, 90, 0, 900_000)],
            1.0,
            &[0, 1_000, 2_000],
        );
        assert!(
            underran_but_on_time.is_valid(),
            "0.9s of an idle queue out of 1s must not invalidate a paced run"
        );
        assert!(underran_but_on_time.kept_the_schedule());

        // The same queue statistics *do* invalidate a work-conserving run, where an idle lane
        // is work the run could have done and did not.
        let same_run_unpaced = stats(vec![lane_waiting(100, 90, 0, 900_000)], 1.0);
        assert!(!same_run_unpaced.is_valid());

        // And lateness beyond tolerance invalidates the paced one, where the queue could not.
        let late = paced(
            vec![lane(100, 0, 8)],
            1.0,
            &[DEFAULT_LATENESS_TOLERANCE_US * 4; 50],
        );
        assert!(
            !late.is_valid(),
            "a schedule this badly missed is not a run"
        );
        assert!(!late.kept_the_schedule());
        assert!(late.underruns() == 0, "and not because of the queue");
    }

    #[test]
    fn lateness_percentiles_come_with_their_request_count() {
        // FR-066a, and the recorded reason for it: a p50 over 20 requests once appeared to show
        // TOUCH three times slower than CHECK, and at 8000 requests the two were within a
        // microsecond. A percentile without its `n` is not a measurement.
        let s = paced(
            vec![LaneStats {
                paced_turns: 4,
                ..lane(4, 0, 3)
            }],
            1.0,
            &[10, 20, 30, 4_000],
        );
        assert_eq!(s.paced_turns(), 4);
        // A histogram reports the top of the bucket a value fell in, not the value: at three
        // significant figures 4000 µs comes back as the highest microsecond equivalent to it. So
        // this is a range rather than an equality — asserting the exact number would be asserting
        // the bucket layout, which is not what is under test.
        let max = s.max_lateness_us();
        assert!((4_000..4_010).contains(&max), "{max}");
        assert!(s.lateness_us(0.5) < 4_000);
    }

    #[test]
    fn a_paced_run_that_kept_its_rate_reports_no_shortfall_and_an_unpaced_one_reports_none() {
        // The cross-check T088d asks for: the achieved virtual/wallclock ratio and the lateness
        // percentiles are two routes to the same fact, and they must agree.
        let mut on_time = paced(vec![lane(10, 0, 4)], 10.0, &[5]);
        on_time.virtual_span = 10.0;
        on_time.rate = 1.0;
        assert!((on_time.virtual_to_wallclock() - 1.0).abs() < 1e-9);
        assert!(on_time.rate_shortfall().unwrap().abs() < 1e-9);

        // Half the schedule achieved is a 50% shortfall, and it is the same information as the
        // accumulated lateness beside it.
        let mut behind = paced(vec![lane(10, 0, 4)], 20.0, &[5]);
        behind.virtual_span = 10.0;
        behind.rate = 1.0;
        assert!((behind.rate_shortfall().unwrap() - 0.5).abs() < 1e-9);

        // A work-conserving run asked for no rate, so there is no shortfall to report — its
        // virtual/wallclock is a speedup and comparing it with 1.0 would mean nothing.
        assert!(stats(vec![lane(10, 0, 4)], 1.0).rate_shortfall().is_none());
    }

    #[test]
    fn wrong_data_invalidates_a_run_in_either_mode() {
        // The one figure that is a correctness failure rather than a cache outcome, so it cannot
        // depend on which question the run was asking.
        let mismatched = LaneStats {
            counters: Counters {
                payload_mismatches: 1,
                payloads_verified: 100,
                ..Default::default()
            },
            ..lane(100, 0, 9)
        };
        assert!(!stats(vec![mismatched], 1.0).is_valid());
        let paced_run = paced(vec![mismatched], 1.0, &[0]);
        assert!(paced_run.kept_the_schedule(), "the schedule was kept");
        assert!(
            !paced_run.is_valid(),
            "a wrong block must invalidate a paced run that kept its schedule perfectly"
        );
    }

    #[test]
    // The constant value is the point: this is a guard against a later "let's make it bigger",
    // and clippy's suggestion to drop it would remove the guard rather than simplify it.
    #[allow(clippy::assertions_on_constants)]
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
