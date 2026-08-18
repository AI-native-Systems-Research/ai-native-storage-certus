//! The prefix trie's **segment census** — the fitting input for a cohort mechanism.
//!
//! `stats::trunk` measures the trunk one depth at a time: width, shared width, occupancy.
//! Those are the trie's *marginals*, and they cannot distinguish "one preamble shared by
//! 5922 sessions" from "5922 sessions spread over 603 unrelated roots". This module
//! recovers the structure instead, because a trace determines it exactly.
//!
//! # Why it is exact
//!
//! The corpus declares `id_semantics: rolling_prefix`, so a block id is a hash over the
//! whole prefix ending at it. Two requests carrying the same id at depth `d` therefore have
//! *identical* paths `[0..d]`, which makes `parent(path[d]) = path[d-1]` well defined and
//! consistent across every request and session. No thresholds, no clustering.
//!
//! # The unit, and what it is for
//!
//! A **segment** is a maximal chain of nodes with out-degree 1 and a constant
//! distinct-session count: an indivisible run of blocks that one cohort walks together.
//! That is what the trace is made of, what a KV cache sees (a run of blocks with one
//! fan-in, which is what an eviction hint would carry), and what the current schema cannot
//! say — `branching` is a function of depth alone, so it must fan out at depth 141 for every
//! root or for none, while measured preamble lengths are per-root and multi-modal
//! (`appworld` 23 / 3194 / 5556, `browsecompplus` 1 / 141 / 939).
//!
//! Two quantities per split matter for generation, and the second is the one a
//! shared-width profile loses:
//!
//! * **fan-in** of each child — how the cohort subdivides, which is what decides how deep
//!   sharing runs; and
//! * **total out-degree**, including children only one session reached. Privacy comes from
//!   there: a split with 4739 children of which 483 are shared means a session can land on a
//!   singleton child and be alone from that depth on. Fitting only the shared subtrie's
//!   width made a generated corpus in which *nothing* was private, against traces where 95%
//!   of nodes are.
//!
//! # Sessions must arrive contiguously
//!
//! [`Census::observe`] counts distinct sessions with a last-seen marker rather than a set
//! per node, which is exact only if all of one session's paths arrive together. The caller
//! must group by session; `Census::finish` cannot detect a violation, so this is stated
//! rather than checked. A set per node would cost more than the whole census.
//!
//! The Python implementation in `research/segment_census.py` follows the same rules
//! deliberately, so the two can be cross-checked on a real trace — as the rolling-prefix
//! counts were, to the reference.

use serde::{Deserialize, Serialize};

use crate::keys::{CacheKey, SessionId};
use crate::stats::FastMap;

/// No parent: this node is a root.
const NO_PARENT: u32 = u32::MAX;

/// The trie under construction, in parallel arrays indexed by a dense node id.
///
/// Arrays rather than a struct per node because a real trace has millions of distinct keys
/// — `swe_agent` has 71.7M — and the four integers a node needs cost 16 bytes where a
/// hashed entry per field would cost several times that. Measured against `fit`'s existing
/// footprint the whole census is about 1% : that footprint is **reference**-proportional at
/// ~45 B/reference for the exact reuse-distance chain, not key-proportional.
#[derive(Debug, Default)]
pub struct Census {
    index: FastMap<CacheKey, u32>,
    parent: Vec<u32>,
    depth: Vec<u32>,
    sessions: Vec<u32>,
    last_session: Vec<u32>,
    /// Keys seen at two different depths or under two different parents.
    ///
    /// Impossible under `rolling_prefix`, so a non-zero count means the trace contradicts
    /// its own manifest. Counted rather than repaired.
    violations: u64,
}

impl Census {
    /// An empty census.
    pub fn new() -> Census {
        Census::default()
    }

    /// Record one request's path. **Call grouped by session** (see the module docs).
    pub fn observe(&mut self, session: SessionId, path: &[CacheKey]) {
        let mut parent = NO_PARENT;
        for (d, key) in path.iter().enumerate() {
            let d = d as u32;
            let node = match self.index.get(key) {
                Some(n) => {
                    let n = *n;
                    if self.depth[n as usize] != d || self.parent[n as usize] != parent {
                        self.violations += 1;
                    }
                    if self.last_session[n as usize] != session.0 {
                        self.last_session[n as usize] = session.0;
                        self.sessions[n as usize] += 1;
                    }
                    n
                }
                None => {
                    let n = self.parent.len() as u32;
                    self.index.insert(*key, n);
                    self.parent.push(parent);
                    self.depth.push(d);
                    self.sessions.push(1);
                    self.last_session.push(session.0);
                    n
                }
            };
            parent = node;
        }
    }

    /// Distinct keys seen.
    pub fn nodes(&self) -> usize {
        self.parent.len()
    }

    /// Keys contradicting `rolling_prefix` identity.
    pub fn violations(&self) -> u64 {
        self.violations
    }

