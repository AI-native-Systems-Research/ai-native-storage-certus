//! Fitting a `branching` profile and `roots.count` (spec FR-055a, FR-055b, FR-055h).
//!
//! The executable form of `research.md` § The branching segmentation rule. Three
//! steps, and **no jump-ratio threshold** anywhere:
//!
//! 1. **Take the shared keys, over every depth that has any.** The trunk is the keys
//!    two or more sessions reached; counting every key at a depth counts the trunk plus
//!    every private descent. The region ends where no key at a depth was reached by two
//!    sessions, because there is no trunk left to measure — and nowhere else.
//!
//! 2. **Estimate each segment's fanout from its endpoints, in log space.** A segment's
//!    fanout is the geometric mean of the *unclipped* per-depth ratios over it, which
//!    telescopes: `exp(mean ln(w(d)/w(d-1))) = (w(to)/w(from-1))^(1/span)`. So the
//!    estimate depends only on the segment's endpoints and per-depth noise inside it
//!    **cancels** instead of accumulating. Rule 8's `fanout >= 1` is then imposed
//!    **once per segment** rather than at every depth.
//!
//! 3. **Merge at the generator's own resolution.** A non-integer mean fanout is
//!    realised by randomised rounding, so the fanout actually produced at a depth of
//!    width `w` is a Bernoulli average with standard error
//!    `sqrt(frac(1-frac)/w)`. Adjacent segments merge unless their fanouts differ by
//!    more than [`MERGE_Z`] such errors. A finer distinction would describe noise the
//!    generator cannot reproduce.
//!
//! # Why the per-depth clip had to go, and the gate with it (2026-08-14)
//!
//! Until 2026-08-14 step 1 clipped **every depth** — `f(d) = max(1, w(d)/w(d-1))` — and
//! step 2 multiplied the results. On a plateau that rectifies noise: the deep trunk of
//! all nine fittable corpus traces has an *unclipped* geometric-mean ratio of
//! **0.995–1.001**, a flat run with a log-slope of −0.001 to −0.006 per depth, and
//! clipping each downward step to 1 before multiplying turns it into unbounded growth.
//! Extending the old estimator to the last shared depth would have multiplied model
//! width by 4x on `browsecompplus`, 576x on `tau2_telecom`, 5.4e4 on `wildchat` and
//! **5e23** on `qwen_toc`.
//!
//! A cumulative-retention floor of 0.99 was what contained that, and it contained it by
//! **amputating the trace**: it stopped the fit at depth 0–74 while shared structure ran
//! 939–6094 deep, discarding **83–99% of the shared trunk on every trace**, and
//! `wildchat` fitted to depth 0 because 1.8% of its requests are one block long. The
//! floor's own justification did not survive either — "12 of 16 traces clean at 0.99"
//! was measured when `w(d)` meant *all* distinct keys at a depth, and under the shared-key
//! definition adopted 2026-08-12 the same test gives 2 of 9, with 7 of 9 traces already
//! admitting decreasing ratios inside the fitted prefix. It was guarding an estimator,
//! not the data.
//!
//! Estimating from endpoints removes the reason for the guard, so the region is no longer
//! bounded by retention. What replaces the guarantee is stronger and is asserted:
//! **the fitted profile is a non-decreasing envelope of the observed shared width and
//! never exceeds its running maximum**, because a segment's `fanout^span` is exactly
//! `max(1, w(to)/w(from-1))`. Retention is still measured and reported, because a width
//! read where few requests survive is thin evidence — but thin evidence is a caveat, not
//! a reason to describe none of the trace.
//!
//! The direction of error is stated in the result and must reach the report: a trunk
//! fitted narrower than reality generates *more* sharing than the trace had.

use serde::{Deserialize, Serialize};

use crate::schema::Segment;
use crate::stats::trunk::TrunkReport;

/// Standard errors two adjacent depths must differ by to stay separate segments.
///
/// Conventional rather than derived, and named as such in `research.md`. The
/// sensitivity is mild: the gap between a flat run and a real fanout event is tens of
/// standard errors, so 2 or 4 would segment these traces the same way.
pub const MERGE_Z: f64 = 3.0;

/// Cumulative retention below which a fitted segment is reported as thin evidence.
///
/// **No longer a gate.** It bounded the fitted region until 2026-08-14, on the grounds
/// that 12 of 16 traces produced no forbidden decreasing ratio at 0.99 against 6 of 16
/// at 0.95 — a criterion measured against the *old* `w(d)` of all distinct keys at a
/// depth, which under the shared-key definition gives 2 of 9. See the module docs: the
/// floor was containing the per-depth clip's rectification bias, and estimating a
/// segment from its endpoints removes the need. It is kept as the threshold at which the
/// report says the evidence thinned, since a width read where few requests survive is
/// worth flagging even though describing none of the trace is worse.
pub const RETENTION_THIN: f64 = 0.99;

