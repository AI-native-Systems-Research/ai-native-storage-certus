//! The capacity sweep, and what makes it discriminate (T079).
//!
//! # What is being checked, and what is deliberately not
//!
//! US4's question is whether the *workload* can tell two eviction policies apart. A workload whose
//! hit-rate curve steps — flat, then a cliff, then flat — cannot: every policy scores the same on
//! either side of the cliff. A curve that slopes can, because the policies differ over which
//! blocks they keep while the cache is genuinely too small.
//!
//! So this replays the plan's own reference stream through an LRU simulator at five capacities
//! spanning a hundredfold range. That is **not** Certus, and the check is not about Certus's
//! policy: it is about whether the reference pattern the generator produces is one a policy
//! comparison could be run over at all. A necessary condition for the real measurement, checkable
//! with no server, no accelerator and no cluster — which is why it runs every time rather than on
//! request.
//!
//! # Two things the quickstart's Scenario 6 had wrong, both found by measuring
//!
//! Scenario 6 said to sweep the hit rate a run reports and expected a uniform workload to fail
//! "by construction". It does not, for two independent reasons, and both are now asserted here so
//! that neither can quietly come back.
//!
//! **The overall hit rate is the wrong curve.** The reactive rule (FR-072a) re-offers a session's
//! whole prefix every turn, so most references are a session re-reading what it stored moments ago.
//! Those all miss while the cache is smaller than the concurrent footprint and all hit once it is
//! larger, which is a cliff rather than a slope: measured, the overall curve puts 85% of its rise
//! into a single step, and it does so whether popularity is concentrated or uniform. Swept
//! directly it therefore fails for *both* descriptions, so it cannot be the curve that tells them
//! apart.
//!
//! What popularity governs is **cross-session** reuse: a reference to a block some *other* session
//! brought in, which is exactly the reference whose outcome a replacement decision determines.
//! Those are spread across the popularity distribution rather than concentrated at one footprint,
//! and that curve does separate the two — 40% in its largest step against uniform's 94%.
//!
//! **The ladder must stop well below the key space.** A capacity equal to the key space evicts
//! nothing, so its hit rate is 1.0 for any workload whatsoever; a ladder ending there flatters
//! every shape that reaches it, and the first version of this check passed a uniform workload for
//! that reason alone. It is also the one case where no policy can differ from another, so it is
//! the least informative point available rather than merely an awkward one. The span therefore runs
//! 0.1% to 10% of the key space — still a hundredfold, entirely inside the region where a
//! replacement decision has consequences.
//!
//! # The check has teeth, and that is asserted rather than asserted-about
//!
//! The same check runs against a description with `selection` removed. Under uniform popularity
//! every instance is equally likely, so cross-session hits need a cache holding a fixed fraction
//! of the whole key space and the hit rate rises roughly *in proportion to capacity* — which on a
//! geometric ladder is one dominant step at the top. Under a heavy tail it rises with the
//! *logarithm* of capacity, which is even steps on the same ladder. That difference in shape, not
//! in height, is what the half-the-rise rule detects. A sweep check that passed on both would be
//! measuring nothing, which is the failure mode this project keeps finding, so the negative case is
//! a test of its own rather than a remark in a comment.
//!
//! Both verdicts were checked at five seeds and held at all five, with the largest step steady near
//! 40% of the rise when concentrated and 90-94% when uniform — see
//! `the_verdicts_hold_across_seeds`, which is `#[ignore]`d for runtime. Worth doing rather than
//! assuming: recorded history on this hardware includes n=3 sampling producing conclusions that
//! later measurement reversed.

#![cfg(feature = "live")]

use std::collections::{BTreeMap, HashMap};

use workload_model::description::WorkloadDescription;
use workload_model::plan::{OpKind, OperationPlan};
use workload_model::sim::Simulation;

