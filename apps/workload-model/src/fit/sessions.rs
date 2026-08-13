//! Fitting `workload.sessions` and `corpus.trees.shared_depth` (spec FR-055).
//!
//! Every measurement here is the one `contracts/workload-schema.md` § Fitting
//! specifies, and none of them is a second definition of something `stats` already
//! computes:
//!
//! | Parameter | Measurement |
//! | --- | --- |
//! | `turns` | invocations per session |
//! | `think_time` | `request_start` delta between consecutive turns of one session |
//! | `private_depth` | turn-1 path depth − that request's longest common prefix |
//! | `growth_per_turn` | path-depth increment between consecutive turns |
//! | `shared_depth` | the realised prefix-sharing of each session's **turn-1** request |
//!
//! The longest-common-prefix rule is **not** reimplemented: it is read from
//! [`Sharing::last_prefix_len`](crate::stats::sharing::Sharing::last_prefix_len), so
//! the `private_depth` this emits and the `shared_depth` a validator checks are on
//! one definition. Two of them would make FR-057's divergence a comparison of
//! definitions rather than of measurements.
//!
//! `shared_depth` is that one definition applied to **turn-1 requests only**, because it
//! is drawn once per session and a parameter has to be fitted in the space it is drawn in
//! (FR-054h). The validator's population is every request, which is the same measurement
//! turn-weighted — and the generator applies that weighting itself by replaying the drawn
//! prefix on every turn, so fitting from the weighted form applies it twice and adds a
//! constant to every generated path.
//!
//! # What is emitted as `empirical`, and why that is not a cop-out
//!
//! `shared_depth` and `turns` come out as `empirical` CDF points rather than as a
//! fitted parametric shape. The contract says so directly — "`shared_depth` **is**
//! the FR-056 validation statistic, so the model is parameterised in the space it is
//! validated in, and `empirical` is the natural default rather than an escape hatch".
//! A lognormal fitted to a bimodal histogram would be a worse model that *looked*
//! more confident.

use serde::{Deserialize, Serialize};

use crate::dist::{Dist, Shape};
use crate::schema::{Growth, GrowthBand, GrowthBands};
use crate::stats::hist::Hist;
use crate::stats::sharing::SharingReport;

/// Most steps an `empirical` distribution is emitted with.
///
/// **Not a readability budget.** A step CDF is exact at its atoms and wrong between
/// them, in two ways that both scale with how much probability mass one step carries:
///
/// - Against the distribution it summarises, its KS distance is the mass of its
///   largest step — the whole step appears at once where the original was spread out.
///   So the step spacing is a **floor under the divergence the fit is gated on**.
/// - Every value inside a step is emitted as one value, so a step biases the mean by
///   the spread of what it absorbs.
///
/// This was measured, not reasoned about. Nine percentile points
/// (`0.05, 0.10, 0.25, 0.50, 0.75, 0.90, 0.95, 0.99, 1.00`) put the emitted median
/// and every other target quantile within 0.005 of the trace's, and still failed the
/// round trip on both counts at once: `shared_depth` came back as 8 atoms against the
/// trace's 37 values, whose largest step carried 0.286 of the mass and set a KS
/// distance of 0.234 against a 0.05 tolerance; and placing each step's mass at the
/// top of its interval inflated `private_depth`'s mean by 24%, `turns`' by 25% and
/// `growth_per_turn`'s by 9%, which came out as a synthetic plan running 35% more
/// references than its source. **A distribution can match at every quantile you
/// checked and still have the wrong mean**, and request length is a sum, so it is the
/// mean that reaches it.
///
/// 64 steps puts the spacing floor at 0.016, inside the 0.02 of the tightest
/// tolerance any gated statistic uses. Below that the emitted YAML is longer than a
/// hand-written one, which is the correct trade: a fitted document is machine output
/// checked by a gate, and the gate is on resemblance rather than on brevity.
const EMPIRICAL_MAX_STEPS: usize = 64;

/// Session-length bands `growth_per_turn` is fitted over, as lower bounds in turns.
///
/// Geometric, because turn counts span orders of magnitude: a linear grid would put
/// almost every session in the first cell and leave the long sessions — the ones whose
/// growth rate the accumulated depth is most sensitive to, since the weight is
/// quadratic in turn count — spread thinly across the rest.
///
/// The ladder is finer than the effect needs and then **merged down to whatever the
/// data supports**, rather than being chosen to match a particular trace. See
/// [`MIN_BAND_SESSIONS`].
const GROWTH_BANDS: [u64; 13] = [2, 3, 4, 6, 8, 12, 16, 24, 32, 48, 64, 96, 128];

/// Sessions a band needs before it is emitted as its own band.
///
/// A band carrying three sessions is a fitted number that means nothing, and the
/// emitted document gives a reader no way to see how thin it was — so thin bands are
/// merged into their neighbour instead. Measured on two agentic traces, the 13-rung
/// ladder above collapses to 6 or 7 bands at this threshold, and the tail bands (96+
/// turns, 1-5 sessions each) fold into their neighbours where they carry almost no
/// weight anyway.
const MIN_BAND_SESSIONS: u64 = 25;

/// Per-session state while fitting.
#[derive(Debug, Clone, Default)]
struct Live {
    /// Every `(turn, path length)` this session showed, in arrival order.
    ///
    /// Retained rather than differenced on the fly because `growth_per_turn` is a
    /// property of the **turn chain** and a reader hands invocations over in
    /// **arrival** order; the two disagree on real traces, and differencing the
    /// arrival sequence measures something else entirely (see
    /// [`SessionShapes::observe`]).
    ///
    /// Two `u32`-plus-`u64` per invocation against the hundreds of block ids each
    /// invocation already costs, so this does not change the memory bound.
    chain: Vec<(u32, u64)>,
    /// The lowest turn seen so far, with its path length and realised prefix.
    ///
    /// The chain's first turn, which is not the first *arrival* when the two orders
    /// disagree — and `private_depth` is defined against the chain's first turn.
    first: Option<(u32, u64, u64)>,
    /// The previous turn index in **arrival** order, for the disorder count.
    last_turn: u32,
    /// The previous timestamp in **arrival** order, for `think_time`.
    last_start: Option<f64>,
    turns: u64,
    started: bool,
}

/// Accumulates the session-shape parameters.
#[derive(Debug, Default)]
pub struct SessionShapes {
    live: crate::stats::FastMap<u32, Live>,
    turns: Hist,
    /// One `(turn-1 path length, realised shared prefix)` pair per session.
    ///
    /// Retained rather than reduced to a histogram of their difference, because
    /// `private_depth` has to be recomputable against a *different* attempted shared
    /// depth than the realised one — see [`SessionShapes::private_depth_at`]. The two
    /// are correlated per session (a deeper path tends to share more), so subtracting
    /// one histogram from another would not give the same answer as subtracting per
    /// session, which is the answer the generator's path formula needs.
    ///
    /// Memory is one pair per session — 16 bytes against the tens of thousands of
    /// bytes each session's blocks already cost, so it does not change the bound.
    turn_one: Vec<(u64, u64)>,
    growth: Hist,
    /// Think times in milliseconds, so the histogram's integer buckets have useful
    /// resolution: a think time of 3 s is 3000 buckets rather than 3.
    think_ms: Hist,
    /// Requests whose turn index went backwards or repeated **in arrival order**.
    ///
    /// A statement about the trace's timestamps, not about its structure: it says the
    /// order requests arrived in disagrees with the order their turn indices give.
    /// `growth_per_turn` is measured along the turn chain and is unaffected;
    /// `think_time` is measured along the arrival stream, which is where those two
    /// orders parting company actually shows up. See [`SessionShapes::observe`] for
    /// why the two statistics use different orders.
    out_of_order: u64,
    /// Adjacent turns **in chain order** whose path got shorter.
    ///
    /// This is the genuine binding restriction, and the one the old `out_of_order`
    /// caveat was reaching for and getting wrong: FR-014a's path can only grow, so a
    /// decrease along the *chain* is something the model cannot express. A
    /// conversation whose context shrinks — a trimmed history, a dropped tool result —
    /// is a real workload, so a non-zero count here is a limit of this model rather
    /// than a fault in the trace (FR-054a). Measured on the three agentic traces the
    /// old caveat blamed, it is **zero**: they are perfect chains, and only their
    /// arrival order was disordered.
    non_monotone_steps: u64,
    /// Whether the chains have been folded in already.
    sealed: bool,
    /// `Σ turns`, `Σ turns × turn-1 depth`, `Σ (depth − turn-1 depth)` and
    /// `Σ turns(turns−1)/2` — the exact terms behind FR-014a's path formula, kept so a
    /// report can account for a mean-length gap by arithmetic. See [`PathBudget`].
    requests: u64,
    weighted_turn_one_depth: u64,
    /// Turn-1 sharing, each session's value repeated once per turn.
    ///
    /// The turn-weighted image of the per-session `shared_depth` population — which is
    /// exactly what the generator's all-requests sharing would look like if
    /// `shared_depth` were conditioned on session length. Kept so the fit can report how
    /// much of the sharing gap that conditioning would actually close; see
    /// [`SessionShapes::shared_depth_buckets_turn_weighted`].
    t1_weighted: Hist,
    /// `Σ turns × turn-1 realised shared prefix`.
    ///
    /// Beside the unweighted mean this says whether the all-requests sharing histogram
    /// differs from the per-session one because sharing **grows within** a session, or
    /// only because sessions that share more deeply run longer. Those have different
    /// fixes and the same symptom — see [`PathBudget::turn_one_shared_weighted`].
    weighted_turn_one_shared: u64,
    accumulated_growth: u64,
    accumulated_steps: u64,
    /// `growth_per_turn` increments, split by the session-length band the session that
    /// produced them falls in. See [`GROWTH_BANDS`].
    growth_bands: Vec<Hist>,
    /// Sessions per band, so a band too thin to fit can be merged rather than emitted.
    band_sessions: Vec<u64>,
    /// One `(turns, turn-1 depth)` pair per session, for fitting the depth ceiling.
    ///
    /// The ceiling is solved against the accumulation it governs, and that calculation
    /// needs each session's length and starting depth — the same two numbers the
    /// generator will build a path from. A pair per session, so this is the same order
    /// of memory as `turn_one`.
    shapes: Vec<(u64, u64)>,
    /// The deepest path each session reached, so the fitted ceiling can be reported
    /// beside where sessions actually topped out and be seen to have independent
    /// meaning rather than being only the number that makes a sum work.
    deepest: Hist,
    /// Turn-1 requests whose shared prefix exceeded their own path length.
    ///
    /// Impossible if the prefix is a prefix, so a non-zero count means the two
    /// measurements disagree and `private_depth` is not trustworthy.
    prefix_longer_than_path: u64,
}

