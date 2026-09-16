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
//! use workload_model::session::{bind, SessionIds, SessionPool};
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
//! // Every session must be bound before its first turn (FR-028). This class
//! // has no `uses`, so it binds nothing — but it still has to be bound.
//! for h in pool.handles().collect::<Vec<_>>() {
//!     bind(pool.session_mut(h), &[], &mut [], &mut [], &mut rng);
//! }
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
use crate::pool::{exponential, poisson, Held, ResidualLife, SharedPool};
use crate::selection::Selector;

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

/// One shared instance a session holds, at one position in its prefix.
///
/// Carries the hold, so that dropping a session without giving these back is a
/// leak the type system makes visible ([`Held`] is not `Clone`).
#[derive(Debug, PartialEq)]
pub struct Bound {
    held: Held,
    length_blocks: u64,
}

impl Bound {
    /// Declaration index of the shared class this instance belongs to.
    pub fn class_id(&self) -> u64 {
        self.held.class_id()
    }

    /// Key identity of the instance — the salt's `instance_index`.
    pub fn mint(&self) -> u64 {
        self.held.mint()
    }

    /// Blocks this instance contributes to the prefix.
    pub fn length_blocks(&self) -> u64 {
        self.length_blocks
    }

    /// The hold, for reading the instance out of its pool.
    pub fn held(&self) -> &Held {
        &self.held
    }

    /// The hold, to be released into the instance's own pool.
    pub fn into_held(self) -> Held {
        self.held
    }
}

/// One live session.
///
/// Its turn schedule is fixed at birth; see the module docs on why. Turn
/// mechanics — the prefix chain and the blocks each turn mints — belong to the
/// turn model, not here.
#[derive(Debug)]
pub struct Session {
    id: u64,
    class_id: u64,
    born_at: f64,
    /// Absolute virtual time of each turn, ascending. Length is the turn count.
    turn_at: Vec<f64>,
    cursor: usize,
    /// Shared instances in canonical order (FR-028). Empty until bound, which is
    /// why `bound` is a separate flag — a class with no `uses` binds nothing.
    chosen: Vec<Bound>,
    bound: bool,
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
        debug_assert!(
            self.bound,
            "session {} took a turn before its shared instances were bound; its \
             prefix would be missing every shared block",
            self.id
        );
        let at = self.turn_at.get(self.cursor).copied()?;
        self.cursor += 1;
        Some(at)
    }

    /// Whether this session's shared instances have been bound.
    pub fn is_bound(&self) -> bool {
        self.bound
    }

    /// The session's shared instances, in canonical order (FR-028).
    pub fn chosen(&self) -> &[Bound] {
        &self.chosen
    }

    /// Take the shared instances back, so their holds can be released into their
    /// own pools. Called once, when the session has finished.
    pub fn take_chosen(&mut self) -> Vec<Bound> {
        std::mem::take(&mut self.chosen)
    }
}

/// A session class's reference to one shared class: which class, and how many
/// instances a session of this class draws from it.
///
/// The **order of a slice of these is semantic** — it is the order the session
/// class listed them in, and it fixes where each class's blocks land in the prefix
/// (FR-028).
#[derive(Debug, Clone)]
pub struct Uses {
    /// Declaration index of the shared class.
    pub class_index: usize,
    /// How many instances to draw. Integral, and capped by the pool at load.
    pub count: Resolved,
}

