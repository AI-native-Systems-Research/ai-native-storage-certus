//! Measure the seed-to-seed divergence floor, for the T075 tolerance derivation.
//!
//! A `fit`/`validate` tolerance has one hard constraint: it must sit above the
//! divergence between two plans that are *the same workload*. Two plans from one
//! document differing only in `seed` are exactly that — same shape, different
//! sample — so the divergence between them is the irreducible floor, and a
//! tolerance below it would fail a model that is correct.
//!
//! This measures that floor for each FR-056 statistic, over several workload shapes
//! and several plan sizes, across all pairs of eight seeds. The size sweep matters
//! because a KS distance's floor falls as `1/sqrt(n)`: a tolerance derived at one
//! plan size would be wrong at another, so the derivation needs the scaling and not
//! just a number.
//!
//! Run with `cargo run --release -p workload-model --example seed_floor`.

use workload_model::plan::{Generator, PlanEvent};
use workload_model::schema::Document;
use workload_model::stats::divergence::{compare, Statistic, Tolerances};
use workload_model::stats::{Ref, Report, Statistics};

/// Seeds per configuration. Eight gives 28 pairs, which is enough for a maximum to
/// mean something without the run taking all day.
const SEEDS: u64 = 8;

/// A document of the given shape, size and seed, optionally warmed.
///
/// `warmup` is the FR-015b question in experimental form: a document whose warmup
/// covers the session-population ramp excludes it from every statistic (FR-045),
/// and one that does not leaves it in the measured window. Since the ramp's
/// composition is seed-dependent by construction, the two cases should have very
/// different floors — and if they do, a tolerance is only meaningful for a document
/// that passes rule 15b.
fn doc_warmed(shape: &str, requests: u64, seed: u64, warmup: Option<&str>) -> Document {
    let base = doc_text(shape, requests, seed);
    let text = match warmup {
        Some(w) => base.replace(
            "run:\n  mode: plan",
            &format!("run:\n  mode: plan\n  warmup: {w}"),
        ),
        None => base,
    };
    Document::from_yaml(&text).unwrap_or_else(|e| panic!("{shape} fixture: {e}"))
}

/// A document of the given shape, size and seed.
fn doc(shape: &str, requests: u64, seed: u64) -> Document {
    Document::from_yaml(&doc_text(shape, requests, seed))
        .unwrap_or_else(|e| panic!("{shape} fixture: {e}"))
}

fn doc_text(shape: &str, requests: u64, seed: u64) -> String {
    // Three shapes spanning the corpus taxonomy: a deep-sharing agentic workload, a
    // shallow-sharing chat workload, and a mixture with a scan arm. The floor is
    // shape dependent, so a single-shape derivation would not generalise.
    let (roots, shared, private, turns, growth, mix) = match shape {
        "agentic" => (
            "12",
            "{dist: empirical, points: [[4, 0.10], [18, 0.75], [40, 1.0]]}",
            "{dist: lognormal, median: 30, sigma: 0.7}",
            "{dist: geometric, mean: 6}",
            "{dist: lognormal, median: 12, sigma: 0.5}",
            "    - {weight: 1.0}\n",
        ),
        "chat" => (
            "4000",
            "{dist: const, value: 2}",
            "{dist: lognormal, median: 8, sigma: 0.8}",
            "{dist: geometric, mean: 3}",
            "{dist: lognormal, median: 6, sigma: 0.5}",
            "    - {weight: 1.0}\n",
        ),
        _ => (
            "12",
            "{dist: empirical, points: [[4, 0.10], [18, 0.75], [40, 1.0]]}",
            "{dist: lognormal, median: 30, sigma: 0.7}",
            "{dist: geometric, mean: 6}",
            "{dist: lognormal, median: 12, sigma: 0.5}",
            "    - {weight: 0.70}\n    - {weight: 0.25, turns: {dist: const, value: 1}}\n    \
             - {weight: 0.05, turns: {dist: const, value: 1}, private_depth: {dist: const, \
             value: 400}}\n",
        ),
    };
    let y = format!(
        r#"
version: 1
seed: {seed}
requests: {requests}
corpus:
  block_bytes: {{dist: lognormal, median: 131072, sigma: 0.3}}
  trees:
    roots: {{count: {roots}, popularity: {{dist: zipf, s: 0.9}}}}
    shared_depth: {shared}
    branching: 1.02
    branch_skew: 0.9
workload:
  arrival: {{model: open_loop, rate: 4000/s, burstiness: 1.8}}
  sessions:
    turns: {turns}
    think_time: {{dist: const, value: 0.05}}
    private_depth: {private}
    growth_per_turn: {growth}
  mix:
{mix}run:
  mode: plan
  wss_window: 20000
"#
    );
    y
}

