//! The virtual-time event loop: where a description becomes a stream of turns.
//!
//! Everything the simulation needs already exists in its own module — shared
//! populations in [`pool`](crate::pool), selection in
//! [`selection`](crate::selection), sessions and their chains in
//! [`session`](crate::session). This module owns only the *ordering*: what happens
//! next, and in what order things happening at the same instant are applied.
//!
//! # Virtual time advances only through think time
//!
//! A turn occupies a single virtual instant. Nothing here models service time, so
//! the wall-clock concurrency at execution is a lane-count choice and not a
//! property of the plan — which is what lets `--batch-keys` and `--lanes` vary
//! without changing the workload (FR-072).
//!
//! # Three kinds of event, and a fixed order among them
//!
//! At each step the loop advances to the earliest of:
//!
//! 1. a **shared-pool event** — an instance is born or retires;
//! 2. a **session arrival** — a fluctuating session class creates one;
//! 3. a **turn**.
//!
//! When several fall at the same instant they are applied **in that order**, and
//! the order is deliberate rather than incidental: a turn at time `t` must see the
//! population as it stands *at* `t`, so population changes are applied first. The
//! reverse order would make a session's shared set depend on the arbitrary order
//! two pools were declared in.
//!
//! Ties among turns break on **session id**, which is run-global and unique, so
//! the delivered order is a total order and is reproducible from the seed without
//! depending on iteration order anywhere. Breaking on a slab handle instead would
//! also be deterministic, but handles depend on allocation history, so the
//! resulting order could not be checked from outside the loop.
//!
//! # Turns come from a heap, not a scan
//!
//! Finding the earliest pending turn by scanning live sessions would cost
//! `O(live)` per turn, against a design target of 10 000 concurrent sessions. The
//! loop keeps a heap of `(next_turn_at, class, handle)` instead, pushing a
//! session's next turn as each one is taken. An entry is popped exactly when its
//! turn is processed, so the heap holds no stale entries — a `debug_assert` pins
//! that rather than leaving lazy-deletion machinery no test exercises.
//!
//! # What this module does *not* do
//!
//! Node placement and migration are User Story 3. Nothing here knows what a node
//! is, which is why the whole loop runs with no accelerator and no server.
//!
//! # Examples
//!
//! ```
//! use workload_model::description::WorkloadDescription;
//! use workload_model::sim::Simulation;
//!
//! let yaml = r#"
//! version: 1
//! blocks: {tokens: 16, bytes: 32768}
//! shared_classes:
//!   manual:
//!     length: {constant: 6}
//!     lifetime: {constant: .inf}
//! session_classes:
//!   chat:
//!     pool: {size: {exact: 4}}
//!     uses: [{class: manual, count: {constant: 1}}]
//!     turns: {constant: 3}
//!     input_growth: {constant: 2}
//!     output_growth: {constant: 1}
//!     think_time: {constant: 10}
//! "#;
//! let description: WorkloadDescription = yaml.parse().unwrap();
//! let mut sim = Simulation::new(&description, 42).unwrap();
//!
//! let mut turns = 0;
//! let mut blocks_read = 0;
//! sim.run_until(1_000.0, &mut |_session, turn| {
//!     turns += 1;
//!     blocks_read += turn.reads_len();
//! });
//!
//! assert!(turns > 4, "every seeded session should have taken its turns");
//! assert!(blocks_read > 0, "later turns read the prefix built by earlier ones");
//! assert_eq!(sim.now(), 1_000.0);
//! ```

use std::cmp::Reverse;
use std::collections::BinaryHeap;

use crate::description::WorkloadDescription;
use crate::pool::SharedPool;
use rand::Rng as _;

use crate::distribution::Resolved;
use crate::rng;
use crate::selection::{SelectionStats, Selector};
use crate::session::{bind, take_turn, Growth, Session, SessionIds, SessionPool, Turn, Uses};
use crate::{Error, Result};

/// Everything one session class needs at run time, resolved once.
#[derive(Debug)]
struct SessionClassState {
    pool: SessionPool,
    uses: Vec<Uses>,
    growth: Growth,
    /// How long between this class's migrations, or `None` if it never migrates.
    migration_interval: Option<Resolved>,
}

