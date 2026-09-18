//! The driver: route each turn to a lane on the node its session is placed on, and submit it.
//!
//! # There is one driver, and this is it (FR-079)
//!
//! Every node goes through an agent, **including the local one** — which is a node that happens
//! to be `localhost` and is launched as a child process rather than over ssh. So there is one
//! transport, one driver and one cleanup mechanism, and the divergence class FR-072 exists to
//! guard is removed rather than tested for.
//!
//! What used to be the local path is gone: it claimed mailbox channels directly and ran its own
//! `TurnExecutor`. Its behaviour is not gone, though — `tests/loopback.rs` compares what crosses
//! the wire against what that path would have executed, and it was written *before* the path was
//! deleted precisely so the unification could be shown to be inert.
//!
//! The loopback hop this adds is free for the numbers that matter: the agent times its own
//! mailbox requests, so reported per-op latency and bandwidth exclude it. What it costs is the
//! rate at which the generator can *feed* work, and that is what the plan queue's underrun count
//! (FR-062) and paced mode's lateness are for.
//!
//! # A lane is a connection; the node is placement, and placement is not the workload
//!
//! A turn goes to `session.node()`, which the simulation decided at birth and may have changed by
//! migration, and then to `session.id() % lanes` **within** that node. The generator does not
//! choose the node here and must not: routing decides *where* a turn goes, never *what* it is.
//! That is what keeps a description's workload identical across deployments (FR-072), and it is
//! why a migrated session's prefix is cold on its new node — nothing moved the blocks, the turns
//! simply started arriving somewhere else.
//!
//! The within-node term is `session.id() % lanes` rather than round-robin because a session's
//! turns are causally dependent — turn *n+1*'s path holds what turn *n* stored — so a session must
//! stay on one connection for its whole life. It is also exactly what the deleted local path did,
//! which is what makes a one-node run through an agent distribute work as it did before.
//!
//! # Merging across nodes: counters sum, quantiles do not
//!
//! Each lane's `Counters` are added, which is exact. Latency **histograms** are merged and only
//! then asked for percentiles, because the median of two lanes' medians is not a median.
//! Returning percentiles per node and averaging them would produce a figure belonging to no
//! distribution — the failure this project keeps finding, one level up.
//!
//! # Pacing changes when a turn is submitted, and nothing else (FR-080)
//!
//! Under [`Pacing::Real`] a lane holds each turn until its virtual time is due, so the offered
//! load is the workload's own rate rather than as fast as the mailbox will go. The due time is
//! **absolute from `t0`** — `t0 + (virtual timestamp − virtual start) / rate` — and never relative
//! to the previous submission: a slow server would otherwise stretch every think time and dilate
//! the very workload it is being judged on.
//!
//! Pacing lives here, in the one driver, which is why FR-079 was done first: with two drivers it
//! would have been built twice.

use std::collections::BTreeMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::thread;
use std::time::{Duration, Instant};

use hdrhistogram::serialization::Deserializer;
use hdrhistogram::Histogram;
use workload_model::description::WorkloadDescription;
use workload_model::plan::{OpKind, OperationPlan};
use workload_wire::frame::{submit_flags, Counters, SubmitTurn};

use crate::agents::{AgentLane, Agents, NodeLost};
use crate::live::{self, Lane, LaneStats, LiveStats, Pacing, RunOptions, QUEUE_CAPACITY};

/// What a run measured.
#[derive(Debug)]
pub struct DriveStats {
    /// The figures the report renders.
    pub stats: LiveStats,
    /// Per-node counters, kept beside the total because a node carrying none of the load is
    /// worth seeing rather than averaging away.
    pub per_node: Vec<(String, Counters)>,
    /// Migrations the simulation performed, so a run that exercised none is visible.
    pub migrations: u64,
}

/// Why a run did not produce figures.
#[derive(Debug)]
pub enum DriveError {
    /// A node stopped answering, so the whole run is abandoned (FR-064).
    Lost(NodeLost),
    /// Setup was refused **before the timed window opened**, so nothing was issued.
    ///
    /// Kept apart from [`DriveError::Lost`] because the two call for different actions: a lost
    /// node is worth retrying the run for, while a refused setup means the invocation or the
    /// deployment is wrong and rerunning it will fail identically.
    Setup(String),
}

impl std::fmt::Display for DriveError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Lost(e) => write!(f, "{e}"),
            Self::Setup(e) => write!(f, "{e}"),
        }
    }
}