/// Draw and acquire a session's shared instances, in canonical order (FR-028).
///
/// `uses` is in the order the session class listed, `pools` and `selectors` are
/// indexed by shared-class declaration index. Every drawn instance is acquired, so
/// the session holds each one until it finishes — see
/// [`Session::take_chosen`].
///
/// # The ordering, and why it is the whole reason sharing works
///
/// Two rules, and the second is the one that is easy to get wrong:
///
/// 1. **Classes in the order the session class lists them.** That order is part of
///    the description, not an implementation detail.
/// 2. **Instances sorted within a class**, which [`Selector::draw`] already
///    guarantees by returning slots ascending.
///
/// Together they make a session's prefix a pure function of the *set* it drew. So
/// a session that drew `{0, 1}` and one that drew `{0, 1, 4}` produce **nested**
/// chains — the second extends the first — rather than divergent ones. Without the
/// sort, the same two sets could be laid out as `[1, 0]` and `[0, 1, 4]` and share
/// nothing at all, because keys are prefix-chained and the first block already
/// differs.
///
/// This is also why ordering is done here rather than left to the caller:
/// assembling the list in the wrong order produces a workload that runs, reports
/// plausible numbers, and has almost no cross-session reuse.
///
/// # Examples
///
/// ```
/// use workload_model::description::{Population, RankBy};
/// use workload_model::distribution::{Distribution, Kind};
/// use workload_model::pool::SharedPool;
/// use workload_model::selection::Selector;
/// use workload_model::session::{bind, SessionIds, SessionPool, Uses};
/// use workload_model::rng;
///
/// let forever = Distribution::new(Kind::Constant { value: f64::INFINITY })
///     .resolve(None)
///     .unwrap();
/// let four = Distribution::new(Kind::Constant { value: 4.0 })
///     .resolve_integral(None)
///     .unwrap();
/// let mut rng = rng::substream(1, "bind");
///
/// let mut docs = SharedPool::new(0, Population::Exact(8), forever, four.clone(), &mut rng);
/// docs.seed(0.0, &mut rng);
/// let mut pools = vec![docs];
/// let mut selectors = vec![Selector::new(RankBy::Slot)];
///
/// let turns = Distribution::new(Kind::Constant { value: 3.0 })
///     .resolve_integral(None)
///     .unwrap();
/// let think = Distribution::new(Kind::Constant { value: 1.0 })
///     .resolve(None)
///     .unwrap();
/// let mut ids = SessionIds::new();
/// let mut sessions = SessionPool::new(0, Population::Exact(1), turns, think, &mut rng);
/// sessions.seed(0.0, &mut ids, &mut rng);
/// let h = sessions.handles().next().unwrap();
///
/// let uses = vec![Uses { class_index: 0, count: four }];
/// bind(sessions.session_mut(h), &uses, &mut pools, &mut selectors, &mut rng);
///
/// let chosen = sessions.session(h).chosen();
/// assert_eq!(chosen.len(), 4);
/// // Sorted within the class, which is what makes overlapping sessions nest.
/// let slots: Vec<u32> = chosen.iter().map(|b| pools[0].held(b.held()).slot()).collect();
/// assert!(slots.windows(2).all(|w| w[0] < w[1]));
///
/// // Give the holds back when the session finishes.
/// for b in sessions.session_mut(h).take_chosen() {
///     pools[0].release(b.into_held());
/// }
/// ```
///
/// # Panics
///
/// If the session is already bound, or if a `Uses` names a class index outside
/// `pools`.
pub fn bind<R: Rng + ?Sized>(
    session: &mut Session,
    uses: &[Uses],
    pools: &mut [SharedPool],
    selectors: &mut [Selector],
    rng: &mut R,
) {
    assert!(
        !session.bound,
        "session {} bound twice; its first holds would leak",
        session.id
    );
    for u in uses {
        let idx = u.class_index;
        assert!(
            idx < pools.len() && idx < selectors.len(),
            "uses names shared class {idx}, but only {} are declared",
            pools.len()
        );
        let count = u.count.sample_int(rng).max(0) as u64;
        // Slots come back ascending, which is rule 2. Copied out because acquiring
        // needs the pool mutably while the selector still owns its buffer.
        let slots: Vec<u32> = selectors[idx].draw(&pools[idx], count, rng).to_vec();
        for slot in slots {
            let held = pools[idx]
                .acquire(slot)
                .expect("selection returned an unoccupied slot");
            let length_blocks = pools[idx].held(&held).length_blocks();
            session.chosen.push(Bound {
                held,
                length_blocks,
            });
        }
    }
    session.bound = true;
    debug_assert!(
        is_canonical(&session.chosen, uses),
        "bound instances are not in canonical order"
    );
}

