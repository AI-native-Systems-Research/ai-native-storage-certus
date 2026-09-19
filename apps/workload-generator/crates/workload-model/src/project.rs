//! Pre-flight projection: what a run will cost, before it writes anything.
//!
//! Requiring a span is not protection on its own. `--until 100000` is legal on the
//! shipped example and costs tens of gigabytes, so FR-073 requires an emit run to
//! project its output from the description's own rates and refuse if that exceeds
//! free space or a documented ceiling — naming both figures.
//!
//! # Everything here comes from rates, not from a rehearsal
//!
//! The projection does not simulate. It uses Little's law and the resolved
//! distributions' own moments, so it costs microseconds and can be offered as a
//! query (`validate --until n`) rather than as a dry run.
//!
//! For a session class of `N` concurrent sessions with mean think time `w` and mean
//! turn count `E[T]`:
//!
//! ```text
//! mean session duration   D = E[T] * w
//! arrival rate            λ = N / D
//! turns per span S        N * S / w        (concurrency times turn rate)
//! ```
//!
//! # Why `E[T²]` needs a Monte Carlo draw
//!
//! A session's *key references* grow quadratically in its own turn count, because
//! every turn checks its whole prefix. Per session, with a shared run of `H` blocks
//! and growth `g = E[input] + E[output]` per turn:
//!
//! ```text
//! references = 3*H*T  +  1.5*g*T*(T-1)  +  3*g*T
//!              └ check+touch+load over the shared run
//!                             └ ...over accumulated growth
//!                                            └ reserve+transfer+commit, both runs
//! ```
//!
//! so the projection needs `E[T²]`, which no distribution here exposes in closed
//! form — `turns` may be written as any of five kinds, including an empirical
//! sample set. It is estimated by drawing [`MONTE_CARLO_DRAWS`] turn counts and
//! averaging `T²`. That is accurate to well under a percent for any sane turn
//! distribution and costs a fraction of a millisecond.
//!
//! # What it deliberately does not claim
//!
//! - **Bytes are for the canonical plan only.** That layout is fixed and known, so
//!   the figure is exact. Per-container *trace* bytes arrive with the trace writer
//!   (T053), because they depend on a record shape that does not exist yet, and
//!   guessing them would put a made-up number in a refusal message.
//! - **The seeded generation is not modelled.** Sessions alive at `t = 0` have
//!   residual turn counts, so a very short span references fewer keys than
//!   projected. The error decays over one mean session duration, and
//!   [`Projection::warnings`] says so when the span is short.
//! - **Key references are an UPPER BOUND when the session-length tail outlives the
//!   span.** Every session is counted for its full quadratic contribution, but a
//!   session cut off by the span never reaches its late, expensive turns. This bites
//!   whenever `turns` is heavy-tailed: on the shipped example, whose `turns` is
//!   exponential with mean 10, a 300-second span gave a projection **2.9x** the
//!   actual — while the mean duration of 105 seconds raised no warning at all. The
//!   warning is therefore keyed on the **p99** session duration, not the mean. The
//!   direction is the safe one for a refusal, and the warning says not to compare
//!   the figure with a run's actual count.
//! - **Emptiness is assumed from means.** A turn is counted as issuing a check and
//!   a load whenever the class has any shared instances or any growth, and as
//!   issuing a store sequence whenever the corresponding growth mean is positive. A
//!   description whose growth is *usually* zero would be over-projected, which is
//!   the safe direction for a refusal.
//!
//! # Examples
//!
//! ```
//! use workload_model::description::WorkloadDescription;
//! use workload_model::project::project;
//!
//! let yaml = r#"
//! version: 1
//! blocks: {tokens: 16, bytes: 32768}
//! shared_classes:
//!   manual:
//!     length: {constant: 4}
//!     lifetime: {constant: .inf}
//! session_classes:
//!   chat:
//!     pool: {size: {exact: 10}}
//!     uses: [{class: manual, count: {constant: 1}}]
//!     turns: {constant: 5}
//!     input_growth: {constant: 2}
//!     output_growth: {constant: 1}
//!     think_time: {constant: 10}
//! "#;
//! let d: WorkloadDescription = yaml.parse().unwrap();
//! let p = project(&d, 1_000.0, 1).unwrap();
//!
//! // 10 concurrent sessions, one turn every 10 virtual seconds each.
//! assert_eq!(p.invocations, 1_000);
//! assert!(p.key_references > p.keys_minted, "every prefix is re-checked");
//! assert!(p.plan_bytes > 0);
//! ```