impl std::error::Error for DriveError {}

impl From<NodeLost> for DriveError {
    fn from(e: NodeLost) -> Self {
        Self::Lost(e)
    }
}

/// Clear every node's memory tier, before the timed window opens (FR-046).
///
/// Returns the entries dropped across the cluster. One lane per node asks: the clear is per node,
/// not per connection, and asking on each lane would clear a cache the previous lane had just
/// cleared and report the entries twice.
///
/// # Errors
///
/// [`DriveError::Lost`] if a node cannot be reached, or [`DriveError::Setup`] if a node **answers
/// and could not clear**. The second is a refusal rather than a warning: a run that believed its
/// cache was cold when it was not would report a plausible hit rate for a different experiment.
fn clear_every_node(agents: &mut Agents) -> Result<u64, DriveError> {
    let mut total = 0u64;
    for mut lane in agents.lanes() {
        if lane.lane_index != 0 {
            continue;
        }
        let ack = lane.clear_cache()?;
        if !ack.is_ok() {
            return Err(DriveError::Setup(format!(
                "node {} could not clear its memory tier: {}. Refusing to run: --clear-cache \
                 exists so the run starts from a known cache state, and measuring a cache whose \
                 state is unknown while reporting that it was cleared is worse than not clearing \
                 (FR-046)",
                lane.node, ack.error
            )));
        }
        total += ack.entries;
    }
    Ok(total)
}

