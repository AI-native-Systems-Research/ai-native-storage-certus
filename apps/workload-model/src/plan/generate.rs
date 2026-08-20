//! Chunked look-ahead generation: the engine that turns a document into events.
//!
//! Generation is **pull-based and bounded**. A caller hands in a buffer and gets
//! back up to a horizon's worth of events; nothing else is retained. That single
//! shape satisfies two requirements which read as if they were in tension:
//!
//! - **FR-037** — events are pre-generated into a flat, allocation-free
//!   representation and never generated on the cores issuing requests.
//! - **FR-021f** — an unbounded run generates *ahead in bounded chunks* rather
//!   than materialising a whole plan.
//!
//! They are the same mechanism with a different budget: only the horizon is
//! finite. So an unbounded run is not an exception to the no-bottleneck claim,
//! and the horizon is [reported](Horizon) rather than buried, because a horizon
//! too short makes the generator the bottleneck FR-037 exists to prevent and a
//! horizon too long is what an unbounded run cannot afford.
//!
//! ## What bounds memory
//!
//! Resident state is the **live session population** and nothing else (FR-010).
//! The trie is never materialised: a key's identity is a hash of the path to it,
//! so each turn re-walks its own path from the root and keeps no part of it. A
//! session therefore costs a fixed number of bytes regardless of how deep its
//! path has grown or how long the run has been going, and the run length does not
//! appear in the memory bound at all.
//!
//! ## Where the path depth comes from
//!
//! [`crate::session::depth_at_turn`] states FR-014a's formula exactly once, and this
//! module does **not** restate it: it advances a session's depth by one growth
//! draw per turn, which is the same series accumulated incrementally rather than
//! recomputed. The equivalence is asserted in
//! `incremental_growth_matches_the_stated_formula` rather than trusted, because
//! two expressions of one formula is exactly the shape of drift.

use std::cmp::Ordering;
use std::collections::BinaryHeap;

use crate::corpus::Corpus;
use crate::dist::{Clamps, Dist, Shape};
use crate::keys::{entry_size, private_child, Generation, SessionId};
use crate::plan::record::{flags, PlanEvent};
use crate::rng::Stream;
use crate::schema::{ArrivalModel, Document, Placement};
use crate::session::{draw_params, Interarrival, Session};
use crate::units::{parse_duration_ns, parse_rate_per_s, UnitError};

/// Default look-ahead: 64Ki events, about 2.5 MiB of records.
///
/// Chosen to sit comfortably inside a last-level cache while being far more than
/// any issuing core can drain between refills. It is a default rather than a
/// constant of the design — the whole point of reporting the horizon is that it
/// can be tuned against a measurement.
pub const DEFAULT_HORIZON_EVENTS: usize = 64 * 1024;

/// Expected cohort size below which a session is treated as alone on the trunk.
///
/// "Shared" means two or more sessions, and that is exactly the threshold the trace-side
/// measurement uses: a key counts toward shared width when two sessions reached it. So a
/// branch whose expected cohort has fallen below 2 is one where this session would be the
/// only occupant, and the blocks below it are private in fact whatever the trunk says.
///
/// This is the mechanism that replaced a drawn `shared_depth` as the binding constraint.
/// It is an *expectation*, not a census — the generator stores nothing per node — so a
/// session can be wrong about being alone in either direction. What matters is that the
/// error is correlated with the branches it took rather than being an independent coin
/// flip per node, which is what an earlier design got wrong.
const COHORT_FLOOR: f64 = 2.0;

/// Whether a session declines a trunk run it cannot walk to the end of (FR-054k).
///
/// EXPERIMENT (`CERTUS_EXP_RUN_COMPLETION=1`), off by default. Read once.
fn run_completion() -> bool {
    static ON: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *ON.get_or_init(|| std::env::var("CERTUS_EXP_RUN_COMPLETION").is_ok_and(|v| v == "1"))
}

/// Whether a session's aloneness is **drawn** rather than thresholded at [`COHORT_FLOOR`].
///
/// EXPERIMENT (`CERTUS_EXP_COHORT_BERNOULLI=1`), off by default. Read once.
///
/// # The defect it addresses
///
/// An **expected** cohort cannot represent "two of these three went that way". With an expected
/// cohort `c` at a split and this session taking a child of probability `p`, the threshold test
/// `c·p < 2` declares the session alone — but the sessions that would have followed it are integers,
/// and the chance that *none* of the other `c − 1` took the same child is `(1 − p)^(c−1)`. At
/// `c = 3, p = 0.6` that is 0.16, so 84% of the time the session really does keep company, while the
/// threshold calls it private because `1.8 < 2`.
///
/// That matters because it is exactly where real traces keep most of their sharing. The measured
/// median cohort of a shared segment is **2 to 3 sessions** on every trace examined, over thousands
/// of segments — small groups walking long shared runs. Thresholding at 2 destroys sharing precisely
/// there, which is why the generated trunk has 3x too few shared segments on `qwen_code` with
/// cohorts 15-60x too thick: the model can only keep a cohort by keeping it *large*.
fn cohort_bernoulli() -> bool {
    static ON: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *ON.get_or_init(|| std::env::var("CERTUS_EXP_COHORT_BERNOULLI").is_ok_and(|v| v == "1"))
}

/// Whether cohort exhaustion is the **sole** trunk boundary, ignoring the drawn cap.
///
/// EXPERIMENT (`CERTUS_EXP_COHORT_BOUNDARY=1`), off by default. Read once — a per-step
/// `env::var` in the trunk walk would cost more than the walk itself.
///
/// # What this isolates, and why it is safe to try
///
/// `shared_depth` is doubly loaded: `session::depth_at_turn` makes turn-1 depth
/// `shared_depth + private_depth`, so the field is a **path-length term** as well as the
/// trunk boundary. An earlier attempt removed it as a boundary by emitting a deliberately
/// non-binding *value*, which inflated every path 3.7x and took `request_length` to 0.99 —
/// that is what found the double load.
///
/// This toggle does not touch the emitted document. It drops the cap **inside the walk**,
/// where the loop bound is the already-drawn total `depth`, so the number of blocks in a
/// request is bit-identical and only the trunk/private *boundary* moves. That isolates the
/// mechanism from the path-length refactor the full fix needs.
fn cohort_boundary_only() -> bool {
    static ON: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *ON.get_or_init(|| std::env::var("CERTUS_EXP_COHORT_BOUNDARY").is_ok_and(|v| v == "1"))
}

/// Domain separators for the generator's own draws, so that two unrelated
/// quantities about one session never consume each other's values.
const TAG_SESSION: u64 = 0x5E55_1014;
const TAG_TRUNK_WALK: u64 = 0x7204_4B01;
const TAG_GROWTH: u64 = 0x6407_0407;
const TAG_ARRIVAL: u64 = 0x4881_1A15;
const TAG_NODE: u64 = 0x0D0D_E101;
/// Domain for the singleton-escape draw at a split.
///
/// Its own stream so that a document stating a `singleton_share` does not shift the child-choice
/// draws of one that does not — the escape is an addition to the walk, not a reordering of it.
const TAG_ESCAPE: u64 = 0x0E5C_4BE0;
/// Domain for the "did anyone follow me?" draw at a split.
const TAG_COMPANION: u64 = 0xC011_4A10;
/// Domain for the **root-level** half of the two-level turn-1 path-length draw.
const TAG_ROOT_PATH: u64 = 0x4007_9A14;

/// How the run ends. Exactly one, which the schema's rule 19 enforces.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Budget {
    /// Stop once plan time passes this.
    Duration(u64),
    /// Stop after this many requests.
    Requests(u64),
    /// Stop after this many block references. The only unit that converts
    /// directly to a file size, which is why file output requires it (FR-021d).
    Blocks(u64),
    /// Run until stopped. Direct-to-server only (FR-021e).
    Unbounded,
}

impl Budget {
    /// Read the run length off a document.
    ///
    /// Rule 19 is a *validation* rule, so a document reaching here with none or
    /// several is a caller that skipped validation; this reports it rather than
    /// picking one.
    pub fn from_document(d: &Document) -> Result<Budget, GenError> {
        let mut found: Vec<Budget> = Vec::new();
        if let Some(s) = &d.duration {
            found.push(Budget::Duration(parse_duration_ns(s).map_err(|e| {
                GenError::Unit {
                    field: "duration",
                    err: e,
                }
            })?));
        }
        if let Some(n) = d.requests {
            found.push(Budget::Requests(n));
        }
        if let Some(n) = d.blocks {
            found.push(Budget::Blocks(n));
        }
        if d.unbounded.unwrap_or(false) {
            found.push(Budget::Unbounded);
        }
        match found.len() {
            1 => Ok(found[0]),
            n => Err(GenError::RunLength(n)),
        }
    }

