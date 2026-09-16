//! Agent lifecycle (T071): FR-050, FR-052, FR-053.
//!
//! The launcher is a trait precisely so this can run here: the *policy* — detect a leftover,
//! replace it, verify the teardown — is exercised against a real agent listening on a real
//! loopback port, and only the ssh mechanics are left untested. A policy testable only on a
//! cluster would be tested rarely, and this one guards a failure that has already cost this
//! repository an entire A/B series.
//!
//! The stand-in agent is `workload-wire`'s own server with a trivial service, so the
//! handshake, the shutdown and the port going quiet are all genuine.

#![cfg(feature = "live")]

use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use workload_gen::agents::{AgentSpec, Agents, Launcher};
use workload_wire::frame::{Hello, HelloAck, ShutdownAck, SubmitTurn, TurnOutcome};
use workload_wire::handshake;
use workload_wire::server::{FnFactory, Server, Service};

/// Stop flags for the stand-in agents this launcher started, keyed by port.
type Running = Arc<Mutex<Vec<(u16, Arc<AtomicBool>)>>>;

/// The trivial service a stand-in agent serves.
struct Stub {
    /// How the handshake should answer, so a stale leftover can be simulated.
    stale: bool,
}

impl Service for Stub {
    fn hello(&mut self, hello: &Hello) -> HelloAck {
        let mut ack = handshake::answer(hello, 8, 32768);
        if self.stale {
            // A different build: what a leftover from an older deployment looks like.
            ack.build_id = handshake::digest("an older tree");
            ack.status = handshake::status::BUILD_MISMATCH;
        }
        ack
    }
    fn submit_turn(&mut self, _t: &SubmitTurn) -> TurnOutcome {
        TurnOutcome::default()
    }
    fn shutdown(&mut self) -> ShutdownAck {
        ShutdownAck {
            ops_submitted: 7,
            ops_failed: 0,
        }
    }
}

/// A launcher that starts stand-in agents in-process instead of over ssh.
#[derive(Clone)]
struct LocalLauncher {
    launches: Arc<AtomicUsize>,
    kills: Arc<AtomicUsize>,
    stale: bool,
    /// Stop flags by port, so a "kill" is genuine **and port-scoped**.
    ///
    /// Port-scoped because the real `SshLauncher::kill` matches on `--port N`: a pattern that
    /// matched only the binary would kill another run's agent on the same host. A fixture that
    /// killed everything would pass tests the production launcher would fail, and it did —
    /// starting a second agent stopped the first, because replacing a non-existent leftover
    /// calls `kill`.
    running: Running,
    /// When set, `launch` starts nothing — for the "never comes up" case.
    refuse_to_launch: bool,
}

impl LocalLauncher {
    fn new() -> Self {
        Self {
            launches: Arc::new(AtomicUsize::new(0)),
            kills: Arc::new(AtomicUsize::new(0)),
            stale: false,
            running: Arc::new(Mutex::new(Vec::new())),
            refuse_to_launch: false,
        }
    }

