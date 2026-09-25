//! Which shared instances a session draws, and what the draw could not deliver.
//!
//! A session binds its shared instances once, when it is born, and those
//! instances' blocks then occupy fixed positions in its prefix chain for its
//! whole life. So this module answers one question — given a pool and a
//! requested count, which instances? — and records what it had to give up
//! answering it.
//!
//! # The index space, and why it must stay bounded
//!
//! Selection does not draw over instance identities. It draws over an **index
//! space**, a bounded range of ranks that maps onto whatever currently occupies
//! each rank:
//!
//! - [`RankBy::Slot`] — a rank *is* a pool slot. Slots are reused when their
//!   occupant dies, so a rank's popularity outlives any particular instance,
//!   which is what "popularity attaches to a position" means (FR-020). The space
//!   is as wide as the pool has ever needed, never wider.
//! - [`RankBy::Recency`] — rank 0 is the newest live instance. The space is
//!   exactly as wide as the live count, and an instance's rank grows as newer
//!   ones arrive, so heat decays with age.
//!
//! Neither space grows with the number of instances ever minted. That is the
//! requirement: numbering by a monotonic mint counter would make heat attach
//! permanently to the earliest instances, and the pool would go on minting
//! objects that nothing ever selects.
//!
//! # Uniform only, and the consequence of that
//!
//! This is the uniform case (FR-021). A non-uniform distribution over the index
//! space is US4 and is not implemented here.
//!
//! Uniform selection makes the working set equal the whole key space, so every
//! eviction policy scores alike — which is why the load-time report calls it out
//! rather than letting it pass unnoticed (`description.rs`). It also makes
//! `rank_by` **inert**: a uniform draw over live instances is the same
//! distribution whichever way the ranks are numbered. `rank_by` is still read and
//! still fixes the index space, because that is what US4 will draw over, but no
//! test here can distinguish the two settings by their outcome distribution and
//! none pretends to.
//!
//! # What a draw cannot deliver (FR-022, FR-023)
//!
//! A draw is bounded by `min(nominal pool size, live count)` and never waits and
//! never fails. Both bounds truncate the count distribution the author wrote, and
//! they mean quite different things, so [`SelectionStats`] counts them apart:
//!
//! - **Bound by nominal** — the description asks for more instances than the
//!   pool can ever hold. That is an authoring error, and the load-time gate
//!   should already have refused it or reported it (FR-004). Seeing it at run
//!   time means the gate was evaded.
//! - **Bound by live** — the population is simply fluctuating below its target,
//!   which is what a [`Population::Poisson`](crate::description::Population::Poisson) pool is *for*. Nothing is wrong; the
//!   requested distribution is nonetheless narrowed, and by how often is
//!   something a reader of the report needs to know.
//!
//! The bound bites only when the requested count is comparable to the pool size.
//! Five instances out of ten thousand will never notice a fluctuation of ±100;
//! eight out of ten will notice constantly.
//!
//! **A Poisson pool can be empty.** `min(nominal, live)` is then zero and the
//! session draws nothing at all from that class — it still runs, with no shared
//! prefix from it. The probability is `e^-N`: 13.5% at a nominal of 2, 0.67% at
//! 5, negligible above 10. It is counted separately as
//! [`SelectionStats::empty_pool`], because "the draw always succeeds" is
//! technically true and substantively empty in that case, and a description that
//! hits it often is not the workload its author wrote. Small populations are
//! exactly where `{exact: N}` is the right form.
//!
//! # Examples
//!
//! ```
//! use workload_model::description::{Population, RankBy};
//! use workload_model::distribution::{Distribution, Kind};
//! use workload_model::pool::SharedPool;
//! use workload_model::rng;
//! use workload_model::selection::Selector;
//!
//! let life = Distribution::new(Kind::Constant { value: f64::INFINITY })
//!     .resolve(None)
//!     .unwrap();
//! let len = Distribution::new(Kind::Constant { value: 4.0 })
//!     .resolve_integral(None)
//!     .unwrap();
//! let mut rng = rng::substream(1, "pools");
//! let mut pool = SharedPool::new(0, Population::Exact(10), life, len, &mut rng);
//! pool.seed(0.0, &mut rng);
//!
//! let mut sel = Selector::new(RankBy::Slot);
//! let drawn = sel.draw(&pool, 3, &mut rng);
//! assert_eq!(drawn.len(), 3);
//! // Distinct, and in canonical slot order so a session's prefix is stable.
//! assert!(drawn.windows(2).all(|w| w[0] < w[1]));
//!
//! // Asking for more than exist delivers what exists, and says so.
//! let drawn = sel.draw(&pool, 25, &mut rng);
//! assert_eq!(drawn.len(), 10);
//! assert_eq!(sel.stats().bound_by_nominal(), 1);
//! ```

