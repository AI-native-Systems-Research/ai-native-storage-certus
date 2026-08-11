//! T042 / SC-004: the cost of characterising a plan.
//!
//! SC-004 is a hard claim with a number in it — `report` computes **every**
//! FR-034a statistic over a 10^7-event plan in under one minute on a single core,
//! "so that characterising a workload is never the reason not to". That makes it a
//! measurement rather than a design intention, and one worth keeping honest,
//! because the reuse-distance CDF is the expensive statistic and it is the one
//! most likely to be replaced by an estimator later (task T076). An exact
//! implementation that fits the budget is the strongest argument for not
//! estimating at all.
//!
//! Three things measured, because they answer different questions:
//!
//! - `sc_004_ten_million` — the claim itself, on a plan of realistic shape, with
//!   an assertion that fails the bench if the budget is blown.
//! - `by_statistic` — where the time goes, one accumulator at a time against all
//!   of them, so a future regression can be attributed instead of guessed at.
//! - `uniform_vs_mixed_sizes` — the cost of the byte-distance Fenwick tree, which
//!   is skipped entirely while every entry is the same size. That shortcut is the
//!   one place the implementation trades generality for speed, so its value is
//!   worth knowing rather than assuming.
//!
//! Run with `cargo bench -p workload-model --bench statistics`.
//!
//! ## Measured, 2026-08-11, one core of this development box (`taskset -c 3`)
//!
//! | Benchmark | Time | Throughput |
//! | --- | --- | --- |
//! | `sc_004_ten_million/all_statistics` (10^7 events) | 1.903 s | 5.26 M events/s |
//! | `by_statistic/key_table_only` (10^6) | 15.3 ms | 65.3 M events/s |
//! | `by_statistic/reuse_distance` (10^6) | 66.0 ms | 15.1 M events/s |
//! | `by_statistic/all_statistics` (10^6) | 113.7 ms | 8.80 M events/s |
//! | `uniform_vs_mixed_sizes/uniform_block_bytes` (10^6) | 113.7 ms | 8.79 M events/s |
//! | `uniform_vs_mixed_sizes/mixed_block_bytes` (10^6) | 160.1 ms | 6.25 M events/s |
//!
//! **SC-004 holds with 31x headroom**: 1.9 s against a one-minute budget. So the
//! exact reuse-distance computation is not merely affordable, it is nowhere near
//! the constraint — which is the useful input to T076, where an estimator would be
//! trading away exactness for a cost that is not currently being paid.
//!
//! **Reuse distance is 45% of the cost** (66 of 114 ms) and the shared key table
//! 13% (15 ms); the six remaining statistics share the other 42%. That is the
//! expected shape — the Fenwick tree is two `O(log n)` traversals per reference
//! against everyone else's `O(1)` — and it is the number to watch if the total
//! ever moves.
//!
//! **The byte-tree shortcut is worth 41%** (8.79 against 6.25 M events/s). Skipping
//! the second Fenwick tree while every entry is the same size costs nothing in
//! exactness — the tree is rebuilt from the object tree, scaled, if a second size
//! ever appears — and `block_bytes: {dist: const, ...}` is the common case, so this
//! is the one place where generality was traded for speed and it paid.
//!
//! **Throughput falls 40% between 10^6 and 10^7 events** (8.80 to 5.26 M/s), which
//! is the key table leaving cache: 10^7 events mint about 1.6 M distinct keys, and
//! neither the table nor the 40 MB Fenwick tree fits in L3. The cost is therefore
//! sublinear-per-event only up to a point, and a 10^8-event plan should be expected
//! to be slower still per event rather than merely ten times slower.

use std::hint::black_box;
use std::time::Instant;

use criterion::{criterion_group, criterion_main, Criterion, Throughput};

use workload_model::plan::{Generator, PlanEvent};
use workload_model::schema::Document;
use workload_model::stats::{Ref, Statistics};

/// The window every measurement uses, so none of them differ in how often the
/// windowed statistics rebuild their tables.
const WINDOW: u64 = 240_000;

/// A plan of realistic shape: a shared trunk, multi-turn sessions, private
/// descents, and a mixture with a short-session arm.
fn doc(events_wanted: u64, block_bytes: &str) -> Document {
    // ~115 blocks a request in this shape, so ask for the requests that gets us
    // there. Overshooting is harmless; the harness truncates.
    let requests = events_wanted / 100;
    let y = format!(
        r#"
version: 1
seed: 424242
requests: {requests}
corpus:
  block_bytes: {block_bytes}
  trees:
    roots: {{count: 12, popularity: {{dist: zipf, s: 0.9}}}}
    shared_depth: {{dist: empirical, points: [[4, 0.10], [18, 0.75], [40, 1.0]]}}
    branching: 1.05
    branch_skew: 0.8
workload:
  arrival: {{model: open_loop, rate: 4000/s, burstiness: 1.8}}
  sessions:
    turns: {{dist: geometric, mean: 6}}
    think_time: {{dist: const, value: 0.05}}
    private_depth: {{dist: lognormal, median: 30, sigma: 0.7}}
    growth_per_turn: {{dist: lognormal, median: 12, sigma: 0.5}}
  mix:
    - {{weight: 0.7}}
    - {{weight: 0.3, turns: {{dist: const, value: 1}}}}
run:
  mode: plan
  wss_window: {WINDOW}
"#
    );
    Document::from_yaml(&y).expect("fixture must parse")
}

