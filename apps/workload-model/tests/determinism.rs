//! T021 and T022: what a plan promises about reproducibility and about memory.
//!
//! These are integration tests rather than unit tests because both properties are
//! about the pipeline as a whole — document in, artifact out — and a unit test on
//! any one stage can hold while the composition does not.
//!
//! **SC-003** is the reproducibility claim: the same document and seed produce a
//! byte-identical plan, and a changed seed produces a different plan with the same
//! distributional properties. Both halves matter. Without the first, no two arms
//! of a comparison can be proven to have seen the same input. Without the second,
//! a seed would be a parameter of the workload rather than of the sample, and a
//! repeat would be measuring a different experiment.
//!
//! **FR-010** is the memory claim: resident state is O(live sessions), so run
//! length does not appear in the bound at all.

use std::collections::HashMap;

use workload_model::keys::CacheKey;
use workload_model::plan::{flags, Generator, PlanEvent, PlanWriter};
use workload_model::schema::Document;

/// A document with several turns per session and a trunk narrow enough to be
/// shared, budgeted by duration so the run outlives its own population ramp.
fn doc(seed: u64, duration: &str) -> Document {
    let y = format!(
        r#"
version: 1
seed: {seed}
duration: {duration}
corpus:
  block_bytes: {{dist: lognormal, median: 131072, sigma: 0.3}}
  trees:
    roots: {{count: 8, popularity: {{dist: zipf, s: 0.9}}}}
    shared_depth: {{dist: empirical, points: [[4, 0.10], [18, 0.75], [40, 1.0]]}}
    branching: 1.02
    branch_skew: 0.9
workload:
  arrival: {{model: open_loop, rate: 4000/s, burstiness: 1.8}}
  sessions:
    turns: {{dist: geometric, mean: 5}}
    think_time: {{dist: const, value: 0.2}}
    private_depth: {{dist: lognormal, median: 8, sigma: 0.8}}
    growth_per_turn: {{dist: const, value: 6}}
  mix:
    - {{weight: 0.70}}
    - {{weight: 0.30, turns: 1}}
topology:
  nodes: [node2, node7, node9, node11]
run:
  mode: hardware
  wss_window: 240000
"#
    );
    Document::from_yaml(&y).expect("fixture must parse")
}

/// Generate a whole plan into `events.bin` bytes plus its realised statistics.
fn plan_bytes(d: &Document) -> (Vec<u8>, workload_model::plan::PlanStats) {
    let mut g = Generator::new(d).unwrap();
    let mut w = PlanWriter::new(Vec::new());
    let mut buf = Vec::new();
    while g.fill(&mut buf) > 0 {
        w.push_all(&buf).expect("a generated plan must be writable");
    }
    w.finish(&d.to_yaml().unwrap()).unwrap()
}

fn events(d: &Document) -> Vec<PlanEvent> {
    let mut g = Generator::new(d).unwrap();
    let mut all = Vec::new();
    let mut buf = Vec::new();
    while g.fill(&mut buf) > 0 {
        all.extend_from_slice(&buf);
    }
    all
}

/// The distributional properties a repeat must preserve: they are what the
/// document states, so a seed change must not move them.
#[derive(Debug)]
struct Shape {
    requests: usize,
    mean_request_len: f64,
    /// Fraction of key references that some other session also touched.
    shared_fraction: f64,
    /// Fraction of requests that are a session's first turn.
    first_turn_fraction: f64,
    mean_bytes: f64,
}

