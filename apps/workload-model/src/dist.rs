//! The one distribution syntax used by every distribution-valued field.
//!
//! A single tagged union (schema design rule 2) so that a mistyped shape is a
//! parse error rather than a silently different model. A bare scalar is sugar
//! for `{dist: const, value: <scalar>}`.
//!
//! ```
//! use workload_model::dist::Dist;
//! let d: Dist = serde_yaml::from_str("{dist: const, value: 4}").unwrap();
//! assert_eq!(d.sample_u64(&mut workload_model::rng::Stream::new(0, 0)), 4);
//! ```
//!
//! Integer-valued draws round half-to-even and clamp to the caller's documented
//! domain. Every clamp is **counted** and surfaced in the plan summary rather
//! than silently applied, because a silently clamped parameter is a model that
//! differs from the document describing it.

use serde::{Deserialize, Serialize};
use std::sync::atomic::{AtomicU64, Ordering};

use crate::rng::Stream;

/// Counts of adjustments applied to drawn values, reported rather than hidden.
///
/// ```
/// use workload_model::dist::Clamps;
/// let c = Clamps::default();
/// assert_eq!(c.total(), 0);
/// ```
#[derive(Debug, Default)]
pub struct Clamps {
    /// Draws pulled up or down into the caller's domain.
    pub domain: AtomicU64,
    /// Draws from a `normal` truncated at zero for a non-negative field.
    pub normal_truncation: AtomicU64,
}

impl Clamps {
    /// Total number of adjustments recorded.
    pub fn total(&self) -> u64 {
        self.domain.load(Ordering::Relaxed) + self.normal_truncation.load(Ordering::Relaxed)
    }
}

/// A distribution-valued field.
///
/// The variants are exactly those in `contracts/workload-schema.md` §
/// Distributions. `deny_unknown_fields` is deliberate: a mistyped parameter
/// must not fall back to a default (spec FR-005).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "dist", rename_all = "snake_case", deny_unknown_fields)]
pub enum Shape {
    /// A fixed value.
    Const { value: f64 },
    /// Inclusive uniform over `[min, max]`.
    Uniform { min: f64, max: f64 },
    /// Truncated at zero for non-negative fields; truncation is counted.
    Normal { mean: f64, stddev: f64 },
    /// The default shape for sizes, lengths and think times.
    Lognormal { median: f64, sigma: f64 },
    /// Mean-parameterised exponential.
    Exponential { mean: f64 },
    /// Discrete; the default for turn counts.
    Geometric { mean: f64 },
    /// `s` is the exponent, `n` the support size.
    Zipf { s: f64, n: Option<u64> },
    /// Heavy tail.
    Pareto { scale: f64, alpha: f64 },
    /// Explicit CDF points, linearly interpolated. What `fit` emits when no
    /// parametric shape fits well.
    Empirical { points: Vec<(f64, f64)> },
}

/// A distribution-valued field, accepting a bare scalar as sugar for `const`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum Dist {
    /// `block_bytes: 128KiB` rather than `{dist: const, value: 131072}`.
    Scalar(f64),
    /// The full tagged form.
    Shaped(Shape),
}

impl Dist {
    /// The shape this field denotes, resolving scalar sugar.
    pub fn shape(&self) -> Shape {
        match self {
            Dist::Scalar(v) => Shape::Const { value: *v },
            Dist::Shaped(s) => s.clone(),
        }
    }

    /// Draw a real value.
    pub fn sample(&self, st: &mut Stream) -> f64 {
        self.sample_counted(st, &Clamps::default())
    }

    /// Draw a real value, recording any truncation in `clamps`.
    pub fn sample_counted(&self, st: &mut Stream, clamps: &Clamps) -> f64 {
        match self.shape() {
            Shape::Const { value } => value,
            Shape::Uniform { min, max } => min + st.next_f64() * (max - min),
            Shape::Normal { mean, stddev } => {
                let v = mean + stddev * standard_normal(st);
                if v < 0.0 {
                    clamps.normal_truncation.fetch_add(1, Ordering::Relaxed);
                    0.0
                } else {
                    v
                }
            }
            Shape::Lognormal { median, sigma } => median * (sigma * standard_normal(st)).exp(),
            Shape::Exponential { mean } => -mean * (1.0 - st.next_f64()).ln(),
            // Mean m of a geometric on {1,2,...} gives p = 1/m.
            Shape::Geometric { mean } => {
                let p = if mean <= 1.0 { 1.0 } else { 1.0 / mean };
                ((1.0 - st.next_f64()).ln() / (1.0 - p).ln()).floor() + 1.0
            }
            Shape::Zipf { s, n } => zipf(st, s, n.unwrap_or(u64::from(u32::MAX))),
            Shape::Pareto { scale, alpha } => scale / (1.0 - st.next_f64()).powf(1.0 / alpha),
            Shape::Empirical { points } => empirical(st.next_f64(), &points),
        }
    }

