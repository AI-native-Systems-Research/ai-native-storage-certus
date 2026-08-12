//! Fitting `workload.sessions` and `corpus.trees.shared_depth` (spec FR-055).
//!
//! Every measurement here is the one `contracts/workload-schema.md` § Fitting
//! specifies, and none of them is a second definition of something `stats` already
//! computes:
//!
//! | Parameter | Measurement |
//! | --- | --- |
//! | `turns` | invocations per session |
//! | `think_time` | `request_start` delta between consecutive turns of one session |
//! | `private_depth` | turn-1 path depth − that request's longest common prefix |
//! | `growth_per_turn` | path-depth increment between consecutive turns |
//! | `shared_depth` | the realised prefix-sharing histogram, from `stats::sharing` |
//!
//! The longest-common-prefix rule is **not** reimplemented: it is read from
//! [`Sharing::last_prefix_len`](crate::stats::sharing::Sharing::last_prefix_len), so
//! the `private_depth` this emits and the `shared_depth` a validator checks are on
//! one definition. Two of them would make FR-057's divergence a comparison of
//! definitions rather than of measurements.
//!
//! # What is emitted as `empirical`, and why that is not a cop-out
//!
//! `shared_depth` and `turns` come out as `empirical` CDF points rather than as a
//! fitted parametric shape. The contract says so directly — "`shared_depth` **is**
//! the FR-056 validation statistic, so the model is parameterised in the space it is
//! validated in, and `empirical` is the natural default rather than an escape hatch".
//! A lognormal fitted to a bimodal histogram would be a worse model that *looked*
//! more confident.

use serde::{Deserialize, Serialize};

use crate::dist::{Dist, Shape};
use crate::stats::hist::Hist;
use crate::stats::sharing::SharingReport;

/// Percentile points an `empirical` distribution is emitted at.
///
/// Enough to carry a bimodal shape — a `scan` arm beside a conversational one — and
/// few enough that the emitted YAML stays readable. The contract's own worked example
/// uses three points; these nine are a superset of its shape.
const EMPIRICAL_POINTS: [f64; 9] = [0.05, 0.10, 0.25, 0.50, 0.75, 0.90, 0.95, 0.99, 1.00];

/// Per-session state while fitting.
#[derive(Debug, Clone, Copy)]
struct Live {
    last_turn: u32,
    last_depth: u64,
    last_start: Option<f64>,
    turns: u64,
}

/// Accumulates the session-shape parameters.
#[derive(Debug, Default)]
pub struct SessionShapes {
    live: crate::stats::FastMap<u32, Live>,
    turns: Hist,
    /// One `(turn-1 path length, realised shared prefix)` pair per session.
    ///
    /// Retained rather than reduced to a histogram of their difference, because
    /// `private_depth` has to be recomputable against a *different* attempted shared
    /// depth than the realised one — see [`SessionShapes::private_depth_at`]. The two
    /// are correlated per session (a deeper path tends to share more), so subtracting
    /// one histogram from another would not give the same answer as subtracting per
    /// session, which is the answer the generator's path formula needs.
    ///
    /// Memory is one pair per session — 16 bytes against the tens of thousands of
    /// bytes each session's blocks already cost, so it does not change the bound.
    turn_one: Vec<(u64, u64)>,
    growth: Hist,
    /// Think times in milliseconds, so the histogram's integer buckets have useful
    /// resolution: a think time of 3 s is 3000 buckets rather than 3.
    think_ms: Hist,
    /// Requests whose turn index went backwards or repeated.
    ///
    /// Counted rather than corrected. Turn n+1's path must extend turn n's (FR-014a),
    /// so out-of-order turns mean the trace is not the strict chain the model
    /// assumes, and a fit from it is describing something else.
    out_of_order: u64,
    /// Turn-1 requests whose shared prefix exceeded their own path length.
    ///
    /// Impossible if the prefix is a prefix, so a non-zero count means the two
    /// measurements disagree and `private_depth` is not trustworthy.
    prefix_longer_than_path: u64,
}

