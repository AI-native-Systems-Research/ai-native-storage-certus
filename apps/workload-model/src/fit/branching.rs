//! Fitting a `branching` profile and `roots.count` (spec FR-055a, FR-055b, FR-055c).
//!
//! The executable form of `research.md` § The branching segmentation rule. Four
//! steps, and **no jump-ratio threshold** anywhere:
//!
//! 1. **Clip.** `f(d) = max(1, w(d)/w(d-1))`. Schema rule 8 requires `fanout >= 1`
//!    at every depth, so an observed *decrease* is not a fanout — it is censoring by
//!    session retirement, and it says nothing about branching. An *increase* cannot
//!    be produced by censoring, so it is genuine and is a **lower bound** on the true
//!    fanout. Width being an integer count, any increase is already at least one
//!    extra node, so nothing needs a threshold to be recognised.
//!
//! 2. **Merge at the generator's own resolution.** A non-integer mean fanout is
//!    realised by randomised rounding, so the fanout actually produced at a depth of
//!    width `w` is a Bernoulli average with standard error
//!    `sqrt(frac(1-frac)/w)`. Adjacent depths merge unless their fanouts differ by
//!    more than [`MERGE_Z`] such errors. A finer distinction would describe noise the
//!    generator cannot reproduce.
//!
//! 3. **Fit only the uncensored prefix, and only the shared keys.** The trunk is the
//!    keys two or more sessions reached — counting every key at a depth counts the
//!    trunk plus every private descent. A segment's fanout is also a *product* over
//!    its depths, so censoring compounds through it: segmentation stops at whichever
//!    comes first, cumulative retention under [`RETENTION_FLOOR`] or a depth no two
//!    sessions shared. Beyond that nothing is fitted and observed width is a lower
//!    bound.
//!
//! 4. **Fold the near-root levels** into `roots.count` for exactly as long as that is
//!    what keeps occupancy at the fitted sharing depth above the FR-009f floor — so
//!    FR-055c follows from FR-009f rather than being asserted, and a deep fanout event
//!    stops the fold rather than being pretended away.
//!
//! The direction of error is stated in the result and must reach the report: a trunk
//! fitted narrower than reality generates *more* sharing than the trace had.

use serde::{Deserialize, Serialize};

use crate::corpus::TARGET_OCCUPANCY;
use crate::schema::Segment;
use crate::stats::trunk::TrunkReport;

/// Standard errors two adjacent depths must differ by to stay separate segments.
///
/// Conventional rather than derived, and named as such in `research.md`. The
/// sensitivity is mild: the gap between a flat run and a real fanout event is tens of
/// standard errors, so 2 or 4 would segment these traces the same way.
pub const MERGE_Z: f64 = 3.0;

/// Cumulative retention below which the width profile is not fitted.
///
/// The knee at which the data stops contradicting the model: at 0.99, 12 of the 16
/// traces measured produce no forbidden decreasing ratio, against 6 of 16 at 0.95.
pub const RETENTION_FLOOR: f64 = 0.99;