use rand::Rng;

use crate::description::RankBy;
use crate::distribution::Resolved;
use crate::pool::SharedPool;

/// How often a draw could not deliver what was asked, and why (FR-023).
///
/// Accumulated across every draw a [`Selector`] makes, for the run report.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct SelectionStats {
    draws: u64,
    bound_by_nominal: u64,
    bound_by_live: u64,
    empty_pool: u64,
    requested: u64,
    delivered: u64,
    exhausted_retries: u64,
}

impl SelectionStats {
    /// Draws whose rejection budget ran out and were completed in rank order.
    ///
    /// Non-zero means a session asked for more instances than its `selection` spread
    /// realistically covers, so the draw was completed by taking the next-highest-ranked
    /// unchosen instances. Counted rather than left to be inferred from a hit-rate curve that
    /// looks slightly flatter than it should.
    pub fn exhausted_retries(&self) -> u64 {
        self.exhausted_retries
    }

    /// Draws made.
    pub fn draws(&self) -> u64 {
        self.draws
    }

    /// Draws truncated because the description asked for more instances than the
    /// pool can ever hold. **An authoring error** the load-time gate should have
    /// caught (FR-004); a non-zero count here means it was evaded.
    pub fn bound_by_nominal(&self) -> u64 {
        self.bound_by_nominal
    }

    /// Draws truncated because the live count was momentarily below the nominal
    /// size. Expected under a fluctuating population, and not an error — but it
    /// narrows the requested count distribution, which is why it is reported.
    pub fn bound_by_live(&self) -> u64 {
        self.bound_by_live
    }

    /// Draws that found the pool **empty** and delivered nothing. Counted apart
    /// because the session then has no shared prefix from that class at all; see
    /// the module docs on `e^-N`.
    pub fn empty_pool(&self) -> u64 {
        self.empty_pool
    }

    /// Instances asked for, summed over all draws.
    pub fn requested(&self) -> u64 {
        self.requested
    }

    /// Instances actually delivered, summed over all draws.
    pub fn delivered(&self) -> u64 {
        self.delivered
    }

    /// Fraction of draws either bound truncated, in `0.0..=1.0`. Zero draws gives
    /// zero rather than a NaN, so a report never prints one.
    pub fn binding_fraction(&self) -> f64 {
        if self.draws == 0 {
            return 0.0;
        }
        (self.bound_by_nominal + self.bound_by_live) as f64 / self.draws as f64
    }

    /// Fraction of requested instances that were not delivered, in `0.0..=1.0`.
    ///
    /// Worth reporting beside [`Self::binding_fraction`], because they answer
    /// different questions: a bound that binds on every draw but costs one
    /// instance each time is a very different distortion from one that binds
    /// rarely and costs most of the draw.
    pub fn shortfall_fraction(&self) -> f64 {
        if self.requested == 0 {
            return 0.0;
        }
        (self.requested - self.delivered) as f64 / self.requested as f64
    }
}

