//! Sessions and their population: the birth-death process behind the workload.
//!
//! A [`SessionPool`] holds the live sessions of one session class and manages
//! their births, taking the same two population forms a shared-object pool does.
//! It does not execute turns — a session's turn schedule is fixed at birth and
//! the simulation loop consumes it.
//!
//! # A session dies when its last turn is taken, and nowhere else
//!
//! A shared object's death is an event in its own right, so a pool can hold a
//! heap of them. A session's is not: it *is* its final turn. So this pool has no
//! death event and no death heap — the simulation loop tells it a session has
//! finished, through [`SessionPool::finish`].
//!
//! A death heap here was written first and removed. It works, but it makes the
//! pool and the loop each decide independently when a session ends, and if they
//! ever disagree — a skipped turn, a re-drawn think time — the population
//! silently stops matching the operation stream. One source of truth is worth an
//! extra call.
//!
//! # `pool.size` is concurrency, so the arrival rate is an output (FR-024)
//!
//! A session class states how many sessions are **concurrently live**, never how
//! often one arrives. The arrival rate then follows from Little's law:
//!
//! ```text
//! arrival rate = size / E[session duration]
//! E[session duration] = E[turns] * E[think_time]
//! ```
//!
//! This is the right way round because concurrency is the thing an operator can
//! observe and reason about — it is what sets the live key-space and the load —
//! whereas an arrival rate only produces a concurrency after you have multiplied
//! it by a duration you also chose. Stating both would let a description be
//! internally contradictory.
//!
//! A session's duration is therefore **emergent**, not drawn: nothing anywhere
//! writes a session lifetime, and it cannot be set directly. It falls out of the
//! turn count and the think time.
//!
//! # Why turn times are drawn at birth, all at once
//!
//! A session's whole turn schedule is drawn when it is born and stored as
//! absolute virtual times. That has three consequences worth stating, because the
//! obvious alternative — drawing each think time as its turn comes up — looks
//! cheaper and is worse:
//!
//! - **The death time is known immediately**, so a session's whole span is a
//!   value the moment it exists — which is what lets the projection size a run,
//!   and what makes `dies_at` meaningful before any turn has been taken.
//! - **Each session's schedule is a function of its own birth**, not of how many
//!   draws other sessions happened to make first. Interleaved lazy draws would
//!   make adding one session shift every later think time in the run.
//! - **The turn model consumes exactly these draws**, so there is one schedule and
//!   not two that must agree.
//!
//! The cost is one `f64` per turn per live session — at the design scale of 10 000
//! concurrent sessions averaging 20 turns, under 2 MB.
//!
//! # Seeding, and the thing it deliberately does not do
//!
//! At `t = 0` sessions are already part-way through their conversations, so each
//! seeded session is given a **residual number of remaining turns** rather than a
//! full turn count — the same equilibrium argument as
//! [`ResidualLife`], in its discrete form: a
//! length-biased pick of a turn count, then a uniform point within it. Seeding
//! every session at its first turn would make them all end together, and the
//! session population would then turn over in cohorts exactly as a shared pool
//! does.
//!
//! What seeding does **not** do is start a session part-way along its *chain*. A
//! session seeded with three remaining turns begins a fresh prefix and mints
//! three turns' worth of blocks. The alternative — giving it the prefix it "would
//! have" accumulated — was rejected: those blocks would be read by an operation in
//! the plan while never having been written by one, so every hit-rate statistic
//! would be computed against a key space partly conjured out of nothing, and the
//! discrepancy would be invisible in aggregate.
//!
//! The price is a recorded bias. For the first mean-duration of a run, seeded
//! sessions have shorter chains than steady state — `E[R] = (E[T²] + E[T]) /
//! (2·E[T])` against `E[T]`, so for a constant 10-turn session, 5.5 turns against
//! 10. It decays as the seeded generation dies out, and the projection warns when
//! a run is short relative to what it is meant to exhibit (FR-073).
//!
//! # Examples
//!
//! ```
//! use workload_model::description::Population;
//! use workload_model::distribution::{Distribution, Kind};
//! use workload_model::rng;
//! use workload_model::session::{SessionIds, SessionPool};
//!
//! let turns = Distribution::new(Kind::Constant { value: 10.0 })
//!     .resolve_integral(None)
//!     .unwrap();
//! let think = Distribution::new(Kind::Exponential { mean: 30.0 })
//!     .resolve(None)
//!     .unwrap();
//! let mut rng = rng::substream(1, "sessions");
//! let mut ids = SessionIds::new();
//! let mut pool = SessionPool::new(0, Population::Poisson(50), turns, think, &mut rng);
//!
//! // Duration is emergent: 10 turns * 30 virtual seconds of think time.
//! assert!((pool.mean_duration() - 300.0).abs() < 1.0);
//! // And so the arrival rate is an output, not an input. (An `Exact` class has
//! // no arrival schedule at all — it arrives only in response to a death.)
//! assert!((pool.arrival_rate() - 50.0 / 300.0).abs() < 1e-3);
//!
//! pool.seed(0.0, &mut ids, &mut rng);
//! // A fluctuating class seeds from a Poisson draw *around* its target rather
//! // than exactly at it, because that is the equilibrium distribution (FR-013).
//! let seeded = pool.live();
//! assert!((30..=70).contains(&seeded));
//!
//! // Turns belong to the loop; a session ends when its last one is taken.
//! let first = pool.handles().next().unwrap();
//! while pool.session_mut(first).take_turn().is_some() {}
//! let done = pool.finish(first, &mut ids, &mut rng);
//!
//! assert_eq!(done.turns_remaining(), 0);
//! assert_eq!(pool.completed(), 1);
//! // A fluctuating class does not backfill; an `Exact` one would replace it here.
//! assert_eq!(pool.live(), seeded - 1);
//! ```

