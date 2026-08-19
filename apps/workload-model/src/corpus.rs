//! The shared key structure: a forest, a piecewise trunk, and occupancy.
//!
//! Three ideas carry most of the weight here.
//!
//! **Width is emergent, not configured.** A document states a *fanout*; the width
//! at each depth is `w(0) = roots.count`, `w(d+1) = w(d) × fanout(d+1)`. So the
//! realised width is only knowable after generating, which is why it is reported.
//!
//! **The fanout profile is piecewise.** Measured tries are flat for long stretches
//! and then fan out at particular depths, so a single exponent describes a
//! different shape rather than a coarse version of the real one.
//!
//! **Occupancy decides whether sharing actually happens.** A drawn `shared_depth`
//! is what a session *attempts*; it is realised only if some earlier session
//! walked the same steps. Occupancy is what makes that checkable before a run.

use crate::dist::Dist;
use crate::keys::{root, trunk_child, CacheKey, Generation};
use crate::rng::Stream;
use crate::schema::{Branching, Segment, Trees};

/// The occupancy this generator designs against.
///
/// A judgement, consistent with observation rather than established by it: in the
/// traces examined, occupancy below the fanout points settled in the low single
/// digits. Reality sits just under this, which is the right side for a floor.
pub const TARGET_OCCUPANCY: f64 = 4.0;

/// One resolved segment: where it starts, how wide it fans, and its child law.
///
/// A struct rather than the tuple this used to be because a segment carries two
/// independent facts about a trunk node — **how many children exist** (`fanout`) and
/// **how a session chooses among them** (`skew`) — and the second was silently dropped
/// for as long as the tuple had room for only the first.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ProfileSegment {
    /// This segment holds from here until the next.
    pub from_depth: u32,
    /// Mean children per trunk node.
    pub fanout: f64,
    /// Zipf exponent over child rank, or `None` to take the document-level one.
    pub skew: Option<f64>,
    /// Fraction of arrivals at a split landing on a child no other session takes.
    ///
    /// `None` means no escape, and the walk then draws nothing — which is what keeps every
    /// document that does not state one byte-identical.
    pub singleton_share: Option<f64>,
}

/// A resolved fanout profile: ascending segments, the first at depth 0.
#[derive(Debug, Clone, PartialEq)]
pub struct Profile {
    segments: Vec<ProfileSegment>,
}

impl Profile {
    /// A uniform profile: one segment from depth 0.
    pub fn uniform(fanout: f64) -> Profile {
        Profile {
            segments: vec![ProfileSegment {
                from_depth: 0,
                fanout,
                skew: None,
                singleton_share: None,
            }],
        }
    }

    /// A piecewise profile from explicit segments.
    pub fn from_segments(segs: &[Segment]) -> Profile {
        Profile::from_parts(
            segs.iter()
                .map(|s| ProfileSegment {
                    from_depth: s.from_depth,
                    fanout: s.fanout,
                    skew: s.skew,
                    // The per-depth `branching` spelling has no singleton share: it describes
                    // width by depth and cannot say how many children exist at a split, which is
                    // the whole reason the node-level spelling exists. Only a fitted
                    // `SegmentBand` carries one.
                    singleton_share: None,
                })
                .collect(),
        )
    }

    /// A profile from resolved segments, sorted, with a depth-0 segment guaranteed.
    ///
    /// The one place a `Profile` is assembled, so the node-level spelling can carry fields the
    /// per-depth `Segment` has no room for — currently the singleton-escape probability — without
    /// a second copy of the sort-and-backfill rule for the two to disagree over.
    pub fn from_parts(mut segments: Vec<ProfileSegment>) -> Profile {
        segments.sort_by_key(|s| s.from_depth);
        if segments.is_empty() {
            segments.push(ProfileSegment {
                from_depth: 0,
                fanout: 1.0,
                skew: None,
                singleton_share: None,
            });
        }
        Profile { segments }
    }

    /// The fanout in force at `depth`.
    pub fn fanout_at(&self, depth: u32) -> f64 {
        self.at(depth).fanout
    }

    /// The child-choice exponent in force at `depth`, if the segment states one.
    ///
    /// `None` means the segment defers to `corpus.trees.branch_skew`, which is what an
    /// absent override has always meant in the schema and what nothing read until now.
    pub fn skew_at(&self, depth: u32) -> Option<f64> {
        self.at(depth).skew
    }

    /// The singleton-escape probability in force at `depth`, if the profile states one.
    pub fn singleton_share_at(&self, depth: u32) -> Option<f64> {
        self.at(depth).singleton_share
    }

    /// The segment in force at `depth`.
    fn at(&self, depth: u32) -> &ProfileSegment {
        let mut f = &self.segments[0];
        for s in &self.segments {
            if depth >= s.from_depth {
                f = s;
            } else {
                break;
            }
        }
        f
    }

    /// Distinct trunk paths at `depth`: `roots × Π fanout(k)`.
    ///
    /// Expressed as a product over the profile rather than `fanout^depth`, which
    /// is what makes occupancy computable for a piecewise trunk. For a uniform
    /// profile it collapses back to the exponential form.
    pub fn paths(&self, depth: u32, roots: u32) -> f64 {
        let mut p = f64::from(roots);
        for k in 1..=depth {
            p *= self.fanout_at(k);
            if p.is_infinite() {
                break;
            }
        }
        p
    }