/// A running simulation of one workload description.
///
/// `shared_pools` and `selectors` are parallel, both indexed by a shared class's
/// declaration index. Kept as two slices rather than one slice of pairs because
/// that is what [`bind`] takes — and it needs both mutably at once, which a slice
/// of pairs cannot give without reborrowing every element.
#[derive(Debug)]
pub struct Simulation {
    shared_pools: Vec<SharedPool>,
    selectors: Vec<Selector>,
    sessions: Vec<SessionClassState>,
    ids: SessionIds,
    rng: rng::Rng,
    /// Pending turns, earliest first, as `(at bits, session id, class, handle)`.
    ///
    /// Ordered on session id rather than on `(class, handle)` so the delivered
    /// order is the one `plan.rs` asserts: by virtual time, then by session id
    /// (FR-035). Session ids are unique run-wide, so the trailing fields never
    /// participate in the comparison and are along only to locate the session.
    turns: BinaryHeap<Reverse<(u64, u64, usize, usize)>>,
    now: f64,
    turns_taken: u64,
    blocks_read: u64,
    blocks_minted: u64,
    /// Nodes to place sessions across. One means migration is inert (FR-049).
    nodes: usize,
    migrations: u64,
}

impl Simulation {
    /// Build and seed a simulation from a validated description.
    ///
    /// Seeds every population at `t = 0` from its equilibrium distribution and
    /// binds the seeded sessions, so the very first turn is already
    /// representative — there is no warm-up period to discard.
    ///
    /// Each concern draws from its own RNG substream, so adding a class or a draw
    /// in one place cannot shift the numbers everywhere else and silently
    /// invalidate a recorded seed.
    ///
    /// # Errors
    ///
    /// If the description does not validate, or a distribution cannot be resolved.
    pub fn new(description: &WorkloadDescription, seed: u64) -> Result<Self> {
        let report = description.validate()?;
        if !report.refusals().is_empty() {
            return Err(Error::new(format!(
                "the description is not valid: {}",
                report.refusals().join("; ")
            )));
        }

        let mut pool_rng = rng::substream(seed, "pools");
        let mut shared_pools = Vec::with_capacity(description.shared_classes.len());
        let mut selectors = Vec::with_capacity(description.shared_classes.len());
        for (index, _, class) in description.shared_classes.iter() {
            let lifetime = class.lifetime.resolve(None)?;
            let length = class.length.resolve_integral(None)?;
            let rank_by = class.pool.rank_by.unwrap_or_default();
            let mut pool = SharedPool::new(
                index as u64,
                class.pool.size,
                lifetime,
                length,
                &mut pool_rng,
            );
            pool.seed(0.0, &mut pool_rng);
            shared_pools.push(pool);
            // The selection distribution is what makes `rank_by` mean anything: a uniform draw
            // is the same distribution however the ranks are numbered (FR-021).
            let selection = match &class.pool.selection {
                Some(d) => Some(d.resolve(None)?),
                None => None,
            };
            selectors.push(Selector::new(rank_by).with_selection(selection));
        }

        let mut session_rng = rng::substream(seed, "sessions");
        let mut ids = SessionIds::new();
        let mut sessions = Vec::with_capacity(description.session_classes.len());
        for (index, name, class) in description.session_classes.iter() {
            let mut uses = Vec::with_capacity(class.uses.len());
            for u in &class.uses {
                let class_index =
                    description
                        .shared_classes
                        .index_of(&u.class)
                        .ok_or_else(|| {
                            Error::new(format!(
                                "session class {name:?} uses undeclared shared class {:?}",
                                u.class
                            ))
                        })?;
                let nominal = description
                    .shared_classes
                    .by_index(class_index)
                    .map(|(_, c)| c.pool.size.nominal())
                    .unwrap_or(0);
                uses.push(Uses {
                    class_index,
                    // The pool size is the implied maximum on the count, exactly as
                    // load-time validation reports it (FR-004).
                    count: u.count.resolve_integral(Some(nominal as f64))?,
                });
            }
            let mut pool = SessionPool::new(
                index as u64,
                class.pool.size,
                class.turns.resolve_integral(None)?,
                class.think_time.resolve(None)?,
                &mut session_rng,
            );
            pool.seed(0.0, &mut ids, &mut session_rng);
            sessions.push(SessionClassState {
                pool,
                uses,
                growth: Growth {
                    input: class.input_growth.resolve_integral(None)?,
                    output: class.output_growth.resolve_integral(None)?,
                },
                // Continuous, not integral: a migration happens at an instant, not after a
                // whole number of anything.
                migration_interval: match &class.migration_interval {
                    Some(d) => Some(d.resolve(None)?),
                    None => None,
                },
            });
        }

        let mut sim = Self {
            shared_pools,
            selectors,
            sessions,
            ids,
            rng: rng::substream(seed, "sim"),
            nodes: 1,
            migrations: 0,
            turns: BinaryHeap::new(),
            now: 0.0,
            turns_taken: 0,
            blocks_read: 0,
            blocks_minted: 0,
        };
        // The seeded sessions exist but are unbound and unscheduled.
        for class in 0..sim.sessions.len() {
            sim.admit_new(class);
        }
        Ok(sim)
    }