    /// Every segment whose cohort is at least `min_sessions`, plus the split that ends it.
    ///
    /// One pass to count children, one to link them, one to walk — each node belongs to
    /// exactly one segment, so the walk is linear.
    pub fn finish(&self, min_sessions: u32) -> Vec<SegmentRow> {
        let n = self.parent.len();
        let mut children = vec![0u32; n];
        for p in &self.parent {
            if *p != NO_PARENT {
                children[*p as usize] += 1;
            }
        }
        // First-child / next-sibling, so a split's child fan-ins can be read without a
        // per-node vector.
        let mut first = vec![NO_PARENT; n];
        let mut next = vec![NO_PARENT; n];
        for node in (0..n).rev() {
            let p = self.parent[node];
            if p != NO_PARENT {
                next[node] = first[p as usize];
                first[p as usize] = node as u32;
            }
        }

        let mut out = Vec::new();
        for node in 0..n {
            let p = self.parent[node];
            // A node continues its parent's segment when the parent has exactly one child
            // and the cohort did not change across the step.
            if p != NO_PARENT
                && children[p as usize] == 1
                && self.sessions[p as usize] == self.sessions[node]
            {
                continue;
            }
            let mut length = 1u32;
            let mut cur = node;
            let ends = loop {
                if children[cur] == 0 {
                    break SegmentEnd::Leaf;
                }
                if children[cur] > 1 {
                    break SegmentEnd::Fanout;
                }
                let child = first[cur] as usize;
                if self.sessions[child] != self.sessions[cur] {
                    break SegmentEnd::Attrition;
                }
                cur = child;
                length += 1;
            };
            if self.sessions[node] < min_sessions {
                continue;
            }
            let mut row = SegmentRow {
                start_depth: self.depth[node],
                length,
                fan_in: self.sessions[node],
                ends,
                out_degree: children[cur],
                shared_children: 0,
                child_fan_in_sum: 0,
                shared_fan_in_sum: 0,
                child_fan_in_sq: 0.0,
                child_fan_in_sq_all: 0.0,
                top_child_fan_in: 0,
            };
            if ends == SegmentEnd::Fanout {
                let mut c = first[cur];
                while c != NO_PARENT {
                    let f = self.sessions[c as usize];
                    row.child_fan_in_sum += u64::from(f);
                    row.child_fan_in_sq_all += f64::from(f) * f64::from(f);
                    if f >= min_sessions {
                        row.shared_children += 1;
                        row.shared_fan_in_sum += u64::from(f);
                        row.child_fan_in_sq += f64::from(f) * f64::from(f);
                        row.top_child_fan_in = row.top_child_fan_in.max(f);
                    }
                    c = next[c as usize];
                }
            }
            out.push(row);
        }
        out
    }
}

/// How a segment stops.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum SegmentEnd {
    /// The last node has more than one child: a genuine cohort **split**.
    Fanout,
    /// Out-degree 1 but a smaller cohort below — sessions ended *on the trunk*.
    ///
    /// Measured to be near-absent: leakage at a split has a median of exactly 0.000 in
    /// every depth band of six traces, because sessions retire in their **private tails**,
    /// which are 95%+ of all nodes. So shared width falls by exhaustion through
    /// subdivision, not by retirement — which is why a design that sheds sessions off the
    /// trunk with a per-node coin flip was measured three times worse.
    Attrition,
    /// Nothing below.
    Leaf,
}

/// One segment of the trie.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SegmentRow {
    /// Depth of the segment's first node.
    pub start_depth: u32,
    /// Nodes in the unary run.
    pub length: u32,
    /// Distinct sessions walking it — the cohort.
    pub fan_in: u32,
    /// What ended it.
    pub ends: SegmentEnd,
    /// Children at the ending node, **including single-session ones**.
    ///
    /// The total, not the shared count, because privacy is generated from the difference:
    /// a session landing on a singleton child is alone from there on.
    pub out_degree: u32,
    /// Children at least `min_sessions` sessions reached.
    pub shared_children: u32,
    /// Summed fan-in over **all** children. Below `fan_in` by the sessions that ended here.
    pub child_fan_in_sum: u64,
    /// Summed fan-in over the **shared** children only, the numerator of `n_eff`.
    pub shared_fan_in_sum: u64,
    /// Summed squared fan-in over shared children, for `n_eff`.
    pub child_fan_in_sq: f64,
    /// Summed squared fan-in over **all** children, for [`SegmentRow::collision`].
    ///
    /// Separate from `child_fan_in_sq` rather than replacing it because the two answer
    /// different questions and both are wanted: `n_eff` over shared children describes how
    /// wide the *shared* subtrie is below the split, and is the figure cross-validated
    /// against `research/segment_census.py`, while the child law the generator applies
    /// chooses among the **total** out-degree — singleton children included, since landing on
    /// one is how a session becomes private.
    pub child_fan_in_sq_all: f64,
    /// The most-taken child's fan-in.
    pub top_child_fan_in: u32,
}

impl SegmentRow {
    /// Effective branching at the split: `(Sum c)^2 / Sum c^2` over shared children.
    ///
    /// The inverse participation ratio, and the **only** functional of the child-choice law
    /// that `corpus::occupancy`, validation rule 16 and `branching: auto` depend on — so it
    /// is the scalar worth fitting even where no rank law describes the shape. Measured,
    /// Zipf fails the two largest fanouts in the corpus in *opposite* directions and
    /// `ragbench`'s 2498 deep splits are exactly uniform, so a shape fit does not transfer
    /// and this does.
    pub fn n_eff(&self) -> Option<f64> {
        if self.shared_children < 2 || self.child_fan_in_sq <= 0.0 {
            return None;
        }
        let sum = self.shared_fan_in_sum as f64;
        Some(sum * sum / self.child_fan_in_sq)
    }

