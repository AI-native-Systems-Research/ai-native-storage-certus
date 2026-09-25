//! The one sampling primitive. Every numeric field in a workload description is
//! one of these.
//!
//! A [`Distribution`] is what the file says: a [`Kind`] plus optional
//! truncation bounds. It cannot be sampled directly, because two properties are
//! decided by the *consumer* rather than by the file — whether the quantity is
//! integral, and whether some pool size caps it — so a distribution is first
//! [`Distribution::resolve`]d into a [`Resolved`], and a `Resolved` is what
//! draws.
//!
//! # Truncation is truncation, not clamping
//!
//! Bounds are applied by inverse transform between `F(min)` and `F(max)`
//! (FR-008): draw `u` uniform, map it into `[F(min), F(max))`, and invert.
//! Clamping — draw, then move out-of-range values to the nearest bound — is
//! the tempting alternative and it is wrong twice over: it piles finite
//! probability mass on the two boundary points, and it shifts the mean. A
//! `normal{mean: 5, sigma: 1, min: 1}` clamped would put a spike at exactly 1;
//! truncated it has no mass there at all.
//!
//! # Integral quantities are half-open, not rounded-at-the-edges
//!
//! When the consumer's type is an integer, the effective bounds widen to
//! `[min − 0.5, max + 0.5]` and the draw rounds to nearest (FR-009), so every
//! integer in range carries equal weight. Without the widening the endpoints get
//! half the weight of the interior — `uniform{min: 0, max: 5}` would give 0 and
//! 5 half as often as 1..4, which is invisible in an aggregate and wrong in a
//! working-set calculation.
//!
//! Integrality is a property of the consumer, not of the written distribution:
//! `uniform{min: 0, max: 5}` is six equiprobable values as a *count* and a
//! continuous range as a *lifetime*. The same text, read twice.
//!
//! # Examples
//!
//! ```
//! use workload_model::distribution::{Distribution, Kind};
//! use workload_model::rng;
//!
//! let d = Distribution::new(Kind::Uniform { min: 0.0, max: 5.0 });
//! let counts = d.resolve_integral(None).unwrap();
//!
//! // Six equiprobable values, endpoints included at full weight.
//! let mut rng = rng::seeded(7);
//! let mut seen = [0usize; 6];
//! for _ in 0..6_000 {
//!     seen[counts.sample_int(&mut rng) as usize] += 1;
//! }
//! assert!(seen.iter().all(|&n| n > 800), "not equiprobable: {seen:?}");
//!
//! // And the effective mean is the midpoint, 2.5.
//! assert!((counts.effective().mean - 2.5).abs() < 1e-12);
//! ```

use std::path::Path;

use rand::Rng;
use serde::de::{self, Deserializer, MapAccess, Visitor};
use serde::Deserialize;

use crate::error::{Error, Result};
use crate::special;

/// Largest integer range this module will enumerate exactly when computing
/// effective statistics. Beyond it, the continuous truncated mean is reported
/// instead; the two differ by O(1/range), so the substitution is invisible
/// precisely where it happens.
const MAX_ENUMERATED: u64 = 1 << 20;

/// The five distribution kinds and their parameters.
///
/// `min` and `max` are *not* here except for [`Kind::Uniform`], where they are
/// the distribution's own support rather than a truncation of something wider.
/// Everything else carries its bounds on the enclosing [`Distribution`].
#[derive(Debug, Clone, PartialEq)]
pub enum Kind {
    /// A single value, with probability one.
    Constant {
        /// The value. May be infinite — `{constant: inf}` is how a description
        /// says "never expires".
        value: f64,
    },
    /// Uniform on `[min, max]`.
    Uniform {
        /// Lower end of the support.
        min: f64,
        /// Upper end of the support.
        max: f64,
    },
    /// Normal with the given mean and standard deviation.
    Normal {
        /// Mean of the untruncated distribution.
        mean: f64,
        /// Standard deviation, strictly positive.
        sigma: f64,
    },
    /// Exponential with the given mean (rate `1 / mean`), supported on
    /// `[0, inf)`.
    Exponential {
        /// Mean of the untruncated distribution, strictly positive.
        mean: f64,
    },
    /// An observed sample set.
    Empirical {
        /// Samples, sorted ascending by [`Distribution::new`].
        samples: Vec<f64>,
        /// Whether to interpolate between samples.
        ///
        /// `false` — the default for counts — draws one of the observed values.
        /// `true` places mass between them, which for a bimodal sample set
        /// means putting draws in a gap the data says is empty.
        interpolate: bool,
    },
}

/// A distribution as written in a description: a [`Kind`] plus optional
/// truncation bounds.
#[derive(Debug, Clone, PartialEq)]
pub struct Distribution {
    kind: Kind,
    min: Option<f64>,
    max: Option<f64>,
    /// Deferred `empirical: {file: ...}`, unresolved until
    /// [`Distribution::load_sample_files`] runs.
    sample_file: Option<String>,
}

