//! T024 — why a pool must be seeded from the equilibrium residual-life
//! distribution and never from the lifetime distribution itself (FR-015).
//!
//! This is the Rust counterpart of `research/population/seeding.py`, on the same
//! pool and the same windows, so the two can be compared directly. What is
//! asserted here is the **structure** of the result and not its digits: the
//! reference run reports zero churn in the first three windows and modulation
//! still around 0.44 after eight generations against 0.04 for residual-life
//! seeding, but pinning those numbers would make this test a flake detector for
//! the RNG rather than a check on the seeding rule.
//!
//! The measured quantity is key churn — replacements per window — because that is
//! what a cache actually sees. A pool holding its size perfectly can still be
//! wrong in exactly this way, which is why the population count is not the thing
//! to look at.

use workload_model::description::Population;
use workload_model::distribution::{Distribution, Kind};
use workload_model::pool::{SeedingPolicy, SharedPool};
use workload_model::rng;

/// Pool size, lifetime, and window, matching `research/population/seeding.py`.
const N: u64 = 10;
const MEAN_LIFE: f64 = 10_000.0;
const SIGMA: f64 = 1_000.0;
const WINDOW: f64 = 2_000.0;
const GENERATIONS: usize = 8;
const WINDOWS_PER_GEN: usize = (MEAN_LIFE / WINDOW) as usize;
/// Averaging over seeds is what makes the structure legible rather than noisy;
/// the reference uses 40 and the shape is already unambiguous by 24.
const SEEDS: u64 = 24;

/// Steady-state replacements per window, `N * window / E[L]`.
fn steady_state() -> f64 {
    N as f64 * WINDOW / MEAN_LIFE
}

/// Replacements per window for one seed, under one seeding policy.
fn churn_windows(seed: u64, policy: SeedingPolicy, lifetime: &Kind) -> Vec<f64> {
    let mut r = rng::substream(seed, "pools");
    let life = Distribution::new(lifetime.clone())
        .bounded(Some(1.0), None)
        .resolve(None)
        .unwrap();
    let length = Distribution::new(Kind::Constant { value: 4.0 })
        .resolve_integral(None)
        .unwrap();
    let mut pool = SharedPool::new(0, Population::Exact(N), life, length, &mut r);
    pool.seed_with_policy(0.0, policy, &mut r);

    let windows = GENERATIONS * WINDOWS_PER_GEN;
    let mut out = Vec::with_capacity(windows);
    let mut previous = pool.total_mints();
    for w in 1..=windows {
        pool.advance_to(w as f64 * WINDOW, &mut r);
        let now = pool.total_mints();
        out.push((now - previous) as f64);
        previous = now;
    }
    out
}

/// Per-window churn averaged over `SEEDS` independent runs.
fn mean_churn(policy: SeedingPolicy, lifetime: &Kind) -> Vec<f64> {
    let windows = GENERATIONS * WINDOWS_PER_GEN;
    let mut acc = vec![0.0; windows];
    for seed in 0..SEEDS {
        for (slot, v) in churn_windows(seed, policy, lifetime).iter().enumerate() {
            acc[slot] += v;
        }
    }
    for a in acc.iter_mut() {
        *a /= SEEDS as f64;
    }
    acc
}

/// Peak-to-trough modulation, `(max - min) / (max + min)`.
///
/// Scale-free, so it compares across generations without needing the steady-state
/// rate, and it is zero for a flat row.
fn modulation(row: &[f64]) -> f64 {
    let hi = row.iter().cloned().fold(f64::NEG_INFINITY, f64::max);
    let lo = row.iter().cloned().fold(f64::INFINITY, f64::min);
    if hi + lo == 0.0 {
        0.0
    } else {
        (hi - lo) / (hi + lo)
    }
}

fn generation(row: &[f64], g: usize) -> &[f64] {
    &row[g * WINDOWS_PER_GEN..(g + 1) * WINDOWS_PER_GEN]
}

fn normal_life() -> Kind {
    Kind::Normal {
        mean: MEAN_LIFE,
        sigma: SIGMA,
    }
}

#[test]
fn residual_life_seeding_gives_flat_churn_from_t_zero() {
    // The requirement itself: a pool seeded from equilibrium is already in
    // equilibrium, so the very first window sees the steady-state rate. There is
    // no warm-up period to discard and no transient to wait out.
    let churn = mean_churn(SeedingPolicy::Equilibrium, &normal_life());
    let steady = steady_state();

    let first = generation(&churn, 0);
    let first_mean = first.iter().sum::<f64>() / first.len() as f64;
    assert!(
        (first_mean - steady).abs() < 0.25 * steady,
        "generation 0 churned {first_mean:.2} per window against a steady state of \
         {steady:.2} — seeding is not starting in equilibrium"
    );

    // Every window, not just the mean: a transient hiding inside generation 0
    // would average away.
    for (w, c) in churn.iter().enumerate() {
        assert!(
            *c > 0.35 * steady,
            "window {w} churned {c:.2}, far below the steady state {steady:.2}"
        );
    }

    // And flat: no cohort structure to modulate.
    let m = modulation(first);
    assert!(
        m < 0.35,
        "generation 0 modulation {m:.2} is too structured for an equilibrium seed"
    );
}