    /// The **collision probability** at this split: `Σ c² / (Σ c)²` over all children.
    ///
    /// The probability that two sessions arriving here descend into the same child, and so —
    /// read the other way — the factor by which a session's own cohort shrinks in
    /// expectation as it takes the step. That is literally the generator's arithmetic:
    /// `plan::generate` carries `cohort *= p(child taken)` and draws the child from `p`, so
    /// the expected factor is `Σ p²`. Fitting a child law to this scalar therefore fits the
    /// only thing about the law the cohort mechanism can observe.
    ///
    /// Over **all** children, matching the generator, which chooses among the total
    /// out-degree. The denominator is the summed child fan-in rather than the segment's own
    /// `fan_in`, so a session that retired *at* the split is excluded from the law it never
    /// exercised; measured leakage is a median of exactly 0.000 anyway.
    ///
    /// `None` where there is no choice to describe: a segment that ended in a leaf or in
    /// attrition, or a split with a single child.
    pub fn collision(&self) -> Option<f64> {
        if self.ends != SegmentEnd::Fanout || self.out_degree < 2 || self.child_fan_in_sum == 0 {
            return None;
        }
        let sum = self.child_fan_in_sum as f64;
        Some(self.child_fan_in_sq_all / (sum * sum))
    }

    /// `n_eff` as a fraction of the shared children it is taken over.
    ///
    /// 1.0 is uniform descent; below 1.0 is skew. Measured descent-weighted: qwen_code
    /// 0.510, ragbench 0.544, tau2_retail 0.722, tau2_airline 0.805 — so uniform descent
    /// overstates effective branching by 1.24x to 1.96x.
    pub fn n_eff_frac(&self) -> Option<f64> {
        self.n_eff().map(|e| e / f64::from(self.shared_children))
    }

    /// Fraction of the cohort that **ended** at this split rather than descending.
    ///
    /// `1 - sum(child fan-in) / fan_in`. Measured median exactly 0.000 across six traces
    /// (session-weighted at most 0.060), which is what established that shared width falls
    /// by subdivision rather than retirement. A *negative* value means sessions whose
    /// invocations fork below the node, so the children's fan-ins sum to more than the
    /// parent's — real, small, and outside FR-019a's one-path-per-session assumption.
    pub fn leak(&self) -> f64 {
        if self.fan_in == 0 || self.ends != SegmentEnd::Fanout {
            return 0.0;
        }
        1.0 - (self.child_fan_in_sum as f64 / f64::from(self.fan_in))
    }
}

/// Depth bands a fitted [`crate::schema::SegmentProcess`] is banded by.
///
/// Geometric, because the structure plainly varies with depth — out-degree 4739 at depth 0
/// against 2 at depth 210 on `qwen_code` — and a linear banding would put every interesting
/// thing in one bucket. The same bands `research/segment_census.py` reports, so a fitted
/// document can be read against that table directly.
///
/// Public so that the census `fit --explain` prints is banded by these rather than by a
/// second copy of the same list: the two tables are read against each other row by row, and a
/// copy that drifted would misattribute every fitted law by one band.
pub const BANDS: [u32; 6] = [0, 1, 8, 32, 128, 512];

/// Fit a node-level trunk process from the census.
///
/// Per band, the distribution of a split's **run length** and of its **total out-degree**,
/// both as empirical step distributions through the same builder `fit::sessions` uses — so
/// the step encoding gets the same care that a readability budget standing in for an
/// accuracy one previously cost us twice.
///
/// # Weighted by fan-in, deliberately
///
/// A walker draws a run length *at a split it has arrived at*, and a split holding 5000
/// sessions is arrived at by 5000 sessions while one holding 2 is arrived at by 2. So the
/// distribution a session experiences is the fan-in-weighted one, not the per-segment one.
/// This matters here more than it usually would: measured, the shared region is numerically
/// dominated by tiny short-lived cohorts (fan-in median 2-3 in the deep bands) while the
/// reference mass sits in a handful of big segments, so the unweighted and weighted
/// distributions are very different objects. Weight the fit the way the statistic is
/// weighted, or `unique_keys` and reuse distance pull in opposite directions.
pub fn fit_process(rows: &[SegmentRow]) -> Option<ProcessFit> {
    use crate::schema::{SegmentBand, SegmentProcess};

    let mut bands: Vec<SegmentBand> = Vec::new();
    let mut skews: Vec<BandSkew> = Vec::new();
    for (i, lo) in BANDS.iter().enumerate() {
        let hi = BANDS.get(i + 1).copied().unwrap_or(u32::MAX);
        let in_band = || {
            rows.iter()
                .filter(move |r| r.start_depth >= *lo && (hi == u32::MAX || r.start_depth < hi))
        };
        let length =
            weighted_empirical(in_band().map(|r| (u64::from(r.length), u64::from(r.fan_in))));
        // Out-degree is only defined where a split ended the segment; a leaf has none, and
        // an attrition boundary is a cohort shrinking rather than dividing.
        let out_degree = weighted_empirical(
            in_band()
                .filter(|r| r.ends == SegmentEnd::Fanout)
                .map(|r| (u64::from(r.out_degree), u64::from(r.fan_in))),
        );
        if let (Some(length), Some(out_degree)) = (length, out_degree) {
            let skew = fit_skew(in_band(), &out_degree);
            bands.push(SegmentBand {
                from_depth: *lo,
                length,
                out_degree,
                skew: skew.as_ref().map(|s| s.skew),
            });
            if let Some(mut s) = skew {
                s.from_depth = *lo;
                skews.push(s);
            }
        }
    }
    if bands.is_empty() {
        return None;
    }
    // Schema rule 8 requires the first band at depth 0. If the shallowest band with any
    // measured split is deeper, its distributions are the best evidence there is for the
    // depths above it — stating them from 0 is honest, whereas omitting the band would leave
    // the document unable to describe its own root layer.
    bands[0].from_depth = 0;
    if let Some(first) = skews.first_mut() {
        first.from_depth = 0;
    }
    Some(ProcessFit {
        process: SegmentProcess { by_depth: bands },
        skews,
    })
}

