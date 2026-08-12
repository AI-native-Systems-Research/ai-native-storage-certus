//! Per-statistic divergence between two measurements (spec FR-057, FR-057a).
//!
//! `fit` and `validate` both ask the same question — do these two reference streams
//! have the same shape — of the four FR-056 statistics: the reuse-distance CDF
//! (primary), the prefix-sharing depth histogram, the request-length distribution,
//! and the unique-keys-over-time curve.
//!
//! # One measure cannot serve all four, and neither can one tolerance
//!
//! Three measures for four statistics, each chosen because the alternatives were
//! measured and found wanting. `research.md` § Fit tolerances has the numbers.
//!
//! **Sharing depth and request length** are distributions compared by the
//! Kolmogorov–Smirnov distance, `sup |F_a(x) - F_b(x)|` — dimensionless, in
//! `[0, 1]`, and assuming nothing about shape. [`Hist`](super::hist) puts every
//! histogram on the same global bucket boundaries, so the two CDFs are evaluated at
//! *shared* bounds where each is exact and no part of the answer is an interpolation
//! artefact.
//!
//! **The reuse-distance CDF is a distribution too, and KS is the wrong measure for
//! it.** A large mass sits at the distance set by the live session population, which
//! makes the CDF steep there, and a supremum over a steep region moves a long way for
//! a small horizontal shift. Two plans from the same document differing only in seed
//! measured a KS floor of 0.06 to 0.31 — a tolerance above that would pass almost
//! anything — while the **area** between the same CDFs stayed at 0.002 to 0.014. So
//! the area is what the comparison gates on, and the sup is reported beside it,
//! because a large sup next to a small area is itself informative.
//!
//! **Unique-keys-over-time is not a distribution** but a monotone curve of counts
//! spanning orders of magnitude, so a difference of 0.05 means nothing without
//! saying 0.05 of what. Its measure is a relative error: the largest log ratio
//! between the two curves over the range they share, excluding the population ramp.
//!
//! **No tolerance can be shared**, which is what FR-057a is about: each floor
//! depends on how many samples are behind it, and the four statistics have wildly
//! different sample counts from the same plan — one per reference, one per request,
//! one per request, and a few dozen curve points.

use serde::{Deserialize, Serialize};

use super::unique::UniqueKeysReport;

/// Which statistic a divergence is about.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Statistic {
    /// The reuse-distance CDF in objects — the primary statistic.
    ReuseDistanceObjects,
    /// The reuse-distance CDF in bytes.
    ReuseDistanceBytes,
    /// The realised prefix-sharing depth histogram.
    SharingDepth,
    /// Blocks per request.
    RequestLength,
    /// Cumulative distinct keys against requests consumed.
    UniqueKeys,
}

impl Statistic {
    /// The measure this statistic is compared with.
    ///
    /// Three different measures for four statistics, each derived in `research.md`
    /// § Fit tolerances rather than chosen for uniformity:
    ///
    /// - The **reuse-distance** CDF is compared by **area**, not by supremum. Its
    ///   CDF has steep regions — a large mass sits at the distance set by the live
    ///   session population — so a small horizontal shift between two seeds of the
    ///   same document moves the sup a long way while barely changing the area.
    ///   Measured: a seed-to-seed sup floor of 0.06–0.31 against an area floor of
    ///   0.002–0.014, a ratio of 19 to 41.
    /// - **Sharing depth** and **request length** are compared by KS, whose floors
    ///   are already small and scale as `1/sqrt(n)`.
    /// - **Unique keys** is a curve of counts, not a distribution, so it is compared
    ///   by relative error.
    pub fn measure(&self) -> Measure {
        match self {
            Statistic::UniqueKeys => Measure::MaxLogRatio,
            Statistic::ReuseDistanceObjects | Statistic::ReuseDistanceBytes => {
                Measure::AreaBetweenCdfs
            }
            _ => Measure::KolmogorovSmirnov,
        }
    }

    /// A stable name for reports.
    pub fn name(&self) -> &'static str {
        match self {
            Statistic::ReuseDistanceObjects => "reuse_distance_objects",
            Statistic::ReuseDistanceBytes => "reuse_distance_bytes",
            Statistic::SharingDepth => "sharing_depth",
            Statistic::RequestLength => "request_length",
            Statistic::UniqueKeys => "unique_keys",
        }
    }
}

/// How two measurements of one statistic are compared.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Measure {
    /// `sup |F_a - F_b|`, dimensionless and in `[0, 1]`.
    KolmogorovSmirnov,
    /// Mean `|F_a - F_b|` over the occupied buckets — the area between the CDFs,
    /// also dimensionless and in `[0, 1]`, but not dominated by one steep region.
    AreaBetweenCdfs,
    /// `max |ln(a/b)|` over the shared domain — a relative error.
    MaxLogRatio,
}

