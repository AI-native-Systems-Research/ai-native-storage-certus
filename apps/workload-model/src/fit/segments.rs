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
                top_child_fan_in: 0,
            };
            if ends == SegmentEnd::Fanout {
                let mut c = first[cur];
                while c != NO_PARENT {
                    let f = self.sessions[c as usize];
                    row.child_fan_in_sum += u64::from(f);
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