use crate::description::WorkloadDescription;
use crate::distribution::Resolved;
use crate::plan::PlanOptions;
use crate::rng;
use crate::Result;

/// Turn-count draws used to estimate `E[T²]`.
///
/// The relative error of a second-moment estimate goes as `1/sqrt(n)`, so 10 000
/// draws place it inside about 1% for any turn distribution with finite variance —
/// far tighter than the projection's other assumptions.
pub const MONTE_CARLO_DRAWS: usize = 10_000;

/// Bytes the canonical plan spends on its header. See
/// [`OperationPlan::write_canonical`](crate::plan::OperationPlan::write_canonical).
const PLAN_HEADER_BYTES: u64 = 8 + 2 + 2 + 8;

/// Bytes per operation record, before its keys.
const PLAN_OP_BYTES: u64 = 8 + 8 + 2 + 2 + 4;

/// What a run of a given span will cost.
#[derive(Debug, Clone, PartialEq)]
pub struct Projection {
    /// The span projected, in virtual seconds.
    pub span: f64,
    /// Turns — one LLM request each.
    pub invocations: u64,
    /// Operations, which is several per turn.
    pub operations: u64,
    /// Distinct keys ever minted: shared instances plus turn growth.
    ///
    /// Grows **linearly** with the span.
    pub keys_minted: u64,
    /// Key references summed over operations.
    ///
    /// Grows linearly with the span too, but **quadratically in session length** —
    /// which is why the two figures are reported separately (FR-073). A description
    /// with long sessions can have a modest key space and an enormous reference
    /// count.
    pub key_references: u64,
    /// Uncompressed size of the canonical plan. Exact, from a fixed layout.
    pub plan_bytes: u64,
    /// Things a reader needs to know about this projection's own validity.
    pub warnings: Vec<String>,
}

impl Projection {
    /// Render as the lines a report or a refusal message prints.
    ///
    /// # Examples
    ///
    /// ```
    /// use workload_model::description::WorkloadDescription;
    /// use workload_model::project::project;
    ///
    /// let yaml = r#"
    /// version: 1
    /// blocks: {tokens: 16, bytes: 32768}
    /// shared_classes:
    ///   m: {length: {constant: 2}, lifetime: {constant: .inf}}
    /// session_classes:
    ///   c:
    ///     pool: {size: {exact: 1}}
    ///     uses: [{class: m, count: {constant: 1}}]
    ///     turns: {constant: 2}
    ///     input_growth: {constant: 1}
    ///     output_growth: {constant: 1}
    ///     think_time: {constant: 1}
    /// "#;
    /// let d: WorkloadDescription = yaml.parse().unwrap();
    /// let text = project(&d, 100.0, 1).unwrap().render();
    /// assert!(text.contains("invocations"));
    /// ```
    pub fn render(&self) -> String {
        let mut out = format!(
            "projection for a {:.0}-virtual-second span:\n  \
             invocations       {}\n  \
             operations        {}\n  \
             keys minted       {}\n  \
             key references    {}\n  \
             canonical plan    {}\n",
            self.span,
            self.invocations,
            self.operations,
            self.keys_minted,
            self.key_references,
            human_bytes(self.plan_bytes),
        );
        for w in &self.warnings {
            out.push_str("  warning: ");
            out.push_str(w);
            out.push('\n');
        }
        out
    }
}