/// A workload whose shared pool is referenced with a heavy tail.
///
/// `empirical` over a geometric ladder with `interpolate: true` puts equal mass in each octave of
/// rank, which is a power law over instance index — a hot set at every scale rather than one hot
/// set. An `exponential` selection was tried first and produces a single hot band, so its curve
/// steps: concentration alone is not enough, the concentration has to be scale-free.
///
/// The shared pool carries most of the references on purpose — up to eight instances of sixteen
/// blocks per session against a private path that grows two blocks a turn — because a description
/// whose references are mostly private measures private locality however its pool is selected.
///
/// `turns` and `uses.count` are heavy-tailed for a separate reason, found by measuring: with both
/// held constant every session has the *same footprint*, so the aggregate working set has a single
/// characteristic size and the curve cliffs there however popularity is distributed. Scale-free
/// popularity is not sufficient; the working set has to be scale-free too.
const CONCENTRATED: &str = r#"
version: 1
blocks: {tokens: 16, bytes: 32768}
shared_classes:
  documents:
    length: {constant: 16}
    lifetime: {constant: .inf}
    pool:
      size: {exact: 16384}
      rank_by: slot
      selection:
        empirical:
          samples: [1, 4, 16, 64, 256, 1024, 4096, 16384]
          interpolate: true
session_classes:
  chat:
    pool: {size: {exact: 128}}
    uses: [{class: documents, count: {empirical: {samples: [1, 2, 4, 8], interpolate: true}}}]
    turns: {empirical: {samples: [1, 2, 4, 8, 16, 32], interpolate: true}}
    input_growth: {constant: 1}
    output_growth: {constant: 1}
    think_time: {constant: 5}
"#;

/// The same workload with `selection` removed, which is the case that must fail the check.
const UNIFORM: &str = r#"
version: 1
blocks: {tokens: 16, bytes: 32768}
shared_classes:
  documents:
    length: {constant: 16}
    lifetime: {constant: .inf}
    pool:
      size: {exact: 16384}
session_classes:
  chat:
    pool: {size: {exact: 128}}
    uses: [{class: documents, count: {empirical: {samples: [1, 2, 4, 8], interpolate: true}}}]
    turns: {empirical: {samples: [1, 2, 4, 8, 16, 32], interpolate: true}}
    input_growth: {constant: 1}
    output_growth: {constant: 1}
    think_time: {constant: 5}
"#;

const SPAN: f64 = 600.0;
const SEED: u64 = 41;

/// The rise a sweep must show to be worth comparing policies over.
///
/// Without a floor the check passes a curve that barely moves, as long as it moves smoothly — and a
/// hit rate rising four points across a hundredfold capacity range gives two policies almost
/// nothing to differ over however evenly those four points are spread.
const MINIMUM_RISE: f64 = 0.10;

/// Every key a run would offer a cache, in order, with the session that offered it.
///
/// `Check` names the prefix a turn reads, which is what a cache serves; the store operations put
/// blocks in and are not themselves references.
fn references(yaml: &str) -> Vec<(u64, u64)> {
    references_seeded(yaml, SEED)
}

/// The same, at a chosen seed, so a verdict can be shown not to rest on one draw.
fn references_seeded(yaml: &str, seed: u64) -> Vec<(u64, u64)> {
    let d: WorkloadDescription = yaml.parse().unwrap();
    let mut sim = Simulation::new(&d, seed).unwrap();
    let mut plan = OperationPlan::default();
    sim.run_until(SPAN, &mut |s, t| plan.record_turn(s, t));
    let mut out = Vec::new();
    for op in plan.operations() {
        if op.kind() == OpKind::Check {
            let session = op.session();
            out.extend(plan.keys_of(op).iter().map(|k| (session, *k)));
        }
    }
    out
}

/// What one capacity point measured.
#[derive(Clone, Copy)]
struct Point {
    /// Every reference, which is the hit rate an operator reads.
    overall: f64,
    /// Only references to a key some **other** session referenced first — the ones whose outcome
    /// depends on the cache having kept a block across a session boundary.
    cross: f64,
}