    /// Draw an integer, rounding half-to-even.
    pub fn sample_u64(&self, st: &mut Stream) -> u64 {
        let v = self.sample(st);
        if v <= 0.0 {
            0
        } else {
            round_half_even(v) as u64
        }
    }

    /// Draw an integer clamped to `[lo, hi]`, counting every clamp.
    pub fn sample_u64_clamped(&self, st: &mut Stream, lo: u64, hi: u64, clamps: &Clamps) -> u64 {
        let raw = self.sample_counted(st, clamps);
        let r = if raw <= 0.0 {
            0
        } else {
            round_half_even(raw) as u64
        };
        if r < lo || r > hi {
            clamps.domain.fetch_add(1, Ordering::Relaxed);
            r.clamp(lo, hi)
        } else {
            r
        }
    }

    /// The distribution's mean, where a closed form exists.
    ///
    /// Used by the derived quantities that must be knowable at plan time —
    /// `sessions_per_window`, the session-population ramp of FR-015b, and the
    /// `branching: auto` closed form — none of which may depend on having
    /// generated anything yet.
    pub fn mean(&self) -> Option<f64> {
        match self.shape() {
            Shape::Const { value } => Some(value),
            Shape::Uniform { min, max } => Some(0.5 * (min + max)),
            Shape::Normal { mean, .. } => Some(mean),
            Shape::Lognormal { median, sigma } => Some(median * (0.5 * sigma * sigma).exp()),
            Shape::Exponential { mean } => Some(mean),
            Shape::Geometric { mean } => Some(mean),
            Shape::Pareto { scale, alpha } if alpha > 1.0 => Some(alpha * scale / (alpha - 1.0)),
            Shape::Empirical { points } => {
                // Midpoint-weighted over the CDF steps.
                let mut prev_p = 0.0;
                let mut acc = 0.0;
                for (v, p) in &points {
                    acc += v * (p - prev_p);
                    prev_p = *p;
                }
                Some(acc)
            }
            // Zipf's mean depends on the support and diverges for s <= 2 as
            // n grows; a caller needing it must say which n it means.
            Shape::Zipf { .. } | Shape::Pareto { .. } => None,
        }
    }

    /// The `q`-quantile, by inverse transform where a closed form exists.
    ///
    /// Needed *before* anything is generated: validation rule 16 tests trunk
    /// occupancy at `p99(shared_depth)`, and the whole point of that rule is to
    /// catch a bad configuration without spending hardware time on it. So it may
    /// not be estimated from a sample of a plan that does not exist yet.
    ///
    /// `None` for `zipf`, for the same reason [`Dist::mean`] is: the answer
    /// depends on a support the shape does not carry.
    pub fn quantile(&self, q: f64) -> Option<f64> {
        let q = q.clamp(0.0, 1.0 - 1e-12);
        match self.shape() {
            Shape::Const { value } => Some(value),
            Shape::Uniform { min, max } => Some(min + q * (max - min)),
            Shape::Normal { mean, stddev } => Some((mean + stddev * probit(q)).max(0.0)),
            Shape::Lognormal { median, sigma } => Some(median * (sigma * probit(q)).exp()),
            Shape::Exponential { mean } => Some(-mean * (1.0 - q).ln()),
            Shape::Geometric { mean } => {
                let p = if mean <= 1.0 { 1.0 } else { 1.0 / mean };
                Some(((1.0 - q).ln() / (1.0 - p).ln()).floor() + 1.0)
            }
            Shape::Pareto { scale, alpha } => Some(scale / (1.0 - q).powf(1.0 / alpha)),
            Shape::Empirical { points } => {
                // The CDF is given explicitly; read it off rather than inverting.
                let mut prev = (points.first().map(|p| p.0).unwrap_or(0.0), 0.0f64);
                let mut out = prev.0;
                for &(v, p) in &points {
                    if q <= p {
                        let span = p - prev.1;
                        let frac = if span <= 0.0 {
                            0.0
                        } else {
                            (q - prev.1) / span
                        };
                        return Some(prev.0 + frac * (v - prev.0));
                    }
                    prev = (v, p);
                    out = v;
                }
                Some(out)
            }
            Shape::Zipf { .. } => None,
        }
    }

    /// The `q`-quantile as a depth. `0` where no closed form exists, which is
    /// what a depth-valued field means by "cannot say".
    pub fn quantile_u32(&self, q: f64) -> u32 {
        match self.quantile(q) {
            Some(v) if v > 0.0 => v.min(f64::from(u32::MAX)) as u32,
            _ => 0,
        }
    }
}