impl SessionShapes {
    /// An empty accumulator.
    pub fn new() -> SessionShapes {
        SessionShapes::default()
    }

    /// Record one closed request.
    ///
    /// `shared_len` must be the request's longest common prefix as
    /// `stats::sharing` measured it, and `turn` its 0-based invocation index.
    ///
    /// # Two statistics, two orders, and the reason they differ
    ///
    /// Requests arrive here in the order the reader sorted them into, which is
    /// **timestamp order** where a trace has real timestamps. That is the right order
    /// for reuse distance and for sharing, which are properties of a stream. It is the
    /// **wrong** order for `growth_per_turn`, which is a property of the *turn chain*
    /// FR-014a describes — and on real traces the two orders disagree badly.
    ///
    /// So `growth_per_turn` is differenced along the chain, once the turns are back in
    /// index order, when the accumulator is sealed. Differencing the arrival sequence
    /// instead was a **measured 2.08x-2.28x over-estimate** on three agentic traces:
    /// clamping each decrease to zero while counting the positive increments on either
    /// side of it in full makes the sum exceed the session's true span by twice the
    /// decreases, and FR-014a then accumulates that excess into every later turn. It
    /// came out as synthetic output 1.6x longer than its source and a `request_length`
    /// divergence of 0.18 against a 0.02 tolerance. In chain order those same traces
    /// have **zero** decreasing steps and an inflation factor of exactly 1.000.
    ///
    /// `think_time` stays on the **arrival** order, and that is not an oversight. It is
    /// a wall-clock gap between one session's consecutive requests, so the stream is
    /// the axis that reproduces it; and in arrival order the gap is non-negative by
    /// construction, so nothing is clamped. Differencing timestamps along the chain
    /// would produce negative gaps on 16-17% of adjacent pairs of those same traces,
    /// carrying 90% of the positive magnitude — clamping *those* would swap one silent
    /// bias for another.
    pub fn observe(
        &mut self,
        session: u32,
        turn: u32,
        path_len: u64,
        shared_len: u64,
        request_start: Option<f64>,
    ) {
        let live = self.live.entry(session).or_default();
        if live.started && turn <= live.last_turn {
            self.out_of_order += 1;
        }
        // The chain's first turn is the **lowest** turn index, which is not the first
        // arrival when the orders disagree. `private_depth` is defined against it, so
        // taking whichever request happened to arrive first put a mid-conversation
        // path where turn one's belonged.
        // `map_or(true, ..)` rather than `is_none_or`, which is stable only since 1.82
        // against this workspace's 1.75 MSRV.
        if live.first.map_or(true, |(t, _, _)| turn < t) {
            live.first = Some((turn, path_len, shared_len));
        }
        live.chain.push((turn, path_len));
        if let (Some(prev), Some(now)) = (live.last_start, request_start) {
            let gap = (now - prev).max(0.0);
            self.think_ms.add((gap * 1000.0) as u64);
        }
        live.last_turn = turn;
        live.last_start = request_start;
        live.turns += 1;
        live.started = true;
    }

    /// Difference every session's chain in turn order, once.
    ///
    /// Idempotent, because the fit's iteration loop calls [`SessionShapes::finish`]
    /// and [`SessionShapes::private_depth_at`] repeatedly against one accumulator.
    fn seal(&mut self) {
        if self.sealed {
            return;
        }
        self.sealed = true;
        for live in self.live.values_mut() {
            if let Some((_, path_len, shared_len)) = live.first {
                if shared_len > path_len {
                    self.prefix_longer_than_path += 1;
                }
                self.turn_one.push((path_len, shared_len));
                for _ in 0..live.turns {
                    self.t1_weighted.add(shared_len);
                }
            }
            self.turns.add(live.turns);
            // Stable by turn index. A trace with duplicate indices within a session
            // keeps its arrival order among the duplicates, which is the only
            // information left to order them by.
            live.chain.sort_by_key(|(t, _)| *t);
            // Turn-weighted, because a session with many turns contributes many
            // requests: what a *request* carries of turn-1 depth is the turn-weighted
            // mean, not the plain one.
            let turn_one_depth = live.chain.first().map(|(_, p)| *p).unwrap_or(0);
            self.shapes.push((live.turns, turn_one_depth));
            if let Some(deepest) = live.chain.iter().map(|(_, p)| *p).max() {
                self.deepest.add(deepest);
            }
            self.requests += live.turns;
            self.weighted_turn_one_depth += live.turns * turn_one_depth;
            self.weighted_turn_one_shared +=
                live.turns * live.first.map(|(_, _, sh)| sh).unwrap_or(0);
            self.accumulated_steps += live.turns * live.turns.saturating_sub(1) / 2;
            for (_, p) in &live.chain {
                self.accumulated_growth += p.saturating_sub(turn_one_depth);
            }
            // Which band this session's increments belong to. Its LENGTH selects the
            // band, so every increment a session contributes lands in the same one —
            // that is the whole point: growth rate is a property of the session, and
            // the session's length is what predicts it.
            let band = GROWTH_BANDS
                .iter()
                .rposition(|b| *b <= live.turns)
                .unwrap_or(0);
            if self.growth_bands.is_empty() {
                self.growth_bands = (0..GROWTH_BANDS.len()).map(|_| Hist::new()).collect();
                self.band_sessions = vec![0; GROWTH_BANDS.len()];
            }
            if live.turns >= 2 {
                self.band_sessions[band] += 1;
            }
            for (&(_, a), &(_, b)) in live.chain.iter().zip(live.chain.iter().skip(1)) {
                if b < a {
                    self.non_monotone_steps += 1;
                }
                self.growth_bands[band].add(b.saturating_sub(a));
                // Still clamped, because rule 8 and FR-014a both forbid a negative
                // growth and the distribution has no way to carry one. The difference
                // is that in chain order a clamp is a genuine violation being
                // surfaced by `non_monotone_steps`, rather than an artefact of the
                // order the requests happened to arrive in.
                self.growth.add(b.saturating_sub(a));
            }
        }
    }

    /// `private_depth` recomputed against an attempted shared depth `scale` times
    /// the realised one.
    ///
    /// The generator's path is `attempted_shared + private_depth + Σ growth`
    /// (FR-014a), while a fit measures `private_depth` as
    /// `turn-1 depth − *realised* shared prefix`. Those agree only when the attempted
    /// and realised sharing agree — and FR-012a says the drawn value is an *upper
    /// bound* on the realised one, so they generally do not. Feed a `shared_depth`
    /// fitted from realised sharing back in as an attempt and paths come out longer
    /// than the trace's by exactly the shortfall.
    ///
    /// So an iteration that raises the attempted sharing to make *realised* sharing
    /// match must lower `private_depth` by the same amount, per session, or it will
    /// fix the sharing statistic by breaking the request-length one. `scale` of 1.0
    /// reproduces the plain measurement.
    ///
    /// Clamped at zero: a session whose attempted sharing exceeds its own path has no
    /// private part, and a negative one is not expressible.
    pub fn private_depth_at(&mut self, scale: f64) -> Option<Dist> {
        self.seal();
        let mut h = Hist::new();
        for (path_len, shared_len) in &self.turn_one {
            let attempted = (*shared_len as f64 * scale).round().max(0.0) as u64;
            h.add(path_len.saturating_sub(attempted));
        }
        empirical_from(&h)
    }