impl SessionShapes {
    /// An empty accumulator.
    pub fn new() -> SessionShapes {
        SessionShapes::default()
    }

    /// Record one closed request.
    ///
    /// `shared_len` must be the request's longest common prefix as
    /// `stats::sharing` measured it, and `turn` its 0-based invocation index.
    pub fn observe(
        &mut self,
        session: u32,
        turn: u32,
        path_len: u64,
        shared_len: u64,
        request_start: Option<f64>,
    ) {
        match self.live.get_mut(&session) {
            None => {
                // Turn 1 in the model's terms, whatever the trace calls it: this is
                // the first request of this session that the read saw.
                if shared_len > path_len {
                    self.prefix_longer_than_path += 1;
                }
                self.turn_one.push((path_len, shared_len));
                self.live.insert(
                    session,
                    Live {
                        last_turn: turn,
                        last_depth: path_len,
                        last_start: request_start,
                        turns: 1,
                    },
                );
            }
            Some(live) => {
                if turn <= live.last_turn {
                    self.out_of_order += 1;
                }
                // Growth is the increment, which FR-014a makes non-negative: turn
                // n+1's path extends turn n's. A decrease means the chain is not
                // strict, which `out_of_order` and this floor both surface rather
                // than silently absorbing into a distribution.
                self.growth.add(path_len.saturating_sub(live.last_depth));
                if let (Some(prev), Some(now)) = (live.last_start, request_start) {
                    let gap = (now - prev).max(0.0);
                    self.think_ms.add((gap * 1000.0) as u64);
                }
                live.last_turn = turn;
                live.last_depth = path_len;
                live.last_start = request_start;
                live.turns += 1;
            }
        }
    }

    /// `private_depth` recomputed against an attempted shared depth `scale` times
    /// the realised one.
    ///
    /// The generator's path is `attempted_shared + private_depth + Σ growth`
    /// (FR-014a), while a fit measures `private_depth` as
    /// `turn-1 depth − *realised* shared prefix`. Those agree only when the attempted
    /// and realised sharing agree — and FR-012a says the drawn value is an *upper
    /// bound* on the realised one, so they generally do not. Feed a `shared_depth`
    /// fitted from realised sharing back in as an attempt and paths come out longer
    /// than the trace's by exactly the shortfall.
    ///
    /// So an iteration that raises the attempted sharing to make *realised* sharing
    /// match must lower `private_depth` by the same amount, per session, or it will
    /// fix the sharing statistic by breaking the request-length one. `scale` of 1.0
    /// reproduces the plain measurement.
    ///
    /// Clamped at zero: a session whose attempted sharing exceeds its own path has no
    /// private part, and a negative one is not expressible.
    pub fn private_depth_at(&self, scale: f64) -> Option<Dist> {
        let mut h = Hist::new();
        for (path_len, shared_len) in &self.turn_one {
            let attempted = (*shared_len as f64 * scale).round().max(0.0) as u64;
            h.add(path_len.saturating_sub(attempted));
        }
        empirical_from(&h)
    }

    /// Turn-1 path lengths, for a report that wants to show what was subtracted from.
    pub fn turn_one_depth(&self) -> Option<Dist> {
        let mut h = Hist::new();
        for (path_len, _) in &self.turn_one {
            h.add(*path_len);
        }
        empirical_from(&h)
    }

    /// Freeze into a fitted set of parameters.
    ///
    /// `sharing` supplies `shared_depth`, so that the emitted parameter and the
    /// statistic a validator recomputes are the same measurement. Borrows rather than
    /// consumes, so a caller iterating on the attempted sharing can keep calling
    /// [`SessionShapes::private_depth_at`] against the same measurements.
    pub fn finish(&mut self, sharing: &SharingReport) -> FittedSessions {
        if self.turns.count() == 0 {
            for live in self.live.values() {
                self.turns.add(live.turns);
            }
        }
        FittedSessions {
            sessions: self.live.len() as u64,
            turns: empirical_from(&self.turns),
            private_depth: self.private_depth_at(1.0),
            growth_per_turn: empirical_from(&self.growth),
            // Seconds, which is what `think_time` is in (`SessionParams::think_time_s`).
            think_time: empirical_from(&self.think_ms).map(|d| scale(&d, 1.0 / 1000.0)),
            shared_depth: empirical_from_buckets(&sharing.depth_buckets),
            unshared_requests: sharing.unshared_requests,
            out_of_order_turns: self.out_of_order,
            prefix_longer_than_path: self.prefix_longer_than_path,
        }
    }
}

