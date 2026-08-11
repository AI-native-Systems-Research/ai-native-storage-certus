//! The compulsory-miss floor (spec FR-044, FR-060).
//!
//! The fraction of measured references that ask for a block the stream has never
//! asked for before. No capacity appears anywhere in that sentence, which is the
//! point: it is the miss rate a consumer with *unbounded* capacity would still
//! report, so it is the floor under every policy and every cache size, and it
//! needs neither to be measured.
//!
//! Its use is negative. When a consumer's measured hit rate sits within noise of
//! this floor, the working set is too large for the capacity under test and every
//! policy looks alike — so the run says nothing about the policy, and FR-060
//! requires the tool to say so rather than let the number be read as a result.
//!
//! # Warmup, and why the floor is not simply "distinct / total"
//!
//! Over a whole stream the floor *is* distinct keys over references. Over the
//! **measured** window it is not, because a key that warmup fetched is not a
//! compulsory miss — warming is precisely the act of paying that cost outside the
//! measured window. So the numerator counts measured references that are a first
//! touch *of the whole stream*, not first touches of the measured window. The two
//! differ by exactly the set of keys warmup reached, and reporting the second as
//! if it were the first would credit warmup with nothing.
//!
//! # Churn needs no term here
//!
//! FR-016d requires the floor to account for churn-induced misses. It does so
//! without any churn-specific arithmetic: a rotation mints a *new key*
//! (`keys::trunk_child` folds the generation into the hash), so the first
//! reference to a rotated node is a first touch like any other and already sits
//! in the numerator. That is a consequence of identity being a hash of the path,
//! and it is why rotation cost is computable from the plan alone.

use serde::{Deserialize, Serialize};

use super::{KeyFacts, Ref};

/// Accumulates the compulsory-miss floor.
#[derive(Debug, Default)]
pub struct Floor {
    references: u64,
    compulsory: u64,
    bytes: u128,
    compulsory_bytes: u128,
    warmup_references: u64,
    warmup_bytes: u128,
}

impl Floor {
    /// An empty accumulator.
    pub fn new() -> Floor {
        Floor::default()
    }

    /// Record one reference.
    pub fn observe(&mut self, r: &Ref, facts: &KeyFacts) {
        if r.warmup {
            self.warmup_references += 1;
            self.warmup_bytes += u128::from(facts.entry_size);
            return;
        }
        self.references += 1;
        self.bytes += u128::from(facts.entry_size);
        if facts.first_touch {
            self.compulsory += 1;
            self.compulsory_bytes += u128::from(facts.entry_size);
        }
    }

    /// Freeze into the serialisable form.
    pub fn finish(self) -> FloorReport {
        let per_object = if self.references == 0 {
            None
        } else {
            Some(self.compulsory as f64 / self.references as f64)
        };
        let per_byte = if self.bytes == 0 {
            None
        } else {
            Some(self.compulsory_bytes as f64 / self.bytes as f64)
        };
        FloorReport {
            references: self.references,
            compulsory_misses: self.compulsory,
            per_object,
            bytes: self.bytes,
            compulsory_bytes: self.compulsory_bytes,
            per_byte,
            warmup_references: self.warmup_references,
            warmup_bytes: self.warmup_bytes,
        }
    }
}

