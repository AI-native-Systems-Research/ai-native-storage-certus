//! Driving a cluster: route each turn to the node its session is placed on.
//!
//! # This is the driver, not a second one
//!
//! FR-079 makes the local path a special case of this one — a node that happens to be
//! `localhost` — so this is written in the shape both will use rather than as a remote
//! counterpart to `live::run`. Two drivers would be free to diverge, which is the whole reason
//! FR-072a's *rule* was given one implementation (`crate::exec`) and the producer one
//! implementation (`live::produce`, routed by a closure).
//!
//! What differs from the local path is only where a turn is sent:
//!
//! | | local | remote |
//! | --- | --- | --- |
//! | target | a lane on this node's mailbox | a lane on some node's agent |
//! | routing | session id modulo lanes | **the node the session is placed on** (FR-048), then a lane within it |
//! | who runs the rule | the generator's own `TurnExecutor` | the agent's, over the same code |
//! | who measures | the mailbox-facing code | the same, reported back by `Stats` |
//!
//! # Routing is placement, and placement is not the workload
//!
//! A turn goes to `session.node()`, which the simulation decided at birth and may have changed
//! by migration. The generator does not choose it here and must not: routing decides *where* a
//! turn goes, never *what* it is. That is what keeps a description's workload identical across
//! deployments (FR-072), and it is why a migrated session's prefix is cold on its new node —
//! nothing moved the blocks, the turns simply started arriving somewhere else.
//!
//! # Merging across nodes: counters sum, quantiles do not
//!
//! Each node's `Counters` are added, which is exact. Its latency **histograms** are merged and
//! only then asked for percentiles, because the median of two nodes' medians is not a median.
//! Returning percentiles per node and averaging them would produce a figure belonging to no
//! distribution — the failure this project keeps finding, one level up.

use std::collections::BTreeMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::thread;
use std::time::Instant;

use hdrhistogram::serialization::Deserializer;
use hdrhistogram::Histogram;
use workload_model::description::WorkloadDescription;
use workload_model::plan::{OpKind, OperationPlan};
use workload_wire::frame::{submit_flags, Counters, SubmitTurn};

use crate::agents::{Agents, NodeLost};
use crate::live::{self, Lane, LaneStats, LiveStats, RunOptions, QUEUE_CAPACITY};

/// What a cluster run measured.
#[derive(Debug)]
pub struct RemoteStats {
    /// The same shape a local run reports, so one report renders either.
    pub stats: LiveStats,
    /// Per-node counters, kept beside the total because a node carrying none of the load is
    /// worth seeing rather than averaging away.
    pub per_node: Vec<(String, Counters)>,
    /// Migrations the simulation performed, so a run that exercised none is visible.
    pub migrations: u64,
}