/// A fitted trunk process together with what its child-law fit achieved.
///
/// The diagnostics travel beside the document rather than inside it because they are
/// evidence about the fit, not parameters of the workload: `fit --explain` prints them and
/// nothing generates from them.
#[derive(Debug, Clone)]
pub struct ProcessFit {
    /// The document's `branching` section.
    pub process: crate::schema::SegmentProcess,
    /// One entry per band whose child law could be fitted, ascending by depth.
    pub skews: Vec<BandSkew>,
}

impl ProcessFit {
    /// What each fitted band implies about how fast a cohort decays with depth.
    ///
    /// The band's two fitted halves decide sharing only jointly, and neither alone is
    /// readable: a run length says how *often* a walker splits and the child law says how much
    /// of its cohort it keeps at each split, so the quantity that decides where sharing ends is
    /// `collision ^ (1/mean length)` per block. Reported because the emitted document states
    /// the two separately and FR-057 may refuse to write it at all, which leaves the fit's own
    /// consequence unreadable at exactly the moment it is being diagnosed.
    ///
    /// One entry per band that has both halves, ascending by depth. A band whose child law was
    /// not fitted is omitted rather than defaulted, since the document-level `branch_skew` that
    /// such a band falls back to is not this fit's statement.
    ///
    /// Each band's decay is taken over **its own span**, never over a fixed depth: a band 24
    /// blocks wide and one 384 blocks wide contribute different amounts of subdivision at the
    /// same per-split rate, so a common yardstick would overstate the narrow bands by more than
    /// an order of magnitude. `cumulative` is the running product down the trunk, and is
    /// **`None` from the first band that stated no law onwards** rather than multiplying across
    /// the hole, because a product missing a factor reads as a smaller number, not as a gap.
    pub fn implied(&self) -> Vec<BandImplied> {
        let bands = &self.process.by_depth;
        let mut out: Vec<BandImplied> = Vec::new();
        let mut cumulative = Some(1.0f64);
        for (i, b) in bands.iter().enumerate() {
            // The last band is unbounded: the profile applies it to every depth below its start,
            // so there is no span to integrate over and no honest end to the product.
            let span_blocks = bands
                .get(i + 1)
                .map(|next| next.from_depth.saturating_sub(b.from_depth));
            let law = self.skews.iter().find(|s| s.from_depth == b.from_depth);
            let mean_length = b.length.mean().filter(|m| *m > 0.0);
            let (Some(law), Some(mean_length)) = (law, mean_length) else {
                // Any gap voids every later cumulative figure, not just this row's.
                cumulative = None;
                continue;
            };
            // The realised collision rather than the target: `achieved` is measured over
            // out-degrees drawn from the distribution the document states, so it carries the
            // emitted empirical's coarsening, which is the whole reason both are kept.
            let splits_per_block = 1.0 / mean_length;
            let row = BandImplied {
                from_depth: b.from_depth,
                span_blocks,
                mean_length,
                collision: law.achieved,
                splits_per_block,
                decay_in_band: None,
                cumulative: None,
            };
            let decay_in_band = span_blocks.map(|s| row.decay_over(f64::from(s)));
            cumulative = match (cumulative, decay_in_band) {
                (Some(c), Some(d)) => Some(c * d),
                // An unbounded final band has no end to accumulate to, so the product stops at
                // the band above it rather than being extended by a guess.
                _ => None,
            };
            out.push(BandImplied {
                decay_in_band,
                cumulative,
                ..row
            });
        }
        out
    }
}

/// What one fitted band implies about cohort decay, derived from both of its halves.
#[derive(Debug, Clone, Copy)]
pub struct BandImplied {
    /// The band this describes.
    pub from_depth: u32,
    /// Blocks this band covers. `None` for the last band, which is unbounded.
    pub span_blocks: Option<u32>,
    /// Mean run length, the renewal rate's reciprocal.
    ///
    /// The **mean**, because the number of splits over a depth is set by the renewal rate and a
    /// run-length distribution measured on this corpus is heavily skewed: `tau2_airline`'s band
    /// at depths 128-511 has a median of 119 and a mean of 71, and its census median is 1
    /// against a p90 of 161. Reading a split rate off a median overstates it by orders of
    /// magnitude, which was measured and recorded as a wrong diagnosis on 2026-08-17.
    pub mean_length: f64,
    /// The collision probability the band's child law realises at one split.
    pub collision: f64,
    /// Splits a walker meets per block of trunk, `1/mean_length`.
    pub splits_per_block: f64,
    /// Expected cohort factor across this band's own span. `None` for an unbounded last band.
    pub decay_in_band: Option<f64>,
    /// Expected cohort factor from depth 0 through the end of this band.
    ///
    /// `None` once any band above stated no law, and for an unbounded final band.
    pub cumulative: Option<f64>,
}

impl BandImplied {
    /// The expected factor a cohort shrinks by over `blocks` blocks of trunk in this band.
    ///
    /// `collision ^ (blocks / mean_length)`: one factor of the collision probability per split,
    /// and `blocks / mean_length` splits. An expectation over a product of independent draws,
    /// so it describes the mean cohort rather than any one walker's.
    pub fn decay_over(&self, blocks: f64) -> f64 {
        self.collision.powf(blocks * self.splits_per_block)
    }
}

/// The widest child law the fit will state.
///
/// At `s = 8` a two-way split already sends 99.6% of a cohort down one child, so the band
/// above this is a law nothing distinguishes; a bound also keeps the bisection finite when a
/// band's measured collision probability is unreachable under any Zipf.
const SKEW_MAX: f64 = 8.0;

/// Bisection steps. `SKEW_MAX / 2^40` is far below any exponent's meaning.
const SKEW_STEPS: usize = 40;

