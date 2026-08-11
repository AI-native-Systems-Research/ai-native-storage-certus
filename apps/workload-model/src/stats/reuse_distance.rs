//! The reuse-distance CDF — the primary statistic (spec FR-034a).
//!
//! The reuse distance of a reference is the number of **distinct** other blocks
//! referenced since that block was last referenced. It is a property of the
//! stream alone, and it is the statistic from which a consumer reads off what
//! any capacity would buy it, which is why this crate needs no cache model and
//! publishes no hit-rate figure of its own (FR-034).
//!
//! Two distance metrics, because a consumer's capacity comes in two units:
//!
//! - **objects** — distinct blocks in between, for a capacity counted in entries.
//! - **bytes** — the summed size of those distinct blocks, for a capacity counted
//!   in bytes. Not derivable from the object distance unless every entry is the
//!   same size, which is exactly the case the byte tree is skipped for.
//!
//! First touches have no finite distance. They are counted, never bucketed, and
//! they are the numerator of the compulsory-miss floor (see [`super::floor`]).
//!
//! # Exact, not sampled
//!
//! Distances are computed exactly with a Fenwick tree over stream positions: a
//! block's most recent reference carries a marker, so the distinct count between
//! two positions is a range sum. `O(log n)` per reference, `O(n)` memory, and no
//! estimation error to reason about later. `research.md` still owes a derivation
//! of an *estimation* method (task T076) for traces too large to hold; nothing
//! here depends on that landing, and an exact implementation is the right thing
//! to validate an estimator against when it arrives.

use serde::{Deserialize, Serialize};

use super::hist::{Hist, Quantiles};
use super::{KeyFacts, Ref};

/// A Fenwick tree over 1-based stream positions.
///
/// Grows by doubling. The rebuild on growth is `O(n)` and reads its point values
/// back out of the tree in place, so the tree is the only copy of the data —
/// which matters when it is 40 MB.
#[derive(Debug)]
struct Fenwick<T> {
    /// 1-based; `t[0]` is unused.
    t: Vec<T>,
}

impl<T> Fenwick<T>
where
    T: Copy + Default + std::ops::AddAssign + std::ops::SubAssign,
{
    fn new(capacity: usize) -> Fenwick<T> {
        Fenwick {
            t: vec![T::default(); capacity + 1],
        }
    }

    fn capacity(&self) -> usize {
        self.t.len() - 1
    }

    /// Turn a Fenwick tree into a plain array of point values, in place.
    fn to_points(t: &mut [T]) {
        let n = t.len() - 1;
        for i in (1..=n).rev() {
            let j = i + (i & i.wrapping_neg());
            if j <= n {
                let v = t[i];
                t[j] -= v;
            }
        }
    }

    /// Turn a plain array of point values into a Fenwick tree, in place.
    fn to_tree(t: &mut [T]) {
        let n = t.len() - 1;
        for i in 1..=n {
            let j = i + (i & i.wrapping_neg());
            if j <= n {
                let v = t[i];
                t[j] += v;
            }
        }
    }

    /// Make room for position `n`, doubling to amortise the rebuild.
    fn reserve(&mut self, n: usize) {
        if n <= self.capacity() {
            return;
        }
        let mut want = (self.capacity() * 2).max(1024);
        while want < n {
            want *= 2;
        }
        Self::to_points(&mut self.t);
        self.t.resize(want + 1, T::default());
        Self::to_tree(&mut self.t);
    }

    fn add(&mut self, mut i: usize, v: T) {
        let n = self.capacity();
        while i <= n {
            self.t[i] += v;
            i += i & i.wrapping_neg();
        }
    }

    fn sub(&mut self, mut i: usize, v: T) {
        let n = self.capacity();
        while i <= n {
            self.t[i] -= v;
            i += i & i.wrapping_neg();
        }
    }

    /// Sum over positions `1..=i`.
    fn prefix(&self, mut i: usize) -> T {
        let mut acc = T::default();
        while i > 0 {
            acc += self.t[i];
            i -= i & i.wrapping_neg();
        }
        acc
    }

    /// Point values, as a fresh array. Used only when seeding the byte tree.
    fn points(&self) -> Vec<T> {
        let mut copy = self.t.clone();
        Self::to_points(&mut copy);
        copy
    }
}

