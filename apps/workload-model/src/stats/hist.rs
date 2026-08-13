//! A log-spaced histogram over non-negative integers.
//!
//! Shared by every distribution the report publishes, so that "the reuse-distance
//! CDF" and "the request-length distribution" mean the same kind of object and a
//! comparison between two of them (spec FR-057) compares like with like.
//!
//! Buckets are **exact** below [`LINEAR`] and [`PER_OCTAVE`] to the octave above
//! it, which bounds a bucket's relative width at `1/PER_OCTAVE` — about 3%. That
//! matters because the CDF is read at bucket boundaries: at a boundary the
//! cumulative probability is exact, and only the *value* axis is quantised. A
//! divergence measured between two histograms at shared boundaries therefore
//! carries no interpolation error at all.
//!
//! The whole table is under 16 KiB regardless of the data's range, which is what
//! lets `report` hold eight of these plus a key table within a sane footprint on
//! a 10^7-event plan (spec SC-004).

use serde::{Deserialize, Serialize};

/// Buckets per octave above [`LINEAR`]. A power of two so bucketing is shifts.
pub const PER_OCTAVE: u64 = 32;

/// Values below this are counted exactly, one bucket each.
pub const LINEAR: u64 = PER_OCTAVE;

/// Number of buckets: the linear run, then one octave per bit.
const BUCKETS: usize = (LINEAR as usize) + 59 * (PER_OCTAVE as usize) + (PER_OCTAVE as usize);

/// The exponent offset, as a shift width.
const SUB: u32 = PER_OCTAVE.trailing_zeros();

/// The bucket `v` falls in.
///
/// Contiguous with the linear run by construction: within the first octave above
/// [`LINEAR`] the index *equals* the value, so `bucket(31) == 31` and
/// `bucket(32) == 32`.
pub fn bucket(v: u64) -> usize {
    if v < LINEAR {
        return v as usize;
    }
    let e = 63 - v.leading_zeros();
    let mant = (v >> (e - SUB)) - LINEAR;
    (LINEAR as usize) + ((e - SUB) as usize) * (PER_OCTAVE as usize) + (mant as usize)
}

/// The smallest value that lands in `idx` — a bucket's inclusive lower bound.
pub fn lower_bound(idx: usize) -> u64 {
    if (idx as u64) < LINEAR {
        return idx as u64;
    }
    let j = idx as u64 - LINEAR;
    let octave = j / PER_OCTAVE;
    let mant = j % PER_OCTAVE;
    (LINEAR + mant) << octave
}

/// The largest value that lands in `idx` — a bucket's inclusive upper bound.
///
/// This, not the lower bound, is the value at which the CDF over this bucket is
/// exact: every sample in the bucket is `<= upper_bound(idx)`.
pub fn upper_bound(idx: usize) -> u64 {
    if (idx as u64) < LINEAR {
        return idx as u64;
    }
    lower_bound(idx + 1) - 1
}

/// A distribution of non-negative integers, plus the exact moments.
///
/// The count, sum, minimum and maximum are exact; only quantiles carry the
/// bucket's width as error, and [`Hist::quantile`] says which way it rounds.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Hist {
    /// Occupancy per bucket. Sparse in practice; serialised sparsely.
    #[serde(skip)]
    counts: Vec<u64>,
    /// Samples recorded.
    count: u64,
    /// Exact sum of every sample, so the mean is not a bucket estimate.
    sum: u128,
    /// Exact extremes.
    min: u64,
    max: u64,
    /// The non-empty buckets, as `(lower_bound, upper_bound, count)`.
    ///
    /// Populated by [`Hist::seal`]; this is the serialised form, and the full
    /// CDF is recoverable from it.
    buckets: Vec<(u64, u64, u64)>,
}

impl Default for Hist {
    fn default() -> Self {
        Hist::new()
    }
}

impl Hist {
    /// An empty histogram.
    pub fn new() -> Hist {
        Hist {
            counts: Vec::new(),
            count: 0,
            sum: 0,
            min: u64::MAX,
            max: 0,
            buckets: Vec::new(),
        }
    }

