//! Fan-in per block: how many **distinct sessions** reference each key.
//!
//! # Why this is measured before it is gated on
//!
//! The FR-056 gate compares four marginals, and a hinted KV cache responds to none of them
//! directly. Given a hint per block, the most valuable input to an eviction decision is *how many
//! other sessions will touch this block* — a block twenty sessions share is worth holding at a
//! reuse distance that would evict a private one. Nothing in the model fits fan-in today, and it is
//! first on the list of statistics such a cache actually reacts to.
//!
//! It is nevertheless **not** gated on yet, deliberately. FR-057c established the rule: a statistic
//! earns a place in the gate by having a **low achievable floor** and a **high sibling bound**
//! measured over several pairs, not by sounding relevant. `request_length` sounds like a direct
//! measure of workload shape and cannot separate the corpus's two closest workloads. So fan-in is
//! measured first, on real traces, and gated only if the numbers say it discriminates.
//!
//! # The contiguity precondition, which is checked rather than trusted
//!
//! Counting *distinct* sessions per key exactly would need a set per key. Instead each key keeps a
//! count and the last session that touched it, which is exact **only if every session's references
//! arrive contiguously** — otherwise `A, B, A` counts three. That is the same constraint
//! [`crate::fit::segments::Census`] documents, and for the same reason: `fit` holds all invocations
//! and can group them by session, while a streaming `Statistics::push` sees them interleaved.
//!
//! Rather than trust the caller, [`FanIn`] detects the violation: a session that appears after it
//! was already left behind is counted in [`FanInReport::out_of_order_sessions`], so a caller that
//! fed an interleaved stream gets a number saying so instead of a plausible histogram. Silent
//! overcounting would bias fan-in **upward**, which is the direction that would make a workload look
//! more shareable than it is.

use crate::keys::{CacheKey, SessionId};
use crate::stats::hist::Hist;
use crate::stats::FastMap;

/// Accumulates distinct-session counts per key.
///
/// Feed every reference in **session-contiguous** order; see the module docs.
#[derive(Debug, Default)]
pub struct FanIn {
    /// Per key: distinct sessions so far, and the last session seen.
    per_key: FastMap<CacheKey, (u32, u32)>,
    /// The session currently being fed.
    current: Option<u32>,
    /// Sessions already left behind, to detect a non-contiguous feed.
    finished: std::collections::HashSet<u32>,
    /// Times a finished session reappeared.
    out_of_order: u64,
    /// References observed, for the reference-weighted view.
    references: u64,
}

impl FanIn {
    /// An empty accumulator.
    pub fn new() -> FanIn {
        FanIn::default()
    }

    /// Observe one reference.
    pub fn observe(&mut self, key: CacheKey, session: SessionId) {
        let s = session.0;
        if self.current != Some(s) {
            if let Some(prev) = self.current.replace(s) {
                self.finished.insert(prev);
            }
            // Not an error the caller must handle: the count is reported, because a fan-in built
            // from an interleaved stream is still worth seeing as long as nobody mistakes it for
            // an exact one.
            if self.finished.contains(&s) {
                self.out_of_order += 1;
            }
        }
        self.references += 1;
        let e = self.per_key.entry(key).or_insert((0, u32::MAX));
        if e.1 != s {
            e.0 += 1;
            e.1 = s;
        }
    }

    /// Summarise.
    pub fn finish(&self) -> FanInReport {
        let mut per_key = Hist::new();
        let mut weighted = Hist::new();
        let mut shared_keys = 0u64;
        let mut fan_in_sum = 0u64;
        for (count, _) in self.per_key.values() {
            let c = u64::from(*count);
            per_key.add(c);
            // Reference-weighted, using the distinct-session count as the weight rather than the
            // reference count: what a *sharing* session sees. A key's own session re-walks its
            // whole path every turn, so weighting by raw references would measure conversation
            // length, which is the trap already recorded for realised sharing.
            for _ in 0..c.min(1024) {
                weighted.add(c);
            }
            fan_in_sum += c;
            if c >= 2 {
                shared_keys += 1;
            }
        }
        FanInReport {
            keys: self.per_key.len() as u64,
            references: self.references,
            shared_keys,
            fan_in_sum,
            out_of_order_sessions: self.out_of_order,
            per_key,
            weighted,
        }
    }
}

