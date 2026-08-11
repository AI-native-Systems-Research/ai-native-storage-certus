//! Unique keys over time (spec FR-034a).
//!
//! Cumulative distinct keys as a function of requests consumed. Read as a curve
//! it answers a question no single number does: whether the workload's key space
//! is *saturating* or still growing when the run ends.
//!
//! That distinction decides how a measurement should be read. A curve still
//! climbing steeply at the end of the run says the consumer was being asked to
//! hold a set that had not finished arriving. A curve that has flattened says the
//! working set is bounded and the run saw all of it.
//!
//! Neither is an error, and this module raises no warning. In this corpus model
//! distinct keys grow linearly *for ever* — every session's private path is novel
//! by construction (FR-009c) — so "still growing" is the normal case and a
//! threshold on it would fire on every realistic workload. What the report
//! publishes instead is the tail novelty rate on the same scale as the
//! compulsory-miss floor, for a reader to compare the two.
//!
//! # Log-spaced samples
//!
//! Sampled at request ordinals spaced geometrically rather than uniformly, for two
//! reasons. The interesting structure is early — a saturation knee is a feature of
//! the first decade, not the last — and geometric spacing needs no advance
//! knowledge of the run's length, which an unbounded run (FR-021f) cannot supply.
//! The series is `O(log n)` entries however long the run.

use serde::{Deserialize, Serialize};

use super::KeyFacts;

/// Samples per octave of request ordinal.
const PER_OCTAVE: u32 = 4;

/// Accumulates the unique-keys-over-time curve.
#[derive(Debug, Default)]
pub struct UniqueKeys {
    requests: u64,
    references: u64,
    distinct: u64,
    distinct_bytes: u128,
    points: Vec<UniquePoint>,
    next_sample: u64,
}

impl UniqueKeys {
    /// An empty accumulator.
    pub fn new() -> UniqueKeys {
        UniqueKeys {
            next_sample: 1,
            ..UniqueKeys::default()
        }
    }

    /// Record one measured reference.
    ///
    /// Counts a key as new on its first reference **in the measured window**: a
    /// key warmup already fetched is not new to the consumer, but it is new to
    /// this curve, and the curve is about what the measured window touched.
    pub fn observe(&mut self, facts: &KeyFacts, size: u32, request_start: bool) {
        if request_start {
            self.requests += 1;
        }
        self.references += 1;
        if facts.first_steady_touch {
            self.distinct += 1;
            self.distinct_bytes += u128::from(size);
        }
        if self.requests >= self.next_sample {
            self.sample();
            self.next_sample = next_geometric(self.next_sample);
        }
    }

    /// Take a final sample, so the curve always ends where the run ended.
    pub fn end(&mut self) {
        if self.points.last().map(|p| p.requests) != Some(self.requests) && self.references > 0 {
            self.sample();
        }
    }

    fn sample(&mut self) {
        self.points.push(UniquePoint {
            requests: self.requests,
            references: self.references,
            distinct_keys: self.distinct,
            distinct_bytes: self.distinct_bytes,
        });
    }

    /// Freeze into the serialisable form.
    pub fn finish(mut self) -> UniqueKeysReport {
        self.end();
        // Novelty over the run's second half: new keys per *reference*, so it is
        // on the same scale as the compulsory-miss floor and can be read against
        // it. Per request it would not be — a request is tens or hundreds of
        // blocks long, so the same workload would score 0.2 or 20 depending only
        // on its request length.
        let tail = match self.points.len() {
            0 | 1 => None,
            n => {
                let last = &self.points[n - 1];
                let prior = &self.points[n / 2];
                let dr = last.references.saturating_sub(prior.references);
                if dr == 0 {
                    None
                } else {
                    Some((last.distinct_keys - prior.distinct_keys) as f64 / dr as f64)
                }
            }
        };
        UniqueKeysReport {
            distinct_keys: self.distinct,
            distinct_bytes: self.distinct_bytes,
            tail_novelty_per_reference: tail,
            points: self.points,
        }
    }
}

/// The next ordinal to sample at: geometric, but always advancing.
fn next_geometric(n: u64) -> u64 {
    let step = (n / u64::from(PER_OCTAVE)).max(1);
    n + step
}

/// One point on the unique-keys curve.
#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub struct UniquePoint {
    /// Requests consumed.
    pub requests: u64,
    /// References consumed.
    pub references: u64,
    /// Distinct keys touched so far in the measured window.
    pub distinct_keys: u64,
    /// Summed entry size over those distinct keys.
    pub distinct_bytes: u128,
}