/// Accumulates the reuse-distance CDF over a reference stream.
#[derive(Debug)]
pub struct ReuseDistance {
    /// One marker per live block, at its most recent position.
    objects: Fenwick<u32>,
    /// Entry size at each live block's most recent position.
    ///
    /// `None` while every entry seen has been the same size, in which case the
    /// byte distance is exactly `objects × that size` and a second 8-byte-per-
    /// position tree would be 80 MB of redundancy on a 10^7-event plan. Built,
    /// exactly and retrospectively, the moment a second size appears.
    bytes: Option<Fenwick<u64>>,
    /// The single size seen so far, while there is one.
    uniform_size: Option<u32>,
    objects_hist: Hist,
    bytes_hist: Hist,
    references: u64,
    first_touches: u64,
    warmup_references: u64,
}

impl Default for ReuseDistance {
    fn default() -> Self {
        ReuseDistance::new()
    }
}

impl ReuseDistance {
    /// An empty accumulator.
    pub fn new() -> ReuseDistance {
        ReuseDistance {
            objects: Fenwick::new(1024),
            bytes: None,
            uniform_size: None,
            objects_hist: Hist::new(),
            bytes_hist: Hist::new(),
            references: 0,
            first_touches: 0,
            warmup_references: 0,
        }
    }

    /// Record one reference.
    ///
    /// `facts` must come from the [`super::KeyTable`] this accumulator shares
    /// with the rest of the report, at the same position: the marker bookkeeping
    /// below is only correct if `facts.prev_pos` really is where this key's
    /// marker currently sits.
    pub fn observe(&mut self, r: &Ref, facts: &KeyFacts) {
        let i = facts.pos as usize;
        let size = facts.entry_size;

        // A second distinct size retires the uniform shortcut. Done before the
        // distances are read so this reference is already measured properly.
        match self.uniform_size {
            None if self.bytes.is_none() => self.uniform_size = Some(size),
            Some(u) if u != size => self.promote_byte_tree(u),
            _ => {}
        }

        self.objects.reserve(i);
        if let Some(b) = self.bytes.as_mut() {
            b.reserve(i);
        }

        match facts.prev_pos {
            Some(p) => {
                let p = p as usize;
                // Distinct blocks strictly between p and i: every live block's
                // marker in that span, and this block's own marker is at p,
                // which the subtraction excludes.
                let d_obj = u64::from(self.objects.prefix(i - 1) - self.objects.prefix(p));
                let d_bytes = match (&self.bytes, self.uniform_size) {
                    (Some(b), _) => b.prefix(i - 1) - b.prefix(p),
                    (None, Some(u)) => d_obj * u64::from(u),
                    (None, None) => 0,
                };
                self.objects.sub(p, 1);
                self.objects.add(i, 1);
                if let Some(b) = self.bytes.as_mut() {
                    b.sub(p, u64::from(size));
                    b.add(i, u64::from(size));
                }
                if r.warmup {
                    self.warmup_references += 1;
                } else {
                    self.references += 1;
                    self.objects_hist.add(d_obj);
                    self.bytes_hist.add(d_bytes);
                }
            }
            None => {
                self.objects.add(i, 1);
                if let Some(b) = self.bytes.as_mut() {
                    b.add(i, u64::from(size));
                }
                if r.warmup {
                    self.warmup_references += 1;
                } else {
                    self.references += 1;
                    self.first_touches += 1;
                }
            }
        }
    }

    /// Materialise the byte tree from the object tree, scaled by the size every
    /// entry has had until now. Exact, because they all had it.
    fn promote_byte_tree(&mut self, uniform: u32) {
        let points = self.objects.points();
        let mut wide: Vec<u64> = points
            .iter()
            .map(|v| u64::from(*v) * u64::from(uniform))
            .collect();
        Fenwick::<u64>::to_tree(&mut wide);
        self.bytes = Some(Fenwick { t: wide });
        self.uniform_size = None;
    }

    /// Measured references, warmup excluded.
    pub fn references(&self) -> u64 {
        self.references
    }

    /// Measured references that were a first touch — no finite reuse distance.
    pub fn first_touches(&self) -> u64 {
        self.first_touches
    }

    /// Fraction of measured references whose object reuse distance is `<= d`.
    ///
    /// The denominator is every measured reference, first touches included, so
    /// this really is the CDF of the reuse distance over the stream rather than
    /// over its reused subset. Exact when `d` is a bucket upper bound.
    pub fn fraction_within_objects(&self, d: u64) -> f64 {
        self.objects_hist.fraction_le(d, self.references)
    }

    /// Fraction of measured references whose byte reuse distance is `<= d`.
    pub fn fraction_within_bytes(&self, d: u64) -> f64 {
        self.bytes_hist.fraction_le(d, self.references)
    }

    /// Freeze into the serialisable form.
    pub fn finish(mut self) -> ReuseDistanceReport {
        self.objects_hist.seal();
        self.bytes_hist.seal();
        ReuseDistanceReport {
            references: self.references,
            first_touches: self.first_touches,
            warmup_references: self.warmup_references,
            objects: self.objects_hist.summary(),
            object_buckets: self.objects_hist.buckets(),
            bytes: self.bytes_hist.summary(),
            byte_buckets: self.bytes_hist.buckets(),
        }
    }
}