use rand::Rng;

use crate::description::Population;
use crate::distribution::Resolved;
use crate::keys::MAX_SESSIONS_PER_RUN;
use crate::pool::{exponential, poisson, ResidualLife};

/// Run-global monotonic session identity.
///
/// Session ids must be unique across session **classes**, not merely within one.
/// The key salt for a session's own blocks carries no class field —
/// [`crate::keys::input_salt`] takes a session id and a block
/// ordinal and nothing else — so two classes each numbering from zero would
/// derive *identical keys* for unrelated sessions. That would show up as
/// inexplicable cross-class cache hits and would be very hard to trace back to
/// its cause, so the allocator is a distinct type rather than a counter inside
/// each pool.
#[derive(Debug, Clone, Default)]
pub struct SessionIds {
    next: u64,
}

impl SessionIds {
    /// A fresh allocator, starting at zero.
    ///
    /// # Examples
    ///
    /// ```
    /// use workload_model::session::SessionIds;
    ///
    /// let mut ids = SessionIds::new();
    /// assert_eq!(ids.issued(), 0);
    /// ```
    pub fn new() -> Self {
        Self::default()
    }

    /// Identities issued so far, which is also the next id.
    pub fn issued(&self) -> u64 {
        self.next
    }

    /// Take the next identity.
    ///
    /// # Panics
    ///
    /// Past [`MAX_SESSIONS_PER_RUN`], because the id would overflow its 38-bit
    /// salt field and alias another session's keys. The pre-flight projection
    /// checks this against the run's span before anything is written, so reaching
    /// it here means the projection was bypassed.
    fn take(&mut self) -> u64 {
        assert!(
            self.next < MAX_SESSIONS_PER_RUN,
            "session id {} reaches the {MAX_SESSIONS_PER_RUN}-session salt limit; \
             keys would alias",
            self.next
        );
        let id = self.next;
        self.next += 1;
        id
    }
}

/// One live session.
///
/// Its turn schedule is fixed at birth; see the module docs on why. Turn
/// mechanics — the prefix chain and the blocks each turn mints — belong to the
/// turn model, not here.
#[derive(Debug, Clone)]
pub struct Session {
    id: u64,
    class_id: u64,
    born_at: f64,
    /// Absolute virtual time of each turn, ascending. Length is the turn count.
    turn_at: Vec<f64>,
    cursor: usize,
}

impl Session {
    /// Run-global identity, and the salt's `session_id`.
    pub fn id(&self) -> u64 {
        self.id
    }

    /// Declaration index of the session class this belongs to.
    pub fn class_id(&self) -> u64 {
        self.class_id
    }

    /// Virtual time this session was created.
    pub fn born_at(&self) -> f64 {
        self.born_at
    }

    /// Turns this session will take in total.
    ///
    /// For a session created by seeding this is its *residual* count, which is
    /// less than a full session's; see the module docs.
    pub fn turns_total(&self) -> usize {
        self.turn_at.len()
    }

