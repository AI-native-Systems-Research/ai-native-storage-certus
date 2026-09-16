//! Shared-object populations: the only place population dynamics live.
//!
//! A [`SharedPool`] holds the live instances of one shared class and moves them
//! through their life cycle in virtual time. It takes both population forms
//! FR-013 defines — a free-running [`Population::Poisson`] process and an
//! [`Population::Exact`] pool with immediate replacement — and seeds either from
//! the equilibrium residual-life distribution at `t = 0` (FR-015).
//!
//! # No feedback controller, and why (FR-016)
//!
//! Nothing here regulates the population toward its target. That is deliberate
//! and it is measured, not assumed: a regulator holds the live count's
//! `var/mean` at 0.48 where an uncontrolled birth-death population sits at 1.00,
//! so it suppresses exactly the fluctuation FR-013 asks for. Tight regulation
//! would be a fidelity loss. The evidence, including the retraction of an earlier
//! and wrong justification for this requirement, is in
//! `research/population/README.md`.
//!
//! # Two indices, not one
//!
//! Each instance carries two identifiers, and conflating them is a defect:
//!
//! - [`SharedInstance::slot`] is the **bounded selection index**. It is reused
//!   when an instance dies, so popularity attached to a position outlives its
//!   occupant, which is what `rank_by: slot` means. Numbering selection by a
//!   monotonic counter would be wrong: heat would attach permanently to the
//!   earliest instances and the pool would mint objects nothing ever selects.
//! - [`SharedInstance::mint`] is the **key identity**, a per-class monotonic
//!   counter, and it is what goes into the salt's `instance_index` field.
//!
//! The distinction is load-bearing. If the salt used the slot, a replacement
//! would inherit the dead object's keys exactly — `lifetime` would produce no key
//! churn at all, and the churn rate that
//! `research/population/seeding.py` measures, the thing a cache actually sees,
//! would be identically zero. `data-model.md` describes one index doing both
//! jobs; it cannot.
//!
//! A consequence worth recording: the salt's 26-bit `instance_index` therefore
//! bounds **total mints per class per run**, not the live count. The live count
//! is checked at load (T017); the mint total depends on the run's span and so
//! belongs to the pre-flight projection.
//!
//! # Retirement is not deletion (FR-018)
//!
//! An instance whose lifetime expires stops being *selectable* immediately: its
//! slot is freed for the next occupant and it leaves both index spaces. It does
//! not stop *existing*. A session that already holds it keeps reading its blocks
//! to the end of its own life, because a real workload does not abandon a
//! document mid-conversation because the document aged out of the popular set.
//! So a hold is refcounted — [`SharedPool::acquire`] returns a [`Held`] token,
//! [`SharedPool::release`] gives it back, and only the last release of a retired
//! instance frees its bookkeeping.
//!
//! Nothing about any of this reaches Certus. There is no eviction message and no
//! delete: a retired object's keys simply stop being requested, and the cache
//! decides on its own when to drop them. That is the whole point of the
//! mechanism — it is what gives the cache a working set that moves.
//!
//! # Examples
//!
//! ```
//! use workload_model::description::Population;
//! use workload_model::distribution::{Distribution, Kind};
//! use workload_model::pool::SharedPool;
//! use workload_model::rng;
//!
//! let lifetime = Distribution::new(Kind::Exponential { mean: 100.0 }).resolve(None).unwrap();
//! let length = Distribution::new(Kind::Constant { value: 4.0 }).resolve_integral(None).unwrap();
//! let mut rng = rng::substream(1, "pools");
//! let mut pool = SharedPool::new(0, Population::Exact(10), lifetime, length, &mut rng);
//!
//! pool.seed(0.0, &mut rng);
//! assert_eq!(pool.live(), 10);
//!
//! // An exact pool holds its size for ever, replacing each death at once.
//! pool.advance_to(1_000.0, &mut rng);
//! assert_eq!(pool.live(), 10);
//! // ...but the keys have churned: replacements got fresh mints.
//! assert!(pool.total_mints() > 10);
//! ```

use rand::Rng;

use crate::description::Population;
use crate::distribution::Resolved;

/// Number of lifetime draws backing the empirical residual-life distribution.
///
/// The residual distribution's accuracy goes as `1/sqrt(n)`, so 65 536 samples
/// place it within about 0.4% — far inside the tolerance of anything that reads
/// it. Drawn once per pool at construction.
const RESIDUAL_GRID: usize = 1 << 16;

/// Largest mean a single Knuth-method Poisson chunk handles before the draw is
/// split. `exp(-lambda)` underflows above about 745, so the split is a
/// correctness requirement rather than a speed one.
const POISSON_CHUNK: f64 = 400.0;

/// One shared object.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SharedInstance {
    mint: u64,
    slot: u32,
    length_blocks: u64,
    born_at: f64,
    dies_at: f64,
    users: u32,
    retired: bool,
}

impl SharedInstance {
    /// Key identity — the salt's `instance_index`. Monotonic within a class, so a
    /// replacement never inherits its predecessor's keys.
    pub fn mint(&self) -> u64 {
        self.mint
    }

    /// Bounded selection index, reused after death so that popularity attached to
    /// a position outlives its occupant.
    pub fn slot(&self) -> u32 {
        self.slot
    }

    /// Blocks in this instance, drawn once at creation.
    pub fn length_blocks(&self) -> u64 {
        self.length_blocks
    }

    /// Virtual time this instance was created.
    pub fn born_at(&self) -> f64 {
        self.born_at
    }

    /// Virtual time this instance stops being selectable.
    pub fn dies_at(&self) -> f64 {
        self.dies_at
    }

    /// Sessions currently using it.
    pub fn users(&self) -> u32 {
        self.users
    }

    /// Whether its lifetime has expired. A retired instance is unselectable but
    /// still usable by the sessions already holding it (FR-018).
    pub fn retired(&self) -> bool {
        self.retired
    }
}