/// One statistic's divergence, and whether it is within tolerance.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Divergence {
    /// Which statistic.
    pub statistic: Statistic,
    /// Which measure produced `value`.
    pub measure: Measure,
    /// The divergence.
    pub value: f64,
    /// The tolerance it was compared against.
    pub tolerance: f64,
    /// Whether the comparison passed.
    pub within: bool,
    /// Samples behind the smaller of the two measurements — the thing that sets
    /// this statistic's own noise floor, so a reader can tell a real divergence
    /// from one a thin sample could produce on its own.
    pub samples: u64,
    /// Why this statistic could not be compared, when it could not be.
    ///
    /// An incomparable statistic is **not** a passing one and not a failing one, so
    /// it is excluded from the verdict and carries its reason instead. The case this
    /// exists for: byte-weighted statistics between a plan and a trace. A plan's
    /// sizes are KV bytes and a trace's are tokens, and no trace in the corpus
    /// carries the `model_config` that would convert between them — so the two
    /// numbers are in different units and the divergence between them measures the
    /// unit, not the workload.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub incomparable: Option<String>,
    /// The KS distance, where the gated measure is the area.
    ///
    /// Reported rather than gated on, because it is the familiar number and because
    /// a large sup beside a small area is itself informative: it says the two CDFs
    /// agree in bulk and disagree over a narrow range of distances.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sup: Option<f64>,
}

/// Every statistic that could be compared, with its verdict.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Report {
    /// One entry per comparable statistic.
    pub divergences: Vec<Divergence>,
}

impl Report {
    /// Whether every *comparable* statistic is within its tolerance.
    ///
    /// An incomparable statistic neither passes nor fails: it was not measured on a
    /// common footing, so counting it either way would put a units artefact into a
    /// verdict about a workload.
    pub fn within_tolerance(&self) -> bool {
        self.divergences
            .iter()
            .filter(|d| d.incomparable.is_none())
            .all(|d| d.within)
    }

    /// The statistics that exceeded their tolerance.
    pub fn failures(&self) -> impl Iterator<Item = &Divergence> {
        self.divergences
            .iter()
            .filter(|d| d.incomparable.is_none() && !d.within)
    }

    /// Mark a statistic incomparable, with the reason it could not be compared.
    pub fn mark_incomparable(&mut self, statistic: Statistic, reason: impl Into<String>) {
        if let Some(d) = self
            .divergences
            .iter_mut()
            .find(|d| d.statistic == statistic)
        {
            d.incomparable = Some(reason.into());
        }
    }
}

/// The tolerance to apply to each statistic.
///
/// Per-statistic and never a single scalar (FR-057a). Supplied on the `fit` and
/// `validate` command line rather than in the YAML, because fitting is an operation
/// performed *on* a workload model rather than a property *of* one — a tolerance in
/// the document would make two models with identical workload content compare
/// unequal.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct Tolerances {
    /// Area-between-CDFs tolerance for the reuse-distance CDF in objects.
    pub reuse_distance_objects: f64,
    /// Area-between-CDFs tolerance for the reuse-distance CDF in bytes.
    pub reuse_distance_bytes: f64,
    /// KS tolerance for the prefix-sharing depth histogram.
    pub sharing_depth: f64,
    /// KS tolerance for the request-length distribution.
    pub request_length: f64,
    /// Max-log-ratio tolerance for the unique-keys curve.
    pub unique_keys: f64,
}

impl Default for Tolerances {
    /// The defaults derived in `research.md` § Fit tolerances.
    ///
    /// Each is set above the **seed-to-seed floor** for its statistic: the
    /// divergence between two plans from the *same* document differing only in
    /// seed. A tolerance below that floor would fail a model that is correct, and
    /// one far above it would pass a model that is not.
    fn default() -> Self {
        Tolerances {
            reuse_distance_objects: DEFAULT_REUSE_DISTANCE_OBJECTS,
            reuse_distance_bytes: DEFAULT_REUSE_DISTANCE_BYTES,
            sharing_depth: DEFAULT_SHARING_DEPTH,
            request_length: DEFAULT_REQUEST_LENGTH,
            unique_keys: DEFAULT_UNIQUE_KEYS,
        }
    }
}

/// The plan size these defaults are stated at, in requests.
///
/// A tolerance without a sample size is meaningless: every floor here falls with
/// the square root of the sample, so the same number is loose at one size and
/// impossible at another. `validate` must refuse to apply a default to a plan
/// materially smaller than this rather than silently comparing against a floor the
/// plan cannot reach.
pub const DEFAULT_TOLERANCE_MIN_REQUESTS: u64 = 50_000;

