//! T037 / FR-037: generation throughput, as a measurement rather than a claim.
//!
//! FR-037 says the generator must not be the bottleneck. That is a quantitative
//! statement, so it needs a number: what matters is **events per second on one
//! core**, against the rate a consumer could plausibly ask for. A four-node
//! measurement at 4000 requests/s and ~150 blocks a request needs roughly 600k
//! events/s; the interesting question is how much headroom one generating core
//! has over that.
//!
//! Four things are measured separately, because they answer different questions:
//!
//! - `steady_state` — the number FR-037 is about, with the buffer already warm.
//! - `by_horizon` — whether the look-ahead depth matters, which is the parameter
//!   FR-021f requires reported. A flat curve is the useful result: it means an
//!   unbounded run can afford a short horizon.
//! - `write` — generation *plus* the writer's invariant checks and hashing, since
//!   `plan` pays both and a claim about generation alone would be optimistic.
//! - `deep_paths` — a scan-shaped session with thousands of private blocks, where
//!   per-event cost is nearly all key derivation rather than session bookkeeping.
//!
//! Run with `cargo bench -p workload-model --bench generation`.
//!
//! ## Measured, 2026-08-11, one core of this development box
//!
//! | Benchmark | Throughput |
//! | --- | --- |
//! | `steady_state` | 5.71 M events/s |
//! | `generation_horizon` 1Ki / 8Ki / 64Ki / 256Ki | 5.68 / 5.68 / 5.67 / 5.67 M events/s |
//! | `generation_and_write/into_sink` | 4.15 M events/s |
//! | `deep_paths/private_depth_4000` | 5.94 M events/s |
//!
//! Three readings, and the second is the one that was not obvious in advance.
//!
//! **FR-037 holds with roughly 9x headroom** — 5.7 M events/s against the ~600k a
//! four-node measurement at 4000 requests/s asks for; ~7x once the writer's
//! checks and both hashes are paid, which is the honest figure for `plan`.
//!
//! **Throughput is flat in the horizon across two orders of magnitude** (0.2%
//! spread over a 256x range). So the look-ahead is not a throughput parameter at
//! all, and an unbounded run may pick a *small* horizon purely to bound its own
//! memory — the tension FR-021f describes between "too short makes the generator
//! the bottleneck" and "too long is what an unbounded run cannot afford" does not
//! bite anywhere in this range. That is a measurement rather than an assumption,
//! and it is why the number is recorded here.
//!
//! **Deep paths are marginally *cheaper* per event** (5.94 vs 5.71), because a
//! 4000-block request amortises the per-turn session bookkeeping — heap pop and
//! push, mixture draw, arrival draw — over far more keys. Per-event cost is
//! dominated by key derivation, which is one blake3 compression apiece.

use criterion::{criterion_group, criterion_main, Criterion, Throughput};

use workload_model::plan::{Generator, PlanEvent, PlanWriter};
use workload_model::schema::Document;

/// A document shaped like the worked example, budgeted in **blocks** so that
/// every iteration does the identical amount of work.
fn doc(blocks: u64, private_depth: u32) -> Document {
    let y = format!(
        r#"
version: 1
seed: 0xC0FFEE
blocks: {blocks}
corpus:
  block_bytes: {{dist: lognormal, median: 131072, sigma: 0.3}}
  trees:
    roots: {{count: 12, popularity: {{dist: zipf, s: 0.9}}}}
    shared_depth: {{dist: empirical, points: [[4, 0.10], [18, 0.75], [40, 1.0]]}}
    branching: 1.02
    branch_skew: 0.9
workload:
  arrival: {{model: open_loop, rate: 4000/s, burstiness: 1.8}}
  sessions:
    turns: {{dist: geometric, mean: 6}}
    think_time: {{dist: lognormal, median: 3, sigma: 1.1}}
    private_depth: {{dist: const, value: {private_depth}}}
    growth_per_turn: {{dist: lognormal, median: 6, sigma: 0.5}}
topology:
  nodes: [node2, node7, node9, node11]
run:
  mode: hardware
  wss_window: 240000
"#
    );
    Document::from_yaml(&y).expect("benchmark fixture must parse")
}

/// Generate the whole budget into a reused buffer, returning the event count.
fn generate(d: &Document, horizon: usize, buf: &mut Vec<PlanEvent>) -> u64 {
    let mut g = Generator::with_horizon(d, horizon).expect("generator");
    while g.fill(buf) > 0 {}
    g.events_emitted()
}

const EVENTS: u64 = 1 << 20;

fn steady_state(c: &mut Criterion) {
    let d = doc(EVENTS, 8);
    let mut buf: Vec<PlanEvent> = Vec::new();
    // Warm the buffer outside the timing loop: that allocation happens once per
    // process in real use, so charging it to every iteration would understate
    // steady-state throughput.
    generate(&d, 64 * 1024, &mut buf);

    let mut g = c.benchmark_group("generation");
    g.throughput(Throughput::Elements(EVENTS));
    g.bench_function("steady_state", |b| {
        b.iter(|| generate(&d, 64 * 1024, &mut buf))
    });
    g.finish();
}

fn by_horizon(c: &mut Criterion) {
    let d = doc(EVENTS, 8);
    let mut g = c.benchmark_group("generation_horizon");
    g.throughput(Throughput::Elements(EVENTS));
    // 1Ki to 256Ki: two orders of magnitude either side of the default.
    for horizon in [1024usize, 8192, 64 * 1024, 256 * 1024] {
        let mut buf: Vec<PlanEvent> = Vec::new();
        generate(&d, horizon, &mut buf);
        g.bench_function(format!("{horizon}"), |b| {
            b.iter(|| generate(&d, horizon, &mut buf))
        });
    }
    g.finish();
}

fn write(c: &mut Criterion) {
    let d = doc(EVENTS, 8);
    let yaml = d.to_yaml().unwrap();
    let mut g = c.benchmark_group("generation_and_write");
    g.throughput(Throughput::Elements(EVENTS));
    // To a sink rather than to a file: what is measured is the writer's per-event
    // cost — the invariant checks, the record encode, the content hash and the
    // stream digest — not the filesystem underneath it.
    g.bench_function("into_sink", |b| {
        b.iter(|| {
            let mut gen = Generator::new(&d).expect("generator");
            let mut w = PlanWriter::new(std::io::sink());
            let mut buf = Vec::new();
            while gen.fill(&mut buf) > 0 {
                w.push_all(&buf).expect("format promise broken");
            }
            w.finish(&yaml).expect("finish")
        })
    });
    g.finish();
}

fn deep_paths(c: &mut Criterion) {
    // A scan-shaped session: 4000 private blocks a request, so one request exceeds
    // a small horizon and per-event cost is nearly all key derivation.
    let d = doc(EVENTS, 4000);
    let mut buf: Vec<PlanEvent> = Vec::new();
    generate(&d, 64 * 1024, &mut buf);
    let mut g = c.benchmark_group("generation_deep_paths");
    g.throughput(Throughput::Elements(EVENTS));
    g.bench_function("private_depth_4000", |b| {
        b.iter(|| generate(&d, 64 * 1024, &mut buf))
    });
    g.finish();
}

criterion_group!(benches, steady_state, by_horizon, write, deep_paths);
criterion_main!(benches);