/// The report for one document.
fn report(d: &Document) -> Report {
    let mut g = Generator::new(d).expect("generator");
    let mut s = Statistics::new(20_000);
    let mut chunk: Vec<PlanEvent> = Vec::new();
    while !g.is_done() {
        chunk.clear();
        if g.fill(&mut chunk) == 0 {
            break;
        }
        for e in &chunk {
            s.push(&Ref::from(e));
        }
    }
    s.finish()
}

fn main() {
    let stats = [
        Statistic::ReuseDistanceObjects,
        Statistic::ReuseDistanceBytes,
        Statistic::SharingDepth,
        Statistic::RequestLength,
        Statistic::UniqueKeys,
    ];
    let tol = Tolerances::default();

    println!(
        "seed-to-seed divergence floor: {SEEDS} seeds, {} pairs per configuration\n",
        SEEDS * (SEEDS - 1) / 2
    );
    println!(
        "{:9} {:>8} {:>9} {:>9}  max divergence per statistic",
        "shape", "requests", "refs", "measure"
    );
    let names: Vec<&str> = stats.iter().map(|s| s.name()).collect();
    println!("{:38}  {}", "", names.join("  "));

    for shape in ["agentic", "chat", "mixed"] {
        for requests in [2_000u64, 10_000, 50_000] {
            let reports: Vec<Report> = (0..SEEDS)
                .map(|s| report(&doc(shape, requests, 1000 + s)))
                .collect();
            let refs = reports[0].references;
            let mut worst = vec![0.0f64; stats.len()];
            for i in 0..reports.len() {
                for j in i + 1..reports.len() {
                    let d = compare(&reports[i], &reports[j], &tol);
                    for (k, s) in stats.iter().enumerate() {
                        let v = d
                            .divergences
                            .iter()
                            .find(|x| x.statistic == *s)
                            .map(|x| x.value)
                            .unwrap_or(0.0);
                        worst[k] = worst[k].max(v);
                    }
                }
            }
            let cells: Vec<String> = worst
                .iter()
                .zip(names.iter())
                .map(|(v, n)| format!("{:>width$.4}", v, width = n.len()))
                .collect();
            println!(
                "{shape:9} {requests:>8} {refs:>9} {:>9}  {}",
                "max",
                cells.join("  ")
            );
        }
    }

    // Does excluding the population ramp stabilise the primary statistic? If it
    // does, a tolerance is only meaningful for a document that passes rule 15b, and
    // that is the more useful thing to say than any single number.
    println!("\nthe effect of warmup on the primary statistic (agentic shape, 4 seeds):");
    println!(
        "{:>9} {:>10} {:>12} {:>12} {:>12}",
        "requests", "warmup", "refs", "KS objects", "KS sharing"
    );
    for requests in [10_000u64, 50_000] {
        for warmup in [None, Some("1s")] {
            let reports: Vec<Report> = (0..4)
                .map(|s| report(&doc_warmed("agentic", requests, 3000 + s, warmup)))
                .collect();
            let mut worst_obj = 0.0f64;
            let mut worst_share = 0.0f64;
            for i in 0..reports.len() {
                for j in i + 1..reports.len() {
                    let d = compare(&reports[i], &reports[j], &tol);
                    let get = |s: Statistic| {
                        d.divergences
                            .iter()
                            .find(|x| x.statistic == s)
                            .map(|x| x.value)
                            .unwrap_or(0.0)
                    };
                    worst_obj = worst_obj.max(get(Statistic::ReuseDistanceObjects));
                    worst_share = worst_share.max(get(Statistic::SharingDepth));
                }
            }
            println!(
                "{requests:>9} {:>10} {:>12} {worst_obj:>12.4} {worst_share:>12.4}",
                warmup.unwrap_or("none"),
                reports[0].references
            );
        }
    }

    // Sup against area for the primary statistic. A sup over a CDF with steep
    // regions moves a long way for a small horizontal shift; the area between the
    // curves does not. If the area floor scales cleanly where the sup floor does
    // not, the sup is the wrong measure for this statistic.
    println!("\nsup (KS) against area (L1) for the primary statistic, all shapes, 4 seeds:");
    println!(
        "{:9} {:>9} {:>11} {:>10} {:>10} {:>8}",
        "shape", "requests", "refs", "KS", "L1", "KS/L1"
    );
    for shape in ["agentic", "chat", "mixed"] {
        for requests in [10_000u64, 50_000, 250_000] {
            let reports: Vec<Report> = (0..4)
                .map(|s| report(&doc(shape, requests, 4000 + s)))
                .collect();
            let mut ks = 0.0f64;
            let mut l1 = 0.0f64;
            for i in 0..reports.len() {
                for j in i + 1..reports.len() {
                    let (a, b) = (&reports[i].reuse_distance, &reports[j].reuse_distance);
                    ks = ks.max(workload_model::stats::divergence::ks_from_buckets(
                        &a.object_buckets,
                        a.references,
                        &b.object_buckets,
                        b.references,
                    ));
                    l1 = l1.max(workload_model::stats::divergence::l1_from_buckets(
                        &a.object_buckets,
                        a.references,
                        &b.object_buckets,
                        b.references,
                    ));
                }
            }
            println!(
                "{shape:9} {requests:>9} {:>11} {ks:>10.4} {l1:>10.4} {:>8.1}",
                reports[0].references,
                if l1 > 0.0 { ks / l1 } else { 0.0 }
            );
        }
    }

    // The 1/sqrt(n) check: a KS floor that scales that way is sampling noise, and a
    // tolerance can be stated once for a stated minimum sample size. One that does
    // not scale is a real difference between seeds and would be a generator defect.
    println!("\n1/sqrt(n) scaling of the primary statistic (agentic shape):");
    let mut prev: Option<(u64, f64)> = None;
    for requests in [2_000u64, 10_000, 50_000, 250_000] {
        let reports: Vec<Report> = (0..4)
            .map(|s| report(&doc("agentic", requests, 2000 + s)))
            .collect();
        let mut worst = 0.0f64;
        for i in 0..reports.len() {
            for j in i + 1..reports.len() {
                let d = compare(&reports[i], &reports[j], &tol);
                worst = worst.max(
                    d.divergences
                        .iter()
                        .find(|x| x.statistic == Statistic::ReuseDistanceObjects)
                        .map(|x| x.value)
                        .unwrap_or(0.0),
                );
            }
        }
        let refs = reports[0].references;
        let note = match prev {
            Some((pr, pv)) if worst > 0.0 => {
                let observed = pv / worst;
                let predicted = ((refs as f64) / (pr as f64)).sqrt();
                format!("  ratio {observed:.2}x against sqrt(n) prediction {predicted:.2}x")
            }
            _ => String::new(),
        };
        println!("  requests {requests:>7}  references {refs:>9}  KS {worst:.5}{note}");
        prev = Some((refs, worst));
    }
}