    /// Virtual time reached so far.
    pub fn now(&self) -> f64 {
        self.now
    }

    /// Turns taken since the run began.
    pub fn turns_taken(&self) -> u64 {
        self.turns_taken
    }

    /// Block reads issued — the sum of every turn's prefix length, which grows
    /// quadratically in session length (FR-025).
    pub fn blocks_read(&self) -> u64 {
        self.blocks_read
    }

    /// Blocks minted by turn growth, not counting shared instances.
    pub fn blocks_minted(&self) -> u64 {
        self.blocks_minted
    }

    /// Live sessions across every class.
    pub fn live_sessions(&self) -> usize {
        self.sessions.iter().map(|c| c.pool.live()).sum()
    }

    /// Selectable shared instances across every class.
    pub fn live_instances(&self) -> usize {
        self.shared_pools.iter().map(|p| p.live()).sum()
    }

    /// One shared class's selection statistics, for the run report (FR-023).
    pub fn selection_stats(&self, class_index: usize) -> Option<SelectionStats> {
        self.selectors.get(class_index).map(|s| s.stats())
    }

    /// Sessions started and completed, per session class.
    pub fn session_counts(&self, class_index: usize) -> Option<(u64, u64)> {
        self.sessions
            .get(class_index)
            .map(|c| (c.pool.started(), c.pool.completed()))
    }

    /// Sessions started and completed across **every** session class.
    ///
    /// What a run report wants. Kept as its own method because reporting class 0's
    /// numbers as the run's is a mistake that reads perfectly plausibly — a
    /// two-class description would simply under-report, with no sign that it had.
    pub fn session_totals(&self) -> (u64, u64) {
        self.sessions.iter().fold((0, 0), |(s, c), class| {
            (s + class.pool.started(), c + class.pool.completed())
        })
    }

    /// The next virtual time anything happens, if anything does.
    pub fn next_event_at(&self) -> Option<f64> {
        let mut next = f64::INFINITY;
        for p in &self.shared_pools {
            next = next.min(p.next_event_at().unwrap_or(f64::INFINITY));
        }
        for c in &self.sessions {
            next = next.min(c.pool.next_event_at().unwrap_or(f64::INFINITY));
        }
        if let Some(Reverse((bits, _, _, _))) = self.turns.peek() {
            next = next.min(f64::from_bits(*bits));
        }
        if next.is_finite() {
            Some(next)
        } else {
            None
        }
    }

    /// Advance to the next event and apply everything that happens at that
    /// instant, calling `on_turn` for each turn taken.
    ///
    /// Returns the virtual time advanced to, or `None` when nothing is left to do —
    /// which happens when every population is mint-once and every session has
    /// finished.
    ///
    /// `on_turn` receives the session and the turn while the session is still
    /// alive, which is the only moment its keys can be read: a session that has
    /// just taken its last turn is finished in this same step and its chain goes
    /// with it.
    pub fn step<F>(&mut self, on_turn: &mut F) -> Option<f64>
    where
        F: FnMut(&Session, &Turn),
    {
        let t = self.next_event_at()?;

        // 1. Population changes first, so a turn at `t` sees the world as of `t`.
        for p in &mut self.shared_pools {
            p.advance_to(t, &mut self.rng);
        }

        // 2. Session arrivals, each bound before it can take a turn.
        for class in 0..self.sessions.len() {
            self.sessions[class]
                .pool
                .advance_births_to(t, &mut self.ids, &mut self.rng);
            self.admit_new(class);
        }

        // 3. Every turn falling at or before `t`. A think-time draw of exactly zero
        //    puts a session's next turn at this same instant, so this batches them
        //    into one step. That is an efficiency choice and not a correctness one:
        //    with an `if` here the remainder would simply be picked up by the next
        //    step, at the same `t` — measured, not assumed.
        while self
            .turns
            .peek()
            .is_some_and(|Reverse((bits, _, _, _))| f64::from_bits(*bits) <= t)
        {
            let Reverse((bits, _, class, handle)) = self.turns.pop().expect("peeked");
            debug_assert_eq!(
                f64::from_bits(bits).to_bits(),
                self.sessions[class]
                    .pool
                    .session(handle)
                    .next_turn_at()
                    .expect("a scheduled turn exists")
                    .to_bits(),
                "stale turn entry for class {class} handle {handle}; the heap is \
                 assumed to hold none"
            );
            self.take_one_turn(class, handle, on_turn);
        }

        self.now = t;
        Some(t)
    }

