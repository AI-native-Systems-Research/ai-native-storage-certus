//! The workload document: four orthogonal sections and nothing else.
//!
//! `corpus` (what keys exist and how they overlap), `workload` (who asks for
//! what, when), `topology` (which node asks for what), `run` (execution and
//! measurement). Normative reference: `contracts/workload-schema.md`.
//!
//! There is deliberately **no `system:` section**. Capacities, eviction policy,
//! watermarks, pinning and the placement of copies are properties of whatever
//! *consumes* a workload, not of the workload, and a document that named them
//! would stop meaning the same thing across consumers. A stale document carrying
//! one is rejected with a message saying where the quantity went, because such a
//! document is a likely input rather than a typo.
//!
//! Every struct is `deny_unknown_fields`: a mistyped parameter must not silently
//! take a default (spec FR-005).

pub mod extends;
pub mod normalise;
pub mod validate;

use serde::{Deserialize, Serialize};

use crate::dist::Dist;

/// The measurement window used when `run.wss_window` is not stated.
///
/// The worked example's value. It is a real default rather than a fallback, and it
/// is named here so that the occupancy floor and the generator cannot disagree
/// about it: occupancy is sessions per path *per window*, so it scales linearly
/// with this number, and two different defaults would make a document pass
/// validation and then be generated against a different check than the one it
/// passed. Where a finding is reported against the default, it says so.
pub const DEFAULT_WSS_WINDOW_REQUESTS: u64 = 240_000;

/// Whether the window came from the document or from the default.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WindowSource {
    /// Written in the document, as a count.
    Stated,
    /// Written as a duration and converted through the configured rate.
    ConvertedFromDuration,
    /// Not written; [`DEFAULT_WSS_WINDOW_REQUESTS`].
    Defaulted,
}

/// Resolve `run.wss_window` to a request count (spec FR-009h).
///
/// The window is canonically a **count**; a duration is sugar and needs a rate to
/// convert, which only `open_loop` supplies. One implementation, used by both the
/// occupancy floor and the report, for the same reason the default itself is
/// shared: a document that passed validation against one window and was then
/// characterised against another would have been measured by a different check
/// than the one it passed.
pub fn wss_window_requests(d: &Document) -> Result<(u64, WindowSource), String> {
    let v = match d.run.wss_window.as_ref() {
        None => return Ok((DEFAULT_WSS_WINDOW_REQUESTS, WindowSource::Defaulted)),
        Some(v) => v,
    };
    if let Some(n) = crate::units::count_from_yaml(v) {
        return match n {
            0 => Err("run.wss_window is zero".into()),
            n => Ok((n, WindowSource::Stated)),
        };
    }
    let ns = v
        .as_str()
        .and_then(|s| crate::units::parse_duration_ns(s).ok())
        .ok_or_else(|| "run.wss_window is neither a request count nor a duration".to_string())?;
    let rate = d
        .workload
        .arrival
        .rate
        .as_deref()
        .and_then(|s| crate::units::parse_rate_per_s(s).ok())
        .ok_or_else(|| {
            "run.wss_window is a duration and no rate is configured to convert it".to_string()
        })?;
    let n = (ns as f64 / 1e9 * rate) as u64;
    if n == 0 {
        return Err("run.wss_window converts to zero requests".into());
    }
    Ok((n, WindowSource::ConvertedFromDuration))
}

/// A complete workload document.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Document {
    /// Schema version; the generator refuses versions it does not implement.
    pub version: u32,
    /// Every random draw derives from this.
    pub seed: u64,
    /// Optional preset to deep-merge beneath this document.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub extends: Option<String>,
    /// Run length: exactly one of these four.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub duration: Option<String>,
    /// Run length as a request count.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub requests: Option<u64>,
    /// Run length as a block count. **Required** for file output, because it is
    /// the only one that converts directly to a file size.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub blocks: Option<u64>,
    /// Run until stopped. Direct-to-server only; nothing accumulates on disk.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub unbounded: Option<bool>,
    /// What keys exist and how they overlap.
    pub corpus: Corpus,
    /// Who asks for what, when.
    pub workload: Workload,
    /// Which node asks for what. Omitted means a single node asks for everything.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub topology: Option<Topology>,
    /// Execution and measurement.
    pub run: Run,
    /// The experiment matrix.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sweep: Option<Sweep>,
}