impl Distribution {
    /// A distribution with no truncation bounds.
    ///
    /// For [`Kind::Uniform`] the kind's own `min`/`max` are adopted as bounds,
    /// since they are the support.
    ///
    /// # Examples
    ///
    /// ```
    /// use workload_model::distribution::{Distribution, Kind};
    ///
    /// let d = Distribution::new(Kind::Exponential { mean: 10.0 });
    /// assert_eq!(d.min(), None);
    /// ```
    pub fn new(kind: Kind) -> Self {
        let (min, max) = match &kind {
            Kind::Uniform { min, max } => (Some(*min), Some(*max)),
            _ => (None, None),
        };
        let kind = match kind {
            Kind::Empirical {
                mut samples,
                interpolate,
            } => {
                samples.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
                Kind::Empirical {
                    samples,
                    interpolate,
                }
            }
            other => other,
        };
        Self {
            kind,
            min,
            max,
            sample_file: None,
        }
    }

    /// Add truncation bounds.
    ///
    /// # Examples
    ///
    /// ```
    /// use workload_model::distribution::{Distribution, Kind};
    ///
    /// let d = Distribution::new(Kind::Normal { mean: 5.0, sigma: 1.0 }).bounded(Some(1.0), None);
    /// assert_eq!(d.min(), Some(1.0));
    /// ```
    pub fn bounded(mut self, min: Option<f64>, max: Option<f64>) -> Self {
        if min.is_some() {
            self.min = min;
        }
        if max.is_some() {
            self.max = max;
        }
        self
    }

    /// The kind, as written.
    pub fn kind(&self) -> &Kind {
        &self.kind
    }

    /// The written lower truncation bound, if any.
    pub fn min(&self) -> Option<f64> {
        self.min
    }

    /// The written upper truncation bound, if any.
    pub fn max(&self) -> Option<f64> {
        self.max
    }

    /// The mean of the distribution as written, ignoring truncation.
    ///
    /// This is the "requested" figure a validation error quotes alongside the
    /// effective one (FR-004), so it is deliberately the number the author typed
    /// rather than anything derived.
    ///
    /// # Examples
    ///
    /// ```
    /// use workload_model::distribution::{Distribution, Kind};
    ///
    /// let d = Distribution::new(Kind::Exponential { mean: 5.0 });
    /// assert_eq!(d.requested_mean(), 5.0);
    /// ```
    pub fn requested_mean(&self) -> f64 {
        match &self.kind {
            Kind::Constant { value } => *value,
            Kind::Uniform { min, max } => 0.5 * (min + max),
            Kind::Normal { mean, .. } | Kind::Exponential { mean } => *mean,
            Kind::Empirical { samples, .. } => {
                if samples.is_empty() {
                    f64::NAN
                } else {
                    samples.iter().sum::<f64>() / samples.len() as f64
                }
            }
        }
    }

    /// Read any `empirical: {file: ...}` samples, resolving the path relative to
    /// `base_dir`.
    ///
    /// Deferred rather than done at parse time so that a description can be
    /// parsed from a string with no filesystem in reach — which is what most of
    /// this crate's tests do.
    ///
    /// # Errors
    ///
    /// If the file cannot be read, or holds no parsable number.
    pub fn load_sample_files(&mut self, base_dir: &Path) -> Result<()> {
        let Some(name) = self.sample_file.take() else {
            return Ok(());
        };
        let path = base_dir.join(&name);
        let text = std::fs::read_to_string(&path)
            .map_err(|e| Error::new(format!("cannot read empirical samples {path:?}: {e}")))?;
        let mut samples = Vec::new();
        for (lineno, line) in text.lines().enumerate() {
            let line = line.trim();
            if line.is_empty() || line.starts_with('#') {
                continue;
            }
            for field in line.split(|c: char| c.is_whitespace() || c == ',') {
                if field.is_empty() {
                    continue;
                }
                let v: f64 = field.parse().map_err(|_| {
                    Error::new(format!(
                        "{path:?} line {}: {field:?} is not a number",
                        lineno + 1
                    ))
                })?;
                samples.push(v);
            }
        }
        if samples.is_empty() {
            return Err(Error::new(format!("{path:?} holds no samples")));
        }
        samples.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
        let interpolate = matches!(
            self.kind,
            Kind::Empirical {
                interpolate: true,
                ..
            }
        );
        self.kind = Kind::Empirical {
            samples,
            interpolate,
        };
        Ok(())
    }

    /// Resolve for a continuous consumer, optionally capped from above.
    ///
    /// # Errors
    ///
    /// If the parameters are unusable, or truncation leaves no probability mass.
    pub fn resolve(&self, cap_max: Option<f64>) -> Result<Resolved> {
        self.resolve_with(false, cap_max)
    }

    /// Resolve for an integer-valued consumer, optionally capped from above.
    ///
    /// The bounds widen by half a unit and draws round to nearest, so every
    /// integer in range carries equal weight (FR-009).
    ///
    /// # Errors
    ///
    /// As [`Distribution::resolve`].
    ///
    /// # Examples
    ///
    /// ```
    /// use workload_model::distribution::{Distribution, Kind};
    ///
    /// // A count from exponential(mean 5, min 1), capped by a pool of 20.
    /// let d = Distribution::new(Kind::Exponential { mean: 5.0 }).bounded(Some(1.0), None);
    /// let counts = d.resolve_integral(Some(20.0)).unwrap();
    /// let eff = counts.effective();
    /// assert!((eff.mean - 5.143508).abs() < 1e-5);
    /// assert!((eff.discarded - 0.018316).abs() < 1e-5);
    /// ```
    pub fn resolve_integral(&self, cap_max: Option<f64>) -> Result<Resolved> {
        self.resolve_with(true, cap_max)
    }