    /// Whether this run has no end.
    pub fn is_unbounded(&self) -> bool {
        matches!(self, Budget::Unbounded)
    }
}

/// The look-ahead depth, reported because FR-021f requires it stated.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Horizon {
    /// Events per chunk.
    pub events: usize,
    /// Bytes of `events.bin` those events occupy.
    pub bytes: usize,
}

impl std::fmt::Display for Horizon {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "look-ahead horizon {} events ({} KiB)",
            self.events,
            self.bytes / 1024
        )
    }
}

/// Why a document could not be turned into a generator.
#[derive(Debug, PartialEq)]
pub enum GenError {
    /// A unit-suffixed scalar did not parse.
    Unit {
        /// Which field.
        field: &'static str,
        /// What was wrong with it.
        err: UnitError,
    },
    /// Not exactly one run length; rule 19.
    RunLength(usize),
    /// `open_loop` with no rate, or `closed_loop` with no concurrency.
    Arrival(&'static str),
}

impl std::fmt::Display for GenError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            GenError::Unit { field, err } => write!(f, "{field}: {err}"),
            GenError::RunLength(n) => write!(
                f,
                "exactly one of duration | requests | blocks | unbounded is required, found {n}"
            ),
            GenError::Arrival(m) => write!(f, "{m}"),
        }
    }
}

impl std::error::Error for GenError {}

/// A live session, plus the little state generation needs beyond [`Session`].
#[derive(Debug, Clone)]
struct Live {
    s: Session,
    /// The depth this session's **last** turn reaches, computed once at birth.
    ///
    /// The trunk is re-walked from the session id every turn, so a later turn traverses the same
    /// nodes to a greater depth, and a node is reached by this session if *any* of its turns reaches
    /// it. That makes the final turn's depth the right bound for "can this session finish the run it
    /// is about to join" (FR-054k) — turn 1's would make sessions leave the trunk far earlier than
    /// the trace's do. Computed from a **clone** of the growth stream so the per-turn draws are
    /// untouched, which keeps it exact rather than an estimate.
    final_depth: u32,
    /// How long a run on this session's root may be: the shortest path among its sessions.
    ///
    /// Per **root**, not per session, which is the property that matters — it caps a run without
    /// making two sessions at one node disagree about where that run ends. Capping at a session's own
    /// depth was tried and is wrong for exactly that reason: run length became session-dependent,
    /// walkers diverged mid-run, and the attrition it was meant to remove came straight back.
    ///
    /// Derived as the `1/(k+1)` quantile — the expected minimum of `k` draws — with `k` the root's
    /// expected session count. See [`Generator::run_cap_for`].
    run_cap: Option<u32>,
    /// This root's own path level and spread, kept so the walk can re-cap as the cohort
    /// subdivides. Resolved once at birth from `roots.turn1_path`, because the rank a level
    /// belongs to is known there and a level without its spread cannot state a quantile.
    root_path_level: Option<(u32, f64)>,
    /// A **ceiling** on how much of this session's path is shared trunk, drawn at birth.
    ///
    /// FR-012a always called the drawn value an upper bound on the realised one; since
    /// 2026-08-15 the binding constraint is the session's own cohort running out (see
    /// [`Live::root_cohort`]), and this only caps it. `fit` no longer emits it, so a
    /// fitted model predicts its sharing depth instead of being told it.
    shared_depth: u32,
    /// Expected sessions sharing this session's root — the head of the cohort product.
    ///
    /// `sessions per window x p(this root's rank)`. The walk multiplies it by
    /// `p(child taken)` at every branch, and where it falls below
    /// [`COHORT_FLOOR`] the session is statistically alone and continues privately.
    /// Nothing is stored per node: the estimate is carried down the walk.
    root_cohort: f64,
    /// Path depth of the turn about to be issued.
    depth: u32,
    /// Turn 1's depth, so the ceiling can never sit below it.
    ///
    /// `max_depth` bounds how far a conversation grows, not how much prefix it starts
    /// with: clipping turn 1 would shorten a *shared* prefix and let a conversation-
    /// length ceiling quietly edit the trunk.
    turn_one_depth: u32,
    /// The session's own growth stream, consumed one draw per turn so that the
    /// realised series is FR-014a's sum accumulated rather than recomputed.
    growth: Stream,
}

// Ordered by issue time, ties broken by session id. The tie-break is not
// cosmetic: `BinaryHeap` does not specify an order among equal elements, so
// without it two runs from the same seed could interleave equal-timestamped
// turns differently and the plan would not be reproducible.
impl Ord for Live {
    fn cmp(&self, other: &Self) -> Ordering {
        other
            .s
            .next_t_ns
            .cmp(&self.s.next_t_ns)
            .then_with(|| other.s.id.0.cmp(&self.s.id.0))
    }
}

impl PartialOrd for Live {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl PartialEq for Live {
    fn eq(&self, other: &Self) -> bool {
        self.s.next_t_ns == other.s.next_t_ns && self.s.id == other.s.id
    }
}

impl Eq for Live {}

/// Generates plan events in bounded chunks.
///
/// ```
/// use workload_model::plan::{Generator, PlanEvent};
/// use workload_model::schema::Document;
/// # let yaml = r#"
/// # version: 1
/// # seed: 7
/// # requests: 50
/// # corpus:
/// #   block_bytes: 131072
/// #   trees:
/// #     roots: {count: 4, popularity: {dist: zipf, s: 0.9}}
/// #     shared_depth: {dist: const, value: 6}
/// #     branching: 1.05
/// # workload:
/// #   arrival: {model: open_loop, rate: 1000/s}
/// #   sessions:
/// #     turns: {dist: const, value: 3}
/// #     think_time: {dist: const, value: 1}
/// #     private_depth: {dist: const, value: 2}
/// #     growth_per_turn: {dist: const, value: 1}
/// # run: {mode: hardware}
/// # "#;
/// let doc = Document::from_yaml(yaml).unwrap();
/// let mut g = workload_model::plan::Generator::new(&doc).unwrap();
/// let mut buf: Vec<PlanEvent> = Vec::new();
/// let mut total = 0;
/// while g.fill(&mut buf) > 0 {
///     total += buf.len();
/// }
/// assert!(total > 0);
/// assert_eq!(g.requests_emitted(), 50);
/// ```
#[derive(Debug)]
pub struct Generator {
    corpus: Corpus,
    document: Document,
    /// `roots.popularity` with its support fixed to `roots.count`, resolved once.
    root_popularity: Dist,
    /// Expected live sessions per occupancy window, the cohort estimate's numerator.
    sessions_per_window: f64,
    shared_depth: Dist,
    /// The measured turn-1 total path length, as a population marginal.
    ///
    /// Held here rather than taken from `SessionParams` alone because it is also the fallback for
    /// a document that states no per-root table, and because the run-length cap reads its
    /// quantiles.
    turn1_path_length: Option<Dist>,
    /// Each root's own turn-1 level and spread, indexed by rank − 1 (FR-054j).
    ///
    /// The root is bound in `birth`, after `draw_params` has run, so the per-root half of a
    /// session's path length is applied there.
    root_turn1: Option<crate::schema::RootTurn1>,
    seed: u64,
    nodes: u16,
    placement: Placement,
    warmup_ns: u64,
    budget: Budget,
    horizon: usize,
    arrival: Option<Interarrival>,
    concurrency: u32,

    live: BinaryHeap<Live>,
    next_arrival_ns: u64,
    next_session: u32,
    next_request: u32,
    clock_ns: u64,
    events: u64,
    requests: u64,
    total_bytes: u64,
    done: bool,
    clamps: Clamps,
}

impl Generator {
    /// Build a generator from a validated document.
    pub fn new(d: &Document) -> Result<Generator, GenError> {
        Generator::with_horizon(d, DEFAULT_HORIZON_EVENTS)
    }

