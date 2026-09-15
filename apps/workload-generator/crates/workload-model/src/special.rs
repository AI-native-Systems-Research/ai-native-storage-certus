//! Numerical special functions backing [`crate::distribution`].
//!
//! Private to the crate. Nothing here is a distribution; these are the
//! `erfc`-family primitives that inverse-transform truncation of a normal
//! distribution needs, and they live in their own file because their accuracy is
//! a separate concern from the sampling logic that uses them.
//!
//! # Why these are hand-written
//!
//! Truncation is specified as inverse-transform between `F(min)` and `F(max)`
//! (FR-008), which needs both `Φ` and `Φ⁻¹`. Rust's standard library has
//! neither, and `rand_distr` provides samplers but no inverse CDF, so it would
//! not help. Both functions below were checked against an independent reference
//! implementation over a dense grid before being committed, and the unit tests
//! at the bottom of this file re-check them against pinned reference values,
//! rather than trusting them from their source.
//!
//! # Reproducibility caveat, recorded deliberately
//!
//! These functions call `f64::exp` and `f64::ln`, which are **not** guaranteed
//! bit-identical across libm implementations. FR-034 wants a plan to be
//! byte-identical on any machine, and a 1-ULP difference here could in principle
//! flip a rounded integral draw and change a plan. It cannot affect a *live*
//! run's correctness — cache keys are integer-only splitmix64, and remote agents
//! replay keys rather than re-simulating — so the exposure is limited to
//! reproducing an experiment on a different box. The plan serialisation is the
//! place to detect it: comparing plan digests turns a silent divergence into a
//! visible one.
//!
//! Every numeric table below is transcribed from a published source, and the
//! digits are kept exactly as published even where the last one or two make no
//! difference to the resulting `f64`. Truncating them to satisfy
//! `clippy::excessive_precision` would make the constants no longer diffable
//! against their source, which is the only way to check a transcription of forty
//! magic numbers. The lint is therefore allowed, per table, on that ground.

/// Chebyshev coefficients for `erfc`, from Numerical Recipes 3rd ed.
///
/// Measured accuracy against a reference `erfc` over `x ∈ [-12, 12]`: max
/// relative error 3.5e-14, re-checked by `erfc_matches_reference_values` below.
#[allow(clippy::excessive_precision)]
const ERFC_COF: [f64; 28] = [
    -1.302_653_719_781_709_4,
    6.419_697_923_564_902_6e-1,
    1.947_647_320_418_583_6e-2,
    -9.561_514_786_808_631e-3,
    -9.465_953_444_820_36e-4,
    3.668_394_978_527_61e-4,
    4.252_332_480_690_7e-5,
    -2.027_857_811_253_4e-5,
    -1.624_290_004_647e-6,
    1.303_655_835_58e-6,
    1.562_644_172_2e-8,
    -8.523_809_591_5e-8,
    6.529_054_439e-9,
    5.059_343_495e-9,
    -9.913_641_56e-10,
    -2.273_651_22e-10,
    9.646_791_1e-11,
    2.394_038e-12,
    -6.886_027e-12,
    8.944_87e-13,
    3.130_92e-13,
    -1.127_08e-13,
    3.81e-16,
    7.106e-15,
    -1.523e-15,
    -9.4e-17,
    1.21e-16,
    -2.8e-17,
];

/// `erfc` for non-negative arguments, by the Chebyshev fit above.
fn erfc_nonneg(z: f64) -> f64 {
    let t = 2.0 / (2.0 + z);
    let ty = 4.0 * t - 2.0;
    let mut d = 0.0f64;
    let mut dd = 0.0f64;
    for j in (1..ERFC_COF.len()).rev() {
        let tmp = d;
        d = ty * d - dd + ERFC_COF[j];
        dd = tmp;
    }
    t * (-z * z + 0.5 * (ERFC_COF[0] + ty * d) - dd).exp()
}

