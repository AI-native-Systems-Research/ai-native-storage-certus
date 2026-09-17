//! T036 — per-key plan-generation cost.
//!
//! This is the measurement Principle I's "the generator is never the bottleneck"
//! claim rests on, and it is the only claim in this crate that cannot be made by a
//! test. The number to compare against is Certus's own per-key cost, which
//! measurement on this hardware puts at roughly **5.6 µs/key**: if plan generation
//! is a small fraction of that, one generator thread can keep the lanes fed and
//! FR-037's plan-queue depth stays a formality rather than a limit.
//!
//! Reported **per key reference**, not per operation or per turn, because that is
//! the quantity the comparison is against and the only one that stays meaningful
//! across descriptions with different session lengths.
//!
//! Three shapes, because the cost per key is not constant across them:
//!
//! - `short_sessions` — 5 turns. Fixed per-turn work is amortised over few keys.
//! - `long_sessions` — 40 turns. Prefixes are long, so the per-key cost should be
//!   *lower*: a turn's read list is a slice of keys already computed, and appending
//!   is O(1), so more keys per turn means the per-turn overhead is spread further.
//! - `many_shared` — a large shared pool with a wide draw, which is where selection
//!   and binding cost show up rather than turn mechanics.
//!
//! If the long-session case ever became *more* expensive per key, that would mean
//! something in the turn path had become superlinear — a rehash of the prefix, say,
//! or a copy of the read list — which is exactly the regression this benchmark
//! exists to catch.
//!
//! # Recorded baseline, 2026-09-17 (T085)
//!
//! **Three consecutive runs**, one thread, because one run of this benchmark cannot
//! be trusted — see below. Per key reference, low to high across the three:
//!
//! | Case | Throughput | Per key |
//! | --- | --- | --- |
//! | `short_sessions` (5 turns) | 81.3–87.6 Melem/s | **11.4–12.3 ns** |
//! | `long_sessions` (40 turns) | 175.4–237.3 Melem/s | **4.2–5.7 ns** |
//! | `many_shared` (2000 instances, draw 16) | 183.4–190.6 Melem/s | **5.25–5.45 ns** |
//! | `projection_10k_sessions` | — | **97.5–112 µs** |
//!
//! The prediction above holds in every run: long sessions are **2.0x to 2.9x cheaper
//! per key** than short ones, because per-turn overhead amortises over a longer
//! prefix and nothing in the turn path rescans it.
//!
//! Against Certus's measured ~5.6 µs/key, plan generation costs **0.08% to 0.22%** of
//! the budget — roughly 450x to 1300x of headroom on one thread. That is the
//! quantitative form of Principle I's claim, and it holds with a wide margin whichever
//! end of the spread is taken. `projection_10k_sessions` keeps `validate --until n` an
//! interactive query for a 10 000-session description over a 100 000-second span, by
//! four orders of magnitude.
//!
//! ## Criterion's change verdict is not usable on this benchmark
//!
//! **Run-to-run drift exceeds the effect sizes Criterion declares significant**, so a
//! single run's "performance has improved/regressed (p < 0.05)" says nothing here.
//! Measured across the three identical runs above, with no code change between them:
//! `long_sessions` moved **+21% then +12%**, monotonically, and `short_sessions` moved
//! **−0.6% then +7.0%** — each reported at p = 0.00. The first run of a session is the
//! worst: it produced 13% and 47% outlier rates and a `long_sessions` confidence
//! interval eleven times wider than the next run's.
//!
//! So: run it at least twice and discard the first, treat anything under ~35% on
//! `long_sessions` or ~8% on the others as noise, and for a real comparison use
//! repetition with a stated test rather than Criterion's verdict — the same n ≥ 8
//! rule the README states for hit-rate comparisons, and for the same reason.
//!
//! **The earlier baseline (2026-09-15) is superseded and was stale, not wrong.** It
//! recorded 19.0 ns for `short_sessions` and 7.9 ns for `many_shared`; both are now
//! about 1.5x faster, from the selection work in US4 rather than from anything aimed
//! at this benchmark.

use criterion::{criterion_group, criterion_main, BenchmarkId, Criterion, Throughput};
use workload_model::description::WorkloadDescription;
use workload_model::plan::OperationPlan;
use workload_model::sim::Simulation;

/// Build a description with the given session length and shared-pool shape.
fn description(turns: u32, pool: u32, draw: u32, sessions: u32) -> WorkloadDescription {
    format!(
        r#"
version: 1
blocks: {{tokens: 16, bytes: 32768}}
shared_classes:
  docs:
    length: {{constant: 8}}
    lifetime: {{exponential: {{mean: 5000, min: 1}}}}
    pool: {{size: {{exact: {pool}}}}}
session_classes:
  chat:
    pool: {{size: {{exact: {sessions}}}}}
    uses: [{{class: docs, count: {{constant: {draw}}}}}]
    turns: {{constant: {turns}}}
    input_growth: {{constant: 2}}
    output_growth: {{constant: 1}}
    think_time: {{constant: 10}}
"#
    )
    .parse()
    .expect("the benchmark's own description must parse")
}

/// Run one plan build and return it, so the benchmark measures the whole path:
/// population events, selection, binding, key chaining, and plan recording.
fn build(description: &WorkloadDescription, span: f64) -> OperationPlan {
    let mut sim = Simulation::new(description, 1).expect("valid description");
    let mut plan = OperationPlan::default();
    sim.run_until(span, &mut |s, t| plan.record_turn(s, t));
    plan
}

fn per_key(c: &mut Criterion) {
    let span = 2_000.0;
    let cases = [
        ("short_sessions", description(5, 20, 2, 40)),
        ("long_sessions", description(40, 20, 2, 40)),
        ("many_shared", description(5, 2_000, 16, 40)),
    ];

    let mut group = c.benchmark_group("plan_generation");
    for (name, d) in &cases {
        // Throughput in key references, so Criterion reports time per key directly
        // rather than per plan — the figure the 5.6 µs/key comparison needs.
        let references = build(d, span).key_references() as u64;
        assert!(references > 0, "{name} produced no key references");
        group.throughput(Throughput::Elements(references));
        group.bench_with_input(BenchmarkId::from_parameter(name), d, |b, d| {
            b.iter(|| build(d, span));
        });
    }
    group.finish();
}

/// The projection is offered as an interactive query, so it has its own budget: it
/// must stay far below the cost of the run it is describing, or `validate --until n`
/// stops being a one-second answer.
fn projection(c: &mut Criterion) {
    let d = description(40, 2_000, 16, 10_000);
    c.bench_function("projection_10k_sessions", |b| {
        b.iter(|| workload_model::project::project(&d, 100_000.0, 1).expect("projects"));
    });
}

criterion_group!(benches, per_key, projection);
criterion_main!(benches);