/// Round half-to-even, so a long run of `.5` draws does not drift upward.
fn round_half_even(v: f64) -> f64 {
    let f = v.floor();
    let diff = v - f;
    if (diff - 0.5).abs() < f64::EPSILON {
        if (f as i64) % 2 == 0 {
            f
        } else {
            f + 1.0
        }
    } else {
        v.round()
    }
}

/// Inverse standard normal CDF, Acklam's rational approximation.
///
/// Accurate to ~1e-9, which is far more than a p99 depth needs. The coefficients
/// are quoted at their published precision rather than trimmed to what an `f64`
/// distinguishes, so they can be checked against the source without a
/// transcription step in the way.
#[allow(clippy::excessive_precision)]
fn probit(p: f64) -> f64 {
    const A: [f64; 6] = [
        -3.969683028665376e+01,
        2.209460984245205e+02,
        -2.759285104469687e+02,
        1.383577518672690e+02,
        -3.066479806614716e+01,
        2.506628277459239e+00,
    ];
    const B: [f64; 5] = [
        -5.447609879822406e+01,
        1.615858368580409e+02,
        -1.556989798598866e+02,
        6.680131188771972e+01,
        -1.328068155288572e+01,
    ];
    const C: [f64; 6] = [
        -7.784894002430293e-03,
        -3.223964580411365e-01,
        -2.400758277161838e+00,
        -2.549732539343734e+00,
        4.374664141464968e+00,
        2.938163982698783e+00,
    ];
    const D: [f64; 4] = [
        7.784695709041462e-03,
        3.224671290700398e-01,
        2.445134137142996e+00,
        3.754408661907416e+00,
    ];
    let p = p.clamp(1e-15, 1.0 - 1e-15);
    let plow = 0.02425;
    if p < plow {
        let q = (-2.0 * p.ln()).sqrt();
        return (((((C[0] * q + C[1]) * q + C[2]) * q + C[3]) * q + C[4]) * q + C[5])
            / ((((D[0] * q + D[1]) * q + D[2]) * q + D[3]) * q + 1.0);
    }
    if p > 1.0 - plow {
        let q = (-2.0 * (1.0 - p).ln()).sqrt();
        return -(((((C[0] * q + C[1]) * q + C[2]) * q + C[3]) * q + C[4]) * q + C[5])
            / ((((D[0] * q + D[1]) * q + D[2]) * q + D[3]) * q + 1.0);
    }
    let q = p - 0.5;
    let r = q * q;
    (((((A[0] * r + A[1]) * r + A[2]) * r + A[3]) * r + A[4]) * r + A[5]) * q
        / (((((B[0] * r + B[1]) * r + B[2]) * r + B[3]) * r + B[4]) * r + 1.0)
}

/// Box–Muller, using two draws from the stream.
fn standard_normal(st: &mut Stream) -> f64 {
    let u1 = st.next_f64().max(f64::MIN_POSITIVE);
    let u2 = st.next_f64();
    (-2.0 * u1.ln()).sqrt() * (std::f64::consts::TAU * u2).cos()
}

/// Zipf by inverse transform over a truncated support.
fn zipf(st: &mut Stream, s: f64, n: u64) -> f64 {
    let u = st.next_f64();
    if s <= 0.0 {
        return (u * n as f64).floor() + 1.0;
    }
    // Continuous approximation to the discrete inverse CDF; adequate for rank
    // selection and monotone in u, which is what callers rely on.
    let n = n as f64;
    let h = |x: f64| -> f64 {
        if (s - 1.0).abs() < 1e-9 {
            x.ln()
        } else {
            (x.powf(1.0 - s) - 1.0) / (1.0 - s)
        }
    };
    let hn = h(n);
    let target = u * hn;
    let x = if (s - 1.0).abs() < 1e-9 {
        target.exp()
    } else {
        (target * (1.0 - s) + 1.0).powf(1.0 / (1.0 - s))
    };
    x.clamp(1.0, n).floor()
}

/// Linear interpolation over explicit `(value, cumulative_probability)` points.
fn empirical(u: f64, points: &[(f64, f64)]) -> f64 {
    if points.is_empty() {
        return 0.0;
    }
    let mut prev = (points[0].0, 0.0f64);
    for &(v, p) in points {
        if u <= p {
            let span = p - prev.1;
            let frac = if span <= 0.0 {
                0.0
            } else {
                (u - prev.1) / span
            };
            return prev.0 + frac * (v - prev.0);
        }
        prev = (v, p);
    }
    points[points.len() - 1].0
}

#[cfg(test)]
mod tests {
    use super::*;

    fn st() -> Stream {
        Stream::new(0xC0FFEE, 1)
    }

    #[test]
    fn scalar_is_sugar_for_const() {
        let a: Dist = serde_yaml::from_str("4").unwrap();
        let b: Dist = serde_yaml::from_str("{dist: const, value: 4}").unwrap();
        assert_eq!(a.shape(), b.shape());
    }