/// Out-degrees drawn from the emitted distribution when checking what the fit achieves.
const SKEW_CHECK_DRAWS: usize = 2048;

/// Seed for that check. Fixed, so a fit is reproducible: this is a diagnostic about the
/// emitted distribution, not a draw the workload depends on.
const TAG_SKEW_CHECK: u64 = 0x5CE7_C4EC;

/// What fitting one band's child law found.
#[derive(Debug, Clone)]
pub struct BandSkew {
    /// The band this describes.
    pub from_depth: u32,
    /// The fitted Zipf exponent over child rank.
    pub skew: f64,
    /// The band's measured collision probability, fan-in weighted.
    pub target: f64,
    /// What the fitted law realises over out-degrees drawn from the emitted distribution.
    ///
    /// The bisection converges on `target` over the *measured* out-degrees to well below any
    /// meaningful precision, so a gap here is the emitted empirical's step coarsening rather
    /// than a failure of the solve — which is why it is worth printing separately.
    pub achieved: f64,
    /// Set when the target was outside what any exponent in `[0, SKEW_MAX]` can reach.
    pub clamped: Option<&'static str>,
    /// Splits the target was measured over.
    pub splits: usize,
}

/// Fit one band's child-choice law from its splits.
///
/// The target is the **fan-in-weighted mean collision probability** over the band's splits
/// (see [`SegmentRow::collision`]), weighted by fan-in for the same reason the run length and
/// out-degree are: a walker experiences a split in proportion to the sessions arriving at it,
/// and measured, the shared region is numerically dominated by tiny cohorts while the
/// reference mass sits in a handful of large segments.
///
/// A single exponent per band, not per split. Within a band the collision probability varies
/// and one `s` cannot match every split; what it matches is the mean, which is exactly the
/// quantity `cohort *= p` accumulates. Conditioning on out-degree instead of depth is the
/// obvious alternative and `fit --explain` prints the within-band spread so the question can
/// be settled on the corpus rather than assumed.
fn fit_skew<'a>(
    rows: impl Iterator<Item = &'a SegmentRow>,
    out_degree: &crate::dist::Dist,
) -> Option<BandSkew> {
    // Weighted by fan-in, deduplicated by out-degree so that the harmonic pass below is over
    // the widest split rather than over every split.
    let mut by_degree: FastMap<u64, f64> = FastMap::default();
    let mut splits = 0usize;
    let mut num = 0.0f64;
    let mut den = 0.0f64;
    for r in rows {
        let Some(c) = r.collision() else { continue };
        let w = f64::from(r.fan_in).max(1.0);
        *by_degree.entry(u64::from(r.out_degree)).or_insert(0.0) += w;
        num += w * c;
        den += w;
        splits += 1;
    }
    if den <= 0.0 || splits == 0 {
        return None;
    }
    let target = num / den;
    let mut pairs: Vec<(u64, f64)> = by_degree.into_iter().collect();
    pairs.sort_unstable_by_key(|(n, _)| *n);

    // Monotone increasing in `s`, so the endpoints decide whether a solution exists at all.
    let at = |s: f64| weighted_collision(s, &pairs);
    let (flat, steep) = (at(0.0)?, at(SKEW_MAX)?);
    let mut clamped = None;
    let skew = if target <= flat {
        // More even than uniform descent — real, and measured: `ragbench`'s deep splits sit
        // at 0.95x uniform, i.e. sub-multinomial. No Zipf is flatter than uniform, so the
        // honest statement is uniform and a note that the trace is flatter still.
        clamped = Some("target is at or below uniform descent; stated as uniform");
        0.0
    } else if target >= steep {
        clamped = Some("target is more concentrated than the widest law fit will state");
        SKEW_MAX
    } else {
        let mut lo = 0.0f64;
        let mut hi = SKEW_MAX;
        for _ in 0..SKEW_STEPS {
            let mid = 0.5 * (lo + hi);
            match at(mid) {
                Some(c) if c < target => lo = mid,
                Some(_) => hi = mid,
                None => return None,
            }
        }
        0.5 * (lo + hi)
    };

    // What the law will actually realise, over out-degrees drawn from the distribution the
    // document states rather than over the ones measured — so the ≤64-step coarsening of the
    // emitted empirical is visible rather than assumed away.
    let mut st = crate::rng::Stream::new(TAG_SKEW_CHECK, 0);
    let mut drawn: FastMap<u64, f64> = FastMap::default();
    for _ in 0..SKEW_CHECK_DRAWS {
        let n = out_degree.sample_u64(&mut st).max(1);
        *drawn.entry(n).or_insert(0.0) += 1.0;
    }
    let mut drawn: Vec<(u64, f64)> = drawn.into_iter().collect();
    drawn.sort_unstable_by_key(|(n, _)| *n);
    let achieved = weighted_collision(skew, &drawn).unwrap_or(f64::NAN);

    Some(BandSkew {
        from_depth: 0,
        skew,
        target,
        achieved,
        clamped,
        splits,
    })
}