/// Draws instances from one shared class for successive sessions.
///
/// Holds a scratch buffer so that a draw allocates nothing, and accumulates the
/// [`SelectionStats`] the run report needs.
#[derive(Debug, Clone)]
pub struct Selector {
    rank_by: RankBy,
    /// Candidate slots, in rank order. Refilled per draw; never freed.
    candidates: Vec<u32>,
    /// How ranks are drawn, or `None` for uniform.
    ///
    /// This is what makes [`RankBy`] mean anything: a uniform draw is the same distribution
    /// whichever way the ranks are numbered, so without a selection distribution `rank_by` is
    /// inert by construction rather than by omission.
    selection: Option<Resolved>,
    /// Chosen ranks for one draw, so a draw allocates nothing.
    picked: Vec<usize>,
    stats: SelectionStats,
}

impl Selector {
    /// Build a selector for a class with the given index space.
    pub fn new(rank_by: RankBy) -> Self {
        Self {
            rank_by,
            candidates: Vec::new(),
            selection: None,
            picked: Vec::new(),
            stats: SelectionStats::default(),
        }
    }

    /// Draw ranks from `selection` instead of uniformly (FR-021).
    ///
    /// # What this buys, and why uniform selection is the wrong default to leave in place
    ///
    /// Under uniform selection every instance is equally likely, so the working set *is* the
    /// whole key space and a cache either holds all of it or thrashes. A hit-rate curve swept
    /// against capacity then has a step in it rather than a slope, and every eviction policy
    /// scores the same — the measurement cannot discriminate, which is what US4 exists to do.
    ///
    /// A concentrated distribution separates the two: the key space stays as large as the pool,
    /// while the *working set* is set by the spread. That is the knob that makes a capacity
    /// sweep informative.
    ///
    /// The distribution is over **rank**, not over slot or key, which is what gives
    /// [`RankBy`] its meaning — see the module docs.
    pub fn with_selection(mut self, selection: Option<Resolved>) -> Self {
        self.selection = selection;
        self
    }

    /// Whether draws are concentrated rather than uniform.
    pub fn is_concentrated(&self) -> bool {
        self.selection.is_some()
    }

    /// Ranks the spread effectively covers, or `None` under uniform selection.
    ///
    /// The spread's **p90 after truncation**, which is the rank band carrying most of the mass.
    /// It is deliberately *not* a count of ranks ever drawn: that would grow with the length of
    /// the run and so would describe the run rather than the description. p90 rather than the
    /// mean because a concentrated distribution's mean sits near rank 0 and says nothing about
    /// how far the references reach.
    ///
    /// `None` means the working set is the whole key space — the regime in which a capacity sweep
    /// cannot discriminate between eviction policies, and which a report has to be able to say
    /// rather than leave to be inferred from the shape of the curve.
    pub fn effective_ranks(&self) -> Option<f64> {
        self.selection.as_ref().map(|d| d.effective().p90.max(0.0))
    }

    /// The index space this selector draws over.
    pub fn rank_by(&self) -> RankBy {
        self.rank_by
    }

    /// What the draws have and have not delivered so far (FR-023).
    pub fn stats(&self) -> SelectionStats {
        self.stats
    }