    /// The resolved segments, for the report.
    pub fn segments(&self) -> &[ProfileSegment] {
        &self.segments
    }
}

/// How long a trunk path survives, when churn is configured.
///
/// A depth-`d` path lives only while all `d+1` of its nodes do, so its effective
/// half-life is `half_life/(d+1)`: shallow prefixes are stable and deep ones
/// fragile, which is the way round real deployments behave.
pub fn path_lifetime_ns(churn_half_life_ns: u64, depth: u32) -> Option<u64> {
    if churn_half_life_ns == 0 {
        None
    } else {
        Some(churn_half_life_ns / u64::from(depth + 1))
    }
}

/// Trunk occupancy: sessions per distinct trunk path at `depth`.
///
/// With churn configured, the window is the shorter of the configured window and
/// the path's own lifetime — because a path accumulates sharers only while it
/// exists. Without that term the floor would approve a configuration whose
/// sharing churn then destroys.
pub fn occupancy(
    profile: &Profile,
    roots: u32,
    sessions_per_window: f64,
    depth: u32,
    window_ns: Option<u64>,
    churn_half_life_ns: u64,
) -> f64 {
    let paths = profile.paths(depth, roots);
    if paths <= 0.0 {
        return 0.0;
    }
    let mut sessions = sessions_per_window;
    if let (Some(win), Some(life)) = (window_ns, path_lifetime_ns(churn_half_life_ns, depth)) {
        if win > 0 && life < win {
            sessions *= life as f64 / win as f64;
        }
    }
    sessions / paths
}

/// Solve for the uniform fanout that keeps sharing realisable.
///
/// `(sessions_per_window / roots / target) ^ (1 / trunk_steps)` — a closed form,
/// never an iterative calibration against generated output, so no part of this
/// model requires a nonlinear fit.
///
/// `trunk_steps` is the number of fanout *steps* from a root down to the deepest
/// shared node, which is one less than `shared_depth`: FR-014a makes a shared
/// prefix of depth `s` occupy ordinals `0..s`, so the walk from ordinal 0 to
/// ordinal `s-1` takes `s-1` steps. Passing `s` instead solves for a slightly
/// smaller fanout, hence fewer paths and higher occupancy than asked for — safe,
/// but not what the closed form says.
///
/// Resolves to a *uniform* profile deliberately: a non-uniform one encodes a
/// claim about where branches diverge, which the generator has no basis to invent
/// and which must come from the user or from `fit`.
pub fn auto_fanout(sessions_per_window: f64, roots: u32, trunk_steps: u32) -> f64 {
    if trunk_steps == 0 || roots == 0 {
        return 1.0;
    }
    let headroom = sessions_per_window / f64::from(roots) / TARGET_OCCUPANCY;
    if headroom <= 1.0 {
        return 1.0;
    }
    headroom.powf(1.0 / f64::from(trunk_steps)).max(1.0)
}

/// One resolved band of a node-level trunk process.
#[derive(Debug, Clone)]
struct ResolvedBand {
    from_depth: u32,
    length: Dist,
    out_degree: Dist,
    skew: Option<f64>,
    singleton_share: Option<f64>,
}

/// A [`crate::schema::SegmentProcess`] with its bands ordered for lookup.
#[derive(Debug, Clone)]
pub struct ResolvedSegments {
    /// Ascending by `from_depth`.
    bands: Vec<ResolvedBand>,
}

impl ResolvedSegments {
    /// The bands of a schema process, ordered for lookup.
    fn new(p: &crate::schema::SegmentProcess) -> ResolvedSegments {
        let mut bands: Vec<ResolvedBand> = p
            .by_depth
            .iter()
            .map(|s| ResolvedBand {
                from_depth: s.from_depth,
                length: s.length.clone(),
                out_degree: s.out_degree.clone(),
                skew: s.skew,
                singleton_share: s.singleton_share,
            })
            .collect();
        bands.sort_by_key(|b| b.from_depth);
        ResolvedSegments { bands }
    }

    /// The band covering `depth`.
    fn at(&self, depth: u32) -> &ResolvedBand {
        let mut chosen = &self.bands[0];
        for b in &self.bands {
            if depth >= b.from_depth {
                chosen = b;
            } else {
                break;
            }
        }
        chosen
    }

    /// Blocks the unary run below the split at `node` continues, at least 1.
    ///
    /// Keyed on the **node**, not on the visit, for the same reason `child_count` is: it is
    /// what keeps a long run reproducible and independent of arrival order, and it is why a
    /// walker needs no stored trie.
    pub fn run_length(&self, seed: u64, node: CacheKey, depth: u32) -> u32 {
        let len = &self.at(depth).length;
        let mut st = Stream::new(seed ^ node.0, u64::from(depth) ^ TAG_SEGMENT);
        len.sample_u64(&mut st).max(1).min(u64::from(u32::MAX)) as u32
    }

    /// Children at the split at `node` — the total, singletons included.
    pub fn out_degree(&self, seed: u64, node: CacheKey, depth: u32) -> u32 {
        let deg = &self.at(depth).out_degree;
        let mut st = Stream::new(
            seed ^ node.0,
            (u64::from(depth) ^ TAG_SEGMENT).wrapping_add(1),
        );
        deg.sample_u64(&mut st).max(1).min(u64::from(u32::MAX)) as u32
    }