/// The weighted mean collision probability at `s` over `(out_degree, weight)` pairs.
///
/// `pairs` must be ascending by out-degree. One pass over ranks `1..=max`, accumulating both
/// harmonic sums and reading each pair's value off the running totals, so the cost is the
/// **widest** split rather than the sum over splits — which matters because the widest
/// measured fanout in the corpus is 204030 and a per-pair sum would be quadratic in it.
///
/// `None` if any out-degree is above the support at which the sampler abandons the exact
/// discrete law, matching [`crate::dist::zipf_collision`]: fitting an exponent against a law
/// the draw is not using would be worse than declining.
fn weighted_collision(s: f64, pairs: &[(u64, f64)]) -> Option<f64> {
    let max = pairs.last()?.0;
    let mut num = 0.0f64;
    let mut den = 0.0f64;
    if s <= 0.0 {
        for (n, w) in pairs {
            let c = crate::dist::zipf_collision(0.0, *n)?;
            num += w * c;
            den += w;
        }
        return if den > 0.0 { Some(num / den) } else { None };
    }
    if max > crate::dist::ZIPF_EXACT_MAX_SUPPORT {
        return None;
    }
    let mut h_s = 0.0f64;
    let mut h_2s = 0.0f64;
    let mut next = 0usize;
    for k in 1..=max {
        let t = (k as f64).powf(-s);
        h_s += t;
        h_2s += t * t;
        while next < pairs.len() && pairs[next].0 == k {
            if h_s > 0.0 {
                num += pairs[next].1 * h_2s / (h_s * h_s);
                den += pairs[next].1;
            }
            next += 1;
        }
    }
    if den > 0.0 {
        Some(num / den)
    } else {
        None
    }
}