/// Unique keys over time.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UniqueKeysReport {
    /// Distinct keys in the measured window.
    pub distinct_keys: u64,
    /// Summed entry size over them — what holding the whole run would take.
    pub distinct_bytes: u128,
    /// Distinct keys gained per **reference** over the run's second half.
    ///
    /// On the same scale as the compulsory-miss floor, and informative read
    /// against it: at the floor, the run's tail is discovering keys at the same
    /// rate as the run as a whole, which is what a steady state looks like in a
    /// model where every session mints private keys. Well *above* the floor means
    /// the key space was still opening up when the run ended. Near zero means the
    /// run saw a closed key space.
    ///
    /// Not a warning. Linear growth in distinct keys is the *expected* behaviour
    /// here — a session's private path is novel by construction (FR-009c) — so a
    /// threshold on this would fire on every realistic workload.
    pub tail_novelty_per_reference: Option<f64>,
    /// The curve, at geometrically spaced request ordinals.
    pub points: Vec<UniquePoint>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::keys::SessionId;

    fn facts(first_steady: bool) -> KeyFacts {
        KeyFacts {
            pos: 1,
            prev_pos: if first_steady { None } else { Some(1) },
            entry_size: 100,
            first_touch: first_steady,
            first_steady_touch: first_steady,
            first_session: SessionId(0),
            newly_shared: false,
            shared: false,
        }
    }

    /// One reference per request; `new[i]` says whether request i was novel.
    fn feed(new: &[bool]) -> UniqueKeysReport {
        let mut u = UniqueKeys::new();
        for n in new {
            u.observe(&facts(*n), 100, true);
        }
        u.finish()
    }

    #[test]
    fn distinct_keys_and_bytes_accumulate_over_the_measured_window() {
        let r = feed(&[true, false, true, false]);
        assert_eq!(r.distinct_keys, 2);
        assert_eq!(r.distinct_bytes, 200);
    }

    #[test]
    fn the_curve_always_ends_at_the_last_request() {
        // Otherwise the final figure would be whatever the last geometric sample
        // happened to catch, which for a short run can be well short of the end.
        let r = feed(&[true; 37]);
        assert_eq!(r.points.last().unwrap().requests, 37);
        assert_eq!(r.points.last().unwrap().distinct_keys, 37);
    }

    #[test]
    fn the_series_stays_logarithmic_in_the_run_length() {
        let short = feed(&[true; 100]);
        let long = feed(&vec![true; 100_000]);
        assert!(
            long.points.len() < 4 * short.points.len(),
            "not logarithmic"
        );
        assert!(
            long.points.len() < 200,
            "{} points is too many",
            long.points.len()
        );
    }

    #[test]
    fn samples_are_geometrically_spaced_and_strictly_advancing() {
        for n in 1..10_000u64 {
            assert!(next_geometric(n) > n, "sampling must advance at {n}");
        }
        // Four samples per doubling, give or take the integer step.
        let mut n = 1u64;
        let mut count = 0;
        while n < 2048 {
            n = next_geometric(n);
            count += 1;
        }
        let octaves = 11.0f64;
        assert!(
            (count as f64 / octaves - f64::from(PER_OCTAVE)).abs() < 1.5,
            "{count} samples over {octaves} octaves"
        );
    }

    #[test]
    fn a_saturated_key_space_has_a_tail_novelty_of_zero() {
        let mut new = vec![true; 50];
        new.extend(std::iter::repeat(false).take(5_000));
        let r = feed(&new);
        assert_eq!(r.tail_novelty_per_reference, Some(0.0));
    }

    #[test]
    fn a_run_still_discovering_keys_has_a_tail_novelty_near_one() {
        // The diagnostic case: every request novel to the end, so no capacity was
        // ever holding a settled set and no hit rate over it is steady state.
        let r = feed(&vec![true; 5_000]);
        let slope = r.tail_novelty_per_reference.unwrap();
        assert!(slope > 0.99, "slope {slope} should be ~1");
    }

    #[test]
    fn nothing_measured_yields_an_empty_curve_rather_than_a_point_at_zero() {
        let r = feed(&[]);
        assert!(r.points.is_empty());
        assert_eq!(r.distinct_keys, 0);
        assert_eq!(r.tail_novelty_per_reference, None);
    }
}