/// A fitted trunk shape.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FittedBranching {
    /// The depth whose width became `roots.count`.
    ///
    /// FR-055c requires this in the report, because it changes what `roots.count`
    /// means: the levels above it are a global preamble prepended to every session.
    pub root_boundary_depth: u32,
    /// Distinct keys at the boundary depth.
    pub roots: u64,
    /// Retention at the boundary, so a `roots.count` read at a depth few requests
    /// reached is visibly that.
    pub retention_at_boundary: f64,
    /// The fitted profile, ascending, first segment at depth 0.
    pub segments: Vec<Segment>,
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
            "the fanout profile is a LOWER BOUND: an observed decrease in width is \
             censoring by session retirement rather than a fanout below 1, so where \
             censoring partly cancelled a real fanout the estimate is too small and \
             never too large. A trunk fitted narrower than reality generates more \
             sharing than the trace had ({} ratios discarded as censoring)",
            self.censored_ratios
        )];
        if self.observed_to_depth > self.fitted_to_depth {
            out.push(format!(
                "depths {}..={} were not fitted: past depth {} the profile fails one of the \
                 two gates — cumulative retention under {RETENTION_FLOOR}, so censoring would \
                 compound through a segment's product, or no key at that depth was reached \
                 by two sessions, so there is no trunk left to measure. Observed width beyond \
                 it is a lower bound and nothing was fitted from it",
                self.fitted_to_depth + 1,
                self.observed_to_depth,
                self.fitted_to_depth
            ));
        }
        if self.root_boundary_depth > 0 {
            out.push(format!(
                "roots.count is the width at depth {}, not at depth 0: the levels above \
                 it are a global preamble every session shares, and expressing that \
                 near-root fanout as trunk branching would fail the occupancy floor at \
                 any useful depth (FR-055c)",
                self.root_boundary_depth
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
    ratios: Vec<f64>,
    widths: Vec<f64>,
}

impl Building {
    /// The geometric mean of the ratios inside, as the contract specifies.
    fn fanout(&self) -> f64 {
        let n = self.ratios.len() as f64;
        (self.ratios.iter().map(|r| r.ln()).sum::<f64>() / n).exp()
    }

    fn mean_width(&self) -> f64 {
        self.widths.iter().sum::<f64>() / self.widths.len() as f64
    }
}

/// Fit a profile from a realised width-by-depth report.
///
/// `sessions_at_depth` supplies the occupancy numerator the root fold needs; it is the
/// distinct sessions reaching the fitted sharing depth. Returns `None` when the report
/// has no width to fit at all.
pub fn fit(report: &TrunkReport, sessions_at_sharing_depth: f64) -> Option<FittedBranching> {
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

    // Step 3 first, since it bounds everything else: the prefix that is both
    // uncensored *and* high-occupancy. Two gates, because they fail for different
    // reasons and neither implies the other.
    //
    // Retention protects against **compounding**: a segment's fanout is a product
    // over its depths, so censoring by session retirement accumulates through it.
    //
    // Occupancy protects against **misreading private descents as trunk**. The fit
    // definition of `w(d)` counts every key at a depth, private ones included,
    // because a trace cannot tell them apart — and where occupancy has collapsed to
    // roughly one session per key, that is what nearly all of them are. Taking the
    // width there as trunk width inflates `paths(d)` by orders of magnitude, and the
    // model that results fails FR-009f's occupancy floor: the fit would emit a
    // document the generator cannot realise. FR-055b already says a measured fanout
    // is trustworthy only in the high-occupancy region; this is that, enforced rather
    // than reported.
    //
    // `research.md` § The branching segmentation rule found the two gates coinciding
    // across all sixteen traces measured — every segment the retention floor admitted
    // sat at occupancy >= 4. That was corroboration, not a guarantee: a synthetic
    // trace with short sessions and deep private paths holds retention long after
    // occupancy has gone, which is exactly the case that needs the second gate.
    let base = depths[0].references_run.max(1) as f64;
    let mut fitted_to = 0usize;
    for d in depths {
        if (d.references_run as f64) / base < RETENTION_FLOOR {
            break;
        }
        // The trunk simply ends where no key at a depth was reached by two sessions.
        // There is nothing to measure past that, and no gate on *pooled* occupancy is
        // needed any more: counting only shared keys is what the pooled figure was
        // standing in for, and it stood in badly — a depth with five well-shared keys
        // beside five thousand private ones has pooled occupancy near 1 while its
        // shared width is perfectly measurable.
        if d.shared_keys_run == 0 {
            break;
        }
        fitted_to = d.depth as usize;
    }
    let fitted_to = fitted_to.min(widths.len() - 1);

    // Step 1: clip. A decrease is censoring, not a fanout below 1.
    let mut censored = 0u64;
    let mut segments: Vec<Building> = Vec::new();
    for d in 1..=fitted_to {
        let prev = widths[d - 1];
        if prev <= 0.0 {
            continue;
        }
        let raw = widths[d] / prev;
        let clipped = if raw < 1.0 {
            censored += 1;
            1.0
        } else {
            raw
        };
        segments.push(Building {
            from: d as u32,
            to: d as u32,
            ratios: vec![clipped],
            widths: vec![prev],
        });
    }

    // Step 2: merge the most consistent adjacent pair until the best merge is
    // distinguishable. Bottom-up rather than top-down because the null hypothesis is
    // the one the traces support overwhelmingly — a flat trunk — so the procedure
    // starts from the data and keeps only distinctions it can defend.
    while segments.len() > 1 {
        let mut best = 0usize;
        let mut best_z = f64::INFINITY;
        for i in 0..segments.len() - 1 {
            let (a, b) = (&segments[i], &segments[i + 1]);
            let (fa, fb) = (a.fanout(), b.fanout());
            let se = ((rounding_se(fa, a.mean_width()).powi(2) / a.ratios.len() as f64)
                + (rounding_se(fb, b.mean_width()).powi(2) / b.ratios.len() as f64))
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
        a.ratios.extend(b.ratios);
        a.widths.extend(b.widths);
    }

    // Step 4: fold near-root segments into roots.count while the occupancy floor
    // requires it.
    let mut boundary = 0usize;
    let mut roots = widths[0];
    let mut kept: Vec<Building> = segments;
    loop {
        let paths: f64 = kept.iter().fold(roots, |acc, s| {
            let span = (s.to.min(fitted_to as u32) as i64 - s.from as i64 + 1).max(0);
            acc * s.fanout().powi(span as i32)
        });
        let occupancy = if paths > 0.0 {
            sessions_at_sharing_depth / paths
        } else {
            0.0
        };
        if occupancy >= TARGET_OCCUPANCY || kept.len() <= 1 {
            break;
        }
        // Only a near-root segment may be folded. A deep fanout event cannot be
        // expressed as roots at all, so the fold stops rather than pretending.
        let first = kept[0].clone();
        if first.from as usize > fitted_to {
            break;
        }
        boundary = first.to as usize;
        roots = widths[boundary.min(widths.len() - 1)];
        kept.remove(0);
    }

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
        root_boundary_depth: boundary as u32,
        roots: roots as u64,
        retention_at_boundary: depths
            .get(boundary)
            .map(|d| d.references_run as f64 / base)
            .unwrap_or(0.0),
        segment_occupancy: kept.iter().map(occupancy_of).collect(),
        // Depths are **re-based on the root boundary**. After a fold, the emitted
        // profile's depth 0 is the trace's `boundary`, because `roots.count` is the
        // width there and every fitted fanout applies beneath it — a profile still
        // numbered in the trace's depths would place its first fanout `boundary`
        // levels too deep and describe a different trunk.
        //
        // The first segment then lands at 0, which is also what schema rule 8
        // requires: `paths(d)` multiplies `fanout(1..d)`, so `fanout_at(0)` is never
        // read and "from depth 0 onward" is the honest label for a segment covering
        // the step into depth 1.
        segments: terminate(
            kept.iter()
                .enumerate()
                .map(|(i, s)| Segment {
                    from_depth: if i == 0 {
                        0
                    } else {
                        s.from.saturating_sub(boundary as u32)
                    },
                    fanout: s.fanout(),
                    skew: None,
                    churn_half_life: None,
                })
                .collect(),
            fitted_to as u32,
            boundary as u32,
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
/// deeper than it was fitted. This is not a conservative default picked for safety: it
/// is what the estimator of `research.md` § The branching segmentation rule *already
/// yields* in that region. The estimator is `max(1, w(d)/w(d-1))`, one-sided because
/// schema rule 8 forbids fanout below 1, and the shared width beyond the fitted depth
/// is flat or falling — 31 keys at depth 71 against 9 at 1216 in the trace above. The
/// bug was never the estimate for those depths; it was that they silently inherited a
/// shallower region's estimate instead of their own.
///
/// Nothing is appended when the fit reached the deepest observed depth: there is no
/// unmeasured region to describe, and a redundant trailing segment would suggest one.
fn terminate(
    mut segments: Vec<Segment>,
    fitted_to: u32,
    boundary: u32,
    observed_to: u32,
) -> Vec<Segment> {
    if observed_to <= fitted_to {
        return segments;
    }
    // Re-based on the root boundary, like every other emitted depth. The step *into*
    // the first unfitted depth is the first one no evidence covers.
    //
    // Except when there are no fitted segments at all — everything folded into
    // `roots.count`, or nothing was fittable — in which case this is the *first*
    // segment and schema rule 8 requires the first to start at 0. That is also the
    // honest profile: with no fanout fitted anywhere, the claim is a flat trunk from
    // the root, which is what a single fanout-1.0 segment at depth 0 says.
    let from_depth = if segments.is_empty() {
        0
    } else {
        fitted_to.saturating_sub(boundary).saturating_add(1)
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
        let f = fit(&r, 200.0).expect("should fit");
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
        let f = fit(&r, 100.0).expect("should fit");
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
        let f = fit(&r, 216.0).expect("should fit");
        let biggest = f.segments.iter().map(|s| s.fanout).fold(0.0f64, f64::max);
        assert!(
            biggest > 2.0,
            "the 3x fanout was averaged away; segments {:?}",
            f.segments
        );
    }

    #[test]
    fn the_near_root_fanout_is_folded_into_roots_when_occupancy_demands_it() {
        // One root splitting many ways is better described as many roots: FR-055c,
        // and the fold goes exactly as deep as the FR-009f floor requires.
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
        // 400 sessions over the 200 paths a depth-1 fanout of 200 produces is
        // occupancy 2, below the target — so expressing that fanout as trunk
        // branching fails the floor and the fold is what the floor requires. At four
        // sessions per branch occupancy would be exactly 4 and no fold would be
        // needed, which is the rule declining rather than failing.
        let f = fit(&r, 400.0).expect("should fit");
        assert!(
            f.root_boundary_depth >= 1,
            "the near-root fanout should have been folded, boundary {}",
            f.root_boundary_depth
        );
        assert!(f.roots >= 100, "roots.count came out {}", f.roots);
        assert!(f.caveats().iter().any(|c| c.contains("global preamble")));
    }

    #[test]
    fn the_caveats_always_state_the_lower_bound() {
        let r = report_of(&shared_trunk(50, 10), 50);
        let f = fit(&r, 50.0).expect("should fit");
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
        let f = fit(&r, 40.0).expect("should fit");
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
        let f = fit(&r, 96.0).expect("should fit");
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
        let closed = terminate(Vec::new(), 71, 71, 2418);
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
        assert_eq!(terminate(one.clone(), 9, 0, 9).len(), 1);
        assert_eq!(terminate(one, 9, 0, 10).len(), 2);
    }

    #[test]
    fn an_empty_report_fits_nothing_rather_than_a_degenerate_profile() {
        let empty = Statistics::new(10).finish().trunk;
        assert!(fit(&empty, 0.0).is_none());
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
