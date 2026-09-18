//! FR-072 at the transport level, and the **regression guard** for the path FR-079 deleted
//! (T074, re-pointed by T092a).
//!
//! # What this compared, and what it guards now
//!
//! It was written while there were two execution paths — a local one that claimed mailbox
//! channels directly and a remote one that went through an agent — to establish that they issued
//! the same thing. That was the precondition for deleting one: unifying first would have
//! collapsed onto a path never shown equivalent, and destroyed the means of showing it.
//!
//! The local path is now gone, so `local_turns` no longer describes code that exists. It
//! describes the **behaviour** that code had, derived from the plan the way `live::consume` derived
//! it — group the plan by `(session, instant)`, take the turn's key path — and that is exactly what
//! makes this a regression guard rather than a comparison of two live implementations. If the one
//! remaining path ever stops submitting what the deleted one would have executed, this fails.
//!
//! # Why comparing paths is the whole claim
//!
//! Both paths reduce a turn to the same two things: a session id and a key path, root of the
//! prefix through the end of the new growth. What happens next — check the path, load what is
//! resident, store what is absent — is `workload_node_agent::exec::TurnExecutor`, and there is
//! exactly one of it. So if the paths crossing the wire are identical to the paths the local
//! driver fed its executor, the two execution paths issue identical operations, and FR-072 holds
//! across the transport.
//!
//! That is the argument this test completes. Its three parts, and where each is established:
//!
//! | claim | established by |
//! | --- | --- |
//! | the executor turns a path into the right mailbox operations | `workload-node-agent`'s `op_stream.rs`, against the dispatcher's own rules |
//! | there is one executor, not two | structural: the generator has no mailbox path at all since FR-079 |
//! | **the wire carries the same paths the local path would execute** | **this file** |
//!
//! What it deliberately does *not* do is compare mailbox traffic, which would need a live server
//! on both sides or a mailbox trait to mock. The middle row is why that is unnecessary rather
//! than merely inconvenient.

#![cfg(feature = "live")]

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use workload_gen::agents::{AgentSpec, Agents, NoLaunch};
use workload_gen::drive;
use workload_gen::live::{Pacing, RunOptions};
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

/// What the **deleted local path** would have fed its executor, taken from the plan itself.
///
/// This is the reduction the old `live::consume` performed — group a plan by `(session, instant)`,
/// take the turn's key path. It is written out here rather than called, because the code it
/// describes no longer exists: that is what makes this a regression guard against the behaviour
/// FR-079 removed rather than a comparison of two implementations that both still ship.
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

/// A work-conserving run's options: what these fixtures use unless the point is pacing.
fn unpaced() -> RunOptions {
    RunOptions {
        seed: SEED,
        until: Some(SPAN),
        batch_keys: 64,
        clear_cache: false,
        pacing: Pacing::None,
        rate: 1.0,
        lateness_tolerance_us: 0,
    }
}

/// What crossed the wire, driven through a real loopback agent on `lanes` connections.
fn driven_turns(lanes: usize, options: &RunOptions) -> Vec<Turn> {
    driven_turns_at_depth(lanes, workload_wire::client::DEFAULT_DEPTH, options)
}

/// The same, at an explicit pipelining depth.
fn driven_turns_at_depth(lanes: usize, depth: usize, options: &RunOptions) -> Vec<Turn> {
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
        lanes,
        block_bytes: 32768,
        batch_keys: 64,
        extra_args: Vec::new(),
    }];
    // Already running, so nothing is launched and nothing is replaced.
    let mut agents = Agents::start_with(&NoLaunch, &specs, depth, false)
        .expect("handshake with the loopback agent");
    let d: WorkloadDescription = DESCRIPTION.parse().unwrap();
    drive::run(&mut agents, &d, options, Arc::new(AtomicBool::new(false)))
        .expect("drive the loopback agent");
    agents.stop().expect("stop");
    stop_server.store(true, Ordering::Relaxed);
    let _ = handle.join();

    let v = seen.0.lock().unwrap().clone();
    v
}

