//! Sessions: the only behavioural unit, and the reason a run has no natural end.
//!
//! A session is born on arrival, binds one root and one node, issues `turns`
//! requests separated by `think_time`, and is **retired** when its last turn
//! completes. Its private keys are dead from that moment. A run of any length
//! therefore comes from continuously retiring and creating sessions, and resident
//! memory is bounded by the live population rather than by run length.
//!
//! **Lifetime and live population are derived, never configured.** Lifetime is
//! the sum of a session's think times; the live count follows from Little's law.
//! A field for either would be a third statement of something `turns` and
//! `think_time` already fix, and the three could disagree.

use crate::dist::Dist;
use crate::keys::SessionId;
use crate::rng::Stream;
use crate::schema::{ArrivalModel, MixEntry, Sessions, Workload};

/// The population arithmetic that must be knowable before generating anything.
///
/// Both quantities below are used by checks that run *before* a plan exists — the
/// occupancy floor and the warmup-ramp rejection — so neither may depend on
/// having generated events.
#[derive(Debug, Clone, PartialEq)]
pub struct Population {
    /// Sessions begun per second.
    pub session_rate: f64,
    /// Mean turns per session.
    pub mean_turns: f64,
    /// Mean session lifetime in seconds: `(turns - 1) × think_time`.
    pub mean_lifetime_s: f64,
    /// Sessions born but not yet retired, by Little's law.
    ///
    /// Not the same quantity as [`Population::sessions_per_window`]: occupancy
    /// needs how many sessions have *walked* the trunk in a window, memory needs
    /// how many are walking it *now*.
    pub live_sessions: f64,
}

impl Population {
    /// Derive the population from the workload and an arrival rate.
    ///
    /// `request_rate` is requests per second under `open_loop`. Under
    /// `closed_loop` the live count *is* the configured concurrency, because
    /// closed loop supplies no rate — which is legitimate there and would be
    /// over-determination anywhere else.
    pub fn derive(w: &Workload, request_rate: Option<f64>) -> Option<Population> {
        let mean_turns = w.sessions.turns.mean()?.max(1.0);
        let mean_think = w.sessions.think_time.mean().unwrap_or(0.0);
        let mean_lifetime_s = (mean_turns - 1.0).max(0.0) * mean_think;
        match w.arrival.model {
            ArrivalModel::OpenLoop => {
                let rr = request_rate?;
                let session_rate = rr / mean_turns;
                Some(Population {
                    session_rate,
                    mean_turns,
                    mean_lifetime_s,
                    live_sessions: session_rate * mean_lifetime_s,
                })
            }
            ArrivalModel::ClosedLoop => {
                let live = f64::from(w.arrival.concurrency?);
                // Under closed loop the population is the constraint and the
                // rate is the consequence, not the other way round.
                let session_rate = if mean_lifetime_s > 0.0 {
                    live / mean_lifetime_s
                } else {
                    0.0
                };
                Some(Population {
                    session_rate,
                    mean_turns,
                    mean_lifetime_s,
                    live_sessions: live,
                })
            }
        }
    }

    /// Sessions begun within a window of `window_requests` requests.
    ///
    /// What the occupancy floor needs. Independent of rate, because the window is
    /// a request count — which is why a fitted model does not lose sharing
    /// fidelity when replayed faster or slower.
    pub fn sessions_per_window(&self, window_requests: u64) -> f64 {
        window_requests as f64 / self.mean_turns
    }

    /// Time for the live population to fill, from an empty start.
    ///
    /// `run.warmup` must cover this. At t=0 nothing is live, and a measured
    /// window opening sooner sees less concurrency, less occupancy and less
    /// sharing than configured — all of which read as properties of the workload
    /// rather than of the clock.
    pub fn ramp_up_s(&self) -> f64 {
        self.mean_lifetime_s
    }
}

/// Why a warmup was refused.
#[derive(Debug, PartialEq)]
pub struct RampTooShort {
    /// What was configured.
    pub warmup_s: f64,
    /// What the session model implies.
    pub required_s: f64,
}

impl std::fmt::Display for RampTooShort {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "run.warmup is {:.1}s but the session population needs ~{:.1}s to reach steady state; \
             a window opening sooner measures the ramp rather than the workload",
            self.warmup_s, self.required_s
        )
    }
}