/// Format a byte count the way a refusal message should read.
fn human_bytes(n: u64) -> String {
    const UNITS: [&str; 5] = ["B", "KiB", "MiB", "GiB", "TiB"];
    let mut v = n as f64;
    let mut u = 0;
    while v >= 1024.0 && u + 1 < UNITS.len() {
        v /= 1024.0;
        u += 1;
    }
    if u == 0 {
        format!("{n} B")
    } else {
        format!("{v:.2} {} ({n} bytes)", UNITS[u])
    }
}

/// Project a run of `span` virtual seconds.
///
/// `seed` is used only for the `E[T²]` Monte Carlo, on its own substream, so
/// projecting cannot disturb a run's draws.
///
/// # Errors
///
/// If a distribution cannot be resolved, or a session class has a non-positive mean
/// think time — the same condition load-time validation refuses, because the
/// arrival rate is then unbounded.
pub fn project(description: &WorkloadDescription, span: f64, seed: u64) -> Result<Projection> {
    project_with(description, span, seed, PlanOptions::default())
}

/// Project with explicit [`PlanOptions`], since the poll cadence changes the
/// operation count.
///
/// # Errors
///
/// As [`project`].
pub fn project_with(
    description: &WorkloadDescription,
    span: f64,
    seed: u64,
    options: PlanOptions,
) -> Result<Projection> {
    let mut rng = rng::substream(seed, "projection");
    let mut warnings = Vec::new();

    // Shared populations: mints are the seeded generation plus births over the span.
    let mut keys_minted = 0u64;
    let mut shared_length = Vec::with_capacity(description.shared_classes.len());
    for (_, name, class) in description.shared_classes.iter() {
        let lifetime = class.lifetime.resolve(None)?;
        let length = class.length.resolve_integral(None)?;
        let nominal = class.pool.size.nominal() as f64;
        let mean_life = lifetime.effective().mean;
        let mean_length = length.effective().mean;
        shared_length.push(mean_length);

        let generations = if mean_life.is_finite() && mean_life > 0.0 {
            1.0 + span / mean_life
        } else {
            // FR-017: an unbounded lifetime is minted once and never turns over.
            1.0
        };
        keys_minted += (nominal * generations * mean_length).ceil() as u64;
        if generations > 1.0 && span < mean_life {
            warnings.push(format!(
                "shared class {name:?} has a mean lifetime of {mean_life:.0} virtual \
                 seconds, longer than the {span:.0}-second span, so the run cannot \
                 exhibit the pool turnover the description specifies"
            ));
        }
    }

    let mut invocations = 0u64;
    let mut operations = 0u64;
    let mut key_references = 0u64;

    for (_, name, class) in description.session_classes.iter() {
        let turns = class.turns.resolve_integral(None)?;
        let think = class.think_time.resolve(None)?;
        let input = class.input_growth.resolve_integral(None)?;
        let output = class.output_growth.resolve_integral(None)?;

        let n = class.pool.size.nominal() as f64;
        let mean_turns = turns.effective().mean;
        let mean_think = think.effective().mean;
        // NaN takes the `is_finite` branch, so it is refused rather than silently
        // producing a NaN rate.
        if mean_think <= 0.0 || !mean_think.is_finite() {
            return Err(crate::Error::new(format!(
                "session class {name:?} has a mean think time of {mean_think}; the \
                 arrival rate needed to sustain {n} concurrent sessions is unbounded"
            )));
        }
        let duration = mean_turns * mean_think;
        if span < duration {
            warnings.push(format!(
                "session class {name:?} has a mean session duration of \
                 {duration:.0} virtual seconds, longer than the {span:.0}-second \
                 span, so most sessions will not complete and the seeded \
                 generation's shorter chains dominate"
            ));
        }
        // The reference figure is driven by the *tail*, not the mean. A class whose
        // p99 session outlives the span has long sessions that never reach their
        // expensive late turns, while this projection counts every session's full
        // quadratic contribution — so the estimate becomes an upper bound.
        //
        // Measured on the shipped example: mean duration 105 s against a 300 s span
        // raises no mean-based warning at all, yet references came out 2.9x over.
        // Warning on the mean alone would have missed that entirely.
        let tail_duration = turns.effective().p99 * mean_think;
        if span < tail_duration && span >= duration {
            warnings.push(format!(
                "session class {name:?} has a mean session duration of \
                 {duration:.0} virtual seconds but a p99 of {tail_duration:.0}, \
                 longer than the {span:.0}-second span. Its longest sessions will be \
                 cut off before their most expensive turns, so the key-reference \
                 figure below is an UPPER BOUND and may exceed the actual by a \
                 factor of a few. Sizing on it is safe; comparing it to a run's \
                 actual count is not"
            ));
        }

        // Little's law: concurrency times turn rate.
        let class_turns = n * span / mean_think;
        invocations += class_turns.ceil() as u64;

        // The shared run this class binds, in blocks.
        let mut shared_blocks = 0.0;
        for u in &class.uses {
            let Some(index) = description.shared_classes.index_of(&u.class) else {
                continue;
            };
            let nominal = description
                .shared_classes
                .by_index(index)
                .map(|(_, c)| c.pool.size.nominal())
                .unwrap_or(0);
            let count = u.count.resolve_integral(Some(nominal as f64))?;
            shared_blocks += count.effective().mean * shared_length[index];
        }

        let g = input.effective().mean + output.effective().mean;
        // Growth blocks are minted once per turn.
        keys_minted += (class_turns * g).ceil() as u64;

        // Operations per turn. See the module docs on the emptiness assumption.
        let mut ops_per_turn = 0.0;
        if shared_blocks > 0.0 || g > 0.0 {
            // Check, touch and load, all over the same prefix. Three, not two: the
            // reference report is its own operation in the shipped client, and
            // `plan.rs` records why leaving it out was a defect.
            ops_per_turn += 3.0;
        }
        if input.effective().mean > 0.0 {
            ops_per_turn += 3.0;
        }
        if output.effective().mean > 0.0 {
            ops_per_turn += 3.0;
        }
        ops_per_turn += 1.0 / options.poll_events_every.max(1) as f64;
        operations += (class_turns * ops_per_turn).ceil() as u64;

        // References per *session*, then scaled by sessions completing in the span.
        // `E[T^2]` by Monte Carlo; see the module docs.
        let (e_t, e_t2) = turn_moments(&turns, &mut rng);
        // Three passes over the prefix (check, touch, load) rather than two.
        let per_session = 3.0 * shared_blocks * e_t + 1.5 * g * (e_t2 - e_t) + 3.0 * g * e_t;
        let sessions_in_span = class_turns / e_t.max(1.0);
        key_references += (sessions_in_span * per_session).ceil() as u64;
    }

    let plan_bytes =
        PLAN_HEADER_BYTES + operations * PLAN_OP_BYTES + key_references.saturating_mul(8);

    Ok(Projection {
        span,
        invocations,
        operations,
        keys_minted,
        key_references,
        plan_bytes,
        warnings,
    })
}

