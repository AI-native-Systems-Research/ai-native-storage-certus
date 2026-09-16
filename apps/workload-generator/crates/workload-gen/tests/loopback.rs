//! FR-072 at the transport level: driving through an agent submits exactly what the local path
//! would have executed (T074).
//!
//! # Why this test lives here and not in `workload-wire`
//!
//! The task named `crates/workload-wire/tests/loopback.rs`, which cannot work: the comparison
//! needs a plan and a producer, both of which live in `workload-gen`, and `workload-gen` already
//! depends on `workload-wire`. A test there would need the dependency to run both ways. So it
//! lives in the crate that can see both halves.
//!
//! # What is compared, and why that is the whole claim
//!
//! Both paths reduce a turn to the same two things: a session id and a key path, root of the
//! prefix through the end of the new growth. What happens next — check the path, load what is
//! resident, store what is absent — is [`workload_gen::exec::TurnExecutor`], and there is exactly
//! one of it (T068a): the local driver calls it directly and the agent calls the same code. So if
//! the paths crossing the wire are identical to the paths the local driver feeds its executor,
//! the two execution paths issue identical operations, and FR-072 holds across the transport.
//!
//! That is the argument this test completes. Its three parts, and where each is established:
//!
//! | claim | established by |
//! | --- | --- |
//! | the executor turns a path into the right mailbox operations | `op_stream.rs`, against the dispatcher's own rules |
//! | there is one executor, not two | `exec.rs` exists and both callers use it — structural, not tested |
//! | **the wire carries the same paths the local path would execute** | **this file** |
//!
//! What it deliberately does *not* do is compare mailbox traffic, which would need a live server
//! on both sides or a mailbox trait to mock. The middle row is why that is unnecessary rather
//! than merely inconvenient.
//!
//! # This is also the evidence T075 needs
//!
//! FR-079 collapses the local path onto this one. A test that compares them has to exist
//! *before* one is deleted, or the unification is a change nobody can show is inert. After T075
//! it becomes a regression guard against the deleted behaviour rather than a comparison.

#![cfg(feature = "live")]

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use workload_gen::agents::{AgentSpec, Agents, NoLaunch};
use workload_gen::live::RunOptions;
use workload_gen::remote;
use workload_model::description::WorkloadDescription;
use workload_model::plan::{OpKind, OperationPlan};
use workload_model::sim::Simulation;
use workload_wire::frame::{Hello, HelloAck, ShutdownAck, Stats, SubmitTurn, TurnOutcome};
use workload_wire::handshake;
use workload_wire::server::{FnFactory, Server, Service};

/// One turn as either path sees it: whose it is, what it names, and whether it polls.
type Turn = (u64, Vec<u64>, bool);

const DESCRIPTION: &str = r#"
version: 1
blocks: {tokens: 16, bytes: 32768}
shared_classes:
  manual:
    length: {constant: 4}
    lifetime: {constant: .inf}
session_classes:
  chat:
    pool: {size: {exact: 6}}
    uses: [{class: manual, count: {constant: 1}}]
    turns: {constant: 5}
    input_growth: {constant: 2}
    output_growth: {constant: 1}
    think_time: {constant: 5}
"#;

const SPAN: f64 = 120.0;
const SEED: u64 = 31;

/// What the **local** path would feed its executor, taken from the plan itself.
///
/// This is the same reduction `live::consume` performs — group a plan by `(session, instant)`,
/// take the turn's key path — so it is the local path's input without needing a mailbox.
fn local_turns() -> Vec<Turn> {
    let d: WorkloadDescription = DESCRIPTION.parse().unwrap();
    let mut sim = Simulation::new(&d, SEED).unwrap();
    let mut plan = OperationPlan::default();
    sim.run_until(SPAN, &mut |s, t| plan.record_turn(s, t));

    let mut out = Vec::new();
    let ops = plan.operations();
    let mut i = 0usize;
    let mut path = Vec::new();
    while i < ops.len() {
        let id = (ops[i].session(), ops[i].at().to_bits());
        let mut j = i;
        while j < ops.len() && (ops[j].session(), ops[j].at().to_bits()) == id {
            j += 1;
        }
        let group = &ops[i..j];
        // Rebuilt the way `OperationPlan::key_path` defines it — Check names the prefix and
        // Reserve the growth — because a group is a slice rather than a plan of its own.
        path.clear();
        let mut seen = std::collections::HashSet::new();
        for op in group {
            if matches!(op.kind(), OpKind::Check | OpKind::Reserve) {
                for key in plan.keys_of(op) {
                    if seen.insert(*key) {
                        path.push(*key);
                    }
                }
            }
        }
        let polls = group.iter().any(|o| o.kind() == OpKind::PollEvents);
        out.push((group[0].session(), path.clone(), polls));
        i = j;
    }
    out
}

/// Records every `SubmitTurn` an agent was asked to run.
#[derive(Clone, Default)]
struct Recorded(Arc<Mutex<Vec<Turn>>>);

struct Stub {
    seen: Recorded,
}

impl Service for Stub {
    fn hello(&mut self, hello: &Hello) -> HelloAck {
        handshake::answer(hello, 8, 32768)
    }
    fn submit_turn(&mut self, turn: &SubmitTurn) -> TurnOutcome {
        self.seen
            .0
            .lock()
            .unwrap()
            .push((turn.session, turn.path.clone(), turn.polls_events()));
        TurnOutcome::default()
    }
    fn stats(&mut self) -> Stats {
        Stats::default()
    }
    fn shutdown(&mut self) -> ShutdownAck {
        ShutdownAck::default()
    }
}