/// A fitted `workload.sessions`, with the parameters that could not be measured
/// left as `None` rather than defaulted (FR-055).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FittedSessions {
    /// Sessions the fit saw.
    pub sessions: u64,
    /// Invocations per session.
    pub turns: Option<Dist>,
    /// Turn-1 depth beyond the shared prefix.
    pub private_depth: Option<Dist>,
    /// Depth increment between consecutive turns.
    pub growth_per_turn: Option<Dist>,
    /// Seconds between consecutive turns of one session; `None` without timestamps.
    pub think_time: Option<Dist>,
    /// The realised sharing histogram, as the schema's `shared_depth`.
    pub shared_depth: Option<Dist>,
    /// Requests that shared nothing at all.
    ///
    /// Not folded into `shared_depth`: "shares nothing" and "shares one block" are
    /// different workloads, and the emitted distribution's support starts at 1.
    pub unshared_requests: u64,
    /// Turns that arrived out of order, which FR-014a's strict chain forbids.
    pub out_of_order_turns: u64,
    /// Turn-1 requests whose prefix exceeded their path — impossible, so non-zero
    /// means the two measurements disagree.
    pub prefix_longer_than_path: u64,
}

impl FittedSessions {
    /// The caveats a fit report must carry about these parameters.
    pub fn caveats(&self) -> Vec<String> {
        let mut out = Vec::new();
        if self.think_time.is_none() {
            out.push(
                "think_time is unset: the trace carries no usable per-request timestamps, so \
                 the gap between turns is not measurable. Left unset rather than defaulted, \
                 since a default would be indistinguishable from a measurement (FR-055)"
                    .to_string(),
            );
        }
        if self.out_of_order_turns > 0 {
            out.push(format!(
                "{} turns arrived out of order. FR-014a makes turn n+1's path a strict \
                 extension of turn n's, so growth_per_turn fitted from this trace is \
                 describing something the model cannot express",
                self.out_of_order_turns
            ));
        }
        if self.prefix_longer_than_path > 0 {
            out.push(format!(
                "{} turn-1 requests had a shared prefix longer than their own path, which \
                 is impossible for a prefix: private_depth from this trace is not \
                 trustworthy",
                self.prefix_longer_than_path
            ));
        }
        if self.unshared_requests > 0 {
            out.push(format!(
                "{} requests shared nothing at all and are not in shared_depth's support, \
                 which starts at 1. A generated model will therefore give every session \
                 some sharing where this trace gave those requests none",
                self.unshared_requests
            ));
        }
        out
    }
}

/// An `empirical` distribution from a histogram, or `None` if it has no samples.
fn empirical_from(h: &Hist) -> Option<Dist> {
    empirical_from_buckets(&h.buckets())
}