    /// Build a generator with an explicit look-ahead.
    pub fn with_horizon(d: &Document, horizon_events: usize) -> Result<Generator, GenError> {
        let budget = Budget::from_document(d)?;
        let rate = match &d.workload.arrival.rate {
            Some(s) => Some(parse_rate_per_s(s).map_err(|e| GenError::Unit {
                field: "workload.arrival.rate",
                err: e,
            })?),
            None => None,
        };
        let (arrival, concurrency) = match d.workload.arrival.model {
            ArrivalModel::OpenLoop => {
                let rate = rate.ok_or(GenError::Arrival("open_loop arrival requires a `rate`"))?;
                let mean_turns = d.workload.sessions.turns.mean().unwrap_or(1.0).max(1.0);
                let burst = d.workload.arrival.burstiness.unwrap_or(1.0);
                // Sessions arrive; requests are what the rate counts, and a
                // session carries `mean_turns` of them (FR-015).
                (Some(Interarrival::new(rate / mean_turns, burst)), 0)
            }
            ArrivalModel::ClosedLoop => (
                None,
                d.workload.arrival.concurrency.ok_or(GenError::Arrival(
                    "closed_loop arrival requires `concurrency`, its bound on in-flight sessions",
                ))?,
            ),
        };
        let warmup_ns = match &d.run.warmup {
            Some(s) => parse_duration_ns(s).map_err(|e| GenError::Unit {
                field: "run.warmup",
                err: e,
            })?,
            None => 0,
        };
        // The occupancy window, needed only to resolve `branching: auto`. Resolved
        // through the same function the occupancy floor uses, so `auto` cannot
        // solve against a different window than the one the document was validated
        // against. The error cases are the ones validation rejects, so falling back
        // to the default here only affects a caller that skipped validation.
        let (window_requests, _) = crate::schema::wss_window_requests(d).unwrap_or((
            crate::schema::DEFAULT_WSS_WINDOW_REQUESTS,
            crate::schema::WindowSource::Defaulted,
        ));
        let mean_turns = d.workload.sessions.turns.mean().unwrap_or(1.0).max(1.0);
        let sessions_per_window = window_requests as f64 / mean_turns;
        // Fanout *steps* to the deepest shared node, one less than the depth: a
        // shared prefix of depth s spans ordinals 0..s (FR-014a).
        let trunk_steps = d
            .corpus
            .trees
            .shared_depth
            .quantile_u32(0.99)
            .saturating_sub(1);
        let corpus = Corpus::resolve(
            &d.corpus.trees,
            d.corpus.block_bytes.clone(),
            d.seed,
            sessions_per_window,
            trunk_steps,
        );
        let nodes = d
            .topology
            .as_ref()
            .map(|t| t.nodes.len().max(1))
            .unwrap_or(1)
            .min(u16::MAX as usize) as u16;
        let placement = d
            .topology
            .as_ref()
            .map(|t| t.placement)
            .unwrap_or(Placement::Sticky);
        Ok(Generator {
            corpus,
            document: d.clone(),
            root_popularity: with_support(
                &d.corpus.trees.roots.popularity,
                d.corpus.trees.roots.count,
            ),
            shared_depth: d.corpus.trees.shared_depth.clone(),
            turn1_path_length: d.workload.sessions.turn1_path_length.clone(),
            root_turn1: d.corpus.trees.roots.turn1_path.clone(),
            // Carried so a session's cohort estimate starts from the population it is
            // actually competing for sharing with, over the window every occupancy
            // quantity in this model is defined over (FR-009h).
            sessions_per_window,
            seed: d.seed,
            nodes,
            placement,
            warmup_ns,
            budget,
            horizon: horizon_events.max(1),
            arrival,
            concurrency,
            live: BinaryHeap::new(),
            next_arrival_ns: 0,
            next_session: 0,
            next_request: 0,
            clock_ns: 0,
            events: 0,
            requests: 0,
            total_bytes: 0,
            done: false,
            clamps: Clamps::default(),
        })
    }

    /// The look-ahead depth, for the report FR-021f requires.
    pub fn horizon(&self) -> Horizon {
        Horizon {
            events: self.horizon,
            bytes: self.horizon * crate::plan::record::RECORD_BYTES,
        }
    }

    /// The document this generator was built from, after defaulting.
    pub fn document(&self) -> &Document {
        &self.document
    }

    /// The resolved corpus, including what `branching: auto` chose.
    pub fn corpus(&self) -> &Corpus {
        &self.corpus
    }

    /// The run length in force.
    pub fn budget(&self) -> Budget {
        self.budget
    }

    /// Sessions born but not yet retired. The whole of the memory bound.
    pub fn live_sessions(&self) -> usize {
        self.live.len()
    }

    /// Events emitted so far.
    pub fn events_emitted(&self) -> u64 {
        self.events
    }

    /// Requests emitted so far.
    pub fn requests_emitted(&self) -> u64 {
        self.requests
    }

    /// Payload bytes referenced so far.
    pub fn total_bytes(&self) -> u64 {
        self.total_bytes
    }

    /// Plan time of the last event emitted.
    pub fn clock_ns(&self) -> u64 {
        self.clock_ns
    }

    /// Adjustments applied to drawn values, surfaced rather than hidden.
    pub fn clamps(&self) -> &Clamps {
        &self.clamps
    }

    /// Whether the budget is spent. Never true for an unbounded run.
    pub fn is_done(&self) -> bool {
        self.done
    }

    /// Fill `out` with up to a horizon's worth of events; returns how many.
    ///
    /// `out` is cleared and refilled, so a caller that keeps one buffer allocates
    /// exactly once however long the run is — which is what FR-037's
    /// allocation-free claim rests on.
    ///
    /// The horizon is a target rather than a hard cap in one case: a request
    /// whose key count exceeds the whole horizon is still emitted **whole**,
    /// because keys of a request are contiguous by contract
    /// (`contracts/plan-format.md`) and half a request is not a request.
    pub fn fill(&mut self, out: &mut Vec<PlanEvent>) -> usize {
        out.clear();
        if out.capacity() < self.horizon {
            out.reserve(self.horizon - out.capacity());
        }
        while !self.done && out.len() < self.horizon {
            // Whichever comes first: the next scheduled turn, or a new arrival.
            let next_turn = self.live.peek().map(|l| (l.s.next_t_ns, l.depth));
            let birth_first = match (&self.arrival, next_turn) {
                (Some(_), Some((t, _))) => self.next_arrival_ns <= t,
                (Some(_), None) => true,
                (None, _) => self.live.len() < self.concurrency as usize,
            };
            if birth_first {
                if !self.can_start(1) {
                    self.done = true;
                    break;
                }
                let at = match &self.arrival {
                    Some(_) => self.next_arrival_ns,
                    None => self.clock_ns,
                };
                if let Some(ia) = &self.arrival {
                    let mut st = Stream::new(self.seed ^ TAG_ARRIVAL, u64::from(self.next_session));
                    self.next_arrival_ns = at.saturating_add(ia.next_ns(&mut st));
                }
                let live = self.birth(at);
                self.live.push(live);
                continue;
            }
            let Some((_, depth)) = next_turn else {
                // Closed loop at full concurrency with nothing scheduled cannot
                // happen, but a `concurrency: 0` document would land here.
                self.done = true;
                break;
            };
            let keys = depth as usize + 1;
            if !self.can_start(keys as u64) {
                self.done = true;
                break;
            }
            if !out.is_empty() && out.len() + keys > self.horizon {
                // Leave the whole request for the next chunk.
                break;
            }
            let mut live = self.live.pop().expect("peeked");
            self.emit_request(&mut live, out);
            live.s.turn += 1;
            if !live.s.is_retired() {
                let think = (live.s.params.think_time_s * 1e9) as u64;
                live.s.next_t_ns = live.s.next_t_ns.saturating_add(think);
                let g = live.s.params.growth_per_turn.clone();
                // The ceiling of `depth_at_turn`, applied to the incremental form. The
                // draw happens either way so a capped session's stream position matches
                // an uncapped one's; only the accumulation stops. Never below the
                // session's turn-1 depth, which `live.depth` started at, so a path
                // cannot shrink and FR-014a's strict extension holds.
                let ceiling = live
                    .s
                    .params
                    .max_depth
                    .map(|c| c.max(live.turn_one_depth))
                    .unwrap_or(u32::MAX);
                live.depth = live
                    .depth
                    .saturating_add(g.sample_u64(&mut live.growth).min(u64::from(u32::MAX)) as u32)
                    .min(ceiling);
                self.live.push(live);
            }
            // Retirement is simply dropping it: its private keys are dead from
            // that moment (FR-014b), and nothing about it needs keeping.
        }
        out.len()
    }