/// Drive every lane on every node, routing each turn by its session's placement.
///
/// # Errors
///
/// [`DriveError::Lost`] if any node stops answering: the whole run aborts and the node is named,
/// because continuing on the survivors would measure a different experiment (FR-064).
/// [`DriveError::Setup`] if a pre-run step was refused.
///
/// # Panics
///
/// If `agents` holds no node, or a node holds no lane. Both are refused earlier — a run with
/// nothing to drive is a caller's arithmetic error rather than a condition to report.
pub fn run(
    agents: &mut Agents,
    description: &WorkloadDescription,
    options: &RunOptions,
    stop: Arc<AtomicBool>,
) -> Result<DriveStats, DriveError> {
    let nodes = agents.len();
    let per_node = agents.lanes_per_node();
    let total_lanes = agents.total_lanes();
    assert!(nodes > 0, "a run needs at least one node");
    assert!(per_node > 0, "a node needs at least one lane");

    // Before the clock starts (FR-046): a clear is setup, and timing it would charge the run for
    // work no operation in the stream performed.
    let cleared = if options.clear_cache {
        Some(clear_every_node(agents)?)
    } else {
        None
    };

    let mut senders: Vec<Lane> = Vec::with_capacity(total_lanes);
    let mut receivers = Vec::with_capacity(total_lanes);
    for _ in 0..total_lanes {
        let (lane, rx) = Lane::pair(QUEUE_CAPACITY);
        senders.push(lane);
        receivers.push(rx);
    }

    let mut lanes_out: Vec<LaneStats> = vec![LaneStats::default(); total_lanes];
    let mut lateness: Vec<Histogram<u64>> = Vec::with_capacity(total_lanes);
    let mut lost: Vec<Option<NodeLost>> = vec![None; total_lanes];

    // `t0` is the reference every due time is measured from, and it is taken once for the whole
    // run rather than per lane: a per-lane origin would let a lane that started late keep a
    // schedule of its own and report no lateness for having missed the run's.
    let started = Instant::now();
    let schedule = Schedule {
        t0: started,
        pacing: options.pacing,
        rate: options.rate,
    };

    // Scoped threads rather than a raw-pointer split: each thread takes a distinct `AgentLane`,
    // which borrows one connection, so the compiler establishes what a comment would promise.
    let produced = thread::scope(|scope| {
        let mut handles = Vec::with_capacity(total_lanes);
        for (lane, rx) in agents.lanes().into_iter().zip(receivers) {
            let index = lane.node_index * per_node + lane.lane_index;
            let depth = senders[index].depth();
            let stop = Arc::clone(&stop);
            handles.push((
                index,
                scope.spawn(move || consume(lane, rx, depth, stop, schedule)),
            ));
        }

        // The producer routes by placement: FR-048's node, then a lane within it.
        let produced = live::produce(
            description,
            options.seed,
            options.until,
            nodes,
            &senders,
            |session, n| {
                let node = session.node().min(nodes - 1);
                // Modulo the session id, not round-robin: a session's turns are causally
                // dependent, so it must stay on one connection for its whole life.
                let within = (session.id() as usize) % per_node;
                (node * per_node + within).min(n - 1)
            },
            &stop,
        );
        // Dropping the senders closes each queue, which is how a consumer tells "the run is
        // over" from "the queue is momentarily quiet".
        drop(senders);

        let mut lateness = vec![None; total_lanes];
        for (index, handle) in handles {
            match handle.join() {
                Ok(Ok((stats, late))) => {
                    lanes_out[index] = stats;
                    lateness[index] = Some(late);
                }
                Ok(Err(e)) => lost[index] = Some(e),
                Err(_) => {
                    lost[index] = Some(NodeLost {
                        node: format!("lane {index}"),
                        during: "submitting turns",
                        reason: "the lane thread panicked".into(),
                    })
                }
            }
        }
        (produced, lateness)
    });
    let (produced, per_lane_lateness) = produced;
    for late in per_lane_lateness.into_iter().flatten() {
        lateness.push(late);
    }

    // A lost node aborts the whole run rather than continuing on the survivors (FR-064).
    if let Some(e) = lost.into_iter().flatten().next() {
        return Err(DriveError::Lost(e));
    }
    let produced = produced.map_err(|e| {
        DriveError::Lost(NodeLost {
            node: "the generator".into(),
            during: "building the plan",
            reason: e,
        })
    })?;
    let elapsed = started.elapsed().as_secs_f64();

    // Statistics after the timed window: each lane reports what its own mailbox-facing code
    // measured, which is the only place a `LOOKUP` can be timed or a moved block counted.
    // Indexed by node, **not** keyed by name: two nodes of one run may share a hostname on
    // different ports, which is how a second run on the same host is kept separate, and grouping
    // by name silently merged three nodes into one.
    let mut per_node_counters: Vec<(String, Counters)> =
        vec![(String::new(), Counters::default()); nodes];
    let mut by_op: BTreeMap<u32, Histogram<u64>> = BTreeMap::new();
    let mut combined = new_histogram().map_err(DriveError::Setup)?;
    for mut lane in agents.lanes() {
        let index = lane.node_index * per_node + lane.lane_index;
        let node = lane.node.clone();
        let s = lane.stats()?;
        // Each lane's counters become its own, which is what makes the report's totals right:
        // `LiveStats` sums over lanes, so leaving these unset reported a run that moved nothing.
        lanes_out[index].counters = s.counters;
        let entry = &mut per_node_counters[lane.node_index];
        entry.0.clone_from(&node);
        entry.1.merge(&s.counters);
        let mut de = Deserializer::new();
        for op in &s.ops {
            let Ok(h) = de.deserialize::<u64, _>(&mut std::io::Cursor::new(&op.histogram)) else {
                // A histogram that will not decode costs a latency figure, not a run: the
                // counters beside it are still true, and aborting here would throw away a
                // completed measurement over a reporting detail.
                eprintln!("node {node}: dropping opcode {}'s histogram", op.op_kind);
                continue;
            };
            // Merged, never averaged: quantiles do not combine, distributions do.
            let _ = combined.add(&h);
            match by_op.entry(u32::from(op.op_kind)) {
                std::collections::btree_map::Entry::Occupied(mut e) => {
                    let _ = e.get_mut().add(&h);
                }
                std::collections::btree_map::Entry::Vacant(e) => {
                    e.insert(h);
                }
            }
        }
    }

    // The same rule one level down: a lane's lateness distribution is merged, and the percentiles
    // are taken from the merge.
    let mut all_lateness = new_histogram().map_err(DriveError::Setup)?;
    for h in &lateness {
        let _ = all_lateness.add(h);
    }

    let block_bytes = u64::from(u32::try_from(description.blocks.bytes).unwrap_or(u32::MAX));
    Ok(DriveStats {
        stats: LiveStats {
            lanes: lanes_out,
            batches_produced: produced.batches,
            producer_completed: produced.completed,
            producer_blocked: produced.blocked,
            cleared_entries: cleared,
            block_bytes,
            elapsed,
            virtual_span: produced.virtual_span,
            latency: combined,
            latency_by_op: by_op,
            pacing: options.pacing,
            rate: options.rate,
            lateness: all_lateness,
            lateness_tolerance_us: options.lateness_tolerance_us,
        },
        per_node: per_node_counters,
        // Taken from the simulation rather than left for the caller to fill: it was declared
        // "so a run that exercised none is visible" and then never filled or printed, which is
        // the failure it was meant to prevent, one level up.
        migrations: produced.migrations,
    })
}