/// What keys exist and how they overlap.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Corpus {
    /// Payload size per key. A pure function of key identity (spec FR-011).
    pub block_bytes: Dist,
    /// The forest.
    pub trees: Trees,
}

/// A forest of prefix trees, not a single tree.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Trees {
    /// The depth-0 keys and how sessions bind to them.
    pub roots: Roots,
    /// Depth at which inter-session sharing ends.
    pub shared_depth: Dist,
    /// How the trunk widens with depth. Piecewise, not a single exponent.
    #[serde(default)]
    pub branching: Branching,
    /// Zipf exponent over child rank; 0 is uniform.
    #[serde(default = "default_branch_skew")]
    pub branch_skew: f64,
    /// How shared content is replaced over time. Absent means an immortal trunk.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub churn: Option<Churn>,
}

/// The `branch_skew` a document takes when it does not state one.
///
/// Public because `fit` omits `branch_skew` — its fitting procedure is an open
/// derivation — and must construct a document carrying the schema's own default
/// rather than a value of its choosing.
pub fn default_branch_skew() -> f64 {
    0.9
}

/// The depth-0 keys.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Roots {
    /// Number of distinct depth-0 keys.
    pub count: u32,
    /// Which root a session binds to; drawn once per session and then sticky.
    pub popularity: Dist,
}

/// How the trunk widens with depth.
///
/// A bare scalar is sugar for one uniform segment at depth 0 — the old
/// `branch_factor`, and still right for a smoothly-branching trunk. `auto`
/// resolves to a uniform profile by the FR-009g closed form.
///
/// A profile is offered rather than a scalar because measured tries are flat for
/// long stretches and then fan out at particular depths: a scalar's shape is not
/// a coarse approximation of the real one, it is a different shape.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(untagged)]
pub enum Branching {
    /// `auto` — solve for a uniform profile keeping sharing realisable.
    Auto(AutoTag),
    /// A single uniform fanout at every depth.
    Uniform(f64),
    /// Piecewise: each entry holds from its depth until the next.
    Profile(Vec<Segment>),
    /// A node-level branching process: **where** splits are, not just how wide.
    Segments(SegmentProcess),
}

/// The trunk as a process over nodes rather than a fanout per depth.
///
/// # Why this spelling exists
///
/// The other three spellings are functions of **depth**: they say what the fanout is at
/// depth `d`, so every node at that depth fans out the same way. Real traces are not like
/// that. Measured preamble lengths are per-root and multi-modal — `appworld` has cohorts
/// splitting at 23, 3194 and 5556 blocks, `browsecompplus` at 1, 141 and 939 — so a
/// depth-indexed profile must fan out at depth 141 for *every* root or for none, and cannot
/// express the shape any of the corpus's six regimes actually has.
///
/// Here a node draws **how far the unary run continues** and **how many children end it**,
/// from its own stream. Per-root variation then comes for free, because each root draws its
/// own run length. Three further properties matter:
///
/// * **It subsumes the older spellings.** `length` const 1 with `out_degree` const `f` is
///   exactly `Branching::Uniform(f)`.
/// * **Nothing is extrapolated.** A depth-indexed profile has to say something about depths
///   past the deepest it was fitted from, and that extrapolation is where a clipped-ratio
///   product blew model width up by 5e23 on one real trace, which is why a retention floor
///   had to exist at all. A node-level process generates unbounded depth from the same
///   distributions with nothing extended.
/// * **`out_degree` is the TOTAL, singletons included.** That is what makes a session go
///   private: a split with 4739 children of which 483 are shared lets a session land on a
///   child nobody else took and be alone from there on. A profile fitted to *shared* width
///   cannot say it, and a generator without it produced a corpus in which nothing was
///   private, against traces where 95% of nodes are.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SegmentProcess {
    /// Bands by depth, ascending, the first starting at 0.
    ///
    /// Banded because the conditioning is real and per-trace: measured, the relationship
    /// between a segment's length and its depth differs between traces **in sign** (rho
    /// -0.60 on `ragbench`, -0.52 on `tau2_airline`, -0.01 on `qwen_code`, +0.45 on
    /// `exgentic_swebench`), and `qwen_code`'s per-band medians run 28, 9, 18, 23, 37, 13 —
    /// non-monotonic, and invisible to any single coefficient. A banded empirical table is
    /// the honest form; a fitted trend would be a shape no trace supports.
    pub by_depth: Vec<SegmentBand>,
}