    /// How long a run may be, given the cohort that will walk it (FR-054k).
    ///
    /// `None` unless the document states a `turn1_path_length`; the cap is meaningless without one.
    ///
    /// # The derivation
    ///
    /// A run is completed by every session on it only if it is no longer than the **shortest** of
    /// them, and the expected minimum of `k` draws sits at the `1/(k+1)` quantile. `k` is the cohort
    /// **at that depth**, not the root's whole population: near the root a run must suit every
    /// session on the root, while at depth 300 the cohort has subdivided to two or three and the
    /// bound is far weaker. Using the root's count everywhere was measured and capped the trunk
    /// before depth 512, where the trace has 35 shared segments.
    ///
    /// Taken against the root's OWN distribution where the document states one, so the cap is the
    /// low quantile of the paths that will actually walk this run rather than of the population's.
    /// With a per-root level and spread that is `level + z(q)·spread`, one step and no mixing.
    fn run_cap_for(&self, root_path_level: Option<(u32, f64)>, cohort: f64) -> Option<u32> {
        let (level, spread) = root_path_level?;
        let q = 1.0 / (cohort.max(1.0) + 1.0);
        if let Some(t) = &self.root_turn1 {
            let z = t.shape.quantile(q).unwrap_or(0.0);
            return Some(crate::session::turn1_about_root(level, spread, z));
        }
        let d = self.turn1_path_length.as_ref()?;
        Some(d.quantile(q).unwrap_or(0.0).clamp(0.0, f64::from(u32::MAX)) as u32)
    }

    /// [`Self::run_cap_for`] against the walk's current cohort, when FR-054k is in force.
    fn run_cap_at(&self, root_path_level: Option<(u32, f64)>, cohort: f64) -> Option<u32> {
        if !run_completion() {
            return None;
        }
        self.run_cap_for(root_path_level, cohort).map(|c| c.max(1))
    }

    /// Whether `keys` more block references fit inside the budget.
    fn can_start(&self, keys: u64) -> bool {
        match self.budget {
            Budget::Unbounded => true,
            Budget::Requests(n) => self.requests < n,
            Budget::Blocks(n) => self.events + keys <= n,
            Budget::Duration(ns) => {
                // A birth or a turn already scheduled past the end contributes
                // nothing, so the clock is read off whichever is next.
                let next = self
                    .live
                    .peek()
                    .map(|l| l.s.next_t_ns)
                    .unwrap_or(self.next_arrival_ns);
                next <= ns
            }
        }
    }

    /// Draw a new session and bind it to a root and a node.
    fn birth(&mut self, at_ns: u64) -> Live {
        let id = SessionId(self.next_session);
        self.next_session = self.next_session.wrapping_add(1);
        let mut st = Stream::new(self.seed ^ TAG_SESSION, u64::from(id.0));
        let params = draw_params(&self.document.workload, &mut st);
        // Rank is 1-based over a support of `roots.count`; index is 0-based.
        let roots = self.corpus.roots.max(1);
        let root_index =
            (self
                .root_popularity
                .sample_u64_clamped(&mut st, 1, u64::from(roots), &self.clamps)
                - 1) as u32;
        let shared_depth =
            self.shared_depth
                .sample_u64_clamped(&mut st, 0, u64::from(u32::MAX), &self.clamps)
                as u32;
        let node = match self.placement {
            // Sticky: bound once at birth, as the root is (FR-019a).
            Placement::Sticky => (st.next_below(u64::from(self.nodes))) as u16,
            // Drawn per request instead; the field below is then unused.
            Placement::PerRequest => 0,
        };
        // Turn-1 path length as a property of the ROOT (FR-054j). Measured, `eta²` is 0.99 on the
        // agentic traces: request length is very nearly a property of the root, and drawing it
        // independently is what let an 11-block request land on a root with a 124-block preamble
        // and fragment it. `rank` indexes the table `roots.popularity` ordered, and `root_index` is
        // already 0-based.
        // The ROOT's own path level. STATED by the document where it has a per-root table, and
        // only otherwise drawn from the population marginal.
        //
        // Stating it is the whole of FR-054j's correction. Drawing it here — a fresh draw from the
        // pooled distribution, per root — reproduces the between-root *correlation* while
        // redrawing *which levels exist*, and at the corpus's 18-27 roots that alone costs a KS
        // distance of 0.15-0.20 against a 0.004 sampling floor. Measured against ground truth with
        // a known root structure, no construction that resamples levels avoids it, and none is
        // needed: `popularity` already ranks the roots, so the level is data at a rank.
        // This root's level and the spread of its sessions about it, by rank. A rank past the end
        // of the table falls through to the marginal rather than to zero: the generator clamps a
        // drawn rank to `roots.count`, so it is unreachable on a document whose table spans its
        // own count, and a silent zero would be a path length of nothing.
        let rank = root_index as usize;
        let root_path_level = match (&self.root_turn1, &self.turn1_path_length) {
            (Some(t), _) if rank < t.level.len() => Some((
                t.level[rank].round().clamp(0.0, f64::from(u32::MAX)) as u32,
                t.spread.get(rank).copied().unwrap_or(0.0),
            )),
            (_, Some(d)) => {
                let mut rs = Stream::new(
                    self.seed ^ TAG_ROOT_PATH,
                    u64::from(root_index) ^ (u64::from(roots) << 32),
                );
                Some((d.sample_u64(&mut rs).min(u64::from(u32::MAX)) as u32, 0.0))
            }
            (_, None) => None,
        };
        let turn1 = match (&self.root_turn1, root_path_level) {
            // The session's own path length about its root's level: the standardised residual
            // scaled by that root's own spread. Both halves are measured, so nothing is fitted
            // here and eta² is reproduced rather than targeted.
            //
            // Drawn on the SESSION's stream, so two sessions on one root differ while the level
            // they differ about does not.
            (Some(t), Some((level, spread))) => {
                let mut zs = Stream::new(self.seed ^ TAG_ROOT_PATH, u64::from(id.0));
                Some(crate::session::turn1_about_root(
                    level,
                    spread,
                    t.shape.sample(&mut zs),
                ))
            }
            // No per-root table: the population marginal `draw_params` already drew.
            _ => params.turn1_path_length,
        };
        let mut growth = Stream::new(self.seed ^ TAG_GROWTH, u64::from(id.0));
        let depth = crate::session::depth_at_turn(
            shared_depth,
            params.private_depth,
            turn1,
            &params.growth_per_turn,
            1,
            params.max_depth,
            &mut growth,
        );
        // The head of the cohort product: how many sessions this root is expected to
        // hold. `p(rank)` comes from the same distribution the rank was drawn from, so a
        // skewed root layer gives a popular root a large cohort and a rare one a small
        // cohort — which is why a session on an unpopular root shares less, as in the
        // trace, without anything having to be told to it.
        let root_cohort = self.sessions_per_window * self.root_rank_p(root_index, roots);
        // The run-length cap: the SHORTEST path among the sessions that will walk this root
        // (FR-054k). Derived rather than tuned. A run is only completed by every session on it if it
        // is no longer than the shortest of them, and the expected minimum of `k` draws sits at the
        // `1/(k+1)` quantile — with `k` the root's expected session count, which is `root_cohort`,
        // already computed above from `roots.popularity`.
        //
        // Capping at the root's LEVEL instead was measured and is not enough: sessions vary around
        // the level (eta² of 0.99 still leaves ~11% within-root spread), so about half fall short,
        // decline the run, and collapse the realised preamble from 88 blocks to 1.
        let run_cap = self.run_cap_for(root_path_level, root_cohort);
        // The last turn's depth, from a clone so the real growth stream is not advanced.
        let final_depth = crate::session::depth_at_turn(
            shared_depth,
            params.private_depth,
            turn1,
            &params.growth_per_turn,
            params.turns,
            params.max_depth,
            &mut growth.clone(),
        );
        Live {
            final_depth,
            run_cap,
            root_path_level,
            turn_one_depth: depth,
            s: Session {
                id,
                node,
                root_index,
                params,
                turn: 1,
                next_t_ns: at_ns,
            },
            shared_depth,
            root_cohort,
            depth,
            growth,
        }
    }