/// Pre-generate a plan so that the measurement is of the statistics alone.
///
/// 40 bytes an event, so 10^7 is 400 MB resident — paid once, outside every
/// timed region, because a figure that included generation would be measuring the
/// wrong thing against SC-004's budget.
fn events(target: usize, block_bytes: &str) -> Vec<Ref> {
    let d = doc(target as u64, block_bytes);
    let mut g = Generator::new(&d).expect("generator");
    let mut out: Vec<Ref> = Vec::with_capacity(target);
    let mut chunk: Vec<PlanEvent> = Vec::new();
    while out.len() < target && !g.is_done() {
        chunk.clear();
        if g.fill(&mut chunk) == 0 {
            break;
        }
        out.extend(chunk.iter().map(Ref::from));
    }
    out.truncate(target);
    out
}

/// One full pass, returning the report so nothing can be optimised away.
fn full_pass(refs: &[Ref]) -> u64 {
    let mut s = Statistics::new(WINDOW);
    for r in refs {
        s.push(r);
    }
    let rep = s.finish();
    rep.references + rep.distinct_keys + rep.reuse_distance.first_touches
}

/// SC-004, as an assertion rather than a hope.
fn sc_004(c: &mut Criterion) {
    const N: usize = 10_000_000;
    let refs = events(N, "{dist: const, value: 131072}");
    assert!(
        refs.len() >= N,
        "only generated {} of {N} events",
        refs.len()
    );

    // The claim is about wall-clock on one core, so it is timed once, plainly,
    // before Criterion's sampling begins. A bench that only reported a mean would
    // let SC-004 fail silently on a slow machine.
    let t = Instant::now();
    let checksum = full_pass(&refs);
    let once = t.elapsed();
    println!(
        "SC-004: {N} events in {:.2}s ({:.2} M events/s), checksum {checksum}",
        once.as_secs_f64(),
        N as f64 / once.as_secs_f64() / 1e6
    );
    assert!(
        once.as_secs_f64() < 60.0,
        "SC-004 requires every statistic over 10^7 events in under a minute on one \
         core; this pass took {:.1}s",
        once.as_secs_f64()
    );

    let mut g = c.benchmark_group("sc_004_ten_million");
    g.sample_size(10);
    g.throughput(Throughput::Elements(N as u64));
    g.bench_function("all_statistics", |b| {
        b.iter(|| black_box(full_pass(black_box(&refs))))
    });
    g.finish();
}

/// Where the time goes. Each arm runs the shared key table plus one accumulator,
/// so the differences are attributable.
fn by_statistic(c: &mut Criterion) {
    const N: usize = 1_000_000;
    let refs = events(N, "{dist: const, value: 131072}");

    let mut g = c.benchmark_group("by_statistic");
    g.throughput(Throughput::Elements(N as u64));

    g.bench_function("key_table_only", |b| {
        b.iter(|| {
            let mut t = workload_model::stats::KeyTable::new();
            let mut acc = 0u64;
            for r in &refs {
                acc += t.observe(r).pos;
            }
            black_box(acc)
        })
    });

    g.bench_function("reuse_distance", |b| {
        b.iter(|| {
            let mut t = workload_model::stats::KeyTable::new();
            let mut rd = workload_model::stats::reuse_distance::ReuseDistance::new();
            for r in &refs {
                let f = t.observe(r);
                rd.observe(r, &f);
            }
            black_box(rd.references())
        })
    });

    g.bench_function("all_statistics", |b| {
        b.iter(|| black_box(full_pass(black_box(&refs))))
    });
    g.finish();
}

/// What the byte-distance shortcut is worth.
fn uniform_vs_mixed_sizes(c: &mut Criterion) {
    const N: usize = 1_000_000;
    let uniform = events(N, "{dist: const, value: 131072}");
    let mixed = events(N, "{dist: lognormal, median: 131072, sigma: 0.4}");

    let mut g = c.benchmark_group("uniform_vs_mixed_sizes");
    g.throughput(Throughput::Elements(N as u64));
    g.bench_function("uniform_block_bytes", |b| {
        b.iter(|| black_box(full_pass(black_box(&uniform))))
    });
    g.bench_function("mixed_block_bytes", |b| {
        b.iter(|| black_box(full_pass(black_box(&mixed))))
    });
    g.finish();
}

criterion_group!(benches, sc_004, by_statistic, uniform_vs_mixed_sizes);
criterion_main!(benches);
