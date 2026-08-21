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

    /// The fraction of arrivals at this split that land on a **singleton** child.
    ///
    /// A child no other session takes is where a session becomes **private**: it walks off the
    /// shared subtrie and, under rolling-prefix identity, can never rejoin it. So this is the
    /// escape probability the trunk boundary is made of, and it is measured rather than inferred
    /// from a rank law.
    ///
    /// # Why a rank law cannot supply it, which is a correction to FR-055j
    ///
    /// FR-055j fits the child law to the collision probability and argues the tail it ignores
    /// "does not affect cohort decay". That was true while a drawn `shared_depth` was the
    /// boundary. Under cohort exhaustion it is **false**: the tail is exactly where sessions go
    /// private, so its *mass* decides when they leave. Measured on `qwen_code`, 24.8% of requests
    /// share one block or less while a Zipf matching the head's collision probability leaves only
    /// 1.3% that short — the tail keeps sessions in cohorts the trace has already scattered.
    ///
    /// `None` where there is no split to describe.
    pub fn singleton_share(&self) -> Option<f64> {
        if self.ends != SegmentEnd::Fanout || self.child_fan_in_sum == 0 {
            return None;
        }
        let total = self.child_fan_in_sum as f64;
        Some(((self.child_fan_in_sum - self.shared_fan_in_sum) as f64 / total).clamp(0.0, 1.0))
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

/// Lower edges of the **fan-in** buckets a band's run length is conditioned on (FR-054o).
///
/// Geometric and starting at 2 because a shared segment has fan-in >= 2 by definition and the
/// distribution is extreme — a median of 2 against a maximum of 16045 on `qwen_code`. Buckets by
/// **value** rather than by quantile, because fan-in is tied at 2 for most segments in most bands, so
/// quantile strata fall inside that tie group and separate nothing: an earlier version of the
/// `--explain` diagnostic partitioned by fan-in quartile and, having sorted `(fan_in, length)`, was
/// tie-broken on **length itself** and reported a 1-to-101 dependence that was length sorted against
/// itself. The failure was invisible in the output.
///
/// Public for the same reason [`BANDS`] is: `fit --explain` prints the census stratified by these and
/// the fit states its scales against them, and a second copy that drifted would misattribute every
/// fitted scale by one bucket.
pub const FAN_IN_BUCKETS: [u32; 5] = [2, 3, 4, 16, 256];

/// Fit a node-level trunk process from the census.
///
/// Per band, the distribution of a split's **run length** and of its **total out-degree**,
/// both as empirical step distributions through the same builder `fit::sessions` uses — so
/// the step encoding gets the same care that a readability budget standing in for an
/// accuracy one previously cost us twice.
///
/// # Weight a law by what its draw is KEYED ON, not by what consumes it (FR-054m)
///
/// The rule is about the key, and it splits this function's laws in two:
///
/// - **`length` is keyed on the NODE.** [`crate::corpus::ResolvedSegments::run_length`] draws
///   it once per node, from that node's own stream, precisely so every walker arriving there
///   agrees where the run ends — a run length that varied by arrival would make the trie
///   inconsistent. So the population it must reproduce is the population of **segments**, one
///   observation each, and it is fitted unweighted.
/// - **`skew` (and the escape share below) are keyed on the ARRIVAL.** A walker meets a split
///   in proportion to the sessions arriving at it and consumes the child law once per arrival,
///   so those stay fan-in weighted.
///
/// Fan-in weighting was originally applied to all of them, on the argument that a walker
/// *experiences* the weighted distribution. That argument is right about experience and wrong
/// about construction, and the error is measurable: on `qwen_code`'s root band the fan-in
/// weighted median run length is **1** where the per-segment median is **29**, because the
/// splits carrying thousands of sessions are the short ones — so the weighting pulled the law
/// short and the walk faithfully drew it short at every node. `tau2_airline` confirms it from
/// the other side: its root band's two medians agree at 124 and its realised preamble is 124
/// exactly, while the bands whose medians disagree are exactly the ones whose realised lengths
/// were wrong. See research.md § The child-choice law.
///
/// `out_degree` has the same defect (`qwen_code` band 0 states 4739 children against a
/// per-node median of 2) and is deliberately **left weighted here**: it is fitted as a pair
/// with `skew` under FR-055j, and moving one without re-fitting the other would break that
/// pairing.
pub fn fit_process(rows: &[SegmentRow]) -> Option<ProcessFit> {
    use crate::schema::{SegmentBand, SegmentProcess};

    let mut bands: Vec<SegmentBand> = Vec::new();
    let mut skews: Vec<BandSkew> = Vec::new();
    // Six depth bands, not one. Pooling every depth into a single band beat this on two traces
    // and was therefore run across the corpus (FR-055f), where it did NOT generalise: better on
    // `sharing_depth` and `request_length`, worse on `unique_keys` and on reuse distance
    // (browsecompplus 0.038 -> 0.072, swebench 0.044 -> 0.073), with coverage unchanged at 8 of
    // 24 and nothing inside tolerance in either arm. The experiment toggle is gone; see
    // research.md § The child-choice law.
    for (i, lo) in BANDS.iter().enumerate() {
        let hi = BANDS.get(i + 1).copied().unwrap_or(u32::MAX);
        let in_band = || {
            rows.iter()
                .filter(move |r| r.start_depth >= *lo && (hi == u32::MAX || r.start_depth < hi))
        };
        // Unweighted — one observation per segment, because the draw is keyed on the node.
        // See the FR-054m section above; this is not an oversight beside the weighted laws.
        let length = weighted_empirical(in_band().map(|r| (u64::from(r.length), 1)));
        // Out-degree is only defined where a split ended the segment; a leaf has none, and
        // an attrition boundary is a cohort shrinking rather than dividing.
        let out_degree = weighted_empirical(
            in_band()
                .filter(|r| r.ends == SegmentEnd::Fanout)
                .map(|r| (u64::from(r.out_degree), u64::from(r.fan_in))),
        );
        if let (Some(length), Some(out_degree)) = (length, out_degree) {
            let skew = fit_skew(in_band(), &out_degree);
            // Fan-in weighted, for the same reason everything else here is: a walker meets a
            // split in proportion to the sessions arriving at it, and the escape probability is
            // a property of an arrival.
            let mut esc_num = 0.0f64;
            let mut esc_den = 0.0f64;
            for r in in_band() {
                if let Some(q) = r.singleton_share() {
                    let w = f64::from(r.fan_in).max(1.0);
                    esc_num += w * q;
                    esc_den += w;
                }
            }
            // EXPERIMENT (`CERTUS_EXP_SINGLETON_ESCAPE=1`), off by default. The quantity is
            // measured correctly — on `qwen_code` band 0 it comes out 0.2216 against the trace's
            // 24.8% of requests sharing one block or less — but **composing** it over every split
            // a walker meets along a ~700-block path escapes far too much: measured, reuse
            // distance 0.0247 -> 0.1296 and `unique_keys` 0.1196 -> 0.5820, against `sharing_depth`
            // 0.4045 -> 0.2847. A net loss on two of three, so it is not emitted by default. See
            // research.md § Cohort exhaustion.
            let singleton_share = (esc_den > 0.0
                && std::env::var("CERTUS_EXP_SINGLETON_ESCAPE").is_ok_and(|v| v == "1"))
            .then(|| esc_num / esc_den);
            // No effective-sample-size floor here, deliberately: one was built and measured on
            // 2026-08-18 and it is not a criterion. `BandSkew::ess` is reported instead, because
            // the measurement refuted the hypothesis that motivated the floor — see research.md
            // § The child-choice law.
            // FR-054o. Fitted from the same rows as `length` and read together with it: `length` is
            // the law at this field's `reference` fan-in, not at every node.
            let length_by_cohort = fit_length_by_cohort(in_band().cloned());
            bands.push(SegmentBand {
                from_depth: *lo,
                length,
                length_by_cohort,
                out_degree,
                skew: skew.as_ref().map(|s| s.skew),
                singleton_share,
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
    /// Kish's effective sample size of the fan-in weights, `(Σw)²/Σw²`.
    ///
    /// How many splits the band's weighted mean *effectively* averages, which is not `splits`:
    /// fan-in spans four orders of magnitude within one band, so a band with dozens of splits can
    /// be a two-observation estimate. Reported because cohort decay is a **product** of these
    /// means down the trunk, so a band whose mean is set by one wide segment does not merely add
    /// noise — it biases every depth below it.
    pub ess: f64,
}

/// Fit one band's child-choice law from its splits.
///
/// The target is the **fan-in-weighted mean collision probability** over the band's splits
/// (see [`SegmentRow::collision`]). Weighted because this law is **keyed on the arrival**: the
/// generator spends it once per session descending the split, so the population to reproduce is
/// arrivals, not splits. That is the opposite key from `length`, which is drawn once per node
/// and is therefore fitted unweighted — see [`fit_process`] and FR-054m for why the distinction
/// is the whole rule and not a stylistic difference.
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
    let mut weight_sq = 0.0f64;
    for r in rows {
        let Some(c) = r.collision() else { continue };
        let w = f64::from(r.fan_in).max(1.0);
        *by_degree.entry(u64::from(r.out_degree)).or_insert(0.0) += w;
        num += w * c;
        den += w;
        weight_sq += w * w;
        splits += 1;
    }
    if den <= 0.0 || splits == 0 {
        return None;
    }
    let target = num / den;
    // Kish's effective sample size, `(Σw)²/Σw²`. The raw split count is the wrong measure of
    // how well-observed a band is when the estimate is a *weighted* mean: a band of 36 splits
    // whose fan-in sits almost entirely on two of them is a two-observation estimate, and
    // `target` is then one segment's collision probability wearing the band's name. This is the
    // same inverse-participation functional as `n_eff` and `collision` itself, one level up —
    // there over children, here over the splits the band averages.
    let ess = den * den / weight_sq;
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
        ess,
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

/// Fit `length`'s dependence on the node's own **fan-in** for one band (FR-054o).
///
/// A **step multiplier per fan-in bucket**, each the bucket's median run length over the band's, so
/// that a node in a bucket realises that bucket's own measured median — see
/// [`crate::schema::CohortStep::scale`]. **Unweighted over segments**: FR-054m's rule applies to this
/// exactly as it applies to the marginal it modifies, because the law is drawn once per node, and
/// weighting by fan-in would fit the scales to the arrivals and reintroduce that defect one level up.
///
/// # A power law was built here first, and its own fit refuted it
///
/// The first version fitted one exponent per band by OLS of `ln(length)` on `ln(fan_in)`. It tracks
/// `qwen_code`'s root band to within 1.1x at fan-in 2, 3 and 8 and then diverges — 1.5x at 60, 2.6x
/// at 1000 and **6.1x at the root**, asking for a 2.9-block run where the trace's top bucket medians
/// 18 — because the dependence **flattens** in log-log and one slope cannot bend. A power law
/// therefore concentrates its whole error on the highest-fan-in node in the trie, which is the node
/// the mechanism exists for. Measured, it was a Pareto loss on `qwen_code` (reuse 0.0264 → 0.1115,
/// `unique_keys` 0.2656 → 0.4729). Two further reasons the step form is right and not merely
/// flexible: it **cannot extrapolate**, so a cohort estimate whose range does not match the census's
/// cannot be turned into an arbitrary run length; and it is fitted on **medians**, matching the
/// median-preserving empirical it scales, where the mean-of-logs slope came out nearly twice as steep
/// as the bucket medians imply (−0.304 against −0.151).
///
/// Returns `None` unless **two buckets** have segments — one bucket is a band with no measured
/// dependence, and stating a single scale of 1.0 would put a field in the document that says nothing.
/// Buckets with no segments are omitted rather than interpolated.
///
/// Deliberately **no sample-size floor**: one was built for the child-law fit on 2026-08-18 and
/// measured not to be a criterion, so the segment count per bucket is *reported* by `fit --explain`
/// instead of silently gating the law. See research.md § The child-choice law.
fn fit_length_by_cohort(
    rows: impl Iterator<Item = SegmentRow> + Clone,
) -> Option<crate::schema::CohortLength> {
    use crate::schema::{CohortLength, CohortStep};

    let median = |v: &mut Vec<u32>| -> Option<f64> {
        if v.is_empty() {
            return None;
        }
        v.sort_unstable();
        Some(f64::from(v[v.len() / 2]))
    };
    // The denominator is restricted to the rows the buckets can hold, so that it and they are the
    // SAME population by construction. Every caller passes `Census::finish(2)` and a shared segment
    // has fan-in >= 2 by definition, so nothing is excluded in practice — but that is the caller's
    // invariant, not this function's, and a scale computed as a ratio of two different populations
    // would be wrong in a way no test of the callers would show.
    let covered = || {
        rows.clone()
            .filter(|r| r.length >= 1 && r.fan_in >= FAN_IN_BUCKETS[0])
    };
    let mut all: Vec<u32> = covered().map(|r| r.length).collect();
    // The band's own median, which is what `length` states since FR-054m. Taken from these rows
    // rather than from the emitted `Dist` so the two cannot disagree through the empirical builder's
    // ≤64-step coarsening.
    let band_median = median(&mut all)?;
    if band_median <= 0.0 {
        return None;
    }
    let mut steps: Vec<CohortStep> = Vec::new();
    for (i, lo) in FAN_IN_BUCKETS.iter().enumerate() {
        let hi = FAN_IN_BUCKETS.get(i + 1).copied().unwrap_or(u32::MAX);
        let mut lens: Vec<u32> = covered()
            .filter(|r| r.fan_in >= *lo && (hi == u32::MAX || r.fan_in < hi))
            .map(|r| r.length)
            .collect();
        if let Some(m) = median(&mut lens) {
            let scale = m / band_median;
            if scale.is_finite() && scale > 0.0 {
                steps.push(CohortStep {
                    from_fan_in: *lo,
                    scale,
                });
            }
        }
    }
    (steps.len() >= 2).then_some(CohortLength { by_fan_in: steps })
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

    /// A bare row carrying only the two fields the cohort fit reads.
    fn len_fan(length: u32, fan_in: u32) -> SegmentRow {
        SegmentRow {
            start_depth: 0,
            length,
            fan_in,
            ends: SegmentEnd::Fanout,
            out_degree: 2,
            shared_children: 2,
            child_fan_in_sum: u64::from(fan_in),
            shared_fan_in_sum: u64::from(fan_in),
            child_fan_in_sq: 0.0,
            child_fan_in_sq_all: 0.0,
            top_child_fan_in: fan_in,
        }
    }

    #[test]
    fn each_fan_in_buckets_scale_makes_it_realise_its_own_measured_median() {
        // The fit's central claim, and it is directly checkable rather than an argument about
        // centring: `scale` is the bucket's median over the band's, and the band's `length` is a
        // median-preserving empirical, so a node in a bucket realises THAT BUCKET's median.
        //
        // Planted with the shape the corpus actually shows — falling and FLATTENING, which is what
        // refuted the power law: 100 / 80 / 60 / 50 / 48 blocks across the five buckets. A single
        // log-log slope cannot bend like that; five scales reproduce it exactly.
        let planted = [(2u32, 100u32), (3, 80), (4, 60), (16, 50), (256, 48)];
        let mut rows: Vec<SegmentRow> = Vec::new();
        for (fan, len) in planted {
            // Five segments per bucket so each has an unambiguous median.
            rows.extend((0..5).map(|_| len_fan(len, fan)));
        }
        let c = fit_length_by_cohort(rows.iter().cloned()).expect("five populated buckets");
        assert_eq!(c.by_fan_in.len(), 5, "one step per populated bucket");
        // 25 segments over five distinct lengths: the band median is the middle bucket's, 60.
        let band_median = 60.0;
        for (step, (fan, len)) in c.by_fan_in.iter().zip(planted) {
            assert_eq!(step.from_fan_in, fan);
            let realised = band_median * step.scale;
            assert!(
                (realised - f64::from(len)).abs() < 1e-9,
                "bucket {fan} must realise its own median {len}, got {realised}"
            );
        }
        // And the flattening is preserved, which is the property a power law loses: the step from
        // the fourth bucket to the fifth is far smaller than the first to the second.
        let d = |a: usize, b: usize| c.by_fan_in[a].scale - c.by_fan_in[b].scale;
        assert!(
            d(0, 1) > 4.0 * d(3, 4),
            "the fitted law must keep the shape's flattening, not straighten it"
        );
    }

    #[test]
    fn the_cohort_law_is_fitted_over_segments_and_not_over_arrivals() {
        // FR-054m's rule applied to FR-054o: weight a law by what its draw is keyed ON. The scales
        // modify `length`, which is drawn once per NODE, so each bucket's median takes one
        // observation per segment. Fan-in weighting would fit them to the ARRIVALS and reintroduce
        // one level up the exact defect FR-054m removed — and invisibly, since plausible scales come
        // out either way.
        //
        // The fixture makes the two disagree: the `fan 2` bucket holds twenty 100-block segments and
        // one 4-block one, so its unweighted median is 100. Weighted by fan-in the single crowded
        // segment in the top bucket would dominate the band median instead, moving every scale.
        let mut rows: Vec<SegmentRow> = (0..20).map(|_| len_fan(100, 2)).collect();
        rows.push(len_fan(4, 2));
        rows.extend((0..20).map(|_| len_fan(10, 300)));
        let c = fit_length_by_cohort(rows.iter().cloned()).expect("two populated buckets");
        assert_eq!(c.by_fan_in.len(), 2, "buckets 3, 4-15 and 16-255 are empty");
        assert_eq!(c.by_fan_in[0].from_fan_in, 2);
        assert_eq!(c.by_fan_in[1].from_fan_in, 256);
        // 41 segments; the band median is 10 (21 of them are 10 or below... the sorted middle is 10).
        // So `fan 2` scales up by 10x and `fan 256+` sits at 1.0.
        assert!(
            c.by_fan_in[0].scale > c.by_fan_in[1].scale,
            "the thin-and-long bucket must scale above the crowded-and-short one: {:?}",
            c.by_fan_in
        );
        let band_median = 10.0;
        assert!(
            (band_median * c.by_fan_in[0].scale - 100.0).abs() < 1e-9,
            "the unweighted median of the fan-2 bucket is 100, not the weighted one"
        );
    }

    #[test]
    fn the_scales_denominator_covers_exactly_the_rows_the_buckets_do() {
        // A `scale` is a ratio of a bucket's median to the band's, so the two must be medians of the
        // SAME population. Rows below the first bucket's fan-in would otherwise sit in the
        // denominator and in no numerator, tilting every scale in the band at once. Every caller
        // passes `Census::finish(2)` so this cannot arise today — which is exactly why it is pinned
        // here rather than left to the callers, whose invariant it currently is.
        //
        // Twenty unshared rows are added with lengths that would drag the band median down from 100
        // to 1 if they counted. The fitted scales must be identical with and without them.
        let shared = [
            len_fan(100, 2),
            len_fan(100, 2),
            len_fan(100, 2),
            len_fan(40, 300),
            len_fan(40, 300),
            len_fan(40, 300),
        ];
        let clean = fit_length_by_cohort(shared.iter().cloned()).expect("two buckets");
        let mut polluted: Vec<SegmentRow> = shared.to_vec();
        polluted.extend((0..20).map(|_| len_fan(1, 1)));
        let dirty = fit_length_by_cohort(polluted.iter().cloned()).expect("two buckets");
        assert_eq!(
            clean.by_fan_in, dirty.by_fan_in,
            "fan-in-1 segments belong to no bucket and must not move the denominator"
        );
    }

    #[test]
    fn a_band_filling_only_one_bucket_states_no_cohort_law() {
        // The honest absence. Most deep bands in the corpus are overwhelmingly fan-in 2 — that tie
        // group is what broke the first version of the `--explain` diagnostic — and a band whose
        // segments all land in one bucket has no measured dependence. Emitting a single step of
        // scale 1.0 would put a field in the document that says nothing, and the walk would then
        // multiply every run by it.
        let rows: Vec<SegmentRow> = (0..50).map(|i| len_fan(i % 17 + 1, 2)).collect();
        assert!(fit_length_by_cohort(rows.iter().cloned()).is_none());
        // One segment cannot populate two buckets either.
        assert!(fit_length_by_cohort([len_fan(9, 4)].iter().cloned()).is_none());
        // Two segments in two buckets is the minimum that CAN state one.
        let two = [len_fan(9, 2), len_fan(90, 300)];
        assert!(fit_length_by_cohort(two.iter().cloned()).is_some());
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
    fn the_effective_sample_size_counts_weights_not_splits() {
        // `ess` exists because the raw split count says nothing about how well-observed a
        // *weighted* mean is, and it is what refuted the per-band sample floor on 2026-08-18:
        // measured, `tau2_airline`'s cohort-annihilating band has ess 12.5 while `qwen_code`'s
        // faithfully-composing root band has 2.4, so the quantity does not separate the two
        // traces and a floor on it is not a criterion.
        //
        // Equal weights give ess == splits; one dominant weight drives it to ~1 however many
        // splits there are. Both directions are asserted, since a measure that only ever went
        // down with concentration could be any monotone function of it.
        let equal: Vec<SegmentRow> = (0..4).map(|_| split_row(&[5, 5])).collect();
        let fit = fit_process(&equal).expect("a process");
        let band = &fit.skews[0];
        assert_eq!(band.splits, 4);
        assert!(
            (band.ess - 4.0).abs() < 1e-9,
            "four equally-weighted splits are four observations, got {}",
            band.ess
        );
        // Same four splits, but one carries 1000x the fan-in of the others.
        let mut skewed = vec![split_row(&[5000, 5000])];
        skewed.extend((0..3).map(|_| split_row(&[5, 5])));
        let fit = fit_process(&skewed).expect("a process");
        let band = &fit.skews[0];
        assert_eq!(band.splits, 4, "still four splits");
        assert!(
            band.ess < 1.01,
            "one split carrying the fan-in is ~one observation, got {}",
            band.ess
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

    /// FR-054m: `length` is drawn once per NODE, so it is fitted over SEGMENTS, unweighted.
    ///
    /// The construction is the one that exposed the defect on `qwen_code`: a single wide
    /// short run and many narrow long ones. Fan-in weighted the median comes out at the wide
    /// run's length; per segment it comes out at the narrow ones', and the narrow ones are
    /// what a per-node draw is asked for at almost every node it visits.
    #[test]
    fn run_length_is_fitted_per_segment_because_it_is_drawn_per_node() {
        let mut reqs: Vec<(u32, Vec<u64>)> = Vec::new();
        // One root run of 2 blocks walked by 40 sessions, then twenty pairs each walking a
        // private 6-block run before splitting again. Weighted, the 40-session run dominates.
        for s in 0..40u32 {
            let pair = u64::from(s) / 2;
            let mut path = vec![1, 2];
            for b in 0..6u64 {
                path.push(1000 + pair * 100 + b);
            }
            path.push(1000 + pair * 100 + 90 + u64::from(s % 2));
            reqs.push((s, path));
        }
        let rows = census_of(&reqs).finish(2);
        let root = rows.iter().find(|r| r.start_depth == 0).expect("root");
        assert_eq!(root.length, 2);
        assert_eq!(root.fan_in, 40);
        // Band 0 holds the root (length 2, fan-in 40) and the twenty pair runs that start at
        // depth 2 (length 6, fan-in 2). Per segment the median is 6; fan-in weighted it is 2.
        let in_band_0: Vec<&SegmentRow> = rows.iter().filter(|r| r.start_depth < 1).collect();
        assert_eq!(in_band_0.len(), 1, "only the root starts in band 0");
        let p = fit_process(&rows).expect("a process").process;
        let band = p
            .by_depth
            .iter()
            .find(|b| b.from_depth == 1)
            .expect("the 1-7 band holds the pair runs");
        let l = band.length.quantile(0.5).expect("median");
        assert!(
            (l - 6.0).abs() < 0.6,
            "the twenty 6-block segments must set the median, got {l}"
        );
    }

    /// The other half of the FR-054m rule: `skew` stays fan-in weighted.
    ///
    /// It is consumed once per arrival, so the population it must reproduce is arrivals. A
    /// blanket de-weighting would have taken this with it, which is why the rule is stated
    /// about the key rather than about the function.
    #[test]
    fn the_child_law_stays_weighted_by_fan_in_because_it_is_consumed_per_arrival() {
        // Two splits in one band with very different fan-in and very different concentration:
        // a 40-session split whose children are lopsided, and a 2-session even one. The
        // fitted collision probability must sit near the wide split's, not midway.
        let wide = split_row(&[38, 2]);
        let narrow = split_row(&[1, 1]);
        let rows = vec![wide, narrow];
        let fit = fit_process(&rows).expect("a process");
        let target = fit.skews[0].target;
        // Weighted: 40 of 42 arrivals meet the lopsided split, whose collision probability is
        // (38²+2²)/40² = 0.9025, against the even split's 0.5. Unweighted the two would
        // average to ~0.70.
        assert!(
            target > 0.85,
            "the 40-session split must dominate the child law, got {target}"
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