/// One depth band of a [`SegmentProcess`].
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SegmentBand {
    /// This band holds from here until the next.
    pub from_depth: u32,
    /// Blocks a unary run continues before the next split. Drawn per split node.
    pub length: Dist,
    /// Children at the split that ends the run — **total**, singletons included.
    pub out_degree: Dist,
    /// Zipf exponent over child rank at this band's splits; overrides `branch_skew`.
    ///
    /// `out_degree` and this are a **pair**, and stating one without the other is worse than
    /// stating neither: `out_degree` says how many children exist, this says how a session
    /// chooses among them, and only the product of the two decides how fast a cohort
    /// subdivides. Measured — `qwen_code`'s root splits 4739 ways with 0.496 of sessions on
    /// its top child, where the document-level default of 0.9 puts 0.072 — so a model given
    /// the measured out-degree and the default law scattered sessions ~7x faster than the
    /// trace and its realised sharing collapsed.
    ///
    /// Fitted as a scalar rather than as a rank curve because the generator's arithmetic is
    /// `cohort *= p(child taken)`, whose expectation is `Σ p²`: that single number is the
    /// child law's whole effect on cohort decay, and matching it recovers the head share as a
    /// consequence (0.464 against the measured 0.496 on that root). See FR-055j.
    ///
    /// Still NO `n_eff_frac` here: it is a different parameterisation of this same quantity,
    /// so accepting both would either double-count or leave one silently ignored — the exact
    /// defect this rework exists to remove, of which there were three.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub skew: Option<f64>,
    /// Fraction of arrivals at a split that land on a child **no other session takes**.
    ///
    /// The escape probability the trunk boundary is made of: a session landing on a singleton
    /// child is private from there, and under rolling-prefix identity can never rejoin the
    /// shared subtrie. Measured per band from the census, not derived from `skew`.
    ///
    /// It exists because a rank law cannot supply it. `skew` is fitted to the collision
    /// probability, whose justification was that the tail it ignores does not affect cohort
    /// decay — true while a drawn `shared_depth` bounded the trunk, and false once cohort
    /// exhaustion does, because the tail is precisely where sessions leave. Measured on
    /// `qwen_code`, 24.8% of requests share one block or less against 1.3% under a Zipf that
    /// matches the head exactly.
    ///
    /// Unset means no escape, which keeps every existing document's stream byte-identical: with
    /// `None` the walk draws nothing at all.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub singleton_share: Option<f64>,
}

impl Default for Branching {
    fn default() -> Self {
        Branching::Auto(AutoTag::Auto)
    }
}

/// The literal `auto`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AutoTag {
    /// Solve for the profile rather than stating one.
    Auto,
}

/// One segment of a branching profile.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Segment {
    /// This fanout holds from here until the next segment.
    pub from_depth: u32,
    /// Mean children per trunk node. Realised by randomised rounding keyed on
    /// the node, so width is stochastic and averages this value.
    pub fanout: f64,
    /// Overrides `branch_skew` for this segment.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub skew: Option<f64>,
    /// Overrides `churn.half_life` for this segment.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub churn_half_life: Option<String>,
}

/// Replacement of shared content over time.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Churn {
    /// `0` or absent means never: shared content, once minted, exists forever.
    pub half_life: String,
}

/// Who asks for what, when.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Workload {
    /// How requests arrive.
    pub arrival: Arrival,
    /// Population defaults; `mix` entries override individual fields.
    pub sessions: Sessions,
    /// Weighted mixture over the same session model.
    #[serde(default)]
    pub mix: Vec<MixEntry>,
    /// Non-stationary root popularity.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub drift: Option<Drift>,
}

/// How requests arrive.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Arrival {
    /// `open_loop` (default) or `closed_loop`.
    pub model: ArrivalModel,
    /// `open_loop` only.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rate: Option<String>,
    /// Index of dispersion; 1.0 reproduces Poisson.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub burstiness: Option<f64>,
    /// `closed_loop` only: bounded in-flight sessions.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub concurrency: Option<u32>,
}

/// Open or closed loop.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ArrivalModel {
    /// Absolute timestamps from a configured rate. The default.
    OpenLoop,
    /// Bounded concurrency, reactive. Arrival depends on system response.
    ClosedLoop,
}

