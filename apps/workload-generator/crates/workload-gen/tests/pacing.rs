//! Paced mode end to end, against a loopback agent stub (T088-T091, FR-080).
//!
//! # What a stub can and cannot show
//!
//! Everything here is about **when** a turn is submitted, which is entirely the driver's business:
//! the schedule is computed from the plan's virtual timestamps and the wallclock, and no part of it
//! depends on what a cache answers. So a stub is not a weakened version of this test — it is the
//! right instrument, and it lets the schedule be checked in the ordinary gate with no server, no
//! accelerator and no cluster.
//!
//! What it cannot show is that a *real* Certus keeps the schedule, which is the measurement the
//! feature exists for and belongs on hardware.
//!
//! # The spans are short and the rates are high, deliberately
//!
//! A paced run costs wallclock equal to its virtual span divided by its rate. That is the whole
//! point of the mode and it makes it expensive to test: at rate 1.0 a 120-second description takes
//! two minutes. So the tests that only need the arithmetic run at a high rate, and the one test
//! that checks the wallclock cost itself uses a span of a couple of seconds.

#![cfg(feature = "live")]

use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use workload_gen::agents::{AgentSpec, Agents, NoLaunch};
use workload_gen::drive::{self, DriveError, DriveStats};
use workload_gen::live::{Pacing, RunOptions};
use workload_model::description::WorkloadDescription;
use workload_wire::frame::{
    ClearCacheAck, Hello, HelloAck, ShutdownAck, Stats, SubmitTurn, TurnOutcome,
};
use workload_wire::handshake;
use workload_wire::server::{FnFactory, Server, Service};

/// A description whose turns are spread over virtual time, so a schedule has something to keep.
///
/// `think_time` 1 second with 4 turns gives each session a 3-second span, and sessions arrive
/// across the run — so at rate 1.0 the turns are genuinely spread out rather than all due at once.
const DESCRIPTION: &str = r#"
version: 1
blocks: {tokens: 16, bytes: 32768}
shared_classes:
  manual:
    length: {constant: 2}
    lifetime: {constant: .inf}
session_classes:
  chat:
    pool: {size: {exact: 4}}
    uses: [{class: manual, count: {constant: 1}}]
    turns: {constant: 4}
    input_growth: {constant: 1}
    output_growth: {constant: 1}
    think_time: {constant: 1}
"#;

const SEED: u64 = 17;

/// How a stub answers, and what it recorded.
#[derive(Clone)]
struct Behaviour {
    /// Wallclock delay per turn, for making a run miss its schedule.
    per_turn: Duration,
    /// Entries a clear reports, or `None` to refuse the clear.
    clears: Option<u64>,
    /// Turns served, across every connection.
    turns: Arc<AtomicUsize>,
    /// When each turn was served, so the realised spacing can be checked.
    at: Arc<Mutex<Vec<Duration>>>,
    /// The run's own origin, so those instants are relative to something.
    t0: Instant,
}

impl Behaviour {
    fn new() -> Self {
        Self {
            per_turn: Duration::ZERO,
            clears: Some(0),
            turns: Arc::new(AtomicUsize::new(0)),
            at: Arc::new(Mutex::new(Vec::new())),
            t0: Instant::now(),
        }
    }
}

struct Stub {
    how: Behaviour,
}

impl Service for Stub {
    fn hello(&mut self, hello: &Hello) -> HelloAck {
        handshake::answer(hello, 8, 32768)
    }

    fn submit_turn(&mut self, turn: &SubmitTurn) -> TurnOutcome {
        if !self.how.per_turn.is_zero() {
            std::thread::sleep(self.how.per_turn);
        }
        self.how.turns.fetch_add(1, Ordering::Relaxed);
        self.how.at.lock().unwrap().push(self.how.t0.elapsed());
        TurnOutcome {
            resident: turn.path.len() as u32,
            ..Default::default()
        }
    }

    fn clear_cache(&mut self) -> ClearCacheAck {
        match self.how.clears {
            Some(n) => ClearCacheAck::cleared(n),
            None => ClearCacheAck::failed("this stub has no memory tier"),
        }
    }

    fn stats(&mut self) -> Stats {
        Stats::default()
    }

    fn shutdown(&mut self) -> ShutdownAck {
        ShutdownAck::default()
    }
}

fn free_port() -> u16 {
    let l = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let p = l.local_addr().unwrap().port();
    drop(l);
    p
}

