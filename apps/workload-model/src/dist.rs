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
use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;
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

/// Support size above which the discrete table is not built and the continuous
/// approximation is used instead.
///
/// An unbounded `Zipf` samples against `u32::MAX`, so some bound is required. Every
/// support a document can mean — `roots.count`, or a trunk node's child count — is
/// orders of magnitude below this, and the fallback is documented on [`zipf`].
const ZIPF_EXACT_MAX_SUPPORT: u64 = 1 << 22;

/// Memoised discrete-Zipf cumulative tables, keyed by `(s.to_bits(), n)`.
///
/// `s` is keyed by its bits because an exponent is an exact document value here, not a
/// computed one, so bitwise identity is the right notion of "the same distribution".
type ZipfTables = HashMap<(u64, u64), Rc<Vec<f64>>>;

/// The discrete Zipf cumulative distribution over ranks `1..=n`, memoised per
/// `(s, n)`.
///
/// Inversion needs the generalised harmonic number `H_n(s) = Σ k^-s`, which has no
/// closed form, so the table is built once per distinct `(s, n)` and shared. A memo
/// rather than a field on the distribution because `Corpus::pick_child` mints a fresh
/// `Zipf` per node — its support is *that node's* child count — so there is nowhere to
/// hang a prepared form. The cache cannot affect a drawn value, only how fast it
/// arrives, so determinism is untouched.
fn zipf_cdf(s: f64, n: u64) -> Rc<Vec<f64>> {
    thread_local! {
        static CACHE: RefCell<ZipfTables> = RefCell::new(ZipfTables::new());
    }
    CACHE.with(|c| {
        let key = (s.to_bits(), n);
        if let Some(t) = c.borrow().get(&key) {
            return Rc::clone(t);
        }
        let mut cdf = Vec::with_capacity(n as usize);
        let mut acc = 0.0f64;
        for k in 1..=n {
            acc += (k as f64).powf(-s);
            cdf.push(acc);
        }
        // Normalise in place, and pin the last entry to exactly 1.0 so a `u` just
        // below 1 cannot fall off the end through rounding.
        let total = acc;
        for v in cdf.iter_mut() {
            *v /= total;
        }
        if let Some(last) = cdf.last_mut() {
            *last = 1.0;
        }
        let t = Rc::new(cdf);
        c.borrow_mut().insert(key, Rc::clone(&t));
        t
    })
}

/// Zipf by inverse transform over a truncated support, as the **discrete** pmf
/// `p_k = k^-s / H_n(s)`.
///
/// # Why this is not the continuous approximation it used to be
///
/// The previous implementation inverted a continuous approximation and floored, so
/// rank `k` received the density's mass on `[k, k+1)`. That has two consequences that
/// are not approximation error but structural defects:
///
/// * **Rank `n` had probability exactly zero.** `h(1) = 0` and [`Stream::next_f64`] is
///   in `[0, 1)`, so the inverted value never reached `n` and the top-numbered rank was
///   unreachable at every support size.
/// * **At `n = 2` the draw was deterministic** for *every* `s > 0`, since
///   `p_1 = h(2)/h(2) = 1`. The 2-way split is the commonest branch point in real
///   traces, so `branch_skew` values of 0.5, 0.9 and 1.5 all produced byte-identical
///   streams and a trunk collapsed to one path per root.
///
/// Both are gone: every rank in `1..=n` has positive probability, and `p_1` at `n = 2`
/// is `1/(1 + 2^-s)`, which is below 1 for every finite `s`.
///
/// `s <= 0` is uniform over the support, which is what the schema documents for 0.
/// Above [`ZIPF_EXACT_MAX_SUPPORT`] the old continuous inverse is used, because an
/// unbounded `Zipf` would otherwise ask for a `u32::MAX`-entry table; no support a
/// document can express comes near that bound.
fn zipf(st: &mut Stream, s: f64, n: u64) -> f64 {
    let u = st.next_f64();
    if s <= 0.0 || n <= 1 {
        return (u * n as f64).floor() + 1.0;
    }
    if n > ZIPF_EXACT_MAX_SUPPORT {
        return zipf_continuous(u, s, n);
    }
    let cdf = zipf_cdf(s, n);
    // The first rank whose cumulative exceeds `u`; ranks are 1-based.
    (cdf.partition_point(|&c| c <= u) as f64 + 1.0).min(n as f64)
}

/// The pre-2026-08 continuous approximation, kept for supports too large to tabulate.
fn zipf_continuous(u: f64, s: f64, n: u64) -> f64 {
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
    fn the_discrete_zipf_reaches_every_rank_and_the_continuous_one_never_reached_the_last() {
        // The defect this law replaced, pinned against the fallback that still
        // implements it — so the improvement is asserted rather than asserted-about,
        // and nobody has to revert the sampler to see what changed.
        //
        // `zipf_continuous` is `zipf`'s own large-support fallback, so these are the
        // two laws side by side at the same `(s, n)`.
        // Rank `n`'s mass under the old law is `min(h(n), h(n+1)) - h(n) = 0`, so it is
        // measure-zero rather than literally unreachable — `x` attains `n` only as
        // `u -> 1`, which no draw produces. Asserted by sampling, not by probing the
        // boundary: a `u` within an epsilon of 1 does round up to `n` and would make
        // this test claim something false about the law.
        for n in [3u64, 8, 64] {
            let mut st = Stream::new(9, n);
            let top = (0..20_000)
                .map(|_| zipf_continuous(st.next_f64(), 0.9, n))
                .fold(0.0f64, f64::max);
            assert!(
                top < n as f64,
                "the continuous inverse must be shown never drawing rank {n}, got {top}"
            );
        }
        // At two children the old law was not merely biased, it was deterministic:
        // h(2)/h(2) = 1 puts every draw on rank 1 for any s > 0. That is why
        // branch_skew 0.5, 0.9 and 1.5 produced byte-identical streams.
        for s in [0.5, 0.9, 1.5] {
            let mut st = Stream::new(3, 3);
            let old: Vec<f64> = (0..200)
                .map(|_| zipf_continuous(st.next_f64(), s, 2))
                .collect();
            assert!(
                old.iter().all(|v| *v == 1.0),
                "the continuous inverse at n=2, s={s} must be shown deterministic"
            );
        }
        // The discrete law: p_1 = 1/(1 + 2^-s), and both ranks occur.
        for s in [0.5, 0.9, 1.5] {
            let d = Dist::Shaped(Shape::Zipf { s, n: Some(2) });
            let mut st = Stream::new(3, 3);
            let ones = (0..20_000).filter(|_| d.sample(&mut st) == 1.0).count();
            let want = 1.0 / (1.0 + 2f64.powf(-s));
            let got = ones as f64 / 20_000.0;
            assert!(
                (got - want).abs() < 0.02,
                "n=2 s={s}: rank 1 realised {got:.4} against the discrete pmf's {want:.4}"
            );
        }
        // And the top rank of a larger support is now reachable.
        let d = Dist::Shaped(Shape::Zipf { s: 0.9, n: Some(8) });
        let mut st = Stream::new(4, 4);
        assert!(
            (0..20_000).any(|_| d.sample(&mut st) == 8.0),
            "rank 8 of 8 must be reachable"
        );
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