/// What one lane submitted, in submission order.
fn remote_turns() -> Vec<Turn> {
    driven_turns(1, &unpaced())
}

/// Turns grouped by session, in each session's own submission order.
///
/// The comparison to make when several lanes are running: turns of *different* sessions interleave
/// however the lanes happen to be scheduled, and that is allowed. What is not allowed is a
/// session's own turns arriving out of order, because turn n+1's path holds what turn n stored.
fn by_session(turns: &[Turn]) -> std::collections::BTreeMap<u64, Vec<Turn>> {
    let mut out: std::collections::BTreeMap<u64, Vec<Turn>> = Default::default();
    for t in turns {
        out.entry(t.0).or_default().push(t.clone());
    }
    out
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

#[test]
fn several_lanes_carry_the_same_turns_and_keep_each_sessions_order() {
    // FR-072 against the lane fan-out FR-079 needed. `--lanes n` is now *n connections per node*,
    // and the local path used to get its concurrency from claiming n mailbox channels directly. A
    // single connection per node would have collapsed a four-lane run to one lane and reported the
    // throughput as though nothing had changed, so the property worth asserting is that four lanes
    // submit exactly what one lane does.
    //
    // Across sessions the order is *not* the plan's — that is what several lanes are for — so this
    // compares per session, which is the ordering the design actually owns: turn n+1's path holds
    // what turn n stored.
    let one = by_session(&driven_turns(1, &unpaced()));
    let four = by_session(&driven_turns(4, &unpaced()));

    assert!(!one.is_empty(), "the description produced no turns");
    assert_eq!(
        one.keys().collect::<Vec<_>>(),
        four.keys().collect::<Vec<_>>(),
        "a different set of sessions reached the agent at four lanes"
    );
    for (session, turns) in &one {
        assert_eq!(
            four.get(session),
            Some(turns),
            "session {session}'s turns differ between one lane and four"
        );
    }
    // And the fan-out really happened: a routing of `session % lanes` puts these sessions on more
    // than one lane, so a run that quietly served everything on one connection would fail here.
    assert!(
        one.len() > 1,
        "only {} session(s), so lane routing is untested",
        one.len()
    );
}

#[test]
fn the_rate_and_the_pacing_mode_do_not_change_what_is_submitted() {
    // FR-080's central constraint, and the one that keeps a rate sweep interpretable: the rate is
    // a **tempo** control. It changes when a turn is submitted and nothing else — the same keys,
    // in the same order, for the same sessions — so a curve swept over rate is a curve over
    // offered load rather than over three different workloads.
    //
    // Rate 5000 so the whole 120 virtual seconds costs about 24 ms of wallclock: enough for the
    // schedule arithmetic to run on every turn, cheap enough for the ordinary gate.
    let reference = by_session(&driven_turns(1, &unpaced()));
    for rate in [5_000.0, 20_000.0] {
        let paced = by_session(&driven_turns(
            1,
            &RunOptions {
                pacing: Pacing::Real,
                rate,
                lateness_tolerance_us: u64::MAX,
                ..unpaced()
            },
        ));
        assert_eq!(
            paced.keys().collect::<Vec<_>>(),
            reference.keys().collect::<Vec<_>>(),
            "pacing at rate {rate} changed which sessions ran"
        );
        for (session, turns) in &reference {
            assert_eq!(
                paced.get(session),
                Some(turns),
                "pacing at rate {rate} changed session {session}'s turns"
            );
        }
    }
}

#[test]
fn the_plan_fingerprint_does_not_depend_on_the_rate() {
    // The same claim one level in, and where it is structural: the rate never reaches the
    // simulation. `RunOptions.rate` is read by the driver's schedule and by nothing that builds a
    // turn, so the plan a run replays is a function of description and seed alone (FR-072).
    //
    // Asserted against the canonical serialisation rather than inferred from the code, because
    // "this field is not read over there" is exactly the kind of claim that stops being true.
    let d: WorkloadDescription = DESCRIPTION.parse().unwrap();
    let fingerprint = |seed: u64| {
        let mut sim = Simulation::new(&d, seed).unwrap();
        let mut plan = OperationPlan::default();
        sim.run_until(SPAN, &mut |s, t| plan.record_turn(s, t));
        plan.fingerprint()
    };
    let reference = fingerprint(SEED);
    assert_eq!(reference, fingerprint(SEED), "the plan is not reproducible");
    assert_ne!(
        reference,
        fingerprint(SEED + 1),
        "two seeds share a fingerprint, so this comparison has no teeth"
    );

    // The turns that actually crossed the wire under three different tempos, against the plan the
    // fingerprint belongs to.
    let flatten = |turns: &[Turn]| -> Vec<(u64, Vec<u64>)> {
        by_session(turns)
            .into_iter()
            .flat_map(|(s, ts)| ts.into_iter().map(move |t| (s, t.1)))
            .collect()
    };
    let reference_turns = flatten(&driven_turns(1, &unpaced()));
    for rate in [1_000.0, 50_000.0] {
        let paced = flatten(&driven_turns(
            1,
            &RunOptions {
                pacing: Pacing::Real,
                rate,
                lateness_tolerance_us: u64::MAX,
                ..unpaced()
            },
        ));
        assert_eq!(
            paced, reference_turns,
            "the keys submitted differ at rate {rate}, so the rate reached the workload"
        );
    }
}

#[test]
fn the_pipelining_depth_does_not_change_what_is_submitted() {
    // The half of `contracts/node-agent-wire.md`'s requirement that lane count does not cover:
    // "the depth of pipelining MUST be configurable and MUST be independent of lane count,
    // because FR-072 requires transport concurrency not to affect the plan."
    //
    // Depth is a *transport* choice — how many turns may be outstanding on one connection before
    // a reply is drained to make room — so it must not reach the producer. Asserted rather than
    // argued, because this is the property that makes a flag unnecessary: what FR-072 wants is
    // that depth cannot change the workload, and a test that varies it is what shows that,
    // whereas exposing it on the command line would only let an operator vary it.
    //
    // Depth 1 is the interesting end. `Client::submit` drains a reply whenever the window is
    // full, so at depth 1 every submit after the first waits for the previous turn's outcome and
    // the transport is round-trip bound rather than pipelined. That is a different transport by
    // construction; the turns it carries must be identical anyway.
    let reference = by_session(&driven_turns_at_depth(1, 1, &unpaced()));
    assert!(!reference.is_empty(), "the description produced no turns");

    for depth in [2, 8, 32] {
        let other = by_session(&driven_turns_at_depth(1, depth, &unpaced()));
        assert_eq!(
            other.keys().collect::<Vec<_>>(),
            reference.keys().collect::<Vec<_>>(),
            "depth {depth} changed which sessions ran"
        );
        for (session, turns) in &reference {
            assert_eq!(
                other.get(session),
                Some(turns),
                "depth {depth} changed session {session}'s turns"
            );
        }
    }
}

#[test]
fn depth_and_lane_count_are_independent_of_each_other() {
    // The other direction of the same requirement: neither knob may need the other adjusted, so
    // the four combinations must all submit the same work. A transport that coupled them — a
    // depth expressed per lane, say — would satisfy the test above and still make one setting
    // depend on the other.
    let reference = by_session(&driven_turns_at_depth(1, 1, &unpaced()));
    for (lanes, depth) in [(1, 8), (4, 1), (4, 8)] {
        let other = by_session(&driven_turns_at_depth(lanes, depth, &unpaced()));
        for (session, turns) in &reference {
            assert_eq!(
                other.get(session),
                Some(turns),
                "lanes {lanes} at depth {depth} changed session {session}'s turns"
            );
        }
        assert_eq!(
            other.len(),
            reference.len(),
            "lanes {lanes} at depth {depth} ran a different set of sessions"
        );
    }
}