/// Check `warmup` against the population ramp.
///
/// A rejection rather than a warning: the resulting numbers are wrong rather than
/// merely noisy.
pub fn check_warmup(pop: &Population, warmup_s: f64) -> Result<(), RampTooShort> {
    let required = pop.ramp_up_s();
    if warmup_s + 1e-9 < required {
        Err(RampTooShort {
            warmup_s,
            required_s: required,
        })
    } else {
        Ok(())
    }
}

/// Gaps between session arrivals, with `burstiness` as an index of dispersion.
///
/// The neutral value is **1.0**, which is exponential inter-arrivals — a Poisson
/// process. Above that the process is over-dispersed: the same mean rate arriving
/// in clumps. This is deliberately the *index of dispersion for counts* rather
/// than some scale-free "burstiness knob", because IDC has a value that means
/// "no burstiness at all", so a document that omits it and a document that states
/// the neutral value describe the same arrival process.
///
/// Realised by a balanced-means two-phase hyperexponential, whose squared
/// coefficient of variation of inter-arrivals *is* the asymptotic index of
/// dispersion for a renewal process. So the configured number is the measured
/// one, which [`Interarrival`]'s own test asserts rather than assumes.
#[derive(Debug, Clone, PartialEq)]
pub struct Interarrival {
    mean_s: f64,
    /// Probability of the fast phase; `None` for the neutral exponential case.
    phase: Option<(f64, f64, f64)>,
}

impl Interarrival {
    /// Gaps with mean `1/rate` seconds and the given index of dispersion.
    ///
    /// A `burstiness` below 1.0 asks for an *under*-dispersed process, which this
    /// generator does not model; it is treated as the neutral 1.0 and
    /// `validate` warns, rather than being silently reinterpreted as something
    /// the document did not ask for.
    pub fn new(rate_per_s: f64, burstiness: f64) -> Interarrival {
        let mean_s = if rate_per_s > 0.0 {
            1.0 / rate_per_s
        } else {
            0.0
        };
        let b = burstiness;
        // Bound rather than negated inline, so that a NaN burstiness lands on the
        // neutral case instead of on an unspecified branch.
        let over_dispersed = b > 1.0 + 1e-12;
        if !over_dispersed {
            return Interarrival {
                mean_s,
                phase: None,
            };
        }
        // Balanced means: p1 = ½(1 + √((b−1)/(b+1))), phase means chosen so that
        // the mixture has mean `mean_s` and squared CoV exactly b.
        let p1 = 0.5 * (1.0 + ((b - 1.0) / (b + 1.0)).sqrt());
        let p2 = 1.0 - p1;
        let m1 = mean_s / (2.0 * p1);
        let m2 = if p2 > 0.0 { mean_s / (2.0 * p2) } else { m1 };
        Interarrival {
            mean_s,
            phase: Some((p1, m1, m2)),
        }
    }

    /// The mean gap in seconds.
    pub fn mean_s(&self) -> f64 {
        self.mean_s
    }

    /// Draw one gap, in seconds.
    pub fn next_s(&self, st: &mut Stream) -> f64 {
        let exp = |m: f64, st: &mut Stream| -> f64 { -m * (1.0 - st.next_f64()).ln() };
        match self.phase {
            None => exp(self.mean_s, st),
            Some((p1, m1, m2)) => {
                if st.next_f64() < p1 {
                    exp(m1, st)
                } else {
                    exp(m2, st)
                }
            }
        }
    }

    /// Draw one gap, in nanoseconds.
    pub fn next_ns(&self, st: &mut Stream) -> u64 {
        (self.next_s(st) * 1e9).max(0.0) as u64
    }
}

/// The parameters one session actually draws from, after mixture selection.
#[derive(Debug, Clone)]
pub struct SessionParams {
    /// Which mixture entry this came from, for per-class reporting.
    pub mix_index: u8,
    /// Requests in this session.
    pub turns: u16,
    /// Turn-1 private path depth.
    pub private_depth: u32,
    /// Gap between turns, in seconds.
    pub think_time_s: f64,
    /// Per-turn path growth.
    pub growth_per_turn: Dist,
    /// Ceiling on this session's path depth, in blocks; `None` is unbounded.
    ///
    /// Carried on the session rather than read from the document at each turn so that
    /// [`depth_at_turn`] takes every term of FR-014a's formula as an argument, which is
    /// what keeps the formula stated exactly once.
    pub max_depth: Option<u32>,
}