/// How a pool gives its instances their initial remaining life at `t = 0`.
///
/// There is only one correct answer, so this exists for a narrow reason: it lets
/// the wrong answer be **measured** in this repository instead of asserted. See
/// [`SharedPool::seed_with_policy`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum SeedingPolicy {
    /// Draw each instance's remaining life from the equilibrium residual-life
    /// distribution. **Required** (FR-015), and what [`SharedPool::seed`] does.
    #[default]
    Equilibrium,
    /// Draw each instance a full life from the lifetime distribution.
    ///
    /// **This is the defect FR-015 forbids, and no run may use it.** Every
    /// instance gets the same birthday, so the pool turns over in synchronised
    /// cohorts: a normal-lifetime pool then sees *zero* key churn for several
    /// windows followed by a burst, with the periodicity still plainly visible
    /// eight generations later.
    ///
    /// It is reachable only through [`SharedPool::seed_with_policy`], never
    /// through [`SharedPool::seed`], so a production path cannot select it by
    /// accident. Its purpose is to be the second arm of a controlled comparison —
    /// a requirement whose justification can be re-run is worth more than one
    /// whose justification is a comment.
    Lifetime,
}

/// One session's hold on one shared instance.
///
/// Returned by [`SharedPool::acquire`] and consumed by [`SharedPool::release`].
/// It is deliberately neither `Clone` nor `Copy`, because the token *is* the
/// refcount: a copy would be an extra decrement and a leak of the original.
///
/// A hold cannot be a slot number. Slots are reused the moment their occupant
/// retires, so by the time a holder came back to a slot it could be looking at
/// the replacement — which is exactly the confusion the `slot`/`mint` split
/// exists to prevent. The `mint` recorded here is what pins the identity, and
/// [`SharedPool::held`] checks it.
#[derive(Debug, PartialEq, Eq)]
#[must_use = "a dropped hold leaks its instance's bookkeeping; pass it to release()"]
pub struct Held {
    class_id: u64,
    handle: usize,
    mint: u64,
}

impl Held {
    /// Key identity of the held instance — the salt's `instance_index`.
    pub fn mint(&self) -> u64 {
        self.mint
    }

    /// Class this hold belongs to. [`SharedPool::release`] refuses a hold from
    /// another class, since the handle would name an unrelated instance.
    pub fn class_id(&self) -> u64 {
        self.class_id
    }
}

/// The equilibrium residual-life distribution of a lifetime distribution.
///
/// At `t = 0` a pool's instances are already part-way through their lives. Their
/// remaining life is *not* distributed like the lifetime itself but like
///
/// ```text
/// P(R <= x) = (1 / E[L]) * integral from 0 to x of (1 - F(u)) du
/// ```
///
/// Seeding from `F` instead gives every instance the same birthday, and the pool
/// then turns over in synchronised cohorts: measured, the first three windows of
/// a normal-lifetime pool see *exactly zero* key churn, and a 44%
/// peak-to-trough modulation is still present eight lifetimes later
/// (`research/population/seeding.py`).
///
/// This is sampled by a **length-biased** pick of a lifetime followed by a
/// uniform point within it, which is the equilibrium construction and needs no
/// closed form — necessary here, because a lifetime may be written as any of
/// five distribution kinds including an empirical sample set.
///
/// # Examples
///
/// ```
/// use workload_model::distribution::{Distribution, Kind};
/// use workload_model::pool::ResidualLife;
/// use workload_model::rng;
///
/// // A constant lifetime L has residual life uniform on [0, L].
/// let life = Distribution::new(Kind::Constant { value: 10.0 }).resolve(None).unwrap();
/// let mut rng = rng::substream(1, "residual");
/// let residual = ResidualLife::build(&life, &mut rng);
/// for _ in 0..100 {
///     let r = residual.sample(&mut rng);
///     assert!((0.0..=10.0).contains(&r));
/// }
/// ```
#[derive(Debug, Clone)]
pub struct ResidualLife {
    grid: Vec<f64>,
    cumulative: Vec<f64>,
    total: f64,
    mean_lifetime: f64,
    immortal: bool,
}

impl ResidualLife {
    /// Build the residual-life distribution of `lifetime`.
    ///
    /// Consumes a fixed number of draws from `rng` (see `RESIDUAL_GRID`), so
    /// callers should pass a dedicated substream — otherwise adding a pool would
    /// shift every later draw in the run and invalidate recorded seeds.
    pub fn build<R: Rng + ?Sized>(lifetime: &Resolved, rng: &mut R) -> Self {
        let mean_lifetime = lifetime.effective().mean;
        if !mean_lifetime.is_finite() {
            // FR-017: an unbounded lifetime means the pool is minted once and
            // never turns over, so every residual life is infinite too. Building
            // a grid of infinities would give a meaningless length bias.
            return Self {
                grid: Vec::new(),
                cumulative: Vec::new(),
                total: 0.0,
                mean_lifetime,
                immortal: true,
            };
        }
        let mut grid = Vec::with_capacity(RESIDUAL_GRID);
        let mut cumulative = Vec::with_capacity(RESIDUAL_GRID);
        let mut running = 0.0f64;
        for _ in 0..RESIDUAL_GRID {
            let l = lifetime.sample(rng).max(0.0);
            running += l;
            grid.push(l);
            cumulative.push(running);
        }
        Self {
            grid,
            cumulative,
            total: running,
            mean_lifetime,
            immortal: false,
        }
    }

    /// Draw one residual life.
    pub fn sample<R: Rng + ?Sized>(&self, rng: &mut R) -> f64 {
        if self.immortal {
            return f64::INFINITY;
        }
        if self.total <= 0.0 {
            return 0.0;
        }
        let u: f64 = rng.gen::<f64>() * self.total;
        // Length-biased pick: the chance of landing in a lifetime is proportional
        // to its length, which is what makes this the equilibrium distribution
        // rather than the lifetime distribution.
        let i = self
            .cumulative
            .partition_point(|c| *c <= u)
            .min(self.grid.len() - 1);
        self.grid[i] * rng.gen::<f64>()
    }

    /// Effective mean of the underlying lifetime, after truncation.
    pub fn mean_lifetime(&self) -> f64 {
        self.mean_lifetime
    }