    // NO `skew_at` here. The band's skew reaches the walk through `mean_profile`, so
    // `Corpus::skew_at` has one implementation covering both trunk spellings; a second
    // accessor would be a place for the two to disagree.

    /// The band-mean fanout per depth, as a [`Profile`].
    ///
    /// A node-level process still has a mean fanout at each depth — `out_degree` children
    /// once every `length` blocks — and occupancy, rule 16 and the reports are all written
    /// against one. `E[deg]^(1/E[len])` per band is that mean in the geometric sense the
    /// profile multiplies in.
    /// The band's own `skew` is carried through, because the profile is what
    /// [`Corpus::skew_at`] consults and a band mean that forgot its child law would put the
    /// two spellings on different laws.
    pub fn mean_profile(&self) -> Profile {
        Profile::from_parts(
            self.bands
                .iter()
                .map(|b| {
                    let l = b.length.mean().unwrap_or(1.0).max(1.0);
                    let d = b.out_degree.mean().unwrap_or(1.0).max(1.0);
                    ProfileSegment {
                        from_depth: b.from_depth,
                        fanout: d.powf(1.0 / l),
                        skew: b.skew,
                        // Carried for the same reason `skew` is: the profile is what the walk
                        // consults, so a band mean that dropped its escape probability would
                        // silently generate a trunk nobody ever leaves.
                        singleton_share: b.singleton_share,
                    }
                })
                .collect(),
        )
    }
}

/// Where the trunk's next split is, carried down one walk.
///
/// The walk already re-derives a session's path from the root every turn, so this rides
/// along with it and nothing is stored per node — the memory bound stays the live session
/// population (FR-010). It is the only state a node-level process needs: "the last split was
/// here and its run is this long" is enough to know whether the current depth is a split.
#[derive(Debug, Clone, Copy)]
pub struct SplitState {
    /// Depth at which the next split happens.
    next_split_depth: u32,
    /// A cap on how deep a run may extend — the root's own path level (FR-054k).
    ///
    /// A run longer than the requests of the sessions walking it cannot be completed, and under
    /// FR-054k those sessions decline it, which empties the trunk instead of lengthening it. The two
    /// are a **joint per-root pair**: a root's preamble and its sessions' path length are not
    /// independent in a trace, because the preamble *is* a prefix of those requests. Capping here
    /// keeps run length a function of the node — every session at a node arrived through the same
    /// root and so carries the same cap — which is what keeps the trie consistent between walkers.
    ///
    /// `u32::MAX` is no cap, which is the behaviour before this existed.
    path_cap: u32,
}

impl SplitState {
    /// Re-cap the runs below this point.
    ///
    /// The cap is not fixed for a walk: a run is completed by the sessions **still on it**, and that
    /// is the cohort at this depth rather than the root's whole population. Near the root a run must
    /// suit 44 sessions and is bounded by the shortest of 44; at depth 300 the cohort has subdivided
    /// to two or three and the bound is the shortest of those — a far weaker constraint, which is
    /// what lets sharing run deep. Measured, using the root's count everywhere capped the trunk
    /// before depth 512 while the trace has 35 shared segments beyond it.
    ///
    /// Still a property of the node: two sessions at one node arrived by the same path, so they
    /// carry the same cohort estimate and agree on the cap.
    pub fn set_cap(&mut self, cap: u32) {
        self.path_cap = cap;
    }
}

/// Domain separator for a node's own run-length and out-degree draws.
const TAG_SEGMENT: u64 = 0x5E67_3E27;

impl SplitState {
    /// The depth at which this walk's next split falls.
    ///
    /// Exposed so a walker can ask whether it will still be issuing requests that deep before it
    /// commits to a run — see FR-054k. A run a session cannot finish is one it must not join.
    pub fn next_split(&self) -> u32 {
        self.next_split_depth
    }

    /// The state a walk starts in at `root`.
    ///
    /// The root is itself a split node, so its run length comes from *its* stream — which is
    /// exactly what gives each root its own preamble length. Under a per-depth profile every
    /// depth is a potential split, which is the degenerate case of the same state.
    pub fn at_root(corpus: &Corpus, root: CacheKey, path_cap: u32) -> SplitState {
        match &corpus.segments {
            None => SplitState {
                next_split_depth: 0,
                path_cap,
            },
            Some(s) => SplitState {
                next_split_depth: s.run_length(corpus.seed, root, 0).min(path_cap),
                path_cap,
            },
        }
    }
}

/// The realised shared structure.
#[derive(Debug, Clone)]
pub struct Corpus {
    /// Number of trees.
    pub roots: u32,
    /// A node-level trunk process, when the document states one.
    ///
    /// `Some` means `branching: {by_depth: [...]}` — the shape is asked of the node rather
    /// than read off the depth. [`Corpus::profile`] is still resolved alongside as the
    /// band-mean fanout, because occupancy, validation rule 16 and reporting are all written
    /// against a per-depth fanout and a node-level process still has a mean at each depth.
    pub segments: Option<ResolvedSegments>,
    /// The resolved fanout profile.
    pub profile: Profile,
    /// Zipf exponent over child rank when a session picks among children.
    pub branch_skew: f64,
    /// Payload size distribution.
    pub block_bytes: Dist,
    /// Root seed.
    pub seed: u64,
}