    /// The probability that a session binds to the root at `index`.
    ///
    /// Read off `roots.popularity` rather than assumed uniform: the whole point of the
    /// cohort estimate is that an unpopular root holds few sessions. For an `empirical`
    /// rank distribution — what `fit` emits — the mass on a rank is the CDF step across
    /// it; for `zipf` it is the discrete pmf; anything else falls back to uniform, which
    /// is the honest answer when the shape carries no closed form here.
    fn root_rank_p(&self, index: u32, roots: u32) -> f64 {
        let rank = u64::from(index) + 1;
        match self.root_popularity.shape() {
            Shape::Zipf { s, n } => {
                crate::dist::zipf_pmf_at(s, n.unwrap_or(u64::from(roots)), rank)
            }
            Shape::Empirical { points } => {
                let v = rank as f64;
                let mut below = 0.0f64;
                let mut at = 0.0f64;
                for (pv, pc) in &points {
                    if *pv < v {
                        below = below.max(*pc);
                    }
                    if *pv <= v {
                        at = at.max(*pc);
                    }
                }
                (at - below).max(0.0)
            }
            _ => 1.0 / f64::from(roots.max(1)),
        }
    }

    /// Emit one turn's request: its whole path, in path order, contiguously.
    fn emit_request(&mut self, live: &mut Live, out: &mut Vec<PlanEvent>) {
        let request_id = self.next_request;
        self.next_request = self.next_request.wrapping_add(1);
        let t_ns = live.s.next_t_ns;
        self.clock_ns = self.clock_ns.max(t_ns);
        let gen = Generation::STABLE;
        let node = match self.placement {
            Placement::Sticky => live.s.node,
            Placement::PerRequest => {
                let mut st = Stream::new(self.seed ^ TAG_NODE, u64::from(request_id));
                st.next_below(u64::from(self.nodes)) as u16
            }
        };
        let warm = if t_ns < self.warmup_ns {
            flags::WARMUP
        } else {
            0
        };
        // A fresh stream per turn, keyed on the session, so every turn walks the
        // *same* trunk path. Turn n's path must be a strict prefix of turn n+1's
        // — a rolling-hash key rehashes everything below a changed prefix.
        let mut walk = Stream::new(self.seed ^ TAG_TRUNK_WALK, u64::from(live.s.id.0));
        let depth = live.depth;
        let mut cur = self.corpus.root_key(live.s.root_index, gen);
        // `depth` is a path **length in blocks**, so the ordinals it covers are
        // `0..depth` and the trunk's are `0..shared_depth`. Both bounds are
        // exclusive, and both used to be inclusive: a document asking for
        // `shared_depth: 4, private_depth: 30` got a 35-block path with 5 shared
        // levels, against the 34 and 4 that FR-014a's formula states ("~56
        // blocks" for the worked example). The root at ordinal 0 is shared
        // whatever `shared_depth` says, since every session bound to a root
        // traverses it — so a trunk of length 0 is not expressible, and
        // `shared_depth` of 0 and 1 both mean "the root and nothing below it".
        // The expected cohort, carried down rather than stored per node. Once a session is
        // alone it stays alone: a rolling-prefix key rehashes everything below a changed
        // prefix, so a path that has left the trunk cannot rejoin it at a deeper level.
        let mut cohort = live.root_cohort;
        let mut alone = false;
        // Where this root's first split is, drawn from the root's own stream — which is what
        // lets two roots have preambles of different lengths.
        // Capped at the session's own reach when FR-054k is in force: a run longer than the
        // requests walking it would be declined by every one of them, which empties the trunk
        // rather than lengthening it. All sessions on this root share a path level, so the cap is
        // still a property of the node and the trie stays consistent between walkers.
        let split_cap = match (run_completion(), live.run_cap) {
            (true, Some(cap)) => cap.max(1),
            _ => u32::MAX,
        };
        let mut split = crate::corpus::SplitState::at_root(&self.corpus, cur, split_cap);
        for d in 0..depth {
            if d > 0 {
                // The boundary is the EARLIER of cohort exhaustion and the drawn cap.
                //
                // Cohort exhaustion is the mechanism the trace's structure actually uses, and
                // it is live here — but on its own it cannot yet replace the cap, which is a
                // measured result rather than a caution. A fitted `branching` profile fits
                // the width of the SHARED subtrie (keys two or more sessions reached), and
                // that width is nearly flat, so the cohort almost never divides and nothing
                // ever becomes private: removing the cap made every block shared and minted
                // no private keys at all, against a trace where 95% of nodes are private.
                //
                // What creates privacy in the trace is *total* out-degree — a split with 4739
                // children of which only 483 are shared, so a session can land on a singleton
                // child and be alone from there. The per-depth shared-width profile cannot
                // express that, so cohort tracking can only become the sole boundary once the
                // segment spelling carries total out-degree. Until then the cap binds first
                // on any fitted document and this is a superset of the old behaviour.
                cur = if !alone && (cohort_boundary_only() || d < live.shared_depth) {
                    // One entry point for both trunk spellings. Under a node-level process
                    // it returns probability 1.0 inside a run and divides the cohort only at
                    // a real split, which is what makes a long run a shared segment rather
                    // than a slow fanout.
                    // Re-cap by the cohort actually here, not the root's whole population: a run
                    // is completed by the sessions still on it, and by depth 300 the cohort has
                    // subdivided. See `Generator::run_cap_for`.
                    if let Some(c) = self.run_cap_at(live.root_path_level, cohort) {
                        split.set_cap(c);
                    }
                    let (next, p) = self
                        .corpus
                        .trunk_step_stateful(cur, d, &mut split, &mut walk, gen);
                    // A split, and the band states how many arrivals land on a child no other
                    // session takes: draw whether this one did. That is where privacy comes from
                    // in a real trace — a split with 4739 children of which 483 are shared — and
                    // it is not derivable from the child law's rank curve, whose fit deliberately
                    // ignores the tail. Measured, `qwen_code` has 24.8% of requests sharing one
                    // block or less against 1.3% under a Zipf matching the head exactly.
                    //
                    // Only at a real split (`p < 1.0`): inside a run there is one child and no
                    // choice to escape through. A band stating no share draws nothing, which is
                    // what keeps existing streams byte-identical.
                    let escaped = if p < 1.0 {
                        match self.corpus.singleton_share_at(d) {
                            Some(q) if q > 0.0 => {
                                let mut esc = Stream::new(
                                    self.seed ^ TAG_ESCAPE,
                                    u64::from(live.s.id.0) ^ (u64::from(d) << 32),
                                );
                                esc.next_f64() < q
                            }
                            _ => false,
                        }
                    } else {
                        false
                    };
                    // Did anyone follow? Either drawn against `(1 - p)^(c-1)` — the chance that
                    // none of the other expected sessions took this child — or thresholded on the
                    // expected cohort, which cannot express a surviving pair. Only at a real
                    // split; inside a run the cohort is intact by construction.
                    let left_alone = if cohort_bernoulli() && p < 1.0 {
                        let others = (cohort - 1.0).max(0.0);
                        let none_followed = (1.0 - p).max(0.0).powf(others);
                        let mut st = Stream::new(
                            self.seed ^ TAG_COMPANION,
                            u64::from(live.s.id.0) ^ (u64::from(d) << 32),
                        );
                        st.next_f64() < none_followed
                    } else {
                        false
                    };
                    // DO NOT JOIN A RUN YOU CANNOT FINISH (FR-054k).
                    //
                    // In every trace examined a shared run ends by **branching**, never by sessions
                    // dropping off it: 0 attrition of 158 segments on `tau2_airline`, 0 of 85 on
                    // `swebench`. Every session on a run walks the whole run. The generator broke
                    // that on 32% of its runs, because a session whose path ends part-way along one
                    // stops emitting and every node below it loses a session.
                    //
                    // Rather than correlate ever more parameters until paths happen to be long
                    // enough, the session **declines** the run: if its deepest turn cannot reach the
                    // far end, it goes private at the split instead of part-way down. That makes the
                    // invariant hold by construction at every depth rather than in expectation, and
                    // it is decidable here from what the session already knows.
                    let cannot_finish = run_completion() && split.next_split() > live.final_depth;
                    cohort *= p;
                    if escaped
                        || left_alone
                        || cannot_finish
                        || (!cohort_bernoulli() && cohort < COHORT_FLOOR)
                    {
                        alone = true;
                    }
                    if escaped || cannot_finish {
                        // Landed on a child of its own, or declined the run: private from here, and
                        // a rolling-prefix key cannot rejoin the trunk below.
                        private_child(cur, live.s.id, d)
                    } else {
                        next
                    }
                } else {
                    private_child(cur, live.s.id, d)
                };
            }
            let mut f = warm;
            if d == 0 {
                f |= flags::REQUEST_START;
            }
            if d + 1 == depth {
                f |= flags::REQUEST_END;
            }
            let size = entry_size(cur, &self.corpus.block_bytes);
            out.push(PlanEvent {
                t_ns,
                key: cur,
                size,
                request_id,
                session_id: live.s.id,
                depth: d,
                turn: live.s.turn,
                node,
                mix_index: live.s.params.mix_index,
                flags: f,
            });
            self.events += 1;
            self.total_bytes += u64::from(size);
        }
        self.requests += 1;
    }
}

/// `roots.popularity` with its support fixed to `roots.count`.
///
/// Rule 8 rejects an `n` written into the document precisely because the support
/// is not the author's to choose — it is the number of roots — so it is supplied
/// here rather than defaulted to Zipf's own unbounded support.
fn with_support(d: &Dist, roots: u32) -> Dist {
    match d.shape() {
        Shape::Zipf { s, .. } => Dist::Shaped(Shape::Zipf {
            s,
            n: Some(u64::from(roots.max(1))),
        }),
        other => Dist::Shaped(other),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::keys::CacheKey;
    use crate::session::depth_at_turn;

    const BASE: &str = r#"
version: 1
seed: 0xC0FFEE
corpus:
  block_bytes: 131072
  trees:
    roots: {count: 8, popularity: {dist: zipf, s: 0.9}}
    shared_depth: {dist: const, value: 6}
    branching: 1.02
workload:
  arrival: {model: open_loop, rate: 4000/s}
  sessions:
    turns: {dist: const, value: 4}
    think_time: {dist: const, value: 0.5}
    private_depth: {dist: const, value: 3}
    growth_per_turn: {dist: const, value: 2}
topology:
  nodes: [node2, node7, node9, node11]
run:
  mode: hardware
  wss_window: 240000
"#;

    fn doc(run_length: &str) -> Document {
        Document::from_yaml(&format!("{run_length}\n{}", BASE.trim_start()))
            .expect("fixture must parse")
    }

    fn drain(g: &mut Generator) -> Vec<PlanEvent> {
        let mut all = Vec::new();
        let mut buf = Vec::new();
        while g.fill(&mut buf) > 0 {
            all.extend_from_slice(&buf);
        }
        all
    }

    /// Turn-1 path length per session, from a drained plan: the length of each session's
    /// lowest-numbered request, which is the quantity `fit` measures on the trace side.
    fn turn1_lengths(ev: &[PlanEvent]) -> std::collections::BTreeMap<u32, u32> {
        let mut first: std::collections::BTreeMap<u32, (u32, u32)> =
            std::collections::BTreeMap::new();
        let mut len: std::collections::BTreeMap<u32, u32> = std::collections::BTreeMap::new();
        for e in ev {
            *len.entry(e.request_id).or_insert(0) += 1;
            let slot = first
                .entry(e.session_id.0)
                .or_insert((e.request_id, e.request_id));
            if e.request_id < slot.0 {
                *slot = (e.request_id, e.request_id);
            }
        }
        first
            .into_iter()
            .map(|(s, (rid, _))| (s, len.get(&rid).copied().unwrap_or(0)))
            .collect()
    }

    /// Two roots of equal popularity, one turn each, and a population `turn1_path_length` of a
    /// flat 220 blocks — a value that is neither root's level, so a realised path length says
    /// unambiguously which of the two mechanisms produced it. With `table`, each root states its
    /// own level (40 and 400) and a spread of 1.
    fn two_root_doc(table: bool) -> String {
        let turn1_path = if table {
            "      turn1_path:
        level: [40, 400]
        spread: [1.0, 1.0]
        shape: {dist: empirical, points: [[-1.0, 0.5], [1.0, 1.0]]}
"
        } else {
            ""
        };
        format!(
            r#"
requests: 300
version: 1
seed: 0xC0FFEE
corpus:
  block_bytes: 131072
  trees:
    roots:
      count: 2
      popularity: {{dist: empirical, points: [[1, 0.5], [2, 0.5], [2, 1.0]]}}
{turn1_path}    shared_depth: {{dist: const, value: 6}}
    branching: 1.02
workload:
  arrival: {{model: open_loop, rate: 4000/s}}
  sessions:
    turns: {{dist: const, value: 1}}
    think_time: {{dist: const, value: 0.5}}
    private_depth: {{dist: const, value: 3}}
    turn1_path_length: {{dist: const, value: 220}}
    growth_per_turn: {{dist: const, value: 2}}
run:
  mode: hardware
  wss_window: 240000
"#
        )
    }

    #[test]
    fn a_stated_per_root_level_makes_path_length_a_property_of_the_root() {
        // FR-054j. Two roots with levels 40 and 400 and almost no spread: every session must
        // land near its own root's level, so the population splits into two clusters rather
        // than spreading over one marginal. That is `eta²` near 1 by construction, and it is
        // what a fresh draw per root cannot deliver — it reproduces the correlation while
        // redrawing which levels exist.
        let d = Document::from_yaml(two_root_doc(true).trim_start()).expect("fixture must parse");
        let mut g = Generator::new(&d).unwrap();
        let lengths: Vec<u32> = turn1_lengths(&drain(&mut g)).into_values().collect();
        assert!(
            lengths.len() > 50,
            "too few sessions to judge: {}",
            lengths.len()
        );
        // Every session sits within a block or two of one of the two stated levels, and none
        // sits at the population marginal of 220 that `turn1_path_length` states.
        for l in &lengths {
            let near_a = l.abs_diff(40) <= 2;
            let near_b = l.abs_diff(400) <= 2;
            assert!(
                near_a || near_b,
                "path length {l} is at neither root's level"
            );
        }
        assert!(
            lengths.iter().any(|l| l.abs_diff(40) <= 2),
            "no session on the short root"
        );
        assert!(
            lengths.iter().any(|l| l.abs_diff(400) <= 2),
            "no session on the long root"
        );
        // And the spread within a root is the stated one, not the gap between roots: the
        // short root's sessions must not reach anywhere near the long root's level.
        let short_max = lengths
            .iter()
            .filter(|l| l.abs_diff(40) <= 2)
            .max()
            .copied();
        assert!(
            short_max.unwrap_or(0) < 100,
            "within-root spread leaked the between-root one"
        );
    }

    #[test]
    fn a_document_without_a_per_root_table_draws_the_population_marginal() {
        // The table is optional, and every document written before FR-054j lacks it. Absent, the
        // population marginal must be what is realised — the SAME fixture with the table removed,
        // so the only difference is the mechanism under test. Comparing a table-less document
        // against itself would only restate determinism, which
        // `the_same_seed_gives_the_identical_stream_and_a_new_seed_does_not` already covers.
        let d = Document::from_yaml(two_root_doc(false).trim_start()).expect("fixture must parse");
        let mut g = Generator::new(&d).unwrap();
        let lengths: Vec<u32> = turn1_lengths(&drain(&mut g)).into_values().collect();
        assert!(lengths.len() > 50, "too few sessions: {}", lengths.len());
        // 220 is the stated marginal and belongs to neither root, so this distinguishes the two
        // mechanisms rather than merely observing that something was drawn.
        for l in &lengths {
            assert_eq!(*l, 220, "path length {l} is not the population marginal");
        }
    }

    #[test]
    fn a_request_is_contiguous_in_path_order_and_bracketed() {
        // `contracts/plan-format.md`: keys of one request are contiguous and in
        // path order, so a consumer batches by scanning to REQUEST_END.
        let mut g = Generator::new(&doc("requests: 40")).unwrap();
        let ev = drain(&mut g);
        let mut seen_requests = 0;
        let mut i = 0;
        while i < ev.len() {
            assert!(ev[i].has(flags::REQUEST_START), "request must open at {i}");
            let rid = ev[i].request_id;
            let sid = ev[i].session_id;
            let mut d = 0;
            loop {
                assert_eq!(ev[i].request_id, rid, "request not contiguous");
                assert_eq!(ev[i].session_id, sid);
                assert_eq!(ev[i].depth, d, "keys not in path order");
                if ev[i].has(flags::REQUEST_END) {
                    break;
                }
                d += 1;
                i += 1;
                assert!(i < ev.len(), "request never ended");
            }
            seen_requests += 1;
            i += 1;
        }
        assert_eq!(seen_requests, 40);
        assert_eq!(g.requests_emitted(), 40);
    }

    #[test]
    fn timestamps_are_non_decreasing_across_interleaved_sessions() {
        // The runner consumes the plan as a schedule, so this is a property of
        // the artifact rather than a convenience.
        let mut g = Generator::new(&doc("requests: 2000")).unwrap();
        let ev = drain(&mut g);
        assert!(ev.windows(2).all(|w| w[0].t_ns <= w[1].t_ns));
        // And sessions really do interleave, which is why session_id is stored
        // rather than derived from request_id grouping.
        let mut switched = 0;
        for w in ev.windows(2) {
            if w[0].session_id != w[1].session_id {
                switched += 1;
            }
        }
        assert!(switched > 10, "sessions never interleaved: {switched}");
    }

    #[test]
    fn each_turn_extends_the_previous_turns_path_exactly() {
        // The rolling-hash requirement: turn n's path is a strict prefix of turn
        // n+1's, so shared blocks stay shared as a session grows.
        //
        // Budgeted by *duration*, not by request count: with a 0.5s think time a
        // few hundred requests are all turn 1, because that is the population
        // ramp — the very transient FR-015b makes `warmup` cover.
        let mut g = Generator::new(&doc("duration: 3s")).unwrap();
        let ev = drain(&mut g);
        let mut by_turn: std::collections::BTreeMap<(u32, u16), Vec<CacheKey>> =
            std::collections::BTreeMap::new();
        for e in &ev {
            by_turn
                .entry((e.session_id.0, e.turn))
                .or_default()
                .push(e.key);
        }
        let mut compared = 0;
        for ((sid, turn), keys) in &by_turn {
            if let Some(next) = by_turn.get(&(*sid, turn + 1)) {
                assert!(next.len() > keys.len(), "a turn must add blocks");
                assert_eq!(&next[..keys.len()], &keys[..], "prefix changed under turn");
                compared += 1;
            }
        }
        assert!(compared > 20, "not enough multi-turn sessions: {compared}");
    }

    #[test]
    fn incremental_growth_matches_the_stated_formula() {
        // This module advances depth one draw per turn; FR-014a's formula lives
        // in session.rs. Two expressions of one formula is the shape of drift,
        // so the equivalence is asserted rather than trusted.
        let d = doc("requests: 200");
        // The drawn value, not the distribution: `depth_at_turn` takes the ceiling this
        // session got. The fixture states none, so this is `None` and the equivalence is
        // asserted for the uncapped path.
        let max_depth = d
            .workload
            .sessions
            .max_depth
            .as_ref()
            .map(|x| x.mean().unwrap_or(0.0) as u32);
        let mut g = Generator::new(&d).unwrap();
        let ev = drain(&mut g);
        let shared = 6u32;
        let private = 3u32;
        let growth = Dist::Scalar(2.0);
        // Counted in **blocks**, which is what the formula states: a path of depth
        // n occupies ordinals 0..n. Asserting the maximum ordinal instead would
        // pass for a path one block too long, which is exactly the defect this
        // test previously agreed with.
        let mut blocks: std::collections::BTreeMap<(u32, u16), u32> =
            std::collections::BTreeMap::new();
        let mut top: std::collections::BTreeMap<(u32, u16), u32> =
            std::collections::BTreeMap::new();
        for e in &ev {
            *blocks.entry((e.session_id.0, e.turn)).or_insert(0) += 1;
            let t = top.entry((e.session_id.0, e.turn)).or_insert(0);
            *t = (*t).max(e.depth);
        }
        assert!(!blocks.is_empty());
        for ((session, turn), realised) in &blocks {
            let stated = depth_at_turn(
                shared,
                private,
                None,
                &growth,
                *turn,
                max_depth,
                &mut Stream::new(g.seed ^ TAG_GROWTH, 0),
            );
            assert_eq!(*realised, stated, "turn {turn}");
            assert_eq!(
                top[&(*session, *turn)],
                stated - 1,
                "the deepest ordinal is one below the length"
            );
        }
    }

    #[test]
    fn the_shared_prefix_is_at_most_shared_depth_and_only_a_branch_point_shortens_it() {
        // The other half of FR-014a's arithmetic, and the defect the length check
        // above cannot see: a trunk one level too deep would still give every path
        // the right total.
        //
        // This asserted `common == shared_depth` for *every* pair on a root until
        // 2026-08-14, and it passed only because `branching` was inert. `child_count`
        // at a fitted fanout is 1 or 2, and the superseded `dist::zipf` returned
        // child 0 with probability 1 at two children, so every session on a root
        // walked one identical chain and no `branching` value could widen it. With a
        // real discrete Zipf the trunk branches where the profile says it does, which
        // is the whole point of the parameter — so the honest properties are:
        //
        // * the realised common prefix is **at most** `shared_depth` (FR-012a: the
        //   drawn value bounds the realised one), and at least the root;
        // * it is **exactly** `shared_depth` unless the pair passed a node with more
        //   than one child, and that node is nameable.
        //
        // The second clause is what keeps this from being a vacuous inequality: a
        // prefix may only be cut short by a genuine branch point.
        let mut g = Generator::new(&doc("requests: 400")).unwrap();
        let ev = drain(&mut g);
        let shared = 6usize;
        let mut paths: std::collections::BTreeMap<u32, Vec<CacheKey>> =
            std::collections::BTreeMap::new();
        for e in &ev {
            if e.turn == 1 {
                paths.entry(e.session_id.0).or_default().push(e.key);
            }
        }
        // Group turn-1 paths by their root, then compare within a group.
        let mut by_root: std::collections::BTreeMap<CacheKey, Vec<Vec<CacheKey>>> =
            std::collections::BTreeMap::new();
        for p in paths.values() {
            if let Some(root) = p.first() {
                by_root.entry(*root).or_default().push(p.clone());
            }
        }
        let mut compared = 0;
        let mut diverged_early = 0;
        for group in by_root.values() {
            for pair in group.windows(2) {
                let common = pair[0]
                    .iter()
                    .zip(pair[1].iter())
                    .take_while(|(a, b)| a == b)
                    .count();
                assert!(
                    (1..=shared).contains(&common),
                    "two sessions on one root shared {common} blocks, outside 1..={shared}"
                );
                if common < shared {
                    diverged_early += 1;
                    // The last key they agreed on must be a real branch point;
                    // `common` is a length, so it is also the depth of the step that
                    // parted them.
                    let last_agreed = pair[0][common - 1];
                    let n = g.corpus.child_count(last_agreed, common as u32);
                    assert!(
                        n > 1,
                        "the pair parted at depth {common} under a node with {n} child(ren): \
                         a shared prefix may only be cut short by a branch point"
                    );
                }
                compared += 1;
            }
        }
        assert!(compared > 5, "only {compared} pairs compared");
        // Both regimes must actually occur in the fixture, or the assertions above
        // are each half-untested. At `branching: 1.02` about 2% of trunk nodes have
        // two children, so a minority of pairs part early and most do not.
        assert!(
            diverged_early > 0 && diverged_early < compared,
            "{diverged_early} of {compared} pairs parted early: the fixture must exercise \
             both the full-prefix and the branch-point case"
        );
    }

    #[test]
    fn the_same_seed_gives_the_identical_stream_and_a_new_seed_does_not() {
        let a = drain(&mut Generator::new(&doc("requests: 500")).unwrap());
        let b = drain(&mut Generator::new(&doc("requests: 500")).unwrap());
        assert_eq!(a, b);
        let mut d2 = doc("requests: 500");
        d2.seed = 0xDECAF;
        let c = drain(&mut Generator::new(&d2).unwrap());
        assert_ne!(a, c);
        // The distributional shape survives the seed change even though the
        // realised keys do not (SC-003).
        assert!(
            a.len().abs_diff(c.len()) * 20 < a.len(),
            "event counts diverged: {} vs {}",
            a.len(),
            c.len()
        );
    }

    #[test]
    fn the_horizon_bounds_a_chunk_and_is_reported() {
        // FR-021f: only the horizon is finite, and it must be stated, since a
        // horizon too short makes the generator the bottleneck.
        let mut g = Generator::with_horizon(&doc("unbounded: true"), 1000).unwrap();
        assert_eq!(g.horizon().events, 1000);
        assert!(format!("{}", g.horizon()).contains("look-ahead horizon 1000 events"));
        let mut buf = Vec::new();
        for _ in 0..5 {
            let n = g.fill(&mut buf);
            assert!(n > 0 && n <= 1000, "chunk was {n}");
        }
        // An unbounded run never reports itself finished.
        assert!(!g.is_done());
        assert!(g.budget().is_unbounded());
    }

    #[test]
    fn refilling_reuses_one_allocation() {
        // What FR-037's allocation-free claim rests on: the buffer is cleared and
        // refilled, so a long run does not allocate per chunk.
        let mut g = Generator::with_horizon(&doc("unbounded: true"), 4096).unwrap();
        let mut buf = Vec::new();
        g.fill(&mut buf);
        let cap = buf.capacity();
        let ptr = buf.as_ptr();
        for _ in 0..20 {
            g.fill(&mut buf);
        }
        assert_eq!(buf.capacity(), cap, "capacity moved");
        assert_eq!(buf.as_ptr(), ptr, "buffer was reallocated");
    }

    #[test]
    fn a_request_longer_than_the_horizon_is_still_emitted_whole() {
        // Contiguity is a contract, so it wins over the horizon: half a request
        // is not a request.
        let y = BASE.replace(
            "private_depth: {dist: const, value: 3}",
            "private_depth: {dist: const, value: 500}",
        );
        let d = Document::from_yaml(&format!("requests: 4\n{}", y.trim_start())).unwrap();
        let mut g = Generator::with_horizon(&d, 16).unwrap();
        let mut buf = Vec::new();
        let n = g.fill(&mut buf);
        assert!(n > 16, "request was split at the horizon: {n}");
        assert!(buf[0].has(flags::REQUEST_START));
        assert!(buf[n - 1].has(flags::REQUEST_END));
    }

    #[test]
    fn memory_is_bounded_by_the_live_population_not_the_run_length() {
        // FR-010, measured as the generator's own resident state.
        let mut short = Generator::new(&doc("requests: 2000")).unwrap();
        let mut long = Generator::new(&doc("requests: 40000")).unwrap();
        let mut buf = Vec::new();
        let mut peak_short = 0;
        while short.fill(&mut buf) > 0 {
            peak_short = peak_short.max(short.live_sessions());
        }
        let mut peak_long = 0;
        while long.fill(&mut buf) > 0 {
            peak_long = peak_long.max(long.live_sessions());
        }
        assert!(
            peak_long < peak_short * 2,
            "live population grew with run length: {peak_short} -> {peak_long}"
        );
    }

    #[test]
    fn the_block_budget_is_never_exceeded() {
        // It converts directly to a file size, which is the whole reason file
        // output requires it (FR-021d), so overshooting would defeat the point.
        for budget in [50u64, 500, 5000] {
            let mut g = Generator::new(&doc(&format!("blocks: {budget}"))).unwrap();
            let ev = drain(&mut g);
            assert!(ev.len() as u64 <= budget, "{} > {budget}", ev.len());
            // And it stops at a request boundary, not mid-request.
            assert!(ev.last().unwrap().has(flags::REQUEST_END));
        }
    }

    #[test]
    fn a_duration_budget_stops_at_the_clock() {
        let mut g = Generator::new(&doc("duration: 2s")).unwrap();
        let ev = drain(&mut g);
        assert!(!ev.is_empty());
        assert!(ev.iter().all(|e| e.t_ns <= 2_000_000_000));
        // A 4000/s rate over 2s is ~8000 requests; anything near zero or wildly
        // over means the arrival clock is not driving generation.
        let requests = g.requests_emitted();
        assert!((4_000..16_000).contains(&requests), "{requests} requests");
    }

    #[test]
    fn warmup_events_are_flagged_and_the_rest_are_not() {
        let y = format!("duration: 4s\n{}", BASE.trim_start())
            .replace("  wss_window: 240000", "  wss_window: 240000\n  warmup: 1s");
        let d = Document::from_yaml(&y).unwrap();
        let mut g = Generator::new(&d).unwrap();
        let ev = drain(&mut g);
        assert!(ev.iter().any(|e| e.has(flags::WARMUP)));
        assert!(ev.iter().any(|e| !e.has(flags::WARMUP)));
        for e in &ev {
            assert_eq!(e.has(flags::WARMUP), e.t_ns < 1_000_000_000);
        }
    }

    #[test]
    fn closed_loop_holds_the_configured_population() {
        let y = BASE.replace(
            "arrival: {model: open_loop, rate: 4000/s}",
            "arrival: {model: closed_loop, concurrency: 32}",
        );
        let d = Document::from_yaml(&format!("requests: 4000\n{}", y.trim_start())).unwrap();
        let mut g = Generator::new(&d).unwrap();
        let mut buf = Vec::new();
        let mut seen_full = false;
        while g.fill(&mut buf) > 0 {
            assert!(g.live_sessions() <= 32, "{} live", g.live_sessions());
            seen_full |= g.live_sessions() == 32;
        }
        assert!(seen_full, "population never filled");
    }

    #[test]
    fn sticky_placement_binds_a_session_to_one_node_and_one_root() {
        // FR-019a: a session's KV lives where it was computed, so it binds at
        // birth as it binds to a root.
        let mut g = Generator::new(&doc("requests: 600")).unwrap();
        let ev = drain(&mut g);
        let mut node_of: std::collections::HashMap<u32, u16> = std::collections::HashMap::new();
        let mut root_of: std::collections::HashMap<u32, CacheKey> =
            std::collections::HashMap::new();
        for e in &ev {
            let n = node_of.entry(e.session_id.0).or_insert(e.node);
            assert_eq!(*n, e.node, "session moved node");
            if e.depth == 0 {
                let r = root_of.entry(e.session_id.0).or_insert(e.key);
                assert_eq!(*r, e.key, "session changed root");
            }
        }
        // All four nodes get used, so placement is uniform rather than degenerate.
        let used: std::collections::HashSet<u16> = ev.iter().map(|e| e.node).collect();
        assert_eq!(used.len(), 4, "used {used:?}");
    }

    #[test]
    fn per_request_placement_scatters_a_session_deliberately() {
        // Offered only for comparison, and never the default, because it makes a
        // session remotely fetch its own earlier turns.
        let y = BASE.replace(
            "  nodes: [node2, node7, node9, node11]",
            "  nodes: [node2, node7, node9, node11]\n  placement: per_request",
        );
        let d = Document::from_yaml(&format!("duration: 3s\n{}", y.trim_start())).unwrap();
        let ev = drain(&mut Generator::new(&d).unwrap());
        let mut moved = false;
        let mut node_of: std::collections::HashMap<u32, u16> = std::collections::HashMap::new();
        for e in &ev {
            if let Some(prev) = node_of.insert(e.session_id.0, e.node) {
                moved |= prev != e.node;
            }
        }
        assert!(moved, "per_request placement did not scatter");
    }

    #[test]
    fn size_is_a_pure_function_of_the_key_across_the_whole_plan() {
        // FR-011, checked over a realised stream rather than over derivation:
        // the same key must carry the same size wherever it appears.
        let ev = drain(&mut Generator::new(&doc("requests: 800")).unwrap());
        let mut size_of: std::collections::HashMap<CacheKey, u32> =
            std::collections::HashMap::new();
        for e in &ev {
            let s = size_of.entry(e.key).or_insert(e.size);
            assert_eq!(*s, e.size, "one key, two sizes");
        }
    }

    #[test]
    fn sessions_share_a_trunk_prefix_and_diverge_below_it() {
        // The point of the whole corpus model: without this there is nothing for
        // a remote lookup to serve.
        let ev = drain(&mut Generator::new(&doc("requests: 3000")).unwrap());
        let mut per_session: std::collections::HashMap<u32, Vec<CacheKey>> =
            std::collections::HashMap::new();
        for e in &ev {
            if e.turn == 1 {
                per_session.entry(e.session_id.0).or_default().push(e.key);
            }
        }
        let mut counts: std::collections::HashMap<CacheKey, u32> = std::collections::HashMap::new();
        for keys in per_session.values() {
            for k in keys {
                *counts.entry(*k).or_default() += 1;
            }
        }
        let shared = counts.values().filter(|c| **c > 1).count();
        assert!(shared > 0, "no key was touched by two sessions");
        // And the private tail is private: the deepest key of a session is its
        // alone.
        let private = counts.values().filter(|c| **c == 1).count();
        assert!(private > shared, "everything was shared, nothing minted");
    }

    #[test]
    fn a_missing_run_length_is_reported_rather_than_guessed() {
        let mut d = doc("requests: 10");
        d.requests = None;
        assert_eq!(Budget::from_document(&d), Err(GenError::RunLength(0)));
        d.requests = Some(10);
        d.blocks = Some(10);
        assert_eq!(Budget::from_document(&d), Err(GenError::RunLength(2)));
    }

    #[test]
    fn a_bad_unit_names_the_field_it_came_from() {
        let mut d = doc("requests: 10");
        d.requests = None;
        d.duration = Some("180x".into());
        let e = Budget::from_document(&d).unwrap_err();
        assert!(format!("{e}").starts_with("duration:"), "{e}");
    }
}