#[test]
fn lifetime_seeding_produces_the_cohort_artifact() {
    // The defect FR-015 forbids, measured rather than asserted. A shared birthday
    // means nothing can die until roughly one whole lifetime has passed.
    let churn = mean_churn(SeedingPolicy::Lifetime, &normal_life());
    let steady = steady_state();

    // 1. Dead silence early. The first windows see essentially no churn at all,
    //    which is the part that would quietly invalidate a short run: the cache
    //    would be handed a completely static key space.
    let early = &churn[..3];
    for (w, c) in early.iter().enumerate() {
        assert!(
            *c < 0.1 * steady,
            "window {w} churned {c:.3}; a lifetime-seeded pool should be frozen \
             this early, so the artifact is not being reproduced"
        );
    }

    // 2. Then a burst well above the steady state, as the whole cohort expires
    //    together.
    let peak = churn.iter().cloned().fold(f64::NEG_INFINITY, f64::max);
    assert!(
        peak > 2.0 * steady,
        "peak churn {peak:.2} never exceeded twice the steady state {steady:.2}; \
         there is no cohort burst"
    );

    // 3. And it does not settle. Eight generations later the periodicity is still
    //    there — this is the part that makes the defect dangerous, because a run
    //    long enough to look reasonable is still modulated.
    let last = generation(&churn, GENERATIONS - 1);
    assert!(
        modulation(last) > 0.2,
        "modulation had decayed to {:.2} by generation {}; the reference still \
         shows strong periodicity there",
        modulation(last),
        GENERATIONS - 1
    );
}

#[test]
fn the_two_seedings_differ_by_far_more_than_seed_noise() {
    // Stated as a comparison so the test cannot pass by both arms being broken in
    // the same direction: whatever the digits, the two seedings must be plainly
    // different in the first generation and converge later.
    let equilibrium = mean_churn(SeedingPolicy::Equilibrium, &normal_life());
    let lifetime = mean_churn(SeedingPolicy::Lifetime, &normal_life());

    let m_eq = modulation(generation(&equilibrium, 0));
    let m_lt = modulation(generation(&lifetime, 0));
    assert!(
        m_lt > 3.0 * m_eq.max(0.05),
        "generation-0 modulation was {m_lt:.2} for lifetime seeding against \
         {m_eq:.2} for equilibrium — not the order-of-magnitude gap the \
         reference reports"
    );

    // Both arms must have the same *total* churn over a long run, because that
    // rate is set by N/E[L] and not by the seeding. If they differ, one arm has a
    // population bug and the modulation comparison above is measuring the wrong
    // thing.
    let total_eq: f64 = equilibrium.iter().sum();
    let total_lt: f64 = lifetime.iter().sum();
    assert!(
        (total_eq - total_lt).abs() < 0.2 * total_eq,
        "total churn differs: {total_eq:.1} vs {total_lt:.1}. The seedings must \
         change *when* keys churn, not how many"
    );
}

#[test]
fn an_exponential_lifetime_is_the_control_where_both_seedings_must_agree() {
    // Memorylessness: the equilibrium residual life of Exp(m) is Exp(m), so here
    // the two policies are the same distribution and must produce the same churn.
    //
    // This is the test that catches a broken residual sampler. A missing length
    // bias, for instance, halves the mean residual life, which shows up as an
    // early churn burst here while the normal-lifetime tests above would still
    // pass — they only ask that equilibrium seeding be *flat*, and a wrong-but-
    // flat sampler would satisfy them.
    let kind = Kind::Exponential { mean: MEAN_LIFE };
    let equilibrium = mean_churn(SeedingPolicy::Equilibrium, &kind);
    let lifetime = mean_churn(SeedingPolicy::Lifetime, &kind);
    let steady = steady_state();

    let eq0 = generation(&equilibrium, 0);
    let lt0 = generation(&lifetime, 0);
    let eq_mean = eq0.iter().sum::<f64>() / eq0.len() as f64;
    let lt_mean = lt0.iter().sum::<f64>() / lt0.len() as f64;
    assert!(
        (eq_mean - lt_mean).abs() < 0.3 * steady,
        "the two seedings disagree on an exponential lifetime ({eq_mean:.2} vs \
         {lt_mean:.2} per window in generation 0). Exponential is memoryless, so \
         they must agree — the residual sampler is wrong"
    );

    // Neither arm should show the cohort artifact: there is no mode for a shared
    // birthday to synchronise around.
    for (name, row) in [("equilibrium", &equilibrium), ("lifetime", &lifetime)] {
        let first = generation(row, 0);
        let mean = first.iter().sum::<f64>() / first.len() as f64;
        assert!(
            (mean - steady).abs() < 0.35 * steady,
            "{name} seeding on an exponential lifetime churned {mean:.2} against a \
             steady state of {steady:.2} in generation 0"
        );
    }
}