    /// The means the generator's path formula is built from, for a report that has to
    /// account for a difference in *mean* request length rather than in shape.
    ///
    /// FR-014a builds a path as `shared_depth + private_depth + Σ growth_per_turn`, so a
    /// synthetic stream that runs longer than its source does so through one of exactly
    /// three terms. A divergence figure cannot say which; these can, by arithmetic.
    ///
    /// `turn_one_shared` is here because it is the term the other two are defined
    /// against: `private_depth` is `turn-1 depth − turn-1 shared prefix`, so if the
    /// `shared_depth` the generator draws has a different mean from the turn-1 prefix
    /// that was subtracted, every generated path is off by that difference before any
    /// growth is added.
    pub fn path_budget(&mut self) -> PathBudget {
        self.seal();
        let sessions = self.turn_one.len() as u64;
        let mean = |sum: u64| {
            if sessions == 0 {
                0.0
            } else {
                sum as f64 / sessions as f64
            }
        };
        PathBudget {
            sessions,
            turn_one_depth: mean(self.turn_one.iter().map(|(p, _)| *p).sum()),
            turn_one_shared: mean(self.turn_one.iter().map(|(_, s)| *s).sum()),
            private_depth: mean(
                self.turn_one
                    .iter()
                    .map(|(p, s)| p.saturating_sub(*s))
                    .sum(),
            ),
            growth: self.growth.mean(),
            growth_steps: self.growth.count(),
            turns: self.turns.mean(),
            requests: self.requests,
            weighted_turn_one_depth: self.weighted_turn_one_depth,
            weighted_turn_one_shared: self.weighted_turn_one_shared,
            accumulated_growth: self.accumulated_growth,
            accumulated_steps: self.accumulated_steps,
        }
    }

    /// Fit `sessions.max_depth` — the context window — against the accumulation it
    /// governs (FR-054c).
    ///
    /// Solved rather than read off a quantile of observed depth, because the ceiling's
    /// job is to reproduce **accumulated growth**, and a quantile is only a proxy for
    /// that. The two agentic traces measured wanted a ceiling of about 1400 blocks while
    /// their observed maximum depth had p75 near 1100 and p90 near 1550 — so no fixed
    /// quantile would have been right for both, and picking one would have been fitting
    /// a coincidence. The report prints those quantiles beside the answer anyway, so a
    /// ceiling that does *not* resemble where sessions top out is visible as such.
    ///
    /// # The closed form
    ///
    /// With a constant increment `g` and a per-session ceiling `C = max(cap, first)`,
    /// the turn with `k` increments applied sits at `min(first + k·g, C)`, so a session
    /// of `T` turns accumulates `Σ over k in 0..T−1 of min(k·g, R)` where `R = C −
    /// first`. Everything below `m = ⌊R/g⌋` increments is unclamped and everything above
    /// it contributes `R`, giving
    ///
    /// ```text
    /// J = min(m, T−1);   accumulated = g·J(J+1)/2 + R·(T−1−J)
    /// ```
    ///
    /// which is why no generate-measure loop is needed: the quantity is monotone in
    /// `cap` and computable directly, so a bisection settles it.
    ///
    /// Returns `None` when the trace shows no saturation — when even an unbounded model
    /// accumulates no more than the trace did, there is no ceiling to report and
    /// inventing one would constrain a workload the trace never constrained.
    pub fn fit_max_depth(&mut self) -> Option<u32> {
        self.seal();
        let g = self.growth.mean()?;
        if g <= 0.0 || self.shapes.is_empty() {
            return None;
        }
        let target = self.accumulated_growth as f64;
        let accumulated = |cap: u64| -> f64 {
            self.shapes
                .iter()
                .map(|(turns, first)| {
                    let k = turns.saturating_sub(1) as f64;
                    let r = (cap.max(*first) - *first) as f64;
                    let m = (r / g).floor();
                    let j = m.min(k);
                    g * j * (j + 1.0) / 2.0 + r * (k - j)
                })
                .sum()
        };
        // An unbounded model that already accumulates no more than the trace needs no
        // ceiling. `hi` starts past the deepest observed path, so the bracket is known
        // to contain the answer if one exists.
        let hi = self.deepest.max().unwrap_or(0).saturating_mul(2).max(1);
        if accumulated(hi) <= target {
            return None;
        }
        let (mut lo, mut hi) = (0u64, hi);
        while lo + 1 < hi {
            let mid = lo + (hi - lo) / 2;
            if accumulated(mid) < target {
                lo = mid;
            } else {
                hi = mid;
            }
        }
        // `hi` is the smallest ceiling reaching the target; take whichever of the two
        // brackets lands closer, since the function steps rather than being continuous.
        let pick = if (accumulated(lo) - target).abs() <= (accumulated(hi) - target).abs() {
            lo
        } else {
            hi
        };
        Some(pick.clamp(1, u64::from(u32::MAX)) as u32)
    }

    /// `growth_per_turn`, banded by session length (FR-054c, FR-054f).
    ///
    /// One distribution per band, over the increments of sessions whose turn count falls
    /// in that band — so a session's length selects where its growth comes from, and the
    /// accumulated depth stops being wrong by a factor that no marginal distribution can
    /// reveal.
    ///
    /// Sparse bands are **merged into their neighbour** rather than emitted, since a band
    /// fitted from three sessions is noise the emitted document gives a reader no way to
    /// discount. Merging is upward-accumulating: rungs are absorbed until the band has
    /// `MIN_BAND_SESSIONS` of them, and a short final band folds back into the one before
    /// it.
    ///
    /// Falls back to a single unbanded distribution when the trace supports only one
    /// band, so a trace of uniform-length sessions emits the simpler spelling rather
    /// than a one-element table pretending to a structure it has not shown.
    pub fn fit_growth(&mut self) -> Option<Growth> {
        self.seal();
        if self.growth_bands.is_empty() {
            return None;
        }
        // Absorb rungs until each band is thick enough to mean something.
        let mut bands: Vec<(u64, Hist, u64)> = Vec::new();
        let mut open: Option<(u64, Hist, u64)> = None;
        for (i, from) in GROWTH_BANDS.iter().enumerate() {
            let (h, n) = (&self.growth_bands[i], self.band_sessions[i]);
            match open.as_mut() {
                None => open = Some((*from, h.clone(), n)),
                Some((_, acc_h, acc_n)) => {
                    acc_h.merge(h);
                    *acc_n += n;
                }
            }
            if let Some((_, _, acc_n)) = open.as_ref() {
                if *acc_n >= MIN_BAND_SESSIONS {
                    bands.push(open.take().expect("open"));
                }
            }
        }
        // Whatever is left over is too thin to stand alone: fold it back.
        if let Some((_, h, n)) = open.take() {
            match bands.last_mut() {
                Some((_, last_h, last_n)) => {
                    last_h.merge(&h);
                    *last_n += n;
                }
                None => bands.push((GROWTH_BANDS[0], h, n)),
            }
        }
        bands.retain(|(_, h, _)| h.count() > 0);
        if bands.is_empty() {
            return None;
        }
        if bands.len() == 1 {
            return empirical_from(&bands[0].1).map(Growth::Uniform);
        }
        // The first band must start at the bottom, or a session shorter than anything
        // observed would fall outside the table. `Growth::at` clamps into the first band
        // anyway; stating it keeps the emitted document readable on its own terms.
        let mut by_turns = Vec::new();
        for (i, (from, h, _)) in bands.iter().enumerate() {
            let growth = match empirical_from(h) {
                Some(d) => d,
                None => continue,
            };
            by_turns.push(GrowthBand {
                from_turns: if i == 0 { 1 } else { *from as u32 },
                growth,
            });
        }
        if by_turns.len() < 2 {
            return by_turns.pop().map(|b| Growth::Uniform(b.growth));
        }
        Some(Growth::Banded(GrowthBands { by_turns }))
    }

    /// Sessions per emitted band, for the fit report.
    ///
    /// Reported because a band's width is a fitting decision a reader should be able to
    /// see: FR-055 requires a thin measurement to be visible as one.
    pub fn growth_band_sessions(&mut self) -> Vec<(u64, u64)> {
        self.seal();
        GROWTH_BANDS
            .iter()
            .zip(self.band_sessions.iter())
            .map(|(a, b)| (*a, *b))
            .collect()
    }

    /// The depth ceiling as a distribution over blocks, drawn once per session.
    ///
    /// Fitted as the **empirical distribution of the depth each session actually
    /// reached**, which is a measurement rather than a solve — and the reason a
    /// measurement is admissible here is asymmetric: a ceiling drawn *above* what a
    /// session would have reached has no effect, so over-estimating it for a session
    /// that merely ran out of conversation costs nothing, while the sessions that do
    /// saturate are exactly the ones whose observed maximum *is* their ceiling.
    ///
    /// [`SessionShapes::fit_max_depth`] solves for a single scalar instead, and that was
    /// measured and rejected: one ceiling reproduced the accumulated depth well — 1.02x
    /// on one trace — and destroyed the distribution, piling 1923 requests into the
    /// bucket at the ceiling where the trace had 173 and leaving the tail above it
    /// empty. `request_length` went from 0.058 to 0.108. It is kept for the report,
    /// because a scalar and a distribution disagreeing is worth seeing, but it is not
    /// what is emitted.
    ///
    /// `None` where the trace shows no saturation at all, so an unbounded model is left
    /// unbounded rather than being given a ceiling it never demonstrated.
    pub fn fit_max_depth_dist(&mut self) -> Option<Dist> {
        self.seal();
        self.fit_max_depth()?;
        empirical_from_buckets(&self.deepest.buckets())
    }