    /// Turns already taken.
    pub fn turns_taken(&self) -> usize {
        self.cursor
    }

    /// Turns not yet taken.
    pub fn turns_remaining(&self) -> usize {
        self.turn_at.len() - self.cursor
    }

    /// Virtual time of the next turn, or `None` when the session is spent.
    pub fn next_turn_at(&self) -> Option<f64> {
        self.turn_at.get(self.cursor).copied()
    }

    /// Virtual time this session's last turn falls, which is when it dies.
    ///
    /// A session with no turns at all cannot occur — a turn count is truncated
    /// to at least one — so this is always a real time.
    pub fn dies_at(&self) -> f64 {
        *self
            .turn_at
            .last()
            .expect("a session always has at least one turn")
    }

    /// Consume the next turn, returning the virtual time it happens at.
    ///
    /// The turn model calls this; the population does not, because a session's
    /// death time is already known from its schedule.
    pub fn take_turn(&mut self) -> Option<f64> {
        let at = self.turn_at.get(self.cursor).copied()?;
        self.cursor += 1;
        Some(at)
    }
}

/// The live sessions of one session class, and their population dynamics.
#[derive(Debug, Clone)]
pub struct SessionPool {
    class_id: u64,
    form: Population,
    turns: Resolved,
    think_time: Resolved,
    /// Discrete equilibrium residual of the turn count, for seeding.
    residual_turns: ResidualLife,
    /// Arena of live sessions. A slab rather than a packed `Vec`, because
    /// callers hold handles across a step: compacting on every death — with
    /// `swap_remove`, say — would silently invalidate the handle of whichever
    /// session got moved, and binding would then be applied to the wrong session.
    live: Vec<Option<Session>>,
    free_handles: Vec<usize>,
    live_count: usize,
    next_birth_at: f64,
    mean_duration: f64,
    arrival_rate: f64,
    started: u64,
    completed: u64,
    /// Handles of sessions created at the last `seed` or `advance_births_to`.
    born: Vec<usize>,
    now: f64,
    seeded: bool,
}

impl SessionPool {
    /// Build a pool for one session class.
    ///
    /// `turns` must be integral. `rng` is used only to build the residual-turn
    /// distribution, so it should be a dedicated substream — otherwise adding a
    /// class would shift every later draw in the run.
    pub fn new<R: Rng + ?Sized>(
        class_id: u64,
        form: Population,
        turns: Resolved,
        think_time: Resolved,
        rng: &mut R,
    ) -> Self {
        let residual_turns = ResidualLife::build(&turns, rng);
        // FR-024, Little's law. Nothing draws a session lifetime; it is this.
        let mean_duration = turns.effective().mean * think_time.effective().mean;
        let arrival_rate = match form {
            Population::Poisson(n) if mean_duration > 0.0 && mean_duration.is_finite() => {
                n as f64 / mean_duration
            }
            _ => 0.0,
        };
        Self {
            class_id,
            form,
            turns,
            think_time,
            residual_turns,
            live: Vec::new(),
            free_handles: Vec::new(),
            live_count: 0,
            next_birth_at: f64::INFINITY,
            mean_duration,
            arrival_rate,
            started: 0,
            completed: 0,
            born: Vec::new(),
            now: 0.0,
            seeded: false,
        }
    }

    /// The class's declaration index.
    pub fn class_id(&self) -> u64 {
        self.class_id
    }

    /// Mean session duration, `E[turns] * E[think_time]` — emergent, never
    /// written (FR-024).
    pub fn mean_duration(&self) -> f64 {
        self.mean_duration
    }

    /// Sessions per virtual second, `size / E[duration]`, or zero for an exact
    /// pool, which replaces each death instead of arriving freely.
    ///
    /// This is an **output** of the description and not a parameter of it.
    pub fn arrival_rate(&self) -> f64 {
        self.arrival_rate
    }

    /// The target concurrency.
    pub fn nominal(&self) -> u64 {
        self.form.nominal()
    }

    /// Sessions currently live.
    pub fn live(&self) -> usize {
        self.live_count
    }

    /// Sessions created since the run began, seeded ones included.
    pub fn started(&self) -> u64 {
        self.started
    }

    /// Sessions that have taken their last turn.
    pub fn completed(&self) -> u64 {
        self.completed
    }

    /// The live sessions, in arena order, which is not creation order.
    pub fn sessions(&self) -> impl Iterator<Item = &Session> {
        self.live.iter().flatten()
    }