    /// Whether the lifetime is unbounded, making the pool mint-once (FR-017).
    pub fn immortal(&self) -> bool {
        self.immortal
    }
}

/// Draw from a Poisson distribution with the given mean.
///
/// Knuth's method, split into chunks because `exp(-lambda)` underflows above
/// about 745 and a Poisson is the sum of independent Poissons whose means sum to
/// the same total. Exact rather than a normal approximation, since it is drawn
/// once per pool at seeding and the cost is irrelevant.
pub(crate) fn poisson<R: Rng + ?Sized>(rng: &mut R, mean: f64) -> u64 {
    if !mean.is_finite() || mean <= 0.0 {
        return 0;
    }
    let mut remaining = mean;
    let mut total = 0u64;
    while remaining > 0.0 {
        let chunk = remaining.min(POISSON_CHUNK);
        remaining -= chunk;
        let limit = (-chunk).exp();
        let mut product: f64 = rng.gen();
        let mut k = 0u64;
        while product > limit {
            product *= rng.gen::<f64>();
            k += 1;
        }
        total += k;
    }
    total
}

/// The live instances of one shared class, and their population dynamics.
#[derive(Debug, Clone)]
pub struct SharedPool {
    class_id: u64,
    form: Population,
    lifetime: Resolved,
    length: Resolved,
    residual: ResidualLife,
    /// Arena of instances that are either selectable or retired-but-still-used.
    instances: Vec<Option<SharedInstance>>,
    free_handles: Vec<usize>,
    /// Retired instances still held by at least one session (FR-018). Counted
    /// rather than derived, because deriving it is a scan of the whole arena.
    retained: usize,
    /// Selection index space: slot -> arena handle. Reused on death.
    slots: Vec<Option<usize>>,
    free_slots: Vec<u32>,
    /// Newest first. Used by `rank_by: recency`.
    recency: std::collections::VecDeque<usize>,
    /// Pending deaths, earliest first, as `(dies_at bits, slot, mint)`.
    ///
    /// A heap rather than a scan of the slots: processing one event would
    /// otherwise cost O(pool size), and a pool may hold 10 000 instances against
    /// a per-key budget far below Certus's own 5.6 µs.
    ///
    /// Every entry is popped exactly when its death is processed, so the heap
    /// holds no stale entries and needs no lazy deletion. `debug_assert_matches`
    /// in `advance_to` pins that: if a later change ever *does* leave a stale
    /// entry, it fails there rather than silently reordering the simulation,
    /// which is what a scan of `BinaryHeap::iter` would do — that iterator is in
    /// arbitrary order, not sorted order.
    ///
    /// Ordering `dies_at` by its bit pattern is exact for non-negative `f64`,
    /// infinities included, and a death time is a sum of non-negative quantities.
    deaths: std::collections::BinaryHeap<std::cmp::Reverse<(u64, u32, u64)>>,
    next_mint: u64,
    total_mints: u64,
    next_birth_at: f64,
    birth_rate: f64,
    now: f64,
    seeded: bool,
}

impl SharedPool {
    /// Build a pool for one shared class.
    ///
    /// `rng` is used only to construct the residual-life distribution; see
    /// [`ResidualLife::build`] for why it should be a dedicated substream.
    pub fn new<R: Rng + ?Sized>(
        class_id: u64,
        form: Population,
        lifetime: Resolved,
        length: Resolved,
        rng: &mut R,
    ) -> Self {
        let residual = ResidualLife::build(&lifetime, rng);
        // FR-013: births are a free-running process at rate N / E[lifetime].
        // There is no error term and no gain; the target enters only here.
        let birth_rate = match (form, residual.immortal()) {
            (Population::Poisson(n), false) => n as f64 / residual.mean_lifetime(),
            _ => 0.0,
        };
        Self {
            class_id,
            form,
            lifetime,
            length,
            residual,
            instances: Vec::new(),
            free_handles: Vec::new(),
            retained: 0,
            slots: Vec::new(),
            free_slots: Vec::new(),
            recency: std::collections::VecDeque::new(),
            deaths: std::collections::BinaryHeap::new(),
            next_mint: 0,
            total_mints: 0,
            next_birth_at: f64::INFINITY,
            birth_rate,
            now: 0.0,
            seeded: false,
        }
    }

    /// The class's declaration index.
    pub fn class_id(&self) -> u64 {
        self.class_id
    }

    /// Seed the pool at `t0` from the equilibrium residual-life distribution.
    ///
    /// The initial live count is the target for [`Population::Exact`], and a
    /// Poisson draw around it for [`Population::Poisson`] — that *is* the
    /// equilibrium distribution of an M/G/inf population, so seeding exactly the
    /// target would start with a variance the process does not have and relax
    /// toward the right one.
    ///
    /// # Panics
    ///
    /// If called twice.
    pub fn seed<R: Rng + ?Sized>(&mut self, t0: f64, rng: &mut R) {
        self.seed_with_policy(t0, SeedingPolicy::Equilibrium, rng)
    }

    /// Seed at `t0` under an explicit [`SeedingPolicy`].
    ///
    /// Callers in a run use [`SharedPool::seed`], which is this with
    /// [`SeedingPolicy::Equilibrium`]. The other policy is the defect FR-015
    /// forbids, and exists only so a test can measure it rather than take the
    /// requirement on trust — see `research/population/seeding.py` for the same
    /// comparison in Python, and `tests/pool.rs` for the assertions.
    ///
    /// # Panics
    ///
    /// If called twice.
    pub fn seed_with_policy<R: Rng + ?Sized>(
        &mut self,
        t0: f64,
        policy: SeedingPolicy,
        rng: &mut R,
    ) {
        assert!(!self.seeded, "pool seeded twice");
        self.seeded = true;
        self.now = t0;
        let nominal = self.form.nominal();
        let initial = match self.form {
            Population::Exact(n) => n,
            Population::Poisson(n) => poisson(rng, n as f64),
        };
        for _ in 0..initial {
            let remaining = match policy {
                SeedingPolicy::Equilibrium => self.residual.sample(rng),
                // Deliberately wrong: a full life, so every instance shares a
                // birthday and the pool turns over in cohorts.
                SeedingPolicy::Lifetime => self.lifetime.sample(rng).max(0.0),
            };
            self.mint(t0, t0 + remaining, rng);
        }
        if self.birth_rate > 0.0 {
            self.next_birth_at = t0 + exponential(rng, 1.0 / self.birth_rate);
        }
        debug_assert!(nominal > 0 || initial == 0);
    }