/// A fitted trunk shape.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FittedBranching {
    /// Shared keys at depth 0 — the whole root layer, with nothing folded away.
    pub roots: u64,
    /// Retention at the deepest fitted depth, so a profile fitted far into a thinning
    /// population is visibly that.
    ///
    /// Reported rather than enforced: it bounded the fitted region until 2026-08-14 and
    /// cost 83–99% of the shared trunk on every corpus trace (see the module docs).
    pub retention_at_fitted_to: f64,
    /// The fitted profile, ascending, first segment at depth 0.
    pub segments: Vec<Segment>,
    /// Each segment's **unclipped** rate, in the same order as `segments`.
    ///
    /// A segment whose raw rate is below 1 is narrowing, and rule 8 forbids the document
    /// from saying so — the profile carries 1.0 there. Publishing both is what lets a
    /// reader tell a trunk that stopped widening from one that is actively narrowing,
    /// which the emitted YAML cannot express.
    pub raw_fanouts: Vec<f64>,
    /// Mean occupancy over each segment's depths, in the same order.
    ///
    /// FR-055b: a fanout near 1 measured at occupancy near 1 is not evidence of a
    /// linear trunk, so the two are never reported apart.
    pub segment_occupancy: Vec<f64>,
    /// Deepest depth that was fitted. Beyond it the profile says nothing.
    pub fitted_to_depth: u32,
    /// Deepest depth with any observed width, fitted or not.
    pub observed_to_depth: u32,
    /// Ratios discarded as censoring, i.e. observed decreases in width.
    pub censored_ratios: u64,
    /// Whether the whole profile is a lower bound — always true, and stated rather
    /// than left for a reader to recall.
    pub is_lower_bound: bool,
}

impl FittedBranching {
    /// The caveats a fit report must carry (FR-055a, FR-055b).
    pub fn caveats(&self) -> Vec<String> {
        let mut out = vec![format!(
            "the fanout profile is a LOWER BOUND: an observed decrease in width may be \
             censoring by session retirement rather than a real narrowing, so where censoring \
             partly cancelled a real fanout the estimate is too small and never too large. A \
             trunk fitted narrower than reality generates more sharing than the trace had \
             ({} of the {} depths fitted stepped DOWN in width; they are averaged into their \
             segment's rate rather than discarded, which is what keeps a flat run flat instead \
             of compounding upward)",
            self.censored_ratios, self.fitted_to_depth
        )];
        if self.observed_to_depth > self.fitted_to_depth {
            out.push(format!(
                "depths {}..={} were not fitted: no key at depth {} was reached by two \
                 sessions, so there is no trunk left to measure. Observed width beyond it is \
                 a lower bound and nothing was fitted from it",
                self.fitted_to_depth + 1,
                self.observed_to_depth,
                self.fitted_to_depth + 1
            ));
        }
        if self.retention_at_fitted_to < RETENTION_THIN {
            out.push(format!(
                "only {:.1}% of requests still reached depth {}, the deepest depth fitted: the \
                 width there is measured over a thinned population, so the fanouts covering \
                 the deep segments rest on less evidence than the near-root ones. The region \
                 is still fitted — a retention floor used to stop the fit here and cost 83-99% \
                 of the shared trunk on every trace measured — but a segment whose occupancy \
                 is also low (below) is where the two thin signals coincide",
                self.retention_at_fitted_to * 100.0,
                self.fitted_to_depth
            ));
        }
        // Aggregated, deliberately: a real trace narrows over most of its depth, so one
        // caveat per segment is dozens of near-identical lines that bury every other
        // caveat in the report. The count, the span and the steepest rate are what a
        // reader acts on.
        let narrowing: Vec<(u32, f64)> = self
            .segments
            .iter()
            .zip(self.raw_fanouts.iter())
            .filter(|(_, raw)| **raw < 1.0)
            .map(|(s, raw)| (s.from_depth, *raw))
            .collect();
        if let Some((first, _)) = narrowing.first() {
            let steepest = narrowing.iter().map(|(_, r)| *r).fold(1.0f64, f64::min);
            let last = narrowing.last().map(|(d, _)| *d).unwrap_or(*first);
            out.push(format!(
                "{} of {} segments are NARROWING rather than widening, from depth {first} to \
                 depth {last}, the steepest at a rate of {steepest:.4} per depth — and the \
                 profile states fanout 1.0 for every one of them, because schema rule 8 forbids \
                 a fanout below 1. So the model's trunk STOPS widening where the trace's starts \
                 shrinking, and from the first such depth onward the model is wider than the \
                 trace: the emitted profile is a non-decreasing envelope of the observed shared \
                 width, which bounds the error but does not remove it. Expressing a narrowing \
                 trunk would need a schema that admits fanout below 1, which is FR-009f's floor \
                 in reverse and is not a change this fit can make on its own",
                narrowing.len(),
                self.segments.len()
            ));
        }
        for (s, occ) in self.segments.iter().zip(self.segment_occupancy.iter()) {
            if *occ < 2.0 {
                out.push(format!(
                    "the segment from depth {} was measured at occupancy {occ:.1}: at \
                     roughly one session per path the width ratio collapses toward 1 \
                     whatever the true branching, so its fanout of {:.4} is not evidence \
                     of a linear trunk (FR-055b)",
                    s.from_depth, s.fanout
                ));
            }
        }
        out
    }
}