    /// The ceiling `fit` actually emits, which is **nothing** — and the measurement is
    /// why (FR-054c).
    ///
    /// A ceiling reproduces the accumulated depth and makes every gated statistic
    /// *worse*, because the gate is a distributional distance and a ceiling's damage to
    /// the shape exceeds its benefit to the mean. Measured on one agentic trace:
    ///
    /// | ceiling | references vs trace | `request_length` | `unique_keys` | reuse |
    /// | --- | --- | --- | --- | --- |
    /// | none | 1.210x | **0.058** | **0.135** | **0.010** |
    /// | one scalar, 1362 | 1.021x | 0.108 | 0.268 | 0.030 |
    /// | drawn per session | 0.875x | 0.113 | 0.565 | 0.047 |
    ///
    /// A single ceiling piles saturating sessions onto one depth — 1923 requests into
    /// the bucket at the ceiling against the trace's 173, with the tail above it empty.
    /// Drawing it per session from where sessions topped out spreads that spike but
    /// over-truncates instead, because a long session can draw a short session's
    /// ceiling and saturate far too early: the observed maximum of a session that simply
    /// ran out of conversation is not a ceiling at all.
    ///
    /// So the ceiling exists as something a document may **state** — a real context
    /// window is a real thing to model — and `fit` reports what it measured without
    /// emitting it. Fitting it would trade a gated statistic for an ungated one, and
    /// FR-054a asks for the limitation to be named rather than papered over. Closing
    /// this properly needs the growth *rate* to depend on session length rather than
    /// the depth to be clamped, which is a larger change.
    pub fn emitted_max_depth(&mut self) -> Option<Dist> {
        None
    }

    /// Quantiles of the deepest path each session reached.
    ///
    /// Reported beside [`SessionShapes::fit_max_depth`] so the fitted ceiling can be
    /// compared against where sessions actually topped out.
    pub fn deepest_depth(&mut self) -> crate::stats::hist::Quantiles {
        self.seal();
        self.deepest.summary()
    }

    /// Turn-1 path lengths, for a report that wants to show what was subtracted from.
    pub fn turn_one_depth(&mut self) -> Option<Dist> {
        self.seal();
        let mut h = Hist::new();
        for (path_len, _) in &self.turn_one {
            h.add(*path_len);
        }
        empirical_from(&h)
    }

    /// `shared_depth` — the realised shared prefix of each session's **turn-1** request
    /// (FR-054h).
    ///
    /// Not the sharing histogram over every request, which is the same measurement over
    /// a larger population and is what a validator recomputes. The distinction is which
    /// **space** the parameter is drawn in, and getting it wrong adds a constant to every
    /// generated path.
    ///
    /// The generator draws `shared_depth` **once per session**, at birth, and every one
    /// of that session's turns then re-walks the same trunk prefix. So the all-requests
    /// sharing distribution the generator produces is already the *turn-weighted image*
    /// of this per-session draw — the generator applies the weighting itself. Fitting the
    /// draw from the trace's all-requests histogram therefore applies that weighting
    /// **twice**, and since sessions that share more deeply also tend to run longer, the
    /// doubled weighting biases it upward.
    ///
    /// Measured on two agentic traces the gap is large and one-directional: sharing over
    /// all turns means 417.2 and 383.9 blocks against 383.9 and 309.3 over turn-1
    /// requests alone. Because `private_depth` is `turn-1 depth − turn-1's own prefix`,
    /// the excess lands on FR-014a's path in full — +33.3 and +74.6 blocks on every
    /// request generated.
    ///
    /// So this is not a trade of one statistic against another: the parameter belongs in
    /// the per-session space because that is where it is drawn, and the FR-056 statistic
    /// over all requests is then reproduced by the generator's own replay across turns
    /// rather than by fitting it directly.
    pub fn shared_depth(&mut self) -> Option<Dist> {
        self.seal();
        let mut h = Hist::new();
        for (_, shared_len) in &self.turn_one {
            h.add(*shared_len);
        }
        empirical_from(&h)
    }

    /// The emitted `shared_depth` population, as buckets plus a total.
    ///
    /// The same measurement [`SessionShapes::shared_depth`] fits, in the form the
    /// divergence code compares.
    pub fn shared_depth_buckets(&mut self) -> (Vec<(u64, u64, u64)>, u64) {
        self.seal();
        let mut h = Hist::new();
        for (_, sh) in &self.turn_one {
            h.add(*sh);
        }
        (h.buckets(), h.count())
    }

    /// The same population **turn-weighted**, which bounds what conditioning
    /// `shared_depth` on session length could achieve.
    ///
    /// The FR-056 sharing statistic is measured over every request while the parameter is
    /// drawn once per session, so the two live in different spaces and the fit cannot be
    /// exact in both. The KS distance from the all-requests histogram to *this* is the
    /// floor a `turns`-conditioned `shared_depth` would leave — everything below it is
    /// within-session deepening of sharing, which one per-session draw cannot express at
    /// all.
    ///
    /// Reported as a **KS distance** rather than as a difference of means, because that is
    /// the measure FR-056 gates on and the two disagree about what is worth doing: on one
    /// agentic trace the mean-space split attributes only 41% of the gap to the
    /// expressible half, while in KS terms closing that half is what crosses the
    /// tolerance.
    pub fn shared_depth_buckets_turn_weighted(&mut self) -> (Vec<(u64, u64, u64)>, u64) {
        self.seal();
        (self.t1_weighted.buckets(), self.t1_weighted.count())
    }

    /// Freeze into a fitted set of parameters.
    ///
    /// `sharing` supplies the requests that shared nothing, which is a property of the
    /// whole stream rather than of turn 1 and is reported as a limit on `shared_depth`'s
    /// support. It no longer supplies `shared_depth` itself — see
    /// [`SessionShapes::shared_depth`] for why that has to be measured over turn-1
    /// requests, in the space the generator draws it in.
    ///
    /// Borrows rather than consumes, so a caller iterating on the attempted sharing can
    /// keep calling [`SessionShapes::private_depth_at`] against the same measurements.
    pub fn finish(&mut self, sharing: &SharingReport) -> FittedSessions {
        self.seal();
        FittedSessions {
            sessions: self.live.len() as u64,
            turns: empirical_from(&self.turns),
            private_depth: self.private_depth_at(1.0),
            growth_per_turn: self.fit_growth(),
            // Seconds, which is what `think_time` is in (`SessionParams::think_time_s`).
            think_time: empirical_from(&self.think_ms).map(|d| scale(&d, 1.0 / 1000.0)),
            shared_depth: self.shared_depth(),
            unshared_requests: sharing.unshared_requests,
            max_depth: self.emitted_max_depth(),
            out_of_order_turns: self.out_of_order,
            non_monotone_steps: self.non_monotone_steps,
            prefix_longer_than_path: self.prefix_longer_than_path,
        }
    }
}

/// The means behind FR-014a's path formula, for accounting for a mean-length gap.
///
/// Every field is a mean over what the trace showed, so a report can compare the sum
/// the generator will build against the length the trace actually had, term by term.
#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub struct PathBudget {
    /// Sessions the means are over.
    pub sessions: u64,
    /// Mean path length of each session's lowest-numbered turn.
    pub turn_one_depth: f64,
    /// Mean realised shared prefix **of those turn-1 requests only**.
    pub turn_one_shared: f64,
    /// Mean `turn_one_depth − turn_one_shared`, which is what `private_depth` carries.
    pub private_depth: f64,
    /// Mean increment between consecutive turns, along the chain.
    pub growth: Option<f64>,
    /// Increments the mean is over — turns beyond the first.
    pub growth_steps: u64,
    /// Mean turns per session.
    pub turns: Option<f64>,
    /// Requests, i.e. `Σ turns`. The denominator every per-request figure below uses.
    pub requests: u64,
    /// `Σ turns × turn-1 depth`. Turn-**weighted**, because a session with many turns
    /// contributes many requests, so the plain mean of turn-1 depth is not the amount
    /// of turn-1 depth a request carries.
    pub weighted_turn_one_depth: u64,
    /// `Σ turns × turn-1 realised shared prefix`. See
    /// [`PathBudget::turn_one_shared_weighted`].
    pub weighted_turn_one_shared: u64,
    /// `Σ over sessions Σ over turns (this turn's depth − turn 1's depth)`.
    ///
    /// What the trace's accumulated growth actually totals, which is the quantity the
    /// generator has to reproduce.
    pub accumulated_growth: u64,
    /// `Σ turns(turns−1)/2` — how many increments the accumulation sums over.
    ///
    /// Beside [`PathBudget::accumulated_growth`] this gives the mean increment
    /// *weighted as the accumulation weights it*, which is the number an i.i.d. draw
    /// from the pooled marginal implicitly claims equals the pooled mean.
    pub accumulated_steps: u64,
}

