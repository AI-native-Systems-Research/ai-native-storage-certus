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
    SessionParams {
        mix_index,
        turns: turns_d.sample_u64(st).clamp(1, u64::from(u16::MAX)) as u16,
        private_depth: priv_d.sample_u64(st).min(u64::from(u32::MAX)) as u32,
        think_time_s: think_d.sample(st).max(0.0),
        growth_per_turn: growth_d,
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
pub fn depth_at_turn(
    shared_depth: u32,
    private_depth: u32,
    growth: &Dist,
    turn: u16,
    st: &mut Stream,
) -> u32 {
    let mut d = shared_depth.saturating_add(private_depth);
    for _ in 2..=turn {
        d = d.saturating_add(growth.sample_u64(st).min(u64::from(u32::MAX)) as u32);
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
                turns: Dist::Shaped(Shape::Geometric { mean: turns_mean }),
                think_time: Dist::Scalar(think),
                private_depth: Dist::Scalar(8.0),
                growth_per_turn: Dist::Scalar(6.0),
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
        let d1 = depth_at_turn(18, 8, &g, 1, &mut st.clone());
        let d6 = depth_at_turn(18, 8, &g, 6, &mut st);
        assert_eq!(d1, 26);
        assert_eq!(d6, 26 + 5 * 6, "six turns adds five growth increments");
        assert!(d6 > d1);
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

    #[test]
    fn a_session_retires_after_its_last_turn() {
        let mut s = Session {
            id: SessionId(1),
            node: 0,
            root_index: 0,
            params: SessionParams {
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