/// A latency or lateness histogram: microseconds, up to a minute, three significant digits.
fn new_histogram() -> Result<Histogram<u64>, String> {
    Histogram::<u64>::new_with_bounds(1, 60_000_000, 3)
        .map_err(|e| format!("cannot build a histogram: {e}"))
}

/// When each turn is due, and how far the wallclock is stretched relative to virtual time.
#[derive(Debug, Clone, Copy)]
struct Schedule {
    /// The run's origin. Every due time is absolute from here.
    t0: Instant,
    /// Whether a turn waits for its due time at all.
    pacing: Pacing,
    /// Virtual seconds per wallclock second.
    rate: f64,
}

/// Longest a paced lane sleeps before looking at the stop flag again.
///
/// A paced run can legitimately wait minutes for its next turn, and a run that would not answer
/// SIGINT until then is one an operator cannot stop. Short enough to be prompt, long enough that
/// waiting is not a busy loop.
const PACE_POLL: Duration = Duration::from_millis(50);

impl Schedule {
    /// Wait until `at` (a virtual timestamp) is due, and return how late we were.
    ///
    /// `None` under [`Pacing::None`], where there is no schedule to be late for.
    fn wait_for(&self, at: f64, stop: &AtomicBool) -> Option<Duration> {
        if self.pacing == Pacing::None {
            return None;
        }
        // Absolute from `t0`, never relative to the previous submission: measuring from the last
        // send would let a slow server stretch every think time and dilate the workload it is
        // being judged on.
        let virtual_offset = if at.is_finite() && at > 0.0 { at } else { 0.0 };
        // `rate` is refused at zero or below on the command line; clamped here so a bad caller
        // cannot turn a schedule into an infinity and hang a lane for ever.
        let offset = virtual_offset / self.rate.max(f64::MIN_POSITIVE);
        let due = match Duration::try_from_secs_f64(offset) {
            Ok(d) => self.t0 + d,
            // Not reachable through the command line, which refuses a rate at or below zero.
            // Fallible rather than `from_secs_f64` because that panics, and a panic inside a lane
            // thread is reported as a lost node — a wrong diagnosis for a bad argument.
            Err(_) => self.t0,
        };
        loop {
            let now = Instant::now();
            if now >= due {
                // One-sided by construction: pacing never submits early, so lateness is the whole
                // of the schedule error.
                return Some(now.duration_since(due));
            }
            if stop.load(Ordering::Relaxed) {
                return Some(Duration::ZERO);
            }
            thread::sleep((due - now).min(PACE_POLL));
        }
    }
}

/// What one lane measured: its queue's view and its own schedule error.
type LaneOutcome = (LaneStats, Histogram<u64>);

