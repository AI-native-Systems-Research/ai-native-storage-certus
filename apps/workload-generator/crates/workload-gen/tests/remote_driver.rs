//! The remote driver (T073a): routing turns to the node each session is placed on.
//!
//! Driven against stand-in agents on loopback ports, so the routing, the merging and the
//! node-loss abort are all exercised without a cluster. What a stand-in cannot show is that a
//! turn reaches a cache — `workload-node-agent`'s own `live_agent` test does that.

#![cfg(feature = "live")]

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use hdrhistogram::serialization::{Serializer, V2Serializer};
use workload_gen::agents::{AgentSpec, Agents, Launcher};
use workload_gen::drive::{self, DriveError};
use workload_gen::live::{Pacing, RunOptions};
use workload_model::description::WorkloadDescription;
use workload_wire::frame::{
    Counters, Hello, HelloAck, OpHistogram, ShutdownAck, Stats, SubmitTurn, TurnOutcome,
};
use workload_wire::handshake;
use workload_wire::server::{FnFactory, Server, Service};

/// Records every session it was asked about, so routing can be checked.
#[derive(Clone, Default)]
struct Seen(Arc<Mutex<Vec<u64>>>);

struct Stub {
    seen: Seen,
}

impl Service for Stub {
    fn hello(&mut self, hello: &Hello) -> HelloAck {
        handshake::answer(hello, 8, 32768)
    }
    fn submit_turn(&mut self, turn: &SubmitTurn) -> TurnOutcome {
        self.seen.0.lock().unwrap().push(turn.session);
        TurnOutcome {
            resident: turn.path.len() as u32,
            blocks_read: turn.path.len() as u32,
            ..Default::default()
        }
    }
    fn stats(&mut self) -> Stats {
        // A histogram with known contents, so the merge can be checked rather than trusted.
        let mut h = hdrhistogram::Histogram::<u64>::new_with_bounds(1, 60_000_000, 3).unwrap();
        for v in [10u64, 20, 30] {
            h.record(v).unwrap();
        }
        let mut bytes = Vec::new();
        V2Serializer::new().serialize(&h, &mut bytes).unwrap();
        Stats {
            counters: Counters {
                requests: 4,
                lookup_hits: 5,
                transfers_attempted: 2,
                ..Default::default()
            },
            ops: vec![OpHistogram {
                op_kind: 1,
                requests: 3,
                histogram: bytes,
            }],
        }
    }
    fn shutdown(&mut self) -> ShutdownAck {
        ShutdownAck::default()
    }
}

/// What each stand-in agent recorded, keyed by the port it served on.
type Observed = Arc<Mutex<Vec<(u16, Seen)>>>;

/// Stop flags for the stand-in agents this launcher started, keyed by port.
type Running = Arc<Mutex<Vec<(u16, Arc<AtomicBool>)>>>;

#[derive(Clone)]
struct LocalLauncher {
    seen: Observed,
    running: Running,
}

impl LocalLauncher {
    fn new() -> Self {
        Self {
            seen: Arc::new(Mutex::new(Vec::new())),
            running: Arc::new(Mutex::new(Vec::new())),
        }
    }
    fn sessions_on(&self, port: u16) -> Vec<u64> {
        for (p, s) in self.seen.lock().unwrap().iter() {
            if *p == port {
                return s.0.lock().unwrap().clone();
            }
        }
        Vec::new()
    }
}