fn shape(ev: &[PlanEvent]) -> Shape {
    let mut per_request = 0usize;
    let mut lens = Vec::new();
    let mut first_turns = 0usize;
    let mut owners: HashMap<CacheKey, u32> = HashMap::new();
    let mut multi: HashMap<CacheKey, bool> = HashMap::new();
    let mut run = 0usize;
    for e in ev {
        run += 1;
        match owners.get(&e.key) {
            Some(sid) if *sid != e.session_id.0 => {
                multi.insert(e.key, true);
            }
            None => {
                owners.insert(e.key, e.session_id.0);
            }
            _ => {}
        }
        if e.has(flags::REQUEST_END) {
            lens.push(run);
            run = 0;
            per_request += 1;
            if e.turn == 1 {
                first_turns += 1;
            }
        }
    }
    let shared_refs = ev.iter().filter(|e| multi.contains_key(&e.key)).count();
    Shape {
        requests: per_request,
        mean_request_len: lens.iter().sum::<usize>() as f64 / lens.len().max(1) as f64,
        shared_fraction: shared_refs as f64 / ev.len().max(1) as f64,
        first_turn_fraction: first_turns as f64 / per_request.max(1) as f64,
        mean_bytes: ev.iter().map(|e| u64::from(e.size)).sum::<u64>() as f64
            / ev.len().max(1) as f64,
    }
}

#[test]
fn the_same_document_and_seed_give_a_byte_identical_plan() {
    // SC-003's first half, and the reason a plan is generated once and
    // distributed rather than generated per node: identity is over bytes.
    let d = doc(0xC0FFEE, "6s");
    let (a, sa) = plan_bytes(&d);
    let (b, sb) = plan_bytes(&d);
    assert!(!a.is_empty(), "the fixture generated nothing");
    assert_eq!(a, b, "events.bin differed between two runs of one document");
    assert_eq!(sa.content_hash, sb.content_hash);
    assert_eq!(sa.stream_digest, sb.stream_digest);
    assert_eq!(sa.event_count, sb.event_count);

    // A second Document parsed from the same text must agree too: identity is
    // over the normalised document, not over the object that produced it.
    let reparsed = Document::from_yaml(&d.to_yaml().unwrap()).unwrap();
    let (c, sc) = plan_bytes(&reparsed);
    assert_eq!(a, c);
    assert_eq!(sa.content_hash, sc.content_hash);
}

#[test]
fn a_changed_seed_changes_the_plan_but_not_what_it_is_a_sample_of() {
    // SC-003's second half. If a seed moved the distributional properties it
    // would be a parameter of the workload rather than of the sample, and a
    // repeat would measure a different experiment.
    let a = doc(0xC0FFEE, "6s");
    let b = doc(0xDECAFBAD, "6s");
    let (bytes_a, stats_a) = plan_bytes(&a);
    let (bytes_b, stats_b) = plan_bytes(&b);
    assert_ne!(bytes_a, bytes_b, "a new seed produced the identical plan");
    assert_ne!(stats_a.content_hash, stats_b.content_hash);
    assert_ne!(
        stats_a.stream_digest, stats_b.stream_digest,
        "two arms with differing seeds must not appear to have seen one stream"
    );

    let sa = shape(&events(&a));
    let sb = shape(&events(&b));
    let close = |x: f64, y: f64, tol: f64, what: &str| {
        let rel = (x - y).abs() / x.max(y).max(f64::MIN_POSITIVE);
        assert!(rel < tol, "{what} moved with the seed: {x} vs {y}");
    };
    close(
        sa.requests as f64,
        sb.requests as f64,
        0.10,
        "request count",
    );
    close(
        sa.mean_request_len,
        sb.mean_request_len,
        0.15,
        "mean request length",
    );
    close(sa.mean_bytes, sb.mean_bytes, 0.05, "mean entry size");
    close(
        sa.first_turn_fraction,
        sb.first_turn_fraction,
        0.10,
        "first-turn fraction",
    );
    // Sharing is the property the whole corpus model exists to produce, so it is
    // the one that would matter most if a seed moved it.
    assert!(sa.shared_fraction > 0.0 && sb.shared_fraction > 0.0);
    close(
        sa.shared_fraction,
        sb.shared_fraction,
        0.25,
        "cross-session shared fraction",
    );
}