    /// Record one sample.
    pub fn add(&mut self, v: u64) {
        if self.counts.is_empty() {
            self.counts = vec![0; BUCKETS];
        }
        self.counts[bucket(v)] += 1;
        self.count += 1;
        self.sum += u128::from(v);
        self.min = self.min.min(v);
        self.max = self.max.max(v);
    }

    /// Fold another histogram into this one.
    ///
    /// Exact for count, sum, min and max, and bucket-for-bucket for the occupancy — so
    /// a merged histogram is indistinguishable from one that saw both sample sets
    /// directly. That matters because the merge exists to combine sparse
    /// session-length bands when fitting `growth_per_turn`, and a band that reported a
    /// different mean after merging than before would make the band boundaries
    /// load-bearing in a way they are not meant to be.
    ///
    /// Either side may be sealed. A sealed histogram keeps its occupancy in the frozen
    /// bucket list rather than the dense table, so both are re-expanded before folding:
    /// merging into a sealed receiver without doing so would start an empty dense table
    /// beside a stale frozen list, and the receiver's own samples would vanish from every
    /// quantile while `count` still reported them. The result is always unsealed, and
    /// [`Hist::seal`] may be called on it afterwards.
    pub fn merge(&mut self, other: &Hist) {
        if other.count == 0 {
            return;
        }
        if self.counts.is_empty() {
            self.counts = vec![0; BUCKETS];
            // Sealed receiver: recover its own occupancy from the frozen list, then drop
            // that list, since the dense table is now the authority for both sides.
            for (lo, _, c) in std::mem::take(&mut self.buckets) {
                self.counts[bucket(lo)] += c;
            }
        }
        if !other.counts.is_empty() {
            for (a, b) in self.counts.iter_mut().zip(other.counts.iter()) {
                *a += *b;
            }
        } else {
            // Sealed source: same recovery, at each bucket's own index so the merged
            // occupancy lands where it did before.
            for (lo, _, c) in &other.buckets {
                self.counts[bucket(*lo)] += *c;
            }
        }
        self.count += other.count;
        self.sum += other.sum;
        self.min = self.min.min(other.min);
        self.max = self.max.max(other.max);
    }

    /// Samples recorded.
    pub fn count(&self) -> u64 {
        self.count
    }

    /// Exact mean, or `None` when empty.
    pub fn mean(&self) -> Option<f64> {
        if self.count == 0 {
            None
        } else {
            Some(self.sum as f64 / self.count as f64)
        }
    }

    /// Exact minimum, or `None` when empty.
    pub fn min(&self) -> Option<u64> {
        if self.count == 0 {
            None
        } else {
            Some(self.min)
        }
    }

    /// Exact maximum, or `None` when empty.
    pub fn max(&self) -> Option<u64> {
        if self.count == 0 {
            None
        } else {
            Some(self.max)
        }
    }

    /// Samples `<= v`, exactly when `v` is a bucket's upper bound.
    ///
    /// For a `v` interior to a bucket the whole bucket is counted, so the result
    /// is an upper bound on the true count. Callers comparing two histograms
    /// should evaluate at [`upper_bound`] values, where the answer is exact.
    ///
    /// Reads the dense table or the sealed bucket list, whichever this histogram
    /// currently holds. Both are supported deliberately: an earlier version read
    /// only the dense table and silently returned the *maximum* for every
    /// quantile once sealed, which is a wrong answer that looks like a plausible
    /// one — every percentile equal to the largest sample.
    pub fn count_le(&self, v: u64) -> u64 {
        if self.count == 0 {
            return 0;
        }
        if self.counts.is_empty() {
            return self
                .buckets
                .iter()
                .filter(|(lo, _, _)| *lo <= v)
                .map(|(_, _, c)| *c)
                .sum();
        }
        let top = bucket(v);
        self.counts[..=top.min(BUCKETS - 1)].iter().sum()
    }