/// Area tolerance for the reuse-distance CDF in objects.
///
/// Derived: the seed-to-seed area floor at [`DEFAULT_TOLERANCE_MIN_REQUESTS`] is
/// 0.0063 at worst across the three shapes measured, so this is roughly 3x the
/// floor. See `research.md` § Fit tolerances.
pub const DEFAULT_REUSE_DISTANCE_OBJECTS: f64 = 0.02;
/// As [`DEFAULT_REUSE_DISTANCE_OBJECTS`]; the byte floor tracks the object floor to
/// within a few percent, every entry size being a function of key identity.
pub const DEFAULT_REUSE_DISTANCE_BYTES: f64 = 0.02;
/// KS tolerance for the prefix-sharing depth histogram. Floor 0.028, so ~2x.
pub const DEFAULT_SHARING_DEPTH: f64 = 0.05;
/// KS tolerance for the request-length distribution. Floor 0.0116, so ~2x.
pub const DEFAULT_REQUEST_LENGTH: f64 = 0.02;
/// Relative-error tolerance for the unique-keys curve. Floor 0.085, so ~2x.
pub const DEFAULT_UNIQUE_KEYS: f64 = 0.15;

/// KS distance between two bucket lists sharing one bucket scheme.
///
/// Evaluated at every bucket's **upper** bound, where each CDF is exact: a bucket
/// holds every sample `<= upper`, so the cumulative count there involves no
/// interpolation. The denominators are supplied because the interesting CDF is
/// often over more than the bucketed samples — a reuse-distance CDF is over every
/// reference, first touches included, and a sharing CDF over every request,
/// including the ones that shared nothing.
pub fn ks_from_buckets(
    a: &[(u64, u64, u64)],
    a_total: u64,
    b: &[(u64, u64, u64)],
    b_total: u64,
) -> f64 {
    if a_total == 0 || b_total == 0 {
        return 0.0;
    }
    let mut bounds: Vec<u64> = a
        .iter()
        .map(|(_, hi, _)| *hi)
        .chain(b.iter().map(|(_, hi, _)| *hi))
        .collect();
    bounds.sort_unstable();
    bounds.dedup();

    let cdf = |buckets: &[(u64, u64, u64)], total: u64, x: u64| -> f64 {
        let n: u64 = buckets
            .iter()
            .filter(|(_, hi, _)| *hi <= x)
            .map(|(_, _, c)| *c)
            .sum();
        n as f64 / total as f64
    };
    bounds
        .iter()
        .map(|x| (cdf(a, a_total, *x) - cdf(b, b_total, *x)).abs())
        .fold(0.0f64, f64::max)
}

/// One evaluation point of a bucket-by-bucket comparison of two CDFs.
///
/// A single divergence number says *how much* two distributions differ; it cannot say
/// **where**. That distinction decides what to do next: a KS distance of 0.23 whose
/// medians agree exactly is a tail or a shoulder problem, and is fixed by a different
/// parameter than a uniform shift would be. So the comparison can be asked for its
/// working, one row per shared bucket bound.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CdfRow {
    /// The bucket's inclusive upper bound — the value at which both CDFs are exact.
    pub upper: u64,
    /// Samples `a` placed in this bucket. Zero where `a` has no bucket at this bound.
    pub a_count: u64,
    /// Samples `b` placed in this bucket.
    pub b_count: u64,
    /// `F_a(upper)`.
    pub a_cdf: f64,
    /// `F_b(upper)`.
    pub b_cdf: f64,
}

impl CdfRow {
    /// `F_a - F_b`, signed: positive where `a` has already accumulated mass that `b`
    /// has not, which is to say where `a` is the *shorter*-tailed of the two.
    pub fn delta(&self) -> f64 {
        self.a_cdf - self.b_cdf
    }
}

/// The bucket-by-bucket working behind [`ks_from_buckets`] and [`l1_from_buckets`].
///
/// Evaluated at the union of both bucket schemes' upper bounds, exactly as the two
/// measures are, so the largest `|delta()|` here **is** the reported KS distance and
/// the mean of them is the reported area. A diagnostic that recomputed the CDFs a
/// second way could disagree with the verdict it is meant to explain.
pub fn cdf_rows(
    a: &[(u64, u64, u64)],
    a_total: u64,
    b: &[(u64, u64, u64)],
    b_total: u64,
) -> Vec<CdfRow> {
    if a_total == 0 || b_total == 0 {
        return Vec::new();
    }
    let mut bounds: Vec<u64> = a
        .iter()
        .map(|(_, hi, _)| *hi)
        .chain(b.iter().map(|(_, hi, _)| *hi))
        .collect();
    bounds.sort_unstable();
    bounds.dedup();

    let at = |buckets: &[(u64, u64, u64)], x: u64| -> u64 {
        buckets
            .iter()
            .filter(|(_, hi, _)| *hi == x)
            .map(|(_, _, c)| *c)
            .sum()
    };
    let mut a_acc = 0u64;
    let mut b_acc = 0u64;
    bounds
        .iter()
        .map(|x| {
            let a_count = at(a, *x);
            let b_count = at(b, *x);
            a_acc += a_count;
            b_acc += b_count;
            CdfRow {
                upper: *x,
                a_count,
                b_count,
                a_cdf: a_acc as f64 / a_total as f64,
                b_cdf: b_acc as f64 / b_total as f64,
            }
        })
        .collect()
}