/// Take turns for one lane and submit them to its node's agent.
fn consume(
    mut lane: AgentLane<'_>,
    rx: std::sync::mpsc::Receiver<OperationPlan>,
    depth: Arc<std::sync::atomic::AtomicUsize>,
    stop: Arc<AtomicBool>,
    schedule: Schedule,
) -> Result<LaneOutcome, NodeLost> {
    let mut mine = LaneStats {
        min_depth: usize::MAX,
        ..LaneStats::default()
    };
    let mut lateness = new_histogram().map_err(|e| NodeLost {
        node: lane.node.clone(),
        during: "preparing to submit",
        reason: e,
    })?;
    let mut path: Vec<u64> = Vec::new();
    // Prime: every queue is empty before the producer has pushed anything, so counting a
    // consumer's first look would invalidate every run — see `live`'s module docs.
    let mut primed = false;

    loop {
        let batch = match rx.try_recv() {
            Ok(b) => b,
            Err(std::sync::mpsc::TryRecvError::Empty) => {
                if primed {
                    mine.underruns += 1;
                    mine.min_depth = 0;
                }
                if stop.load(Ordering::Relaxed) {
                    break;
                }
                match rx.recv() {
                    Ok(b) => b,
                    Err(_) => break,
                }
            }
            Err(std::sync::mpsc::TryRecvError::Disconnected) => break,
        };
        primed = true;
        let observed = depth.fetch_sub(1, Ordering::Relaxed).saturating_sub(1);
        mine.pops += 1;
        mine.min_depth = mine.min_depth.min(observed);

        batch.key_path(&mut path);
        let first = batch.operations().first();
        let session = first.map(|o| o.session()).unwrap_or_default();
        let at = first.map(|o| o.at()).unwrap_or_default();
        let polls = batch
            .operations()
            .iter()
            .any(|o| o.kind() == OpKind::PollEvents);
        // Held until due, if this run is paced. The wait is outside the submit so that what is
        // recorded as lateness is the schedule error and not the server's service time.
        if let Some(late) = schedule.wait_for(at, &stop) {
            let _ = lateness.record(late.as_micros().min(u128::from(u64::MAX)) as u64);
            mine.paced_turns += 1;
        }
        let turn = SubmitTurn {
            session,
            flags: if polls { submit_flags::POLL_EVENTS } else { 0 },
            path: std::mem::take(&mut path),
        };
        // Any transport failure is this node lost, and the run aborts (FR-064).
        lane.submit(&turn)?;
        path = turn.path;
    }
    // Drain whatever is still pipelined before reporting.
    lane.finish()?;
    if mine.min_depth == usize::MAX {
        mine.min_depth = 0;
    }
    Ok((mine, lateness))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_unpaced_run_has_no_schedule_to_be_late_for() {
        let s = Schedule {
            t0: Instant::now(),
            pacing: Pacing::None,
            rate: 1.0,
        };
        let stop = AtomicBool::new(false);
        assert!(
            s.wait_for(1_000_000.0, &stop).is_none(),
            "a work-conserving run must not wait, however far in the future the turn is"
        );
    }

    #[test]
    fn a_due_time_already_past_does_not_wait_and_is_reported_late() {
        // The ordinary case once a run falls behind: the turn goes out immediately and the
        // shortfall is recorded rather than slept off, so lateness accumulates against `t0`
        // instead of being absorbed.
        let s = Schedule {
            t0: Instant::now() - Duration::from_millis(500),
            pacing: Pacing::Real,
            rate: 1.0,
        };
        let stop = AtomicBool::new(false);
        let started = Instant::now();
        let late = s.wait_for(0.1, &stop).expect("paced");
        assert!(
            started.elapsed() < Duration::from_millis(100),
            "a turn whose due time has passed must not sleep"
        );
        // Due 100 ms after a `t0` that is 500 ms old, so about 400 ms late.
        assert!(
            late >= Duration::from_millis(300) && late < Duration::from_millis(600),
            "expected roughly 400 ms of lateness, got {late:?}"
        );
    }

    #[test]
    fn the_rate_divides_the_wait_and_a_turn_at_time_zero_is_due_immediately() {
        let stop = AtomicBool::new(false);
        // Rate 1000: 200 virtual milliseconds are 200 wallclock microseconds, so this returns
        // essentially at once — and it must have *waited* rather than reported lateness.
        let s = Schedule {
            t0: Instant::now(),
            pacing: Pacing::Real,
            rate: 1000.0,
        };
        let late = s.wait_for(0.2, &stop).expect("paced");
        assert!(late < Duration::from_millis(50), "{late:?}");

        // Virtual time zero is `t0` itself.
        let s = Schedule {
            t0: Instant::now(),
            pacing: Pacing::Real,
            rate: 1.0,
        };
        let started = Instant::now();
        let late = s.wait_for(0.0, &stop).expect("paced");
        assert!(started.elapsed() < Duration::from_millis(50));
        assert!(late < Duration::from_millis(50), "{late:?}");
    }

    #[test]
    fn a_stopped_run_does_not_sit_out_the_rest_of_its_schedule() {
        // A paced run can legitimately have nothing due for minutes. An operator pressing
        // Ctrl-C must not have to wait for a turn that is an hour of virtual time away.
        let s = Schedule {
            t0: Instant::now(),
            pacing: Pacing::Real,
            rate: 1.0,
        };
        let stop = AtomicBool::new(true);
        let started = Instant::now();
        assert_eq!(s.wait_for(3_600.0, &stop), Some(Duration::ZERO));
        assert!(
            started.elapsed() < Duration::from_millis(200),
            "took {:?} to notice the stop flag",
            started.elapsed()
        );
    }
}