/// Drive one loopback stub and return what the run measured.
fn drive_stub(how: &Behaviour, options: &RunOptions) -> Result<(DriveStats, Duration), DriveError> {
    let port = free_port();
    let for_service = how.clone();
    let server = Server::bind(
        ("127.0.0.1", port),
        FnFactory(move || {
            Ok(Stub {
                how: for_service.clone(),
            })
        }),
    )
    .expect("bind the loopback agent")
    .with_linger(Duration::from_secs(120));
    let stop_server = Arc::new(AtomicBool::new(false));
    let flag = Arc::clone(&stop_server);
    let handle = std::thread::spawn(move || {
        let _ = server.serve(flag);
    });
    for _ in 0..300 {
        if std::net::TcpStream::connect(("127.0.0.1", port)).is_ok() {
            break;
        }
        std::thread::sleep(Duration::from_millis(10));
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
    let mut agents = Agents::start_with(&NoLaunch, &specs, 8, false).expect("handshake");
    let d: WorkloadDescription = DESCRIPTION.parse().unwrap();
    let started = Instant::now();
    let out = drive::run(&mut agents, &d, options, Arc::new(AtomicBool::new(false)));
    let wallclock = started.elapsed();
    agents.stop().expect("stop");
    stop_server.store(true, Ordering::Relaxed);
    let _ = handle.join();
    out.map(|o| (o, wallclock))
}

fn paced(rate: f64, until: f64) -> RunOptions {
    RunOptions {
        seed: SEED,
        until: Some(until),
        batch_keys: 64,
        clear_cache: false,
        pacing: Pacing::Real,
        rate,
        lateness_tolerance_us: workload_gen::live::DEFAULT_LATENESS_TOLERANCE_US,
    }
}

#[test]
fn a_paced_run_costs_its_virtual_span_divided_by_the_rate() {
    // The defining behaviour, and the reason the cost is projected before a run starts: at rate
    // 1.0 the wallclock *is* the virtual span. Two virtual seconds so the assertion is cheap.
    let how = Behaviour::new();
    let (out, wallclock) = drive_stub(&how, &paced(1.0, 2.0)).expect("drive");

    assert!(
        wallclock >= Duration::from_millis(1_800),
        "a paced run over 2 virtual seconds finished in {wallclock:?}; it did not wait at all"
    );
    // Only a hang guard, deliberately loose. The claim under test is that the run *waited*, which
    // is the lower bound above; an upper bound tight enough to be a timing assertion would be a
    // statement about how busy the machine running the test is.
    assert!(
        wallclock < Duration::from_secs(60),
        "took {wallclock:?} for 2 virtual seconds at rate 1.0, which is a hang rather than pacing"
    );
    // The cross-check T088d asks for: the achieved ratio must come out near the rate asked for.
    // One-sided on the fast side, because overshooting the requested rate is what would mean the
    // schedule was not being kept, while a loaded machine can legitimately undershoot.
    let achieved = out.stats.virtual_to_wallclock();
    assert!(
        achieved < 2.0,
        "asked for rate 1.0 and achieved {achieved}, so the run outran its own schedule"
    );
    assert!(out.stats.is_valid(), "a stub cannot make a run late");
    assert!(out.stats.paced_turns() > 0, "no turn was held for its time");
    assert_eq!(
        out.stats.paced_turns(),
        how.turns.load(Ordering::Relaxed) as u64,
        "every turn submitted must have been scheduled"
    );
}

#[test]
fn the_rate_divides_the_cost_so_the_same_span_runs_faster() {
    // What makes `--rate` a calibration control rather than a convenience: the same description
    // aimed at a faster target.
    //
    // Measured **against a rate-1.0 baseline in this same test** rather than against a millisecond
    // bound. An absolute bound here is a statement about how busy the machine running the test is,
    // and a flake in this suite is what prompted the change; a ratio between two runs on the same
    // machine is the claim actually being made.
    let baseline = drive_stub(&Behaviour::new(), &paced(1.0, 2.0))
        .expect("drive the baseline")
        .1;
    let (out, wallclock) = drive_stub(&Behaviour::new(), &paced(20.0, 2.0)).expect("drive");
    assert!(
        wallclock * 4 < baseline,
        "rate 20 took {wallclock:?} against {baseline:?} at rate 1.0; a twentyfold rate should be \
         far more than four times faster, so the rate is not dividing the schedule"
    );
    assert!(out.stats.is_valid());
}

#[test]
fn an_unpaced_run_does_not_wait_at_all() {
    // The other mode, and the contrast that makes the one above meaningful: work-conserving issues
    // as fast as the transport allows, so the same 2 virtual seconds cost milliseconds.
    let how = Behaviour::new();
    let baseline = drive_stub(&Behaviour::new(), &paced(1.0, 2.0))
        .expect("drive the baseline")
        .1;
    let (out, wallclock) = drive_stub(
        &how,
        &RunOptions {
            pacing: Pacing::None,
            rate: 1.0,
            ..paced(1.0, 2.0)
        },
    )
    .expect("drive");
    // Against the paced baseline rather than a millisecond bound, for the reason given in
    // `the_rate_divides_the_cost_so_the_same_span_runs_faster`.
    assert!(
        wallclock * 4 < baseline,
        "a work-conserving run took {wallclock:?} against {baseline:?} paced; it appears to have \
         waited for something"
    );
    // Nothing was scheduled, so there is no lateness — and validity falls back to the queue.
    assert_eq!(out.stats.paced_turns(), 0);
    assert!(out.stats.kept_the_schedule(), "there was no schedule");
    assert!(out.stats.rate_shortfall().is_none());
}

#[test]
fn a_slow_node_makes_the_run_late_and_that_invalidates_it() {
    // FR-080's validity metric, and the failure it names. A node too slow to keep the schedule
    // pushes every subsequent turn past its due time; because due times are absolute from `t0`
    // that lateness **accumulates** rather than being absorbed, which is the whole reason the
    // schedule is not measured from the previous submission.
    //
    // 40 ms per turn against turns due every virtual second at rate 100 — one turn per 10 ms —
    // so the node is four times too slow and falls behind steadily.
    let mut how = Behaviour::new();
    how.per_turn = Duration::from_millis(40);
    let (out, _) = drive_stub(&how, &paced(100.0, 12.0)).expect("drive");

    assert!(out.stats.paced_turns() >= 8, "too few turns to fall behind");
    assert!(
        !out.stats.kept_the_schedule(),
        "p99 lateness was {} us against a {} us tolerance, so the slow node went unnoticed",
        out.stats.lateness_us(0.99),
        out.stats.lateness_tolerance_us
    );
    assert!(
        !out.stats.is_valid(),
        "a run that missed its schedule must be invalid (FR-080)"
    );
    // **And the old metric would have called this run valid**, which is the substance of replacing
    // it rather than adding one. The node here is slower than the schedule, so the producer stays
    // comfortably ahead and no lane ever runs dry: FR-062's test sees a healthy queue while the run
    // is failing to keep the very schedule it set itself.
    assert_eq!(
        out.stats.underruns(),
        0,
        "the queue underran, so this run does not demonstrate what it is here to demonstrate"
    );

    // The second route to the same fact: the achieved rate falls short of the requested one.
    let shortfall = out.stats.rate_shortfall().expect("paced");
    assert!(
        shortfall > 0.1,
        "lateness accumulated but the achieved rate was within {:.1}% of the request, so the two \
         routes disagree",
        shortfall * 100.0
    );
}

#[test]
fn a_tolerance_wide_enough_accepts_the_same_slow_run() {
    // The tolerance is a real knob rather than a constant dressed up as one: the same run that
    // fails above passes when the schedule it is held to is loose enough. This is what a rate
    // sweep varies against, and it is why the tolerance is on the report beside the percentiles.
    let mut how = Behaviour::new();
    how.per_turn = Duration::from_millis(40);
    let (out, _) = drive_stub(
        &how,
        &RunOptions {
            lateness_tolerance_us: 60_000_000,
            ..paced(100.0, 12.0)
        },
    )
    .expect("drive");
    assert!(out.stats.kept_the_schedule());
    assert!(out.stats.is_valid());
    assert!(
        out.stats.lateness_us(0.99) > 0,
        "the run was late; the tolerance is what makes it acceptable, not the absence of lateness"
    );
}

#[test]
fn a_clear_that_the_node_could_not_do_refuses_the_run_before_it_starts() {
    // FR-046 through FR-079: the generator has no mailbox, so it asks. A node that answers and
    // could not clear must stop the run — otherwise it reports a cold cache it never had, and the
    // hit rate below is a plausible number for a different experiment.
    let mut how = Behaviour::new();
    how.clears = None;
    let err = drive_stub(
        &how,
        &RunOptions {
            clear_cache: true,
            ..paced(1_000.0, 4.0)
        },
    )
    .expect_err("a refused clear must not be run past");
    match err {
        // Setup, not a lost node: nothing was issued, so this is a rejected invocation rather than
        // a run worth retrying.
        DriveError::Setup(why) => {
            assert!(why.contains("clear"), "{why}");
            assert!(why.contains("FR-046"), "the reason must be cited: {why}");
        }
        DriveError::Lost(l) => panic!("a refused clear was reported as a lost node: {l}"),
    }
    assert_eq!(
        how.turns.load(Ordering::Relaxed),
        0,
        "turns were submitted although the cache clear had failed"
    );
}

#[test]
fn a_clear_that_worked_is_counted_and_the_run_proceeds() {
    // The success path, and the count that goes on the report. Asked once per node rather than
    // once per lane: the clear is per node, and asking on each lane would clear a cache the
    // previous lane had just cleared and then report the entries twice.
    let mut how = Behaviour::new();
    how.clears = Some(4_242);
    let (out, _) = drive_stub(
        &how,
        &RunOptions {
            clear_cache: true,
            ..paced(1_000.0, 4.0)
        },
    )
    .expect("drive");
    assert_eq!(out.stats.cleared_entries, Some(4_242));
    assert!(how.turns.load(Ordering::Relaxed) > 0);

    // And a run that did not ask reports `None` rather than zero: "no clear was requested" and
    // "the clear dropped nothing" are different facts.
    let (out, _) = drive_stub(&Behaviour::new(), &paced(1_000.0, 4.0)).expect("drive");
    assert_eq!(out.stats.cleared_entries, None);
}

// ---------------------------------------------------------------------------
// The command line: what it refuses, before anything is started.
// ---------------------------------------------------------------------------

/// A `run` invocation against a port nothing is listening on, with the given extra flags.
///
/// The refusals under test happen **before** any agent is contacted, so nothing needs to be
/// running for them — and a test that had to start something would be testing two things.
fn run_argv(extra: &[&str]) -> i32 {
    let dir = tempfile::TempDir::new().unwrap();
    let path = dir.path().join("d.yml");
    std::fs::write(&path, DESCRIPTION).unwrap();
    let mut argv: Vec<String> = vec![
        "workload-gen".into(),
        "run".into(),
        path.display().to_string(),
        "--seed".into(),
        "1".into(),
        "--until".into(),
        "1".into(),
        "--no-launch".into(),
        "--instance".into(),
        format!("127.0.0.1:{}", free_port()),
    ];
    argv.extend(extra.iter().map(|s| (*s).to_string()));
    workload_gen::cli::run_argv(&argv)
}

#[test]
fn a_rate_of_zero_or_less_is_refused_rather_than_quietly_running_work_conserving() {
    // Zero would make every turn due at `t0`, which is work-conserving wearing a rate's name — a
    // run reported as paced that measured a ceiling. Negative has no meaning at all.
    for rate in ["0", "-1", "nan"] {
        assert_eq!(
            run_argv(&["--rate", rate]),
            2,
            "--rate {rate} must be refused, and before anything is started"
        );
    }
}

#[test]
fn the_contradiction_that_needed_a_refusal_is_now_unsayable() {
    // `--pacing none --rate 10` asked for two different things, and honouring one silently is how
    // a run gets quoted as the other. FR-081 deletes the flag instead of policing the pair: with
    // `--rate` the only knob, the mode is derived from the value and the two cannot disagree.
    assert_eq!(
        run_argv(&["--pacing", "none"]),
        2,
        "--pacing must no longer be accepted at all"
    );
    // And the mode it used to select is now a value of the rate.
    assert_ne!(
        run_argv(&["--rate", "inf"]),
        2,
        "--rate inf is a work-conserving run, not a bad argument"
    );
}

#[test]
fn a_large_finite_rate_is_not_inf_and_keeps_its_schedule() {
    // Somebody will type a large number meaning "flat out". It is accepted, because it is a
    // legitimate calibration, and it stays *paced* — so a machine that cannot keep up records
    // real lateness and the run is correctly invalid. Only `inf` switches the schedule off.
    assert_ne!(run_argv(&["--rate", "1e12"]), 2);
}