    /// Draw up to `requested` distinct instances, returning their **slots** in
    /// ascending order.
    ///
    /// Uniform over the live instances, without replacement, bounded by
    /// `min(nominal, live)` (FR-022) — so it always returns, possibly empty, and
    /// never waits. Pass each returned slot to [`SharedPool::acquire`] to take a
    /// hold on it.
    ///
    /// The result is ordered by **slot**, not by rank, and that is deliberate for two reasons.
    /// The second is the load-bearing one and was not written down until someone asked.
    ///
    /// The first is stability: a session's `uses` order fixes where each instance's blocks land
    /// in its prefix chain, so the order has to hold for the session's whole life. A recency rank
    /// does not — it changes every time another instance is born — so ordering by rank would
    /// silently rearrange a live session's prefix.
    ///
    /// The second is that **sorting is what makes cross-session sharing possible at all**. Keys
    /// are a rolling prefix: block *n*'s key is derived from every block before it. Two sessions
    /// therefore share a prefix only if they lay the same instances down in the same *order*. Had
    /// the draw returned them in the order they were sampled, two sessions holding the same set
    /// of *k* instances would agree only when their orderings happened to coincide — probability
    /// `1/k!` — and the shared pools would produce almost no reuse while appearing to be shared.
    /// A canonical order takes that from `1/k!` to certain.
    ///
    /// It also makes *partial* sharing systematic rather than accidental: sessions holding
    /// `{3, 7}` and `{3, 9}` share slot 3's blocks and diverge after, because the common part of
    /// their sets is a common *prefix* once both are sorted.
    ///
    /// And slot order in particular interacts with `rank_by: slot`, where a concentrated
    /// selection favours low ranks: the hottest instances are then also the ones most likely to
    /// sit at the *front* of a chain. That puts the heaviest sharing at the prefix root, which is
    /// the shape a real workload has — a system prompt every session begins with. That was not
    /// designed; it falls out of the two choices and is worth knowing before either is changed.
    ///
    /// Cost is `O(live)` per draw, from refilling the candidate buffer, against
    /// `O(requested)` for the draw itself. Draws happen once per session per
    /// class rather than per turn, so this is not on the per-key path; if a
    /// 10 000-instance pool with a high session birth rate ever makes it matter,
    /// it wants a benchmark before it wants cleverness.
    pub fn draw<R: Rng + ?Sized>(
        &mut self,
        pool: &SharedPool,
        requested: u64,
        rng: &mut R,
    ) -> &[u32] {
        self.candidates.clear();
        match self.rank_by {
            // Rank is the slot itself, so rank order is slot order. Empty slots
            // are simply absent: a vacant position has nothing to select.
            RankBy::Slot => self.candidates.extend(pool.selectable().map(|i| i.slot())),
            // Rank 0 is newest.
            RankBy::Recency => self.candidates.extend(pool.by_recency().map(|i| i.slot())),
        }
        let live = self.candidates.len() as u64;
        let nominal = pool.nominal();
        // `live` may exceed `nominal`: a Poisson population fluctuates both ways,
        // and FR-022 bounds the draw by the *smaller* of the two, so a pool
        // temporarily above target still yields at most its nominal size. That is
        // the spec's rule and not an oversight — the nominal size is what the
        // author asked a session to be able to draw.
        let bound = requested.min(nominal).min(live);
        self.stats.draws += 1;
        self.stats.requested += requested;
        self.stats.delivered += bound;
        if bound < requested {
            // Attribute to the tighter cause. Nominal first: if the description
            // asks for more than the pool can ever hold, that is the authoring
            // error, and a fluctuation below it is a symptom rather than the
            // finding.
            if nominal < requested {
                self.stats.bound_by_nominal += 1;
            } else {
                self.stats.bound_by_live += 1;
            }
        }
        if live == 0 {
            self.stats.empty_pool += 1;
        }

        let k = bound as usize;
        match self.selection.clone() {
            // Partial Fisher-Yates: after k steps the first k entries are a uniform
            // sample without replacement, and no allocation happened.
            None => {
                for i in 0..k {
                    let j = i + rng.gen_range(0..(self.candidates.len() - i));
                    self.candidates.swap(i, j);
                }
                self.candidates.truncate(k);
            }
            // Concentrated: ranks come from the distribution, so a spread narrower than the pool
            // makes the working set smaller than the key space.
            Some(dist) => self.draw_ranked(&dist, k, live as usize, rng),
        }
        self.candidates.sort_unstable();
        &self.candidates
    }
}