    /// The next virtual time at which this pool changes, if any.
    pub fn next_event_at(&self) -> Option<f64> {
        let death = self
            .peek_death()
            .map(|(_, _, at)| at)
            .unwrap_or(f64::INFINITY);
        let next = death.min(self.next_birth_at);
        if next.is_finite() {
            Some(next)
        } else {
            None
        }
    }

    /// Advance to `t`, processing deaths and births in virtual-time order.
    pub fn advance_to<R: Rng + ?Sized>(&mut self, t: f64, rng: &mut R) {
        assert!(self.seeded, "pool advanced before being seeded");
        loop {
            let death = self.peek_death();
            let next = death.map(|(_, _, at)| at).unwrap_or(f64::INFINITY);
            let birth = self.next_birth_at;
            let step = next.min(birth);
            if step > t || !step.is_finite() {
                break;
            }
            if next <= birth {
                let (slot, mint, at) = death.expect("a finite death time has a slot");
                debug_assert!(
                    self.entry_is_live(slot, mint),
                    "stale death entry for slot {slot} mint {mint}; the heap is \
                     assumed to hold none, see the note on `deaths`"
                );
                self.now = at;
                self.deaths.pop();
                let replace = matches!(self.form, Population::Exact(_));
                self.kill(slot, !replace);
                if replace {
                    // Immediate replacement, in the SAME slot, so popularity
                    // attached to that position carries to the new occupant —
                    // with a fresh mint, so the keys churn.
                    let life = self.lifetime.sample(rng);
                    self.mint_in_slot(slot, at, at + life, rng);
                }
            } else {
                self.now = birth;
                let life = self.lifetime.sample(rng);
                self.mint(birth, birth + life, rng);
                self.next_birth_at = birth + exponential(rng, 1.0 / self.birth_rate);
            }
        }
        self.now = self.now.max(t);
    }

    /// Instances currently selectable, in slot order.
    pub fn selectable(&self) -> impl Iterator<Item = &SharedInstance> {
        self.slots
            .iter()
            .flatten()
            .filter_map(|h| self.instances[*h].as_ref())
    }

    /// Number of selectable instances.
    pub fn live(&self) -> usize {
        self.slots.iter().flatten().count()
    }

    /// Instance occupying a selection slot, if any.
    pub fn by_slot(&self, slot: u32) -> Option<&SharedInstance> {
        self.slots
            .get(slot as usize)
            .and_then(|h| h.as_ref())
            .and_then(|h| self.instances[*h].as_ref())
    }

    /// Instances newest-first, which is the index space `rank_by: recency` uses.
    pub fn by_recency(&self) -> impl Iterator<Item = &SharedInstance> {
        self.recency
            .iter()
            .filter_map(|h| self.instances[*h].as_ref())
            .filter(|i| !i.retired)
    }

    /// Highest slot ever occupied, plus one — the realised width of the selection
    /// index space.
    pub fn slot_capacity(&self) -> usize {
        self.slots.len()
    }

    /// Total instances ever created. Bounded by the salt's 26-bit
    /// `instance_index` field, which the pre-flight projection must check against
    /// the run's span.
    pub fn total_mints(&self) -> u64 {
        self.total_mints
    }

    /// Effective mean lifetime after truncation.
    pub fn mean_lifetime(&self) -> f64 {
        self.residual.mean_lifetime()
    }

    /// Births per virtual second, or zero for an exact or mint-once pool.
    pub fn birth_rate(&self) -> f64 {
        self.birth_rate
    }

    /// The nominal population, whichever form this pool takes.
    ///
    /// For [`Population::Exact`] this is also the live count at every instant;
    /// for [`Population::Poisson`] the live count fluctuates around it with
    /// variance equal to it, so it is a target rather than a guarantee. Selection
    /// bounds a draw by `min(nominal, live)` (FR-022), and the two bounds mean
    /// different things — see `selection.rs`.
    pub fn nominal(&self) -> u64 {
        self.form.nominal()
    }

    /// Take a hold on the instance occupying `slot`, keeping its bookkeeping
    /// alive across retirement for as long as the holder needs it (FR-018).
    ///
    /// `None` means the slot is empty, which is the only way this fails: a slot's
    /// occupant is always selectable, because retirement frees the slot.
    ///
    /// # Examples
    ///
    /// ```
    /// use workload_model::description::Population;
    /// use workload_model::distribution::{Distribution, Kind};
    /// use workload_model::pool::SharedPool;
    /// use workload_model::rng;
    ///
    /// let life = Distribution::new(Kind::Constant { value: 10.0 }).resolve(None).unwrap();
    /// let len = Distribution::new(Kind::Constant { value: 4.0 }).resolve_integral(None).unwrap();
    /// let mut rng = rng::substream(1, "pools");
    /// let mut pool = SharedPool::new(0, Population::Exact(1), life, len, &mut rng);
    /// pool.seed(0.0, &mut rng);
    ///
    /// let held = pool.acquire(0).unwrap();
    /// let blocks = pool.held(&held).length_blocks();
    ///
    /// // Long enough that the instance has retired and been replaced...
    /// pool.advance_to(100.0, &mut rng);
    /// assert!(pool.held(&held).retired());
    /// assert_ne!(pool.by_slot(0).unwrap().mint(), held.mint());
    /// // ...but the holder still sees the same object it started with.
    /// assert_eq!(pool.held(&held).length_blocks(), blocks);
    /// assert_eq!(pool.retained(), 1);
    ///
    /// pool.release(held);
    /// assert_eq!(pool.retained(), 0);
    /// ```
    #[must_use = "a dropped hold leaks its instance's bookkeeping; pass it to release()"]
    pub fn acquire(&mut self, slot: u32) -> Option<Held> {
        let handle = *self.slots.get(slot as usize)?.as_ref()?;
        let class_id = self.class_id;
        let inst = self.instances[handle].as_mut()?;
        debug_assert!(
            !inst.retired,
            "a retired instance still occupies slot {slot}; retirement must free it"
        );
        inst.users += 1;
        Some(Held {
            class_id,
            handle,
            mint: inst.mint,
        })
    }