impl PathBudget {
    /// Turn-1 realised sharing, **turn-weighted** — the diagnostic that separates two
    /// causes of one symptom.
    ///
    /// The sharing histogram over *all* requests exceeds the one over turn-1 requests on
    /// every real trace measured, and there are two possible reasons. Either sharing
    /// **grows within** a session, which this model cannot express at all — a session
    /// walks one trunk prefix of `shared_depth` blocks and every turn re-walks exactly
    /// that — or sessions that share more deeply simply **run longer**, so the
    /// all-requests histogram weights their deeper sharing more heavily.
    ///
    /// This figure tells them apart. It applies the turn weighting to turn-1's sharing
    /// and nothing else, so:
    ///
    /// - if it matches the all-requests mean, sharing is constant within a session and
    ///   the whole gap is the length-to-sharing correlation, which is expressible —
    ///   `shared_depth` has to be conditioned on session length the way FR-054f
    ///   conditions `growth_per_turn`;
    /// - if it falls short of the all-requests mean, sharing also deepens along the
    ///   conversation, and that part is a limit of the model rather than a fitting error.
    pub fn turn_one_shared_weighted(&self) -> f64 {
        if self.requests == 0 {
            0.0
        } else {
            self.weighted_turn_one_shared as f64 / self.requests as f64
        }
    }

    /// Accumulated growth per request, as the trace has it.
    pub fn accumulated_per_request(&self) -> f64 {
        if self.requests == 0 {
            0.0
        } else {
            self.accumulated_growth as f64 / self.requests as f64
        }
    }

    /// Accumulated growth per request that an **i.i.d.** per-turn draw from the pooled
    /// mean would produce.
    ///
    /// The increment at position `i` of a session with `T` turns is inherited by every
    /// turn after it, so it enters the accumulation with weight `T − i`; summed over a
    /// session that is `T(T−1)/2`. An i.i.d. draw therefore yields
    /// `pooled mean × Σ T(T−1)/2`, and that equals the truth only if the mean increment
    /// under that weighting is the pooled mean. The weighting is quadratic in `T`, so
    /// the longest sessions dominate it — and where their growth rate differs from the
    /// population's, this diverges while every marginal distribution stays correct.
    pub fn accumulated_per_request_iid(&self) -> Option<f64> {
        if self.requests == 0 {
            return None;
        }
        self.growth
            .map(|g| g * self.accumulated_steps as f64 / self.requests as f64)
    }

    /// How far an i.i.d. per-turn draw overstates the accumulated depth.
    pub fn iid_inflation(&self) -> Option<f64> {
        let truth = self.accumulated_per_request();
        if truth <= 0.0 {
            return None;
        }
        self.accumulated_per_request_iid().map(|i| i / truth)
    }
}

/// A fitted `workload.sessions`, with the parameters that could not be measured
/// left as `None` rather than defaulted (FR-055).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FittedSessions {
    /// Sessions the fit saw.
    pub sessions: u64,
    /// Invocations per session.
    pub turns: Option<Dist>,
    /// Turn-1 depth beyond the shared prefix.
    pub private_depth: Option<Dist>,
    /// Depth increment between consecutive turns.
    pub growth_per_turn: Option<Growth>,
    /// Seconds between consecutive turns of one session; `None` without timestamps.
    pub think_time: Option<Dist>,
    /// The realised sharing histogram, as the schema's `shared_depth`.
    pub shared_depth: Option<Dist>,
    /// The fitted depth ceiling — a distribution over blocks, drawn once per session,
    /// or `None` where the trace shows no saturation (FR-054c).
    pub max_depth: Option<Dist>,
    /// Requests that shared nothing at all.
    ///
    /// Not folded into `shared_depth`: "shares nothing" and "shares one block" are
    /// different workloads, and the emitted distribution's support starts at 1.
    pub unshared_requests: u64,
    /// Turns whose arrival order disagreed with their turn index.
    ///
    /// Informational: it affects no fitted parameter, since `growth_per_turn` is
    /// differenced along the chain and `think_time` along the stream.
    pub out_of_order_turns: u64,
    /// Adjacent turns **in chain order** whose path got shorter — a workload FR-014a's
    /// grow-only path cannot express, and so a limit of the model (FR-054a).
    pub non_monotone_steps: u64,
    /// Turn-1 requests whose prefix exceeded their path — impossible, so non-zero
    /// means the two measurements disagree.
    pub prefix_longer_than_path: u64,
}

impl FittedSessions {
    /// The caveats a fit report must carry about these parameters.
    pub fn caveats(&self) -> Vec<String> {
        let mut out = Vec::new();
        if self.think_time.is_none() {
            out.push(
                "MODEL LIMITATION (FR-054a): think_time is unset because the trace carries no \
                 usable per-request timestamps. A trace is under no obligation to record them, \
                 and this model requires an arrival process, so the gap is left unset rather \
                 than defaulted — a default would be indistinguishable from a measurement \
                 (FR-055)"
                    .to_string(),
            );
        }
        if self.out_of_order_turns > 0 {
            out.push(format!(
                "{} turns arrived in an order that disagrees with their turn indices. This is a \
                 property of the trace's timestamps, not of its structure: growth_per_turn is \
                 differenced along the turn chain and is unaffected, while think_time is the \
                 wall-clock gap between one session's consecutive arrivals and so is measured \
                 along the stream. Where the two orders part company those are different \
                 questions, and each statistic is answered on its own axis",
                self.out_of_order_turns
            ));
        }
        if self.non_monotone_steps > 0 {
            out.push(format!(
                "MODEL LIMITATION (FR-054a): {} adjacent turns get shorter along the turn chain, \
                 and FR-014a's path model can only grow — turn n+1's path is a strict extension \
                 of turn n's. A conversation whose context shrinks is a real thing (a trimmed or \
                 summarised history, a tool result dropped) and this model has no way to say it, \
                 so growth_per_turn fitted here understates those turns: the decrease is clamped \
                 to zero rather than inventing a negative growth. The trace is not at fault; the \
                 model's path formula is too narrow for it",
                self.non_monotone_steps
            ));
        }
        if self.prefix_longer_than_path > 0 {
            out.push(format!(
                "{} turn-1 requests had a shared prefix longer than their own path, which \
                 is impossible for a prefix: private_depth from this trace is not \
                 trustworthy",
                self.prefix_longer_than_path
            ));
        }
        if self.unshared_requests > 0 {
            out.push(format!(
                "MODEL LIMITATION (FR-054a): {} requests shared nothing at all, and \
                 `shared_depth`'s support starts at 1 — this model has no way to say that a \
                 request shares no prefix with anything. A generated model will therefore give \
                 every session some sharing where this trace gave those requests none. \
                 Cold-start requests are ordinary workload, so the gap is in the model's support, \
                 not in the trace",
                self.unshared_requests
            ));
        }
        out
    }
}

/// An `empirical` distribution from a histogram, or `None` if it has no samples.
fn empirical_from(h: &Hist) -> Option<Dist> {
    empirical_from_buckets(&h.buckets())
}

/// An `empirical` distribution from a bucket list, as a **step** CDF.
///
/// `dist::empirical` interpolates its points linearly, which is right for a
/// continuous quantity and wrong for these: `turns`, `private_depth`,
/// `growth_per_turn` and `shared_depth` are all counts of blocks or invocations.
/// Emitted as bare `(value, cumulative)` points, a session population of 1, 4 and 4
/// turns comes back with a median of 2 — a value the trace never contained.
///
/// So each step contributes **two** points, `(v, c_before)` and `(v, c_after)`, which
/// makes the interpolated CDF a staircase and reproduces a discrete distribution. The
/// zero-width segment between them is what `dist::empirical` already handles by taking
/// the lower value.
///
/// # Where the steps go, which is the part that decides whether the fit passes
///
/// One step per **occupied bucket** wherever that fits inside
/// [`EMPIRICAL_MAX_STEPS`], which for a count distribution of modest range means the
/// emitted distribution *is* the measured one — no spacing floor under the divergence
/// and no bias in any moment. [`Hist`](crate::stats::hist) counts values below
/// `LINEAR` exactly, so `turns`, `shared_depth` and most block counts land here.
///
/// Above the budget, consecutive buckets merge into equal-mass groups, and each
/// group's atom sits at the **mass-weighted mean of the values it absorbs** rather
/// than at the top of its interval. That placement is the whole difference between a
/// summary that preserves the mean and one that inflates it: emitting the interval's
/// top biased `private_depth`'s mean 24% high (see [`EMPIRICAL_MAX_STEPS`]), and a
/// distribution the generator *sums* is one whose mean has to survive.
///
/// A bucket's own value is likewise its midpoint rather than its lower bound, which
/// is the same argument one level down. Below `LINEAR` a bucket is a single integer
/// and the midpoint *is* that integer, so the exact case stays exact.
fn empirical_from_buckets(buckets: &[(u64, u64, u64)]) -> Option<Dist> {
    let total: u64 = buckets.iter().map(|(_, _, c)| *c).sum();
    if total == 0 {
        return None;
    }
    // Midpoint, not lower bound: it equals the value where the bucket is exact, and
    // is the better estimate of the bucket's mean where it is not.
    let occupied: Vec<(f64, u64)> = buckets
        .iter()
        .filter(|(_, _, c)| *c > 0)
        .map(|(lo, hi, c)| ((*lo as f64 + *hi as f64) / 2.0, *c))
        .collect();
    if occupied.is_empty() {
        return None;
    }

    let groups: Vec<(f64, u64)> = if occupied.len() <= EMPIRICAL_MAX_STEPS {
        occupied
    } else {
        let per = total as f64 / EMPIRICAL_MAX_STEPS as f64;
        let mut out: Vec<(f64, u64)> = Vec::new();
        let mut weighted = 0.0f64;
        let mut count = 0u64;
        let mut closed = 0u64;
        for (v, c) in &occupied {
            weighted += v * *c as f64;
            count += *c;
            if ((closed + count) as f64) >= (out.len() + 1) as f64 * per {
                out.push((weighted / count as f64, count));
                closed += count;
                weighted = 0.0;
                count = 0;
            }
        }
        if count > 0 {
            out.push((weighted / count as f64, count));
        }
        out
    };

    let mut steps: Vec<(f64, f64)> = Vec::new();
    let mut acc = 0u64;
    for (v, c) in groups {
        acc += c;
        steps.push((v, acc as f64 / total as f64));
    }
    // The top of the support must reach 1.0, or a draw above the last point would
    // clamp to it and the tail would be lost.
    if let Some(last) = steps.last_mut() {
        last.1 = 1.0;
    }

    let mut points: Vec<(f64, f64)> = Vec::new();
    let mut prev_c = 0.0f64;
    for (v, c) in steps {
        if prev_c > 0.0 {
            points.push((v, prev_c));
        }
        points.push((v, c));
        prev_c = c;
    }
    Some(Dist::Shaped(Shape::Empirical { points }))
}