    /// Run until virtual time `until`, applying every event up to and including it.
    ///
    /// Stops early only when nothing is left to do; the clock still ends at
    /// `until`, because a run's span is what was asked for and not what happened to
    /// be busy.
    pub fn run_until<F>(&mut self, until: f64, on_turn: &mut F)
    where
        F: FnMut(&Session, &Turn),
    {
        while let Some(next) = self.next_event_at() {
            if next > until {
                break;
            }
            self.step(on_turn);
        }
        self.now = self.now.max(until);
    }

    /// Place sessions across `nodes`, and enable migration.
    ///
    /// `1` — the default — makes migration **inert** rather than an error (FR-049): a
    /// single-node run is the ordinary case, and refusing a description that happens to
    /// declare a `migration_interval` would make every multi-node description unusable
    /// locally.
    ///
    /// # Panics
    ///
    /// If `nodes` is zero: there would be nowhere to place a session, and treating it as one
    /// would hide a caller's arithmetic error behind a working run.
    pub fn with_nodes(mut self, nodes: usize) -> Self {
        assert!(nodes > 0, "a run needs at least one node");
        self.nodes = nodes;
        self
    }

    /// Nodes sessions are placed across.
    pub fn nodes(&self) -> usize {
        self.nodes
    }

    /// What each shared class's `selection` spread effectively covers, by declaration index.
    ///
    /// `None` for a class drawing uniformly, which is the case a capacity sweep must be able to
    /// recognise: the working set is then the whole key space, so the curve steps rather than
    /// slopes and no eviction policy can be distinguished however it is swept (FR-021).
    ///
    /// Measured as twice the spread's effective standard deviation — the ranks carrying most of
    /// the mass — rather than as a count of ranks ever drawn, which would depend on how long the
    /// run happened to be.
    pub fn working_set(&self) -> Vec<(u64, u64, Option<f64>)> {
        self.shared_pools
            .iter()
            .zip(self.selectors.iter())
            .enumerate()
            .map(|(index, (pool, selector))| {
                (
                    index as u64,
                    pool.nominal(),
                    selector
                        .effective_ranks()
                        .map(|r| r.min(pool.nominal() as f64)),
                )
            })
            .collect()
    }

    /// Migrations performed so far.
    ///
    /// Reported because a run whose migration interval is long relative to session lifetime
    /// performs none, and a multi-node measurement that meant to exercise migration would
    /// otherwise look like one that did.
    pub fn migrations(&self) -> u64 {
        self.migrations
    }

    /// Place a newly born session, and schedule its first migration.
    ///
    /// Uniform over nodes (FR-048). Drawn even when there is only one node, so that the
    /// sequence of random draws — and therefore every later decision — does not depend on the
    /// deployment: a description run on one node and on four must produce the same *workload*,
    /// differing only in where each turn is sent.
    fn place(&mut self, class: usize, handle: usize) {
        let node = self.rng.gen_range(0..self.nodes);
        let interval = self.sessions[class]
            .migration_interval
            .as_ref()
            .map(|i| i.sample(&mut self.rng));
        let session = self.sessions[class].pool.session_mut(handle);
        session.place(node);
        let born = session.born_at();
        session.schedule_migration(interval.map(|dt| born + dt.max(0.0)));
    }

