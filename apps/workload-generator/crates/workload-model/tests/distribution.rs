//! Distribution tests (T012).
//!
//! The statistical properties here are the ones that are *silently* wrong if
//! the implementation is careless — each of them looks fine in an aggregate
//! summary. Reference values were computed with an independent implementation
//! rather than read off this code.

use rand::Rng;
use workload_model::distribution::{Distribution, Kind};
use workload_model::rng;

const N: usize = 200_000;

fn draws(d: &Distribution, cap: Option<f64>, seed: u64) -> Vec<f64> {
    let r = d.resolve(cap).expect("resolves");
    let mut g = rng::seeded(seed);
    (0..N).map(|_| r.sample(&mut g)).collect()
}

fn mean(xs: &[f64]) -> f64 {
    xs.iter().sum::<f64>() / xs.len() as f64
}

// ---------------------------------------------------------------------------
// Truncation is truncation.
// ---------------------------------------------------------------------------

#[test]
fn truncation_does_not_pile_mass_at_the_bounds() {
    // Clamping would put P = Φ(-1) = 0.1587 of the mass exactly on 4.0 — a
    // sixth of every draw sitting on one value. Truncation puts none there.
    let d = Distribution::new(Kind::Normal {
        mean: 5.0,
        sigma: 1.0,
    })
    .bounded(Some(4.0), Some(6.0));
    let xs = draws(&d, None, 11);

    let at_lo = xs.iter().filter(|x| (**x - 4.0).abs() < 1e-9).count();
    let at_hi = xs.iter().filter(|x| (**x - 6.0).abs() < 1e-9).count();
    assert!(
        at_lo < N / 1000 && at_hi < N / 1000,
        "mass piled at the bounds: {at_lo} at 4.0 and {at_hi} at 6.0 out of {N} — that is clamping"
    );
    assert!(xs.iter().all(|x| (4.0..=6.0).contains(x)), "escaped bounds");
}

#[test]
fn truncated_mean_is_the_truncated_one_not_the_clamped_one() {
    // The two differ by 0.204 here, and both are plausible-looking numbers near
    // 5. Only the asymmetric case separates them: truncated to [4, inf) the
    // mean is 5.2876, whereas clamping at 4 gives 5.0833.
    let d = Distribution::new(Kind::Normal {
        mean: 5.0,
        sigma: 1.0,
    })
    .bounded(Some(4.0), None);
    let r = d.resolve(None).unwrap();
    let eff = r.effective();
    assert!(
        (eff.mean - 5.287_599_971).abs() < 1e-6,
        "closed-form truncated mean is wrong: {}",
        eff.mean
    );
    assert!(
        (eff.mean - 5.083_315_471).abs() > 0.1,
        "closed-form mean matches the CLAMPED mean, so truncation is clamping"
    );

    // And the sampler agrees with the closed form.
    let xs = draws(&d, None, 12);
    assert!(
        (mean(&xs) - 5.287_599_971).abs() < 0.01,
        "sampled mean {} disagrees with the closed form",
        mean(&xs)
    );
}

#[test]
fn truncated_exponential_matches_its_closed_form() {
    for (lo, hi, want) in [
        (2.0, Some(50.0), 11.601_694_186),
        (0.0, None, 10.0),
        // Memorylessness: shifting the lower bound shifts the mean by the same
        // amount, which is a property no clamping implementation reproduces.
        (5.0, None, 15.0),
    ] {
        let d = Distribution::new(Kind::Exponential { mean: 10.0 }).bounded(Some(lo), hi);
        let r = d.resolve(None).unwrap();
        assert!(
            (r.effective().mean - want).abs() < 1e-6,
            "exponential(10) on [{lo}, {hi:?}]: got {}, want {want}",
            r.effective().mean
        );
        let xs = draws(&d, None, 13);
        assert!(
            (mean(&xs) - want).abs() < 0.1,
            "sampled mean {} disagrees with {want}",
            mean(&xs)
        );
    }
}

// ---------------------------------------------------------------------------
// Integral draws.
// ---------------------------------------------------------------------------

#[test]
fn integral_uniform_gives_every_value_equal_weight() {
    // `uniform{min: 0, max: 5}` as a count is SIX equiprobable values. Without
    // the half-unit widening, 0 and 5 would each appear half as often as 1..4 —
    // a defect invisible in the mean, which stays 2.5 either way.
    let d = Distribution::new(Kind::Uniform { min: 0.0, max: 5.0 });
    let r = d.resolve_integral(None).unwrap();
    let mut g = rng::seeded(21);
    let mut seen = [0usize; 6];
    for _ in 0..N {
        let v = r.sample_int(&mut g);
        assert!((0..=5).contains(&v), "count {v} outside 0..=5");
        seen[v as usize] += 1;
    }
    let expect = N as f64 / 6.0;
    for (v, count) in seen.iter().enumerate() {
        let dev = (*count as f64 - expect).abs() / expect;
        assert!(
            dev < 0.05,
            "value {v} appeared {count} times, expected ~{expect:.0} ({:.1}% off) — \
             endpoints at half weight is the classic form of this bug: {seen:?}",
            dev * 100.0
        );
    }
    // The endpoints specifically, stated as its own claim.
    let interior = (seen[1] + seen[2] + seen[3] + seen[4]) as f64 / 4.0;
    let ends = (seen[0] + seen[5]) as f64 / 2.0;
    assert!(
        (ends / interior - 1.0).abs() < 0.05,
        "endpoints are {:.3}x the interior rate, not 1x",
        ends / interior
    );
}