/// Drive every node, routing each turn to the node its session is placed on.
///
/// # Errors
///
/// [`NodeLost`] if any node stops answering: the whole run aborts and the node is named, because
/// continuing on the survivors would measure a different experiment (FR-064).
pub fn run(
    agents: &mut Agents,
    description: &WorkloadDescription,
    options: &RunOptions,
    stop: Arc<AtomicBool>,
) -> Result<RemoteStats, NodeLost> {
    let nodes = agents.len();
    assert!(nodes > 0, "a cluster run needs at least one node");

    // One lane per node for now: a lane is a connection, and `Agents` opens one per node. More
    // lanes per node means more connections, which the agent already supports — it claims a
    // mailbox channel per connection — and is a change to `Agents` rather than to this routing.
    let mut senders: Vec<Lane> = Vec::with_capacity(nodes);
    let mut receivers = Vec::with_capacity(nodes);
    for _ in 0..nodes {
        let (lane, rx) = Lane::pair(QUEUE_CAPACITY);
        senders.push(lane);
        receivers.push(rx);
    }

    let started = Instant::now();
    let mut lanes_out: Vec<LaneStats> = vec![LaneStats::default(); nodes];
    let mut lost: Vec<Option<NodeLost>> = vec![None; nodes];

    // Scoped threads rather than a raw-pointer split: `iter_mut` hands each thread a distinct
    // `&mut Agent`, so the compiler establishes what a comment would otherwise have to promise.
    let produced = thread::scope(|scope| {
        let mut handles = Vec::with_capacity(nodes);
        for ((index, agent), rx) in agents.agents().iter_mut().enumerate().zip(receivers) {
            let depth = senders[index].depth();
            let stop = Arc::clone(&stop);
            let node = agent.node.clone();
            handles.push(scope.spawn(move || consume(node, agent, rx, depth, stop)));
        }

        // The producer routes by placement: FR-048's node, then a lane within it. With one lane
        // per node the second term is inert, and it is written out so adding lanes is one line.
        let produced = live::produce(
            description,
            options.seed,
            options.until,
            nodes,
            &senders,
            |session, n| session.node().min(n - 1),
            &stop,
        );
        // Dropping the senders closes each queue, which is how a consumer tells "the run is
        // over" from "the queue is momentarily quiet".
        drop(senders);

        for (index, handle) in handles.into_iter().enumerate() {
            match handle.join() {
                Ok(Ok(stats)) => lanes_out[index] = stats,
                Ok(Err(e)) => lost[index] = Some(e),
                Err(_) => {
                    lost[index] = Some(NodeLost {
                        node: names_of(index),
                        during: "submitting turns",
                        reason: "the lane thread panicked".into(),
                    })
                }
            }
        }
        produced
    });

    // A lost node aborts the whole run rather than continuing on the survivors (FR-064).
    if let Some(e) = lost.into_iter().flatten().next() {
        return Err(e);
    }
    let (batches, virtual_span, completed, blocked) = produced.map_err(|e| NodeLost {
        node: "the generator".into(),
        during: "building the plan",
        reason: e,
    })?;
    let elapsed = started.elapsed().as_secs_f64();

    // Statistics after the timed window: each node reports what its own mailbox-facing code
    // measured, which is the only place a `LOOKUP` can be timed or a moved block counted.
    let mut per_node = Vec::with_capacity(nodes);
    let mut by_op: BTreeMap<u32, Histogram<u64>> = BTreeMap::new();
    let mut combined =
        Histogram::<u64>::new_with_bounds(1, 60_000_000, 3).map_err(|e| NodeLost {
            node: "the generator".into(),
            during: "building the latency histogram",
            reason: e.to_string(),
        })?;
    for (index, agent) in agents.agents().iter_mut().enumerate() {
        let node = agent.node.clone();
        let s = agent.stats()?;
        // Each node's counters become its lane's, which is what makes the report's totals right:
        // `LiveStats` sums over lanes, so leaving these unset reported a run that moved nothing.
        lanes_out[index].counters = s.counters;
        per_node.push((node.clone(), s.counters));
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

    let block_bytes = u64::from(u32::try_from(description.blocks.bytes).unwrap_or(u32::MAX));
    Ok(RemoteStats {
        stats: LiveStats {
            lanes: lanes_out,
            batches_produced: batches,
            producer_completed: completed,
            producer_blocked: blocked,
            cleared_entries: None,
            block_bytes,
            elapsed,
            virtual_span,
            latency: combined,
            latency_by_op: by_op,
        },
        per_node,
        // Filled by the caller, which owns the simulation's report.
        migrations: 0,
    })
}

/// Take turns for one node and submit them to its agent.
fn consume(
    node: String,
    agent: &mut crate::agents::Agent,
    rx: std::sync::mpsc::Receiver<OperationPlan>,
    depth: Arc<std::sync::atomic::AtomicUsize>,
    stop: Arc<AtomicBool>,
) -> Result<LaneStats, NodeLost> {
    let mut mine = LaneStats {
        min_depth: usize::MAX,
        ..LaneStats::default()
    };
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
        let session = batch
            .operations()
            .first()
            .map(|o| o.session())
            .unwrap_or_default();
        let polls = batch
            .operations()
            .iter()
            .any(|o| o.kind() == OpKind::PollEvents);
        let turn = SubmitTurn {
            session,
            flags: if polls { submit_flags::POLL_EVENTS } else { 0 },
            path: std::mem::take(&mut path),
        };
        // Any transport failure is this node lost, and the run aborts (FR-064).
        agent.submit(&turn)?;
        path = turn.path;
    }
    // Drain whatever is still pipelined before reporting.
    agent.finish()?;
    if mine.min_depth == usize::MAX {
        mine.min_depth = 0;
    }
    let _ = node;
    Ok(mine)
}

/// A placeholder name for a panicking lane, whose agent cannot be consulted.
fn names_of(index: usize) -> String {
    format!("lane {index}")
}