impl Corpus {
    /// Resolve a `corpus.trees` section, solving `auto` if needed.
    pub fn resolve(
        trees: &Trees,
        block_bytes: Dist,
        seed: u64,
        sessions_per_window: f64,
        trunk_steps: u32,
    ) -> Corpus {
        let segments = match &trees.branching {
            Branching::Segments(p) if !p.by_depth.is_empty() => Some(ResolvedSegments::new(p)),
            _ => None,
        };
        let profile = match &trees.branching {
            Branching::Auto(_) => Profile::uniform(auto_fanout(
                sessions_per_window,
                trees.roots.count,
                trunk_steps,
            )),
            Branching::Uniform(f) => Profile::uniform(*f),
            Branching::Profile(segs) => Profile::from_segments(segs),
            // The band means, so occupancy and rule 16 still have a per-depth fanout to
            // judge. An empty band list resolves to a flat trunk rather than panicking;
            // validation rejects it, and a caller that skipped validation gets the
            // documented default rather than a crash.
            Branching::Segments(_) => segments
                .as_ref()
                .map(|s| s.mean_profile())
                .unwrap_or_else(|| Profile::uniform(1.0)),
        };
        Corpus {
            roots: trees.roots.count,
            segments,
            profile,
            branch_skew: trees.branch_skew,
            block_bytes,
            seed,
        }
    }

    /// How many children the node at `key`, depth `depth`, has.
    ///
    /// A non-integer mean is realised **exactly** by randomised rounding: a node
    /// gets `floor(m)` or `ceil(m)` children with the probabilities that make
    /// `E[children] = m`. So realised width is stochastic and converges on the
    /// configured value as more of the trie is visited.
    ///
    /// The draw is keyed on the **node**, not on the visit. That is what keeps a
    /// long run reproducible and independent of arrival order — and it is why a
    /// Chinese-restaurant rule was rejected: minting on arrival would make the
    /// key space a function of the request sequence.
    pub fn child_count(&self, key: CacheKey, depth: u32) -> u32 {
        let m = self.profile.fanout_at(depth).max(1.0);
        let lo = m.floor();
        let frac = m - lo;
        let mut st = Stream::new(self.seed ^ key.0, u64::from(depth) ^ 0xC1D_u64);
        let n = if frac > 0.0 && st.next_f64() < frac {
            lo + 1.0
        } else {
            lo
        };
        n.max(1.0) as u32
    }

    /// The child-choice exponent in force at `depth`.
    ///
    /// The segment's own `skew` where it states one, otherwise the document-level
    /// `branch_skew`. Both trunk spellings arrive here: a node-level process carries its
    /// band's skew through [`ResolvedSegments::mean_profile`], so there is one lookup
    /// rather than one per spelling.
    ///
    /// The per-segment override is not new — `corpus.trees.branching[].skew` has been in the
    /// schema since the profile spelling shipped — but nothing read it, so a document could
    /// state a child law and be silently generated under a different one. Measured before
    /// the fix: `skew: 0.0` and `skew: 3.0` produced byte-identical streams.
    pub fn skew_at(&self, depth: u32) -> f64 {
        self.profile.skew_at(depth).unwrap_or(self.branch_skew)
    }

    /// The singleton-escape probability at `depth`, or `None` where the document states none.
    ///
    /// There is deliberately no document-level default to fall back on: an escape probability is
    /// a measured property of a split's total out-degree, and inventing one would put sessions
    /// into private tails at a rate nothing measured. Absent means no escape.
    pub fn singleton_share_at(&self, depth: u32) -> Option<f64> {
        self.profile.singleton_share_at(depth)
    }

    /// Which child a descending session picks, under the child law `skew`.
    ///
    /// Keyed on the *session's* stream rather than the node's, because this is a
    /// choice the session makes; the node's own stream decides only how many
    /// children exist.
    pub fn pick_child(&self, st: &mut Stream, children: u32, skew: f64) -> u32 {
        self.pick_child_p(st, children, skew).0
    }

    /// The chosen child **and the probability of choosing it**.
    ///
    /// `skew` is passed rather than read off `self`, so that the one place a child law can
    /// come from is [`Corpus::skew_at`] and a caller cannot accidentally reach past a
    /// segment's override to the document-level default.
    ///
    /// The probability is what lets a walker carry an *expected cohort size* down the
    /// trunk — `sessions on this root x PROD p(child taken)` — and so know, without any
    /// per-node census, roughly how many other sessions are still beside it. That is the
    /// quantity the trace's structure is actually made of: measured over six traces, a
    /// cohort's fan-in falls by **subdivision** until a branch holds one session, at which
    /// point that session is in its private tail. Leakage at a split — sessions retiring
    /// *on* the shared trunk — has a median of exactly 0.000 in every depth band, because
    /// sessions retire in their private tails instead, those being 95%+ of all nodes.
    ///
    /// So a session leaves the shared region when its own cohort is exhausted, which
    /// depends on how popular the branches it took were. That correlation is the whole
    /// point: an earlier design shed sessions off the trunk with a per-node coin flip,
    /// uncorrelated with the session, and it made `sharing_depth` three times worse.
    pub fn pick_child_p(&self, st: &mut Stream, children: u32, skew: f64) -> (u32, f64) {
        if children <= 1 {
            return (0, 1.0);
        }
        let n = u64::from(children);
        if skew <= 0.0 {
            return (st.next_below(n) as u32, 1.0 / children as f64);
        }
        let d = Dist::Shaped(crate::dist::Shape::Zipf {
            s: skew,
            n: Some(n),
        });
        // Zipf yields a 1-based rank; children are 0-based.
        let rank = d.sample_u64(st).max(1).min(n);
        let idx = (rank - 1) as u32;
        (idx, crate::dist::zipf_pmf_at(skew, n, rank))
    }