#[test]
fn resident_state_is_bounded_by_live_sessions_not_by_run_length() {
    // FR-010. Run length grows by an order of magnitude; what must not grow is
    // the generator's own state. The trie is never materialised — a key is a hash
    // of the path to it — so each turn re-walks its path and keeps none of it.
    let short = doc(7, "2s");
    let long = doc(7, "20s");

    let measure = |d: &Document| -> (usize, u64, usize) {
        let mut g = Generator::new(d).unwrap();
        let mut buf = Vec::new();
        let mut peak_live = 0usize;
        let mut peak_cap = 0usize;
        while g.fill(&mut buf) > 0 {
            peak_live = peak_live.max(g.live_sessions());
            peak_cap = peak_cap.max(buf.capacity());
        }
        (peak_live, g.events_emitted(), peak_cap)
    };

    let (live_short, events_short, cap_short) = measure(&short);
    let (live_long, events_long, cap_long) = measure(&long);

    assert!(
        events_long > events_short * 8,
        "the run did not actually grow: {events_short} -> {events_long}"
    );
    // The population is set by arrival rate and session lifetime (Little's law),
    // neither of which is a function of how long the run is.
    let ratio = live_long as f64 / live_short.max(1) as f64;
    assert!(
        ratio < 1.5,
        "live population grew with run length: {live_short} -> {live_long} ({ratio:.2}x) \
         while events grew {:.1}x",
        events_long as f64 / events_short as f64
    );
    // And the look-ahead buffer is a horizon, so it does not grow either.
    assert_eq!(cap_short, cap_long, "the chunk buffer grew with run length");

    // Stated in bytes, since that is what "resident memory" means: the heap the
    // generator holds is the live population times a fixed per-session cost.
    let per_session = std::mem::size_of::<workload_model::session::Session>();
    let bound = live_long * (per_session * 2);
    assert!(
        bound < 4 * 1024 * 1024,
        "a {live_long}-session population should not be megabytes: {bound}"
    );
}

#[test]
fn a_longer_run_produces_proportionally_more_events() {
    // The same claim from the other direction: what *should* scale, does. A test
    // that only asserted the memory bound would also pass on a generator that had
    // quietly stopped generating.
    let mut totals = Vec::new();
    for secs in [1u32, 4, 16] {
        let d = doc(11, &format!("{secs}s"));
        let mut g = Generator::new(&d).unwrap();
        let mut buf = Vec::new();
        while g.fill(&mut buf) > 0 {}
        totals.push(g.events_emitted());
    }
    assert!(
        totals[1] > totals[0] * 2 && totals[2] > totals[1] * 2,
        "events did not grow with run length: {totals:?}"
    );
}

#[test]
fn two_late_windows_of_one_run_carry_comparable_load() {
    // Steady state, measured where it exists. Events *per plan-second* is
    // deliberately not asserted flat across whole runs of different lengths,
    // because it genuinely is not: a run starts with an empty population, and
    // `growth_per_turn` deepens a session's path every turn, so a geometric turn
    // count gives long-lived sessions ever-longer requests and the sample mean
    // over a whole run keeps climbing with run length. That is a property of the
    // configured model rather than a defect, and the way to see steady state is
    // to compare equal windows *inside* one run rather than whole runs against
    // each other.
    let ev = events(&doc(11, "16s"));
    let window = |from: u64, to: u64| -> usize {
        ev.iter()
            .filter(|e| e.t_ns >= from * 1_000_000_000 && e.t_ns < to * 1_000_000_000)
            .count()
    };
    let a = window(6, 11);
    let b = window(11, 16);
    assert!(a > 0 && b > 0, "no events in the late windows: {a}, {b}");
    let rel = (a as f64 - b as f64).abs() / a.max(b) as f64;
    assert!(
        rel < 0.25,
        "two equal late windows disagreed by {:.0}%: {a} vs {b}",
        rel * 100.0
    );
}