/// Resolve the mixture: pick an entry, then draw the session's parameters.
///
/// A mixture entry is a *parameter set*, not a behavioural mode — one sampling
/// path and one fitting routine, which is why `conversation`/`one_shot`/`scan`
/// are presets rather than schema.
pub fn draw_params(w: &Workload, st: &mut Stream) -> SessionParams {
    let (mix_index, entry) = pick_mix(&w.mix, st);
    let s: &Sessions = &w.sessions;
    let turns_d = entry
        .and_then(|e| e.turns.clone())
        .unwrap_or(s.turns.clone());
    let priv_d = entry
        .and_then(|e| e.private_depth.clone())
        .unwrap_or(s.private_depth.clone());
    let think_d = entry
        .and_then(|e| e.think_time.clone())
        .unwrap_or(s.think_time.clone());
    let growth_d = entry
        .and_then(|e| e.growth_per_turn.clone())
        .unwrap_or(s.growth_per_turn.clone());
    let turns = turns_d.sample_u64(st).clamp(1, u64::from(u16::MAX)) as u16;
    SessionParams {
        mix_index,
        turns,
        private_depth: priv_d.sample_u64(st).min(u64::from(u32::MAX)) as u32,
        think_time_s: think_d.sample(st).max(0.0),
        // Banding resolved HERE, once, where the turn count is known — so
        // `depth_at_turn` and the incremental advance both still see one `Dist` and
        // FR-014a's formula does not learn about bands. Drawn per turn from it as
        // before; what a session's length selects is the distribution, not the value.
        growth_per_turn: growth_d.at(u64::from(turns)).clone(),
        // Drawn per session: a single ceiling piles every long conversation onto one
        // depth, which is visible as a spike in `request_length` (see
        // `Sessions::max_depth`). Not per-arm, though — a context window is a property
        // of the model being served rather than of one arm of a workload.
        max_depth: s
            .max_depth
            .as_ref()
            .map(|d| d.sample_u64(st).min(u64::from(u32::MAX)) as u32),
    }
}

/// Choose a mixture entry by weight; weights are normalised, not required to sum.
fn pick_mix<'a>(mix: &'a [MixEntry], st: &mut Stream) -> (u8, Option<&'a MixEntry>) {
    if mix.is_empty() {
        return (0, None);
    }
    let total: f64 = mix.iter().map(|m| m.weight.max(0.0)).sum();
    if total <= 0.0 {
        return (0, Some(&mix[0]));
    }
    let mut r = st.next_f64() * total;
    for (i, m) in mix.iter().enumerate() {
        r -= m.weight.max(0.0);
        if r <= 0.0 {
            return (i.min(u8::MAX as usize) as u8, Some(m));
        }
    }
    let last = mix.len() - 1;
    (last.min(u8::MAX as usize) as u8, Some(&mix[last]))
}

/// Path depth at turn `n`, stated exactly once.
///
/// `shared_depth + private_depth + Σ growth_per_turn(i)` for i in 2..=n. Turn n's
/// path is necessarily a strict prefix of turn n+1's, which the rolling-hash key
/// requires: a changed prefix would rehash every block below it.
///
/// A **length in blocks**, not the ordinal of the last one — the worked example's
/// "~56 blocks" is this quantity. So a path of depth `n` occupies ordinals
/// `0..n`, and `depth_at_turn(4, 30, .., 1, ..)` is 34 blocks of which 4 are
/// shared. The floor of 1 is FR-009's zero-length-request clamp: a request for
/// nothing is not a request, so a path that draws to zero becomes the root alone.
///
/// # The ceiling
///
/// `max_depth` is the context window (`Sessions::max_depth`): growth stops once the
/// path reaches it, which is why a long conversation grows more slowly per turn than a
/// short one. Two properties of how it is applied matter, and both are deliberate:
///
/// - **Turn 1 is never truncated.** The ceiling is raised to turn-1's own depth where
///   that is already deeper, so `shared_depth + private_depth` is always realised in
///   full. Clipping it instead would shorten a *shared prefix*, which changes what
///   sharing the corpus realises and would make the cap interfere with `roots.count`
///   and the FR-009f occupancy floor — a ceiling on conversation length would be
///   quietly editing the trunk.
/// - **The path never shrinks**, so FR-014a's strict-extension guarantee holds: turn
///   n's path stays a prefix of turn n+1's, which the rolling-hash key requires.
///
/// Growth is still *drawn* per turn even once the ceiling is reached, so the stream
/// position is unchanged — a session that hits its cap draws the same numbers it would
/// have and simply does not use them. That keeps a capped run's keys comparable with an
/// uncapped one's rather than shifting every subsequent draw.
pub fn depth_at_turn(
    shared_depth: u32,
    private_depth: u32,
    growth: &Dist,
    turn: u16,
    max_depth: Option<u32>,
    st: &mut Stream,
) -> u32 {
    let base = shared_depth.saturating_add(private_depth);
    // Never below turn 1's own depth: the cap bounds how far a conversation grows, not
    // how much prefix it starts with.
    let ceiling = max_depth.map(|c| c.max(base)).unwrap_or(u32::MAX);
    let mut d = base;
    for _ in 2..=turn {
        let g = growth.sample_u64(st).min(u64::from(u32::MAX)) as u32;
        d = d.saturating_add(g).min(ceiling);
    }
    d.max(1)
}