/// Counts below which a curve point is dominated by its own counting noise.
///
/// The relative standard deviation of a count `n` is `1/sqrt(n)`, so at 100 the
/// noise on a single point is about 10% — a log ratio of 0.1 between two runs before
/// any difference in workload.
pub const CURVE_POINT_FLOOR: u64 = 100;

/// Fraction of the run the curve comparison skips at the start.
///
/// The head of a unique-keys curve is the **session-population ramp**, and its
/// composition is seed-dependent by construction: at request ordinal 7 one run has
/// accumulated 413 distinct keys and another 245, which is a log ratio of 0.52 and
/// says nothing about whether the two workloads have the same shape. By ordinal
/// 13 000 the same pair agrees to 3.9%, and by 50 000 to 0.9%.
///
/// A document that passes rule 15b already excludes the ramp, via a `warmup` long
/// enough to cover it — this is the same exclusion FR-045 makes, for the same
/// reason. But a document that sets no warmup leaves the ramp inside the measured
/// window, and a *trace* has no warmup concept at all, so the measure has to make
/// the exclusion itself rather than assume it was made upstream.
///
/// Without it the floor for this statistic measured **0.90 to 2.12 and identical
/// across three plan sizes** — the signature of a measure pinned by one point rather
/// than by its data, since a size-independent floor cannot be sampling noise.
pub const CURVE_RAMP_FRACTION: f64 = 0.1;

/// Mean absolute difference between two CDFs over the buckets they occupy.
///
/// The area between the curves, divided by the number of evaluation points — so it
/// is dimensionless and in `[0, 1]` like the KS distance, but it is an *average*
/// rather than a supremum.
///
/// The distinction matters for the reuse-distance CDF specifically, which has steep
/// regions: a small horizontal shift there moves the sup a long way while barely
/// changing the area. See `research.md` § Fit tolerances for the measurement that
/// motivated adding this — the seed-to-seed KS floor for the primary statistic runs
/// to 0.56 at plan sizes where the area floor is 0.02.
pub fn l1_from_buckets(
    a: &[(u64, u64, u64)],
    a_total: u64,
    b: &[(u64, u64, u64)],
    b_total: u64,
) -> f64 {
    if a_total == 0 || b_total == 0 {
        return 0.0;
    }
    let mut bounds: Vec<u64> = a
        .iter()
        .map(|(_, hi, _)| *hi)
        .chain(b.iter().map(|(_, hi, _)| *hi))
        .collect();
    bounds.sort_unstable();
    bounds.dedup();
    if bounds.is_empty() {
        return 0.0;
    }
    let cdf = |buckets: &[(u64, u64, u64)], total: u64, x: u64| -> f64 {
        let n: u64 = buckets
            .iter()
            .filter(|(_, hi, _)| *hi <= x)
            .map(|(_, _, c)| *c)
            .sum();
        n as f64 / total as f64
    };
    let sum: f64 = bounds
        .iter()
        .map(|x| (cdf(a, a_total, *x) - cdf(b, b_total, *x)).abs())
        .sum();
    sum / bounds.len() as f64
}

/// Largest `|ln(a/b)|` between two unique-keys curves, over their steady-state range.
///
/// Both curves are sampled at their own geometrically spaced request ordinals, so
/// `b` is interpolated at each of `a`'s ordinals. Three restrictions, each removing a
/// difference that is not a difference in workload shape:
///
/// - the **shared ordinal range**, since beyond it one run simply ran longer;
/// - the first [`CURVE_RAMP_FRACTION`] of it, which is the population ramp;
/// - points where either count is under [`CURVE_POINT_FLOOR`], where the measure
///   would report counting noise.
pub fn max_log_ratio_curve(a: &UniqueKeysReport, b: &UniqueKeysReport) -> f64 {
    worst_log_ratio_point(a, b)
        .map(|p| p.log_ratio)
        .unwrap_or(0.0)
}