    /// Fraction of samples `<= v`, over a caller-supplied denominator.
    ///
    /// The denominator is explicit because the interesting CDF is often over
    /// *more* than the samples in this histogram: a reuse-distance CDF is over
    /// every reference, including first touches, which have no finite distance
    /// to record. Passing `hist.count()` gives the CDF over the samples alone.
    pub fn fraction_le(&self, v: u64, denominator: u64) -> f64 {
        if denominator == 0 {
            return 0.0;
        }
        self.count_le(v) as f64 / denominator as f64
    }

    /// The bucket lower bound at which the cumulative fraction first reaches `p`.
    ///
    /// Rounds **down** to the bucket's lower bound, so a reported quantile is
    /// always a value the data could have taken.
    pub fn quantile(&self, p: f64) -> Option<u64> {
        if self.count == 0 {
            return None;
        }
        let target = (p.clamp(0.0, 1.0) * self.count as f64).ceil() as u64;
        let target = target.max(1);
        let mut acc = 0u64;
        if self.counts.is_empty() {
            for (lo, _, c) in &self.buckets {
                acc += c;
                if acc >= target {
                    return Some(*lo);
                }
            }
        } else {
            for (i, c) in self.counts.iter().enumerate() {
                acc += c;
                if acc >= target {
                    return Some(lower_bound(i));
                }
            }
        }
        Some(self.max)
    }

    /// The non-empty buckets as `(lower, upper, count)`, ascending.
    pub fn buckets(&self) -> Vec<(u64, u64, u64)> {
        if !self.buckets.is_empty() || self.counts.is_empty() {
            return self.buckets.clone();
        }
        self.counts
            .iter()
            .enumerate()
            .filter(|(_, c)| **c > 0)
            .map(|(i, c)| (lower_bound(i), upper_bound(i), *c))
            .collect()
    }

    /// Freeze the sparse bucket list for serialisation and drop the dense table.
    ///
    /// Called once at the end of accumulation. After sealing the histogram is
    /// read-only: [`Hist::add`] would start a fresh dense table and the two
    /// would disagree, so a sealed histogram is never added to.
    pub fn seal(&mut self) {
        if self.buckets.is_empty() {
            self.buckets = self.buckets();
        }
        self.counts = Vec::new();
    }

    /// Quantiles at the fractions a report always shows.
    pub fn summary(&self) -> Quantiles {
        Quantiles {
            count: self.count,
            mean: self.mean(),
            min: self.min(),
            max: self.max(),
            p50: self.quantile(0.50),
            p90: self.quantile(0.90),
            p99: self.quantile(0.99),
            p999: self.quantile(0.999),
        }
    }
}