/// Replay `refs` through an LRU cache of `capacity` blocks.
///
/// LRU because it is the policy Certus ships and the one whose curve is easiest to reason about —
/// not because the conclusion depends on it. What is being measured is the reference pattern.
///
/// Kept O(log n) per reference rather than the obvious list-and-scan: the sweep replays the whole
/// stream once per capacity point, and a scan per hit made the check take minutes, which is the
/// difference between a check that runs every time and one that runs when someone remembers.
fn replay(refs: &[(u64, u64)], capacity: usize) -> Point {
    if capacity == 0 || refs.is_empty() {
        return Point {
            overall: 0.0,
            cross: 0.0,
        };
    }
    // `recency` orders keys by last use; `at` finds a key's place in it.
    let mut recency: BTreeMap<u64, u64> = BTreeMap::new();
    let mut at: HashMap<u64, u64> = HashMap::with_capacity(capacity * 2);
    // Who referenced each key first. A reference from anyone else is cross-session whether or not
    // the cache still holds the block, so the denominator does not move with capacity — which is
    // what makes the cross-session rate comparable from one rung of the ladder to the next.
    let mut first: HashMap<u64, u64> = HashMap::new();
    let mut clock: u64 = 0;
    let (mut hits, mut cross_hits, mut cross_total) = (0usize, 0usize, 0usize);
    for (session, key) in refs {
        clock += 1;
        let foreign = match first.get(key) {
            Some(owner) => *owner != *session,
            None => {
                first.insert(*key, *session);
                false
            }
        };
        let hit = match at.get(key).copied() {
            Some(when) => {
                recency.remove(&when);
                true
            }
            None => {
                if at.len() == capacity {
                    if let Some((oldest, evicted)) = recency.iter().next().map(|(k, v)| (*k, *v)) {
                        recency.remove(&oldest);
                        at.remove(&evicted);
                    }
                }
                false
            }
        };
        recency.insert(clock, *key);
        at.insert(*key, clock);
        if hit {
            hits += 1;
        }
        if foreign {
            cross_total += 1;
            if hit {
                cross_hits += 1;
            }
        }
    }
    Point {
        overall: hits as f64 / refs.len() as f64,
        cross: if cross_total == 0 {
            0.0
        } else {
            cross_hits as f64 / cross_total as f64
        },
    }
}

/// Distinct keys in a reference stream — the key space the sweep is scaled against.
fn distinct(refs: &[(u64, u64)]) -> usize {
    let mut set = std::collections::HashSet::new();
    for (_, k) in refs {
        set.insert(*k);
    }
    set.len()
}

/// Five capacities spanning a hundredfold range, as a fraction of the key space.
///
/// 0.1% to 10%, for the reason given in the module docs: the top of the ladder must stay inside the
/// region where the cache is too small, or its hit rate is 1.0 by construction and the shape of the
/// curve below it stops mattering.
fn capacities(distinct_keys: usize) -> Vec<usize> {
    let space = distinct_keys.max(1_000) as f64;
    (0..5)
        .map(|i| {
            let frac = 0.001f64 * 100f64.powf(i as f64 / 4.0);
            ((space * frac).round() as usize).max(1)
        })
        .collect()
}

/// The curve, and whether it slopes.
struct Curve {
    points: Vec<(usize, Point)>,
}

impl Curve {
    fn measure(refs: &[(u64, u64)]) -> Self {
        let caps = capacities(distinct(refs));
        Self {
            points: caps.into_iter().map(|c| (c, replay(refs, c))).collect(),
        }
    }

    /// Rises enough to be worth measuring, monotonically, with no single step carrying more than
    /// half the total rise.
    ///
    /// The last clause is the one that matters. A step-shaped curve is still monotone, so
    /// monotonicity alone would pass a workload that discriminates nothing. `pick` chooses which of
    /// the two hit rates is under test — see the module docs on why it is the cross-session one.
    fn discriminates(&self, pick: fn(&Point) -> f64) -> Result<(), String> {
        let first = pick(&self.points.first().ok_or("no points")?.1);
        let last = pick(&self.points.last().ok_or("no points")?.1);
        let rise = last - first;
        if rise <= MINIMUM_RISE {
            return Err(format!(
                "hit rate rose only {:.1} points across a hundredfold capacity range ({first:.3} \
                 to {last:.3}), so there is nothing for two policies to differ over",
                rise * 100.0
            ));
        }
        for w in self.points.windows(2) {
            if pick(&w[1].1) + 1e-9 < pick(&w[0].1) {
                return Err(format!(
                    "hit rate fell from {:.3} at capacity {} to {:.3} at {}",
                    pick(&w[0].1),
                    w[0].0,
                    pick(&w[1].1),
                    w[1].0
                ));
            }
        }
        let mut worst = 0.0f64;
        let mut worst_at = (0usize, 0usize);
        for w in self.points.windows(2) {
            let step = pick(&w[1].1) - pick(&w[0].1);
            if step > worst {
                worst = step;
                worst_at = (w[0].0, w[1].0);
            }
        }
        if worst > rise / 2.0 {
            return Err(format!(
                "one step carried {:.1}% of a {:.1}-point rise (capacity {} to {}), so the curve \
                 steps rather than slopes and every policy scores the same on either side",
                worst / rise * 100.0,
                rise * 100.0,
                worst_at.0,
                worst_at.1
            ));
        }
        Ok(())
    }