/// Whether a bound list satisfies FR-028: classes in `uses` order, mints of
/// ascending slot within a class.
///
/// Checks what it can from the outside — class grouping and order. The
/// within-class sort is [`Selector::draw`]'s guarantee and is asserted there.
fn is_canonical(chosen: &[Bound], uses: &[Uses]) -> bool {
    let mut expected = uses.iter().map(|u| u.class_index as u64);
    let mut current: Option<u64> = None;
    for b in chosen {
        if current != Some(b.class_id()) {
            // A new class must be the next one `uses` lists, skipping any that
            // drew nothing.
            loop {
                match expected.next() {
                    Some(c) if c == b.class_id() => break,
                    Some(_) => continue,
                    None => return false,
                }
            }
            current = Some(b.class_id());
        }
    }
    true
}

/// The live sessions of one session class, and their population dynamics.
///
/// Deliberately **not** `Clone`: its sessions hold refcounted shared instances, so
/// a clone would duplicate every [`Held`] without incrementing the refcount, and
/// releasing both copies would free an instance still in use. [`Held`] not being
/// `Clone` is what makes that a compile error rather than a leak to find later.
#[derive(Debug)]
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
            chosen: Vec::new(),
            bound: false,
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
    use crate::description::RankBy;
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

    /// Bind every session that has not been bound yet.
    ///
    /// These population tests use classes with no `uses`, so binding is empty —
    /// but it must still happen, because `take_turn` refuses an unbound session: a
    /// session that took a turn before binding would have a prefix missing every
    /// shared block, which is a workload, just not the one asked for.
    fn bind_new<R: Rng + ?Sized>(p: &mut SessionPool, rng: &mut R) {
        for h in p.handles().collect::<Vec<_>>() {
            if !p.session(h).is_bound() {
                bind(p.session_mut(h), &[], &mut [], &mut [], rng);
            }
        }
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
            bind_new(p, rng);
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
        bind_new(&mut p, &mut r);
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
        bind_new(&mut p, &mut r);
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
        bind_new(&mut p, &mut r);
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

    // ---- T026: canonical ordering (FR-028) ----------------------------------

    /// A shared class with `n` immortal instances, so slots are stable while a
    /// binding test runs and the ordering is the only thing under test.
    fn shared(class_id: u64, n: u64, r: &mut rng::Rng) -> (SharedPool, Selector) {
        let life = Distribution::new(Kind::Constant {
            value: f64::INFINITY,
        })
        .resolve(None)
        .unwrap();
        let length = Distribution::new(Kind::Constant { value: 4.0 })
            .resolve_integral(None)
            .unwrap();
        let mut p = SharedPool::new(class_id, Population::Exact(n), life, length, r);
        p.seed(0.0, r);
        (p, Selector::new(RankBy::Slot))
    }

    fn uses_of(class_index: usize, count: f64) -> Uses {
        Uses {
            class_index,
            count: Distribution::new(Kind::Constant { value: count })
                .resolve_integral(None)
                .unwrap(),
        }
    }

    /// One unbound session, for binding tests.
    fn lone_session(ids: &mut SessionIds, r: &mut rng::Rng) -> (SessionPool, usize) {
        let mut p = SessionPool::new(
            0,
            Population::Exact(1),
            resolved(Kind::Constant { value: 3.0 }, true),
            resolved(Kind::Constant { value: 1.0 }, false),
            r,
        );
        p.seed(0.0, ids, r);
        let h = p.handles().next().unwrap();
        (p, h)
    }

    #[test]
    fn classes_appear_in_the_order_the_session_class_lists_them() {
        // Rule 1 of FR-028. The `uses` order is part of the description, so it
        // fixes where each class's blocks land in the prefix — reversing the list
        // must reverse the binding.
        let mut r = rng::substream(31, "bind");
        let mut ids = SessionIds::new();
        let (p0, s0) = shared(0, 6, &mut r);
        let (p1, s1) = shared(1, 6, &mut r);
        let (p2, s2) = shared(2, 6, &mut r);
        let mut pools = vec![p0, p1, p2];
        let mut sels = vec![s0, s1, s2];

        for order in [vec![0usize, 1, 2], vec![2, 0, 1]] {
            let uses: Vec<Uses> = order.iter().map(|i| uses_of(*i, 2.0)).collect();
            let (mut sp, h) = lone_session(&mut ids, &mut r);
            bind(sp.session_mut(h), &uses, &mut pools, &mut sels, &mut r);
            let classes: Vec<u64> = sp
                .session(h)
                .chosen()
                .iter()
                .map(|b| b.class_id())
                .collect();
            let want: Vec<u64> = order.iter().flat_map(|i| [*i as u64; 2]).collect();
            assert_eq!(classes, want, "classes are not in `uses` order");
            for b in sp.session_mut(h).take_chosen() {
                let c = b.class_id() as usize;
                pools[c].release(b.into_held());
            }
        }
    }

    #[test]
    fn instances_are_sorted_within_a_class() {
        // Rule 2. `Selector::draw` guarantees it; this checks the binding does not
        // undo it, which is the only way it could be lost here.
        let mut r = rng::substream(32, "bind");
        let mut ids = SessionIds::new();
        let (p0, s0) = shared(0, 12, &mut r);
        let mut pools = vec![p0];
        let mut sels = vec![s0];
        let uses = vec![uses_of(0, 5.0)];
        for _ in 0..200 {
            let (mut sp, h) = lone_session(&mut ids, &mut r);
            bind(sp.session_mut(h), &uses, &mut pools, &mut sels, &mut r);
            let slots: Vec<u32> = sp
                .session(h)
                .chosen()
                .iter()
                .map(|b| pools[0].held(&b.held).slot())
                .collect();
            assert!(
                slots.windows(2).all(|w| w[0] < w[1]),
                "instances not ascending: {slots:?}"
            );
            for b in sp.session_mut(h).take_chosen() {
                pools[0].release(b.into_held());
            }
        }
    }

    #[test]
    fn overlapping_sets_produce_nested_chains_not_divergent_ones() {
        // **The reason canonical ordering exists.** Because the order is a function
        // of the set, two sessions agree on exactly the leading run their sorted
        // sets agree on — so `{0,1}` and `{0,1,4}` share their first two objects.
        // Keys are prefix-chained, so a shared leading run is the only thing that
        // produces reuse at all; a shuffled order would share nothing.
        let mut r = rng::substream(33, "bind");
        let mut ids = SessionIds::new();
        let (p0, s0) = shared(0, 10, &mut r);
        let mut pools = vec![p0];
        let mut sels = vec![s0];

        // Vary the count so the pairs genuinely differ in size.
        let mut checked = 0;
        let mut bindings: Vec<(Vec<u32>, Vec<u64>)> = Vec::new();
        for count in [2.0, 3.0, 4.0, 5.0] {
            let uses = vec![uses_of(0, count)];
            for _ in 0..40 {
                let (mut sp, h) = lone_session(&mut ids, &mut r);
                bind(sp.session_mut(h), &uses, &mut pools, &mut sels, &mut r);
                let slots: Vec<u32> = sp
                    .session(h)
                    .chosen()
                    .iter()
                    .map(|b| pools[0].held(&b.held).slot())
                    .collect();
                let mints: Vec<u64> = sp.session(h).chosen().iter().map(|b| b.mint()).collect();
                bindings.push((slots, mints));
                for b in sp.session_mut(h).take_chosen() {
                    pools[0].release(b.into_held());
                }
            }
        }

        for (a, b) in bindings.iter().zip(bindings.iter().skip(1)) {
            // The expected leading run comes from sorting each *set*
            // independently — not from the bound order — or the assertion would
            // be comparing the bound order against itself and hold whatever that
            // order was. (It did: an earlier version of this test passed with the
            // sort removed.)
            let mut sa = a.0.clone();
            let mut sb = b.0.clone();
            sa.sort_unstable();
            sb.sort_unstable();
            let expected = sa.iter().zip(&sb).take_while(|(x, y)| x == y).count();
            let actual = a.0.iter().zip(&b.0).take_while(|(x, y)| x == y).count();
            assert_eq!(
                actual, expected,
                "bound orders {:?} and {:?} share a leading run of {actual}, but \
                 their sorted sets share {expected} — the prefix is not a function \
                 of the set, so overlapping sessions would diverge instead of nest",
                a.0, b.0
            );
            // And the mints must track the slots, which is what makes a shared
            // leading run of slots a shared run of *blocks*.
            let mint_common = a.1.iter().zip(&b.1).take_while(|(x, y)| x == y).count();
            assert_eq!(mint_common, actual, "mints and slots disagree");
            checked += 1;
        }
        assert!(checked > 100, "not enough pairs compared");
    }

    #[test]
    fn a_class_that_draws_nothing_does_not_break_the_ordering() {
        // A count of zero, or a class whose pool is momentarily empty, must leave
        // the remaining classes in order rather than shifting them.
        let mut r = rng::substream(34, "bind");
        let mut ids = SessionIds::new();
        let (p0, s0) = shared(0, 4, &mut r);
        let (p1, s1) = shared(1, 4, &mut r);
        let mut pools = vec![p0, p1];
        let mut sels = vec![s0, s1];
        let uses = vec![uses_of(0, 0.0), uses_of(1, 2.0)];
        let (mut sp, h) = lone_session(&mut ids, &mut r);
        bind(sp.session_mut(h), &uses, &mut pools, &mut sels, &mut r);
        let classes: Vec<u64> = sp
            .session(h)
            .chosen()
            .iter()
            .map(|b| b.class_id())
            .collect();
        assert_eq!(classes, vec![1, 1]);
        for b in sp.session_mut(h).take_chosen() {
            pools[b.class_id() as usize].release(b.into_held());
        }
    }

    #[test]
    fn binding_holds_every_instance_it_drew() {
        // Each drawn instance must be acquired, or it could retire out from under a
        // session still reading it (FR-018).
        let mut r = rng::substream(35, "bind");
        let mut ids = SessionIds::new();
        let (p0, s0) = shared(0, 8, &mut r);
        let mut pools = vec![p0];
        let mut sels = vec![s0];
        let uses = vec![uses_of(0, 3.0)];
        let (mut sp, h) = lone_session(&mut ids, &mut r);
        bind(sp.session_mut(h), &uses, &mut pools, &mut sels, &mut r);
        assert_eq!(sp.session(h).chosen().len(), 3);
        for b in sp.session(h).chosen() {
            assert_eq!(pools[0].held(&b.held).users(), 1);
            assert_eq!(b.length_blocks(), 4);
        }
        for b in sp.session_mut(h).take_chosen() {
            pools[0].release(b.into_held());
        }
        assert!(pools[0].selectable().all(|i| i.users() == 0));
    }

    #[test]
    #[should_panic(expected = "bound twice")]
    fn binding_twice_is_refused() {
        // The first binding's holds would leak, and the session's prefix would
        // silently double in length.
        let mut r = rng::substream(36, "bind");
        let mut ids = SessionIds::new();
        let (p0, s0) = shared(0, 4, &mut r);
        let mut pools = vec![p0];
        let mut sels = vec![s0];
        let uses = vec![uses_of(0, 2.0)];
        let (mut sp, h) = lone_session(&mut ids, &mut r);
        bind(sp.session_mut(h), &uses, &mut pools, &mut sels, &mut r);
        bind(sp.session_mut(h), &uses, &mut pools, &mut sels, &mut r);
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