/// Scale an empirical distribution's values.
///
/// Used for a unit conversion (milliseconds to seconds) and by the iteration that
/// raises an attempted `shared_depth` above the realised one. Values only: the
/// cumulative probabilities are the shape and must not move.
pub fn scale_values(d: &Dist, factor: f64) -> Dist {
    scale(d, factor)
}

/// Scale an empirical distribution's values, for a unit conversion.
fn scale(d: &Dist, factor: f64) -> Dist {
    match d {
        Dist::Shaped(Shape::Empirical { points }) => Dist::Shaped(Shape::Empirical {
            points: points.iter().map(|(v, p)| (v * factor, *p)).collect(),
        }),
        other => other.clone(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn quantile_of(d: &Option<Dist>, q: f64) -> f64 {
        d.as_ref().and_then(|d| d.quantile(q)).expect("a quantile")
    }

    /// The single distribution a `Growth` resolves to when it is not banded.
    ///
    /// Fixtures whose sessions are all one length fit one band, so `fit_growth` emits
    /// the unbanded spelling and these assertions read as they did before banding.
    fn growth_dist(g: &Option<Growth>) -> Option<Dist> {
        match g {
            Some(Growth::Uniform(d)) => Some(d.clone()),
            Some(Growth::Banded(b)) => b.by_turns.first().map(|x| x.growth.clone()),
            None => None,
        }
    }

    /// A sharing report over the given realised depths.
    fn sharing_with(depths: &[u64]) -> SharingReport {
        let mut h = Hist::new();
        for d in depths {
            h.add(*d);
        }
        h.seal();
        SharingReport {
            requests: depths.len() as u64,
            sharing_requests: h.count(),
            unshared_requests: 0,
            shared_fraction: Some(1.0),
            realised_depth: h.summary(),
            depth_buckets: h.buckets(),
        }
    }

    fn empty_sharing() -> SharingReport {
        SharingReport {
            requests: 0,
            sharing_requests: 0,
            unshared_requests: 0,
            shared_fraction: None,
            realised_depth: Hist::new().summary(),
            depth_buckets: vec![],
        }
    }

    #[test]
    fn turns_are_invocations_per_session() {
        let mut s = SessionShapes::new();
        // Three sessions with 1, 4 and 4 turns.
        for (session, turns) in [(1u32, 1u32), (2, 4), (3, 4)] {
            for t in 0..turns {
                s.observe(session, t, 10 + u64::from(t), 0, None);
            }
        }
        let f = s.finish(&empty_sharing());
        assert_eq!(f.sessions, 3);
        // The median session has four turns.
        assert_eq!(quantile_of(&f.turns, 0.5), 4.0);
    }

    #[test]
    fn private_depth_is_turn_one_depth_less_its_shared_prefix() {
        // The contract's definition, and the reason `shared_len` is passed in rather
        // than recomputed.
        let mut s = SessionShapes::new();
        for session in 0..50u32 {
            s.observe(session, 0, 30, 18, None);
        }
        let f = s.finish(&empty_sharing());
        assert_eq!(quantile_of(&f.private_depth, 0.5), 12.0);
    }

    #[test]
    fn growth_is_the_increment_between_turns_not_the_depth() {
        let mut s = SessionShapes::new();
        for session in 0..40u32 {
            s.observe(session, 0, 20, 0, None);
            s.observe(session, 1, 28, 0, None);
            s.observe(session, 2, 36, 0, None);
        }
        let f = s.finish(&empty_sharing());
        assert_eq!(quantile_of(&growth_dist(&f.growth_per_turn), 0.5), 8.0);
        // Turn 1 contributes to private_depth, not to growth.
        assert_eq!(quantile_of(&f.private_depth, 0.5), 20.0);
    }

    #[test]
    fn think_time_comes_out_in_seconds() {
        // `SessionParams::think_time_s` is seconds, so a fit that emitted
        // milliseconds would rescale every generated plan by a thousand.
        let mut s = SessionShapes::new();
        for session in 0..30u32 {
            s.observe(session, 0, 10, 0, Some(100.0));
            s.observe(session, 1, 12, 0, Some(102.5));
        }
        let f = s.finish(&empty_sharing());
        let v = quantile_of(&f.think_time, 0.5);
        // Toleranced at the histogram's own resolution, not tighter. 2500 ms lands in
        // the log bucket [2496, 2559], and the emitted value is that bucket's midpoint
        // — so the answer carries up to half a bucket, about 1.6%, and an assertion
        // inside that would be pinning which end of a bucket the estimator picks
        // rather than the unit this test is about.
        assert!(
            (v - 2.5).abs() < 2.5 * 0.02,
            "think time came out {v}s, which is not 2.5s to within a bucket"
        );
    }

    #[test]
    fn a_trace_without_timestamps_leaves_think_time_unset_and_says_so() {
        // FR-055: unset, never defaulted.
        let mut s = SessionShapes::new();
        for session in 0..10u32 {
            s.observe(session, 0, 10, 0, None);
            s.observe(session, 1, 12, 0, None);
        }
        let f = s.finish(&empty_sharing());
        assert!(f.think_time.is_none());
        assert!(f
            .caveats()
            .iter()
            .any(|c| c.contains("think_time is unset")));
        assert!(f.growth_per_turn.is_some(), "growth needs no timestamps");
    }

    #[test]
    fn growth_is_differenced_along_the_turn_chain_not_the_arrival_order() {
        // THE regression test for the defect that made every real trace unfittable.
        //
        // One session whose paths grow 10 -> 20 -> 30 -> 40 along its turn chain, but
        // whose requests *arrive* 0, 2, 1, 3 — which is what a real trace's timestamps
        // do. Differencing arrivals gives +20, then a clamped 0 where the path appears
        // to shrink, then +20 again: a total of 40 against a true span of 30, and a
        // median growth of 20 instead of 10. On three real agentic traces that same
        // error inflated the growth total 2.08x-2.28x, ran the synthetic output 1.6x
        // longer than its source, and put `request_length` at 0.18 against a 0.02
        // tolerance.
        let mut s = SessionShapes::new();
        s.observe(7, 0, 10, 0, None);
        s.observe(7, 2, 30, 0, None);
        s.observe(7, 1, 20, 0, None);
        s.observe(7, 3, 40, 0, None);
        let f = s.finish(&empty_sharing());
        assert_eq!(
            quantile_of(&growth_dist(&f.growth_per_turn), 0.5),
            10.0,
            "growth was differenced in arrival order, which doubles it here"
        );
        // The chain is strict once it is in chain order, so the genuine violation
        // count must be zero — this trace is perfectly expressible.
        assert_eq!(
            f.non_monotone_steps, 0,
            "a chain that only ARRIVED out of order is still a strict chain"
        );
        // The arrival disorder is still reported, because it is what makes think_time
        // and growth answer on different axes.
        assert_eq!(f.out_of_order_turns, 1);
        assert!(f
            .caveats()
            .iter()
            .any(|c| c.contains("disagrees with their turn indices")));
        // And no caveat may report the *chain* as something the model cannot express,
        // which is what the old caveat did and what sent the first investigation down
        // the wrong path. Arrival disorder now affects no fitted parameter, so there is
        // nothing for the model to be short of here.
        //
        // Scoped to the chain rather than asserting no limitation at all: this fixture
        // passes no timestamps, so `think_time` is legitimately unset and reports its
        // own limitation. An earlier, broader version of this assertion failed on that
        // and would have been satisfied by weakening the wrong message.
        assert!(
            !f.caveats()
                .iter()
                .any(|c| c.contains("MODEL LIMITATION") && c.contains("turn chain")),
            "arrival disorder is not a model-expressiveness problem: {:?}",
            f.caveats()
        );
    }

    #[test]
    fn the_path_budget_accounts_for_the_mean_exactly() {
        // Two identities, and the whole diagnosis of the residual request-length gap
        // rests on the second one.
        //
        // 1. mean request length == turn-weighted turn-1 depth + accumulated growth,
        //    both per request. If this drifts, the budget stops adding up and any
        //    conclusion drawn from it is arithmetic on sand.
        // 2. accumulated growth == SUM over i of (T - i) * g_i. This is why an i.i.d.
        //    per-turn draw is not neutral: increment i is inherited by every later
        //    turn, so it enters with weight (T - i), and the mean increment under that
        //    weighting is what the draw has to match — not the pooled mean.
        // Session 7: depths 10, 20, 40 (increments 10, 20). Session 8: depths 100, 130.
        let mut s = SessionShapes::new();
        for (sess, turn, p) in [
            (7u32, 0u32, 10u64),
            (7, 1, 20),
            (7, 2, 40),
            (8, 0, 100),
            (8, 1, 130),
        ] {
            s.observe(sess, turn, p, 0, None);
        }
        let b = s.path_budget();

        assert_eq!(b.requests, 5);
        // 3 turns x 10 + 2 turns x 100 = 230.
        assert_eq!(b.weighted_turn_one_depth, 230);
        // Session 7: 0 + 10 + 30 = 40. Session 8: 0 + 30 = 30.
        assert_eq!(b.accumulated_growth, 70);
        // SUM (T - i) g_i, written as the formula rather than as its total, since the
        // weight is the whole point: session 7 is 2x10 + 1x20 = 40, session 8 is 1x30.
        let weighted = |turns: u64, increments: &[(u64, u64)]| -> u64 {
            increments.iter().map(|(i, g)| (turns - i) * g).sum()
        };
        assert_eq!(
            b.accumulated_growth,
            weighted(3, &[(1, 10), (2, 20)]) + weighted(2, &[(1, 30)]),
            "the weighted-sum identity the i.i.d. comparison depends on"
        );
        // T(T-1)/2: 3 + 1 = 4 increments' worth of weight.
        assert_eq!(b.accumulated_steps, 4);

        // Identity 1, to a rounding tolerance: 230/5 + 70/5 == (10+20+40+100+130)/5.
        let mean = (10 + 20 + 40 + 100 + 130) as f64 / 5.0;
        let from_terms = b.weighted_turn_one_depth as f64 / 5.0 + b.accumulated_per_request();
        assert!(
            (from_terms - mean).abs() < 1e-9,
            "budget does not add up: {from_terms} against {mean}"
        );

        // And the i.i.d. counterfactual overstates here, because the long session's
        // increments are smaller than the pooled mean of (10, 20, 30) = 20.
        assert_eq!(b.accumulated_per_request(), 14.0);
        let iid = b.accumulated_per_request_iid().expect("growth measured");
        assert!(
            (iid - 20.0 * 4.0 / 5.0).abs() < 1e-9,
            "i.i.d. accumulation is pooled mean x steps / requests, got {iid}"
        );
        assert!(b.iid_inflation().expect("both") > 1.0);
    }

    #[test]
    fn a_path_that_shrinks_along_the_chain_is_the_real_violation() {
        // Distinguished from mere arrival disorder above: here turn 1's path is
        // genuinely shorter than turn 0's, which FR-014a's grow-only path cannot
        // express. FR-054a makes that a limitation of the model, so the caveat must be
        // classified as one — the trace is a perfectly ordinary shrinking context.
        let mut s = SessionShapes::new();
        s.observe(7, 0, 20, 0, None);
        s.observe(7, 1, 18, 0, None);
        let f = s.finish(&empty_sharing());
        assert_eq!(f.non_monotone_steps, 1);
        assert_eq!(f.out_of_order_turns, 0, "these arrived in chain order");
        assert!(f.caveats().iter().any(|c| c.contains("MODEL LIMITATION")));
        assert!(
            f.caveats()
                .iter()
                .any(|c| c.contains("The trace is not at fault")),
            "FR-054a: a trace we cannot express is a limit of the model: {:?}",
            f.caveats()
        );
        // A shrinking path floors at zero growth rather than producing a negative.
        assert_eq!(quantile_of(&growth_dist(&f.growth_per_turn), 0.5), 0.0);
    }

    #[test]
    fn private_depth_comes_from_the_chains_first_turn_not_the_first_arrival() {
        // `private_depth` is turn one's path less its own shared prefix. When the
        // orders disagree the first *arrival* is a mid-conversation request, and using
        // it put a deeper path where turn one's belonged — inflating private_depth and
        // so every generated path built from it.
        let mut s = SessionShapes::new();
        s.observe(3, 5, 500, 100, None); // arrives first, but is turn 5
        s.observe(3, 0, 60, 10, None); // the chain's actual first turn
        let f = s.finish(&empty_sharing());
        assert_eq!(
            quantile_of(&f.private_depth, 0.5),
            50.0,
            "private_depth must be 60 - 10 from turn 0, not 500 - 100 from turn 5"
        );
    }

    #[test]
    fn a_prefix_longer_than_its_path_is_flagged_as_impossible() {
        let mut s = SessionShapes::new();
        s.observe(1, 0, 5, 9, None);
        let f = s.finish(&empty_sharing());
        assert_eq!(f.prefix_longer_than_path, 1);
        assert!(f.caveats().iter().any(|c| c.contains("impossible")));
    }

    /// Two populations whose per-session and all-requests sharing disagree.
    ///
    /// Short sessions share shallowly, long ones share deeply — the correlation real
    /// traces have — so the all-requests histogram is dominated by the long sessions'
    /// deeper sharing while the per-session one is not. Returns the accumulator and the
    /// matching all-requests `SharingReport`.
    fn correlated_sharing() -> (SessionShapes, SharingReport) {
        let mut s = SessionShapes::new();
        let mut all = Vec::new();
        for i in 0..30u32 {
            s.observe(i, 0, 50, 10, None);
            all.push(10);
        }
        for i in 0..30u32 {
            for t in 0..9u32 {
                s.observe(1000 + i, t, 60 + u64::from(t) * 5, 30, None);
                all.push(30);
            }
        }
        (s, sharing_with(&all))
    }

    #[test]
    fn shared_depth_is_fitted_in_the_space_the_generator_draws_it_in() {
        // FR-054h. `shared_depth` is drawn ONCE PER SESSION, so the parameter is the
        // per-session distribution — turn 1's realised prefix. The all-requests histogram
        // is the same measurement turn-weighted, and the generator applies that weighting
        // itself by replaying the drawn prefix on every turn; fitting from the weighted
        // form applies it twice.
        let (mut s, sharing) = correlated_sharing();
        let f = s.finish(&sharing);
        let d = f.shared_depth.expect("fitted");
        // Per session the population is 30 at 10 and 30 at 30, so the median is 10.
        assert_eq!(d.quantile(0.5), Some(10.0));
        // Over all 300 requests it would be 30 — 270 of them belong to the long
        // sessions. That is the value this must NOT come back with.
        assert_eq!(
            sharing.realised_depth.p50,
            Some(30),
            "fixture must actually disagree, or this test proves nothing"
        );
    }

    #[test]
    fn what_shared_depth_draws_equals_what_private_depth_subtracts() {
        // The identity term 1 exists to enforce: FR-014a builds
        // `shared_depth + private_depth + Σ growth`, while `private_depth` is measured as
        // `turn-1 depth − turn-1's own prefix`. If the drawn prefix and the subtracted one
        // have different means, the difference lands on every generated path as a
        // constant — +33.3 and +74.6 blocks per request on two real traces.
        let (mut s, sharing) = correlated_sharing();
        let budget = s.path_budget();
        let f = s.finish(&sharing);
        let drawn = f
            .shared_depth
            .as_ref()
            .and_then(|d| d.mean())
            .expect("a mean");
        assert!(
            (drawn - budget.turn_one_shared).abs() < 1e-9,
            "drawn {drawn} against subtracted {}",
            budget.turn_one_shared
        );
    }

    #[test]
    fn the_sharing_gap_is_split_into_the_expressible_part_and_the_rest() {
        // Two causes, one symptom, different fixes — so the budget has to separate them.
        // In this fixture sharing is constant within each session and only the
        // length-to-sharing correlation is present, so the turn-weighted turn-1 figure
        // must account for the whole gap to the all-requests mean and leave nothing over.
        let (mut s, sharing) = correlated_sharing();
        let budget = s.path_budget();
        let all_mean = sharing.realised_depth.mean.expect("a mean");
        let weighted = budget.turn_one_shared_weighted();
        // 30 requests at 10 and 270 at 30.
        let expect = (30.0 * 10.0 + 270.0 * 30.0) / 300.0;
        assert!(
            (all_mean - expect).abs() < 0.5,
            "all-requests mean {all_mean}"
        );
        assert!(
            (weighted - all_mean).abs() < 0.5,
            "sharing is constant within a session here, so the turn-weighted turn-1 mean \
             {weighted} should account for the all-requests mean {all_mean} entirely"
        );
        // And the correlation half is the whole gap from the unweighted figure.
        assert!(weighted - budget.turn_one_shared > 7.0);
    }

    #[test]
    fn requests_that_shared_nothing_are_reported_rather_than_folded_in() {
        let mut h = Hist::new();
        h.add(4);
        h.seal();
        let sharing = SharingReport {
            requests: 10,
            sharing_requests: 1,
            unshared_requests: 9,
            shared_fraction: Some(0.1),
            realised_depth: h.summary(),
            depth_buckets: h.buckets(),
        };
        let f = SessionShapes::new().finish(&sharing);
        assert_eq!(f.unshared_requests, 9);
        assert!(f.caveats().iter().any(|c| c.contains("shared nothing")));
    }

    #[test]
    fn private_depth_recomputes_against_a_raised_attempted_sharing() {
        // The prerequisite for any iteration on the realised-versus-attempted gap.
        // Turn-1 depth 30 with a realised prefix of 18 gives private_depth 12; if the
        // attempt is raised to 24 the private part must fall to 6, or the generated
        // path — attempted + private + growth — would run 6 blocks longer than the
        // trace's and fix the sharing statistic by breaking request length.
        let mut s = SessionShapes::new();
        for session in 0..50u32 {
            s.observe(session, 0, 30, 18, None);
        }
        assert_eq!(quantile_of(&s.private_depth_at(1.0), 0.5), 12.0);
        let raised = 24.0 / 18.0;
        assert_eq!(quantile_of(&s.private_depth_at(raised), 0.5), 6.0);
        // And the sum is invariant, which is the property that keeps path length fixed
        // while sharing moves.
        assert_eq!(quantile_of(&s.turn_one_depth(), 0.5), 30.0);
    }

    #[test]
    fn recomputing_at_scale_one_is_the_plain_measurement() {
        // Otherwise an iteration's first step would already have moved the model.
        let mut s = SessionShapes::new();
        for session in 0..30u32 {
            s.observe(
                session,
                0,
                20 + u64::from(session % 5),
                3 + u64::from(session % 3),
                None,
            );
        }
        let f = s.finish(&sharing_with(&[3]));
        assert_eq!(
            f.private_depth.as_ref().and_then(|d| d.quantile(0.5)),
            s.private_depth_at(1.0).and_then(|d| d.quantile(0.5))
        );
    }

    #[test]
    fn an_attempt_deeper_than_the_path_clamps_to_no_private_part() {
        // A negative private depth is not expressible, and the clamp is what keeps an
        // over-raised iteration from emitting one.
        let mut s = SessionShapes::new();
        for session in 0..20u32 {
            s.observe(session, 0, 10, 8, None);
        }
        assert_eq!(quantile_of(&s.private_depth_at(4.0), 0.5), 0.0);
    }

    #[test]
    fn the_pairs_survive_finishing_so_an_iteration_can_keep_asking() {
        // `finish` borrows rather than consumes, which is what lets a caller fit,
        // generate, measure and come back for another private_depth.
        let mut s = SessionShapes::new();
        for session in 0..25u32 {
            s.observe(session, 0, 40, 10, None);
        }
        let first = s.finish(&sharing_with(&[10]));
        let second = s.finish(&sharing_with(&[10]));
        assert_eq!(first.sessions, second.sessions);
        assert_eq!(
            first.private_depth.as_ref().and_then(|d| d.quantile(0.5)),
            second.private_depth.as_ref().and_then(|d| d.quantile(0.5))
        );
        assert!(s.private_depth_at(2.0).is_some());
    }

    #[test]
    fn an_empty_fit_leaves_every_parameter_unset() {
        let f = SessionShapes::new().finish(&empty_sharing());
        assert!(f.turns.is_none());
        assert!(f.private_depth.is_none());
        assert!(f.growth_per_turn.is_none());
        assert!(f.think_time.is_none());
        assert!(f.shared_depth.is_none());
    }

    #[test]
    fn an_emitted_empirical_distribution_round_trips_through_yaml() {
        // What `fit` writes has to be what `plan` can read.
        let mut s = SessionShapes::new();
        for session in 0..60u32 {
            s.observe(session, 0, 20 + u64::from(session % 7), 3, None);
        }
        let f = s.finish(&empty_sharing());
        let y = serde_yaml::to_string(&f.private_depth).expect("serialise");
        let back: Option<Dist> = serde_yaml::from_str(&y).expect("deserialise");
        assert_eq!(
            back.as_ref().and_then(|d| d.quantile(0.5)),
            f.private_depth.as_ref().and_then(|d| d.quantile(0.5))
        );
    }

    /// Observe `sessions` sessions of `turns` turns each, every turn adding `growth`.
    ///
    /// Session ids are offset by `base` so several populations can share one
    /// accumulator without colliding.
    fn population(s: &mut SessionShapes, base: u32, sessions: u32, turns: u32, growth: u64) {
        for i in 0..sessions {
            for t in 0..turns {
                s.observe(base + i, t, 100 + u64::from(t) * growth, 0, None);
            }
        }
    }

    #[test]
    fn growth_is_banded_by_session_length_and_thin_bands_are_merged() {
        // FR-054f. Two populations that differ in length *and* in growth rate, which is
        // the situation a single pooled distribution cannot represent, plus a third that
        // is too thin to stand as its own band.
        let mut s = SessionShapes::new();
        population(&mut s, 0, 40, 3, 10); // short and slow
        population(&mut s, 1000, 40, 40, 100); // long and fast
        population(&mut s, 2000, 3, 100, 7); // three sessions: not a band

        let g = s.fit_growth().expect("a fit");
        let b = match &g {
            Growth::Banded(b) => b,
            Growth::Uniform(_) => panic!("two populations must not collapse to one band"),
        };
        // Exactly two bands: the 100-turn rung carries three sessions, below
        // MIN_BAND_SESSIONS, so it folds into its neighbour rather than being emitted as
        // a band fitted from three samples.
        assert_eq!(
            b.by_turns.iter().map(|x| x.from_turns).collect::<Vec<_>>(),
            vec![1, 4],
            "thin rungs should merge, and the first band must start at 1"
        );
        // A session's *length* selects which distribution it draws from.
        assert!(
            (g.at(3).mean().expect("mean") - 10.0).abs() < 0.2,
            "a 3-turn session drew from {:?}",
            g.at(3)
        );
        // The thin rung's samples are *absorbed* by the band below it, not dropped and
        // not emitted separately — so the upper band's mean is the exact pooled mean of
        // both sets of increments. Written as the arithmetic rather than as its value,
        // because what this pins is that the merge loses nothing: 40 sessions of 40 turns
        // contribute 39 increments of 100 each, and 3 sessions of 100 turns contribute 99
        // increments of 7 each.
        let expect = (40.0 * 39.0 * 100.0 + 3.0 * 99.0 * 7.0) / (40.0 * 39.0 + 3.0 * 99.0);
        let long = g.at(40).mean().expect("mean");
        // Toleranced at the histogram's own resolution rather than tighter: an increment
        // of 100 lands in the log bucket [100, 101] and the fit emits that bucket's
        // midpoint, so the answer carries up to half a bucket -- about 0.5% here. An
        // assertion inside that would be pinning which end of a bucket the estimator
        // picks, which is not what this test is about.
        assert!(
            (long - expect).abs() < expect * 0.01,
            "the upper band came out {long}, not the {expect} its own samples average to"
        );
        // Which is also to say the 100-turn sessions draw from that band, not their own.
        assert_eq!(g.at(100).mean(), g.at(40).mean());
    }

    #[test]
    fn one_population_emits_the_unbanded_spelling() {
        // A trace whose sessions are all one length has shown no length dependence, so
        // banding it would be a one-row table pretending to a structure it has not
        // demonstrated — and the unbanded spelling is what every pre-FR-054f document
        // already uses.
        let mut s = SessionShapes::new();
        population(&mut s, 0, 40, 3, 8);
        let g = s.fit_growth().expect("a fit");
        assert!(
            matches!(g, Growth::Uniform(_)),
            "one population must emit a bare distribution, got {g:?}"
        );
        assert!((g.at(3).mean().expect("mean") - 8.0).abs() < 0.2);
    }

    #[test]
    fn a_banded_fit_reproduces_the_accumulation_the_pooled_one_overstates() {
        // The measurement FR-054f exists for. A session's accumulated depth is
        // `Σᵢ (T − i)·gᵢ`, so an increment's weight is quadratic in the turn count once
        // summed over the session — and when growth rate varies with length, a draw from
        // the pooled marginal reproduces every marginal correctly and the *total*
        // wrongly. Short-and-fast beside long-and-slow, which is the real traces' shape.
        const POP: [(u32, u64, u64); 2] = [(60, 4, 40), (30, 40, 5)];
        let mut s = SessionShapes::new();
        population(&mut s, 0, 60, 4, 40);
        population(&mut s, 1000, 30, 40, 5);

        let budget = s.path_budget();
        let g = s.fit_growth().expect("a fit");

        // What each scheme predicts for `Σ over sessions (mean growth) × T(T−1)/2`.
        let steps = |turns: u64| turns * turns.saturating_sub(1) / 2;
        let banded: f64 = POP
            .iter()
            .map(|(n, turns, _)| {
                f64::from(*n) * g.at(*turns).mean().expect("a band mean") * steps(*turns) as f64
            })
            .sum();
        let pooled = budget.growth.expect("a pooled mean") * budget.accumulated_steps as f64;
        let truth = budget.accumulated_growth as f64;

        // The pooled marginal is *correct as a marginal* — it is the mean increment —
        // and still overstates the accumulation by three quarters, because the long
        // sessions carry most of the quadratic weight and grow the most slowly.
        let inflation = pooled / truth;
        assert!(
            inflation > 1.5,
            "the pooled draw should overstate here; it came out {inflation:.3}x"
        );
        // Banding recovers it, because each session's length now selects its rate.
        assert!(
            (banded / truth - 1.0).abs() < 0.02,
            "banded predicted {banded} against the trace's {truth} ({:.3}x)",
            banded / truth
        );
    }
}