/// The curve point where the two unique-keys curves disagree most, and by how much.
///
/// [`max_log_ratio_curve`] is this function's `log_ratio`, so the point it names is
/// the one that set the verdict — the unique-keys equivalent of asking a KS distance
/// where its supremum was. Returns `None` when the restrictions in
/// [`max_log_ratio_curve`] leave no comparable point, which is a real outcome and not
/// a divergence of zero.
pub fn worst_log_ratio_point(a: &UniqueKeysReport, b: &UniqueKeysReport) -> Option<CurveDelta> {
    if a.points.is_empty() || b.points.is_empty() {
        return None;
    }
    let b_lo = b.points.first().unwrap().requests;
    let b_hi = b.points.last().unwrap().requests;
    let a_hi = a.points.last().unwrap().requests;
    let shared_hi = a_hi.min(b_hi);
    let start = b_lo.max((shared_hi as f64 * CURVE_RAMP_FRACTION) as u64);
    let mut worst: Option<CurveDelta> = None;
    for p in &a.points {
        if p.requests < start || p.requests > b_hi || p.distinct_keys < CURVE_POINT_FLOOR {
            continue;
        }
        let Some(other) = interpolate(b, p.requests) else {
            continue;
        };
        if other < CURVE_POINT_FLOOR as f64 {
            continue;
        }
        let ratio = ((p.distinct_keys as f64) / other).ln();
        // `map_or(true, ..)` rather than `is_none_or`, which is stable only since
        // 1.82 and this workspace's MSRV is 1.75.
        if worst.as_ref().map_or(true, |w| ratio.abs() > w.log_ratio) {
            worst = Some(CurveDelta {
                requests: p.requests,
                a_distinct: p.distinct_keys,
                b_distinct: other,
                log_ratio: ratio.abs(),
                signed_log_ratio: ratio,
            });
        }
    }
    worst
}

/// The worst-disagreeing point of a unique-keys curve comparison.
#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub struct CurveDelta {
    /// The request ordinal the two curves were compared at.
    pub requests: u64,
    /// `a`'s distinct-key count there.
    pub a_distinct: u64,
    /// `b`'s count there, interpolated onto `a`'s ordinal.
    pub b_distinct: f64,
    /// `|ln(a/b)|` — the value the verdict used.
    pub log_ratio: f64,
    /// The same ratio with its sign, so a reader can tell which curve is above.
    pub signed_log_ratio: f64,
}

/// The bucket table behind one statistic's verdict.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Explanation {
    /// Which statistic these rows explain.
    pub statistic: Statistic,
    /// One row per shared bucket bound.
    pub rows: Vec<CdfRow>,
    /// The denominator behind `a_cdf` — often more than the bucketed samples.
    pub a_total: u64,
    /// The denominator behind `b_cdf`.
    pub b_total: u64,
}

/// The bucket-by-bucket working behind a [`compare`] verdict.
///
/// Covers the four statistics that are distributions. Unique-keys is a monotone curve
/// rather than a distribution and has no buckets to tabulate;
/// [`worst_log_ratio_point`] is the equivalent question for it.
pub fn explain(a: &super::Report, b: &super::Report) -> Vec<Explanation> {
    let refs_a = a.reuse_distance.references;
    let refs_b = b.reuse_distance.references;
    [
        (
            Statistic::ReuseDistanceObjects,
            &a.reuse_distance.object_buckets,
            refs_a,
            &b.reuse_distance.object_buckets,
            refs_b,
        ),
        (
            Statistic::ReuseDistanceBytes,
            &a.reuse_distance.byte_buckets,
            refs_a,
            &b.reuse_distance.byte_buckets,
            refs_b,
        ),
        (
            Statistic::SharingDepth,
            &a.sharing.depth_buckets,
            a.sharing.requests,
            &b.sharing.depth_buckets,
            b.sharing.requests,
        ),
        (
            Statistic::RequestLength,
            &a.request_length.block_buckets,
            a.request_length.requests,
            &b.request_length.block_buckets,
            b.request_length.requests,
        ),
    ]
    .into_iter()
    .map(|(statistic, ab, at, bb, bt)| Explanation {
        statistic,
        rows: cdf_rows(ab, at, bb, bt),
        a_total: at,
        b_total: bt,
    })
    .collect()
}

/// Linear interpolation of a curve's distinct-key count at `requests`.
fn interpolate(curve: &UniqueKeysReport, requests: u64) -> Option<f64> {
    let pts = &curve.points;
    if pts.is_empty() {
        return None;
    }
    let mut prev = pts[0];
    for p in pts {
        if p.requests >= requests {
            if p.requests == prev.requests {
                return Some(p.distinct_keys as f64);
            }
            let span = (p.requests - prev.requests) as f64;
            let frac = (requests - prev.requests) as f64 / span;
            let lo = prev.distinct_keys as f64;
            let hi = p.distinct_keys as f64;
            return Some(lo + frac * (hi - lo));
        }
        prev = *p;
    }
    Some(pts[pts.len() - 1].distinct_keys as f64)
}