/// An `empirical` distribution from a bucket list, as a **step** CDF.
///
/// `dist::empirical` interpolates its points linearly, which is right for a
/// continuous quantity and wrong for these: `turns`, `private_depth`,
/// `growth_per_turn` and `shared_depth` are all counts of blocks or invocations.
/// Emitted as bare `(value, cumulative)` points, a session population of 1, 4 and 4
/// turns comes back with a median of 2 — a value the trace never contained.
///
/// So each distinct value contributes **two** points, `(v, c_before)` and `(v, c_after)`,
/// which makes the interpolated CDF a staircase and reproduces the discrete
/// distribution exactly. The zero-width segment between them is what
/// `dist::empirical` already handles by taking the lower value.
fn empirical_from_buckets(buckets: &[(u64, u64, u64)]) -> Option<Dist> {
    let total: u64 = buckets.iter().map(|(_, _, c)| *c).sum();
    if total == 0 {
        return None;
    }
    // Values at the target quantiles, with the cumulative probability *at* each —
    // not the probe that found it, which would understate it.
    let mut steps: Vec<(f64, f64)> = Vec::new();
    let mut acc = 0u64;
    let mut next = 0usize;
    for (lo, _, c) in buckets {
        acc += c;
        let cumulative = acc as f64 / total as f64;
        let mut wanted = false;
        while next < EMPIRICAL_POINTS.len() && cumulative >= EMPIRICAL_POINTS[next] {
            next += 1;
            wanted = true;
        }
        if wanted {
            steps.push((*lo as f64, cumulative));
        }
    }
    if steps.is_empty() {
        return None;
    }
    // The top of the support must reach 1.0, or a draw above the last point would
    // clamp to it and the tail would be lost.
    if let Some(last) = steps.last_mut() {
        last.1 = 1.0;
    }

    let mut points: Vec<(f64, f64)> = Vec::new();
    let mut prev_c = 0.0f64;
    for (v, c) in steps {
        if prev_c > 0.0 {
            points.push((v, prev_c));
        }
        points.push((v, c));
        prev_c = c;
    }
    Some(Dist::Shaped(Shape::Empirical { points }))
}

/// Scale an empirical distribution's values.
///
/// Used for a unit conversion (milliseconds to seconds) and by the iteration that
/// raises an attempted `shared_depth` above the realised one. Values only: the
/// cumulative probabilities are the shape and must not move.
pub fn scale_values(d: &Dist, factor: f64) -> Dist {
    scale(d, factor)
}