/// The session model. The only behavioural unit.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Sessions {
    /// Requests per session; 1 is a one-shot.
    pub turns: Dist,
    /// Gap between turns.
    pub think_time: Dist,
    /// Turn-1 path depth below the shared trunk, private to this session.
    pub private_depth: Dist,
    /// Blocks added by each turn after the first. Drawn per turn.
    ///
    /// Either one distribution, or one **per session-length band** — see [`Growth`].
    pub growth_per_turn: Growth,
    /// Ceiling on a session's path depth, in blocks — the context window.
    ///
    /// A prompt cannot exceed the model's context window, so a conversation that runs
    /// many turns must grow more slowly per turn than a short one: it is pressed
    /// against a ceiling. Without this, `growth_per_turn` accumulates without bound and
    /// a long session's path grows linearly in its turn count forever.
    ///
    /// It was measured before it was added (FR-054c). A session's accumulated depth is
    /// `Σᵢ (T − i)·gᵢ`, so an increment is inherited by every later turn and the total
    /// is quadratic in the turn count; an unbounded i.i.d. draw overstated it by
    /// **1.478x and 1.545x** on two agentic traces. In those traces mean final depth
    /// plateaus — around 1250 blocks whether a session runs 27 turns or 85 — and one
    /// ceiling of **1400 blocks reproduced the accumulation to 1.028x and 0.976x on
    /// both**, which is why this is a ceiling rather than a per-session growth rate
    /// conditioned on turn count. The same number fitting two traces is the difference
    /// between a mechanism and a fudge factor; and a per-session rate drawn from its
    /// own marginal was measured *worse* than the i.i.d. draw (1.590x), because the
    /// unweighted mean of session rates exceeds the pooled mean of increments.
    ///
    /// **A distribution, drawn once per session, not a single value.** A single
    /// ceiling was measured and rejected: it fixed the mean (synthetic references came
    /// to 1.021x the trace's, from 1.210x) and **broke the shape**, piling 1923
    /// requests into the bucket at the ceiling where the trace had 173 and emptying
    /// the tail above it, which took `request_length` from 0.058 to 0.108. Real
    /// sessions top out all over the place — p50 592, p90 1536, p99 2176, max 3583
    /// blocks on one trace — so one number cannot describe where they stop.
    ///
    /// Drawing it per session is sound even though a session's *observed* maximum is
    /// usually just where its conversation ended rather than where a window bound it:
    /// a ceiling drawn above what a session would have reached has no effect at all,
    /// so over-estimating it for a short session costs nothing, and only the sessions
    /// that actually saturate are sensitive to the value.
    ///
    /// **Unset means unbounded**, so a document written before this existed generates
    /// exactly the stream it always did.
    ///
    /// Not overridable per mixture arm: a context window is a property of the model
    /// being served rather than of one arm of a workload.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_depth: Option<Dist>,
    /// Agent fan-out. Disabled by default.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub spawn: Option<Spawn>,
}

/// Per-turn path growth: one distribution, or one per session-length band.
///
/// A bare distribution — `{dist: lognormal, median: 6, sigma: 0.5}`, or the scalar
/// sugar `6` — means every session draws its growth from the same place. That is the
/// original behaviour and remains the default spelling.
///
/// # Why a session's length has to select the distribution
///
/// Because a session's accumulated depth is `Σᵢ (T − i)·gᵢ`: the increment at position
/// `i` is inherited by every turn after it, so it enters with weight `T − i`, and
/// summed over a session that weight is **quadratic in the turn count**. One
/// distribution for every session is therefore only correct if growth rate does not
/// vary with session length — and in real agentic traces it varies a great deal, and
/// **non-monotonically**: measured on two of them the rate climbs from about 21 blocks
/// per turn at 2–3 turns to 37–38 around 8–16 turns, then falls away to 3–9 beyond 96.
/// A conversation that runs very long is one that grows slowly, which is what lets it
/// run long.
///
/// The cost of ignoring it was measured (FR-054c): a single pooled distribution
/// accumulates **1.478x and 1.545x** the depth the traces actually have, which came out
/// as synthetic output running 1.2–1.6x long. Banding by turn count brings the same
/// arithmetic to **1.001x and 1.004x**.
///
/// A Pearson correlation between session length and rate reads only −0.12 and −0.25 on
/// those traces and invites exactly the wrong conclusion; the relationship rises and
/// then collapses, which is what a linear coefficient cannot see. Bands were chosen
/// over a fitted functional form for that reason: there is no monotone shape to fit.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(untagged)]
pub enum Growth {
    /// One distribution for every session, whatever its length.
    Uniform(Dist),
    /// One distribution per session-length band.
    Banded(GrowthBands),
}