/// Standard error of a realised fanout `f` at a depth of width `w`.
///
/// Randomised rounding gives each of the `w` nodes `floor(f)` or `ceil(f)` children,
/// taking the higher with probability `frac`, so the realised mean is a Bernoulli
/// average. Floored at half a child per node: a segment whose width is tiny resolves
/// nothing and must not claim a small error.
fn rounding_se(f: f64, w: f64) -> f64 {
    let w = w.max(1.0);
    let frac = f - f.floor();
    (frac * (1.0 - frac) / w).sqrt().max(0.5 / w)
}

/// One segment under construction.
#[derive(Debug, Clone)]
struct Building {
    from: u32,
    to: u32,
    /// Per-depth **log** ratios `ln(w(d)/w(d-1))`, unclipped — a decrease is a negative
    /// entry and is allowed to cancel a neighbouring increase, which is the whole point.
    log_ratios: Vec<f64>,
    widths: Vec<f64>,
}

impl Building {
    /// The segment's fanout: the geometric mean of the unclipped ratios, floored at 1.
    ///
    /// In log space the mean telescopes, so this equals `(w(to)/w(from-1))^(1/span)` and
    /// depends only on the endpoints. Rule 8's floor is applied **here, once**, rather
    /// than at every depth: `fanout^span` is then exactly `max(1, w(to)/w(from-1))`, so
    /// the segment can never claim more width than was observed at its own end. Clipping
    /// each depth instead makes the product `prod(max(1, r_d))`, which on a flat noisy
    /// run grows without bound — measured up to 5e23 over one real trace's depth range.
    fn fanout(&self) -> f64 {
        let n = self.log_ratios.len() as f64;
        (self.log_ratios.iter().sum::<f64>() / n).exp().max(1.0)
    }

    /// The unclipped rate, for reporting a segment that is genuinely narrowing.
    ///
    /// Kept separate from [`Building::fanout`] because the difference between the two is
    /// exactly what rule 8 forbids the document from saying, and a reader who is deciding
    /// whether a flat segment is real needs to see it.
    fn raw_fanout(&self) -> f64 {
        let n = self.log_ratios.len() as f64;
        (self.log_ratios.iter().sum::<f64>() / n).exp()
    }

    fn mean_width(&self) -> f64 {
        self.widths.iter().sum::<f64>() / self.widths.len() as f64
    }
}