/// The compulsory-miss floor, by object and by byte.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FloorReport {
    /// Measured references.
    pub references: u64,
    /// Measured references to a block the stream had never referenced.
    pub compulsory_misses: u64,
    /// The floor as a fraction of references. `None` when nothing was measured —
    /// absent rather than a 0 that would read as "no compulsory misses".
    pub per_object: Option<f64>,
    /// Measured bytes referenced.
    pub bytes: u128,
    /// Bytes attributable to compulsory misses.
    pub compulsory_bytes: u128,
    /// The floor as a fraction of bytes.
    pub per_byte: Option<f64>,
    /// Warmup references, excluded from the floor and reported separately
    /// (FR-045).
    pub warmup_references: u64,
    /// Warmup bytes.
    pub warmup_bytes: u128,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::keys::{CacheKey, SessionId};
    use crate::stats::reuse_distance::ReuseDistance;
    use crate::stats::KeyTable;

    fn feed(stream: &[(u64, u32, bool)]) -> FloorReport {
        let mut t = KeyTable::new();
        let mut f = Floor::new();
        for (k, size, warmup) in stream {
            let r = Ref {
                key: CacheKey(*k),
                size: *size,
                depth: 0,
                session: SessionId(0),
                request_start: true,
                warmup: *warmup,
            };
            let facts = t.observe(&r);
            f.observe(&r, &facts);
        }
        f.finish()
    }

    #[test]
    fn the_floor_is_distinct_keys_over_references_when_nothing_is_warmed() {
        let r = feed(&[
            (1, 10, false),
            (2, 10, false),
            (1, 10, false),
            (3, 10, false),
        ]);
        assert_eq!(r.compulsory_misses, 3);
        assert_eq!(r.references, 4);
        assert_eq!(r.per_object, Some(0.75));
    }

    #[test]
    fn a_warmed_key_is_not_a_compulsory_miss() {
        // Warming is the act of paying the compulsory cost outside the window.
        let r = feed(&[(1, 10, true), (1, 10, false), (2, 10, false)]);
        assert_eq!(r.warmup_references, 1);
        assert_eq!(r.references, 2);
        assert_eq!(r.compulsory_misses, 1, "only key 2 was never fetched");
        assert_eq!(r.per_object, Some(0.5));
    }

    #[test]
    fn the_byte_floor_weights_by_entry_size_rather_than_by_reference() {
        // One big first touch and many small repeats: the object floor is low and
        // the byte floor is high, and conflating them would misstate both.
        let mut s = vec![(1u64, 1_000_000u32, false)];
        for _ in 0..9 {
            s.push((2, 1_000, false));
        }
        let r = feed(&s);
        assert_eq!(r.per_object, Some(0.2), "keys 1 and 2 of ten references");
        let expected = (1_000_000.0 + 1_000.0) / (1_000_000.0 + 9_000.0);
        assert!((r.per_byte.unwrap() - expected).abs() < 1e-12);
    }

    #[test]
    fn an_empty_measured_window_reports_absence_rather_than_a_zero_floor() {
        let r = feed(&[(1, 10, true), (2, 10, true)]);
        assert_eq!(r.references, 0);
        assert_eq!(r.per_object, None);
        assert_eq!(r.per_byte, None);
        assert_eq!(r.warmup_references, 2);
    }

    #[test]
    fn the_floor_equals_the_miss_rate_at_unbounded_capacity() {
        // FR-034a's claim, checked against the reuse-distance CDF rather than
        // asserted: at unbounded capacity every reference with a finite reuse
        // distance hits, so the miss rate is exactly the first-touch fraction.
        // Two independent accumulators, one arithmetic identity.
        let stream: Vec<(u64, u32, bool)> = (0..500u64)
            .map(|i| ((i * 31) % 77, 64 + (i as u32 % 3) * 8, i < 40))
            .collect();

        let mut t = KeyTable::new();
        let mut floor = Floor::new();
        let mut reuse = ReuseDistance::new();
        for (k, size, warmup) in &stream {
            let r = Ref {
                key: CacheKey(*k),
                size: *size,
                depth: 0,
                session: SessionId(0),
                request_start: true,
                warmup: *warmup,
            };
            let facts = t.observe(&r);
            floor.observe(&r, &facts);
            reuse.observe(&r, &facts);
        }
        let floor = floor.finish().per_object.unwrap();
        let unbounded_miss = 1.0 - reuse.fraction_within_objects(u64::MAX / 2);
        assert!(
            (floor - unbounded_miss).abs() < 1e-12,
            "floor {floor} != unbounded miss rate {unbounded_miss}"
        );
    }
}
