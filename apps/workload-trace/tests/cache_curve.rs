//! FR-057d end to end: `fit --cache-curve` over a trace this suite produced itself.
//!
//! The unit tests in `certus-trace`'s own `cache` module pin the two adapters and the
//! sweep. What they cannot reach is the whole path — read a trace, fit a model,
//! regenerate, replay both arms through a real `IEvictionPolicy` component, print — and
//! that path is where a bridge between two grouping conventions goes wrong.
//!
//! The fixture is a trace the generator emitted, which makes this a **self-consistency**
//! check with exact ground truth: the model is fitted from its own output, so both arms
//! describe one workload and their hit-rate curves must very nearly coincide. A curve
//! comparison that cannot agree with itself could never say anything about a real trace.
//! Real traces are not in the repository (see quickstart § 4), so they cannot be tested
//! here at all.
//!
//! It shells out to the binary rather than calling the module, because `cache` lives in
//! a binary crate; that also means the assertions are made against what a reader
//! actually sees, which is the artifact FR-057d specifies.

use std::path::PathBuf;
use std::process::Command;

use workload_model::plan::{Generator, PlanEvent};
use workload_model::schema::Document;
use workload_model::trace::{requests, Emitter, TraceManifest, DEFAULT_BLOCK_SIZE_TOKENS};

/// Small enough to fit and sweep inside a unit-test budget, with real sharing: 12
/// roots, multi-turn sessions, and a mixture so not every session has the same shape.
fn fixture() -> Document {
    Document::from_yaml(
        r#"
version: 1
seed: 4242
requests: 3000
corpus:
  block_bytes: {dist: const, value: 131072}
  trees:
    roots: {count: 12, popularity: {dist: zipf, s: 0.9}}
    shared_depth: {dist: empirical, points: [[4, 0.10], [18, 0.75], [40, 1.0]]}
    branching: 1.02
    branch_skew: 0.9
workload:
  arrival: {model: open_loop, rate: 2000/s, burstiness: 1.8}
  sessions:
    turns: {dist: geometric, mean: 5}
    think_time: {dist: const, value: 0.05}
    private_depth: {dist: lognormal, median: 20, sigma: 0.6}
    growth_per_turn: {dist: lognormal, median: 8, sigma: 0.4}
  mix:
    - {weight: 0.7}
    - {weight: 0.3, turns: {dist: const, value: 1}}
run: {mode: plan, wss_window: 5000}
"#,
    )
    .expect("fixture must parse")
}

/// Emit the fixture as a JSONL trace with its manifest, and return the file path.
fn emit_trace() -> PathBuf {
    let d = fixture();
    let mut g = Generator::new(&d).expect("generator");
    let mut ev: Vec<PlanEvent> = Vec::new();
    let mut chunk = Vec::new();
    while !g.is_done() {
        chunk.clear();
        if g.fill(&mut chunk) == 0 {
            break;
        }
        ev.extend_from_slice(&chunk);
    }
    let dir = std::env::temp_dir().join(format!("certus-cache-curve-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("temp dir");
    let mut em = Emitter::new("cache-curve", DEFAULT_BLOCK_SIZE_TOKENS, 0);
    let mut text = String::new();
    for r in requests(&ev) {
        if let Some(rec) = em.request(r) {
            text.push_str(&serde_json::to_string(&rec).expect("record"));
            text.push('\n');
        }
    }
    let file = dir.join("trace.jsonl");
    std::fs::write(&file, text).expect("write trace");
    std::fs::write(
        dir.join("manifest.json"),
        TraceManifest::synthetic("cache-curve", em.block_size(), em.stats())
            .to_json()
            .expect("manifest"),
    )
    .expect("write manifest");
    file
}

/// One row of the printed curve.
#[derive(Debug)]
struct Row {
    policy: String,
    cache: usize,
    trace: f64,
    plan: f64,
}

/// Parse the curve table out of what a reader sees, so the test is pinned to the
/// report rather than to an internal type.
fn parse_rows(out: &str) -> Vec<Row> {
    out.lines()
        .filter_map(|l| {
            let f: Vec<&str> = l.split_whitespace().collect();
            if f.len() != 6 || !(f[0] == "lru" || f[0] == "session-lists") {
                return None;
            }
            Some(Row {
                policy: f[0].to_string(),
                cache: f[1].parse().ok()?,
                trace: f[3].trim_end_matches('%').parse().ok()?,
                plan: f[4].trim_end_matches('%').parse().ok()?,
            })
        })
        .collect()
}

#[test]
fn the_cache_curve_of_a_self_fitted_trace_agrees_with_itself() {
    let file = emit_trace();
    let out = Command::new(env!("CARGO_BIN_EXE_certus-trace"))
        .args([
            "fit",
            "-t",
            file.to_str().expect("path"),
            "--block-bytes",
            "131072",
            "--wss-window",
            "5000",
            "--no-floor",
            "--cache-curve",
            "--cache-policy",
            "both",
        ])
        .output()
        .expect("run certus-trace");
    let text = String::from_utf8_lossy(&out.stdout).to_string();

    // Printed at all, and printed whatever the verdict was — FR-057d requires the curve
    // even on a refused fit, since "is a rejected model still usable" is the question.
    assert!(
        text.contains("cache curve"),
        "no curve in the report:\n{text}\n{}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(
        text.contains("NOT GATED"),
        "the report must say the curve gates nothing yet"
    );

    let rows = parse_rows(&text);
    assert!(
        rows.iter().any(|r| r.policy == "lru") && rows.iter().any(|r| r.policy == "session-lists"),
        "--cache-policy both must sweep both components, got {} rows",
        rows.len()
    );

    for policy in ["lru", "session-lists"] {
        let mut rs: Vec<&Row> = rows.iter().filter(|r| r.policy == policy).collect();
        rs.sort_by_key(|r| r.cache);
        assert!(rs.len() >= 4, "{policy}: too few points to be a curve");

        // A hit-rate curve is non-decreasing in capacity, on both arms. This is a
        // property of any correct replay — more cache cannot cause a miss that a
        // smaller one avoided under LRU or under leaf-only eviction — so it checks the
        // simulator wiring rather than the model.
        for w in rs.windows(2) {
            assert!(
                w[1].trace >= w[0].trace - 1e-9,
                "{policy}: trace arm fell from {} to {} as capacity grew",
                w[0].trace,
                w[1].trace
            );
            assert!(
                w[1].plan >= w[0].plan - 1e-9,
                "{policy}: synthetic arm fell from {} to {} as capacity grew",
                w[0].plan,
                w[1].plan
            );
        }

        // The point of the fixture: fitted from its own output, the two arms are two
        // samples of one workload, so the curves must nearly coincide. Measured 3.8
        // points at this fixture size, so the 10-point bound carries about 2.6x
        // headroom — loose enough not to pin the fit's search, and far tighter than the
        // 45-point gaps measured against real traces, so a bridge that mismatched
        // order, session identity or request grouping could not pass it.
        let worst = rs
            .iter()
            .map(|r| (r.plan - r.trace).abs())
            .fold(0.0f64, f64::max);
        assert!(
            worst < 10.0,
            "{policy}: a self-fitted trace disagrees with itself by {worst:.1} points"
        );
    }

    let _ = std::fs::remove_dir_all(file.parent().expect("dir"));
}