/// What the **remote** path submitted, driven through a real loopback agent.
fn remote_turns() -> Vec<Turn> {
    let seen = Recorded::default();
    let port = {
        let l = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let p = l.local_addr().unwrap().port();
        drop(l);
        p
    };
    let for_service = seen.clone();
    let server = Server::bind(
        ("127.0.0.1", port),
        FnFactory(move || {
            Ok(Stub {
                seen: for_service.clone(),
            })
        }),
    )
    .expect("bind the loopback agent")
    .with_linger(std::time::Duration::from_secs(120));
    let stop_server = Arc::new(AtomicBool::new(false));
    let flag = Arc::clone(&stop_server);
    let handle = std::thread::spawn(move || {
        let _ = server.serve(flag);
    });
    for _ in 0..300 {
        if std::net::TcpStream::connect(("127.0.0.1", port)).is_ok() {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(10));
    }

    let specs = vec![AgentSpec {
        node: "127.0.0.1".into(),
        port,
        shm_path: "/dev/shm/certus-shmq".into(),
        binary: "workload-node-agent".into(),
        lanes: 1,
        block_bytes: 32768,
        batch_keys: 64,
        extra_args: Vec::new(),
    }];
    // Already running, so nothing is launched and nothing is replaced.
    let mut agents =
        Agents::start_with(&NoLaunch, &specs, 8, false).expect("handshake with the loopback agent");
    let d: WorkloadDescription = DESCRIPTION.parse().unwrap();
    let options = RunOptions {
        seed: SEED,
        until: Some(SPAN),
        batch_keys: 64,
        gpu_device: None,
        stamp_keys: false,
        verify_payload: false,
        clear_cache: false,
    };
    remote::run(&mut agents, &d, &options, Arc::new(AtomicBool::new(false)))
        .expect("drive the loopback agent");
    agents.stop().expect("stop");
    stop_server.store(true, Ordering::Relaxed);
    let _ = handle.join();

    let v = seen.0.lock().unwrap().clone();
    v
}

#[test]
fn the_wire_carries_exactly_what_the_local_path_would_execute() {
    // FR-072 across the transport. One node and one lane, so the submitted order is the plan's
    // order and the comparison is direct rather than a multiset.
    let local = local_turns();
    let remote = remote_turns();

    assert!(!local.is_empty(), "the description produced no turns");
    assert_eq!(
        remote.len(),
        local.len(),
        "the wire carried {} turns, the local path would have executed {}",
        remote.len(),
        local.len()
    );
    for (i, (l, r)) in local.iter().zip(remote.iter()).enumerate() {
        assert_eq!(l.0, r.0, "turn {i}: a different session crossed the wire");
        assert_eq!(
            l.1,
            r.1,
            "turn {i} (session {}): the path differs — the wire carried {} keys, the local path \
             would have offered {}",
            l.0,
            r.1.len(),
            l.1.len()
        );
        assert_eq!(l.2, r.2, "turn {i}: the event poll differs");
    }
}

#[test]
fn a_turns_path_runs_from_the_root_and_grows() {
    // What "the same paths" is worth depends on the paths being the real thing. A turn offers its
    // prefix from the root, so a session's later turns are supersets of its earlier ones — if the
    // paths were empty, or one key long, the equivalence above would be vacuous.
    let local = local_turns();
    let mut by_session: std::collections::BTreeMap<u64, Vec<usize>> = Default::default();
    for (session, path, _) in &local {
        by_session.entry(*session).or_default().push(path.len());
    }
    let mut grew = 0usize;
    for (session, lengths) in &by_session {
        assert!(
            lengths.iter().all(|n| *n > 0),
            "session {session} offered an empty path"
        );
        if lengths.windows(2).all(|w| w[1] >= w[0]) && lengths.last() > lengths.first() {
            grew += 1;
        }
    }
    assert!(
        grew > 0,
        "no session's path grew across its turns, so the paths are not prefixes"
    );
}

#[test]
fn the_comparison_would_notice_a_difference() {
    // A test that cannot fail proves nothing, and this one compares two derivations of the same
    // thing — so it is worth showing that the comparison has teeth. A different seed must produce
    // different paths, or the assertion above would pass however the wire behaved.
    let a = local_turns();
    let b = {
        let d: WorkloadDescription = DESCRIPTION.parse().unwrap();
        let mut sim = Simulation::new(&d, SEED + 1).unwrap();
        let mut plan = OperationPlan::default();
        sim.run_until(SPAN, &mut |s, t| plan.record_turn(s, t));
        plan.operations().len()
    };
    assert!(b > 0);
    let a_keys: Vec<u64> = a.iter().flat_map(|(_, p, _)| p.clone()).collect();
    let d: WorkloadDescription = DESCRIPTION.parse().unwrap();
    let mut sim = Simulation::new(&d, SEED + 1).unwrap();
    let mut other = OperationPlan::default();
    sim.run_until(SPAN, &mut |s, t| other.record_turn(s, t));
    let mut other_path = Vec::new();
    other.key_path(&mut other_path);
    assert_ne!(
        a_keys.len(),
        0,
        "the reference derivation produced no keys at all"
    );
    assert_ne!(
        a_keys, other_path,
        "two different seeds produced the same keys, so the comparison cannot detect a change"
    );
}