/// Fit a profile from a realised width-by-depth report.
///
/// Returns `None` when the report has no width to fit at all.
///
/// Took a `sessions_at_sharing_depth` occupancy numerator until 2026-08-14, which existed
/// only to drive the near-root fold. Occupancy is judged where it belongs — by schema rule
/// 16, against the assembled document, at `p99(shared_depth)` — and the fit's job is to
/// report the trunk the trace has and let that judgement stand (FR-055a).
pub fn fit(report: &TrunkReport) -> Option<FittedBranching> {
    let depths = &report.depths;
    if depths.is_empty() || depths[0].width_run == 0 {
        return None;
    }
    // The trunk is the **shared** keys, not every key at a depth.
    //
    // `contracts/workload-schema.md` § Fitting used to define this as "distinct keys
    // at depth d", on the grounds that a trace cannot tell a shared node from a
    // private one. It can, wherever it has session identity: a node two sessions
    // reached is trunk, and a node one session reached is a private descent. Counting
    // both is counting the trunk plus every private path, which for a deep-private
    // workload is off by orders of magnitude — the fitted `roots.count` came out 1770
    // against the 12 the source document stated, and the resulting model failed
    // FR-009f's occupancy floor.
    //
    // The cost is that this is measurable only with session identity, which
    // `Capabilities::trunk_fittable` already requires for exactly this reason.
    let widths: Vec<f64> = depths.iter().map(|d| d.shared_keys_run as f64).collect();
    let observed_to_depth = (widths.len() - 1) as u32;

    // Step 1: the region. The trunk ends where no key at a depth was reached by two
    // sessions, because there is nothing left to measure — and nowhere else.
    //
    // A cumulative-retention floor used to bound this as well, and removing it is the
    // substance of the 2026-08-14 change (see the module docs): it was containing the
    // per-depth clip's rectification bias at the cost of 83–99% of the shared trunk on
    // every trace. Retention is measured below and reported; it no longer decides which
    // depths exist.
    //
    // No gate on *pooled* occupancy is needed either: counting only shared keys is what
    // the pooled figure was standing in for, and it stood in badly — a depth with five
    // well-shared keys beside five thousand private ones has pooled occupancy near 1
    // while its shared width is perfectly measurable.
    let base = depths[0].references_run.max(1) as f64;
    let mut fitted_to = 0usize;
    for d in depths {
        if d.shared_keys_run == 0 {
            break;
        }
        fitted_to = d.depth as usize;
    }
    let fitted_to = fitted_to.min(widths.len() - 1);

    // Step 2: one candidate segment per depth, carrying its **unclipped** log ratio.
    // Rule 8's floor is imposed per segment by `Building::fanout`, after merging, so a
    // downward step here cancels an upward one instead of being rectified into growth.
    // The count of downward steps is still reported: it is the evidence that a flat
    // segment is flat because the trunk stopped widening rather than because nothing
    // was measured.
    let mut censored = 0u64;
    let mut segments: Vec<Building> = Vec::new();
    for d in 1..=fitted_to {
        let prev = widths[d - 1];
        if prev <= 0.0 || widths[d] <= 0.0 {
            continue;
        }
        let raw = widths[d] / prev;
        if raw < 1.0 {
            censored += 1;
        }
        segments.push(Building {
            from: d as u32,
            to: d as u32,
            log_ratios: vec![raw.ln()],
            widths: vec![prev],
        });
    }

    // Step 3: merge the most consistent adjacent pair until the best merge is
    // distinguishable. Bottom-up rather than top-down because the null hypothesis is
    // the one the traces support overwhelmingly — a flat trunk — so the procedure
    // starts from the data and keeps only distinctions it can defend.
    //
    // Compared on the **unclipped** rate. Two narrowing segments both clip to 1.0 and
    // would look identical, so comparing the clipped values would merge a steep decline
    // with a shallow one and then describe both as flat; the distinction is real even
    // though rule 8 forbids the document from expressing it.
    while segments.len() > 1 {
        let mut best = 0usize;
        let mut best_z = f64::INFINITY;
        for i in 0..segments.len() - 1 {
            let (a, b) = (&segments[i], &segments[i + 1]);
            let (fa, fb) = (a.raw_fanout(), b.raw_fanout());
            let se = ((rounding_se(fa, a.mean_width()).powi(2) / a.log_ratios.len() as f64)
                + (rounding_se(fb, b.mean_width()).powi(2) / b.log_ratios.len() as f64))
                .sqrt();
            let z = if se > 0.0 {
                (fa - fb).abs() / se
            } else {
                f64::INFINITY
            };
            if z < best_z {
                best_z = z;
                best = i;
            }
        }
        if best_z > MERGE_Z {
            break;
        }
        let b = segments.remove(best + 1);
        let a = &mut segments[best];
        a.to = b.to;
        a.log_ratios.extend(b.log_ratios);
        a.widths.extend(b.widths);
    }

    // `roots.count` is the shared width at depth 0, and the near-root **fold is gone**
    // (2026-08-14). It used to re-base the profile on a deeper "boundary" depth while the
    // model's implied path count left occupancy below `TARGET_OCCUPANCY`, on FR-055c's
    // reasoning that one root splitting many ways is better described as many roots.
    //
    // Three measurements retired it, and the first is decisive on its own:
    //
    // * **It could not move the quantity its own loop tested.** A segment's
    //   `fanout^span` is exactly `w(to)/w(from-1)` — it telescopes — so dropping the
    //   shallowest segment and re-basing `roots` onto the width at its end leaves the
    //   product *identical*. The loop therefore iterated until it ran out of segments
    //   rather than until occupancy was satisfied, on the one corpus trace where it
    //   fired at all. Under the old per-depth clip the identity was broken only by
    //   clipping, which is to say the fold's entire effect on its own metric came from
    //   the estimator's bias.
    // * **It changed a different quantity by 4.6x.** While the loop's path count stayed
    //   at 719 on `qwen_code`, the emitted profile's width at the last fitted depth moved
    //   3941 -> 857 — so it did move what rule 16 judges, just not what it measured.
    // * **Forcing it changed nothing observable.** On `tau2_airline` (roots 26 -> 40) and
    //   `browsecompplus` (24 -> 42) the generated workload was bit-identical on all four
    //   FR-056 statistics, because `roots.count` does not survive to the generator once
    //   `roots.popularity` is empirical. Where it did change output (`qwen_code`, three
    //   seeds) removing it *improved* reuse distance from 3.12x to 1.40x tolerance.
    //
    // What replaces it is what FR-055a already required: **fail rather than substitute.**
    // A trace whose measured trunk cannot meet the FR-009f occupancy floor is a model
    // limitation to be reported, not a profile to be silently re-based — and re-basing was
    // exactly the silent substitution that requirement forbids. Rule 16 now judges the
    // trunk the trace actually has.
    let roots = widths[0];
    let kept: Vec<Building> = segments;

    let occupancy_of = |s: &Building| -> f64 {
        let vals: Vec<f64> = (s.from..=s.to)
            .filter_map(|d| depths.get(d as usize).and_then(|x| x.occupancy))
            .collect();
        if vals.is_empty() {
            0.0
        } else {
            vals.iter().sum::<f64>() / vals.len() as f64
        }
    };

    Some(FittedBranching {
        roots: roots as u64,
        retention_at_fitted_to: depths
            .get(fitted_to)
            .map(|d| d.references_run as f64 / base)
            .unwrap_or(0.0),
        segment_occupancy: kept.iter().map(occupancy_of).collect(),
        raw_fanouts: kept.iter().map(|s| s.raw_fanout()).collect(),
        // Depths are the trace's own, since `roots.count` is now the width at depth 0
        // and nothing is re-based. The first segment still lands at 0, which is what
        // schema rule 8 requires: `paths(d)` multiplies `fanout(1..d)`, so `fanout_at(0)`
        // is never read and "from depth 0 onward" is the honest label for a segment
        // covering the step into depth 1.
        segments: terminate(
            kept.iter()
                .enumerate()
                .map(|(i, s)| Segment {
                    from_depth: if i == 0 { 0 } else { s.from },
                    fanout: s.fanout(),
                    skew: None,
                    churn_half_life: None,
                })
                .collect(),
            fitted_to as u32,
            observed_to_depth,
        ),
        fitted_to_depth: fitted_to as u32,
        observed_to_depth,
        censored_ratios: censored,
        is_lower_bound: true,
    })
}