    fn resolve_with(&self, integral: bool, cap_max: Option<f64>) -> Result<Resolved> {
        self.check_parameters()?;

        // Bounds as written, widened by half a unit for an integral consumer.
        // The widening applies to *explicit* bounds only: a kind's natural
        // support is not moved, so rounding a non-negative exponential with no
        // `min` still cannot produce -1.
        let half = if integral { 0.5 } else { 0.0 };
        let authored_lo = self.min.map(|v| v - half);
        let authored_hi = self.max.map(|v| v + half);
        let capped_hi = match (authored_hi, cap_max) {
            (Some(a), Some(c)) => Some(a.min(c + half)),
            (Some(a), None) => Some(a),
            (None, Some(c)) => Some(c + half),
            (None, None) => None,
        };

        // For a uniform, the bounds *are* the support, so the half-unit widening
        // has to move the parameters too. Widening only the truncation interval
        // would leave the CDF clamping at the original endpoints, which is
        // exactly the half-weight-endpoints bug FR-009 exists to prevent.
        let kind = match (&self.kind, integral) {
            (Kind::Uniform { min, max }, true) => Kind::Uniform {
                min: min - 0.5,
                max: max + 0.5,
            },
            (other, _) => other.clone(),
        };

        let (support_lo, support_hi) = kind.support();
        let lo = authored_lo.unwrap_or(support_lo).max(support_lo);
        let hi = capped_hi.unwrap_or(support_hi).min(support_hi);
        let a_hi = authored_hi.unwrap_or(support_hi).min(support_hi);

        if lo > hi {
            return Err(Error::new(format!(
                "truncation leaves an empty range: min {lo} exceeds max {hi}"
            )));
        }

        let f_lo = kind.cdf(lo);
        let f_hi = kind.cdf(hi);
        let f_a_hi = kind.cdf(a_hi);
        // A degenerate support — `{constant: 10}`, `{uniform: {min: 3, max: 3}}`,
        // a one-sample empirical — is a point mass, not an empty range. Its CDF
        // jumps at the point, so `F(hi) - F(lo)` is legitimately zero there and
        // the emptiness test must not be applied. `{constant: inf}` takes this
        // path too, which is how a description says "never expires".
        let degenerate = lo == hi;
        if !degenerate && f_hi - f_lo <= 0.0 {
            return Err(Error::new(format!(
                "truncation to [{lo}, {hi}] leaves no probability mass; the \
                 distribution as written puts nothing in that range"
            )));
        }

        // A discrete empirical draws one of its observed values, so resolve the
        // retained index range now rather than inverting a step function per
        // draw.
        let retained = match &kind {
            Kind::Empirical {
                samples,
                interpolate: false,
            } => {
                let first = samples.partition_point(|s| *s < lo);
                let last = samples.partition_point(|s| *s <= hi);
                if first >= last {
                    return Err(Error::new(format!(
                        "no empirical sample lies in [{lo}, {hi}]"
                    )));
                }
                Some((first, last))
            }
            _ => None,
        };

        // Retained and authored mass. For a discrete empirical these must be
        // counted the same way the *draw* is resolved — by index — because its
        // step CDF disagrees with the retained range at a bound that coincides
        // with an observation: `cdf(lo)` already counts the sample at `lo`,
        // while the draw includes it. Deriving both from the same index range
        // keeps the reported mass and the realised draws consistent.
        let (kept_mass, authored_mass) = match (&kind, retained) {
            (Kind::Empirical { samples, .. }, Some((first, last))) => {
                let n = samples.len() as f64;
                let a_last = samples.partition_point(|s| *s <= a_hi);
                (
                    (last - first) as f64 / n,
                    (a_last.saturating_sub(first)) as f64 / n,
                )
            }
            _ => (f_hi - f_lo, f_a_hi - f_lo),
        };

        Ok(Resolved {
            kind,
            lo,
            hi,
            f_lo,
            f_hi,
            kept_mass,
            authored_mass,
            requested_mean: self.requested_mean(),
            integral,
            retained,
        })
    }