    fn render(&self, pick: fn(&Point) -> f64) -> String {
        self.points
            .iter()
            .map(|(c, p)| format!("{c}:{:.3}", pick(p)))
            .collect::<Vec<_>>()
            .join("  ")
    }
}

fn overall(p: &Point) -> f64 {
    p.overall
}

fn cross(p: &Point) -> f64 {
    p.cross
}

#[test]
fn a_concentrated_workload_produces_a_curve_that_slopes() {
    let refs = references(CONCENTRATED);
    assert!(refs.len() > 5_000, "only {} references", refs.len());
    let curve = Curve::measure(&refs);
    eprintln!("concentrated cross-session: {}", curve.render(cross));
    eprintln!("concentrated overall:       {}", curve.render(overall));
    curve
        .discriminates(cross)
        .unwrap_or_else(|e| panic!("a concentrated selection should discriminate: {e}"));
}

#[test]
fn the_same_check_fails_under_uniform_selection() {
    // What proves the check has teeth. Under uniform popularity a cross-session hit needs a cache
    // holding a fixed fraction of the whole key space, so the hit rate rises in proportion to
    // capacity — one dominant step at the top of a geometric ladder.
    let refs = references(UNIFORM);
    assert!(refs.len() > 5_000, "only {} references", refs.len());
    let curve = Curve::measure(&refs);
    eprintln!("uniform cross-session:      {}", curve.render(cross));
    eprintln!("uniform overall:            {}", curve.render(overall));
    let verdict = curve.discriminates(cross);
    assert!(
        verdict.is_err(),
        "the check passed under uniform selection, so it cannot detect a workload that \
         discriminates nothing: {}",
        curve.render(cross)
    );
    eprintln!("uniform correctly rejected: {}", verdict.unwrap_err());
}

#[test]
fn the_overall_hit_rate_is_the_wrong_curve_to_sweep() {
    // Why this file sweeps cross-session reuse rather than the hit rate an operator reads, which is
    // what Scenario 6 originally said to sweep. Asserted rather than remarked: if the overall curve
    // ever becomes able to separate the two workloads, this file is sweeping the harder curve for
    // no reason and should be simplified back.
    let c = Curve::measure(&references(CONCENTRATED));
    let u = Curve::measure(&references(UNIFORM));
    eprintln!("overall concentrated: {}", c.render(overall));
    eprintln!("overall uniform:      {}", u.render(overall));

    // The overall curve fails on the concentrated workload too, so it cannot be the discriminator:
    // a check that rejects the workload it is supposed to accept has no teeth, it has no bite.
    let c_overall = c.discriminates(overall);
    assert!(
        c_overall.is_err(),
        "the overall curve now discriminates on the concentrated workload, so the cross-session \
         curve is no longer needed: {}",
        c.render(overall)
    );
    eprintln!(
        "overall rejected even when concentrated: {}",
        c_overall.unwrap_err()
    );
    assert!(
        u.discriminates(overall).is_err(),
        "the overall curve passed on the uniform workload: {}",
        u.render(overall)
    );

    // And the cross-session curve does separate them, which is the whole reason for the extra work.
    assert!(
        c.discriminates(cross).is_ok() && u.discriminates(cross).is_err(),
        "the cross-session curve no longer separates the two workloads"
    );
}