/// Close the profile at the last depth it was fitted from.
///
/// A segment's fanout holds "from here until the next segment", so the last segment in
/// the list applies to **unbounded depth** — and a fitted profile that ends in a
/// fanout above 1 therefore claims a trunk that widens forever, on evidence from a
/// region that may be a fraction of the trace's depth. That is not a conservative
/// error, because width is a *product*: it compounds.
///
/// Measured on a real agentic trace, fitted over depths 0..=71 and observed to 2418:
/// the model's implied width at depth 1216 was **1 816 434 against an observed 66**,
/// and at 2418 it was 1.2e11 against an observed 1. Rule 16 judges occupancy at
/// `p99(shared_depth)`, which sat at 1231 — deep inside the extrapolated region — so
/// the document was refused for a trunk the trace never had. Every individual segment
/// looked reasonable; only the extrapolation was wrong.
///
/// So a terminal segment of fanout **1.0** is appended wherever the trace was observed
/// deeper than it was fitted. This is not a conservative default picked for safety: it is
/// what the estimator yields in that region. Past `fitted_to` no key at a depth was
/// reached by two sessions, so there is no shared width left to divide — and rule 8
/// forbids stating the narrowing that the observed width actually does. The bug was never
/// the estimate for those depths; it was that they silently inherited a shallower region's
/// estimate instead of their own.
///
/// Far less of the trace reaches this path since 2026-08-14: the region now ends where
/// sharing ends rather than where retention crosses a floor, so `fitted_to` moved from
/// depth 48 to 1300 on `tau2_airline` and from 2 to 1234 on `qwen_code`.
///
/// Nothing is appended when the fit reached the deepest observed depth: there is no
/// unmeasured region to describe, and a redundant trailing segment would suggest one.
fn terminate(mut segments: Vec<Segment>, fitted_to: u32, observed_to: u32) -> Vec<Segment> {
    if observed_to <= fitted_to {
        return segments;
    }
    // In the trace's own depths, since nothing is re-based any more. The step *into*
    // the first unfitted depth is the first one no evidence covers.
    //
    // Except when there are no fitted segments at all — nothing was fittable — in which
    // case this is the *first* segment and schema rule 8 requires the first to start at
    // 0. That is also the honest profile: with no fanout fitted anywhere, the claim is a
    // flat trunk from the root, which is what a single fanout-1.0 segment at depth 0 says.
    let from_depth = if segments.is_empty() {
        0
    } else {
        fitted_to.saturating_add(1)
    };
    // A segment already starting there would be one the fit produced, which cannot
    // happen — nothing is fitted past `fitted_to` — but if the arithmetic ever made
    // them coincide, overwriting the fitted value with 1.0 would discard a
    // measurement.
    if segments.last().is_some_and(|s| s.from_depth >= from_depth) {
        return segments;
    }
    segments.push(Segment {
        from_depth,
        fanout: 1.0,
        skew: None,
        churn_half_life: None,
    });
    segments
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::keys::{CacheKey, SessionId};
    use crate::stats::{Ref, Statistics};

    /// A report over requests given as `(session, path)`.
    fn report_of(requests: &[(u32, Vec<u64>)], window: u64) -> TrunkReport {
        let mut s = Statistics::new(window);
        for (session, path) in requests {
            for (i, k) in path.iter().enumerate() {
                s.push(&Ref {
                    key: CacheKey(*k),
                    size: 16,
                    depth: i as u32,
                    session: SessionId(*session),
                    request_start: i == 0,
                    warmup: false,
                });
            }
        }
        s.finish().trunk
    }

    /// `n` sessions all walking one shared path of `depth` blocks.
    /// The model's implied width at `depth`: roots times every fanout at or below it.
    ///
    /// Deliberately `Profile::paths` rather than a second implementation: `paths`
    /// multiplies `fanout_at(1..=depth)`, so `fanout_at(0)` is never read and the first
    /// segment's fanout describes the step *into* depth 1. A hand-rolled version here
    /// counted a level at depth 0 and reported 12 where the profile means 4.
    fn model_width(f: &FittedBranching, depth: usize) -> f64 {
        crate::corpus::Profile::from_segments(&f.segments).paths(depth as u32, f.roots as u32)
    }

    fn shared_trunk(n: u32, depth: usize) -> Vec<(u32, Vec<u64>)> {
        (0..n)
            .map(|s| (s, (0..depth).map(|d| d as u64).collect()))
            .collect()
    }

    #[test]
    fn a_perfectly_flat_trunk_fits_one_segment_at_fanout_one() {
        // Every session on the same path: width 1 at every depth, so there is one
        // segment and its fanout is exactly 1.
        let r = report_of(&shared_trunk(200, 30), 200);
        let f = fit(&r).expect("should fit");
        assert_eq!(f.segments.len(), 1);
        assert!((f.segments[0].fanout - 1.0).abs() < 1e-12);
        assert_eq!(f.censored_ratios, 0, "a flat trunk censors nothing");
        assert!(f.is_lower_bound, "always a lower bound");
    }

    #[test]
    fn a_decrease_in_width_is_censoring_and_never_a_fanout_below_one() {
        // Schema rule 8 forbids fanout < 1, so a narrowing profile must not produce
        // one — a fitted profile that did would be rejected by the validator it was
        // fitted for.
        let mut reqs = shared_trunk(100, 4);
        // Ten sessions continue deeper on their own private paths, so width at depth
        // 4 and 5 falls as they retire.
        for s in 0..10u32 {
            reqs.push((
                s,
                (0..6)
                    .map(|d| {
                        if d < 4 {
                            d as u64
                        } else {
                            900 + u64::from(s) * 10 + d as u64
                        }
                    })
                    .collect(),
            ));
        }
        let r = report_of(&reqs, 500);
        let f = fit(&r).expect("should fit");
        for s in &f.segments {
            assert!(
                s.fanout >= 1.0,
                "fitted a fanout of {} below 1, which rule 8 forbids",
                s.fanout
            );
        }
    }

    #[test]
    fn a_fanout_event_survives_as_its_own_segment() {
        // Twelve roots, one shared level, then each root splits three ways: the
        // profile should carry a segment whose fanout is near 3 rather than average
        // it into the flat run around it.
        let mut reqs = Vec::new();
        for root in 0..12u64 {
            for child in 0..3u64 {
                for rep in 0..6u32 {
                    let session = (root * 18 + child * 6 + u64::from(rep)) as u32;
                    reqs.push((
                        session,
                        vec![
                            root,
                            100 + root,
                            1000 + root * 10 + child,
                            5000 + root * 10 + child,
                        ],
                    ));
                }
            }
        }
        let r = report_of(&reqs, 1000);
        let f = fit(&r).expect("should fit");
        let biggest = f.segments.iter().map(|s| s.fanout).fold(0.0f64, f64::max);
        assert!(
            biggest > 2.0,
            "the 3x fanout was averaged away; segments {:?}",
            f.segments
        );
    }

    #[test]
    fn a_near_root_fanout_stays_a_fanout_and_roots_is_the_width_at_depth_zero() {
        // The near-root fold's replacement, and the inverse of the test that stood here.
        // One root splitting 200 ways used to be re-described as ~200 roots whenever the
        // implied occupancy fell below TARGET_OCCUPANCY (FR-055c). It is now left as
        // what it is: `roots.count` is the shared width at depth 0, the fanout stays in
        // the profile, and whether the result meets the FR-009f floor is rule 16's
        // judgement on the assembled document — which is what FR-055a's "fail rather
        // than substitute" already required.
        let mut reqs = Vec::new();
        for branch in 0..200u64 {
            for rep in 0..2u32 {
                reqs.push((
                    (branch * 2 + u64::from(rep)) as u32,
                    vec![0, 1000 + branch, 5000 + branch],
                ));
            }
        }
        let r = report_of(&reqs, 2000);
        let f = fit(&r).expect("should fit");
        assert_eq!(f.roots, 1, "roots.count is the shared width at depth 0");
        let biggest = f.segments.iter().map(|s| s.fanout).fold(0.0f64, f64::max);
        assert!(
            biggest > 100.0,
            "the near-root fanout must survive as a fanout, segments {:?}",
            f.segments
        );
        assert!(
            !f.caveats().iter().any(|c| c.contains("global preamble")),
            "nothing is folded away, so nothing is a hidden preamble: {:?}",
            f.caveats()
        );
    }

    #[test]
    fn a_noisy_plateau_fits_flat_instead_of_compounding_into_absurdity() {
        // The defect that retired the per-depth clip. A flat trunk measured with noise
        // has ratios scattered either side of 1; clipping each to `max(1, r)` and
        // multiplying rectifies the noise into growth, which is what forced a retention
        // floor to cut the fit off after a few dozen depths. Extending the old estimator
        // over one real trace's depth range would have inflated model width by 5e23.
        //
        // Built so the width wobbles 18-22 with no trend over 60 depths.
        let mut reqs: Vec<(u32, Vec<u64>)> = Vec::new();
        let mut session = 0u32;
        let mut key = 1_000u64;
        let widths: Vec<usize> = (0..60)
            .map(|d| [20usize, 22, 19, 21, 18, 20, 21, 19][d % 8])
            .collect();
        // Each depth's keys are shared by two sessions, so every one counts as trunk.
        for (d, w) in widths.iter().enumerate() {
            for _ in 0..*w {
                key += 1;
                for _ in 0..2 {
                    reqs.push((session, (0..=d).map(|i| key * 100 + i as u64).collect()));
                    session += 1;
                }
            }
        }
        let r = report_of(&reqs, 100_000);
        let f = fit(&r).expect("should fit");
        // The model's width at the deepest fitted depth, the quantity rule 16 divides
        // the session population into.
        let model = model_width(&f, f.fitted_to_depth as usize);
        let observed_max = r
            .depths
            .iter()
            .take(f.fitted_to_depth as usize + 1)
            .map(|d| d.shared_keys_run)
            .max()
            .unwrap_or(0) as f64;
        assert!(
            model <= observed_max * 1.001,
            "model width {model} at depth {} exceeds the observed running maximum \
             {observed_max}: the estimator is compounding noise again",
            f.fitted_to_depth
        );
        assert!(
            f.censored_ratios > 0,
            "the fixture must actually contain downward steps, or this proves nothing"
        );
    }

    #[test]
    fn the_fitted_profile_never_claims_more_width_than_was_observed() {
        // The invariant that replaces the retention gate's guarantee, stated as a
        // property rather than as a threshold: a segment's fanout^span is exactly
        // max(1, w(to)/w(from-1)), so the profile is a non-decreasing envelope of the
        // observed shared width and can never exceed its running maximum. Checked at
        // EVERY fitted depth, on a trunk with a real fanout event and real attrition.
        let mut reqs = Vec::new();
        for root in 0..8u64 {
            for child in 0..4u64 {
                for rep in 0..3u32 {
                    let s = (root * 12 + child * 3 + u64::from(rep)) as u32;
                    let mut path = vec![root, 50 + root];
                    path.push(500 + root * 10 + child);
                    // A tail whose length varies by child, so width falls with depth.
                    for i in 0..(2 + child) {
                        path.push(9000 + root * 100 + child * 10 + i);
                    }
                    reqs.push((s, path));
                }
            }
        }
        let r = report_of(&reqs, 10_000);
        let f = fit(&r).expect("should fit");
        let mut running_max = 0f64;
        for d in 0..=f.fitted_to_depth as usize {
            running_max = running_max.max(r.depths[d].shared_keys_run as f64);
            let model = model_width(&f, d);
            assert!(
                model <= running_max * 1.001,
                "at depth {d} the model claims width {model} against an observed running \
                 maximum of {running_max}"
            );
        }
    }

    #[test]
    fn the_caveats_always_state_the_lower_bound() {
        let r = report_of(&shared_trunk(50, 10), 50);
        let f = fit(&r).expect("should fit");
        let c = f.caveats();
        assert!(c[0].contains("LOWER BOUND"), "{c:?}");
        assert!(c[0].contains("more sharing than the trace had"), "{c:?}");
    }

    #[test]
    fn a_low_occupancy_depth_is_not_fitted_at_all() {
        // FR-055b, enforced rather than merely reported: at one session per key the
        // distinct keys at a depth are private descents, so a width ratio there is a
        // count of private paths. Fitting it as trunk inflates `paths(d)` and produces
        // a document that fails FR-009f's occupancy floor — a fit that emits a model
        // the generator cannot realise.
        let reqs: Vec<(u32, Vec<u64>)> = (0..40u32)
            .map(|s| (s, vec![u64::from(s) * 10, u64::from(s) * 10 + 1]))
            .collect();
        let r = report_of(&reqs, 100);
        let f = fit(&r).expect("should fit");
        assert_eq!(
            f.fitted_to_depth, 0,
            "depth 1 sits at occupancy 1 and must be excluded"
        );
        assert!(
            f.caveats().iter().any(|c| c.contains("were not fitted")),
            "the exclusion was not reported: {:?}",
            f.caveats()
        );
    }

    #[test]
    fn the_profile_stops_claiming_fanout_where_it_stopped_being_fitted() {
        // The defect this pins, measured on a real agentic trace: a fanout of 1.009
        // fitted over depths 0..=71 was extrapolated to depth 1216, where the model's
        // implied width came to 1_816_434 against an **observed 66** — and rule 16
        // judges occupancy at p99(shared_depth), which sat deep inside that region.
        // Width is a product, so the error compounds rather than staying small.
        //
        // Here: a wide shared trunk that a few sessions descend far past. The fitted
        // region ends where the shared width does, so the profile must be closed off
        // at fanout 1.0 rather than carrying the near-root fanout downward forever.
        let mut reqs = Vec::new();
        for root in 0..6u64 {
            for child in 0..2u64 {
                for rep in 0..8u32 {
                    let session = (root * 16 + child * 8 + u64::from(rep)) as u32;
                    let mut path = vec![root, 100 + root, 1000 + root * 10 + child];
                    // Two sessions per root run on far past the shared region, on
                    // paths no one else touches — the private tail every real
                    // agentic trace has.
                    if rep < 2 {
                        path.extend((0..40u64).map(|i| 50_000 + session as u64 * 100 + i));
                    }
                    reqs.push((session, path));
                }
            }
        }
        let r = report_of(&reqs, 500);
        let f = fit(&r).expect("should fit");
        assert!(
            f.observed_to_depth > f.fitted_to_depth,
            "the fixture must have an unfitted region for this test to mean anything"
        );
        let last = f.segments.last().expect("a profile");
        assert_eq!(
            last.fanout, 1.0,
            "the profile's last segment claims fanout {} past depth {}, where nothing \
             was fitted; segments {:?}",
            last.fanout, f.fitted_to_depth, f.segments
        );

        // And the model's width must stay near what was actually observed, rather
        // than compounding away from it. This is the assertion that would have
        // caught the real-trace failure.
        let width_at = |d: u32| -> f64 {
            let mut w = f.roots as f64;
            for (i, s) in f.segments.iter().enumerate() {
                if s.from_depth > d {
                    break;
                }
                let end = f
                    .segments
                    .get(i + 1)
                    .map(|n| n.from_depth)
                    .unwrap_or(u32::MAX)
                    .min(d + 1);
                w *= s.fanout.powi(end.saturating_sub(s.from_depth) as i32);
            }
            w
        };
        let deep = width_at(f.observed_to_depth);
        let fitted_edge = width_at(f.fitted_to_depth);
        assert!(
            deep <= fitted_edge * 1.001,
            "width grew from {fitted_edge} to {deep} across depths nothing was fitted from"
        );
    }

    #[test]
    fn a_profile_with_no_fitted_segments_still_starts_at_depth_zero() {
        // Schema rule 8 requires the first segment at from_depth 0. When every fitted
        // segment is folded into `roots.count`, the terminal segment *is* the first
        // one — and emitting it at the depth where fitting stopped produced a document
        // rule 8 rejected outright. A flat trunk from the root is both rule-8-valid and
        // the honest claim when no fanout was fitted anywhere.
        let closed = terminate(Vec::new(), 71, 2418);
        assert_eq!(closed.len(), 1);
        assert_eq!(closed[0].from_depth, 0);
        assert_eq!(closed[0].fanout, 1.0);
    }

    #[test]
    fn a_profile_fitted_to_the_bottom_gets_no_terminal_segment() {
        // Nothing unmeasured to describe, and a redundant trailing segment would
        // suggest there was.
        let one = vec![Segment {
            from_depth: 0,
            fanout: 2.0,
            skew: None,
            churn_half_life: None,
        }];
        assert_eq!(terminate(one.clone(), 9, 9).len(), 1);
        assert_eq!(terminate(one, 9, 10).len(), 2);
    }

    #[test]
    fn an_empty_report_fits_nothing_rather_than_a_degenerate_profile() {
        let empty = Statistics::new(10).finish().trunk;
        assert!(fit(&empty).is_none());
    }

    #[test]
    fn the_rounding_error_matches_the_generators_measured_resolution() {
        // The number the merge criterion rests on: a 1.05 profile over 200 nodes
        // predicts 0.0154, and the generator measured 1.014x at p90.
        let se = rounding_se(1.05, 200.0);
        assert!((se - 0.0154).abs() < 0.0005, "se came out {se}");
        // Wider depths resolve finer, as 1/sqrt(w).
        assert!(rounding_se(1.05, 800.0) < se / 1.9);
    }
}