    fn check_parameters(&self) -> Result<()> {
        match &self.kind {
            Kind::Constant { value } => {
                if value.is_nan() {
                    return Err(Error::new("constant is not a number"));
                }
            }
            Kind::Uniform { min, max } => {
                if !(min.is_finite() && max.is_finite()) {
                    return Err(Error::new("uniform bounds must both be finite"));
                }
                if min > max {
                    return Err(Error::new(format!("uniform min {min} exceeds max {max}")));
                }
            }
            Kind::Normal { mean, sigma } => {
                if !mean.is_finite() {
                    return Err(Error::new("normal mean must be finite"));
                }
                // `!sigma.is_finite()` first, so NaN is rejected here rather
                // than slipping through a comparison that is false either way.
                if !sigma.is_finite() || *sigma <= 0.0 {
                    return Err(Error::new(format!(
                        "normal sigma must be finite and positive, got {sigma}"
                    )));
                }
            }
            Kind::Exponential { mean } => {
                if !mean.is_finite() || *mean <= 0.0 {
                    return Err(Error::new(format!(
                        "exponential mean must be finite and positive, got {mean}"
                    )));
                }
            }
            Kind::Empirical { samples, .. } => {
                if self.sample_file.is_some() {
                    return Err(Error::new(
                        "empirical samples were not loaded; call load_sample_files first",
                    ));
                }
                if samples.is_empty() {
                    return Err(Error::new("empirical needs at least one sample"));
                }
                if samples.iter().any(|s| !s.is_finite()) {
                    return Err(Error::new("empirical samples must all be finite"));
                }
            }
        }
        if let (Some(lo), Some(hi)) = (self.min, self.max) {
            if lo > hi {
                return Err(Error::new(format!("min {lo} exceeds max {hi}")));
            }
        }
        Ok(())
    }
}

impl Kind {
    /// Natural support, before any truncation.
    fn support(&self) -> (f64, f64) {
        match self {
            Kind::Constant { value } => (*value, *value),
            Kind::Uniform { min, max } => (*min, *max),
            Kind::Normal { .. } => (f64::NEG_INFINITY, f64::INFINITY),
            Kind::Exponential { .. } => (0.0, f64::INFINITY),
            Kind::Empirical { samples, .. } => (
                samples.first().copied().unwrap_or(f64::NAN),
                samples.last().copied().unwrap_or(f64::NAN),
            ),
        }
    }

    /// Cumulative distribution function, clamped to `[0, 1]`.
    fn cdf(&self, x: f64) -> f64 {
        let f = match self {
            Kind::Constant { value } => {
                if x >= *value {
                    1.0
                } else {
                    0.0
                }
            }
            Kind::Uniform { min, max } => {
                if max <= min {
                    if x >= *min {
                        1.0
                    } else {
                        0.0
                    }
                } else {
                    (x - min) / (max - min)
                }
            }
            Kind::Normal { mean, sigma } => special::normal_cdf((x - mean) / sigma),
            Kind::Exponential { mean } => {
                if x <= 0.0 {
                    0.0
                } else {
                    -(-x / mean).exp_m1()
                }
            }
            Kind::Empirical {
                samples,
                interpolate,
            } => empirical_cdf(samples, *interpolate, x),
        };
        f.clamp(0.0, 1.0)
    }

    /// Inverse CDF on `[0, 1]`.
    fn ppf(&self, p: f64) -> f64 {
        match self {
            Kind::Constant { value } => *value,
            Kind::Uniform { min, max } => min + p * (max - min),
            Kind::Normal { mean, sigma } => mean + sigma * special::normal_ppf(p),
            // `ln_1p(-p)` rather than `(1 - p).ln()`: it keeps its accuracy for
            // small p, where the naive form cancels.
            Kind::Exponential { mean } => -mean * (-p).ln_1p(),
            Kind::Empirical {
                samples,
                interpolate,
            } => empirical_ppf(samples, *interpolate, p),
        }
    }
}

fn empirical_cdf(samples: &[f64], interpolate: bool, x: f64) -> f64 {
    let n = samples.len();
    if n == 0 {
        return f64::NAN;
    }
    if !interpolate {
        return samples.partition_point(|s| *s <= x) as f64 / n as f64;
    }
    if n == 1 {
        return if x >= samples[0] { 1.0 } else { 0.0 };
    }
    // Piecewise linear through the order statistics, F(s_i) = i / (n - 1), so
    // the support is exactly [min sample, max sample].
    if x <= samples[0] {
        return 0.0;
    }
    if x >= samples[n - 1] {
        return 1.0;
    }
    let i = samples.partition_point(|s| *s <= x).saturating_sub(1);
    let (a, b) = (samples[i], samples[i + 1]);
    let within = if b > a { (x - a) / (b - a) } else { 0.0 };
    (i as f64 + within) / (n - 1) as f64
}

fn empirical_ppf(samples: &[f64], interpolate: bool, p: f64) -> f64 {
    let n = samples.len();
    if n == 0 {
        return f64::NAN;
    }
    if !interpolate {
        let idx = ((p * n as f64).floor() as usize).min(n - 1);
        return samples[idx];
    }
    if n == 1 {
        return samples[0];
    }
    let pos = (p.clamp(0.0, 1.0)) * (n - 1) as f64;
    let i = (pos.floor() as usize).min(n - 2);
    let frac = pos - i as f64;
    samples[i] + frac * (samples[i + 1] - samples[i])
}

/// A distribution resolved for one consumer: truncation folded in,
/// integrality fixed, and the CDF at the bounds precomputed so a draw costs one
/// uniform sample and one inverse-CDF evaluation.
#[derive(Debug, Clone, PartialEq)]
pub struct Resolved {
    kind: Kind,
    lo: f64,
    hi: f64,
    f_lo: f64,
    f_hi: f64,
    /// Mass surviving the effective bounds, cap included.
    kept_mass: f64,
    /// Mass of the distribution *as written* that survives its own bounds — the
    /// denominator for [`Effective::discarded`], so that a consumer's cap is
    /// measured against what the author asked for rather than against the
    /// untruncated kind.
    authored_mass: f64,
    requested_mean: f64,
    integral: bool,
    retained: Option<(usize, usize)>,
}