    #[test]
    fn unknown_distribution_field_is_an_error() {
        // FR-005: a mistyped parameter must not silently take a default.
        let r: Result<Shape, _> = serde_yaml::from_str("{dist: lognormal, median: 8, sigmaa: 0.8}");
        assert!(r.is_err(), "unknown field must be rejected");
    }

    #[test]
    fn unknown_shape_is_an_error() {
        let r: Result<Shape, _> = serde_yaml::from_str("{dist: gaussian, mean: 1}");
        assert!(r.is_err());
    }

    #[test]
    fn round_half_to_even_does_not_drift_upward() {
        assert_eq!(round_half_even(0.5), 0.0);
        assert_eq!(round_half_even(1.5), 2.0);
        assert_eq!(round_half_even(2.5), 2.0);
        assert_eq!(round_half_even(3.5), 4.0);
    }

    #[test]
    fn clamps_are_counted_not_silent() {
        let c = Clamps::default();
        let d = Dist::Scalar(100.0);
        assert_eq!(d.sample_u64_clamped(&mut st(), 1, 10, &c), 10);
        assert_eq!(c.domain.load(Ordering::Relaxed), 1);
    }

    #[test]
    fn normal_truncation_is_counted() {
        let c = Clamps::default();
        let d = Dist::Shaped(Shape::Normal {
            mean: -1000.0,
            stddev: 1.0,
        });
        let mut s = st();
        let v = d.sample_counted(&mut s, &c);
        assert_eq!(v, 0.0);
        assert_eq!(c.normal_truncation.load(Ordering::Relaxed), 1);
    }

    #[test]
    fn empirical_interpolates_between_points() {
        let d = Dist::Shaped(Shape::Empirical {
            points: vec![(4.0, 0.10), (18.0, 0.75), (40.0, 1.0)],
        });
        // Below the first cumulative probability, the first value.
        assert!((empirical(0.0, &[(4.0, 0.10), (18.0, 0.75)]) - 4.0).abs() < 1e-9);
        // A mid draw lands strictly between the bracketing values.
        let mut s = st();
        let v = d.sample(&mut s);
        assert!((4.0..=40.0).contains(&v), "got {v}");
    }

    #[test]
    fn lognormal_mean_has_the_closed_form() {
        let d = Dist::Shaped(Shape::Lognormal {
            median: 8.0,
            sigma: 0.0,
        });
        assert!((d.mean().unwrap() - 8.0).abs() < 1e-9);
    }

    #[test]
    fn zipf_mean_is_unavailable_rather_than_wrong() {
        // Deliberately None: the mean depends on the support and diverges for
        // s <= 2, so a caller must say which n it means.
        let d = Dist::Shaped(Shape::Zipf { s: 0.9, n: None });
        assert!(d.mean().is_none());
    }

    #[test]
    fn quantiles_are_analytic_rather_than_sampled() {
        // Validation rule 16 tests occupancy at p99(shared_depth) before anything
        // is generated, so its p99 cannot come from a plan that does not exist.
        assert_eq!(Dist::Scalar(18.0).quantile_u32(0.99), 18);
        let ln = Dist::Shaped(Shape::Lognormal {
            median: 18.0,
            sigma: 0.6,
        });
        // median * exp(0.6 * 2.326) = 18 * 4.04 = 72.7
        let q = ln.quantile_u32(0.99);
        assert!((70..76).contains(&q), "lognormal p99 was {q}");
        let emp = Dist::Shaped(Shape::Empirical {
            points: vec![(4.0, 0.10), (18.0, 0.75), (40.0, 1.0)],
        });
        let q = emp.quantile_u32(0.99);
        assert!((37..=40).contains(&q), "empirical p99 was {q}");
        // The median of a symmetric shape is its centre, which is the cheapest
        // check that probit is the right way round.
        let n = Dist::Shaped(Shape::Normal {
            mean: 50.0,
            stddev: 10.0,
        });
        assert_eq!(n.quantile_u32(0.5), 50);
        assert!(n.quantile(0.99).unwrap() > 70.0);
        assert!(n.quantile(0.01).unwrap() < 30.0);
    }

    #[test]
    fn a_zipf_quantile_is_unavailable_for_the_same_reason_its_mean_is() {
        // The answer depends on a support the shape does not carry, and inventing
        // one would make the rule-16 floor a statement about a different corpus.
        let d = Dist::Shaped(Shape::Zipf { s: 0.9, n: None });
        assert!(d.quantile(0.99).is_none());
        assert_eq!(d.quantile_u32(0.99), 0);
    }

    #[test]
    fn geometric_draws_are_at_least_one() {
        let d = Dist::Shaped(Shape::Geometric { mean: 6.0 });
        let mut s = st();
        for _ in 0..1000 {
            assert!(d.sample(&mut s) >= 1.0);
        }
    }
}