/// An empirical step distribution over `(value, weight)` pairs.
fn weighted_empirical(obs: impl Iterator<Item = (u64, u64)>) -> Option<crate::dist::Dist> {
    let mut pairs: Vec<(u64, u64)> = obs.collect();
    if pairs.is_empty() {
        return None;
    }
    pairs.sort_unstable();
    // Merge equal values, since the builder expects one bucket per distinct value.
    let mut buckets: Vec<(u64, u64, u64)> = Vec::new();
    for (v, w) in pairs {
        match buckets.last_mut() {
            Some((lo, _, c)) if *lo == v => *c += w.max(1),
            _ => buckets.push((v, v, w.max(1))),
        }
    }
    super::sessions::empirical_from_buckets(&buckets)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A census over requests given as `(session, path)`, fed **grouped by session**.
    fn census_of(requests: &[(u32, Vec<u64>)]) -> Census {
        let mut c = Census::new();
        let mut sessions: Vec<u32> = requests.iter().map(|(s, _)| *s).collect();
        sessions.sort_unstable();
        sessions.dedup();
        for s in sessions {
            for (sess, path) in requests.iter().filter(|(x, _)| *x == s) {
                let keys: Vec<CacheKey> = path.iter().map(|k| CacheKey(*k)).collect();
                c.observe(SessionId(*sess), &keys);
            }
        }
        c
    }

    #[test]
    fn a_preamble_then_a_split_is_one_segment_and_its_children_are_measured() {
        // The shape every regime in the corpus has: a run all sessions walk, then a split.
        // Four sessions share three blocks, then two take one child and two another.
        let reqs = vec![
            (1, vec![1, 2, 3, 10, 11]),
            (2, vec![1, 2, 3, 10, 12]),
            (3, vec![1, 2, 3, 20, 21]),
            (4, vec![1, 2, 3, 20, 22]),
        ];
        let rows = census_of(&reqs).finish(2);
        let head = rows
            .iter()
            .find(|r| r.start_depth == 0)
            .expect("a segment at the root");
        assert_eq!(head.length, 3, "the shared run is 3 blocks");
        assert_eq!(head.fan_in, 4, "all four sessions walk it");
        assert_eq!(head.ends, SegmentEnd::Fanout);
        assert_eq!(head.out_degree, 2);
        assert_eq!(head.shared_children, 2);
        assert_eq!(head.child_fan_in_sum, 4, "both children carry two sessions");
        assert!(head.leak().abs() < 1e-12, "nobody retired at the split");
        // Two equal children: n_eff is exactly 2, i.e. uniform.
        let e = head.n_eff().expect("two shared children");
        assert!((e - 2.0).abs() < 1e-9, "n_eff {e}");
        assert!((head.n_eff_frac().unwrap() - 1.0).abs() < 1e-9);
    }

    #[test]
    fn total_out_degree_counts_singleton_children_because_privacy_comes_from_them() {
        // The measurement a shared-width profile loses, and the reason the generated
        // corpus had no private keys at all: at this split 2 of 3 children are reached by
        // one session each, so a session landing there is alone from that depth on. A
        // profile fitted to shared width sees out-degree 1 here and can never diverge.
        let reqs = vec![
            (1, vec![1, 50]),
            (2, vec![1, 50]),
            (3, vec![1, 60]),
            (4, vec![1, 70]),
        ];
        let rows = census_of(&reqs).finish(2);
        let head = rows.iter().find(|r| r.start_depth == 0).expect("root");
        assert_eq!(head.out_degree, 3, "three children exist");
        assert_eq!(head.shared_children, 1, "only one of them is shared");
        assert_eq!(
            head.child_fan_in_sum, 4,
            "the singletons still count toward the cohort's descent"
        );
    }

    #[test]
    fn a_session_ending_on_the_trunk_shows_up_as_leakage_not_as_a_shorter_segment() {
        // Retirement is measured where it happens. Session 3's path STOPS at depth 1
        // while 1 and 2 continue, so the split at depth 1 carries three sessions and its
        // children carry two: leak = 1/3.
        let reqs = vec![(1, vec![1, 2, 30]), (2, vec![1, 2, 40]), (3, vec![1, 2])];
        let rows = census_of(&reqs).finish(2);
        let seg = rows.iter().find(|r| r.start_depth == 0).expect("root");
        assert_eq!(seg.fan_in, 3);
        assert_eq!(seg.ends, SegmentEnd::Fanout);
        assert_eq!(seg.child_fan_in_sum, 2, "only two sessions descended");
        assert!(
            (seg.leak() - 1.0 / 3.0).abs() < 1e-9,
            "leak was {}",
            seg.leak()
        );
    }

    /// One split with the given child fan-ins, as a row at depth 0.
    ///
    /// Built directly rather than through a census so a test can state a child law exactly:
    /// realising a planted exponent through sessions would need thousands of them and would
    /// test the sampler rather than the inversion.
    fn split_row(fan_ins: &[u32]) -> SegmentRow {
        let sum: u64 = fan_ins.iter().map(|f| u64::from(*f)).sum();
        SegmentRow {
            start_depth: 0,
            length: 1,
            fan_in: sum as u32,
            ends: SegmentEnd::Fanout,
            out_degree: fan_ins.len() as u32,
            shared_children: fan_ins.iter().filter(|f| **f >= 2).count() as u32,
            child_fan_in_sum: sum,
            shared_fan_in_sum: fan_ins
                .iter()
                .filter(|f| **f >= 2)
                .map(|f| u64::from(*f))
                .sum(),
            child_fan_in_sq: 0.0,
            child_fan_in_sq_all: fan_ins.iter().map(|f| f64::from(*f) * f64::from(*f)).sum(),
            top_child_fan_in: fan_ins.iter().copied().max().unwrap_or(0),
        }
    }

    #[test]
    fn the_fitted_child_law_reproduces_the_collision_probability_it_was_fitted_to() {
        // The inversion, on a planted law: child fan-ins proportional to k^-1.5 over eight
        // children, so the split's collision probability is the one a Zipf at s = 1.5
        // realises and the fit must recover that exponent.
        //
        // This is the parameter whose absence made `--branching-segments` unusable:
        // `out_degree` and the child law are a pair, and a measured out-degree with the
        // document-level default put 0.072 on a head the trace gives 0.496.
        let n = 8u64;
        let total = 100_000.0f64;
        let h: f64 = (1..=n).map(|k| (k as f64).powf(-1.5)).sum();
        let fan_ins: Vec<u32> = (1..=n)
            .map(|k| ((k as f64).powf(-1.5) / h * total).round() as u32)
            .collect();
        let row = split_row(&fan_ins);
        let want = crate::dist::zipf_collision(1.5, n).unwrap();
        assert!(
            (row.collision().unwrap() - want).abs() < 1e-4,
            "the fixture's own collision is {} against zipf(1.5)'s {want}",
            row.collision().unwrap()
        );
        let fit = fit_process(&[row]).expect("a process");
        let band = &fit.skews[0];
        assert!(
            (band.skew - 1.5).abs() < 0.01,
            "planted s = 1.5, fitted {}",
            band.skew
        );
        assert!(
            (band.achieved - band.target).abs() < 1e-6,
            "a const out-degree cannot be coarsened, so achieved {} must equal target {}",
            band.achieved,
            band.target
        );
        assert!(band.clamped.is_none());
        assert_eq!(
            fit.process.by_depth[0].skew,
            Some(band.skew),
            "the fitted law must reach the document, not only the report"
        );
    }

    #[test]
    fn a_split_no_more_concentrated_than_uniform_is_stated_as_uniform_and_says_so() {
        // Measured, `ragbench`'s 2498 deep splits are EXACTLY uniform — literally equal child
        // counts — and some traces are flatter still (0.95x uniform, sub-multinomial). No Zipf
        // is flatter than uniform, so the honest emission is uniform plus a note; silently
        // fitting the nearest exponent would state a concentration the trace contradicts.
        let fit = fit_process(&[split_row(&[25, 25, 25, 25])]).expect("a process");
        let band = &fit.skews[0];
        assert_eq!(band.skew, 0.0, "uniform descent is skew 0");
        assert!(
            band.clamped.is_some_and(|c| c.contains("uniform")),
            "the clamp must be reported: {:?}",
            band.clamped
        );
        assert!((band.target - 0.25).abs() < 1e-12, "four equal children");
    }

    #[test]
    fn a_split_more_concentrated_than_any_stated_law_clamps_and_says_so() {
        // A cohort of 1001 where 1000 take one child: collision 0.998, above what s = 8
        // reaches at a 2-way split (0.992). Clamping is the right answer — beyond this the
        // exponent is unidentifiable, every value sending essentially the whole cohort one way
        // — but it must be visible, because a clamped band is a band whose realised sharing
        // will be slightly shallower than the trace's.
        let fit = fit_process(&[split_row(&[1000, 1])]).expect("a process");
        let band = &fit.skews[0];
        assert_eq!(band.skew, SKEW_MAX);
        assert!(
            band.clamped.is_some_and(|c| c.contains("concentrated")),
            "{:?}",
            band.clamped
        );
    }

    #[test]
    fn the_implied_decay_is_the_two_fitted_halves_multiplied_out() {
        // The point of reporting this: a run length and a child law are individually readable
        // and jointly decisive, and the joint quantity is what the residual is about — measured
        // 2026-08-17, the segments spelling mints 1.6-1.7x too many keys while per-split
        // collision matches its target to 0.4%, i.e. the cohort divides too OFTEN rather than
        // too widely, which only this product can show.
        let row = split_row(&[50, 50]);
        let fit = fit_process(&[row]).expect("a process");
        let implied = fit.implied();
        assert_eq!(implied.len(), 1, "one band was fitted, so one row");
        let b = implied[0];
        // A single band is the unbounded last one, so there is no span to integrate over and
        // both derived figures decline rather than extrapolating to a guessed depth.
        assert_eq!(b.span_blocks, None);
        assert_eq!(b.decay_in_band, None);
        assert_eq!(b.cumulative, None);
        // Two equal children is uniform descent, so the law is stated as uniform and the
        // collision is 1/2 exactly — the boundary case, which keeps this test's arithmetic
        // checkable by hand rather than against the bisection's output.
        assert!((b.collision - 0.5).abs() < 1e-12, "{}", b.collision);
        // `split_row` is one block long, so the walker splits once per block and 100 blocks of
        // trunk cost 100 halvings.
        assert!((b.mean_length - 1.0).abs() < 0.01, "{}", b.mean_length);
        assert!((b.splits_per_block - 1.0 / b.mean_length).abs() < 1e-12);
        let want = 0.5f64.powf(100.0 * b.splits_per_block);
        assert!((b.decay_over(100.0) - want).abs() < 1e-12);
        // Doubling the run length halves the split count, so the decay over a fixed depth is
        // the square root — the relationship that makes run length, not just the child law, a
        // first-class suspect for the residual.
        let slower = BandImplied {
            mean_length: 2.0 * b.mean_length,
            splits_per_block: 1.0 / (2.0 * b.mean_length),
            ..b
        };
        assert!(
            (slower.decay_over(100.0) - b.decay_over(100.0).sqrt()).abs() < 1e-12,
            "{} against {}",
            slower.decay_over(100.0),
            b.decay_over(100.0).sqrt()
        );
    }

    #[test]
    fn a_band_with_no_split_states_no_law_rather_than_a_default() {
        // A segment ending in a leaf or in attrition is a cohort that shrank rather than
        // divided, so there is no choice to describe. An absent `skew` defers to the
        // document-level `branch_skew`, which is what the schema has always meant; inventing
        // 0.9 here would look fitted and be arbitrary.
        let mut leaf = split_row(&[4, 4]);
        leaf.ends = SegmentEnd::Leaf;
        leaf.out_degree = 0;
        assert!(leaf.collision().is_none());
        // `fit_process` needs an out-degree to state a band at all, so a corpus of only leaves
        // yields no band; the check that matters is that no law is invented for one.
        assert!(fit_process(&[leaf]).is_none());
    }

    #[test]
    fn a_fitted_process_states_the_measured_run_length_and_total_out_degree() {
        // Four sessions share three blocks then split two ways, so the root band should
        // state a run of 3 and an out-degree of 2 — and the out-degree must be the TOTAL,
        // which is what a shared-width profile cannot say.
        let reqs = vec![
            (1, vec![1, 2, 3, 10, 11]),
            (2, vec![1, 2, 3, 10, 12]),
            (3, vec![1, 2, 3, 20, 21]),
            (4, vec![1, 2, 3, 20, 22]),
        ];
        let rows = census_of(&reqs).finish(2);
        let p = fit_process(&rows).expect("a process").process;
        assert_eq!(
            p.by_depth[0].from_depth, 0,
            "rule 8 wants the first band at 0"
        );
        let l = p.by_depth[0].length.quantile(0.5).expect("a median length");
        assert!((l - 3.0).abs() < 0.5, "median run length came out {l}");
        let d = p.by_depth[0]
            .out_degree
            .quantile(0.5)
            .expect("a median degree");
        assert!((d - 2.0).abs() < 0.5, "median out-degree came out {d}");
    }

    #[test]
    fn the_fit_is_weighted_by_fan_in_because_that_is_what_a_session_experiences() {
        // A walker draws a run length at a split it has ARRIVED at, so a split holding many
        // sessions is arrived at by many. Here one segment of length 2 carries 10 sessions
        // and twenty segments of length 50 carry 2 each: unweighted the median length is 50,
        // fan-in weighted it is 2, and the sessions overwhelmingly experience 2.
        //
        // This is not a nicety — measured, the shared region is numerically tiny cohorts
        // while the reference mass is a few big segments, so the two weightings are
        // different objects.
        let mut reqs: Vec<(u32, Vec<u64>)> = Vec::new();
        for s in 0..10u32 {
            // Ten sessions share a 2-block root run, then each pair goes its own way.
            reqs.push((s, vec![1, 2, 100 + u64::from(s) / 2]));
        }
        let rows = census_of(&reqs).finish(2);
        let root = rows.iter().find(|r| r.start_depth == 0).expect("root");
        assert_eq!(root.length, 2);
        assert_eq!(root.fan_in, 10);
        let p = fit_process(&rows).expect("a process").process;
        let l = p.by_depth[0].length.quantile(0.5).expect("median");
        assert!(
            (l - 2.0).abs() < 0.6,
            "the 10-session run must dominate the median, got {l}"
        );
    }

    #[test]
    fn a_key_at_two_depths_is_counted_as_a_rolling_prefix_violation() {
        // Impossible under a hash over the prefix, so it means the trace contradicts its
        // own manifest. Counted, never repaired — two corpus traces do this on 3.6% and
        // 6.7% of rows.
        let mut c = Census::new();
        c.observe(SessionId(1), &[CacheKey(1), CacheKey(7)]);
        c.observe(SessionId(2), &[CacheKey(7), CacheKey(9)]);
        assert!(
            c.violations() > 0,
            "block 7 was seen at depth 1 and depth 0"
        );
    }

    #[test]
    fn a_session_re_walking_its_own_path_counts_once_toward_fan_in() {
        // Turn n+1 re-reads every block of turn n by construction (FR-014a), so a fan-in
        // counting references rather than distinct sessions would measure conversation
        // length instead of sharing.
        let reqs = vec![
            (1, vec![1, 2]),
            (1, vec![1, 2, 3]),
            (1, vec![1, 2, 3, 4]),
            (2, vec![1, 2]),
        ];
        let rows = census_of(&reqs).finish(2);
        let seg = rows.iter().find(|r| r.start_depth == 0).expect("root");
        assert_eq!(seg.fan_in, 2, "two sessions, not four requests");
    }
}