impl Resolved {
    /// Draw one continuous value in `[lo, hi]`.
    ///
    /// The lower bound is attainable and the upper is not, because the uniform
    /// source is half-open. For an integral consumer the difference vanishes
    /// under rounding; for a continuous one it has measure zero.
    ///
    /// # Examples
    ///
    /// ```
    /// use workload_model::distribution::{Distribution, Kind};
    /// use workload_model::rng;
    ///
    /// let d = Distribution::new(Kind::Normal { mean: 5.0, sigma: 1.0 }).bounded(Some(4.0), Some(6.0));
    /// let r = d.resolve(None).unwrap();
    /// let mut rng = rng::seeded(1);
    /// for _ in 0..1_000 {
    ///     let x = r.sample(&mut rng);
    ///     assert!((4.0..=6.0).contains(&x));
    /// }
    /// ```
    pub fn sample<R: Rng + ?Sized>(&self, rng: &mut R) -> f64 {
        let u: f64 = rng.gen();
        if let Some((first, last)) = self.retained {
            // Discrete empirical: inverse transform on the retained run of
            // observed values, which is uniform over them by construction.
            let n = last - first;
            let idx = first + ((u * n as f64).floor() as usize).min(n - 1);
            return match &self.kind {
                Kind::Empirical { samples, .. } => samples[idx],
                _ => unreachable!("retained range is only set for a discrete empirical"),
            };
        }
        let p = self.f_lo + u * (self.f_hi - self.f_lo);
        self.kind.ppf(p).clamp(self.lo, self.hi)
    }

    /// Draw one integer value.
    ///
    /// # Panics
    ///
    /// If this was resolved for a continuous consumer, since rounding a
    /// continuous quantity would silently misreport it.
    ///
    /// # Examples
    ///
    /// ```
    /// use workload_model::distribution::{Distribution, Kind};
    /// use workload_model::rng;
    ///
    /// let d = Distribution::new(Kind::Uniform { min: 0.0, max: 5.0 });
    /// let r = d.resolve_integral(None).unwrap();
    /// let mut rng = rng::seeded(1);
    /// for _ in 0..100 {
    ///     assert!((0..=5).contains(&r.sample_int(&mut rng)));
    /// }
    /// ```
    pub fn sample_int<R: Rng + ?Sized>(&self, rng: &mut R) -> i64 {
        assert!(
            self.integral,
            "sample_int on a distribution resolved as continuous"
        );
        self.sample(rng).round() as i64
    }

    /// Whether this was resolved for an integer-valued consumer.
    pub fn is_integral(&self) -> bool {
        self.integral
    }

    /// The effective bounds, after folding the written bounds, the consumer's
    /// cap, the kind's support, and any half-unit widening.
    pub fn bounds(&self) -> (f64, f64) {
        (self.lo, self.hi)
    }

    /// The quantile at `p`, after truncation.
    ///
    /// # Examples
    ///
    /// ```
    /// use workload_model::distribution::{Distribution, Kind};
    ///
    /// let d = Distribution::new(Kind::Uniform { min: 0.0, max: 10.0 });
    /// let r = d.resolve(None).unwrap();
    /// assert!((r.quantile(0.5) - 5.0).abs() < 1e-12);
    /// ```
    pub fn quantile(&self, p: f64) -> f64 {
        let x = if let Some((first, last)) = self.retained {
            let n = last - first;
            let idx = first + ((p * n as f64).floor() as usize).min(n - 1);
            match &self.kind {
                Kind::Empirical { samples, .. } => samples[idx],
                _ => unreachable!("retained range is only set for a discrete empirical"),
            }
        } else {
            let q = self.f_lo + p.clamp(0.0, 1.0) * (self.f_hi - self.f_lo);
            self.kind.ppf(q).clamp(self.lo, self.hi)
        };
        if self.integral {
            x.round()
        } else {
            x
        }
    }

    /// Everything FR-003 reports at load and FR-004 gates on.
    ///
    /// # Examples
    ///
    /// ```
    /// use workload_model::distribution::{Distribution, Kind};
    ///
    /// let d = Distribution::new(Kind::Uniform { min: 0.0, max: 5.0 });
    /// let eff = d.resolve_integral(None).unwrap().effective();
    /// assert_eq!(eff.requested_mean, 2.5);
    /// assert!((eff.mean - 2.5).abs() < 1e-12);
    /// assert_eq!(eff.discarded, 0.0);
    /// ```
    pub fn effective(&self) -> Effective {
        Effective {
            requested_mean: self.requested_mean,
            mean: self.effective_mean(),
            discarded: self.discarded(),
            p50: self.quantile(0.5),
            p90: self.quantile(0.9),
            p99: self.quantile(0.99),
            lo: self.lo,
            hi: self.hi,
            integral: self.integral,
        }
    }