/// The fixed quantile set every distribution in a report shows.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Quantiles {
    /// Samples behind these figures.
    pub count: u64,
    /// Exact mean.
    pub mean: Option<f64>,
    /// Exact minimum.
    pub min: Option<u64>,
    /// Exact maximum.
    pub max: Option<u64>,
    /// Median, rounded down to a bucket bound.
    pub p50: Option<u64>,
    /// 90th percentile.
    pub p90: Option<u64>,
    /// 99th percentile.
    pub p99: Option<u64>,
    /// 99.9th percentile.
    pub p999: Option<u64>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_linear_run_is_exact_and_contiguous_with_the_first_octave() {
        // The property the CDF comparison leans on: below LINEAR a bucket holds
        // exactly one value, and index == value across the join.
        for v in 0..LINEAR {
            assert_eq!(bucket(v), v as usize);
            assert_eq!(lower_bound(bucket(v)), v);
            assert_eq!(upper_bound(bucket(v)), v);
        }
        for v in LINEAR..2 * LINEAR {
            assert_eq!(bucket(v), v as usize, "first octave should be exact too");
            assert_eq!(upper_bound(bucket(v)), v);
        }
    }

    #[test]
    fn bucketing_is_monotone_and_bounds_are_consistent() {
        let mut last = 0usize;
        for v in 0..100_000u64 {
            let b = bucket(v);
            assert!(b >= last, "bucketing must be monotone in the value");
            assert!(lower_bound(b) <= v, "{v} below its bucket's lower bound");
            assert!(upper_bound(b) >= v, "{v} above its bucket's upper bound");
            last = b;
        }
    }

    #[test]
    fn relative_bucket_width_stays_within_one_part_in_per_octave() {
        // What licenses reading the CDF at boundaries: the value axis is
        // quantised by at most 1/PER_OCTAVE, so a divergence measured at shared
        // boundaries is not a bucketing artefact.
        for v in [LINEAR, 1_000, 1 << 20, 1 << 40, u64::MAX / 4] {
            let b = bucket(v);
            let (lo, hi) = (lower_bound(b), upper_bound(b));
            let width = (hi - lo + 1) as f64;
            assert!(
                width / lo as f64 <= 1.0 / PER_OCTAVE as f64 + f64::EPSILON,
                "bucket at {v} spans {lo}..={hi}, too wide"
            );
        }
    }

    #[test]
    fn extremes_and_mean_are_exact_not_bucket_estimates() {
        let mut h = Hist::new();
        for v in [3u64, 7, 1_000_003, 999] {
            h.add(v);
        }
        assert_eq!(h.count(), 4);
        assert_eq!(h.min(), Some(3));
        assert_eq!(h.max(), Some(1_000_003));
        // Exactly the arithmetic mean, not the mean of bucket midpoints.
        assert_eq!(h.mean(), Some((3.0 + 7.0 + 1_000_003.0 + 999.0) / 4.0));
    }

    #[test]
    fn the_cdf_is_exact_at_bucket_upper_bounds() {
        let mut h = Hist::new();
        for v in 0..1_000u64 {
            h.add(v);
        }
        for idx in 0..bucket(999) {
            let hi = upper_bound(idx);
            if hi >= 999 {
                break;
            }
            // Values 0..=hi were all added exactly once.
            assert_eq!(h.count_le(hi), hi + 1, "cdf wrong at boundary {hi}");
        }
    }

    #[test]
    fn a_caller_supplied_denominator_carries_mass_outside_the_histogram() {
        // A reuse-distance CDF is over every reference; first touches have no
        // finite distance, so they are denominator without being samples.
        let mut h = Hist::new();
        h.add(1);
        h.add(2);
        assert_eq!(h.fraction_le(2, h.count()), 1.0);
        assert_eq!(h.fraction_le(2, 4), 0.5);
    }

    #[test]
    fn quantiles_round_down_to_a_representable_value() {
        let mut h = Hist::new();
        for _ in 0..100 {
            h.add(10);
        }
        assert_eq!(h.quantile(0.5), Some(10));
        assert_eq!(h.quantile(1.0), Some(10));
        assert_eq!(h.quantile(0.0), Some(10));
    }

    #[test]
    fn sealing_preserves_the_bucket_list_and_frees_the_dense_table() {
        let mut h = Hist::new();
        for v in [1u64, 1, 5, 1_000_000] {
            h.add(v);
        }
        let before = h.buckets();
        h.seal();
        assert_eq!(h.buckets(), before);
        assert_eq!(h.count(), 4);
        // Totals survive; the table does not.
        assert_eq!(h.max(), Some(1_000_000));
    }

    #[test]
    fn merging_is_indistinguishable_from_seeing_both_sample_sets() {
        // `fit::sessions` merges sparse session-length bands, and a merge that shifted
        // any moment would make the band boundaries load-bearing when they are only a
        // fitting convenience. So the assertion is equality with the direct histogram,
        // not a tolerance.
        let a: [u64; 5] = [1, 1, 5, 40, 1_000_000];
        let b: [u64; 4] = [0, 5, 5, 97];
        let mut direct = Hist::new();
        let mut left = Hist::new();
        let mut right = Hist::new();
        for v in a {
            direct.add(v);
            left.add(v);
        }
        for v in b {
            direct.add(v);
            right.add(v);
        }
        left.merge(&right);
        assert_eq!(left.count(), direct.count());
        assert_eq!(left.mean(), direct.mean());
        assert_eq!(left.min(), direct.min());
        assert_eq!(left.max(), direct.max());
        assert_eq!(left.buckets(), direct.buckets());
        for p in [0.0, 0.1, 0.5, 0.9, 0.99, 1.0] {
            assert_eq!(left.quantile(p), direct.quantile(p), "quantile {p}");
        }
    }

    #[test]
    fn merging_a_sealed_histogram_into_a_sealed_one_loses_nothing() {
        // A sealed histogram keeps its occupancy in the frozen bucket list and has no
        // dense table. Merging into one used to start a fresh dense table beside the
        // stale frozen list, so the receiver's own samples vanished from every quantile
        // while `count` went on reporting them -- the same class of defect as reading a
        // quantile off a sealed histogram, and just as invisible at a glance.
        let a: [u64; 4] = [2, 2, 60, 5_000];
        let b: [u64; 3] = [7, 60, 900_000];
        let mut direct = Hist::new();
        for v in a.iter().chain(b.iter()) {
            direct.add(*v);
        }
        let mut left = Hist::new();
        for v in a {
            left.add(v);
        }
        let mut right = Hist::new();
        for v in b {
            right.add(v);
        }
        left.seal();
        right.seal();
        left.merge(&right);
        assert_eq!(left.count(), direct.count());
        assert_eq!(left.mean(), direct.mean());
        assert_eq!(left.min(), direct.min());
        assert_eq!(left.max(), direct.max());
        // The frozen list was dropped in favour of the recovered dense table, so the
        // merged histogram is a normal unsealed one and sealing it again agrees.
        assert_eq!(left.buckets(), direct.buckets());
        left.seal();
        assert_eq!(left.buckets(), direct.buckets());
        for p in [0.0, 0.5, 0.9, 1.0] {
            assert_eq!(left.quantile(p), direct.quantile(p), "quantile {p}");
        }
    }

    #[test]
    fn sealing_does_not_change_a_single_quantile_or_cdf_value() {
        // The regression this exists for: reading quantiles off a sealed
        // histogram used to fall through to the maximum, so every percentile of
        // every distribution in a report came out equal to the largest sample --
        // a wrong answer indistinguishable at a glance from a very skewed one.
        let mut h = Hist::new();
        for i in 0..10_000u64 {
            h.add((i * 7919) % 5_000);
        }
        let ps = [0.0, 0.01, 0.1, 0.25, 0.5, 0.75, 0.9, 0.99, 1.0];
        let before: Vec<_> = ps.iter().map(|p| h.quantile(*p)).collect();
        let cdf_before: Vec<_> = (0..40).map(|i| h.count_le(i * 137)).collect();
        let summary_before = format!("{:?}", h.summary());
        h.seal();
        let after: Vec<_> = ps.iter().map(|p| h.quantile(*p)).collect();
        let cdf_after: Vec<_> = (0..40).map(|i| h.count_le(i * 137)).collect();
        assert_eq!(before, after, "quantiles moved on sealing");
        assert_eq!(cdf_before, cdf_after, "the CDF moved on sealing");
        assert_eq!(summary_before, format!("{:?}", h.summary()));
        // And the quantiles are genuinely spread, not all pinned to the maximum.
        assert!(
            after[4] < after[8],
            "p50 {:?} vs max {:?}",
            after[4],
            after[8]
        );
    }

    #[test]
    fn an_empty_histogram_reports_absence_rather_than_zero() {
        // FR-012's failure mode in miniature: a 0 standing in for "no data" is a
        // realised value that is wrong.
        let h = Hist::new();
        assert_eq!(h.count(), 0);
        assert_eq!(h.mean(), None);
        assert_eq!(h.min(), None);
        assert_eq!(h.max(), None);
        assert_eq!(h.quantile(0.5), None);
    }
}