    /// One step down the trunk, carrying the walk's split state.
    ///
    /// Returns the child key and the probability of having taken it, which is what the caller
    /// multiplies into its expected cohort. This is the single entry point both trunk
    /// spellings answer:
    ///
    /// * per-depth profile — every depth is a potential split, so the node's child count is
    ///   read off the profile and a choice is made among them;
    /// * node-level segments — a split happens only where the run drawn at the last split
    ///   ends. Between splits the node has exactly one child, so the cohort is *not* divided,
    ///   which is what makes a long run a shared segment rather than a slow fanout.
    pub fn trunk_step_stateful(
        &self,
        cur: CacheKey,
        depth: u32,
        state: &mut SplitState,
        st: &mut Stream,
        gen: Generation,
    ) -> (CacheKey, f64) {
        let skew = self.skew_at(depth);
        match &self.segments {
            None => {
                let n = self.child_count(cur, depth);
                let (idx, p) = self.pick_child_p(st, n, skew);
                (trunk_child(cur, idx, gen), p)
            }
            Some(s) => {
                if depth < state.next_split_depth {
                    // Inside a run: one child, cohort intact.
                    (trunk_child(cur, 0, gen), 1.0)
                } else {
                    let n = s.out_degree(self.seed, cur, depth);
                    let (idx, p) = self.pick_child_p(st, n, skew);
                    let child = trunk_child(cur, idx, gen);
                    state.next_split_depth = depth
                        .saturating_add(s.run_length(self.seed, child, depth))
                        .min(state.path_cap);
                    (child, p)
                }
            }
        }
    }

    /// The trunk child at `index` of `cur` — the key half of a trunk step.
    ///
    /// Split out from [`Corpus::trunk_step`] so a caller that needs the *probability* of
    /// the step (to carry an expected cohort) can draw the child itself with
    /// [`Corpus::pick_child_p`] and still mint the key through one implementation.
    pub fn trunk_child_at(&self, cur: CacheKey, index: u32, gen: Generation) -> CacheKey {
        trunk_child(cur, index, gen)
    }

    /// The key at `root_index`.
    pub fn root_key(&self, root_index: u32, gen: Generation) -> CacheKey {
        root(root_index, gen)
    }

    /// One level down the trunk: the child of `cur` this session descends into.
    ///
    /// The unit of a trunk walk, so that a caller which cannot afford to
    /// materialise the path — the generator, which walks a trunk per turn and
    /// keeps nothing — uses the *same* derivation as one which can.
    pub fn trunk_step(
        &self,
        cur: CacheKey,
        depth: u32,
        st: &mut Stream,
        gen: Generation,
    ) -> CacheKey {
        let n = self.child_count(cur, depth);
        let idx = self.pick_child(st, n, self.skew_at(depth));
        trunk_child(cur, idx, gen)
    }