    /// Apply any migrations this session is due, before its turn at `at`.
    ///
    /// # Lazy, and observably identical to a scheduled event
    ///
    /// A session's node matters only when it takes a turn, so a migration between turns is
    /// invisible except through where the next turn goes. Evaluating it here rather than from a
    /// second event heap is therefore not an approximation — and it keeps the event loop's
    /// ordering, which `plan.rs` asserts, in one place.
    ///
    /// The loop matters: two intervals may elapse between turns, and applying one migration
    /// where two were due would leave the session on the wrong node, since "uniform among the
    /// others" excludes a different node each time.
    fn migrate_due(&mut self, class: usize, handle: usize, at: f64) {
        if self.nodes < 2 {
            // Inert, and not merely a no-op: no draw is taken, so a single-node run does not
            // consume randomness a multi-node run would spend elsewhere.
            return;
        }
        loop {
            let session = self.sessions[class].pool.session(handle);
            let Some(due) = session.next_migration_at() else {
                return;
            };
            if due > at {
                return;
            }
            let from = session.node();
            // Uniform among the *others* (FR-048): drawing over all nodes would leave a
            // session where it was with probability 1/nodes, which is not a migration.
            let step = 1 + self.rng.gen_range(0..self.nodes - 1);
            let to = (from + step) % self.nodes;
            let interval = self.sessions[class]
                .migration_interval
                .as_ref()
                .map(|i| i.sample(&mut self.rng));
            let session = self.sessions[class].pool.session_mut(handle);
            session.migrate_to(to);
            // From the due time, not from `at`: pacing the next migration off when we happened
            // to notice would stretch the interval by however long the session was idle.
            session.schedule_migration(interval.map(|dt| due + dt.max(0.0)));
            self.migrations += 1;
        }
    }

    /// Take one turn, then either reschedule the session or finish it.
    fn take_one_turn<F>(&mut self, class: usize, handle: usize, on_turn: &mut F)
    where
        F: FnMut(&Session, &Turn),
    {
        // Migrations first: a turn is issued to wherever the session is *now*.
        let at = self.sessions[class]
            .pool
            .session(handle)
            .next_turn_at()
            .unwrap_or(self.now);
        self.migrate_due(class, handle, at);

        let growth = self.sessions[class].growth.clone();
        let session = self.sessions[class].pool.session_mut(handle);
        let Some(turn) = take_turn(session, &growth, &mut self.rng) else {
            debug_assert!(false, "a scheduled turn was not takeable");
            return;
        };
        self.turns_taken += 1;
        self.blocks_read += turn.reads_len() as u64;
        self.blocks_minted += (turn.new_input_len() + turn.new_output_len()) as u64;
        on_turn(self.sessions[class].pool.session(handle), &turn);

        let session = self.sessions[class].pool.session(handle);
        match session.next_turn_at() {
            Some(at) => {
                let id = session.id();
                self.turns.push(Reverse((at.to_bits(), id, class, handle)));
            }
            None => self.retire_session(class, handle),
        }
    }

    /// Finish a spent session and give its shared instances back.
    ///
    /// Releasing the holds is what lets a retired instance's bookkeeping be freed
    /// (FR-018); nothing else can do it, because the holds belong to pools this
    /// session's own pool knows nothing about.
    fn retire_session(&mut self, class: usize, handle: usize) {
        let mut finished = self.sessions[class]
            .pool
            .finish(handle, &mut self.ids, &mut self.rng);
        for b in finished.take_chosen() {
            let owner = b.class_id() as usize;
            self.shared_pools[owner].release(b.into_held());
        }
        // An exact class replaces the death at once, so there may be a new session
        // needing binding and scheduling.
        self.admit_new(class);
    }

