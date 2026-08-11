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

/// A resolved fanout profile: ascending segments, the first at depth 0.
#[derive(Debug, Clone, PartialEq)]
pub struct Profile {
    segments: Vec<(u32, f64)>,
}

impl Profile {
    /// A uniform profile: one segment from depth 0.
    pub fn uniform(fanout: f64) -> Profile {
        Profile {
            segments: vec![(0, fanout)],
        }
    }

    /// A piecewise profile from explicit segments.
    pub fn from_segments(segs: &[Segment]) -> Profile {
        let mut segments: Vec<(u32, f64)> = segs.iter().map(|s| (s.from_depth, s.fanout)).collect();
        segments.sort_by_key(|(d, _)| *d);
        if segments.is_empty() {
            segments.push((0, 1.0));
        }
        Profile { segments }
    }

    /// The fanout in force at `depth`.
    pub fn fanout_at(&self, depth: u32) -> f64 {
        let mut f = self.segments[0].1;
        for (from, v) in &self.segments {
            if depth >= *from {
                f = *v;
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
    pub fn segments(&self) -> &[(u32, f64)] {
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

/// The realised shared structure.
#[derive(Debug, Clone)]
pub struct Corpus {
    /// Number of trees.
    pub roots: u32,
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
        let profile = match &trees.branching {
            Branching::Auto(_) => Profile::uniform(auto_fanout(
                sessions_per_window,
                trees.roots.count,
                trunk_steps,
            )),
            Branching::Uniform(f) => Profile::uniform(*f),
            Branching::Profile(segs) => Profile::from_segments(segs),
        };
        Corpus {
            roots: trees.roots.count,
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

    /// Which child a descending session picks, skewed by `branch_skew`.
    ///
    /// Keyed on the *session's* stream rather than the node's, because this is a
    /// choice the session makes; the node's own stream decides only how many
    /// children exist.
    pub fn pick_child(&self, st: &mut Stream, children: u32) -> u32 {
        if children <= 1 {
            return 0;
        }
        if self.branch_skew <= 0.0 {
            return st.next_below(u64::from(children)) as u32;
        }
        let d = Dist::Shaped(crate::dist::Shape::Zipf {
            s: self.branch_skew,
            n: Some(u64::from(children)),
        });
        // Zipf yields a 1-based rank; children are 0-based.
        (d.sample_u64(st).max(1) - 1).min(u64::from(children - 1)) as u32
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
        let idx = self.pick_child(st, n);
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
                assert!(c.pick_child(&mut st, n) < n);
            }
        }
    }
}