    /// Walk `depth` levels of shared trunk from `from`, using `st` for the
    /// child choices, returning every key on the path.
    pub fn walk_trunk(
        &self,
        from: CacheKey,
        depth: u32,
        st: &mut Stream,
        gen: Generation,
    ) -> Vec<CacheKey> {
        let mut out = Vec::with_capacity(depth as usize);
        let mut cur = from;
        for d in 1..=depth {
            cur = self.trunk_step(cur, d, st, gen);
            out.push(cur);
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::schema::{AutoTag, Roots};

    fn trees(branching: Branching) -> Trees {
        Trees {
            roots: Roots {
                count: 12,
                popularity: Dist::Shaped(crate::dist::Shape::Zipf { s: 0.9, n: None }),
            },
            shared_depth: Dist::Scalar(18.0),
            branching,
            branch_skew: 0.9,
            churn: None,
        }
    }

    fn corpus(branching: Branching) -> Corpus {
        Corpus::resolve(&trees(branching), Dist::Scalar(131_072.0), 7, 40_000.0, 40)
    }

    #[test]
    fn a_flat_segment_keeps_width_constant() {
        // The property a scalar cannot express: 40 depths at fanout 1.0 leave
        // width unchanged, whereas a uniform 1.05 would widen it about 7x.
        let p = Profile::from_segments(&[Segment {
            from_depth: 0,
            fanout: 1.0,
            skew: None,
            churn_half_life: None,
        }]);
        assert_eq!(p.paths(40, 12), 12.0);
        let uniform = Profile::uniform(1.05);
        assert!(uniform.paths(40, 12) > 12.0 * 6.0);
    }

    #[test]
    fn a_piecewise_profile_fans_out_only_where_told() {
        // Global preamble, one fanout, then flat: the shape real traces show.
        let p = Profile::from_segments(&[
            Segment {
                from_depth: 0,
                fanout: 1.0,
                skew: None,
                churn_half_life: None,
            },
            Segment {
                from_depth: 12,
                fanout: 40.0,
                skew: None,
                churn_half_life: None,
            },
            Segment {
                from_depth: 13,
                fanout: 1.0,
                skew: None,
                churn_half_life: None,
            },
        ]);
        assert_eq!(p.fanout_at(0), 1.0);
        assert_eq!(p.fanout_at(11), 1.0);
        assert_eq!(p.fanout_at(12), 40.0);
        assert_eq!(p.fanout_at(30), 1.0);
        // Width jumps once and then holds.
        assert_eq!(p.paths(11, 1), 1.0);
        assert_eq!(p.paths(12, 1), 40.0);
        assert_eq!(p.paths(40, 1), 40.0);
    }

    #[test]
    fn paths_collapses_to_the_exponential_form_when_uniform() {
        let p = Profile::uniform(1.5);
        let expected = 4.0 * 1.5f64.powi(6);
        assert!((p.paths(6, 4) - expected).abs() < 1e-9);
    }

    #[test]
    fn randomised_rounding_averages_the_configured_mean() {
        // A non-integer mean is realised exactly rather than approximately.
        let c = corpus(Branching::Uniform(1.25));
        let n = 20_000;
        let total: u64 = (0..n)
            .map(|i| u64::from(c.child_count(CacheKey(i as u64), 5)))
            .sum();
        let mean = total as f64 / n as f64;
        assert!((mean - 1.25).abs() < 0.02, "mean was {mean}");
        // And every node has at least one child, so no session runs off the end.
        for i in 0..1000 {
            assert!(c.child_count(CacheKey(i), 5) >= 1);
        }
    }

    #[test]
    fn child_count_is_keyed_on_the_node_not_the_visit() {
        // The property that keeps a long run reproducible and independent of
        // arrival order, and the reason a Chinese-restaurant rule was rejected.
        let c = corpus(Branching::Uniform(1.25));
        let k = CacheKey(0xABCD);
        let first = c.child_count(k, 7);
        for _ in 0..100 {
            assert_eq!(
                c.child_count(k, 7),
                first,
                "child count moved between visits"
            );
        }
    }

    #[test]
    fn auto_resolves_to_a_uniform_profile_within_the_occupancy_floor() {
        // FR-009g: a closed form, and deliberately uniform -- a non-uniform
        // profile would encode a claim the generator has no basis to invent.
        let c = corpus(Branching::Auto(AutoTag::Auto));
        assert_eq!(c.profile.segments().len(), 1, "auto must be uniform");
        let f = c.profile.fanout_at(1);
        assert!(
            f > 1.0 && f < 1.3,
            "auto fanout {f} outside the useful range"
        );
        // At the p99 depth it solved for, occupancy lands at the target.
        let occ = occupancy(&c.profile, c.roots, 40_000.0, 40, None, 0);
        assert!((occ - TARGET_OCCUPANCY).abs() < 0.5, "occupancy {occ}");
    }

    #[test]
    fn auto_never_returns_below_one() {
        // Too few sessions to occupy any trunk: a fanout below 1 would let a
        // session run off the end of the trunk, so clamp rather than solve.
        assert_eq!(auto_fanout(1.0, 1000, 40), 1.0);
    }

    #[test]
    fn occupancy_falls_as_the_trunk_widens() {
        let p = Profile::uniform(1.25);
        let a = occupancy(&p, 12, 40_000.0, 4, None, 0);
        let b = occupancy(&p, 12, 40_000.0, 40, None, 0);
        assert!(a > b, "occupancy must fall with depth: {a} vs {b}");
    }

    #[test]
    fn churn_shortens_the_window_and_bites_hardest_deep() {
        // A path lives half_life/(d+1), so a half-life generous at depth 4 can
        // be far too short at depth 40. Without this term the occupancy floor
        // would approve sharing that churn then destroys.
        let p = Profile::uniform(1.0); // width fixed, so only the window varies
        let window = 3_600_000_000_000u64; // 1 h
        let half_life = 3_600_000_000_000u64; // 1 h
        let shallow = occupancy(&p, 1, 1000.0, 4, Some(window), half_life);
        let deep = occupancy(&p, 1, 1000.0, 40, Some(window), half_life);
        assert!(deep < shallow, "{deep} should be below {shallow}");
        // With no churn the window term is absent entirely.
        let none = occupancy(&p, 1, 1000.0, 40, Some(window), 0);
        assert!(none > deep);
    }

    #[test]
    fn path_lifetime_is_none_without_churn() {
        assert_eq!(path_lifetime_ns(0, 10), None);
        assert_eq!(path_lifetime_ns(1000, 9), Some(100));
    }

    #[test]
    fn walking_the_trunk_is_reproducible_and_shares_a_prefix() {
        let c = corpus(Branching::Uniform(1.02));
        let r = c.root_key(3, Generation::STABLE);
        let a = c.walk_trunk(r, 20, &mut Stream::new(1, 1), Generation::STABLE);
        let b = c.walk_trunk(r, 20, &mut Stream::new(1, 1), Generation::STABLE);
        assert_eq!(a, b, "same session stream must walk the same path");
        // A narrow trunk means two different sessions usually agree for a while.
        let d = c.walk_trunk(r, 20, &mut Stream::new(1, 2), Generation::STABLE);
        let common = a.iter().zip(&d).take_while(|(x, y)| x == y).count();
        assert!(common > 0, "a narrow trunk should share at least one level");
    }

    #[test]
    fn pick_child_stays_in_range() {
        let c = corpus(Branching::Uniform(1.25));
        let mut st = Stream::new(5, 5);
        for n in 1..40u32 {
            for _ in 0..50 {
                assert!(c.pick_child(&mut st, n, c.skew_at(1)) < n);
            }
        }
    }

    /// A node-level process with the given run length and out-degree, one band.
    fn seg_corpus(length: f64, out_degree: f64) -> Corpus {
        use crate::schema::{SegmentBand, SegmentProcess};
        corpus(Branching::Segments(SegmentProcess {
            by_depth: vec![SegmentBand {
                from_depth: 0,
                length: Dist::Scalar(length),
                out_degree: Dist::Scalar(out_degree),
                skew: None,
                singleton_share: None,
            }],
        }))
    }

    #[test]
    fn a_node_level_process_gives_each_root_its_own_preamble() {
        // The property the whole spelling exists for, and the one a per-depth profile
        // cannot express: measured preamble lengths are per-ROOT and multi-modal —
        // `appworld` splits at 23, 3194 and 5556 blocks, `browsecompplus` at 1, 141, 939 —
        // so a profile keyed on depth must fan out at depth 141 for every root or for none.
        //
        // With a run length drawn from the node's own stream, two roots get different first
        // splits. A geometric length gives a spread; the assertion is that the splits are
        // NOT at the same depth for every root, which is precisely what the old spelling
        // guaranteed.
        let c = corpus(Branching::Segments(crate::schema::SegmentProcess {
            by_depth: vec![crate::schema::SegmentBand {
                from_depth: 0,
                length: Dist::Shaped(crate::dist::Shape::Geometric { mean: 20.0 }),
                out_degree: Dist::Scalar(3.0),
                skew: None,
                singleton_share: None,
            }],
        }));
        let firsts: Vec<u32> = (0..12u32)
            .map(|i| {
                SplitState::at_root(&c, c.root_key(i, Generation::STABLE), u32::MAX)
                    .next_split_depth
            })
            .collect();
        let distinct: std::collections::BTreeSet<u32> = firsts.iter().copied().collect();
        assert!(
            distinct.len() > 3,
            "twelve roots produced only {} distinct preamble lengths: {firsts:?}",
            distinct.len()
        );
        assert!(
            firsts.iter().all(|l| *l >= 1),
            "a preamble is at least one block: {firsts:?}"
        );
    }

    #[test]
    fn inside_a_run_the_cohort_is_not_divided_and_at_a_split_it_is() {
        // What makes a long run a shared SEGMENT rather than a slow fanout: between splits
        // the node has exactly one child, so the probability of the step is 1 and a walker's
        // expected cohort is unchanged. Only a split divides it.
        let c = seg_corpus(5.0, 4.0);
        let root = c.root_key(0, Generation::STABLE);
        let mut state = SplitState::at_root(&c, root, u32::MAX);
        assert_eq!(state.next_split_depth, 5, "a const run length of 5");
        let mut st = Stream::new(1, 1);
        let mut cur = root;
        let mut ps = Vec::new();
        for d in 1..=6u32 {
            let (next, p) = c.trunk_step_stateful(cur, d, &mut state, &mut st, Generation::STABLE);
            ps.push(p);
            cur = next;
        }
        // Depths 1..4 are inside the run; depth 5 is the split.
        assert_eq!(&ps[..4], &[1.0, 1.0, 1.0, 1.0], "a run must not divide");
        assert!(
            ps[4] < 1.0,
            "the split at depth 5 must divide the cohort, p was {}",
            ps[4]
        );
    }

    #[test]
    fn a_segment_process_still_offers_a_per_depth_mean_for_occupancy() {
        // Occupancy, rule 16 and the reports are all written against a per-depth fanout, and
        // a node-level process still has one: `out_degree` children once every `length`
        // blocks is a geometric mean of `deg^(1/len)` per step. Without this, adopting the
        // new spelling would silently disable the occupancy floor.
        let c = seg_corpus(4.0, 16.0);
        // 16 children every 4 blocks is 16^(1/4) = 2.0 per step.
        assert!(
            (c.profile.fanout_at(1) - 2.0).abs() < 1e-9,
            "band-mean fanout was {}",
            c.profile.fanout_at(1)
        );
        assert!(c.segments.is_some(), "the node-level process is retained");
    }

    #[test]
    fn pick_child_realises_a_skewed_but_non_degenerate_histogram() {
        // `pick_child_stays_in_range` above asserts only the *range*, which is how a
        // law that returned child 0 on **every** 2-way split survived unnoticed until
        // 2026-08-14: `branch_skew` 0.5, 0.9 and 1.5 all produced byte-identical
        // streams, and a 2-way split is the commonest branch point in real traces
        // (~65% of descents). A range assertion cannot see that; a histogram can.
        let c = corpus(Branching::Uniform(1.25));
        let mut st = Stream::new(11, 11);
        const DRAWS: u64 = 20_000;

        // Discrete Zipf at s = 0.9 over two children: p_1 = 1/(1 + 2^-0.9) = 0.6511.
        let mut hist = [0u64; 2];
        for _ in 0..DRAWS {
            hist[c.pick_child(&mut st, 2, 0.9) as usize] += 1;
        }
        let p0 = hist[0] as f64 / DRAWS as f64;
        assert!(
            (p0 - 0.6511).abs() < 0.02,
            "a 2-way split at branch_skew 0.9 must take child 0 about 65% of the time, \
             not 100% and not 50%; got {p0:.4}"
        );

        // And every child must be reachable. The superseded continuous inverse gave
        // the top *rank* probability exactly zero, so the last child was unreachable
        // at every support size, not just at two.
        for n in 2..12u32 {
            let mut seen = vec![false; n as usize];
            for _ in 0..DRAWS {
                seen[c.pick_child(&mut st, n, 0.9) as usize] = true;
            }
            let missing: Vec<usize> = seen
                .iter()
                .enumerate()
                .filter(|(_, s)| !**s)
                .map(|(i, _)| i)
                .collect();
            assert!(
                missing.is_empty(),
                "every one of {n} children must be reachable; never picked {missing:?}"
            );
        }
    }

    #[test]
    fn a_segments_per_segment_skew_is_read_and_an_absent_one_falls_back() {
        // `Segment.skew` has been in the schema since the profile spelling shipped and
        // NOTHING read it — `Profile` kept only `(from_depth, fanout)`. So a document could
        // state a child law and be generated under a different one, silently. Measured
        // before this fix: `skew: 0.0` and `skew: 3.0` gave byte-identical streams.
        let seg = |skew: Option<f64>| {
            corpus(Branching::Profile(vec![Segment {
                from_depth: 0,
                fanout: 4.0,
                skew,
                churn_half_life: None,
            }]))
        };
        assert_eq!(
            seg(Some(0.0)).skew_at(1),
            0.0,
            "an override of 0 is uniform"
        );
        assert_eq!(seg(Some(3.0)).skew_at(1), 3.0);
        assert_eq!(
            seg(None).skew_at(1),
            0.9,
            "an absent override defers to the document's branch_skew"
        );
        // And the law reaching the walk really differs, not just the accessor: uniform
        // descent over four children takes the head a quarter of the time, s = 3 nearly
        // always.
        let head = |c: &Corpus| {
            let mut st = Stream::new(3, 3);
            let n = 4;
            let hits = (0..8000)
                .filter(|_| c.pick_child(&mut st, n, c.skew_at(1)) == 0)
                .count();
            hits as f64 / 8000.0
        };
        let flat = head(&seg(Some(0.0)));
        let steep = head(&seg(Some(3.0)));
        assert!(
            (flat - 0.25).abs() < 0.02,
            "skew 0 must be uniform over 4 children, got {flat:.4}"
        );
        assert!(
            steep > 0.8,
            "skew 3 must concentrate on the head, got {steep:.4}"
        );
    }

    #[test]
    fn a_bands_skew_reaches_the_walk_and_only_its_own_depths() {
        // The node-level spelling carries its skew through `mean_profile`, which is the one
        // path `Corpus::skew_at` consults, so a two-band document must apply each band's law
        // at that band's depths and nowhere else. Without this a fitted per-band law would be
        // accepted and ignored exactly as `Segment.skew` was.
        use crate::schema::{SegmentBand, SegmentProcess};
        let c = corpus(Branching::Segments(SegmentProcess {
            by_depth: vec![
                SegmentBand {
                    from_depth: 0,
                    length: Dist::Scalar(4.0),
                    out_degree: Dist::Scalar(16.0),
                    singleton_share: None,
                    skew: Some(2.5),
                },
                SegmentBand {
                    from_depth: 8,
                    length: Dist::Scalar(4.0),
                    out_degree: Dist::Scalar(16.0),
                    singleton_share: None,
                    skew: Some(0.0),
                },
            ],
        }));
        assert_eq!(c.skew_at(0), 2.5);
        assert_eq!(c.skew_at(7), 2.5, "the first band holds up to the second");
        assert_eq!(c.skew_at(8), 0.0);
        assert_eq!(c.skew_at(99), 0.0, "the last band holds to the end");
        // The bands are otherwise identical, so any difference in realised cohort decay is
        // the law's alone: at a 16-way split s = 2.5 keeps most of the cohort on the head
        // while s = 0 divides it sixteen ways.
        let decay = |depth: u32| {
            let mut st = Stream::new(9, 9);
            let mut state = SplitState::at_root(&c, c.root_key(0, Generation::STABLE), u32::MAX);
            state.next_split_depth = depth;
            let (_, p) = c.trunk_step_stateful(
                c.root_key(0, Generation::STABLE),
                depth,
                &mut state,
                &mut st,
                Generation::STABLE,
            );
            p
        };
        assert!(
            decay(4) > decay(12),
            "the skewed band must divide the cohort less than the uniform one: {} vs {}",
            decay(4),
            decay(12)
        );
        assert!(
            (decay(12) - 1.0 / 16.0).abs() < 1e-12,
            "a uniform 16-way split divides by exactly 16, got {}",
            decay(12)
        );
    }
}