/// `growth_per_turn` banded by session length.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GrowthBands {
    /// Ascending by `from_turns`; the applicable band is the last one whose
    /// `from_turns` does not exceed the session's turn count.
    pub by_turns: Vec<GrowthBand>,
}

/// One session-length band and the growth distribution sessions in it draw from.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GrowthBand {
    /// This band applies to sessions of at least this many turns, until the next band.
    pub from_turns: u32,
    /// Blocks added per turn by a session in this band.
    pub growth: Dist,
}

impl Growth {
    /// The distribution a session of `turns` turns draws its growth from.
    ///
    /// Resolved **once per session**, at birth, where the turn count is drawn — so
    /// everything downstream still sees a single [`Dist`] and FR-014a's path formula
    /// does not learn about banding.
    ///
    /// A turn count below the first band falls into the first band rather than
    /// producing nothing: bands describe where the population was observed, and a
    /// session shorter than anything observed is closer to the shortest band than to
    /// no growth at all.
    pub fn at(&self, turns: u64) -> &Dist {
        match self {
            Growth::Uniform(d) => d,
            Growth::Banded(b) => {
                let mut chosen = &b.by_turns[0].growth;
                for band in &b.by_turns {
                    if u64::from(band.from_turns) <= turns {
                        chosen = &band.growth;
                    } else {
                        break;
                    }
                }
                chosen
            }
        }
    }

    /// Every distribution this may resolve to, for checks that must cover all of them.
    pub fn distributions(&self) -> Vec<&Dist> {
        match self {
            Growth::Uniform(d) => vec![d],
            Growth::Banded(b) => b.by_turns.iter().map(|x| &x.growth).collect(),
        }
    }

    /// The mean over bands, unweighted, for a report that wants one number.
    ///
    /// Unweighted because the band weights depend on the `turns` distribution, which
    /// this type does not have; a caller needing the population mean should weight it
    /// itself rather than trusting this.
    pub fn mean(&self) -> Option<f64> {
        let ds = self.distributions();
        let means: Vec<f64> = ds.iter().filter_map(|d| d.mean()).collect();
        if means.is_empty() {
            return None;
        }
        Some(means.iter().sum::<f64>() / means.len() as f64)
    }
}

/// Agent fan-out: a session spawning children that inherit its context.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Spawn {
    /// Children per spawning session; 0 disables.
    #[serde(default)]
    pub fanout: u32,
    /// Fraction of sessions that spawn.
    #[serde(default)]
    pub probability: f64,
    /// Which turn triggers the spawn.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub at_turn: Option<Dist>,
    /// How much of the parent's prefix a child inherits.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub depth: Option<serde_yaml::Value>,
    /// 1 means children do not themselves spawn.
    #[serde(default = "one")]
    pub generations: u32,
    /// Where children land.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub placement: Option<SpawnPlacement>,
}

fn one() -> u32 {
    1
}

/// Where a spawned child runs.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SpawnPlacement {
    /// Anywhere but the parent's node. The default, and the point of fan-out.
    OtherNodes,
    /// Uniform over all nodes.
    Any,
    /// The parent's node.
    SameNode,
}

/// One entry of the mixture: a parameter set, not a behavioural mode.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MixEntry {
    /// Normalised across entries; not required to sum to 1.
    pub weight: f64,
    /// Overrides `sessions.turns`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub turns: Option<Dist>,
    /// Overrides `sessions.think_time`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub think_time: Option<Dist>,
    /// Overrides `sessions.private_depth`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub private_depth: Option<Dist>,
    /// Overrides `sessions.growth_per_turn`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub growth_per_turn: Option<Growth>,
}

/// Non-stationary root popularity.
///
/// Re-weights **which** shared keys are popular. It never changes **which**
/// shared keys exist — that is `churn`, and the two must stay separate because a
/// popularity shift leaves a consumer's cached entries valid whereas a content
/// replacement invalidates them.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Drift {
    /// `0` (default) is stationary.
    pub half_life: String,
}