/// Compare two plan reports statistic by statistic.
pub fn compare(a: &super::Report, b: &super::Report, tol: &Tolerances) -> Report {
    let mut out = Vec::new();

    // The reuse-distance CDFs are gated on area and carry their sup alongside.
    let refs = a.reuse_distance.references.min(b.reuse_distance.references);
    for (statistic, a_buckets, b_buckets, tolerance) in [
        (
            Statistic::ReuseDistanceObjects,
            &a.reuse_distance.object_buckets,
            &b.reuse_distance.object_buckets,
            tol.reuse_distance_objects,
        ),
        (
            Statistic::ReuseDistanceBytes,
            &a.reuse_distance.byte_buckets,
            &b.reuse_distance.byte_buckets,
            tol.reuse_distance_bytes,
        ),
    ] {
        let area = l1_from_buckets(
            a_buckets,
            a.reuse_distance.references,
            b_buckets,
            b.reuse_distance.references,
        );
        let sup = ks_from_buckets(
            a_buckets,
            a.reuse_distance.references,
            b_buckets,
            b.reuse_distance.references,
        );
        out.push(Divergence {
            statistic,
            measure: statistic.measure(),
            value: area,
            tolerance,
            within: area <= tolerance,
            samples: refs,
            incomparable: None,
            sup: Some(sup),
        });
    }

    let mut push = |statistic: Statistic, value: f64, tolerance: f64, samples: u64| {
        out.push(Divergence {
            statistic,
            measure: statistic.measure(),
            value,
            tolerance,
            within: value <= tolerance,
            samples,
            incomparable: None,
            sup: None,
        });
    };

    push(
        Statistic::SharingDepth,
        ks_from_buckets(
            &a.sharing.depth_buckets,
            a.sharing.requests,
            &b.sharing.depth_buckets,
            b.sharing.requests,
        ),
        tol.sharing_depth,
        a.sharing.requests.min(b.sharing.requests),
    );
    push(
        Statistic::RequestLength,
        ks_from_buckets(
            &a.request_length.block_buckets,
            a.request_length.requests,
            &b.request_length.block_buckets,
            b.request_length.requests,
        ),
        tol.request_length,
        a.request_length.requests.min(b.request_length.requests),
    );
    push(
        Statistic::UniqueKeys,
        max_log_ratio_curve(&a.unique_keys, &b.unique_keys),
        tol.unique_keys,
        a.unique_keys.points.len().min(b.unique_keys.points.len()) as u64,
    );

    Report { divergences: out }
}

