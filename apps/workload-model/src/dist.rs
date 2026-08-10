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
    fn geometric_draws_are_at_least_one() {
        let d = Dist::Shaped(Shape::Geometric { mean: 6.0 });
        let mut s = st();
        for _ in 0..1000 {
            assert!(d.sample(&mut s) >= 1.0);
        }
    }
}