impl Selector {
    /// Choose `k` distinct ranks from `dist`, then keep those candidates.
    ///
    /// # Rejection, with a budget, and what happens when it runs out
    ///
    /// Sampling *without replacement* from a concentrated distribution has no closed form: the
    /// obvious alternative — drawing into the shrinking list of remaining candidates — silently
    /// reshapes the distribution as the list shrinks, so a "concentrated" draw would flatten
    /// exactly when it was asked for the most instances.
    ///
    /// So a duplicate rank is rejected and redrawn, with a budget proportional to `k`. When the
    /// budget runs out the remainder is filled **in rank order** from what has not been chosen,
    /// which is the least distorting completion available: it favours the ranks the distribution
    /// already favours. That case means the description asked one session for more instances
    /// than its spread realistically covers, and [`SelectionStats::exhausted_retries`] counts it
    /// rather than leaving it to be inferred from a curve that looks slightly wrong.
    fn draw_ranked<R: Rng + ?Sized>(
        &mut self,
        dist: &Resolved,
        k: usize,
        live: usize,
        rng: &mut R,
    ) {
        self.picked.clear();
        let budget = 8 * k.max(1) + 16;
        let mut attempts = 0usize;
        while self.picked.len() < k && attempts < budget {
            attempts += 1;
            // Truncated to the live ranks: a distribution wider than the pool would otherwise
            // spend most of its mass outside it, which would look like a much smaller working
            // set than was asked for.
            let raw = dist.sample(rng);
            if !raw.is_finite() {
                continue;
            }
            let rank = (raw.round().max(0.0) as usize).min(live.saturating_sub(1));
            if !self.picked.contains(&rank) {
                self.picked.push(rank);
            }
        }
        if self.picked.len() < k {
            self.stats.exhausted_retries += 1;
            for rank in 0..live {
                if self.picked.len() >= k {
                    break;
                }
                if !self.picked.contains(&rank) {
                    self.picked.push(rank);
                }
            }
        }
        // Map ranks to slots, then keep only those.
        let chosen: Vec<u32> = self.picked.iter().map(|r| self.candidates[*r]).collect();
        self.candidates.clear();
        self.candidates.extend(chosen);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::description::Population;
    use crate::distribution::{Distribution, Kind};
    use crate::pool::SharedPool;
    use crate::rng;

    fn pool(form: Population, lifetime: Kind) -> (SharedPool, rng::Rng) {
        let mut r = rng::substream(11, "pools");
        let lt = Distribution::new(lifetime).resolve(None).unwrap();
        let len = Distribution::new(Kind::Constant { value: 4.0 })
            .resolve_integral(None)
            .unwrap();
        let mut p = SharedPool::new(0, form, lt, len, &mut r);
        p.seed(0.0, &mut r);
        (p, r)
    }

    fn immortal(n: u64) -> (SharedPool, rng::Rng) {
        pool(
            Population::Exact(n),
            Kind::Constant {
                value: f64::INFINITY,
            },
        )
    }

    #[test]
    fn a_draw_is_distinct_and_in_slot_order() {
        let (p, mut r) = immortal(12);
        let mut s = Selector::new(RankBy::Slot);
        for _ in 0..200 {
            let got = s.draw(&p, 5, &mut r);
            assert_eq!(got.len(), 5);
            assert!(got.windows(2).all(|w| w[0] < w[1]), "not sorted/distinct");
            assert!(got.iter().all(|slot| p.by_slot(*slot).is_some()));
        }
        assert_eq!(s.stats().binding_fraction(), 0.0);
        assert_eq!(s.stats().shortfall_fraction(), 0.0);
    }

    #[test]
    fn selection_is_uniform_over_live_instances() {
        // The property FR-021 describes: with no selection distribution every
        // instance is equally likely, so the working set is the whole key space.
        let n = 8u64;
        let (p, mut r) = immortal(n);
        let mut s = Selector::new(RankBy::Slot);
        let mut hits = vec![0u32; n as usize];
        let draws = 40_000;
        for _ in 0..draws {
            for slot in s.draw(&p, 2, &mut r) {
                hits[*slot as usize] += 1;
            }
        }
        let expect = (draws * 2) as f64 / n as f64;
        for (slot, h) in hits.iter().enumerate() {
            let rel = (*h as f64 - expect).abs() / expect;
            assert!(rel < 0.05, "slot {slot}: {h} hits, expected ~{expect:.0}");
        }
    }

    #[test]
    fn rank_by_is_inert_under_uniform_selection() {
        // Recorded as a property rather than left implicit: uniform over the live
        // instances is the same distribution whichever way ranks are numbered, so
        // `rank_by` cannot matter until US4 puts a shape on the index space. A
        // test that claimed to distinguish them would be measuring its own seed.
        let n = 6u64;
        let (p, _) = immortal(n);
        let mut totals = Vec::new();
        for rank_by in [RankBy::Slot, RankBy::Recency] {
            let mut r = rng::substream(3, "selection");
            let mut s = Selector::new(rank_by);
            let mut hits = vec![0u32; n as usize];
            for _ in 0..20_000 {
                for slot in s.draw(&p, 2, &mut r) {
                    hits[*slot as usize] += 1;
                }
            }
            totals.push(hits);
        }
        for (a, b) in totals[0].iter().zip(&totals[1]) {
            let rel = (*a as f64 - *b as f64).abs() / *a as f64;
            assert!(rel < 0.06, "slot counts diverged: {a} vs {b}");
        }
    }

    #[test]
    fn asking_for_more_than_the_pool_holds_is_bound_by_nominal() {
        // FR-022: never fails, never waits. And FR-023: the reason is recorded,
        // because this particular reason is an authoring error that the load-time
        // gate was supposed to catch.
        let (p, mut r) = immortal(4);
        let mut s = Selector::new(RankBy::Slot);
        let got = s.draw(&p, 10, &mut r);
        assert_eq!(got.len(), 4);
        let st = s.stats();
        assert_eq!(st.bound_by_nominal(), 1);
        assert_eq!(st.bound_by_live(), 0, "misattributed to the population");
        assert_eq!(st.requested(), 10);
        assert_eq!(st.delivered(), 4);
        assert!((st.shortfall_fraction() - 0.6).abs() < 1e-12);
        assert!((st.binding_fraction() - 1.0).abs() < 1e-12);
    }

    #[test]
    fn a_fluctuating_population_binds_by_live_not_by_nominal() {
        // The distinction that makes the counter worth having: the description is
        // fine, the pool is simply below target at that instant.
        let mut r = rng::substream(5, "pools");
        let lt = Distribution::new(Kind::Exponential { mean: 30.0 })
            .resolve(None)
            .unwrap();
        let len = Distribution::new(Kind::Constant { value: 2.0 })
            .resolve_integral(None)
            .unwrap();
        let mut p = SharedPool::new(0, Population::Poisson(12), lt, len, &mut r);
        p.seed(0.0, &mut r);
        let mut s = Selector::new(RankBy::Slot);
        // Ask for the whole nominal pool, so any shortfall is the population's.
        for step in 1..=400 {
            p.advance_to(step as f64 * 5.0, &mut r);
            let got = s.draw(&p, 12, &mut r).len();
            assert!(got <= 12);
        }
        let st = s.stats();
        assert_eq!(
            st.bound_by_nominal(),
            0,
            "requested == nominal, so nothing should be attributed to the description"
        );
        assert!(
            st.bound_by_live() > 0,
            "a Poisson pool asked for its whole nominal size never fell short — \
             the population is not fluctuating"
        );
        assert!(st.binding_fraction() > 0.0 && st.binding_fraction() <= 1.0);
    }

    #[test]
    fn a_small_poisson_pool_can_be_empty_and_is_counted_separately() {
        // P(live = 0) = e^-N, so 13.5% at a nominal of 2. The draw returns
        // nothing rather than failing, and the case is counted apart because
        // "always succeeds" is substantively empty here.
        let mut r = rng::substream(9, "pools");
        let lt = Distribution::new(Kind::Exponential { mean: 10.0 })
            .resolve(None)
            .unwrap();
        let len = Distribution::new(Kind::Constant { value: 1.0 })
            .resolve_integral(None)
            .unwrap();
        let mut p = SharedPool::new(0, Population::Poisson(2), lt, len, &mut r);
        p.seed(0.0, &mut r);
        let mut s = Selector::new(RankBy::Slot);
        for step in 1..=3000 {
            p.advance_to(step as f64 * 2.0, &mut r);
            let got = s.draw(&p, 1, &mut r);
            if got.is_empty() {
                assert_eq!(p.live(), 0, "empty draw from a non-empty pool");
            }
        }
        let st = s.stats();
        assert!(
            st.empty_pool() > 0,
            "a nominal-2 Poisson pool was never empty in 3000 samples; \
             e^-2 is 13.5%, so this is a bug and not luck"
        );
        // Sanity on the magnitude rather than a tight bound: the empirical rate
        // should be in the neighbourhood of e^-2, not orders away.
        let rate = st.empty_pool() as f64 / st.draws() as f64;
        assert!(
            (0.03..0.35).contains(&rate),
            "empty-pool rate {rate:.3} is nowhere near e^-2 = 0.135"
        );
    }

    #[test]
    fn an_empty_draw_still_counts_as_a_draw() {
        // A zero-instance pool must not silently vanish from the statistics, or
        // the report would understate how badly the description is behaving.
        let (p, mut r) = pool(Population::Poisson(0), Kind::Exponential { mean: 5.0 });
        let mut s = Selector::new(RankBy::Slot);
        assert!(s.draw(&p, 3, &mut r).is_empty());
        let st = s.stats();
        assert_eq!(st.draws(), 1);
        assert_eq!(st.empty_pool(), 1);
        assert_eq!(st.delivered(), 0);
        assert!((st.shortfall_fraction() - 1.0).abs() < 1e-12);
    }

    #[test]
    fn requesting_zero_is_not_recorded_as_a_shortfall() {
        let (p, mut r) = immortal(5);
        let mut s = Selector::new(RankBy::Slot);
        assert!(s.draw(&p, 0, &mut r).is_empty());
        let st = s.stats();
        assert_eq!(st.draws(), 1);
        assert_eq!(st.bound_by_nominal(), 0);
        assert_eq!(st.bound_by_live(), 0);
        assert_eq!(st.shortfall_fraction(), 0.0);
    }

    #[test]
    fn the_index_space_stays_bounded_as_the_pool_turns_over() {
        // The requirement behind "bounded index space": ranks must not grow with
        // the number of instances ever minted, or heat would attach permanently
        // to the earliest ones and later mints would never be selected.
        let mut r = rng::substream(13, "pools");
        let lt = Distribution::new(Kind::Constant { value: 10.0 })
            .resolve(None)
            .unwrap();
        let len = Distribution::new(Kind::Constant { value: 2.0 })
            .resolve_integral(None)
            .unwrap();
        let mut p = SharedPool::new(0, Population::Exact(6), lt, len, &mut r);
        p.seed(0.0, &mut r);
        let mut s = Selector::new(RankBy::Slot);
        let mut highest = 0u32;
        for step in 1..=300 {
            p.advance_to(step as f64 * 10.0, &mut r);
            for slot in s.draw(&p, 6, &mut r) {
                highest = highest.max(*slot);
            }
        }
        assert!(p.total_mints() > 100, "no turnover, so nothing was tested");
        assert!(
            highest < 6,
            "selection reached slot {highest} in a 6-instance pool after {} mints",
            p.total_mints()
        );
    }

    #[test]
    fn draws_do_not_allocate_after_the_first() {
        // The buffer exists so a draw allocates nothing; a regression here would
        // put an allocation on every session birth.
        let (p, mut r) = immortal(64);
        let mut s = Selector::new(RankBy::Slot);
        s.draw(&p, 8, &mut r);
        let cap = s.candidates.capacity();
        for _ in 0..500 {
            s.draw(&p, 8, &mut r);
        }
        assert_eq!(s.candidates.capacity(), cap, "the candidate buffer regrew");
    }
}