    /// The instance a hold refers to, retired or not.
    ///
    /// # Panics
    ///
    /// If the hold has been released, or belongs to another pool — both are
    /// programming errors rather than states the simulation can reach.
    pub fn held(&self, held: &Held) -> &SharedInstance {
        assert_eq!(
            held.class_id, self.class_id,
            "hold from class {} read in class {}",
            held.class_id, self.class_id
        );
        let inst = self.instances[held.handle]
            .as_ref()
            .expect("a held instance is never freed while the hold exists");
        assert_eq!(
            inst.mint, held.mint,
            "hold on mint {} now names mint {}: the arena entry was reused early",
            held.mint, inst.mint
        );
        inst
    }

    /// Give up a hold.
    ///
    /// The last release of a *retired* instance frees its bookkeeping and returns
    /// its arena entry for reuse, which is the `Released` transition in
    /// `data-model.md`. Releasing a still-live instance only drops the refcount.
    /// Certus is told nothing either way (FR-018).
    ///
    /// # Panics
    ///
    /// If the hold belongs to another pool, or names an instance that has already
    /// been freed.
    pub fn release(&mut self, held: Held) {
        assert_eq!(
            held.class_id, self.class_id,
            "hold from class {} released into class {}",
            held.class_id, self.class_id
        );
        let handle = held.handle;
        let inst = self.instances[handle]
            .as_mut()
            .expect("a held instance is never freed while the hold exists");
        assert_eq!(
            inst.mint, held.mint,
            "hold on mint {} now names mint {}: the arena entry was reused early",
            held.mint, inst.mint
        );
        debug_assert!(inst.users > 0, "release without a matching acquire");
        inst.users -= 1;
        if inst.users == 0 && inst.retired {
            self.instances[handle] = None;
            self.free_handles.push(handle);
            // Before the handle can be reused, or the reused entry would appear
            // twice in the recency index and be selectable at two ranks at once.
            self.recency.retain(|h| *h != handle);
            self.retained -= 1;
        }
    }

    /// Retired instances still held by at least one session — the population that
    /// exists but cannot be selected.
    pub fn retained(&self) -> usize {
        self.retained
    }

    /// Arena entries reserved for instances.
    ///
    /// A freed entry is reused rather than removed, so this is the high-water mark
    /// of `live() + retained()`. It is exposed because "bookkeeping is freed" is a
    /// claim about this number staying bounded, and a test should be able to check
    /// it rather than take it on trust.
    pub fn arena_len(&self) -> usize {
        self.instances.len()
    }

    /// The earliest pending death, left on the heap.
    fn peek_death(&self) -> Option<(u32, u64, f64)> {
        self.deaths
            .peek()
            .map(|std::cmp::Reverse((bits, slot, mint))| (*slot, *mint, f64::from_bits(*bits)))
    }

    /// Whether a heap entry still describes the current occupant of its slot.
    ///
    /// Should always hold; see the note on [`SharedPool::deaths`].
    fn entry_is_live(&self, slot: u32, mint: u64) -> bool {
        self.slots
            .get(slot as usize)
            .and_then(|h| h.as_ref())
            .and_then(|h| self.instances[*h].as_ref())
            .is_some_and(|i| i.mint == mint && !i.retired)
    }

    /// Retire the occupant of `slot`, optionally returning the slot for reuse.
    ///
    /// The instance itself survives while sessions still hold it (FR-018); only
    /// its selectability ends. Certus is told nothing.
    ///
    /// `free_slot` is false when the caller is about to refill the same slot, as
    /// an exact pool does. Freeing it unconditionally is wrong there: the slot
    /// would sit in `free_slots` while occupied, so `free_slots` would grow by one
    /// per death for the life of the run, and a later birth would hand out a slot
    /// that already had an occupant.
    fn kill(&mut self, slot: u32, free_slot: bool) {
        let Some(handle) = self.slots[slot as usize].take() else {
            return;
        };
        if let Some(inst) = self.instances[handle].as_mut() {
            inst.retired = true;
            if inst.users == 0 {
                // The common case: nobody is reading it, so retirement and release
                // happen at the same instant.
                self.instances[handle] = None;
                self.free_handles.push(handle);
                self.recency.retain(|h| *h != handle);
            } else {
                self.retained += 1;
            }
        }
        if free_slot {
            self.free_slots.push(slot);
        }
    }

    fn mint<R: Rng + ?Sized>(&mut self, born_at: f64, dies_at: f64, rng: &mut R) -> u32 {
        // Lowest free slot, so the index space stays compact instead of growing
        // with the number of births.
        let slot = match self.free_slots.pop() {
            Some(s) => {
                debug_assert!(
                    self.slots[s as usize].is_none(),
                    "slot {s} was free-listed while still occupied"
                );
                s
            }
            None => {
                self.slots.push(None);
                (self.slots.len() - 1) as u32
            }
        };
        self.mint_in_slot(slot, born_at, dies_at, rng);
        slot
    }

    fn mint_in_slot<R: Rng + ?Sized>(
        &mut self,
        slot: u32,
        born_at: f64,
        dies_at: f64,
        rng: &mut R,
    ) {
        let length_blocks = self.length.sample_int(rng).max(1) as u64;
        let inst = SharedInstance {
            mint: self.next_mint,
            slot,
            length_blocks,
            born_at,
            dies_at,
            users: 0,
            retired: false,
        };
        self.next_mint += 1;
        self.total_mints += 1;
        let handle = match self.free_handles.pop() {
            Some(h) => {
                self.instances[h] = Some(inst);
                h
            }
            None => {
                self.instances.push(Some(inst));
                self.instances.len() - 1
            }
        };
        if (slot as usize) >= self.slots.len() {
            self.slots.resize(slot as usize + 1, None);
        }
        self.slots[slot as usize] = Some(handle);
        self.recency.push_front(handle);
        debug_assert!(
            dies_at >= 0.0,
            "death time must be non-negative to order by bits"
        );
        self.deaths.push(std::cmp::Reverse((
            dies_at.to_bits(),
            slot,
            self.next_mint - 1,
        )));
    }
}

