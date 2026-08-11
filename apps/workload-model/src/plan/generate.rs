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
use crate::units::{count_from_yaml, parse_duration_ns, parse_rate_per_s, UnitError};

/// Default look-ahead: 64Ki events, about 2.5 MiB of records.
///
/// Chosen to sit comfortably inside a last-level cache while being far more than
/// any issuing core can drain between refills. It is a default rather than a
/// constant of the design — the whole point of reporting the horizon is that it
/// can be tuned against a measurement.
pub const DEFAULT_HORIZON_EVENTS: usize = 64 * 1024;

/// Domain separators for the generator's own draws, so that two unrelated
/// quantities about one session never consume each other's values.
const TAG_SESSION: u64 = 0x5E55_1014;
const TAG_TRUNK_WALK: u64 = 0x7204_4B01;
const TAG_GROWTH: u64 = 0x6407_0407;
const TAG_ARRIVAL: u64 = 0x4881_1A15;
const TAG_NODE: u64 = 0x0D0D_E101;

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
    /// How much of this session's path is shared trunk; drawn once at birth.
    shared_depth: u32,
    /// Path depth of the turn about to be issued.
    depth: u32,
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
    shared_depth: Dist,
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
        // The occupancy window, needed only to resolve `branching: auto`.
        let window_requests = d
            .run
            .wss_window
            .as_ref()
            .and_then(count_from_yaml)
            .unwrap_or(crate::schema::DEFAULT_WSS_WINDOW_REQUESTS);
        let mean_turns = d.workload.sessions.turns.mean().unwrap_or(1.0).max(1.0);
        let sessions_per_window = window_requests as f64 / mean_turns;
        let p99_shared = d.corpus.trees.shared_depth.quantile_u32(0.99);
        let corpus = Corpus::resolve(
            &d.corpus.trees,
            d.corpus.block_bytes.clone(),
            d.seed,
            sessions_per_window,
            p99_shared,
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
                live.depth = live
                    .depth
                    .saturating_add(g.sample_u64(&mut live.growth).min(u64::from(u32::MAX)) as u32);
                self.live.push(live);
            }
            // Retirement is simply dropping it: its private keys are dead from
            // that moment (FR-014b), and nothing about it needs keeping.
        }
        out.len()
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
        let mut growth = Stream::new(self.seed ^ TAG_GROWTH, u64::from(id.0));
        let depth = crate::session::depth_at_turn(
            shared_depth,
            params.private_depth,
            &params.growth_per_turn,
            1,
            &mut growth,
        );
        Live {
            s: Session {
                id,
                node,
                root_index,
                params,
                turn: 1,
                next_t_ns: at_ns,
            },
            shared_depth,
            depth,
            growth,
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
        for d in 0..=depth {
            if d > 0 {
                cur = if d <= live.shared_depth {
                    self.corpus.trunk_step(cur, d, &mut walk, gen)
                } else {
                    private_child(cur, live.s.id, d)
                };
            }
            let mut f = warm;
            if d == 0 {
                f |= flags::REQUEST_START;
            }
            if d == depth {
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
        let mut g = Generator::new(&doc("requests: 200")).unwrap();
        let ev = drain(&mut g);
        let shared = 6u32;
        let private = 3u32;
        let growth = Dist::Scalar(2.0);
        let mut depths: std::collections::BTreeMap<(u32, u16), u32> =
            std::collections::BTreeMap::new();
        for e in &ev {
            let d = depths.entry((e.session_id.0, e.turn)).or_insert(0);
            *d = (*d).max(e.depth);
        }
        assert!(!depths.is_empty());
        for ((_, turn), realised) in depths {
            let stated = depth_at_turn(
                shared,
                private,
                &growth,
                turn,
                &mut Stream::new(g.seed ^ TAG_GROWTH, 0),
            );
            assert_eq!(realised, stated, "turn {turn}");
        }
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