    /// Seed the pool at `t0`, each session part-way through its conversation.
    ///
    /// # Panics
    ///
    /// If called twice, or if the mean session duration is not positive and
    /// finite — a class whose think time has mean zero would need an infinite
    /// arrival rate to sustain any concurrency at all. Load-time validation
    /// refuses that, so reaching it here means validation was bypassed.
    pub fn seed<R: Rng + ?Sized>(&mut self, t0: f64, ids: &mut SessionIds, rng: &mut R) {
        assert!(!self.seeded, "session pool seeded twice");
        assert!(
            self.mean_duration > 0.0 && self.mean_duration.is_finite(),
            "class {} has a mean session duration of {}; concurrency cannot be \
             sustained and the arrival rate is undefined",
            self.class_id,
            self.mean_duration
        );
        self.seeded = true;
        self.now = t0;
        let initial = match self.form {
            Population::Exact(n) => n,
            Population::Poisson(n) => poisson(rng, n as f64),
        };
        self.born.clear();
        for _ in 0..initial {
            // Discrete equilibrium residual: a length-biased turn count, then a
            // uniform point within it. `ceil` turns a uniform point in `[0, T)`
            // into a uniform integer in `1..=T`, which is the discrete form.
            let residual = self.residual_turns.sample(rng).ceil().max(1.0);
            let turns = residual as usize;
            self.create(t0, turns, ids, rng);
        }
        if self.arrival_rate > 0.0 {
            self.next_birth_at = t0 + exponential(rng, 1.0 / self.arrival_rate);
        }
    }

    /// The next virtual time this pool creates a session on its own, if ever.
    ///
    /// Only a [`Population::Poisson`] class has one: an exact class arrives only
    /// in response to a death, so it has no schedule of its own. This is **not**
    /// the next turn and not a death — the loop takes the earliest of this and its
    /// own turn schedule.
    pub fn next_event_at(&self) -> Option<f64> {
        if self.next_birth_at.is_finite() {
            Some(self.next_birth_at)
        } else {
            None
        }
    }

    /// Create every session whose free-running arrival falls at or before `t`.
    ///
    /// A no-op for an exact class. New sessions are listed by
    /// [`Self::newly_born`], and the caller must bind their shared instances
    /// before their first turn.
    pub fn advance_births_to<R: Rng + ?Sized>(
        &mut self,
        t: f64,
        ids: &mut SessionIds,
        rng: &mut R,
    ) {
        assert!(self.seeded, "session pool advanced before being seeded");
        self.born.clear();
        while self.next_birth_at <= t {
            let at = self.next_birth_at;
            let turns = self.draw_turns(rng);
            self.create(at, turns, ids, rng);
            self.next_birth_at = at + exponential(rng, 1.0 / self.arrival_rate);
            self.now = at;
        }
        self.now = self.now.max(t);
    }

    /// Retire the session at `handle`, whose last turn has been taken, and
    /// replace it at once if this is an exact class.
    ///
    /// Returns the finished session so the caller can release its shared-instance
    /// holds — which only the caller can do, since the holds belong to other
    /// pools. A replacement is a **new** session with a fresh identity and a full
    /// turn count: it inherits nothing, because a session's identity is what its
    /// keys are derived from.
    ///
    /// # Panics
    ///
    /// If the handle is not live, or if the session still has turns to take —
    /// retiring early would drop operations from the plan silently, which is the
    /// one failure here that no statistic would reveal.
    pub fn finish<R: Rng + ?Sized>(
        &mut self,
        handle: usize,
        ids: &mut SessionIds,
        rng: &mut R,
    ) -> Session {
        let session = self.live[handle].take().expect("finish on a dead handle");
        assert_eq!(
            session.turns_remaining(),
            0,
            "session {} finished with {} turns unspent; those operations would \
             vanish from the plan without a trace",
            session.id(),
            session.turns_remaining()
        );
        self.free_handles.push(handle);
        self.live_count -= 1;
        self.completed += 1;
        let at = session.dies_at();
        if let Population::Exact(_) = self.form {
            // Appends rather than clears: a loop step calls `advance_births_to`
            // once and then `finish` several times, and the caller needs every
            // session created during that step, not just the last one.
            let turns = self.draw_turns(rng);
            self.create(at, turns, ids, rng);
        }
        self.now = self.now.max(at);
        session
    }