/// Exponential draw with the given mean, by inverse transform.
pub(crate) fn exponential<R: Rng + ?Sized>(rng: &mut R, mean: f64) -> f64 {
    -mean * (-rng.gen::<f64>()).ln_1p()
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

    fn pool(form: Population, lifetime: Kind) -> (SharedPool, rng::Rng) {
        let mut r = rng::substream(7, "pools");
        let p = SharedPool::new(
            0,
            form,
            resolved(lifetime, false),
            resolved(Kind::Constant { value: 4.0 }, true),
            &mut r,
        );
        (p, r)
    }

    #[test]
    fn residual_of_a_constant_lifetime_is_uniform() {
        // The one case with a closed form to check against: for L constant, the
        // equilibrium residual life is Uniform(0, L), so its mean is L/2.
        let life = resolved(Kind::Constant { value: 10.0 }, false);
        let mut r = rng::substream(1, "residual");
        let residual = ResidualLife::build(&life, &mut r);
        let n = 50_000;
        let mut sum = 0.0;
        for _ in 0..n {
            let x = residual.sample(&mut r);
            assert!((0.0..=10.0).contains(&x));
            sum += x;
        }
        let mean = sum / n as f64;
        assert!((mean - 5.0).abs() < 0.05, "residual mean {mean}, want 5.0");
    }

    #[test]
    fn residual_of_an_exponential_lifetime_is_the_same_exponential() {
        // Memorylessness: the equilibrium residual life of Exp(m) is Exp(m). This
        // is the control that catches a wrong length bias — an unbiased pick
        // would give mean m/2 instead of m.
        let life = resolved(Kind::Exponential { mean: 100.0 }, false);
        let mut r = rng::substream(2, "residual");
        let residual = ResidualLife::build(&life, &mut r);
        let n = 50_000;
        let mean: f64 = (0..n).map(|_| residual.sample(&mut r)).sum::<f64>() / n as f64;
        assert!(
            (mean - 100.0).abs() < 2.0,
            "residual mean {mean}, want 100 (50 would mean the length bias is missing)"
        );
    }

    #[test]
    fn an_unbounded_lifetime_is_mint_once() {
        // FR-017. Nothing ever dies, so nothing is ever born after seeding.
        let (mut p, mut r) = pool(
            Population::Exact(3),
            Kind::Constant {
                value: f64::INFINITY,
            },
        );
        p.seed(0.0, &mut r);
        assert!(p.mean_lifetime().is_infinite());
        assert_eq!(p.birth_rate(), 0.0);
        assert_eq!(p.next_event_at(), None);
        p.advance_to(1e12, &mut r);
        assert_eq!(p.live(), 3);
        assert_eq!(p.total_mints(), 3);
    }

    #[test]
    fn exact_pool_holds_its_size_and_reuses_slots() {
        let (mut p, mut r) = pool(Population::Exact(10), Kind::Exponential { mean: 50.0 });
        p.seed(0.0, &mut r);
        assert_eq!(p.live(), 10);
        for step in 1..=20 {
            p.advance_to(step as f64 * 100.0, &mut r);
            assert_eq!(p.live(), 10, "exact pool changed size");
        }
        // Replacements happened, and the selection index space stayed at 10 —
        // a monotonic index would have grown with every birth.
        assert!(p.total_mints() > 10, "no turnover at all");
        assert_eq!(p.slot_capacity(), 10, "slot space grew past the pool size");
    }

    #[test]
    fn a_replacement_gets_a_fresh_mint_so_keys_churn() {
        // The reason mint and slot must be different things: if the salt used the
        // slot, a replacement would inherit its predecessor's keys and `lifetime`
        // would produce no key churn at all.
        let (mut p, mut r) = pool(Population::Exact(1), Kind::Constant { value: 10.0 });
        p.seed(0.0, &mut r);
        let first = p.by_slot(0).unwrap();
        let (first_mint, first_slot) = (first.mint(), first.slot());
        p.advance_to(100.0, &mut r);
        let later = p.by_slot(0).unwrap();
        assert_eq!(later.slot(), first_slot, "slot should be reused");
        assert_ne!(later.mint(), first_mint, "mint should be fresh");
    }

    #[test]
    fn poisson_pool_fluctuates_around_its_target() {
        // FR-013: the live count is Poisson(N), so sd = sqrt(N). Checked loosely,
        // because the point is that it fluctuates at all — an implementation that
        // regulated would sit exactly on target and fail the variance check below.
        let n = 200u64;
        let (mut p, mut r) = pool(Population::Poisson(n), Kind::Exponential { mean: 100.0 });
        p.seed(0.0, &mut r);
        let mut counts = Vec::new();
        for step in 1..=400 {
            p.advance_to(step as f64 * 25.0, &mut r);
            counts.push(p.live() as f64);
        }
        let mean = counts.iter().sum::<f64>() / counts.len() as f64;
        let var = counts.iter().map(|c| (c - mean).powi(2)).sum::<f64>() / counts.len() as f64;
        assert!(
            (mean - n as f64).abs() < 0.15 * n as f64,
            "mean {mean}, target {n}"
        );
        // var/mean should be near 1 for M/G/inf. A regulator would drive it to
        // ~0.5, which is exactly what FR-016 rejects.
        assert!(
            var / mean > 0.4,
            "var/mean = {:.3}; the population is not fluctuating like a real one",
            var / mean
        );
        assert!(p.birth_rate() > 0.0);
    }

    #[test]
    fn poisson_seeding_is_not_exactly_the_target() {
        // The equilibrium count of an M/G/inf population is Poisson(N), so
        // seeding exactly N would start with a variance the process does not
        // have. Over many pools the seeded counts must vary.
        let mut seen = std::collections::BTreeSet::new();
        for s in 0..40u64 {
            let mut r = rng::substream(s, "pools");
            let mut p = SharedPool::new(
                0,
                Population::Poisson(100),
                resolved(Kind::Exponential { mean: 100.0 }, false),
                resolved(Kind::Constant { value: 1.0 }, true),
                &mut r,
            );
            p.seed(0.0, &mut r);
            seen.insert(p.live());
        }
        assert!(
            seen.len() > 5,
            "seeded live counts barely varied: {seen:?} — seeding is not Poisson"
        );
    }

    #[test]
    fn poisson_draw_has_the_right_mean_and_variance() {
        // Poisson has var = mean, which is the property the pool relies on.
        let mut r = rng::substream(3, "poisson");
        for lambda in [0.5f64, 5.0, 100.0, 1000.0] {
            let n = 8_000;
            let xs: Vec<f64> = (0..n).map(|_| poisson(&mut r, lambda) as f64).collect();
            let mean = xs.iter().sum::<f64>() / n as f64;
            let var = xs.iter().map(|x| (x - mean).powi(2)).sum::<f64>() / n as f64;
            assert!(
                (mean - lambda).abs() < 0.05 * lambda.max(1.0),
                "lambda {lambda}: mean {mean}"
            );
            assert!(
                (var / mean - 1.0).abs() < 0.1,
                "lambda {lambda}: var/mean {:.3}",
                var / mean
            );
        }
    }

    #[test]
    fn next_event_is_the_earliest_death_even_with_stale_heap_entries() {
        // The invariant the sim loop depends on: the next event this pool reports
        // is the earliest thing that will actually happen. Getting it wrong is a
        // silent reordering of the whole simulation rather than a crash.
        //
        // Honest note on this test's strength: it does not currently distinguish a
        // heap peek from a scan of `BinaryHeap::iter`, because the heap holds no
        // stale entries — every entry is popped exactly at its death — so the
        // first element of the backing vector is always the live root. It is kept
        // because the invariant is worth pinning, and `advance_to`'s
        // `debug_assert` is what guards the assumption that makes it easy.
        let (mut p, mut r) = pool(Population::Poisson(30), Kind::Exponential { mean: 40.0 });
        p.seed(0.0, &mut r);
        for step in 1..=30 {
            let t = step as f64 * 20.0;
            p.advance_to(t, &mut r);
            let want = p
                .selectable()
                .map(|i| i.dies_at())
                .fold(f64::INFINITY, f64::min)
                .min(p.next_birth_at);
            let got = p.next_event_at().unwrap();
            assert_eq!(
                got.to_bits(),
                want.to_bits(),
                "at t={t}: next_event_at gave {got}, earliest live event is {want}"
            );
            assert!(got >= t, "next event {got} is in the past at t={t}");
        }
    }

    #[test]
    fn a_held_instance_survives_retirement_but_is_unselectable() {
        // FR-018, both halves in one test: retirement ends selectability at once,
        // and does not end existence while a session is still reading it.
        let (mut p, mut r) = pool(Population::Exact(1), Kind::Constant { value: 10.0 });
        p.seed(0.0, &mut r);
        let held = p.acquire(0).unwrap();
        let (mint, blocks) = (held.mint(), p.held(&held).length_blocks());

        p.advance_to(100.0, &mut r);

        assert!(p.held(&held).retired(), "the hold's instance never retired");
        assert_eq!(p.held(&held).length_blocks(), blocks, "the object changed");
        assert_eq!(p.retained(), 1);
        // Unselectable: it is in neither index space, and its slot has a new
        // occupant with a different key identity.
        assert!(p.selectable().all(|i| i.mint() != mint));
        assert!(p.by_recency().all(|i| i.mint() != mint));
        assert_ne!(p.by_slot(0).unwrap().mint(), mint);
        assert_eq!(p.live(), 1, "the pool did not stay at its exact size");

        p.release(held);
        assert_eq!(p.retained(), 0);
    }

    #[test]
    fn the_last_release_frees_the_bookkeeping() {
        // The refcount is what decides, not the first release: two sessions on one
        // instance means one of them can outlive the other.
        let (mut p, mut r) = pool(Population::Exact(1), Kind::Constant { value: 10.0 });
        p.seed(0.0, &mut r);
        let a = p.acquire(0).unwrap();
        let b = p.acquire(0).unwrap();
        assert_eq!(a.mint(), b.mint());
        assert_eq!(p.held(&a).users(), 2);

        p.advance_to(100.0, &mut r);
        assert_eq!(p.retained(), 1);

        p.release(a);
        assert_eq!(
            p.retained(),
            1,
            "freed while a session was still reading it"
        );
        assert_eq!(p.held(&b).users(), 1);
        p.release(b);
        assert_eq!(p.retained(), 0);
    }

    #[test]
    fn releasing_a_live_instance_only_drops_the_refcount() {
        // A session can finish before the object it was reading ages out. That must
        // not retire the object.
        let (mut p, mut r) = pool(Population::Exact(1), Kind::Constant { value: 1e9 });
        p.seed(0.0, &mut r);
        let held = p.acquire(0).unwrap();
        let mint = held.mint();
        p.release(held);
        assert_eq!(p.retained(), 0);
        assert_eq!(p.live(), 1);
        assert_eq!(p.by_slot(0).unwrap().mint(), mint, "the object was retired");
        assert_eq!(p.by_slot(0).unwrap().users(), 0);
    }

    #[test]
    fn a_full_pool_stays_deliverable_when_every_object_is_a_zombie() {
        // The worry this answers: if a retired-but-still-held instance kept its
        // selection slot until its last reader left, then a session asking for the
        // whole class could not be served — one slot would be occupied by something
        // unselectable, and FR-022's draw would come up short through no fault of
        // the population process.
        //
        // It cannot happen, because retirement and release are separate events. The
        // slot is freed at the instant of death and an exact pool refills it in the
        // same instant; only the *bookkeeping* waits for the refcount. So the pool
        // is at full strength even with every one of its objects zombied.
        let n = 6;
        let (mut p, mut r) = pool(Population::Exact(n), Kind::Constant { value: 10.0 });
        p.seed(0.0, &mut r);

        // One session holding the entire class, as FR-022's largest draw would.
        let holds: Vec<Held> = (0..n as u32).map(|s| p.acquire(s).unwrap()).collect();
        let held_mints: std::collections::BTreeSet<u64> = holds.iter().map(|h| h.mint()).collect();
        assert_eq!(held_mints.len(), n as usize);

        // Past every one of their lifetimes, so all six are zombies at once.
        p.advance_to(50.0, &mut r);
        assert_eq!(p.retained(), n as usize, "not every object zombied");
        assert!(holds.iter().all(|h| p.held(h).retired()));

        // The class can still deliver a full draw, from live replacements only.
        assert_eq!(p.live(), n as usize, "a zombie cost the pool a slot");
        assert_eq!(p.selectable().count(), n as usize);
        assert_eq!(p.by_recency().count(), n as usize);
        let now: std::collections::BTreeSet<u64> = p.selectable().map(|i| i.mint()).collect();
        assert_eq!(now.len(), n as usize, "two slots share an instance");
        assert!(
            now.is_disjoint(&held_mints),
            "a zombie is still selectable: {now:?} meets {held_mints:?}"
        );
        // And the slots are still the same compact index space, so `rank_by: slot`
        // means the same thing before and after.
        assert_eq!(p.slot_capacity(), n as usize);

        for h in holds {
            p.release(h);
        }
        assert_eq!(p.retained(), 0);
        assert_eq!(p.live(), n as usize);
    }

    #[test]
    fn bookkeeping_stays_bounded_across_many_generations() {
        // "Released" has to mean something: an entry must come back for reuse. With
        // one hold outstanding at a time the arena needs the pool plus one, and if
        // release leaked, this would grow with the number of deaths instead.
        let (mut p, mut r) = pool(Population::Exact(4), Kind::Constant { value: 10.0 });
        p.seed(0.0, &mut r);
        for step in 1..=200 {
            let held = p.acquire(step as u32 % 4).unwrap();
            p.advance_to(step as f64 * 10.0, &mut r);
            p.release(held);
            assert_eq!(p.live(), 4);
            assert_eq!(p.retained(), 0);
        }
        assert!(p.total_mints() > 50, "no turnover, so nothing was tested");
        assert!(
            p.arena_len() <= 5,
            "arena grew to {} over {} mints: bookkeeping is not being freed",
            p.arena_len(),
            p.total_mints()
        );
        // And the recency index tracks it, rather than accumulating dead handles.
        assert_eq!(p.by_recency().count(), 4);
        assert_eq!(p.recency.len(), 4);
        // The exact pool refills each slot itself, so nothing should ever have been
        // put on the free list; an entry there while occupied would later hand out
        // a slot that already has an occupant.
        assert!(
            p.free_slots.is_empty(),
            "{} occupied slots were free-listed",
            p.free_slots.len()
        );
        assert_eq!(p.slot_capacity(), 4);
    }

    #[test]
    fn a_poisson_pool_frees_slots_and_reuses_them() {
        // The other side of the same fix: here deaths are *not* replaced, so the
        // slot genuinely does go back on the free list and must be reused rather
        // than the index space growing with every birth.
        let (mut p, mut r) = pool(Population::Poisson(20), Kind::Exponential { mean: 50.0 });
        p.seed(0.0, &mut r);
        for step in 1..=200 {
            p.advance_to(step as f64 * 10.0, &mut r);
            for (slot, handle) in p.slots.iter().enumerate() {
                if let Some(h) = handle {
                    assert_eq!(p.instances[*h].unwrap().slot(), slot as u32);
                }
            }
        }
        assert!(p.total_mints() > 100, "no turnover, so nothing was tested");
        assert!(
            p.slot_capacity() < 60,
            "slot space grew to {} over {} mints: slots are not being reused",
            p.slot_capacity(),
            p.total_mints()
        );
    }

    #[test]
    fn acquiring_an_empty_slot_gives_nothing() {
        let (mut p, mut r) = pool(Population::Poisson(5), Kind::Exponential { mean: 20.0 });
        p.seed(0.0, &mut r);
        let beyond = p.slot_capacity() as u32 + 10;
        assert!(p.acquire(beyond).is_none());
    }

    #[test]
    #[should_panic(expected = "released into class")]
    fn a_hold_cannot_cross_classes() {
        // Handles are per-pool, so a hold used in the wrong pool would decrement an
        // unrelated instance's refcount and free it under its own users.
        let (mut a, mut r) = pool(Population::Exact(1), Kind::Constant { value: 1e9 });
        let mut b = SharedPool::new(
            1,
            Population::Exact(1),
            resolved(Kind::Constant { value: 1e9 }, false),
            resolved(Kind::Constant { value: 4.0 }, true),
            &mut r,
        );
        a.seed(0.0, &mut r);
        b.seed(0.0, &mut r);
        let held = a.acquire(0).unwrap();
        b.release(held);
    }

    #[test]
    fn seeding_is_reproducible_from_a_seed() {
        let snapshot = |seed: u64| {
            let mut r = rng::substream(seed, "pools");
            let mut p = SharedPool::new(
                0,
                Population::Exact(8),
                resolved(
                    Kind::Normal {
                        mean: 100.0,
                        sigma: 10.0,
                    },
                    false,
                ),
                resolved(Kind::Constant { value: 3.0 }, true),
                &mut r,
            );
            p.seed(0.0, &mut r);
            p.advance_to(500.0, &mut r);
            p.selectable()
                .map(|i| (i.mint(), i.slot(), i.dies_at().to_bits()))
                .collect::<Vec<_>>()
        };
        assert_eq!(snapshot(5), snapshot(5));
        assert_ne!(snapshot(5), snapshot(6));
    }
}