/// Scale an empirical distribution's values, for a unit conversion.
fn scale(d: &Dist, factor: f64) -> Dist {
    match d {
        Dist::Shaped(Shape::Empirical { points }) => Dist::Shaped(Shape::Empirical {
            points: points.iter().map(|(v, p)| (v * factor, *p)).collect(),
        }),
        other => other.clone(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn quantile_of(d: &Option<Dist>, q: f64) -> f64 {
        d.as_ref().and_then(|d| d.quantile(q)).expect("a quantile")
    }

    /// A sharing report over the given realised depths.
    fn sharing_with(depths: &[u64]) -> SharingReport {
        let mut h = Hist::new();
        for d in depths {
            h.add(*d);
        }
        h.seal();
        SharingReport {
            requests: depths.len() as u64,
            sharing_requests: h.count(),
            unshared_requests: 0,
            shared_fraction: Some(1.0),
            realised_depth: h.summary(),
            depth_buckets: h.buckets(),
        }
    }

    fn empty_sharing() -> SharingReport {
        SharingReport {
            requests: 0,
            sharing_requests: 0,
            unshared_requests: 0,
            shared_fraction: None,
            realised_depth: Hist::new().summary(),
            depth_buckets: vec![],
        }
    }

    #[test]
    fn turns_are_invocations_per_session() {
        let mut s = SessionShapes::new();
        // Three sessions with 1, 4 and 4 turns.
        for (session, turns) in [(1u32, 1u32), (2, 4), (3, 4)] {
            for t in 0..turns {
                s.observe(session, t, 10 + u64::from(t), 0, None);
            }
        }
        let f = s.finish(&empty_sharing());
        assert_eq!(f.sessions, 3);
        // The median session has four turns.
        assert_eq!(quantile_of(&f.turns, 0.5), 4.0);
    }

    #[test]
    fn private_depth_is_turn_one_depth_less_its_shared_prefix() {
        // The contract's definition, and the reason `shared_len` is passed in rather
        // than recomputed.
        let mut s = SessionShapes::new();
        for session in 0..50u32 {
            s.observe(session, 0, 30, 18, None);
        }
        let f = s.finish(&empty_sharing());
        assert_eq!(quantile_of(&f.private_depth, 0.5), 12.0);
    }

    #[test]
    fn growth_is_the_increment_between_turns_not_the_depth() {
        let mut s = SessionShapes::new();
        for session in 0..40u32 {
            s.observe(session, 0, 20, 0, None);
            s.observe(session, 1, 28, 0, None);
            s.observe(session, 2, 36, 0, None);
        }
        let f = s.finish(&empty_sharing());
        assert_eq!(quantile_of(&f.growth_per_turn, 0.5), 8.0);
        // Turn 1 contributes to private_depth, not to growth.
        assert_eq!(quantile_of(&f.private_depth, 0.5), 20.0);
    }

    #[test]
    fn think_time_comes_out_in_seconds() {
        // `SessionParams::think_time_s` is seconds, so a fit that emitted
        // milliseconds would rescale every generated plan by a thousand.
        let mut s = SessionShapes::new();
        for session in 0..30u32 {
            s.observe(session, 0, 10, 0, Some(100.0));
            s.observe(session, 1, 12, 0, Some(102.5));
        }
        let f = s.finish(&empty_sharing());
        let v = quantile_of(&f.think_time, 0.5);
        assert!((v - 2.5).abs() < 0.01, "think time came out {v}s");
    }

    #[test]
    fn a_trace_without_timestamps_leaves_think_time_unset_and_says_so() {
        // FR-055: unset, never defaulted.
        let mut s = SessionShapes::new();
        for session in 0..10u32 {
            s.observe(session, 0, 10, 0, None);
            s.observe(session, 1, 12, 0, None);
        }
        let f = s.finish(&empty_sharing());
        assert!(f.think_time.is_none());
        assert!(f
            .caveats()
            .iter()
            .any(|c| c.contains("think_time is unset")));
        assert!(f.growth_per_turn.is_some(), "growth needs no timestamps");
    }

    #[test]
    fn out_of_order_turns_are_counted_rather_than_absorbed() {
        // FR-014a makes turn n+1 a strict extension of turn n, so a trace that
        // breaks the chain cannot be fitted honestly and the report must say so.
        let mut s = SessionShapes::new();
        s.observe(7, 0, 20, 0, None);
        s.observe(7, 0, 18, 0, None);
        let f = s.finish(&empty_sharing());
        assert_eq!(f.out_of_order_turns, 1);
        assert!(f.caveats().iter().any(|c| c.contains("out of order")));
        // A shrinking path floors at zero growth rather than producing a negative.
        assert_eq!(quantile_of(&f.growth_per_turn, 0.5), 0.0);
    }

    #[test]
    fn a_prefix_longer_than_its_path_is_flagged_as_impossible() {
        let mut s = SessionShapes::new();
        s.observe(1, 0, 5, 9, None);
        let f = s.finish(&empty_sharing());
        assert_eq!(f.prefix_longer_than_path, 1);
        assert!(f.caveats().iter().any(|c| c.contains("impossible")));
    }

    #[test]
    fn shared_depth_comes_from_the_sharing_histogram_it_will_be_validated_against() {
        let mut h = Hist::new();
        for d in [4u64, 4, 18, 18, 18, 40] {
            h.add(d);
        }
        h.seal();
        let sharing = SharingReport {
            requests: 6,
            sharing_requests: 6,
            unshared_requests: 0,
            shared_fraction: Some(1.0),
            realised_depth: h.summary(),
            depth_buckets: h.buckets(),
        };
        let f = SessionShapes::new().finish(&sharing);
        let d = f.shared_depth.expect("fitted");
        // The median of that histogram is 18, and the emitted empirical must agree.
        assert_eq!(d.quantile(0.5), Some(18.0));
        assert_eq!(d.quantile(1.0), Some(40.0));
    }

    #[test]
    fn requests_that_shared_nothing_are_reported_rather_than_folded_in() {
        let mut h = Hist::new();
        h.add(4);
        h.seal();
        let sharing = SharingReport {
            requests: 10,
            sharing_requests: 1,
            unshared_requests: 9,
            shared_fraction: Some(0.1),
            realised_depth: h.summary(),
            depth_buckets: h.buckets(),
        };
        let f = SessionShapes::new().finish(&sharing);
        assert_eq!(f.unshared_requests, 9);
        assert!(f.caveats().iter().any(|c| c.contains("shared nothing")));
    }

    #[test]
    fn private_depth_recomputes_against_a_raised_attempted_sharing() {
        // The prerequisite for any iteration on the realised-versus-attempted gap.
        // Turn-1 depth 30 with a realised prefix of 18 gives private_depth 12; if the
        // attempt is raised to 24 the private part must fall to 6, or the generated
        // path — attempted + private + growth — would run 6 blocks longer than the
        // trace's and fix the sharing statistic by breaking request length.
        let mut s = SessionShapes::new();
        for session in 0..50u32 {
            s.observe(session, 0, 30, 18, None);
        }
        assert_eq!(quantile_of(&s.private_depth_at(1.0), 0.5), 12.0);
        let raised = 24.0 / 18.0;
        assert_eq!(quantile_of(&s.private_depth_at(raised), 0.5), 6.0);
        // And the sum is invariant, which is the property that keeps path length fixed
        // while sharing moves.
        assert_eq!(quantile_of(&s.turn_one_depth(), 0.5), 30.0);
    }

    #[test]
    fn recomputing_at_scale_one_is_the_plain_measurement() {
        // Otherwise an iteration's first step would already have moved the model.
        let mut s = SessionShapes::new();
        for session in 0..30u32 {
            s.observe(
                session,
                0,
                20 + u64::from(session % 5),
                3 + u64::from(session % 3),
                None,
            );
        }
        let f = s.finish(&sharing_with(&[3]));
        assert_eq!(
            f.private_depth.as_ref().and_then(|d| d.quantile(0.5)),
            s.private_depth_at(1.0).and_then(|d| d.quantile(0.5))
        );
    }

    #[test]
    fn an_attempt_deeper_than_the_path_clamps_to_no_private_part() {
        // A negative private depth is not expressible, and the clamp is what keeps an
        // over-raised iteration from emitting one.
        let mut s = SessionShapes::new();
        for session in 0..20u32 {
            s.observe(session, 0, 10, 8, None);
        }
        assert_eq!(quantile_of(&s.private_depth_at(4.0), 0.5), 0.0);
    }

    #[test]
    fn the_pairs_survive_finishing_so_an_iteration_can_keep_asking() {
        // `finish` borrows rather than consumes, which is what lets a caller fit,
        // generate, measure and come back for another private_depth.
        let mut s = SessionShapes::new();
        for session in 0..25u32 {
            s.observe(session, 0, 40, 10, None);
        }
        let first = s.finish(&sharing_with(&[10]));
        let second = s.finish(&sharing_with(&[10]));
        assert_eq!(first.sessions, second.sessions);
        assert_eq!(
            first.private_depth.as_ref().and_then(|d| d.quantile(0.5)),
            second.private_depth.as_ref().and_then(|d| d.quantile(0.5))
        );
        assert!(s.private_depth_at(2.0).is_some());
    }

    #[test]
    fn an_empty_fit_leaves_every_parameter_unset() {
        let f = SessionShapes::new().finish(&empty_sharing());
        assert!(f.turns.is_none());
        assert!(f.private_depth.is_none());
        assert!(f.growth_per_turn.is_none());
        assert!(f.think_time.is_none());
        assert!(f.shared_depth.is_none());
    }

    #[test]
    fn an_emitted_empirical_distribution_round_trips_through_yaml() {
        // What `fit` writes has to be what `plan` can read.
        let mut s = SessionShapes::new();
        for session in 0..60u32 {
            s.observe(session, 0, 20 + u64::from(session % 7), 3, None);
        }
        let f = s.finish(&empty_sharing());
        let y = serde_yaml::to_string(&f.private_depth).expect("serialise");
        let back: Option<Dist> = serde_yaml::from_str(&y).expect("deserialise");
        assert_eq!(
            back.as_ref().and_then(|d| d.quantile(0.5)),
            f.private_depth.as_ref().and_then(|d| d.quantile(0.5))
        );
    }
}