/// A live session's state while it is being generated.
#[derive(Debug, Clone)]
pub struct Session {
    /// Identity, stored in every event because turns interleave.
    pub id: SessionId,
    /// Which node asks. Sticky by default: a session's KV lives where it was
    /// computed.
    pub node: u16,
    /// Which tree it binds to, drawn once at birth and then fixed.
    pub root_index: u32,
    /// Drawn parameters.
    pub params: SessionParams,
    /// 1-based; `turn > params.turns` means retired.
    pub turn: u16,
    /// When the next turn is due, in nanoseconds.
    pub next_t_ns: u64,
}

impl Session {
    /// Whether every turn has been issued.
    pub fn is_retired(&self) -> bool {
        self.turn > self.params.turns
    }

    /// Lifetime in nanoseconds: derived from turns and think time, never a field.
    pub fn lifetime_ns(&self) -> u64 {
        let gaps = u64::from(self.params.turns.saturating_sub(1));
        (self.params.think_time_s * 1e9) as u64 * gaps
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dist::Shape;
    use crate::schema::Arrival;

    fn workload(turns_mean: f64, think: f64) -> Workload {
        Workload {
            arrival: Arrival {
                model: ArrivalModel::OpenLoop,
                rate: Some("4000/s".into()),
                burstiness: Some(1.8),
                concurrency: None,
            },
            sessions: Sessions {
                max_depth: None,
                turns: Dist::Shaped(Shape::Geometric { mean: turns_mean }),
                think_time: Dist::Scalar(think),
                private_depth: Dist::Scalar(8.0),
                growth_per_turn: crate::schema::Growth::Uniform(Dist::Scalar(6.0)),
                spawn: None,
            },
            mix: vec![],
            drift: None,
        }
    }

    #[test]
    fn little_s_law_gives_the_worked_examples_population() {
        // 4000/s over mean 6 turns is ~667 sessions/s; lifetime is 5 gaps of 3s
        // = 15s; so ~10 000 live. The figure the spec quotes, derived rather
        // than configured.
        let p = Population::derive(&workload(6.0, 3.0), Some(4000.0)).unwrap();
        assert!((p.session_rate - 666.67).abs() < 1.0, "{}", p.session_rate);
        assert!((p.mean_lifetime_s - 15.0).abs() < 1e-9);
        assert!(
            (p.live_sessions - 10_000.0).abs() < 20.0,
            "{}",
            p.live_sessions
        );
    }

    #[test]
    fn closed_loop_takes_the_population_as_given() {
        // Legitimate there because closed loop supplies no rate; stating it
        // anywhere else would be over-determination.
        let mut w = workload(6.0, 3.0);
        w.arrival.model = ArrivalModel::ClosedLoop;
        w.arrival.rate = None;
        w.arrival.concurrency = Some(256);
        let p = Population::derive(&w, None).unwrap();
        assert_eq!(p.live_sessions, 256.0);
    }

    #[test]
    fn sessions_per_window_is_independent_of_rate() {
        // Because the window is a request count. This is what stops a fitted
        // model losing sharing fidelity when replayed faster or slower.
        let slow = Population::derive(&workload(6.0, 3.0), Some(1000.0)).unwrap();
        let fast = Population::derive(&workload(6.0, 3.0), Some(9000.0)).unwrap();
        assert_eq!(
            slow.sessions_per_window(240_000),
            fast.sessions_per_window(240_000)
        );
    }

    #[test]
    fn live_population_and_sessions_per_window_are_different_quantities() {
        let p = Population::derive(&workload(6.0, 3.0), Some(4000.0)).unwrap();
        assert_ne!(p.live_sessions, p.sessions_per_window(240_000));
    }

    #[test]
    fn the_worked_example_warmup_covers_its_ramp() {
        // 15s ramp inside a 20s warmup -- which the spec notes survived on luck.
        let p = Population::derive(&workload(6.0, 3.0), Some(4000.0)).unwrap();
        assert!(check_warmup(&p, 20.0).is_ok());
    }

    #[test]
    fn a_long_session_model_needs_a_far_longer_warmup() {
        // turns geometric(50) with think 30s implies a ~24 minute ramp, against
        // which a 20s warmup measures pure transient. Rejected, not warned.
        let p = Population::derive(&workload(50.0, 30.0), Some(4000.0)).unwrap();
        let e = check_warmup(&p, 20.0).unwrap_err();
        assert!(e.required_s > 1400.0, "ramp was {}", e.required_s);
        assert!(format!("{e}").contains("measures the ramp"));
    }

    #[test]
    fn a_one_shot_population_has_no_ramp() {
        // turns == 1 means no think gaps, so nothing accumulates.
        let p = Population::derive(&workload(1.0, 3.0), Some(4000.0)).unwrap();
        assert_eq!(p.mean_lifetime_s, 0.0);
        assert!(check_warmup(&p, 0.0).is_ok());
    }

    #[test]
    fn depth_grows_with_turn_and_each_path_extends_the_last() {
        // FR-014a, and the strict-prefix property the rolling hash requires.
        let g = Dist::Scalar(6.0);
        let mut st = Stream::new(1, 1);
        let d1 = depth_at_turn(18, 8, &g, 1, None, &mut st.clone());
        let d6 = depth_at_turn(18, 8, &g, 6, None, &mut st);
        assert_eq!(d1, 26);
        assert_eq!(d6, 26 + 5 * 6, "six turns adds five growth increments");
        assert!(d6 > d1);
    }

    #[test]
    fn a_depth_ceiling_stops_growth_without_shortening_any_path() {
        // FR-054c. Turn 1 is 26 blocks and growth is 6, so an uncapped session reaches
        // 26 + 5*6 = 56 by turn 6. A ceiling of 40 stops it there.
        let g = Dist::Scalar(6.0);
        let cap = Some(40);
        let st = Stream::new(1, 1);
        assert_eq!(depth_at_turn(18, 8, &g, 1, cap, &mut st.clone()), 26);
        assert_eq!(depth_at_turn(18, 8, &g, 3, cap, &mut st.clone()), 38);
        assert_eq!(
            depth_at_turn(18, 8, &g, 4, cap, &mut st.clone()),
            40,
            "44 uncapped, so the ceiling binds here"
        );
        assert_eq!(
            depth_at_turn(18, 8, &g, 20, cap, &mut st.clone()),
            40,
            "and it stays there rather than creeping"
        );

        // Monotone throughout, which FR-014a's strict extension requires: a capped path
        // must never be shorter than the turn before it, or the rolling-hash key of a
        // later turn would not extend the earlier one.
        let mut last = 0;
        for turn in 1..=20 {
            let d = depth_at_turn(18, 8, &g, turn, cap, &mut Stream::new(1, 1));
            assert!(d >= last, "turn {turn} shrank from {last} to {d}");
            last = d;
        }
    }

    #[test]
    fn a_ceiling_below_turn_ones_own_depth_never_truncates_it() {
        // The ceiling bounds how far a conversation GROWS, not how much prefix it
        // starts with. Clipping turn 1 would shorten a *shared* prefix, so a
        // conversation-length ceiling would be quietly editing the trunk — changing
        // what sharing the corpus realises and how `roots.count` and the FR-009f
        // occupancy floor read.
        let g = Dist::Scalar(6.0);
        let st = Stream::new(1, 1);
        assert_eq!(
            depth_at_turn(18, 8, &g, 1, Some(5), &mut st.clone()),
            26,
            "turn 1 keeps its full shared + private depth"
        );
        assert_eq!(
            depth_at_turn(18, 8, &g, 9, Some(5), &mut st.clone()),
            26,
            "and such a session simply never grows"
        );
    }

    #[test]
    fn an_unset_ceiling_generates_exactly_what_it_did_before() {
        // The compatibility guarantee: `max_depth` unset must leave every existing
        // document's stream untouched, so `None` cannot be quietly treated as some
        // large finite bound.
        let g = Dist::Shaped(Shape::Lognormal {
            median: 6.0,
            sigma: 0.5,
        });
        for turn in [1u16, 2, 7, 50] {
            let uncapped = depth_at_turn(18, 8, &g, turn, None, &mut Stream::new(9, 9));
            let huge = depth_at_turn(18, 8, &g, turn, Some(u32::MAX), &mut Stream::new(9, 9));
            assert_eq!(uncapped, huge, "turn {turn}");
        }
    }

    #[test]
    fn mixture_entries_override_only_what_they_state() {
        let mut w = workload(6.0, 3.0);
        w.mix = vec![
            MixEntry {
                weight: 0.0,
                turns: None,
                think_time: None,
                private_depth: None,
                growth_per_turn: None,
            },
            MixEntry {
                weight: 1.0,
                turns: Some(Dist::Scalar(1.0)),
                think_time: None,
                private_depth: Some(Dist::Scalar(4000.0)),
                growth_per_turn: None,
            },
        ];
        let p = draw_params(&w, &mut Stream::new(2, 2));
        assert_eq!(p.mix_index, 1);
        assert_eq!(p.turns, 1, "override applied");
        assert_eq!(p.private_depth, 4000);
        assert_eq!(p.think_time_s, 3.0, "unstated field inherited");
    }

    #[test]
    fn mixture_weights_need_not_sum_to_one() {
        let mut w = workload(6.0, 3.0);
        w.mix = vec![
            MixEntry {
                weight: 70.0,
                turns: None,
                think_time: None,
                private_depth: None,
                growth_per_turn: None,
            },
            MixEntry {
                weight: 30.0,
                turns: Some(Dist::Scalar(1.0)),
                think_time: None,
                private_depth: None,
                growth_per_turn: None,
            },
        ];
        let mut counts = [0u32; 2];
        for i in 0..4000 {
            let p = draw_params(&w, &mut Stream::new(3, i));
            counts[p.mix_index as usize] += 1;
        }
        let frac = f64::from(counts[0]) / 4000.0;
        assert!((frac - 0.7).abs() < 0.05, "weights not normalised: {frac}");
    }

    /// Mean and squared coefficient of variation of a sample of gaps.
    fn gap_moments(ia: &Interarrival, n: usize) -> (f64, f64) {
        let mut st = Stream::new(0xBEEF, 1);
        let xs: Vec<f64> = (0..n).map(|_| ia.next_s(&mut st)).collect();
        let mean = xs.iter().sum::<f64>() / n as f64;
        let var = xs.iter().map(|x| (x - mean) * (x - mean)).sum::<f64>() / n as f64;
        (mean, var / (mean * mean))
    }

    #[test]
    fn burstiness_one_is_poisson_and_is_the_neutral_value() {
        // FR-017: the neutral value has to be a value, so that omitting the field
        // and stating 1.0 describe the same process.
        let ia = Interarrival::new(4000.0, 1.0);
        assert_eq!(ia, Interarrival::new(4000.0, 1.0));
        let (mean, scv) = gap_moments(&ia, 200_000);
        assert!((mean - 1.0 / 4000.0).abs() < 1e-6, "mean {mean}");
        assert!((scv - 1.0).abs() < 0.05, "exponential scv was {scv}");
    }

    #[test]
    fn the_configured_index_of_dispersion_is_the_measured_one() {
        // The point of choosing IDC over a scale-free knob: 1.8 in the document
        // is 1.8 in the stream. Asserted rather than assumed.
        for b in [1.8, 4.0] {
            let ia = Interarrival::new(1000.0, b);
            let (mean, scv) = gap_moments(&ia, 400_000);
            assert!((mean - 1e-3).abs() < 1e-4, "mean moved with burstiness");
            assert!((scv - b).abs() < 0.15 * b, "asked {b}, measured {scv}");
        }
    }

    #[test]
    fn under_dispersion_falls_back_to_poisson_rather_than_inventing_a_shape() {
        // Not modelled, so not silently reinterpreted; validate warns about it.
        assert_eq!(Interarrival::new(100.0, 0.4), Interarrival::new(100.0, 1.0));
    }

    #[test]
    fn a_session_retires_after_its_last_turn() {
        let mut s = Session {
            id: SessionId(1),
            node: 0,
            root_index: 0,
            params: SessionParams {
                max_depth: None,
                mix_index: 0,
                turns: 3,
                private_depth: 4,
                think_time_s: 2.0,
                growth_per_turn: Dist::Scalar(1.0),
            },
            turn: 1,
            next_t_ns: 0,
        };
        assert!(!s.is_retired());
        s.turn = 3;
        assert!(!s.is_retired(), "the last turn has not been issued yet");
        s.turn = 4;
        assert!(s.is_retired());
        // Lifetime is derived from turns and think time, never configured.
        assert_eq!(s.lifetime_ns(), 2 * 2_000_000_000);
    }
}