    /// Fraction of the distribution *as written* that this resolution's bounds
    /// discard.
    ///
    /// Measured against the author's own bounds, not against the untruncated
    /// kind: a `min` the author wrote is a deliberate choice, whereas a pool
    /// size that clips the top is the thing FR-004 refuses to let pass
    /// silently.
    fn discarded(&self) -> f64 {
        if self.authored_mass <= 0.0 {
            return 0.0;
        }
        ((self.authored_mass - self.kept_mass) / self.authored_mass).clamp(0.0, 1.0)
    }

    /// The mean actually realised, including the effect of rounding when
    /// integral.
    fn effective_mean(&self) -> f64 {
        // Point mass: the mean is the point, and no enumeration or closed form
        // is meaningful because the CDF has no interval to integrate over.
        if self.lo == self.hi {
            return if self.integral && self.lo.is_finite() {
                self.lo.round()
            } else {
                self.lo
            };
        }
        if let Some((first, last)) = self.retained {
            let Kind::Empirical { samples, .. } = &self.kind else {
                unreachable!("retained range is only set for a discrete empirical");
            };
            let run = &samples[first..last];
            return run.iter().sum::<f64>() / run.len() as f64;
        }
        if self.integral {
            if let Some(m) = self.integral_mean() {
                return m;
            }
        }
        self.continuous_mean()
    }

    /// Exact mean of the rounded, truncated variable, by enumerating the
    /// integers it can produce.
    ///
    /// Returns `None` when the integer range is too wide to enumerate, in which
    /// case the continuous mean is used instead — a substitution that is only
    /// ever reached once the two agree to within O(1/range).
    fn integral_mean(&self) -> Option<f64> {
        let k0 = self.lo.ceil();
        if !k0.is_finite() {
            return None;
        }
        let z = self.f_hi - self.f_lo;
        if z <= 0.0 {
            return None;
        }
        let k1 = self.hi.floor();
        let span = if k1.is_finite() {
            (k1 - k0) as u64 + 1
        } else {
            MAX_ENUMERATED
        };
        if k1.is_finite() && span > MAX_ENUMERATED {
            return None;
        }

        let mut mean = 0.0f64;
        let mut mass = 0.0f64;
        let mut k = k0;
        for _ in 0..span {
            let bin = (self.kind.cdf(k + 0.5).min(self.f_hi)
                - self.kind.cdf(k - 0.5).max(self.f_lo))
            .max(0.0);
            mean += k * bin;
            mass += bin;
            // An unbounded upper end stops once the residue cannot move the
            // mean: the tail of any distribution with a finite mean gets there
            // in a few hundred steps.
            if !k1.is_finite() && z - mass < 1e-15 * z.max(1.0) {
                break;
            }
            k += 1.0;
        }
        if mass <= 0.0 {
            return None;
        }
        Some(mean / mass)
    }

    /// Mean of the truncated continuous distribution, in closed form.
    fn continuous_mean(&self) -> f64 {
        let (lo, hi) = (self.lo, self.hi);
        match &self.kind {
            Kind::Constant { value } => *value,
            Kind::Uniform { min, max } => {
                let a = lo.max(*min);
                let b = hi.min(*max);
                0.5 * (a + b)
            }
            Kind::Normal { mean, sigma } => {
                let alpha = (lo - mean) / sigma;
                let beta = (hi - mean) / sigma;
                let z = special::normal_cdf(beta) - special::normal_cdf(alpha);
                if z <= 0.0 {
                    return 0.5 * (lo + hi);
                }
                mean + sigma * (special::normal_pdf(alpha) - special::normal_pdf(beta)) / z
            }
            Kind::Exponential { mean } => {
                let ea = (-lo / mean).exp();
                let eb = if hi.is_finite() {
                    (-hi / mean).exp()
                } else {
                    0.0
                };
                let den = ea - eb;
                if den <= 0.0 {
                    return 0.5 * (lo + hi.min(f64::MAX));
                }
                let hi_term = if hi.is_finite() {
                    (hi + mean) * eb
                } else {
                    0.0
                };
                ((lo + mean) * ea - hi_term) / den
            }
            Kind::Empirical {
                samples,
                interpolate,
            } => {
                if !interpolate {
                    let run: Vec<f64> = samples
                        .iter()
                        .copied()
                        .filter(|s| *s >= lo && *s <= hi)
                        .collect();
                    if run.is_empty() {
                        return 0.5 * (lo + hi);
                    }
                    return run.iter().sum::<f64>() / run.len() as f64;
                }
                // Between two order statistics the interpolated density is
                // uniform, so each clipped segment contributes its own midpoint
                // weighted by the mass left in it. Exact, not a quadrature.
                let n = samples.len();
                if n < 2 {
                    return samples.first().copied().unwrap_or(f64::NAN);
                }
                let seg_mass = 1.0 / (n - 1) as f64;
                let mut num = 0.0;
                let mut den = 0.0;
                for w in samples.windows(2) {
                    let (a, b) = (w[0], w[1]);
                    if b <= a {
                        // Duplicate observations make the piecewise-linear CDF
                        // *jump*, so the segment is an atom of mass `seg_mass`
                        // at that value rather than a zero-width interval to be
                        // skipped. Skipping it drops real mass from the
                        // denominator, which shows up as a mean pulled toward
                        // whichever mode has fewer repeats — on the shipped
                        // example's bimodal `tool` samples, 11 of 19 segments
                        // are duplicates and the mean came out 37.19 instead of
                        // 49.71.
                        if a >= lo && a <= hi {
                            num += seg_mass * a;
                            den += seg_mass;
                        }
                        continue;
                    }
                    let ca = a.max(lo);
                    let cb = b.min(hi);
                    if cb <= ca {
                        continue;
                    }
                    let mass = seg_mass * (cb - ca) / (b - a);
                    num += mass * 0.5 * (ca + cb);
                    den += mass;
                }
                if den <= 0.0 {
                    0.5 * (lo + hi)
                } else {
                    num / den
                }
            }
        }
    }
}

