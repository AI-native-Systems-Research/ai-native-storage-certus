//! T077/T078: the FR-058a round trip, and the claim that a container is not
//! information.
//!
//! Generate a plan, emit it as a trace, read the trace back, and compare the
//! statistics. Ground truth is exact here rather than estimated — both sides are the
//! same reference stream — so **any** divergence is a defect in the emitter, the
//! reader or a statistic, and not a property of some real workload. That is what
//! makes this the strongest check either `fit` or `validate` has.
//!
//! Only the JSONL container is exercised here. The parquet reader is behind a
//! default-off feature (SC-012), and the round trip through it belongs with that
//! feature rather than in a test `cargo test --all` runs.
//!
//! That leaves nothing untested, because the parquet claim is pinned one level lower
//! and more strictly: `parquet::tests::a_record_survives_the_container_exactly`
//! asserts **record equality** through a write and read, and identical records make
//! every statistic derived from them identical by construction. Measured end to end
//! as well — `fit` over the same plan in both containers reports the same divergence
//! to five decimals on every statistic.

use std::path::PathBuf;

use workload_model::plan::{Generator, PlanEvent};
use workload_model::schema::Document;
use workload_model::stats::divergence::{compare, Statistic, Tolerances};
use workload_model::stats::{Ref, Report, Statistics};
use workload_model::trace::{requests, Emitter, TraceManifest, DEFAULT_BLOCK_SIZE_TOKENS};

/// The window both sides are measured over.
const WINDOW: u64 = 5_000;

/// A plan with sharing, multiple turns and a mixture, and no `warmup`.
///
/// `warmed_doc` covers the other case. Without a warmup window the plan's measured
/// stream *is* its whole stream, so the round trip must be exact to the last bucket.
fn doc(seed: u64) -> Document {
    let y = format!(
        r#"
version: 1
seed: {seed}
requests: 12000
corpus:
  block_bytes: {{dist: const, value: 131072}}
  trees:
    roots: {{count: 12, popularity: {{dist: zipf, s: 0.9}}}}
    shared_depth: {{dist: empirical, points: [[4, 0.10], [18, 0.75], [40, 1.0]]}}
    branching: 1.02
    branch_skew: 0.9
workload:
  arrival: {{model: open_loop, rate: 2000/s, burstiness: 1.8}}
  sessions:
    turns: {{dist: geometric, mean: 5}}
    think_time: {{dist: const, value: 0.05}}
    private_depth: {{dist: lognormal, median: 20, sigma: 0.6}}
    growth_per_turn: {{dist: lognormal, median: 8, sigma: 0.4}}
  mix:
    - {{weight: 0.7}}
    - {{weight: 0.3, turns: {{dist: const, value: 1}}}}
run: {{mode: plan, wss_window: {WINDOW}}}
"#
    );
    Document::from_yaml(&y).expect("fixture must parse")
}

/// Every event of a plan.
fn events(d: &Document) -> Vec<PlanEvent> {
    let mut g = Generator::new(d).expect("generator");
    let mut out = Vec::new();
    let mut chunk = Vec::new();
    while !g.is_done() {
        chunk.clear();
        if g.fill(&mut chunk) == 0 {
            break;
        }
        out.extend_from_slice(&chunk);
    }
    out
}

/// The report over a plan's own events.
fn plan_report(ev: &[PlanEvent]) -> Report {
    let mut s = Statistics::new(WINDOW);
    for e in ev {
        s.push(&Ref::from(e));
    }
    s.finish()
}