/// The reuse-distance CDF, in both metrics.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReuseDistanceReport {
    /// Measured references behind the CDF, first touches included.
    pub references: u64,
    /// Measured references with no finite distance.
    pub first_touches: u64,
    /// Warmup references, which primed the stream but supplied no samples.
    pub warmup_references: u64,
    /// Distance in distinct objects.
    pub objects: Quantiles,
    /// The object CDF as `(lower, upper, count)`, ascending.
    pub object_buckets: Vec<(u64, u64, u64)>,
    /// Distance in distinct bytes.
    pub bytes: Quantiles,
    /// The byte CDF as `(lower, upper, count)`, ascending.
    pub byte_buckets: Vec<(u64, u64, u64)>,
}

impl ReuseDistanceReport {
    /// Fraction of measured references with object distance `<= d`.
    ///
    /// Exact at a bucket's upper bound; elsewhere the bucket containing `d` is
    /// counted whole, matching [`Hist::count_le`](super::hist::Hist::count_le).
    /// A comparison between two of these should therefore be evaluated at the
    /// shared bucket bounds, where no interpolation is involved.
    pub fn fraction_within_objects(&self, d: u64) -> f64 {
        Self::fraction(&self.object_buckets, d, self.references)
    }

    /// Fraction of measured references with byte distance `<= d`.
    pub fn fraction_within_bytes(&self, d: u64) -> f64 {
        Self::fraction(&self.byte_buckets, d, self.references)
    }