#[test]
fn integral_draws_round_rather_than_truncate_toward_zero() {
    // A `as i64` cast instead of `.round()` would bias every count downward by
    // half a unit, which looks like a slightly small mean rather than a bug.
    let d = Distribution::new(Kind::Uniform { min: 3.0, max: 3.0 });
    let r = d.resolve_integral(None).unwrap();
    let mut g = rng::seeded(22);
    for _ in 0..1_000 {
        assert_eq!(r.sample_int(&mut g), 3);
    }
}

#[test]
fn example_count_statistics_match_the_shipped_description() {
    // `doc_analysis` draws its long_document count from exponential(mean 5,
    // min 1), capped by the pool size. These are the figures FR-003 reports and
    // FR-004 gates on, and they are integral figures: the realised mean of the
    // ROUNDED variable, not the continuous truncated mean.
    let d = Distribution::new(Kind::Exponential { mean: 5.0 }).bounded(Some(1.0), None);

    let at20 = d.resolve_integral(Some(20.0)).unwrap().effective();
    assert_eq!(at20.requested_mean, 5.0);
    assert!(
        (at20.mean - 5.143_508).abs() < 1e-5,
        "pool 20 mean {}",
        at20.mean
    );
    assert!(
        (at20.discarded - 0.018_316).abs() < 1e-5,
        "pool 20 discards {}",
        at20.discarded
    );
    assert!(at20.discarded < 0.05, "pool 20 must pass the 5% gate");

    let at10 = d.resolve_integral(Some(10.0)).unwrap().effective();
    assert!(
        (at10.mean - 3.951_479).abs() < 1e-5,
        "pool 10 mean {}",
        at10.mean
    );
    assert!(
        (at10.discarded - 0.135_335).abs() < 1e-5,
        "pool 10 discards {}",
        at10.discarded
    );
    assert!(at10.discarded > 0.05, "pool 10 must fail the 5% gate");

    // Quantiles after truncation, integral.
    assert_eq!(at20.p50, 4.0);
    assert_eq!(at20.p90, 11.0);
    assert_eq!(at20.p99, 18.0);

    // And the sampler realises the reported mean, so the report is not a
    // separate calculation that happens to look right.
    let r = d.resolve_integral(Some(20.0)).unwrap();
    let mut g = rng::seeded(23);
    let mut total = 0i64;
    for _ in 0..N {
        let v = r.sample_int(&mut g);
        assert!((1..=20).contains(&v), "count {v} outside 1..=20");
        total += v;
    }
    let sampled = total as f64 / N as f64;
    assert!(
        (sampled - 5.143_508).abs() < 0.02,
        "sampled count mean {sampled} disagrees with the reported 5.1435"
    );
}

// ---------------------------------------------------------------------------
// Reproducibility.
// ---------------------------------------------------------------------------

#[test]
fn a_fixed_seed_reproduces_a_draw_sequence_exactly() {
    let d = Distribution::new(Kind::Normal {
        mean: 100.0,
        sigma: 15.0,
    })
    .bounded(Some(1.0), None);
    let r = d.resolve(None).unwrap();

    let seq = |seed: u64| {
        let mut g = rng::seeded(seed);
        (0..1_000).map(|_| r.sample(&mut g)).collect::<Vec<_>>()
    };
    // Bit-for-bit, not approximately: FR-034 wants a recorded seed to reproduce
    // a plan, and a plan is compared byte-wise.
    assert_eq!(seq(5), seq(5));
    // The other half of the claim, and the half that catches a seed which is
    // accepted but not wired through — that failure looks exactly like
    // determinism.
    assert_ne!(seq(5), seq(6));
}

#[test]
fn substreams_are_independent_but_reproducible() {
    let a: u64 = rng::substream(1, "sessions").gen();
    let b: u64 = rng::substream(1, "pools").gen();
    let c: u64 = rng::substream(1, "sessions").gen();
    let d: u64 = rng::substream(2, "sessions").gen();
    assert_ne!(a, b, "two named substreams collided");
    assert_eq!(a, c, "the same name and seed gave a different stream");
    assert_ne!(a, d, "the seed does not reach the substream");
}