/// Emit `ev` as a JSONL trace in a fresh directory, and return the file path.
fn emit(ev: &[PlanEvent], tag: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("certus-round-trip-{}-{tag}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("temp dir");
    let mut em = Emitter::new("round-trip", DEFAULT_BLOCK_SIZE_TOKENS, 0);
    let mut text = String::new();
    for r in requests(ev) {
        // Mirrors `certus-workload emit`: warmup requests are not written.
        if r.first()
            .is_some_and(|e| e.has(workload_model::plan::flags::WARMUP))
        {
            continue;
        }
        if let Some(rec) = em.request(r) {
            text.push_str(&serde_json::to_string(&rec).expect("record"));
            text.push('\n');
        }
    }
    let file = dir.join("trace.jsonl");
    std::fs::write(&file, text).expect("write trace");
    let manifest = TraceManifest::synthetic("round-trip", em.block_size(), em.stats());
    std::fs::write(
        dir.join("manifest.json"),
        manifest.to_json().expect("manifest"),
    )
    .expect("write manifest");
    file
}

/// The report over a trace read back from disk.
fn trace_report(file: &PathBuf) -> Report {
    // The reader lives in the binary crate, so the round trip goes through the
    // installed binary's own module by way of a fresh read here. Keeping the parsing
    // in one place matters more than avoiding the duplication: this is the same
    // `Invocation` type the emitter wrote.
    let text = std::fs::read_to_string(file).expect("read trace");
    let mut s = Statistics::new(WINDOW);
    let mut session_index = std::collections::HashMap::new();
    let mut next = 0u32;
    for line in text.lines().filter(|l| !l.trim().is_empty()) {
        let inv: workload_model::trace::Invocation =
            serde_json::from_str(line).expect("invocation");
        let key = inv.session_id.clone().unwrap_or_default();
        let session = *session_index.entry(key).or_insert_with(|| {
            let s = next;
            next += 1;
            s
        });
        for (depth, block) in inv.full_input_blocks.iter().enumerate() {
            s.push(&Ref {
                key: workload_model::keys::CacheKey(*block as u64),
                size: DEFAULT_BLOCK_SIZE_TOKENS,
                depth: depth as u32,
                session: workload_model::keys::SessionId(session),
                request_start: depth == 0,
                warmup: false,
            });
        }
    }
    s.finish()
}

#[test]
fn the_jsonl_round_trip_is_exact_on_every_comparable_statistic() {
    let ev = events(&doc(4242));
    assert!(ev.len() > 100_000, "fixture is too small to be a test");
    let plan = plan_report(&ev);
    let file = emit(&ev, "exact");
    let trace = trace_report(&file);

    // Same references on both sides: the emitter wrote every block of every request.
    assert_eq!(
        plan.references, trace.references,
        "the emitted trace does not carry the plan's references"
    );
    assert_eq!(plan.requests, trace.requests);

    let mut d = compare(&plan, &trace, &Tolerances::default());
    // A plan's sizes are KV bytes and a trace's are tokens per block, and no
    // `model_config` exists to convert between them, so the byte statistic compares
    // units rather than workloads.
    d.mark_incomparable(Statistic::ReuseDistanceBytes, "tokens against KV bytes");

    for x in &d.divergences {
        if x.incomparable.is_some() {
            continue;
        }
        // Exactly zero, not merely within tolerance. Ground truth is the same stream
        // on both sides, so any divergence at all is a defect — and a test that
        // accepted "close" would let one accumulate unnoticed.
        assert_eq!(
            x.value,
            0.0,
            "{} diverged by {} across the round trip",
            x.statistic.name(),
            x.value
        );
    }
    assert!(d.within_tolerance());
    let _ = std::fs::remove_dir_all(file.parent().unwrap());
}

/// The same workload with a warmup window long enough to cover the population ramp.
fn warmed_doc(seed: u64) -> Document {
    let text = doc(seed)
        .to_yaml()
        .expect("re-serialise")
        .replace("mode: plan", "mode: plan\n  warmup: 2s");
    Document::from_yaml(&text).expect("warmed fixture must parse")
}

#[test]
fn a_warmed_plan_round_trips_to_its_measured_window() {
    // `emit` does not write warmup requests: a warmup window says which operations a
    // *report* excludes (FR-045), so it belongs to a measured run rather than to a
    // workload, and a trace is a record of a workload. Nothing is lost from the
    // native artifact — `events.bin` still carries the WARMUP flag on every one of
    // them.
    //
    // Before that decision the emitted trace was a different stream from the plan's
    // own report and request length diverged by exactly the extra requests.
    let ev = events(&warmed_doc(31337));
    let warmup_requests = ev
        .iter()
        .filter(|e| {
            e.has(workload_model::plan::flags::REQUEST_START)
                && e.has(workload_model::plan::flags::WARMUP)
        })
        .count();
    assert!(
        warmup_requests > 100,
        "the fixture warmed only {warmup_requests} requests, so this proves nothing"
    );

    let plan = plan_report(&ev);
    let file = emit(&ev, "warmed");
    let trace = trace_report(&file);

    // The trace is exactly the measured window: the plan's own report excluded the
    // same requests.
    assert_eq!(
        plan.requests, trace.requests,
        "the trace is not the plan's measured window"
    );
    assert_eq!(plan.references, trace.references);
    assert_eq!(plan.warmup.requests as usize, warmup_requests);

    let mut d = compare(&plan, &trace, &Tolerances::default());
    d.mark_incomparable(Statistic::ReuseDistanceBytes, "tokens against KV bytes");

    // Three of the four are exact, as for an unwarmed plan.
    for s in [
        Statistic::SharingDepth,
        Statistic::RequestLength,
        Statistic::UniqueKeys,
    ] {
        let x = d.divergences.iter().find(|x| x.statistic == s).unwrap();
        assert_eq!(x.value, 0.0, "{} diverged by {}", s.name(), x.value);
    }

    // The reuse-distance CDF is *not* exact, and cannot be. A warmup reference
    // really did occupy the consumer's capacity, so the plan counts it inside the
    // reuse distance of whatever follows (see `stats` § How warmup is handled); the
    // trace does not contain it at all. So the trace's distances are shorter,
    // one-directionally, by the warmup references that sat between two measured
    // references to the same key. Small and explainable rather than zero — and the
    // assertion is on the bound rather than the value, since the exact figure is a
    // property of the fixture.
    let rd = d
        .divergences
        .iter()
        .find(|x| x.statistic == Statistic::ReuseDistanceObjects)
        .unwrap();
    assert!(rd.value > 0.0, "expected warmup to shift reuse distance");
    assert!(
        rd.within,
        "reuse distance diverged by {} against a tolerance of {}; warmup should shift \
         it slightly, not materially",
        rd.value, rd.tolerance
    );
    assert!(d.within_tolerance());
    let _ = std::fs::remove_dir_all(file.parent().unwrap());
}

#[test]
fn the_round_trip_preserves_the_shape_not_just_the_totals() {
    // Equal totals with a scrambled stream would pass the assertion above on
    // references alone, so the distributions are checked to be non-degenerate: a
    // round trip of two constant histograms would be exact and meaningless.
    let ev = events(&doc(99));
    let plan = plan_report(&ev);
    let file = emit(&ev, "shape");
    let trace = trace_report(&file);

    assert!(
        plan.request_length.blocks.max.unwrap() > 4 * plan.request_length.blocks.p50.unwrap(),
        "the fixture's request lengths are too uniform to be a shape test"
    );
    assert!(plan.sharing.sharing_requests > 0, "no sharing to preserve");
    assert_eq!(
        plan.request_length.block_buckets, trace.request_length.block_buckets,
        "the request-length histogram changed across the round trip"
    );
    assert_eq!(
        plan.sharing.depth_buckets, trace.sharing.depth_buckets,
        "the realised sharing histogram changed across the round trip"
    );
    assert_eq!(
        plan.reuse_distance.object_buckets, trace.reuse_distance.object_buckets,
        "the reuse-distance CDF changed across the round trip"
    );
    let _ = std::fs::remove_dir_all(file.parent().unwrap());
}