    /// Handles of the live sessions, in arena order.
    ///
    /// Stable across deaths, which is why they are handles and not indices into a
    /// packed list.
    pub fn handles(&self) -> impl Iterator<Item = usize> + '_ {
        self.live
            .iter()
            .enumerate()
            .filter_map(|(h, s)| s.as_ref().map(|_| h))
    }

    /// The session at `handle`.
    ///
    /// # Panics
    ///
    /// If the handle is not live.
    pub fn session(&self, handle: usize) -> &Session {
        self.live[handle].as_ref().expect("dead session handle")
    }

    /// The session at `handle`, mutably, so the turn model can consume turns.
    ///
    /// # Panics
    ///
    /// If the handle is not live.
    pub fn session_mut(&mut self, handle: usize) -> &mut Session {
        self.live[handle].as_mut().expect("dead session handle")
    }

    /// Handles of sessions created since the last [`Self::seed`] or
    /// [`Self::advance_births_to`], so the caller can bind their shared instances
    /// before their first turn.
    ///
    /// Those two calls clear it; [`Self::finish`] **appends** to it. A loop step
    /// advances births once and then finishes several sessions, so the caller
    /// needs everything created during the whole step.
    pub fn newly_born(&self) -> &[usize] {
        &self.born
    }

    /// A full turn count, for a session born during the run rather than seeded.
    fn draw_turns<R: Rng + ?Sized>(&self, rng: &mut R) -> usize {
        self.turns.sample_int(rng).max(1) as usize
    }

    /// Create a session with `turns` turns, its whole schedule drawn now.
    fn create<R: Rng + ?Sized>(
        &mut self,
        born_at: f64,
        turns: usize,
        ids: &mut SessionIds,
        rng: &mut R,
    ) {
        debug_assert!(turns >= 1, "a session must have at least one turn");
        let mut turn_at = Vec::with_capacity(turns);
        let mut t = born_at;
        for _ in 0..turns {
            // Think time comes *before* each turn, so a session's span is the sum
            // of its think times and its first turn is never at its birth instant
            // unless think time is zero.
            t += self.think_time.sample(rng).max(0.0);
            turn_at.push(t);
        }
        let session = Session {
            id: ids.take(),
            class_id: self.class_id,
            born_at,
            turn_at,
            cursor: 0,
        };
        let handle = match self.free_handles.pop() {
            Some(h) => {
                debug_assert!(self.live[h].is_none(), "reused a live handle");
                self.live[h] = Some(session);
                h
            }
            None => {
                self.live.push(Some(session));
                self.live.len() - 1
            }
        };
        self.live_count += 1;
        self.born.push(handle);
        self.started += 1;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::distribution::{Distribution, Kind};
    use crate::rng;

    fn resolved(kind: Kind, integral: bool) -> Resolved {
        let d = Distribution::new(kind);
        if integral {
            d.resolve_integral(None).unwrap()
        } else {
            d.resolve(None).unwrap()
        }
    }

    fn pool(form: Population, turns: Kind, think: Kind) -> (SessionPool, SessionIds, rng::Rng) {
        let mut r = rng::substream(21, "sessions");
        let p = SessionPool::new(
            0,
            form,
            resolved(turns, true),
            resolved(think, false),
            &mut r,
        );
        (p, SessionIds::new(), r)
    }

    /// What one step of the simulation loop (T029) does, reduced to the part this
    /// module owns: create arrivals, take every turn now due, and finish any
    /// session whose schedule is spent.
    ///
    /// Written here rather than waiting for T029 because a session pool cannot be
    /// exercised at all without something driving turns — its deaths are
    /// turn-driven by design.
    #[derive(Default)]
    struct Driven {
        finished: Vec<Session>,
        born: usize,
    }

    fn drive<R: Rng + ?Sized>(
        p: &mut SessionPool,
        t: f64,
        ids: &mut SessionIds,
        rng: &mut R,
    ) -> Driven {
        let mut out = Driven::default();
        loop {
            p.advance_births_to(t, ids, rng);
            let mut created = p.newly_born().len();
            let due: Vec<usize> = p
                .handles()
                .filter(|h| p.session(*h).next_turn_at().is_some_and(|a| a <= t))
                .collect();
            if due.is_empty() {
                out.born += created;
                return out;
            }
            for h in due {
                while p.session_mut(h).next_turn_at().is_some_and(|a| a <= t) {
                    p.session_mut(h).take_turn();
                }
                if p.session(h).turns_remaining() == 0 {
                    let before = p.newly_born().len();
                    out.finished.push(p.finish(h, ids, rng));
                    created += p.newly_born().len() - before;
                }
            }
            out.born += created;
        }
    }

    #[test]
    fn duration_is_emergent_and_the_arrival_rate_follows_from_it() {
        // FR-024: nothing writes a session lifetime. It is E[turns] * E[think],
        // and the arrival rate is the target concurrency divided by it.
        let (p, _, _) = pool(
            Population::Poisson(200),
            Kind::Constant { value: 12.0 },
            Kind::Exponential { mean: 25.0 },
        );
        assert!((p.mean_duration() - 300.0).abs() < 1e-9);
        assert!((p.arrival_rate() - 200.0 / 300.0).abs() < 1e-9);
    }

    #[test]
    fn an_exact_class_has_no_arrival_schedule_of_its_own() {
        // It arrives only in response to a death, so it has no event to report.
        let (mut p, mut ids, mut r) = pool(
            Population::Exact(10),
            Kind::Constant { value: 4.0 },
            Kind::Constant { value: 5.0 },
        );
        assert_eq!(p.arrival_rate(), 0.0);
        p.seed(0.0, &mut ids, &mut r);
        assert_eq!(p.next_event_at(), None);
    }

    #[test]
    fn an_exact_class_holds_its_concurrency_exactly() {
        let (mut p, mut ids, mut r) = pool(
            Population::Exact(40),
            Kind::Constant { value: 5.0 },
            Kind::Exponential { mean: 10.0 },
        );
        p.seed(0.0, &mut ids, &mut r);
        assert_eq!(p.live(), 40);
        for step in 1..=40 {
            drive(&mut p, step as f64 * 50.0, &mut ids, &mut r);
            assert_eq!(p.live(), 40, "concurrency drifted");
        }
        assert!(p.completed() > 40, "no session turnover at all");
        assert_eq!(p.started(), p.completed() + 40);
    }

    #[test]
    fn a_poisson_class_realises_the_arrival_rate_littles_law_predicts() {
        // What makes "arrival rate is an output" a claim rather than a definition:
        // the realised birth count over a long run must match rate * span, and the
        // realised concurrency must match the target.
        let n = 150u64;
        let (mut p, mut ids, mut r) = pool(
            Population::Poisson(n),
            Kind::Constant { value: 8.0 },
            Kind::Exponential { mean: 20.0 },
        );
        let span = 40_000.0;
        let steps = 400;
        p.seed(0.0, &mut ids, &mut r);
        let mut concurrency = Vec::new();
        for step in 1..=steps {
            drive(
                &mut p,
                step as f64 * (span / steps as f64),
                &mut ids,
                &mut r,
            );
            concurrency.push(p.live() as f64);
        }
        let mean_live = concurrency.iter().sum::<f64>() / concurrency.len() as f64;
        assert!(
            (mean_live - n as f64).abs() < 0.1 * n as f64,
            "mean concurrency {mean_live:.1} against a target of {n}"
        );
        let realised = (p.started() - n) as f64 / span;
        assert!(
            (realised - p.arrival_rate()).abs() < 0.15 * p.arrival_rate(),
            "realised arrival rate {realised:.4} against a predicted {:.4}",
            p.arrival_rate()
        );
    }

    #[test]
    fn a_poisson_session_population_fluctuates_like_a_real_one() {
        // The same argument FR-016 makes for shared pools: an M/G/inf population
        // has var/mean = 1, and suppressing that would be a fidelity loss.
        let n = 200u64;
        let (mut p, mut ids, mut r) = pool(
            Population::Poisson(n),
            Kind::Constant { value: 6.0 },
            Kind::Exponential { mean: 15.0 },
        );
        p.seed(0.0, &mut ids, &mut r);
        let mut counts = Vec::new();
        for step in 1..=400 {
            drive(&mut p, step as f64 * 60.0, &mut ids, &mut r);
            counts.push(p.live() as f64);
        }
        let mean = counts.iter().sum::<f64>() / counts.len() as f64;
        let var = counts.iter().map(|c| (c - mean).powi(2)).sum::<f64>() / counts.len() as f64;
        assert!(var / mean > 0.4, "var/mean = {:.3}", var / mean);
    }

    #[test]
    fn seeded_sessions_have_residual_turn_counts_not_full_ones() {
        // The equilibrium argument in discrete form. Seeding every session at turn
        // 0 would make them all finish together; instead the seeded generation is
        // part-way through, so E[remaining] is about (T+1)/2 for a constant T and
        // the counts are spread over 1..=T.
        let t = 20.0;
        let (mut p, mut ids, mut r) = pool(
            Population::Exact(4000),
            Kind::Constant { value: t },
            Kind::Constant { value: 1.0 },
        );
        p.seed(0.0, &mut ids, &mut r);
        let totals: Vec<usize> = p.sessions().map(|s| s.turns_total()).collect();
        let mean = totals.iter().sum::<usize>() as f64 / totals.len() as f64;
        let expect = (t + 1.0) / 2.0;
        assert!(
            (mean - expect).abs() < 0.5,
            "mean residual turn count {mean:.2}, expected about {expect:.2}"
        );
        assert_eq!(
            *totals.iter().min().unwrap(),
            1,
            "no session was nearly done"
        );
        assert_eq!(
            *totals.iter().max().unwrap(),
            t as usize,
            "no session was freshly started"
        );
    }

    #[test]
    fn sessions_born_during_the_run_get_full_turn_counts() {
        // The counterpart: the residual applies only at t = 0. A replacement is a
        // new conversation, not the tail of an old one.
        let t = 15.0;
        let (mut p, mut ids, mut r) = pool(
            Population::Exact(50),
            Kind::Constant { value: t },
            Kind::Constant { value: 2.0 },
        );
        p.seed(0.0, &mut ids, &mut r);
        drive(&mut p, 5_000.0, &mut ids, &mut r);
        assert!(p.completed() >= 50, "the seeded generation is not gone");
        for s in p.sessions() {
            assert_eq!(
                s.turns_total(),
                t as usize,
                "a session born during the run has a residual turn count"
            );
        }
    }

    #[test]
    fn the_turn_schedule_is_ascending_and_ends_at_the_death_time() {
        let (mut p, mut ids, mut r) = pool(
            Population::Exact(30),
            Kind::Normal {
                mean: 8.0,
                sigma: 3.0,
            },
            Kind::Exponential { mean: 5.0 },
        );
        p.seed(0.0, &mut ids, &mut r);
        drive(&mut p, 400.0, &mut ids, &mut r);
        for s in p.sessions() {
            assert!(s.turns_total() >= 1);
            let mut previous = s.born_at();
            for k in 0..s.turns_total() {
                let at = s.turn_at[k];
                assert!(at >= previous, "turn {k} at {at} precedes {previous}");
                previous = at;
            }
            assert_eq!(s.dies_at().to_bits(), previous.to_bits());
        }
    }

    #[test]
    fn taking_turns_walks_the_schedule_and_then_stops() {
        let (mut p, mut ids, mut r) = pool(
            Population::Exact(1),
            Kind::Constant { value: 4.0 },
            Kind::Constant { value: 10.0 },
        );
        p.seed(0.0, &mut ids, &mut r);
        let h = p.handles().next().unwrap();
        let total = p.session(h).turns_total();
        let mut seen = Vec::new();
        while let Some(at) = p.session_mut(h).take_turn() {
            seen.push(at);
        }
        assert_eq!(seen.len(), total);
        assert_eq!(p.session(h).turns_remaining(), 0);
        assert_eq!(p.session(h).turns_taken(), total);
        assert!(p.session(h).next_turn_at().is_none());
        assert!(seen.windows(2).all(|w| w[0] <= w[1]));
    }

    #[test]
    #[should_panic(expected = "turns unspent")]
    fn finishing_a_session_early_is_refused() {
        // Retiring a session with turns left would drop operations from the plan,
        // and no statistic downstream would reveal it — so it must be loud.
        let (mut p, mut ids, mut r) = pool(
            Population::Exact(1),
            Kind::Constant { value: 6.0 },
            Kind::Constant { value: 1.0 },
        );
        p.seed(0.0, &mut ids, &mut r);
        let h = p.handles().next().unwrap();
        p.session_mut(h).take_turn();
        p.finish(h, &mut ids, &mut r);
    }

    #[test]
    fn a_handle_survives_other_sessions_dying() {
        // Why the arena is a slab. A caller holds handles across a step, so
        // compacting on death would silently move some other session under an
        // existing handle, and its shared instances would then be bound to the
        // wrong conversation.
        let (mut p, mut ids, mut r) = pool(
            Population::Exact(6),
            Kind::Constant { value: 3.0 },
            Kind::Constant { value: 1.0 },
        );
        p.seed(0.0, &mut ids, &mut r);
        let handles: Vec<usize> = p.handles().collect();
        let watched = *handles.last().unwrap();
        let watched_id = p.session(watched).id();

        let victim = handles[0];
        while p.session_mut(victim).take_turn().is_some() {}
        p.finish(victim, &mut ids, &mut r);

        assert_eq!(
            p.session(watched).id(),
            watched_id,
            "a handle now names a different session"
        );
    }

    #[test]
    fn session_ids_are_unique_across_classes() {
        // The reason the allocator is its own type: a session's key salt has no
        // class field, so two classes numbering from zero would derive identical
        // keys for unrelated sessions.
        let mut r = rng::substream(4, "sessions");
        let mut ids = SessionIds::new();
        let mut seen = std::collections::BTreeSet::new();
        for class_id in 0..3u64 {
            let mut p = SessionPool::new(
                class_id,
                Population::Exact(20),
                resolved(Kind::Constant { value: 3.0 }, true),
                resolved(Kind::Constant { value: 1.0 }, false),
                &mut r,
            );
            p.seed(0.0, &mut ids, &mut r);
            let done = drive(&mut p, 200.0, &mut ids, &mut r);
            for s in p.sessions() {
                assert!(seen.insert(s.id()), "duplicate session id {}", s.id());
                assert_eq!(s.class_id(), class_id);
            }
            for s in done.finished {
                assert!(seen.insert(s.id()), "duplicate finished id {}", s.id());
            }
        }
        assert_eq!(seen.len() as u64, ids.issued());
    }

    #[test]
    fn finished_sessions_are_handed_back_exactly_once() {
        // The caller releases each finished session's shared-instance holds, so a
        // session that ends without being handed over is a leak, and one handed
        // over twice is a double release.
        let (mut p, mut ids, mut r) = pool(
            Population::Exact(25),
            Kind::Constant { value: 3.0 },
            Kind::Exponential { mean: 4.0 },
        );
        p.seed(0.0, &mut ids, &mut r);
        let mut collected = std::collections::BTreeSet::new();
        for step in 1..=200 {
            for s in drive(&mut p, step as f64 * 5.0, &mut ids, &mut r).finished {
                assert_eq!(s.turns_remaining(), 0);
                assert!(collected.insert(s.id()), "session {} ended twice", s.id());
            }
        }
        assert_eq!(
            collected.len() as u64,
            p.completed(),
            "handed back {} against a completed count of {}",
            collected.len(),
            p.completed()
        );
        assert!(p.completed() > 25, "no turnover, so nothing was tested");
    }

    #[test]
    fn every_session_created_is_reported_for_binding() {
        // A session must have its shared instances bound before its first turn, so
        // a birth the caller never hears about is a session with no shared prefix
        // at all — which would look like a workload, just the wrong one.
        let (mut p, mut ids, mut r) = pool(
            Population::Poisson(30),
            Kind::Constant { value: 4.0 },
            Kind::Exponential { mean: 10.0 },
        );
        p.seed(0.0, &mut ids, &mut r);
        assert_eq!(p.newly_born().len(), p.live());
        let mut total = p.newly_born().len() as u64;
        for step in 1..=200 {
            total += drive(&mut p, step as f64 * 20.0, &mut ids, &mut r).born as u64;
        }
        assert_eq!(total, p.started(), "births were not all reported");
    }

    #[test]
    fn seeding_is_reproducible_from_a_seed() {
        let snapshot = |seed: u64| {
            let mut r = rng::substream(seed, "sessions");
            let mut ids = SessionIds::new();
            let mut p = SessionPool::new(
                0,
                Population::Poisson(20),
                resolved(Kind::Constant { value: 6.0 }, true),
                resolved(Kind::Exponential { mean: 8.0 }, false),
                &mut r,
            );
            p.seed(0.0, &mut ids, &mut r);
            drive(&mut p, 500.0, &mut ids, &mut r);
            p.sessions()
                .map(|s| (s.id(), s.turns_total(), s.dies_at().to_bits()))
                .collect::<Vec<_>>()
        };
        assert_eq!(snapshot(7), snapshot(7));
        assert_ne!(snapshot(7), snapshot(8));
    }
}