/// The complementary error function.
pub(crate) fn erfc(x: f64) -> f64 {
    if x.is_nan() {
        return f64::NAN;
    }
    if x >= 0.0 {
        erfc_nonneg(x)
    } else {
        2.0 - erfc_nonneg(-x)
    }
}

/// Standard-normal CDF, `Φ`.
pub(crate) fn normal_cdf(x: f64) -> f64 {
    if x == f64::NEG_INFINITY {
        return 0.0;
    }
    if x == f64::INFINITY {
        return 1.0;
    }
    // Written as erfc(-x/√2)/2 rather than (1 + erf(x/√2))/2 because the former
    // keeps its relative accuracy in the left tail, where the latter cancels.
    0.5 * erfc(-x / std::f64::consts::SQRT_2)
}

/// Standard-normal PDF, `φ`.
pub(crate) fn normal_pdf(x: f64) -> f64 {
    if !x.is_finite() {
        return 0.0;
    }
    (-0.5 * x * x).exp() / (2.0 * std::f64::consts::PI).sqrt()
}

// Acklam's rational approximation to the probit function. Raw accuracy is ~4e-9
// absolute; one Halley refinement against `normal_cdf` takes it to ~3.5e-13,
// which is what `normal_ppf` returns.
#[rustfmt::skip]
#[allow(clippy::excessive_precision)]
const PROBIT_A: [f64; 6] = [
    -3.969_683_028_665_376e1, 2.209_460_984_245_205e2, -2.759_285_104_469_687e2,
    1.383_577_518_672_69e2, -3.066_479_806_614_716e1, 2.506_628_277_459_239,
];
#[rustfmt::skip]
#[allow(clippy::excessive_precision)]
const PROBIT_B: [f64; 5] = [
    -5.447_609_879_822_406e1, 1.615_858_368_580_409e2, -1.556_989_798_598_866e2,
    6.680_131_188_771_972e1, -1.328_068_155_288_572e1,
];
#[rustfmt::skip]
#[allow(clippy::excessive_precision)]
const PROBIT_C: [f64; 6] = [
    -7.784_894_002_430_293e-3, -3.223_964_580_411_365e-1, -2.400_758_277_161_838,
    -2.549_732_539_343_734, 4.374_664_141_464_968, 2.938_163_982_698_783,
];
#[rustfmt::skip]
#[allow(clippy::excessive_precision)]
const PROBIT_D: [f64; 4] = [
    7.784_695_709_041_462e-3, 3.224_671_290_700_398e-1, 2.445_134_137_142_996,
    3.754_408_661_907_416,
];
/// Boundary between Acklam's tail and central branches.
const PROBIT_SPLIT: f64 = 0.02425;

fn probit_raw(p: f64) -> f64 {
    if p < PROBIT_SPLIT {
        let q = (-2.0 * p.ln()).sqrt();
        tail_ratio(q)
    } else if p <= 1.0 - PROBIT_SPLIT {
        let q = p - 0.5;
        let r = q * q;
        let num = ((((PROBIT_A[0] * r + PROBIT_A[1]) * r + PROBIT_A[2]) * r + PROBIT_A[3]) * r
            + PROBIT_A[4])
            * r
            + PROBIT_A[5];
        let den = ((((PROBIT_B[0] * r + PROBIT_B[1]) * r + PROBIT_B[2]) * r + PROBIT_B[3]) * r
            + PROBIT_B[4])
            * r
            + 1.0;
        num * q / den
    } else {
        let q = (-2.0 * (1.0 - p).ln()).sqrt();
        -tail_ratio(q)
    }
}

fn tail_ratio(q: f64) -> f64 {
    let num = ((((PROBIT_C[0] * q + PROBIT_C[1]) * q + PROBIT_C[2]) * q + PROBIT_C[3]) * q
        + PROBIT_C[4])
        * q
        + PROBIT_C[5];
    let den = (((PROBIT_D[0] * q + PROBIT_D[1]) * q + PROBIT_D[2]) * q + PROBIT_D[3]) * q + 1.0;
    num / den
}