/// Which node asks for what.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Topology {
    /// Participating nodes.
    pub nodes: Vec<String>,
    /// How a session maps onto nodes.
    #[serde(default)]
    pub placement: Placement,
    /// How far the per-node request streams overlap.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub self_affinity: Option<f64>,
    /// How many distinct nodes ask for the same key.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub replication: Option<Replication>,
    /// Keys the warmup phase deliberately does not pre-request.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cold_fraction: Option<f64>,
    /// Scheduled membership changes.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub membership_events: Vec<MembershipEvent>,
}

/// How a session maps onto nodes.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Placement {
    /// A session binds to one node at birth, as it binds to one root, because a
    /// session's KV lives where it was computed. The default.
    #[default]
    Sticky,
    /// Each request placed independently. Makes a session remotely fetch its own
    /// earlier turns, which no deployment does; for deliberate comparison only.
    PerRequest,
}

/// How many distinct nodes ask for the same key.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Replication {
    /// Distinct asking nodes per key.
    pub nodes_per_key: Dist,
}

/// A scheduled node stop or start.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MembershipEvent {
    /// Absolute plan time.
    pub at: String,
    /// What happens.
    pub action: MembershipAction,
    /// Which node.
    pub node: String,
}

/// Stop or start.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MembershipAction {
    /// Take the node out.
    Stop,
    /// Bring it back.
    Start,
}

/// Execution and measurement.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Run {
    /// Where the generated requests go.
    pub mode: String,
    /// Endpoint pattern for the direct-to-server mode.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub endpoint_template: Option<String>,
    /// Keys per RPC.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub batch_size: Option<u32>,
    /// Client threads.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub workers: Option<u32>,
    /// Concurrent RPCs per worker.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub inflight: Option<u32>,
    /// One process-wide device allocation.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub gpu_buffer: Option<String>,
    /// Excluded from steady-state statistics. Must cover the session-population
    /// ramp, or the measured window opens on a partly-filled population.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub warmup: Option<String>,
    /// Explicit connection-warm phase before measuring.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub warm_connections: Option<bool>,
    /// Window for the working-set size and trunk occupancy. Canonically a
    /// **request count**; a duration is sugar and needs a rate to convert.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub wss_window: Option<serde_yaml::Value>,
    /// Preflight fails above this.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub clock_skew_bound: Option<String>,
    /// Optional human-readable trace, for debugging. Never an input.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub emit_trace: Option<String>,
}

/// The experiment matrix.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Sweep {
    /// Dotted paths into this document. A consumer-side quantity is not
    /// addressable here and never was a legitimate axis.
    pub axes: std::collections::BTreeMap<String, Vec<serde_yaml::Value>>,
    /// Repeats per point; 8 by default, because n = 3 has previously produced
    /// misleading conclusions on this bench.
    #[serde(default = "eight")]
    pub repeat: u32,
    /// `interleaved` (default) or `blocked`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub order: Option<String>,
}

fn eight() -> u32 {
    8
}

/// Why a document could not be read.
#[derive(Debug)]
pub enum SchemaError {
    /// The YAML did not parse, or did not match the schema.
    Yaml(serde_yaml::Error),
    /// A unit-suffixed scalar could not be read as its field's unit.
    ///
    /// Held apart from the serde error because it is a *better* error: it names
    /// the path and the accepted forms, where serde can only report that nothing
    /// matched an untagged enum.
    Unit(normalise::NormaliseError),
}

impl std::fmt::Display for SchemaError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            SchemaError::Yaml(e) => write!(f, "{e}"),
            SchemaError::Unit(e) => write!(f, "{e}"),
        }
    }
}

impl std::error::Error for SchemaError {}

impl From<serde_yaml::Error> for SchemaError {
    fn from(e: serde_yaml::Error) -> Self {
        SchemaError::Yaml(e)
    }
}

impl From<normalise::NormaliseError> for SchemaError {
    fn from(e: normalise::NormaliseError) -> Self {
        SchemaError::Unit(e)
    }
}

impl Document {
    /// Parse a document from YAML, rejecting unknown fields.
    ///
    /// Unit-suffixed scalars are normalised first (see [`normalise`]), so
    /// `block_bytes: 128KiB` and `block_bytes: 131072` are the same document.
    pub fn from_yaml(s: &str) -> Result<Document, SchemaError> {
        let v: serde_yaml::Value = serde_yaml::from_str(s)?;
        Document::from_value(v)
    }