    /// Start a stand-in agent on `port` and return once it is listening.
    fn spawn(&self, port: u16, stale: bool) -> Arc<AtomicBool> {
        let server = Server::bind(("127.0.0.1", port), FnFactory(move || Ok(Stub { stale })))
            .expect("bind the stand-in agent");
        let stop = Arc::new(AtomicBool::new(false));
        let flag = Arc::clone(&stop);
        std::thread::spawn(move || {
            let _ = server.serve(flag);
        });
        // Wait until it accepts, so a test never races its own fixture.
        for _ in 0..200 {
            if std::net::TcpStream::connect(("127.0.0.1", port)).is_ok() {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        self.running.lock().unwrap().push((port, Arc::clone(&stop)));
        stop
    }
}

impl Launcher for LocalLauncher {
    fn launch(&self, spec: &AgentSpec) -> Result<(), String> {
        self.launches.fetch_add(1, Ordering::Relaxed);
        if self.refuse_to_launch {
            return Ok(()); // issued, but nothing comes up
        }
        self.spawn(spec.port, self.stale);
        Ok(())
    }

    fn kill(&self, spec: &AgentSpec) -> Result<(), String> {
        self.kills.fetch_add(1, Ordering::Relaxed);
        for (port, stop) in self.running.lock().unwrap().iter() {
            if *port == spec.port {
                stop.store(true, Ordering::Relaxed);
            }
        }
        Ok(())
    }
}

/// A free loopback port.
fn free_port() -> u16 {
    let l = std::net::TcpListener::bind("127.0.0.1:0").expect("a port");
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
        lanes: 4,
        block_bytes: 32768,
        batch_keys: 64,
        extra_args: Vec::new(),
    }
}

#[test]
fn an_agent_is_started_and_the_handshake_reports_its_capacity() {
    let launcher = LocalLauncher::new();
    let specs = vec![spec(free_port())];
    let mut agents = Agents::start_default(&launcher, &specs).expect("start");
    assert_eq!(agents.len(), 1);
    assert_eq!(launcher.launches.load(Ordering::Relaxed), 1);
    let a = &agents.agents()[0];
    assert_eq!(a.ack.channels, 8, "capacity must reach the generator");
    assert_eq!(a.ack.block_bytes, 32768);

    let teardown = agents.stop().expect("stop");
    assert_eq!(teardown.len(), 1);
    assert_eq!(teardown[0].ack.ops_submitted, 7, "the agent's own tally");
    assert!(
        teardown[0].port_released,
        "the port never went quiet, so teardown was not verified (FR-053)"
    );
}

#[test]
fn a_leftover_of_the_current_build_is_replaced_rather_than_reused() {
    // FR-052's harder half. The provenance check already makes reusing a *stale* agent
    // impossible; a *current* one is equally unusable, because it holds the previous run's
    // mailbox channels and device memory and its counters would be reported as this run's.
    let port = free_port();
    let launcher = LocalLauncher::new();
    // A leftover, listening before the run starts.
    launcher.spawn(port, false);
    assert!(
        std::net::TcpStream::connect(("127.0.0.1", port)).is_ok(),
        "the fixture leftover is not listening"
    );

    let specs = vec![spec(port)];
    let agents = Agents::start_default(&launcher, &specs).expect("start over a leftover");
    // It was replaced: a fresh one was launched, and the old one was stopped or killed.
    assert_eq!(
        launcher.launches.load(Ordering::Relaxed),
        1,
        "no fresh agent was launched, so the leftover was reused"
    );
    assert_eq!(agents.len(), 1);
    agents.stop().expect("stop");
}

#[test]
fn a_leftover_that_will_not_answer_is_killed() {
    // The worst of the three leftovers: a process holding channels and answering nothing. It
    // cannot be asked to stop, so it must be killed, or the run would share its channels.
    let port = free_port();
    // Something occupying the port that never speaks the protocol.
    let squatter = std::net::TcpListener::bind(("127.0.0.1", port)).expect("squat");
    let launcher = LocalLauncher::new();
    let specs = vec![spec(port)];

    // The squatter accepts but never replies, so the handshake fails and the launcher is asked
    // to kill. Dropping the listener here is what a successful kill looks like.
    let killed = Arc::clone(&launcher.kills);
    std::thread::spawn(move || {
        while killed.load(Ordering::Relaxed) == 0 {
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        drop(squatter);
    });

    let result = Agents::start_default(&launcher, &specs);
    assert!(
        launcher.kills.load(Ordering::Relaxed) > 0,
        "a silent leftover was not killed"
    );
    // Whether the start then succeeds depends on the race with the fixture's drop; what matters
    // is that a kill was attempted rather than the leftover being adopted.
    if let Ok(agents) = result {
        agents.stop().expect("stop");
    }
}

#[test]
fn a_stale_leftover_is_replaced_and_the_refusal_would_name_the_node() {
    // A leftover from an older deployment. It is replaced like any other; the provenance check
    // is what stops it being *driven*, and this is what stops it being left in place.
    let port = free_port();
    let mut launcher = LocalLauncher::new();
    launcher.spawn(port, true); // the leftover is stale
    launcher.stale = false; // the replacement is this build

    let specs = vec![spec(port)];
    let agents = Agents::start_default(&launcher, &specs).expect("start over a stale leftover");
    assert_eq!(launcher.launches.load(Ordering::Relaxed), 1);
    agents.stop().expect("stop");
}

#[test]
fn a_stale_agent_is_refused_by_name_when_it_is_the_one_that_was_started() {
    // The complement: if the *replacement* is stale, the run must not start at all, and the
    // message must name the node — a refusal an operator cannot attribute to a machine is one
    // they cannot act on.
    let mut launcher = LocalLauncher::new();
    launcher.stale = true;
    let specs = vec![spec(free_port())];
    let err = Agents::start_default(&launcher, &specs).expect_err("a stale agent must be refused");
    assert!(err.contains("127.0.0.1"), "the node must be named: {err}");
    assert!(err.contains("FR-051"), "and the reason cited: {err}");
}

#[test]
fn a_node_that_never_comes_up_is_reported_with_its_node_and_port() {
    // A launch that issues but never listens — a wrong binary path, a missing library. The
    // message has to name both, since on a cluster the next step is to go and look.
    let mut launcher = LocalLauncher::new();
    launcher.refuse_to_launch = true;
    let port = free_port();
    let specs = vec![spec(port)];
    let err = Agents::start_default(&launcher, &specs).expect_err("nothing came up");
    assert!(err.contains("127.0.0.1"), "{err}");
    assert!(err.contains(&port.to_string()), "{err}");
}

#[test]
fn dropping_the_agents_stops_them_so_a_panicking_run_leaves_nothing_holding_channels() {
    // FR-053's ordinary and panicking cases. A SIGKILL runs nothing at all, which is why
    // leftover replacement exists — the two requirements are halves of one mechanism.
    let port = free_port();
    let launcher = LocalLauncher::new();
    let specs = vec![spec(port)];
    {
        let _agents = Agents::start_default(&launcher, &specs).expect("start");
        assert!(
            std::net::TcpStream::connect(("127.0.0.1", port)).is_ok(),
            "the agent should be listening while the guard is alive"
        );
    } // dropped here without `stop`

    // The port must go quiet without anyone having called `stop`.
    let mut quiet = false;
    for _ in 0..100 {
        if std::net::TcpStream::connect(("127.0.0.1", port)).is_err() {
            quiet = true;
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(20));
    }
    assert!(
        quiet,
        "dropping Agents left an agent listening, so a panicking run would leave it holding \
         mailbox channels and a device allocation"
    );
}

#[test]
fn several_nodes_are_all_started_and_all_stopped() {
    let launcher = LocalLauncher::new();
    let specs: Vec<AgentSpec> = (0..3).map(|_| spec(free_port())).collect();
    let agents = Agents::start_default(&launcher, &specs).expect("start three");
    assert_eq!(agents.len(), 3);
    assert_eq!(launcher.launches.load(Ordering::Relaxed), 3);
    let teardown = agents.stop().expect("stop three");
    assert_eq!(teardown.len(), 3);
    for t in &teardown {
        assert!(t.port_released, "node {} did not release its port", t.node);
    }
}

#[test]
fn stopping_twice_is_not_attempted() {
    // `stop` consumes the guard, so `Drop` must not stop them again — a second `Shutdown` on a
    // closed connection would be reported as a teardown failure that never happened.
    let launcher = LocalLauncher::new();
    let specs = vec![spec(free_port())];
    let agents = Agents::start_default(&launcher, &specs).expect("start");
    let first = agents.stop().expect("stop");
    assert_eq!(first.len(), 1);
    // Nothing to assert beyond not panicking or hanging: the guard is gone by construction.
}

// ---------------------------------------------------------------------------
// T072: node loss aborts the run and names the node (FR-064).
// ---------------------------------------------------------------------------

use workload_gen::agents::NodeLost;
use workload_wire::client::ClientError;

#[test]
fn a_lost_node_is_named_and_the_message_says_why_the_run_cannot_continue() {
    // The message has to carry the reasoning, because "continue on the survivors" is the
    // tempting thing to do and it produces a plausible number for a different experiment.
    let lost = NodeLost::from_client("node5", "submitting a turn", ClientError::Closed);
    let text = lost.to_string();
    assert!(text.contains("node5"), "the node must be named: {text}");
    assert!(
        text.contains("submitting a turn"),
        "and what was happening: {text}"
    );
    assert!(text.contains("FR-064"), "and the requirement: {text}");
    // The three consequences, since each alone would invalidate the run.
    assert!(text.contains("prefixes"), "unreachable prefixes: {text}");
    assert!(text.contains("migration"), "shrunken target set: {text}");
    assert!(text.contains("load"), "redistributed load: {text}");
}

#[test]
fn every_transport_failure_counts_as_losing_the_node() {
    // There is no variant worth retrying: a retry extends the window in which the run is
    // measuring a cluster it no longer has. A clean close, a timeout and a protocol error all
    // mean the same thing to a run.
    for (label, err) in [
        ("closed", ClientError::Closed),
        (
            "timeout",
            ClientError::Io(std::io::Error::from(std::io::ErrorKind::TimedOut)),
        ),
        ("unexpected", ClientError::Unexpected { corr: 9 }),
    ] {
        let lost = NodeLost::from_client("node7", "draining", err);
        assert_eq!(lost.node, "node7", "{label}");
        assert!(!lost.reason.is_empty(), "{label} lost its reason");
    }
}

#[test]
fn losing_a_node_is_detected_rather_than_hung_on() {
    // The property T071's read timeout bought. A node that accepts and stops answering must
    // produce an error in bounded time — a hang is not an abort, and a run that neither
    // finishes nor fails cannot even be reported as invalid.
    let port = free_port();
    let launcher = LocalLauncher::new();
    let specs = vec![spec(port)];
    let mut agents = Agents::start_default(&launcher, &specs).expect("start");

    // Take the agent down and wait until it really is down: the fixture's kill only sets a
    // stop flag, and a connection already accepted will happily serve one more frame first. The
    // scenario being tested is a node that is *gone*, not one that is about to go.
    launcher.kill(&specs[0]).expect("kill");
    let mut down = false;
    for _ in 0..200 {
        if std::net::TcpStream::connect(("127.0.0.1", port)).is_err() {
            down = true;
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(20));
    }
    assert!(
        down,
        "the fixture agent never went down, so nothing is tested"
    );

    let started = std::time::Instant::now();
    let turn = workload_wire::frame::SubmitTurn {
        session: 1,
        flags: 0,
        path: vec![1, 2, 3],
    };
    // Submitting and draining must fail rather than block. The submit itself may succeed into a
    // socket buffer, so the drain is what has to notice.
    let outcome = agents.agents()[0]
        .submit(&turn)
        .and_then(|_| agents.agents()[0].finish().map(|_| ()));
    let elapsed = started.elapsed();
    assert!(
        outcome.is_err(),
        "a dead node was not noticed after {elapsed:?}"
    );
    let lost = outcome.unwrap_err();
    assert_eq!(lost.node, "127.0.0.1");
    assert!(
        elapsed < std::time::Duration::from_secs(35),
        "took {elapsed:?} to notice a dead node; a run must abort, not hang"
    );
    // The guard still tears down what remains: a failed run must leave nothing holding channels.
    drop(agents);
}