    fn fraction(buckets: &[(u64, u64, u64)], d: u64, denominator: u64) -> f64 {
        if denominator == 0 {
            return 0.0;
        }
        let n: u64 = buckets
            .iter()
            .filter(|(lo, _, _)| *lo <= d)
            .map(|(_, _, c)| *c)
            .sum();
        n as f64 / denominator as f64
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::keys::{CacheKey, SessionId};
    use crate::stats::KeyTable;

    /// Feed a key sequence through a shared key table, as the report does.
    fn run(keys: &[u64], size: u32) -> ReuseDistance {
        run_sized(&keys.iter().map(|k| (*k, size)).collect::<Vec<_>>())
    }

    fn run_sized(keys: &[(u64, u32)]) -> ReuseDistance {
        let mut t = KeyTable::new();
        let mut rd = ReuseDistance::new();
        for (k, size) in keys {
            let r = Ref {
                key: CacheKey(*k),
                size: *size,
                depth: 0,
                session: SessionId(0),
                request_start: true,
                warmup: false,
            };
            let f = t.observe(&r);
            rd.observe(&r, &f);
        }
        rd
    }

    #[test]
    fn an_immediate_repeat_has_distance_zero() {
        let rd = run(&[1, 1], 64);
        assert_eq!(rd.references(), 2);
        assert_eq!(rd.first_touches(), 1);
        assert_eq!(rd.fraction_within_objects(0), 0.5, "one of two references");
    }

    #[test]
    fn distance_counts_distinct_blocks_not_references() {
        // A,B,B,B,A: only B intervenes, however many times it is referenced.
        let rd = run(&[1, 2, 2, 2, 1], 64);
        let r = rd.finish();
        assert_eq!(r.objects.max, Some(1), "B is one distinct block");
    }

    #[test]
    fn a_worked_sequence_matches_hand_computed_distances() {
        // A B C A C B  ->  A:2 (B,C)   C:1 (A)   B:2 (A,C)
        let rd = run(&[1, 2, 3, 1, 3, 2], 64);
        let r = rd.finish();
        assert_eq!(r.first_touches, 3);
        assert_eq!(r.objects.count, 3);
        let hand = [2u64, 1, 2];
        assert_eq!(r.objects.mean, Some(hand.iter().sum::<u64>() as f64 / 3.0));
        assert_eq!(r.objects.max, Some(2));
        assert_eq!(r.objects.min, Some(1));
    }

    #[test]
    fn the_cdf_is_over_every_reference_so_first_touches_sit_above_the_top() {
        // Six references, three of which never become hits at any capacity.
        let r = run(&[1, 2, 3, 1, 3, 2], 64).finish();
        assert_eq!(r.fraction_within_objects(2), 0.5);
        assert_eq!(r.fraction_within_objects(u64::MAX / 2), 0.5);
        assert_eq!(r.references, 6);
    }

    #[test]
    fn byte_distance_is_the_object_distance_scaled_when_sizes_are_uniform() {
        let rd = run(&[1, 2, 3, 1], 1000);
        let r = rd.finish();
        assert_eq!(r.objects.max, Some(2));
        assert_eq!(r.bytes.max, Some(2000));
    }

    #[test]
    fn a_second_size_promotes_the_byte_tree_without_losing_the_history() {
        // The retrospective build must be exact: distances that span the switch
        // are the reason the shortcut is safe to take at all.
        //  A(100) B(100) C(500) A -> objects 2, bytes 600
        let rd = run_sized(&[(1, 100), (2, 100), (3, 500), (1, 100)]);
        let r = rd.finish();
        assert_eq!(r.objects.max, Some(2));
        assert_eq!(r.bytes.max, Some(600));
    }

    #[test]
    fn byte_distance_agrees_with_a_direct_recomputation_over_mixed_sizes() {
        // Brute force over a longer mixed-size sequence, as a check on the two
        // trees staying in step through promotion, marker moves and growth.
        let seq: Vec<(u64, u32)> = (0..400u64)
            .map(|i| {
                let k = (i * 7) % 23;
                (k, 100 + (k as u32 % 5) * 300)
            })
            .collect();
        let r = run_sized(&seq).finish();

        let mut expect_obj = Vec::new();
        let mut expect_byt = Vec::new();
        for (i, (k, _)) in seq.iter().enumerate() {
            if let Some(p) = seq[..i].iter().rposition(|(pk, _)| pk == k) {
                let between: std::collections::BTreeMap<u64, u32> =
                    seq[p + 1..i].iter().map(|(k, s)| (*k, *s)).collect();
                expect_obj.push(between.len() as u64);
                expect_byt.push(between.values().map(|s| u64::from(*s)).sum::<u64>());
            }
        }
        assert_eq!(r.objects.count, expect_obj.len() as u64);
        assert_eq!(
            r.objects.mean,
            Some(expect_obj.iter().sum::<u64>() as f64 / expect_obj.len() as f64)
        );
        assert_eq!(r.bytes.max, expect_byt.iter().copied().max());
        assert_eq!(
            r.bytes.mean,
            Some(expect_byt.iter().sum::<u64>() as f64 / expect_byt.len() as f64)
        );
    }

    #[test]
    fn growth_past_the_initial_capacity_preserves_every_distance() {
        // The rebuild reads point values back out of the tree; if that were
        // wrong, distances would be silently wrong only for long streams.
        let n = 5_000u64;
        let keys: Vec<u64> = (0..n).map(|i| i % 100).collect();
        let r = run(&keys, 8).finish();
        assert_eq!(r.first_touches, 100);
        assert_eq!(r.objects.count, n - 100);
        // Every repeat is 100 references apart with 99 distinct blocks between.
        assert_eq!(r.objects.min, Some(99));
        assert_eq!(r.objects.max, Some(99));
    }

    #[test]
    fn warmup_references_occupy_distance_but_supply_no_samples() {
        // FR-045. The warmup fetch of B still sits between A's two references,
        // and A's second reference is not a first touch.
        let mut t = KeyTable::new();
        let mut rd = ReuseDistance::new();
        let mk = |k: u64, warmup: bool| Ref {
            key: CacheKey(k),
            size: 10,
            depth: 0,
            session: SessionId(0),
            request_start: true,
            warmup,
        };
        for (k, warm) in [(1, true), (2, true), (1, false)] {
            let r = mk(k, warm);
            let f = t.observe(&r);
            rd.observe(&r, &f);
        }
        let r = rd.finish();
        assert_eq!(r.references, 1, "only the measured reference is a sample");
        assert_eq!(r.warmup_references, 2);
        assert_eq!(r.first_touches, 0, "warmup had already fetched it");
        assert_eq!(r.objects.max, Some(1), "the warmup fetch of B intervened");
    }

    #[test]
    fn a_stream_of_all_distinct_keys_is_all_first_touches() {
        let r = run(&[1, 2, 3, 4, 5], 64).finish();
        assert_eq!(r.first_touches, 5);
        assert_eq!(r.objects.count, 0);
        assert_eq!(r.fraction_within_objects(u64::MAX / 2), 0.0);
    }

    #[test]
    fn the_sealed_report_answers_the_cdf_the_same_way_the_accumulator_does() {
        let rd = run(&[1, 2, 3, 1, 3, 2, 1], 64);
        let live: Vec<f64> = (0..8).map(|d| rd.fraction_within_objects(d)).collect();
        let sealed = rd.finish();
        let after: Vec<f64> = (0..8).map(|d| sealed.fraction_within_objects(d)).collect();
        assert_eq!(live, after);
    }
}