    /// Parse a document from an already-merged YAML tree.
    ///
    /// The entry point for a caller that has resolved an `extends` chain itself.
    /// Normalisation happens **here** rather than in the caller so that there is
    /// exactly one way into a `Document` that skips it: none.
    pub fn from_value(mut v: serde_yaml::Value) -> Result<Document, SchemaError> {
        normalise::normalise(&mut v)?;
        Ok(serde_yaml::from_value(v)?)
    }

    /// Re-serialise, so a report can embed the normalised input.
    pub fn to_yaml(&self) -> Result<String, serde_yaml::Error> {
        serde_yaml::to_string(self)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The worked example from `contracts/workload-schema.md`, trimmed to the
    /// fields this phase parses. Using the contract's own example as a test is
    /// deliberate: it is the one document guaranteed to be kept current.
    const WORKED: &str = r#"
version: 1
seed: 0xC0FFEE
duration: 180s
corpus:
  block_bytes: 131072
  trees:
    roots:
      count: 12
      popularity: {dist: zipf, s: 0.9}
    shared_depth: {dist: empirical, points: [[4, 0.10], [18, 0.75], [40, 1.0]]}
    branching: auto
    branch_skew: 0.9
workload:
  arrival: {model: open_loop, rate: 4000/s, burstiness: 1.8}
  sessions:
    turns: {dist: geometric, mean: 6}
    think_time: {dist: lognormal, median: 3, sigma: 1.1}
    private_depth: {dist: lognormal, median: 8, sigma: 0.8}
    growth_per_turn: {dist: lognormal, median: 6, sigma: 0.5}
  mix:
    - {weight: 0.70}
    - {weight: 0.25, turns: 1}
    - {weight: 0.05, turns: 1, private_depth: 4000}
topology:
  nodes: [node2, node7, node9, node11]
  self_affinity: 0.25
  replication: {nodes_per_key: 1}
  cold_fraction: 0.05
run:
  mode: hardware
  batch_size: 64
  workers: 8
  warmup: 20s
  wss_window: 240000
"#;

    #[test]
    fn parses_the_contracts_worked_example() {
        let d = Document::from_yaml(WORKED).expect("worked example must parse");
        assert_eq!(d.version, 1);
        assert_eq!(d.corpus.trees.roots.count, 12);
        assert_eq!(d.workload.mix.len(), 3);
        assert_eq!(d.topology.as_ref().unwrap().nodes.len(), 4);
        // Placement defaults to sticky (FR-019a), not per-request.
        assert_eq!(d.topology.as_ref().unwrap().placement, Placement::Sticky);
        // Churn absent means an immortal trunk, which is the default.
        assert!(d.corpus.trees.churn.is_none());
        // Fan-out is off unless asked for (FR-018e).
        assert!(d.workload.sessions.spawn.is_none());
    }

    #[test]
    fn round_trips_through_yaml() {
        let d = Document::from_yaml(WORKED).unwrap();
        let again = Document::from_yaml(&d.to_yaml().unwrap()).unwrap();
        assert_eq!(again.corpus.trees.roots.count, d.corpus.trees.roots.count);
    }

    #[test]
    fn a_system_section_is_rejected() {
        // Rule 13: a stale document carrying the removed consumer-side section
        // is a likely input rather than a typo, so it must not be ignored.
        let y = WORKED.to_string() + "system:\n  capacity: {dram: {fraction_of_wss: 0.25}}\n";
        assert!(Document::from_yaml(&y).is_err());
    }

    #[test]
    fn an_unknown_field_is_rejected() {
        let y = WORKED.replace("branch_skew: 0.9", "branch_skewness: 0.9");
        assert!(Document::from_yaml(&y).is_err());
    }

    #[test]
    fn branching_accepts_auto_scalar_and_profile() {
        for b in [
            "auto",
            "1.18",
            "[{from_depth: 0, fanout: 1.0}, {from_depth: 12, fanout: 40.0}]",
        ] {
            let y = WORKED.replace("branching: auto", &format!("branching: {b}"));
            Document::from_yaml(&y).unwrap_or_else(|e| panic!("branching {b}: {e}"));
        }
    }

    #[test]
    fn sweep_repeat_defaults_to_eight() {
        let y = WORKED.to_string() + "sweep:\n  axes: {topology.self_affinity: [0.0, 1.0]}\n";
        let d = Document::from_yaml(&y).unwrap();
        assert_eq!(d.sweep.unwrap().repeat, 8);
    }
}