/// Compare a report against itself — which must be exactly zero everywhere.
///
/// Not a test helper. `validate` compares a plan against a trace, and a plan
/// against a plan, and if the identity case were not exactly zero every divergence
/// it reports would carry an unknown offset.
pub fn self_divergence(a: &super::Report) -> Report {
    compare(a, a, &Tolerances::default())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::keys::{CacheKey, SessionId};
    use crate::stats::{Ref, Statistics};

    fn report(requests: &[(u32, Vec<u64>)], window: u64) -> crate::stats::Report {
        let mut s = Statistics::new(window);
        for (session, path) in requests {
            for (i, k) in path.iter().enumerate() {
                s.push(&Ref {
                    key: CacheKey(*k),
                    size: 4096,
                    depth: i as u32,
                    session: SessionId(*session),
                    request_start: i == 0,
                    warmup: false,
                });
            }
        }
        s.finish()
    }

    fn shaped(n: u32, span: u64, depth: usize) -> Vec<(u32, Vec<u64>)> {
        (0..n)
            .map(|i| {
                (
                    i % 16,
                    (0..depth)
                        .map(|d| (u64::from(i) % span) * 100 + d as u64)
                        .collect(),
                )
            })
            .collect()
    }

    #[test]
    fn a_report_does_not_diverge_from_itself() {
        // The identity case must be exactly zero, or every reported divergence
        // carries an unknown offset.
        let r = report(&shaped(400, 20, 4), 100);
        let d = self_divergence(&r);
        for x in &d.divergences {
            assert_eq!(x.value, 0.0, "{} diverged from itself", x.statistic.name());
        }
        assert!(d.within_tolerance());
    }

    #[test]
    fn a_different_shape_diverges_on_the_statistic_that_differs() {
        // Longer requests move request length and, through it, reuse distance —
        // but the point of per-statistic reporting is that a reader sees *which*.
        let short = report(&shaped(400, 20, 2), 100);
        let long = report(&shaped(400, 20, 40), 100);
        let d = compare(&short, &long, &Tolerances::default());
        let by = |s: Statistic| {
            d.divergences
                .iter()
                .find(|x| x.statistic == s)
                .expect("present")
                .value
        };
        assert!(
            by(Statistic::RequestLength) > 0.9,
            "request length should differ almost completely, got {}",
            by(Statistic::RequestLength)
        );
        assert!(by(Statistic::ReuseDistanceObjects) > 0.1);
    }

    #[test]
    fn the_ks_distance_is_bounded_and_symmetric() {
        let a = report(&shaped(300, 10, 3), 100);
        let b = report(&shaped(300, 40, 9), 100);
        let ab = compare(&a, &b, &Tolerances::default());
        let ba = compare(&b, &a, &Tolerances::default());
        for (x, y) in ab.divergences.iter().zip(ba.divergences.iter()) {
            assert!(
                (x.value - y.value).abs() < 1e-12,
                "{} is not symmetric",
                x.statistic.name()
            );
            if x.measure == Measure::KolmogorovSmirnov {
                assert!((0.0..=1.0).contains(&x.value), "{} out of range", x.value);
            }
        }
    }

    #[test]
    fn the_unique_keys_measure_is_relative_not_absolute() {
        // Two curves an order of magnitude apart in count must diverge by about
        // ln(10) whatever the absolute counts, which is the property that makes a
        // single tolerance meaningful across workloads of different sizes.
        let small = report(&shaped(200, 200, 1), 1000);
        let large = report(&shaped(2000, 2000, 1), 10_000);
        let d = compare(&small, &large, &Tolerances::default());
        let v = d
            .divergences
            .iter()
            .find(|x| x.statistic == Statistic::UniqueKeys)
            .unwrap();
        assert_eq!(v.measure, Measure::MaxLogRatio);
        // Both curves are "every request novel", so on their shared ordinal range
        // they agree: a relative measure sees no divergence between a short run and
        // a long one of the same workload.
        assert!(v.value < 0.05, "relative divergence {} too large", v.value);
    }

    #[test]
    fn the_reuse_distance_cdf_is_gated_on_area_and_reports_its_sup() {
        // The design this module exists to get right: the sup of a CDF with steep
        // regions moves a long way for a small shift, so it is reported rather than
        // gated on. A sup at least as large as the area is arithmetic — the mean of
        // a set cannot exceed its maximum — and the test is that both are present
        // and that the gate used the area.
        let a = report(&shaped(400, 20, 6), 100);
        let b = report(&shaped(400, 24, 7), 100);
        let d = compare(&a, &b, &Tolerances::default());
        for s in [
            Statistic::ReuseDistanceObjects,
            Statistic::ReuseDistanceBytes,
        ] {
            let x = d.divergences.iter().find(|x| x.statistic == s).unwrap();
            assert_eq!(x.measure, Measure::AreaBetweenCdfs);
            let sup = x.sup.expect("the sup is reported alongside");
            assert!(sup >= x.value - 1e-12, "sup {sup} below area {}", x.value);
            assert!(x.within == (x.value <= x.tolerance));
        }
        // The other three gate on their own measure and report no sup.
        for s in [
            Statistic::SharingDepth,
            Statistic::RequestLength,
            Statistic::UniqueKeys,
        ] {
            let x = d.divergences.iter().find(|x| x.statistic == s).unwrap();
            assert!(x.sup.is_none(), "{} should not carry a sup", s.name());
        }
    }

    #[test]
    fn the_tolerance_defaults_name_the_size_they_hold_at() {
        // A tolerance without a sample size is meaningless: every floor behind these
        // defaults falls as 1/sqrt(n), so the same number is loose at one plan size
        // and unreachable at another.
        assert_eq!(DEFAULT_TOLERANCE_MIN_REQUESTS, 50_000);
        let t = Tolerances::default();
        assert!(t.reuse_distance_objects > 0.0 && t.reuse_distance_objects < t.unique_keys);
        assert_eq!(t.reuse_distance_objects, t.reuse_distance_bytes);
    }

    #[test]
    fn every_fr_056_statistic_is_compared() {
        let r = report(&shaped(200, 20, 3), 100);
        let d = self_divergence(&r);
        let names: Vec<&str> = d.divergences.iter().map(|x| x.statistic.name()).collect();
        for expected in [
            "reuse_distance_objects",
            "sharing_depth",
            "request_length",
            "unique_keys",
        ] {
            assert!(names.contains(&expected), "{expected} not compared");
        }
    }

    #[test]
    fn a_failure_names_the_statistic_and_its_tolerance() {
        let a = report(&shaped(400, 20, 2), 100);
        let b = report(&shaped(400, 20, 40), 100);
        let d = compare(&a, &b, &Tolerances::default());
        assert!(!d.within_tolerance());
        let f: Vec<&str> = d.failures().map(|x| x.statistic.name()).collect();
        assert!(f.contains(&"request_length"), "failures were {f:?}");
        for x in d.failures() {
            assert!(x.value > x.tolerance);
            assert_eq!(x.measure, x.statistic.measure());
        }
    }

    #[test]
    fn the_bucket_table_reproduces_the_verdict_it_explains() {
        // The whole point of the explanation is that it is the *same* arithmetic: the
        // largest row delta must be the reported KS distance and the mean of them the
        // reported area, or the diagnostic would send a reader after a divergence the
        // verdict never saw.
        let a = report(&shaped(500, 20, 6), 100);
        let b = report(&shaped(500, 31, 11), 100);
        let d = compare(&a, &b, &Tolerances::default());
        for e in explain(&a, &b) {
            let x = d
                .divergences
                .iter()
                .find(|x| x.statistic == e.statistic)
                .expect("every explained statistic is compared");
            let sup = e
                .rows
                .iter()
                .map(|r| r.delta().abs())
                .fold(0.0f64, f64::max);
            let area = e.rows.iter().map(|r| r.delta().abs()).sum::<f64>() / e.rows.len() as f64;
            match x.measure {
                Measure::KolmogorovSmirnov => assert!(
                    (sup - x.value).abs() < 1e-12,
                    "{}: table sup {sup} against reported {}",
                    e.statistic.name(),
                    x.value
                ),
                Measure::AreaBetweenCdfs => {
                    assert!(
                        (area - x.value).abs() < 1e-12,
                        "{}: table area {area} against reported {}",
                        e.statistic.name(),
                        x.value
                    );
                    let reported_sup = x.sup.expect("area gates carry their sup");
                    assert!((sup - reported_sup).abs() < 1e-12);
                }
                Measure::MaxLogRatio => unreachable!("curves are not tabulated"),
            }
        }
    }

    #[test]
    fn the_bucket_table_says_where_and_not_only_how_much() {
        // Two distributions with the same median and different tails: the divergence
        // is real, and the table has to put it in the tail rather than at the median,
        // since that difference is what decides which parameter to reach for.
        let a = report(&shaped(600, 20, 4), 100);
        let mut long = shaped(600, 20, 4);
        for (i, (_, path)) in long.iter_mut().enumerate() {
            if i % 10 == 0 {
                let base = path[0];
                path.extend((0..60).map(|d| base + 1000 + d));
            }
        }
        let b = report(&long, 100);
        let e = explain(&a, &b)
            .into_iter()
            .find(|e| e.statistic == Statistic::RequestLength)
            .unwrap();
        let median_row = e
            .rows
            .iter()
            .find(|r| r.a_cdf >= 0.5)
            .expect("a median exists");
        let worst = e
            .rows
            .iter()
            .max_by(|x, y| x.delta().abs().total_cmp(&y.delta().abs()))
            .unwrap();
        assert!(
            worst.delta().abs() > 0.05,
            "fixture does not diverge: {}",
            worst.delta()
        );
        assert!(
            median_row.upper <= worst.upper,
            "the divergence landed at or below the median, so this fixture does not \
             test tail attribution"
        );
        // `a` is the shorter-tailed side, so it accumulates its mass first.
        assert!(worst.delta() > 0.0);
    }

    #[test]
    fn the_curve_comparison_names_the_point_that_set_its_verdict() {
        let small = report(&shaped(400, 400, 1), 1000);
        let large = report(&shaped(4000, 40, 1), 10_000);
        let v = max_log_ratio_curve(&small.unique_keys, &large.unique_keys);
        let p = worst_log_ratio_point(&small.unique_keys, &large.unique_keys);
        match p {
            Some(p) => {
                assert!((p.log_ratio - v).abs() < 1e-12);
                assert_eq!(p.log_ratio, p.signed_log_ratio.abs());
                assert!(p.requests > 0 && p.a_distinct >= CURVE_POINT_FLOOR);
            }
            // No comparable point is a real outcome, and then the measure is zero.
            None => assert_eq!(v, 0.0),
        }
    }

    #[test]
    fn samples_are_reported_so_a_thin_comparison_is_visible() {
        // A KS distance from a handful of samples is noise; the count is the only
        // way a reader can tell.
        let tiny = report(&shaped(3, 3, 2), 100);
        let big = report(&shaped(4000, 40, 2), 100);
        let d = compare(&tiny, &big, &Tolerances::default());
        for x in &d.divergences {
            if x.measure == Measure::KolmogorovSmirnov {
                assert!(
                    x.samples <= 6,
                    "{} reported {} samples",
                    x.statistic.name(),
                    x.samples
                );
            }
        }
    }
}