/// A distribution's realised statistics after truncation — what FR-003 reports
/// at load time and FR-004 gates on.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Effective {
    /// The mean as written, ignoring truncation. The figure an author recognises.
    pub requested_mean: f64,
    /// The mean actually realised, including rounding when integral.
    pub mean: f64,
    /// Fraction of the as-written distribution that the effective bounds
    /// discard, in `[0, 1]`.
    pub discarded: f64,
    /// Median after truncation.
    pub p50: f64,
    /// 90th percentile after truncation.
    pub p90: f64,
    /// 99th percentile after truncation.
    pub p99: f64,
    /// Effective lower bound.
    pub lo: f64,
    /// Effective upper bound.
    pub hi: f64,
    /// Whether the consumer draws integers.
    pub integral: bool,
}

impl Effective {
    /// Whether the realised mean differs from the written one by enough to be
    /// worth reporting (FR-003).
    ///
    /// # Examples
    ///
    /// ```
    /// use workload_model::distribution::{Distribution, Kind};
    ///
    /// let untouched = Distribution::new(Kind::Constant { value: 3.0 });
    /// assert!(!untouched.resolve(None).unwrap().effective().differs());
    /// ```
    pub fn differs(&self) -> bool {
        let scale = self.requested_mean.abs().max(1.0);
        (self.mean - self.requested_mean).abs() > 1e-9 * scale
    }
}

// ---------------------------------------------------------------------------
// Deserialisation
// ---------------------------------------------------------------------------

/// An `f64` that also accepts the spellings of infinity a description may use.
///
/// YAML's own core schema writes infinity as `.inf`, but the natural thing to
/// type — and what the shipped example uses — is `inf`, which arrives as a
/// string. FR-011 makes `inf` legal in any numeric position, so both are taken.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Num(pub f64);