    /// Bind and schedule every session this class has just created.
    ///
    /// Idempotent, and it has to be: `newly_born` accumulates across a whole step —
    /// `advance_births_to` clears it, `finish` appends to it — so this runs more
    /// than once per step with earlier handles still listed.
    ///
    /// The bound check is inside the loop, not a filter before it. Slab handles are
    /// **reused**, so when a session is created, finishes, and is replaced all
    /// within one step — which a think time with an atom at zero makes routine —
    /// `newly_born` lists the same handle twice for two different sessions.
    /// Filtering first would find that handle unbound and bind it twice.
    fn admit_new(&mut self, class: usize) {
        let handles: Vec<usize> = self.sessions[class].pool.newly_born().to_vec();
        for handle in handles {
            if self.sessions[class].pool.session(handle).is_bound() {
                continue;
            }
            // Three disjoint fields borrowed at once, which is why they are fields
            // and not one nested structure.
            let SessionClassState { pool, uses, .. } = &mut self.sessions[class];
            bind(
                pool.session_mut(handle),
                uses,
                &mut self.shared_pools,
                &mut self.selectors,
                &mut self.rng,
            );
            // Placed after binding, so the draw order is: shared instances, then node. A
            // session's node is decided once, at birth (FR-048).
            self.place(class, handle);
            let pool = &self.sessions[class].pool;
            if let Some(at) = pool.session(handle).next_turn_at() {
                let id = pool.session(handle).id();
                self.turns.push(Reverse((at.to_bits(), id, class, handle)));
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A description with one immortal shared class and one exact session class,
    /// so counts are deterministic and the loop's ordering is what is under test.
    fn description(turns: u32, sessions: u32) -> WorkloadDescription {
        format!(
            r#"
version: 1
blocks: {{tokens: 16, bytes: 32768}}
shared_classes:
  manual:
    length: {{constant: 5}}
    lifetime: {{constant: .inf}}
session_classes:
  chat:
    pool: {{size: {{exact: {sessions}}}}}
    uses: [{{class: manual, count: {{constant: 1}}}}]
    turns: {{constant: {turns}}}
    input_growth: {{constant: 2}}
    output_growth: {{constant: 1}}
    think_time: {{constant: 10}}
"#
        )
        .parse()
        .unwrap()
    }

    #[test]
    fn a_run_is_reproducible_from_its_seed() {
        // FR-072 in its simplest form: same description and seed, same turns.
        let d = description(4, 6);
        let snapshot = |seed: u64| {
            let mut sim = Simulation::new(&d, seed).unwrap();
            let mut out = Vec::new();
            sim.run_until(500.0, &mut |s, t| {
                out.push((s.id(), t.index(), t.at().to_bits(), t.reads_len()));
            });
            out
        };
        assert_eq!(snapshot(9), snapshot(9));
        assert_ne!(snapshot(9), snapshot(10));
    }

    #[test]
    fn turns_are_delivered_in_virtual_time_order() {
        // The whole point of the loop. Out-of-order delivery would mean a turn
        // reading a prefix that did not exist yet, and no aggregate would show it.
        let d = description(5, 12);
        let mut sim = Simulation::new(&d, 3).unwrap();
        let mut times = Vec::new();
        sim.run_until(1_000.0, &mut |_, t| times.push(t.at()));
        assert!(times.len() > 12);
        assert!(
            times.windows(2).all(|w| w[0] <= w[1]),
            "turns were delivered out of order"
        );
    }

    #[test]
    fn each_session_takes_its_turns_in_order_from_zero() {
        let d = description(4, 8);
        let mut sim = Simulation::new(&d, 4).unwrap();
        let mut by_session: std::collections::BTreeMap<u64, Vec<usize>> = Default::default();
        sim.run_until(2_000.0, &mut |s, t| {
            by_session.entry(s.id()).or_default().push(t.index());
        });
        assert!(by_session.len() > 8, "no session turnover");
        for (id, indices) in &by_session {
            let want: Vec<usize> = (0..indices.len()).collect();
            assert_eq!(indices, &want, "session {id} took turns out of order");
        }
    }

    #[test]
    fn a_turns_prefix_is_never_empty_once_a_shared_class_is_used() {
        // Binding happens before the first turn, so even turn 0 reads the shared
        // run. A missing bind would show up here as a zero-length first read.
        let d = description(3, 5);
        let mut sim = Simulation::new(&d, 5).unwrap();
        let mut first_reads = Vec::new();
        sim.run_until(400.0, &mut |s, t| {
            if t.index() == 0 {
                first_reads.push((s.id(), t.reads_len()));
            }
        });
        assert!(!first_reads.is_empty());
        for (id, reads) in first_reads {
            assert_eq!(
                reads, 5,
                "session {id}'s first turn read {reads} blocks; it should read its \
                 one 5-block shared instance"
            );
        }
    }

    #[test]
    fn reads_grow_by_the_growth_per_turn_within_a_session() {
        // FR-025 end to end: each turn reads exactly what the previous one read
        // plus what it minted.
        let d = description(6, 3);
        let mut sim = Simulation::new(&d, 6).unwrap();
        let mut last: std::collections::BTreeMap<u64, usize> = Default::default();
        sim.run_until(2_000.0, &mut |s, t| {
            if let Some(previous) = last.get(&s.id()) {
                assert_eq!(
                    t.reads_len(),
                    previous + 3,
                    "session {} turn {} read {} blocks, expected {}",
                    s.id(),
                    t.index(),
                    t.reads_len(),
                    previous + 3
                );
            }
            last.insert(s.id(), t.reads_len());
        });
        assert!(last.len() >= 3);
    }

    #[test]
    fn an_exact_session_class_holds_its_concurrency_through_the_loop() {
        let d = description(3, 10);
        let mut sim = Simulation::new(&d, 7).unwrap();
        assert_eq!(sim.live_sessions(), 10);
        for step in 1..=40 {
            sim.run_until(step as f64 * 50.0, &mut |_, _| {});
            assert_eq!(sim.live_sessions(), 10, "concurrency drifted");
        }
        let (started, completed) = sim.session_counts(0).unwrap();
        assert!(completed > 10, "no session turnover");
        assert_eq!(started, completed + 10);
    }

    #[test]
    fn every_hold_is_released_when_its_session_finishes() {
        // The leak that would otherwise be invisible: a retired instance whose
        // bookkeeping is never freed because a finished session kept its hold.
        let d = description(2, 8);
        let mut sim = Simulation::new(&d, 8).unwrap();
        sim.run_until(3_000.0, &mut |_, _| {});
        let (_, completed) = sim.session_counts(0).unwrap();
        assert!(completed > 8, "no turnover, so nothing was tested");
        // Exactly the live sessions hold the one instance, and nothing else does.
        let users: u32 = sim.shared_pools[0].selectable().map(|i| i.users()).sum();
        assert_eq!(
            users as usize,
            sim.live_sessions(),
            "holds outstanding ({users}) does not match live sessions ({})",
            sim.live_sessions()
        );
        assert_eq!(sim.shared_pools[0].retained(), 0);
    }

    #[test]
    fn the_clock_ends_where_it_was_asked_to_even_when_idle() {
        // A mint-once pool with sessions that all finish leaves nothing to do. The
        // span is still what was requested, because an emit run's reported span is
        // the question asked, not the busiest part of the answer.
        let d = description(1, 2);
        let mut sim = Simulation::new(&d, 9).unwrap();
        sim.run_until(50.0, &mut |_, _| {});
        assert_eq!(sim.now(), 50.0);
    }

    #[test]
    fn a_zero_mean_think_time_is_refused_before_any_simulation() {
        // Not the same-instant case below: a mean of zero makes sessions
        // instantaneous, so no finite arrival rate sustains concurrency and the
        // description is refused at load.
        let yaml = r#"
version: 1
blocks: {tokens: 16, bytes: 32768}
shared_classes:
  manual:
    length: {constant: 2}
    lifetime: {constant: .inf}
session_classes:
  burst:
    pool: {size: {exact: 3}}
    uses: [{class: manual, count: {constant: 1}}]
    turns: {constant: 4}
    input_growth: {constant: 1}
    output_growth: {constant: 1}
    think_time: {constant: 0}
"#;
        let d: WorkloadDescription = yaml.parse().unwrap();
        let err = Simulation::new(&d, 1).unwrap_err().to_string();
        assert!(
            err.contains("think_time"),
            "expected a think_time refusal, got: {err}"
        );
    }

    #[test]
    fn several_turns_can_fall_at_one_instant() {
        // An empirical think time with an atom at zero has a positive mean — so it
        // is accepted — while individual draws of exactly zero put a session's next
        // turn at the instant already being processed. A whole session can then be
        // born, run and finish inside one step, and its slab handle be reused by its
        // replacement within that same step.
        //
        // That is what this test pins, and it found a real defect: `admit_new`
        // filtered unbound handles *before* binding them, so one handle listed twice
        // for two different sessions was bound twice. Verified by reinstating the
        // filter, which fails here with "session 57 bound twice".
        //
        // It does **not** pin the `while` in step 3. Changing that to an `if` breaks
        // nothing, because the next step processes the remainder at the same instant
        // — checked, after wrongly claiming otherwise in a comment.
        let yaml = r#"
version: 1
blocks: {tokens: 16, bytes: 32768}
shared_classes:
  manual:
    length: {constant: 2}
    lifetime: {constant: .inf}
session_classes:
  burst:
    pool: {size: {exact: 40}}
    uses: [{class: manual, count: {constant: 1}}]
    turns: {constant: 6}
    input_growth: {constant: 1}
    output_growth: {constant: 1}
    think_time: {empirical: {samples: [0, 0, 0, 12], interpolate: false}}
"#;
        let d: WorkloadDescription = yaml.parse().unwrap();
        let mut sim = Simulation::new(&d, 12).unwrap();
        let mut seen: Vec<(u64, usize, u64)> = Vec::new();
        sim.run_until(600.0, &mut |s, t| {
            seen.push((s.id(), t.index(), t.at().to_bits()))
        });

        assert!(seen.len() > 40, "only {} turns", seen.len());
        // Same session, consecutive turns, identical instant: the case the `while`
        // exists for. If this never happens the test is not testing anything.
        let same_instant = seen
            .windows(2)
            .filter(|w| w[0].0 == w[1].0 && w[1].1 == w[0].1 + 1 && w[0].2 == w[1].2)
            .count();
        assert!(
            same_instant > 0,
            "no session took two turns at one instant, so the same-instant path was \
             never exercised"
        );
        // And delivery is still in order.
        let times: Vec<f64> = seen.iter().map(|(_, _, b)| f64::from_bits(*b)).collect();
        assert!(times.windows(2).all(|w| w[0] <= w[1]));
    }

    #[test]
    fn a_poisson_session_class_runs_and_reports_its_selection_statistics() {
        let yaml = r#"
version: 1
blocks: {tokens: 16, bytes: 32768}
shared_classes:
  docs:
    length: {constant: 3}
    lifetime: {exponential: {mean: 200}}
    pool: {size: {poisson: 20}}
session_classes:
  chat:
    pool: {size: {poisson: 30}}
    uses: [{class: docs, count: {constant: 4}}]
    turns: {constant: 5}
    input_growth: {constant: 1}
    output_growth: {constant: 1}
    think_time: {exponential: {mean: 8}}
"#;
        let d: WorkloadDescription = yaml.parse().unwrap();
        let mut sim = Simulation::new(&d, 11).unwrap();
        let mut turns = 0u64;
        sim.run_until(5_000.0, &mut |_, _| turns += 1);
        assert!(turns > 100, "only {turns} turns in 5000 virtual seconds");
        assert_eq!(turns, sim.turns_taken());

        let stats = sim.selection_stats(0).unwrap();
        assert!(stats.draws() > 0);
        assert_eq!(stats.draws(), sim.session_counts(0).unwrap().0);
        // Instances turn over, so the pool is sometimes short of its nominal 20 and
        // a draw of 4 occasionally binds — but never because of the description.
        assert_eq!(stats.bound_by_nominal(), 0);
    }

    #[test]
    fn an_invalid_description_is_refused_rather_than_simulated() {
        let yaml = r#"
version: 1
blocks: {tokens: 16, bytes: 32768}
shared_classes:
  docs:
    length: {constant: 3}
    lifetime: {constant: .inf}
session_classes:
  chat:
    pool: {size: {exact: 2}}
    uses: [{class: missing, count: {constant: 1}}]
    turns: {constant: 2}
    input_growth: {constant: 1}
    output_growth: {constant: 1}
    think_time: {constant: 1}
"#;
        let d: WorkloadDescription = yaml.parse().unwrap();
        let err = Simulation::new(&d, 1).unwrap_err().to_string();
        assert!(err.contains("not declared"), "unexpected error: {err}");
    }

    #[test]
    fn the_shipped_example_simulates() {
        // The normative input schema must run, not merely parse.
        let yaml = include_str!(
            "../../../specs/001-synthetic-workload-generator/contracts/workload-input.example.yml"
        );
        let d: WorkloadDescription = yaml.parse().unwrap();
        let mut sim = Simulation::new(&d, 1).unwrap();
        let mut turns = 0u64;
        sim.run_until(200.0, &mut |_, _| turns += 1);
        assert!(turns > 0, "the shipped example produced no turns");
        assert!(sim.blocks_read() > 0);
        assert!(sim.blocks_minted() > 0);
    }
}