/// `(E[T], E[T²])` for an integral turn-count distribution, by Monte Carlo.
///
/// The first moment is taken from the same draws rather than from
/// [`Resolved::effective`] so that the two are consistent: mixing an analytic mean
/// with a sampled second moment can make `E[T²] - E[T]²` come out negative for a
/// near-degenerate distribution, and a negative variance in a projection is worse
/// than a slightly noisier one.
fn turn_moments<R: rand::Rng + ?Sized>(turns: &Resolved, rng: &mut R) -> (f64, f64) {
    let mut sum = 0.0;
    let mut sum_sq = 0.0;
    for _ in 0..MONTE_CARLO_DRAWS {
        let t = turns.sample_int(rng).max(1) as f64;
        sum += t;
        sum_sq += t * t;
    }
    let n = MONTE_CARLO_DRAWS as f64;
    (sum / n, sum_sq / n)
}

/// The span that would produce about `target` invocations.
///
/// Inverts [`project`]'s invocation rate, which is linear in the span, so this is
/// exact rather than a search. Turns "pick a number, wait, discover it was 27 GB"
/// into a one-line query.
///
/// # Errors
///
/// If a distribution cannot be resolved, or no session class can produce turns at
/// all — in which case no span would reach the target and returning a number would
/// be a lie.
pub fn span_for_invocations(description: &WorkloadDescription, target: u64) -> Result<f64> {
    let mut rate = 0.0;
    for (_, _, class) in description.session_classes.iter() {
        let think = class.think_time.resolve(None)?;
        let mean_think = think.effective().mean;
        if mean_think > 0.0 && mean_think.is_finite() {
            rate += class.pool.size.nominal() as f64 / mean_think;
        }
    }
    if rate <= 0.0 {
        return Err(crate::Error::new(
            "no session class can produce invocations, so no span reaches the target".to_string(),
        ));
    }
    Ok(target as f64 / rate)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::plan::OperationPlan;
    use crate::sim::Simulation;

    fn simple() -> WorkloadDescription {
        r#"
version: 1
blocks: {tokens: 16, bytes: 32768}
shared_classes:
  manual:
    length: {constant: 4}
    lifetime: {constant: .inf}
session_classes:
  chat:
    pool: {size: {exact: 10}}
    uses: [{class: manual, count: {constant: 1}}]
    turns: {constant: 5}
    input_growth: {constant: 2}
    output_growth: {constant: 1}
    think_time: {constant: 10}
"#
        .parse()
        .unwrap()
    }

    #[test]
    fn invocations_follow_littles_law_exactly() {
        // 10 concurrent sessions, one turn each per 10 virtual seconds.
        let p = project(&simple(), 1_000.0, 1).unwrap();
        assert_eq!(p.invocations, 1_000);
        let p = project(&simple(), 2_000.0, 1).unwrap();
        assert_eq!(p.invocations, 2_000, "invocations are linear in the span");
    }

    #[test]
    fn keys_minted_and_references_are_reported_separately() {
        // FR-073: they grow differently, and conflating them would hide the case
        // this projection exists to catch — a modest key space with an enormous
        // reference count.
        let p = project(&simple(), 1_000.0, 1).unwrap();
        // `manual` declares no `pool:`, and an absent pool means `exact: 1` rather
        // than the Poisson default — so one immortal 4-block instance, plus three
        // growth blocks per turn. (The `exact: 10` in this description is the
        // *session* pool.)
        assert_eq!(p.keys_minted, 4 + 3 * 1_000);
        assert!(p.key_references > p.keys_minted);
    }

    #[test]
    fn the_projection_matches_an_actual_run_within_tolerance() {
        // T035, on the shipped example. The projection is rate-based and the run is
        // stochastic, so the tolerance is stated rather than tight — but it has to
        // be close enough to be worth refusing a run over.
        let yaml = include_str!(
            "../../../specs/001-synthetic-workload-generator/contracts/workload-input.example.yml"
        );
        let d: WorkloadDescription = yaml.parse().unwrap();
        let span = 1_000.0;

        let projected = project(&d, span, 5).unwrap();
        let mut sim = Simulation::new(&d, 5, 1).unwrap();
        let mut plan = OperationPlan::default();
        sim.run_until(span, &mut |s, t| plan.record_turn(s, t));

        let check = |name: &str, projected: u64, actual: u64, tolerance: f64| {
            let p = projected as f64;
            let a = actual as f64;
            let error = (p - a).abs() / a.max(1.0);
            assert!(
                error < tolerance,
                "{name}: projected {projected}, actual {actual}, error {:.1}% \
                 (tolerance {:.0}%)",
                error * 100.0,
                tolerance * 100.0
            );
        };
        check("invocations", projected.invocations, plan.turns(), 0.25);
        check("operations", projected.operations, plan.len() as u64, 0.30);
        check(
            "key references",
            projected.key_references,
            plan.key_references() as u64,
            0.60,
        );
        check(
            "plan bytes",
            projected.plan_bytes,
            plan.to_canonical_bytes().len() as u64,
            0.60,
        );
    }

    #[test]
    fn plan_bytes_are_exact_for_a_deterministic_description() {
        // The byte figure comes from a fixed layout, so where the counts are exact
        // the bytes must be too — this is the figure a refusal quotes.
        let d = simple();
        let span = 1_000.0;
        let projected = project(&d, span, 3).unwrap();
        let mut sim = Simulation::new(&d, 3, 1).unwrap();
        let mut plan = OperationPlan::default();
        sim.run_until(span, &mut |s, t| plan.record_turn(s, t));

        assert_eq!(
            projected.operations,
            plan.len() as u64,
            "a constant description should project its operation count exactly"
        );
        let actual = plan.to_canonical_bytes().len() as u64;
        let error = (projected.plan_bytes as f64 - actual as f64).abs() / actual as f64;
        assert!(
            error < 0.15,
            "projected {} bytes against an actual {actual}",
            projected.plan_bytes
        );
    }

    #[test]
    fn span_inversion_round_trips() {
        let d = simple();
        let span = span_for_invocations(&d, 5_000).unwrap();
        let p = project(&d, span, 1).unwrap();
        assert_eq!(p.invocations, 5_000);
    }

    #[test]
    fn a_short_span_is_warned_about() {
        // T034: a run too short to exhibit what it was configured for.
        let yaml = r#"
version: 1
blocks: {tokens: 16, bytes: 32768}
shared_classes:
  docs:
    length: {constant: 2}
    lifetime: {exponential: {mean: 100000}}
    pool: {size: {poisson: 5}}
session_classes:
  chat:
    pool: {size: {exact: 2}}
    uses: [{class: docs, count: {constant: 1}}]
    turns: {constant: 100}
    input_growth: {constant: 1}
    output_growth: {constant: 1}
    think_time: {constant: 10}
"#;
        let d: WorkloadDescription = yaml.parse().unwrap();
        let p = project(&d, 50.0, 1).unwrap();
        assert!(
            p.warnings.iter().any(|w| w.contains("turnover")),
            "no pool-turnover warning: {:?}",
            p.warnings
        );
        assert!(
            p.warnings.iter().any(|w| w.contains("not complete")),
            "no session-completion warning: {:?}",
            p.warnings
        );
        assert!(p.render().contains("warning:"));
    }

    #[test]
    fn a_heavy_tailed_turn_count_warns_that_references_are_an_upper_bound() {
        // The case a mean-based warning misses entirely, and the reason the warning
        // is keyed on p99. Exponential turns with mean 10 give a mean duration of
        // ~105 s, so a 300 s span raises no mean warning — but the p99 session runs
        // ~460 s and is cut off before its expensive turns.
        let yaml = r#"
version: 1
blocks: {tokens: 16, bytes: 32768}
shared_classes:
  m: {length: {constant: 4}, lifetime: {constant: .inf}}
session_classes:
  chat:
    pool: {size: {exact: 50}}
    uses: [{class: m, count: {constant: 1}}]
    turns: {exponential: {mean: 10, min: 1}}
    input_growth: {constant: 5}
    output_growth: {constant: 2}
    think_time: {exponential: {mean: 10}}
"#;
        let d: WorkloadDescription = yaml.parse().unwrap();
        let p = project(&d, 300.0, 1).unwrap();
        assert!(
            p.warnings.iter().any(|w| w.contains("UPPER BOUND")),
            "no upper-bound warning for a heavy-tailed turn count: {:?}",
            p.warnings
        );
        assert!(
            !p.warnings
                .iter()
                .any(|w| w.contains("most sessions will not")),
            "the mean-based warning should NOT fire here — that is the point"
        );
        // And it stops firing once the span covers the tail.
        let long = project(&d, 20_000.0, 1).unwrap();
        assert!(
            !long.warnings.iter().any(|w| w.contains("UPPER BOUND")),
            "a warning that always fires is noise: {:?}",
            long.warnings
        );
    }

    #[test]
    fn a_long_enough_span_is_not_warned_about() {
        // The counterpart: a warning that always fires is noise.
        let p = project(&simple(), 100_000.0, 1).unwrap();
        assert!(p.warnings.is_empty(), "spurious warnings: {:?}", p.warnings);
    }

    #[test]
    fn the_second_moment_estimate_is_stable_across_seeds() {
        // If `E[T^2]` were noisy the whole reference figure would be, and a refusal
        // would depend on the projection's own seed.
        let d = simple();
        let a = project(&d, 1_000.0, 1).unwrap().key_references;
        for seed in 2..8 {
            let b = project(&d, 1_000.0, seed).unwrap().key_references;
            let error = (a as f64 - b as f64).abs() / a as f64;
            assert!(error < 0.01, "seed {seed} moved references by {error:.3}");
        }
    }

    #[test]
    fn references_are_quadratic_in_session_length_at_a_fixed_turn_rate() {
        // The reason `E[T^2]` is needed at all. Holding the turn *rate* fixed and
        // lengthening sessions leaves invocations unchanged while references climb.
        let with_turns = |t: u32| {
            let yaml = format!(
                r#"
version: 1
blocks: {{tokens: 16, bytes: 32768}}
shared_classes:
  m: {{length: {{constant: 2}}, lifetime: {{constant: .inf}}}}
session_classes:
  c:
    pool: {{size: {{exact: 4}}}}
    uses: [{{class: m, count: {{constant: 1}}}}]
    turns: {{constant: {t}}}
    input_growth: {{constant: 1}}
    output_growth: {{constant: 1}}
    think_time: {{constant: 10}}
"#
            );
            let d: WorkloadDescription = yaml.parse().unwrap();
            project(&d, 10_000.0, 1).unwrap()
        };
        let short = with_turns(5);
        let long = with_turns(20);
        assert_eq!(
            short.invocations, long.invocations,
            "the turn rate is the same, so invocations must be too"
        );
        let ratio = long.key_references as f64 / short.key_references as f64;
        assert!(
            ratio > 2.5,
            "quadrupling session length changed references by only {ratio:.2}x"
        );
    }

    #[test]
    fn a_zero_mean_think_time_is_refused() {
        let yaml = r#"
version: 1
blocks: {tokens: 16, bytes: 32768}
shared_classes:
  m: {length: {constant: 2}, lifetime: {constant: .inf}}
session_classes:
  c:
    pool: {size: {exact: 1}}
    uses: [{class: m, count: {constant: 1}}]
    turns: {constant: 2}
    input_growth: {constant: 1}
    output_growth: {constant: 1}
    think_time: {constant: 0}
"#;
        let d: WorkloadDescription = yaml.parse().unwrap();
        let err = project(&d, 100.0, 1).unwrap_err().to_string();
        assert!(err.contains("unbounded"), "unexpected error: {err}");
    }

    #[test]
    fn bytes_are_rendered_for_a_human() {
        assert_eq!(human_bytes(512), "512 B");
        assert!(human_bytes(27 * 1024 * 1024 * 1024).starts_with("27.00 GiB"));
        // The exact count is kept alongside the rounded figure, because a refusal
        // that says only "27 GiB" cannot be checked against a free-space number.
        assert!(human_bytes(2048).contains("2048 bytes"));
    }
}