// ---------------------------------------------------------------------------
// Empirical.
// ---------------------------------------------------------------------------

/// The bimodal sample set from the shipped example's `tool.length`: eleven
/// observations in 1..12, nine in 98..100, and nothing between.
fn tool_samples() -> Vec<f64> {
    vec![
        1.0, 8.0, 9.0, 9.0, 10.0, 10.0, 10.0, 11.0, 11.0, 12.0, 12.0, 98.0, 99.0, 99.0, 100.0,
        100.0, 100.0, 99.0, 99.0, 98.0,
    ]
}

#[test]
fn discrete_empirical_never_emits_a_value_it_did_not_observe() {
    let samples = tool_samples();
    let d = Distribution::new(Kind::Empirical {
        samples: samples.clone(),
        interpolate: false,
    });
    let xs = draws(&d, None, 31);
    for x in &xs {
        assert!(
            samples.contains(x),
            "discrete empirical produced {x}, which is not an observation"
        );
    }
    // Nothing in the gap the data says is empty.
    assert_eq!(
        xs.iter().filter(|x| **x > 12.0 && **x < 98.0).count(),
        0,
        "discrete empirical put draws in the empty 12..98 gap"
    );
    let r = d.resolve(None).unwrap();
    assert!((r.effective().mean - 49.75).abs() < 1e-9);
}

#[test]
fn interpolated_empirical_fills_the_gap_the_data_says_is_empty() {
    // This is permitted and is why it is not the default for counts: the
    // example's own comment predicts ~5% of draws landing in 12..98, and the
    // piecewise-linear CDF puts exactly 1/19 = 5.263% there.
    let d = Distribution::new(Kind::Empirical {
        samples: tool_samples(),
        interpolate: true,
    });
    let xs = draws(&d, None, 32);
    let in_gap = xs.iter().filter(|x| **x > 12.0 && **x < 98.0).count() as f64 / N as f64;
    assert!(
        (in_gap - 0.052_632).abs() < 0.005,
        "interpolated put {:.4} of its mass in the gap, expected 0.0526",
        in_gap
    );
    let r = d.resolve(None).unwrap();
    assert!(
        (r.effective().mean - 49.710_526).abs() < 1e-5,
        "interpolated mean {}",
        r.effective().mean
    );
    // Still inside the observed range, though.
    assert!(xs.iter().all(|x| (1.0..=100.0).contains(x)));
}

#[test]
fn truncated_discrete_empirical_keeps_only_the_retained_samples() {
    // Capped by the *consumer* rather than by a written bound, so `discarded` is
    // the quantity FR-004 gates on: what a pool size takes away from what the
    // author asked for. A written `max: 50` would report 0% discarded, because
    // the author asked for that.
    let d = Distribution::new(Kind::Empirical {
        samples: tool_samples(),
        interpolate: false,
    });
    let xs = draws(&d, Some(50.0), 33);
    assert!(xs.iter().all(|x| *x <= 12.0), "kept a sample above the cap");
    let r = d.resolve(Some(50.0)).unwrap();
    // Mean of the eleven retained observations.
    let want = (1.0 + 8.0 + 9.0 + 9.0 + 10.0 + 10.0 + 10.0 + 11.0 + 11.0 + 12.0 + 12.0) / 11.0;
    assert!(
        (r.effective().mean - want).abs() < 1e-9,
        "{}",
        r.effective().mean
    );
    // Nine of twenty observations discarded.
    assert!(
        (r.effective().discarded - 9.0 / 20.0).abs() < 1e-9,
        "{}",
        r.effective().discarded
    );

    // A written bound is not a cap: same range, nothing "discarded".
    let written = Distribution::new(Kind::Empirical {
        samples: tool_samples(),
        interpolate: false,
    })
    .bounded(None, Some(50.0));
    let wr = written.resolve(None).unwrap();
    assert_eq!(wr.effective().discarded, 0.0);
    assert!((wr.effective().mean - want).abs() < 1e-9);
}