impl<'de> Deserialize<'de> for Num {
    fn deserialize<D: Deserializer<'de>>(d: D) -> std::result::Result<Self, D::Error> {
        struct V;
        impl Visitor<'_> for V {
            type Value = Num;
            fn expecting(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                f.write_str("a number, or one of inf / -inf / .inf / infinity")
            }
            fn visit_f64<E: de::Error>(self, v: f64) -> std::result::Result<Num, E> {
                Ok(Num(v))
            }
            fn visit_i64<E: de::Error>(self, v: i64) -> std::result::Result<Num, E> {
                Ok(Num(v as f64))
            }
            fn visit_u64<E: de::Error>(self, v: u64) -> std::result::Result<Num, E> {
                Ok(Num(v as f64))
            }
            fn visit_str<E: de::Error>(self, v: &str) -> std::result::Result<Num, E> {
                let t = v.trim();
                let (sign, rest) = match t.strip_prefix('-') {
                    Some(r) => (-1.0, r),
                    None => (1.0, t.strip_prefix('+').unwrap_or(t)),
                };
                let lower = rest.to_ascii_lowercase();
                let bare = lower.strip_prefix('.').unwrap_or(&lower);
                if bare == "inf" || bare == "infinity" {
                    return Ok(Num(sign * f64::INFINITY));
                }
                t.parse::<f64>()
                    .map(Num)
                    .map_err(|_| E::custom(format!("{v:?} is not a number")))
            }
        }
        d.deserialize_any(V)
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct UniformParams {
    min: Num,
    max: Num,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct NormalParams {
    mean: Num,
    sigma: Num,
    min: Option<Num>,
    max: Option<Num>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ExponentialParams {
    mean: Num,
    min: Option<Num>,
    max: Option<Num>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct EmpiricalParams {
    #[serde(default)]
    samples: Option<Vec<Num>>,
    #[serde(default)]
    file: Option<String>,
    /// Discrete by default: interpolating a bimodal sample set places mass in
    /// gaps the data says are empty (FR-010).
    #[serde(default)]
    interpolate: bool,
    min: Option<Num>,
    max: Option<Num>,
}

impl<'de> Deserialize<'de> for Distribution {
    fn deserialize<D: Deserializer<'de>>(d: D) -> std::result::Result<Self, D::Error> {
        struct V;
        impl<'de> Visitor<'de> for V {
            type Value = Distribution;
            fn expecting(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                f.write_str("a single-entry map {kind: params}")
            }
            fn visit_map<M: MapAccess<'de>>(
                self,
                mut map: M,
            ) -> std::result::Result<Distribution, M::Error> {
                let Some(tag) = map.next_key::<String>()? else {
                    return Err(de::Error::custom(
                        "a distribution needs one entry, {kind: params}",
                    ));
                };
                let dist = match tag.as_str() {
                    "constant" => {
                        let v: Num = map.next_value()?;
                        Distribution::new(Kind::Constant { value: v.0 })
                    }
                    "uniform" => {
                        let p: UniformParams = map.next_value()?;
                        Distribution::new(Kind::Uniform {
                            min: p.min.0,
                            max: p.max.0,
                        })
                    }
                    "normal" => {
                        let p: NormalParams = map.next_value()?;
                        Distribution::new(Kind::Normal {
                            mean: p.mean.0,
                            sigma: p.sigma.0,
                        })
                        .bounded(p.min.map(|n| n.0), p.max.map(|n| n.0))
                    }
                    "exponential" => {
                        let p: ExponentialParams = map.next_value()?;
                        Distribution::new(Kind::Exponential { mean: p.mean.0 })
                            .bounded(p.min.map(|n| n.0), p.max.map(|n| n.0))
                    }
                    "empirical" => {
                        let p: EmpiricalParams = map.next_value()?;
                        if p.samples.is_some() && p.file.is_some() {
                            return Err(de::Error::custom(
                                "empirical takes `samples` or `file`, not both",
                            ));
                        }
                        let samples: Vec<f64> = p
                            .samples
                            .unwrap_or_default()
                            .into_iter()
                            .map(|n| n.0)
                            .collect();
                        if samples.is_empty() && p.file.is_none() {
                            return Err(de::Error::custom("empirical needs `samples` or `file`"));
                        }
                        let mut d = Distribution::new(Kind::Empirical {
                            samples,
                            interpolate: p.interpolate,
                        })
                        .bounded(p.min.map(|n| n.0), p.max.map(|n| n.0));
                        d.sample_file = p.file;
                        d
                    }
                    other => {
                        return Err(de::Error::custom(format!(
                            "unknown distribution kind {other:?}; expected one of \
                             constant, uniform, normal, exponential, empirical"
                        )))
                    }
                };
                if map.next_key::<String>()?.is_some() {
                    return Err(de::Error::custom(format!(
                        "a distribution takes exactly one kind; {tag:?} is followed by another"
                    )));
                }
                Ok(dist)
            }
        }
        d.deserialize_map(V)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(yaml: &str) -> Distribution {
        serde_yaml::from_str(yaml).expect("parses")
    }

    #[test]
    fn parses_the_five_kinds() {
        assert_eq!(
            parse("{constant: 10}").kind(),
            &Kind::Constant { value: 10.0 }
        );
        assert_eq!(
            parse("{uniform: {min: 1, max: 100}}").kind(),
            &Kind::Uniform {
                min: 1.0,
                max: 100.0
            }
        );
        assert_eq!(
            parse("{normal: {mean: 10000, sigma: 1000, min: 1}}").kind(),
            &Kind::Normal {
                mean: 10000.0,
                sigma: 1000.0
            }
        );
        assert_eq!(
            parse("{exponential: {mean: 10, min: 1}}").kind(),
            &Kind::Exponential { mean: 10.0 }
        );
        assert_eq!(
            parse("{empirical: {interpolate: false, samples: [3, 1, 2]}}").kind(),
            // sorted on construction
            &Kind::Empirical {
                samples: vec![1.0, 2.0, 3.0],
                interpolate: false
            }
        );
    }

    #[test]
    fn bounds_come_off_the_kind_map() {
        let d = parse("{exponential: {mean: 10, min: 1}}");
        assert_eq!(d.min(), Some(1.0));
        assert_eq!(d.max(), None);
    }

    #[test]
    fn accepts_every_spelling_of_infinity() {
        for text in [
            "{constant: inf}",
            "{constant: .inf}",
            "{constant: .Inf}",
            "{constant: INF}",
            "{constant: infinity}",
        ] {
            assert_eq!(
                parse(text).kind(),
                &Kind::Constant {
                    value: f64::INFINITY
                },
                "{text}"
            );
        }
        assert_eq!(
            parse("{constant: -inf}").kind(),
            &Kind::Constant {
                value: f64::NEG_INFINITY
            }
        );
    }

    #[test]
    fn rejects_unknown_kinds_and_stray_fields() {
        assert!(serde_yaml::from_str::<Distribution>("{pareto: {alpha: 1}}").is_err());
        assert!(
            serde_yaml::from_str::<Distribution>("{uniform: {min: 1, max: 2, gain: 3}}").is_err()
        );
        assert!(
            serde_yaml::from_str::<Distribution>("{constant: 1, uniform: {min: 0, max: 1}}")
                .is_err()
        );
    }

    #[test]
    fn rejects_unusable_parameters() {
        let bad = [
            "{normal: {mean: 1, sigma: 0}}",
            "{normal: {mean: 1, sigma: -1}}",
            "{exponential: {mean: 0}}",
            "{uniform: {min: 5, max: 1}}",
        ];
        for text in bad {
            assert!(parse(text).resolve(None).is_err(), "{text} was accepted");
        }
    }

    #[test]
    fn empty_truncation_is_an_error() {
        let d = parse("{uniform: {min: 0, max: 1}}").bounded(Some(5.0), Some(6.0));
        assert!(d.resolve(None).is_err());
    }
}