/// What [`FanIn`] measured.
#[derive(Debug, Clone)]
pub struct FanInReport {
    /// Distinct keys seen.
    pub keys: u64,
    /// References observed.
    pub references: u64,
    /// Keys reached by two or more sessions — the shared set.
    pub shared_keys: u64,
    /// Summed fan-in over keys, i.e. distinct `(key, session)` pairs.
    pub fan_in_sum: u64,
    /// Times a session reappeared after being left behind; see the module docs.
    ///
    /// Non-zero means the feed was not session-contiguous and every figure here is an
    /// **upper** bound rather than a measurement.
    pub out_of_order_sessions: u64,
    /// Distribution of fan-in over **keys** — the population view.
    pub per_key: Hist,
    /// Distribution of fan-in weighted by the sessions sharing each key.
    ///
    /// Capped at 1024 increments per key so one universally-shared preamble block cannot dominate
    /// the accumulator's cost; the cap is above every fan-in that matters for a cache decision and
    /// is stated here because it makes this a lower bound in the extreme tail.
    pub weighted: Hist,
}

impl FanInReport {
    /// Mean distinct sessions per key.
    pub fn mean(&self) -> Option<f64> {
        if self.keys == 0 {
            return None;
        }
        Some(self.fan_in_sum as f64 / self.keys as f64)
    }

    /// Fraction of keys any second session ever touches.
    pub fn shared_fraction(&self) -> f64 {
        if self.keys == 0 {
            return 0.0;
        }
        self.shared_keys as f64 / self.keys as f64
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fan_in_counts_distinct_sessions_not_references() {
        // A session re-walks its own prefix every turn, so a key touched five times by one
        // session has fan-in 1. Getting this wrong would turn fan-in into conversation length —
        // the exact trap already recorded for realised sharing depth.
        let mut f = FanIn::new();
        for _ in 0..5 {
            f.observe(CacheKey(1), SessionId(0));
        }
        f.observe(CacheKey(1), SessionId(1));
        let r = f.finish();
        assert_eq!(r.references, 6);
        assert_eq!(r.keys, 1);
        assert_eq!(r.fan_in_sum, 2, "two distinct sessions, six references");
        assert_eq!(r.shared_keys, 1);
        assert_eq!(r.out_of_order_sessions, 0);
    }

    #[test]
    fn a_private_key_and_a_shared_key_are_distinguished() {
        let mut f = FanIn::new();
        f.observe(CacheKey(1), SessionId(0)); // shared trunk block
        f.observe(CacheKey(9), SessionId(0)); // private to session 0
        f.observe(CacheKey(1), SessionId(1));
        f.observe(CacheKey(8), SessionId(1)); // private to session 1
        let r = f.finish();
        assert_eq!(r.keys, 3);
        assert_eq!(r.shared_keys, 1);
        assert!((r.shared_fraction() - 1.0 / 3.0).abs() < 1e-12);
        assert!((r.mean().unwrap() - 4.0 / 3.0).abs() < 1e-12);
    }

    #[test]
    fn an_interleaved_feed_is_reported_rather_than_silently_overcounted() {
        // The precondition is checked because breaking it biases fan-in UPWARD — a workload would
        // look more shareable than it is, which is the direction that flatters the model.
        let mut f = FanIn::new();
        f.observe(CacheKey(1), SessionId(0));
        f.observe(CacheKey(1), SessionId(1));
        f.observe(CacheKey(1), SessionId(0)); // session 0 came back
        let r = f.finish();
        assert_eq!(r.out_of_order_sessions, 1, "the violation must be visible");
        assert_eq!(
            r.fan_in_sum, 3,
            "and the count really is inflated: 3 against a true 2, which is why it is reported"
        );
    }
}