#[test]
fn empirical_reads_samples_from_a_file() {
    let dir = std::env::temp_dir().join(format!("wm-empirical-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("lengths.txt");
    std::fs::write(&path, "# a comment\n3 1\n2,5\n\n4\n").unwrap();

    let mut d: Distribution =
        serde_yaml::from_str("{empirical: {file: lengths.txt}}").expect("parses");
    // Unloaded, it refuses to resolve rather than silently sampling nothing.
    assert!(d.resolve(None).is_err());
    d.load_sample_files(&dir).expect("loads");
    assert_eq!(
        d.kind(),
        &Kind::Empirical {
            samples: vec![1.0, 2.0, 3.0, 4.0, 5.0],
            interpolate: false,
        }
    );
    assert!((d.resolve(None).unwrap().effective().mean - 3.0).abs() < 1e-12);
    std::fs::remove_dir_all(&dir).ok();
}

// ---------------------------------------------------------------------------
// The sampler agrees with the CDF it claims to invert.
// ---------------------------------------------------------------------------

#[test]
fn sampled_quantiles_match_the_reported_quantiles() {
    // A Kolmogorov-Smirnov-flavoured check across every kind under truncation:
    // if `sample` and `quantile` were inverting different functions, one of them
    // would be wrong and nothing else in this file would notice.
    let cases: Vec<(&str, Distribution)> = vec![
        (
            "uniform",
            Distribution::new(Kind::Uniform {
                min: 1.0,
                max: 100.0,
            }),
        ),
        (
            "normal truncated below",
            Distribution::new(Kind::Normal {
                mean: 10_000.0,
                sigma: 1_000.0,
            })
            .bounded(Some(1.0), None),
        ),
        (
            "exponential truncated below",
            Distribution::new(Kind::Exponential { mean: 100.0 }).bounded(Some(1.0), None),
        ),
        (
            "exponential truncated both ends",
            Distribution::new(Kind::Exponential { mean: 10.0 }).bounded(Some(2.0), Some(30.0)),
        ),
        (
            "empirical interpolated",
            Distribution::new(Kind::Empirical {
                samples: tool_samples(),
                interpolate: true,
            }),
        ),
    ];

    for (name, d) in cases {
        let r = d.resolve(None).unwrap();
        let mut xs = draws(&d, None, 41);
        xs.sort_by(|a, b| a.partial_cmp(b).unwrap());
        for p in [0.01, 0.1, 0.25, 0.5, 0.75, 0.9, 0.99] {
            let reported = r.quantile(p);
            // The quantile characterisation that holds whether or not the
            // distribution has atoms: P(X < q) <= p <= P(X <= q). A plain
            // |P(X < q) - p| < tol would be wrong here — the interpolated
            // empirical has an atom of mass 3/19 at 10, so at p = 0.25 only
            // 0.21 of samples fall strictly below the correct quantile.
            let strictly_below = xs.partition_point(|x| *x < reported) as f64 / N as f64;
            let at_or_below = xs.partition_point(|x| *x <= reported) as f64 / N as f64;
            let tol = 0.01;
            assert!(
                strictly_below <= p + tol && at_or_below >= p - tol,
                "{name}: quantile({p}) = {reported}, but P(X < q) = {strictly_below:.4} \
                 and P(X <= q) = {at_or_below:.4} do not bracket {p}"
            );
        }
    }
}

#[test]
fn effective_bounds_are_never_escaped() {
    // Every kind, truncated on both sides, over a lot of draws — the guarantee
    // every consumer relies on, including the salt-field ceilings in `keys`.
    let cases = vec![
        Distribution::new(Kind::Constant { value: 7.0 }),
        Distribution::new(Kind::Uniform {
            min: -3.0,
            max: 9.0,
        }),
        Distribution::new(Kind::Normal {
            mean: 0.0,
            sigma: 100.0,
        })
        .bounded(Some(-1.0), Some(2.0)),
        Distribution::new(Kind::Exponential { mean: 1.0 }).bounded(Some(0.5), Some(0.75)),
        Distribution::new(Kind::Empirical {
            samples: tool_samples(),
            interpolate: true,
        })
        .bounded(Some(9.5), Some(99.5)),
    ];
    for d in cases {
        let r = d.resolve(None).unwrap();
        let (lo, hi) = r.bounds();
        let mut g = rng::seeded(51);
        for _ in 0..50_000 {
            let x = r.sample(&mut g);
            assert!(
                x >= lo && x <= hi,
                "{x} escaped [{lo}, {hi}] for {:?}",
                d.kind()
            );
            assert!(x.is_finite() || lo.is_infinite() || hi.is_infinite());
        }
    }
}

#[test]
fn infinity_is_legal_in_any_numeric_position() {
    // `lifetime: {constant: inf}` is how a description says "never expires", so
    // it must resolve and sample rather than being rejected as degenerate.
    let d: Distribution = serde_yaml::from_str("{constant: inf}").unwrap();
    let r = d.resolve(None).unwrap();
    let mut g = rng::seeded(61);
    assert_eq!(r.sample(&mut g), f64::INFINITY);
    assert_eq!(r.effective().mean, f64::INFINITY);

    // And as an explicit upper bound, where it must be a no-op.
    let bounded: Distribution =
        serde_yaml::from_str("{exponential: {mean: 10, min: 1, max: inf}}").unwrap();
    let unbounded: Distribution =
        serde_yaml::from_str("{exponential: {mean: 10, min: 1}}").unwrap();
    assert_eq!(
        bounded.resolve(None).unwrap().effective(),
        unbounded.resolve(None).unwrap().effective()
    );
}