impl Launcher for LocalLauncher {
    fn launch(&self, spec: &AgentSpec) -> Result<(), String> {
        let seen = Seen::default();
        self.seen.lock().unwrap().push((spec.port, seen.clone()));
        let for_service = seen.clone();
        let server = Server::bind(
            ("127.0.0.1", spec.port),
            FnFactory(move || {
                Ok(Stub {
                    seen: for_service.clone(),
                })
            }),
        )
        .map_err(|e| e.to_string())?
        .with_linger(std::time::Duration::from_secs(120));
        let stop = Arc::new(AtomicBool::new(false));
        let flag = Arc::clone(&stop);
        std::thread::spawn(move || {
            let _ = server.serve(flag);
        });
        for _ in 0..300 {
            if std::net::TcpStream::connect(("127.0.0.1", spec.port)).is_ok() {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        self.running.lock().unwrap().push((spec.port, stop));
        Ok(())
    }
    fn kill(&self, spec: &AgentSpec) -> Result<(), String> {
        for (port, stop) in self.running.lock().unwrap().iter() {
            if *port == spec.port {
                stop.store(true, Ordering::Relaxed);
            }
        }
        Ok(())
    }
}

fn free_port() -> u16 {
    let l = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let p = l.local_addr().unwrap().port();
    drop(l);
    p
}

fn spec(port: u16) -> AgentSpec {
    AgentSpec {
        node: "127.0.0.1".into(),
        port,
        shm_path: "/dev/shm/certus-shmq".into(),
        binary: "workload-node-agent".into(),
        lanes: 1,
        block_bytes: 32768,
        batch_keys: 64,
        extra_args: Vec::new(),
    }
}

const DESCRIPTION: &str = r#"
version: 1
blocks: {tokens: 16, bytes: 32768}
shared_classes:
  manual:
    length: {constant: 4}
    lifetime: {constant: .inf}
session_classes:
  chat:
    pool: {size: {exact: 10}}
    uses: [{class: manual, count: {constant: 1}}]
    turns: {constant: 4}
    input_growth: {constant: 2}
    output_growth: {constant: 1}
    think_time: {constant: 5}
"#;

/// The same shape as `DESCRIPTION`, with a migration interval short relative to a session's
/// life, so a run over 60 virtual seconds must perform migrations rather than merely be able to.
const MIGRATING: &str = r#"
version: 1
blocks: {tokens: 16, bytes: 32768}
shared_classes:
  manual:
    length: {constant: 4}
    lifetime: {constant: .inf}
session_classes:
  chat:
    pool: {size: {exact: 10}}
    uses: [{class: manual, count: {constant: 1}}]
    turns: {constant: 8}
    input_growth: {constant: 2}
    output_growth: {constant: 1}
    think_time: {constant: 1}
    migration_interval: {constant: 1.0}
"#;

fn options() -> RunOptions {
    RunOptions {
        seed: 21,
        until: Some(120.0),
        batch_keys: 64,
        clear_cache: false,
        // Work-conserving, so these fixtures assert what a turn stream is rather than when it is
        // submitted. Pacing has its own tests, and holding every turn for its virtual time would
        // make a 120-virtual-second fixture take two wallclock minutes.
        pacing: Pacing::None,
        rate: 1.0,
        lateness_tolerance_us: 0,
    }
}

#[test]
fn every_turn_goes_to_the_node_its_session_is_placed_on() {
    // The routing property, and the reason it matters: a turn sent to the wrong node would find
    // a cold prefix and store it, which looks exactly like a migration that never happened.
    let launcher = LocalLauncher::new();
    let ports: Vec<u16> = (0..3).map(|_| free_port()).collect();
    let specs: Vec<AgentSpec> = ports.iter().copied().map(spec).collect();
    let mut agents = Agents::start_default(&launcher, &specs).expect("start");

    let d: WorkloadDescription = DESCRIPTION.parse().unwrap();
    let stop = Arc::new(AtomicBool::new(false));
    let out = drive::run(&mut agents, &d, &options(), Arc::clone(&stop)).expect("drive");

    // Every node saw traffic, and no session appeared on two nodes at once — with no migration
    // interval a session stays where it was placed, so a session on two nodes means routing is
    // not following placement.
    let mut owner: std::collections::BTreeMap<u64, u16> = std::collections::BTreeMap::new();
    let mut total = 0usize;
    for port in &ports {
        let sessions = launcher.sessions_on(*port);
        assert!(!sessions.is_empty(), "node on {port} received nothing");
        total += sessions.len();
        for s in sessions {
            if let Some(prev) = owner.insert(s, *port) {
                assert_eq!(prev, *port, "session {s} was routed to two different nodes");
            }
        }
    }
    assert!(total > 20, "only {total} turns were submitted");
    assert_eq!(
        out.stats.lanes.len(),
        3,
        "one lane per node, so a routing imbalance is visible"
    );
    agents.stop().expect("stop");
}

#[test]
fn counters_sum_and_histograms_merge_across_nodes() {
    // Counters add exactly; quantiles do not, so the distributions are merged and the
    // percentiles taken afterwards. Averaging three nodes' p99s would give a number belonging to
    // no distribution.
    let launcher = LocalLauncher::new();
    let specs: Vec<AgentSpec> = (0..3).map(|_| spec(free_port())).collect();
    let mut agents = Agents::start_default(&launcher, &specs).expect("start");
    let d: WorkloadDescription = DESCRIPTION.parse().unwrap();
    let out = drive::run(
        &mut agents,
        &d,
        &options(),
        Arc::new(AtomicBool::new(false)),
    )
    .expect("drive");

    assert_eq!(out.per_node.len(), 3, "per-node counters must be kept");
    // Each stub reports the same fixed counters, so the total is exactly three times one.
    let total: u64 = out.per_node.iter().map(|(_, c)| c.lookup_hits).sum();
    assert_eq!(total, 15, "three nodes at five hits each");
    assert_eq!(out.stats.lookup_hits(), 15, "the merged total must agree");

    // Three nodes each recorded 3 samples, so the merged histogram holds 9 — not 3, which is
    // what taking one node's would give, and not an average.
    let merged = out
        .stats
        .latency_by_op
        .get(&1)
        .expect("opcode 1's histogram");
    assert_eq!(merged.len(), 9, "histograms must merge, not replace");
    assert_eq!(merged.min(), 10);
    assert_eq!(merged.max(), 30);
    agents.stop().expect("stop");
}

#[test]
fn losing_a_node_mid_run_aborts_and_names_it() {
    // FR-064. The run must not finish on the survivors: their latency would reflect a
    // concurrency nobody asked for, and the report would look ordinary.
    let launcher = LocalLauncher::new();
    let ports: Vec<u16> = (0..2).map(|_| free_port()).collect();
    let specs: Vec<AgentSpec> = ports.iter().copied().map(spec).collect();
    let mut agents = Agents::start_default(&launcher, &specs).expect("start");

    // Take one node down while the run is being driven.
    let l = launcher.clone();
    let victim = specs[1].clone();
    std::thread::spawn(move || {
        std::thread::sleep(std::time::Duration::from_millis(30));
        let _ = l.kill(&victim);
    });

    let d: WorkloadDescription = DESCRIPTION.parse().unwrap();
    let long = RunOptions {
        until: Some(100_000.0),
        ..options()
    };
    let result = drive::run(&mut agents, &d, &long, Arc::new(AtomicBool::new(false)));
    match result {
        // A node taken down mid-run is `Lost`, not `Setup`: the distinction is what tells a sweep
        // driver to retry the run rather than to go and fix the invocation.
        Err(DriveError::Lost(lost)) => {
            // Named by instance, not by host (FR-081). Both instances of this test are on
            // 127.0.0.1, so "the host" would not say which one died — and this test is the shape
            // the defect hid in: several instances of one run sharing a machine.
            assert_eq!(lost.node, format!("127.0.0.1:{}", ports[1]));
            let text = lost.to_string();
            assert!(text.contains("FR-064"), "{text}");
        }
        Err(DriveError::Setup(e)) => panic!("a lost node was reported as a setup failure: {e}"),
        Ok(_) => panic!("the run completed although a node was taken down"),
    }
}

#[test]
fn migrations_reach_the_report_rather_than_staying_at_zero() {
    // `DriveStats::migrations` existed, was documented as being there "so a run that exercised
    // none is visible", and was never filled in or printed — `drive::run` hard-coded 0 and left a
    // comment saying the caller would fill it. Nothing did. Found by running T101 on hardware and
    // looking for the count the check depends on.
    //
    // Two instances and a migration interval short relative to session lifetime, so the
    // simulation must perform some; the assertion is that a non-zero count *arrives*, not what it
    // equals, which is the simulation's business and is tested in `workload-model`.
    let launcher = LocalLauncher::new();
    let specs: Vec<AgentSpec> = (0..2).map(|_| spec(free_port())).collect();
    let mut agents = Agents::start_default(&launcher, &specs).expect("start");

    let d: WorkloadDescription = MIGRATING.parse().expect("a migrating description");
    let out = drive::run(
        &mut agents,
        &d,
        &RunOptions {
            until: Some(60.0),
            ..options()
        },
        Arc::new(AtomicBool::new(false)),
    )
    .expect("drive");
    agents.stop().expect("stop");

    assert!(
        out.migrations > 0,
        "two instances and a 1s migration interval over 60 virtual seconds performed no \
         migrations, so either the simulation did not migrate or the count is not reaching the \
         report — the defect this test exists for"
    );
}