/// Standard-normal inverse CDF, `Φ⁻¹`.
///
/// Returns `-inf` at `p <= 0` and `+inf` at `p >= 1`, so a caller that has
/// already truncated to a finite range never sees an infinity.
pub(crate) fn normal_ppf(p: f64) -> f64 {
    if p.is_nan() {
        return f64::NAN;
    }
    if p <= 0.0 {
        return f64::NEG_INFINITY;
    }
    if p >= 1.0 {
        return f64::INFINITY;
    }
    let x = probit_raw(p);
    // One Halley step. Worth its cost: it removes Acklam's ~4e-9 error, which
    // would otherwise be the accuracy floor of every truncated normal draw.
    let e = normal_cdf(x) - p;
    let u = e * (2.0 * std::f64::consts::PI).sqrt() * (x * x / 2.0).exp();
    let refined = x - u / (1.0 + x * u / 2.0);
    if refined.is_finite() {
        refined
    } else {
        x
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // Reference values from an independent implementation (CPython's `math.erfc`
    // and `statistics.NormalDist.inv_cdf`), not from this code. They are what
    // makes the accuracy claims in the module header checkable rather than
    // asserted.

    #[rustfmt::skip]
    #[allow(clippy::excessive_precision)]
    const ERFC_REF: [(f64, f64); 13] = [
        (-8.0, 2.000_000_000_000_000_0e0),
        (-5.0, 1.999_999_999_998_462_56e0),
        (-3.0, 1.999_977_909_503_001_47e0),
        (-1.5, 1.966_105_146_475_310_76e0),
        (-0.5, 1.520_499_877_813_046_52e0),
        (-0.1, 1.112_462_916_018_284_84e0),
        (0.0, 1.000_000_000_000_000_0e0),
        (0.1, 8.875_370_839_817_151_58e-1),
        (0.5, 4.795_001_221_869_534_81e-1),
        (1.5, 3.389_485_352_468_927_38e-2),
        (3.0, 2.209_049_699_858_543_78e-5),
        (5.0, 1.537_459_794_428_035_14e-12),
        (8.0, 1.122_429_717_298_292_78e-29),
    ];

    #[rustfmt::skip]
    #[allow(clippy::excessive_precision)]
    const PPF_REF: [(f64, f64); 15] = [
        (1e-10, -6.361_340_902_404_055_7e0),
        (1e-06, -4.753_424_308_822_898_68e0),
        (0.001, -3.090_232_306_167_813_19e0),
        (0.01, -2.326_347_874_040_840_76e0),
        (0.02425, -1.972_961_051_311_884_48e0),
        (0.1, -1.281_551_565_544_600_81e0),
        (0.25, -6.744_897_501_960_817_05e-1),
        (0.5, 0.0),
        (0.75, 6.744_897_501_960_817_05e-1),
        (0.9, 1.281_551_565_544_600_81e0),
        (0.97575, 1.972_961_051_311_884_71e0),
        (0.99, 2.326_347_874_040_840_76e0),
        (0.999, 3.090_232_306_167_813_19e0),
        (0.999999, 4.753_424_308_817_089_11e0),
        (0.9999999999, 6.361_340_889_697_420_84e0),
    ];

    #[test]
    fn erfc_matches_reference_values() {
        for (x, want) in ERFC_REF {
            let got = erfc(x);
            let rel = (got - want).abs() / want.abs();
            assert!(
                rel < 1e-13,
                "erfc({x}) = {got:e}, want {want:e}, relative error {rel:e}"
            );
        }
    }

    #[test]
    fn ppf_matches_reference_values() {
        // Deliberately includes p exactly at PROBIT_SPLIT and its mirror, so a
        // mis-placed branch boundary shows up rather than hiding mid-interval.
        //
        // The tolerance is derivative-scaled rather than a flat constant,
        // because Φ⁻¹ has slope 1/φ(x) — about 94 000 at p = 0.999999. One ULP
        // of p there is already 1e-11 in x, so a flat 1e-12 would be asserting
        // something f64 cannot represent, while a flat 1e-9 would let a wrong
        // coefficient through in the central region. This tolerance is tight
        // where the arithmetic is exact and loose only where it cannot be.
        for (p, want) in PPF_REF {
            let got = normal_ppf(p);
            let floor = 4.0 * f64::EPSILON * p.max(1.0 - p) / normal_pdf(want);
            let tol = 1e-12 + floor;
            assert!(
                (got - want).abs() < tol,
                "Φ⁻¹({p}) = {got}, want {want}, error {:e}, tolerance {tol:e}",
                (got - want).abs()
            );
        }
    }

    #[test]
    fn halley_refinement_beats_the_raw_approximation() {
        // If this ever fails, the refinement has stopped doing anything and the
        // accuracy floor has silently risen to Acklam's ~4e-9.
        //
        // Measured in *p-space* — how far Φ(Φ⁻¹(p)) lands from p — which is
        // what the refinement actually minimises, and which unlike x-space is
        // not swamped by the 1/φ(x) blow-up in the far tail.
        let mut worst_raw = 0.0f64;
        let mut worst_refined = 0.0f64;
        for (p, _) in PPF_REF {
            worst_raw = worst_raw.max((normal_cdf(probit_raw(p)) - p).abs() / p);
            worst_refined = worst_refined.max((normal_cdf(normal_ppf(p)) - p).abs() / p);
        }
        assert!(
            worst_refined < worst_raw / 100.0,
            "refinement gained little: raw {worst_raw:e} vs refined {worst_refined:e}"
        );
    }

    #[test]
    fn erfc_endpoints() {
        // erfc(0) is 1 to within one ULP; the Chebyshev fit is not exact there
        // and is not required to be.
        assert!((erfc(0.0) - 1.0).abs() <= f64::EPSILON);
        // The far tail must underflow to a non-negative zero, never to a small
        // negative, which would make a truncated CDF interval negative.
        assert!(erfc(30.0) >= 0.0 && erfc(30.0) < 1e-300);
        assert!((erfc(-30.0) - 2.0).abs() < 1e-15);
    }

    #[test]
    fn cdf_never_leaves_the_unit_interval() {
        // Truncation divides by `F(hi) - F(lo)`, so a CDF that overshoots 1 or
        // undershoots 0 anywhere would produce a negative interval and NaN
        // draws. Checked across the whole representable range, tails included.
        for i in -2000..=2000 {
            let x = i as f64 / 50.0;
            let f = normal_cdf(x);
            assert!((0.0..=1.0).contains(&f), "Φ({x}) = {f} is outside [0, 1]");
        }
        assert_eq!(normal_cdf(f64::NEG_INFINITY), 0.0);
        assert_eq!(normal_cdf(f64::INFINITY), 1.0);
    }

    #[test]
    fn normal_cdf_is_symmetric_about_zero() {
        assert!((normal_cdf(0.0) - 0.5).abs() < 1e-15);
        for x in [0.1f64, 0.5, 1.0, 2.0, 5.0, 8.0] {
            let s = normal_cdf(x) + normal_cdf(-x);
            assert!((s - 1.0).abs() < 1e-14, "Φ({x}) + Φ(-{x}) = {s}");
        }
    }

    #[test]
    fn ppf_inverts_cdf() {
        for i in 1..1000 {
            let p = i as f64 / 1000.0;
            let x = normal_ppf(p);
            assert!(
                (normal_cdf(x) - p).abs() < 1e-13,
                "Φ(Φ⁻¹({p})) = {}",
                normal_cdf(x)
            );
        }
    }

    #[test]
    fn ppf_saturates_outside_the_unit_interval() {
        assert_eq!(normal_ppf(0.0), f64::NEG_INFINITY);
        assert_eq!(normal_ppf(1.0), f64::INFINITY);
        assert_eq!(normal_ppf(-0.5), f64::NEG_INFINITY);
        assert_eq!(normal_ppf(1.5), f64::INFINITY);
    }
}