#[test]
fn the_ladder_stops_below_the_key_space_because_the_top_would_otherwise_be_free() {
    // The other thing measurement corrected. At a capacity equal to the key space nothing is ever
    // evicted, so every repeat reference hits whatever the popularity distribution is — and a
    // uniform workload reaching 1.000 at the top of the ladder passed the shape check for that
    // reason alone. Both halves are asserted: that the top rung is short of the key space, and that
    // the full key space really is the free point that would flatter it.
    let refs = references(UNIFORM);
    let space = distinct(&refs);
    let caps = capacities(space);
    assert_eq!(caps.len(), 5, "five points");
    assert!(
        *caps.last().unwrap() * 4 < space,
        "the top rung ({}) is close enough to the key space ({space}) that eviction barely bites",
        caps.last().unwrap()
    );
    assert!(
        (*caps.last().unwrap() as f64) / (caps[0] as f64) > 90.0,
        "the ladder no longer spans a hundredfold range: {caps:?}"
    );
    let free = replay(&refs, space);
    assert!(
        free.cross > 0.99,
        "a cache the size of the key space should hit essentially everything, got {:.3} — if it \
         does not, this test's reasoning about the top of the ladder is wrong",
        free.cross
    );
}

#[test]
fn the_two_workloads_differ_in_popularity_but_not_in_how_much_they_reference() {
    // The claim underneath the sweep: `selection` changes which instances are picked, not how many
    // references a run makes. If it changed the reference count, the two curves would not be
    // comparable and the teeth test would be detecting the wrong difference.
    let c = references(CONCENTRATED);
    let u = references(UNIFORM);
    eprintln!(
        "references: concentrated {} over {} keys, uniform {} over {} keys",
        c.len(),
        distinct(&c),
        u.len(),
        distinct(&u)
    );
    // Close, not equal: `selection` draws from the same generator as everything else, so adding it
    // shifts the stream and the two runs differ by a fraction of a percent. What matters is that it
    // did not change the *amount* of work — a description referencing half as much would produce a
    // different curve for a reason that has nothing to do with popularity.
    let ratio = c.len() as f64 / u.len() as f64;
    assert!(
        (0.95..1.05).contains(&ratio),
        "selection changed how much the workload references, not just what: {} against {}",
        c.len(),
        u.len()
    );
    let hottest_share = |refs: &[(u64, u64)]| {
        let mut counts: HashMap<u64, usize> = HashMap::new();
        for (_, k) in refs {
            *counts.entry(*k).or_default() += 1;
        }
        let mut v: Vec<usize> = counts.into_values().collect();
        v.sort_unstable_by_key(|n| std::cmp::Reverse(*n));
        v.iter().take(256).sum::<usize>() as f64 / refs.len() as f64
    };
    let (sc, su) = (hottest_share(&c), hottest_share(&u));
    eprintln!("share of references on the 256 hottest keys: {sc:.3} against {su:.3}");
    assert!(
        sc > su * 2.0,
        "concentrated selection did not concentrate references ({sc:.3} against {su:.3})"
    );
    assert!(
        distinct(&c) < distinct(&u),
        "a concentrated selection should touch fewer distinct keys: {} against {}",
        distinct(&c),
        distinct(&u)
    );
}

#[test]
#[ignore = "replays the stream at several seeds; ~5 minutes"]
fn the_verdicts_hold_across_seeds() {
    // A shape verdict resting on one seed is worth very little in this repository: recorded history
    // includes n=3 sampling producing conclusions that later measurement reversed. So both verdicts
    // — concentrated accepted, uniform rejected — are checked at five seeds.
    //
    // Ignored by default because it replays the whole stream fifty times. The gate runs one seed;
    // this is what to run before trusting a change to either description or to the rule.
    let mut accepted = 0;
    let mut rejected = 0;
    for seed in SEED..SEED + 5 {
        let c = Curve::measure(&references_seeded(CONCENTRATED, seed));
        let u = Curve::measure(&references_seeded(UNIFORM, seed));
        let cv = c.discriminates(cross);
        let uv = u.discriminates(cross);
        eprintln!(
            "seed {seed}: concentrated {} [{}]",
            if cv.is_ok() { "accepted" } else { "REJECTED" },
            c.render(cross)
        );
        eprintln!(
            "seed {seed}: uniform      {} [{}]",
            if uv.is_err() { "rejected" } else { "ACCEPTED" },
            u.render(cross)
        );
        if let Err(e) = &cv {
            eprintln!("  concentrated was rejected: {e}");
        }
        accepted += usize::from(cv.is_ok());
        rejected += usize::from(uv.is_err());
    }
    assert_eq!(
        accepted, 5,
        "a concentrated selection was rejected at some seed, so the check is not stable"
    );
    assert_eq!(
        rejected, 5,
        "a uniform selection was accepted at some seed, so the teeth are not stable"
    );
}
