//! Validation rules.
//!
//! Two properties matter as much as the rules themselves.
//!
//! **Every violation is reported, not just the first.** A half-configured
//! document usually has several, and surfacing one at a time makes fixing it a
//! guessing game.
//!
//! **Some rules reject rather than warn.** The distinction is whether the
//! resulting numbers would be *wrong* or merely *noisy*. A warmup shorter than
//! the session-population ramp yields a measurement of the clock rather than of
//! the workload, so it is refused; occupancy below the comfortable threshold is
//! still measurable, so it warns.

use super::{ArrivalModel, Branching, Document, Growth, Placement};
use crate::corpus::{occupancy, Corpus, TARGET_OCCUPANCY};
use crate::dist::Shape;
use crate::session::{check_warmup, Population};
use crate::units::{parse_duration_ns, parse_rate_per_s};

/// How seriously a finding should be taken.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Severity {
    /// The document cannot be used.
    Reject,
    /// Usable, but the reader should know.
    Warn,
}

/// One validation finding.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Finding {
    /// Reject or warn.
    pub severity: Severity,
    /// Which rule, matching `contracts/workload-schema.md` § Validation rules.
    pub rule: &'static str,
    /// What is wrong, and where the quantity belongs if it has moved.
    pub message: String,
}

/// The outcome of validating a document.
#[derive(Debug, Default)]
pub struct Report {
    /// Everything found, in rule order.
    pub findings: Vec<Finding>,
}

impl Report {
    /// Whether anything rejects the document.
    pub fn is_rejected(&self) -> bool {
        self.findings.iter().any(|f| f.severity == Severity::Reject)
    }

    /// Only the rejections.
    pub fn rejections(&self) -> impl Iterator<Item = &Finding> {
        self.findings
            .iter()
            .filter(|f| f.severity == Severity::Reject)
    }

    fn reject(&mut self, rule: &'static str, message: impl Into<String>) {
        self.findings.push(Finding {
            severity: Severity::Reject,
            rule,
            message: message.into(),
        });
    }

    fn warn(&mut self, rule: &'static str, message: impl Into<String>) {
        self.findings.push(Finding {
            severity: Severity::Warn,
            rule,
            message: message.into(),
        });
    }
}

/// Validate a document, returning **all** findings.
pub fn validate(d: &Document) -> Report {
    let mut r = Report::default();

    // Rule: version must be implemented (FR-006).
    if d.version != 1 {
        r.reject(
            "version",
            format!(
                "schema version {} is not implemented by this generator; only version 1 is",
                d.version
            ),
        );
    }

    /// Where a mixture arm's finding should say it came from.
    fn mix_label(i: usize) -> String {
        format!("workload.mix[{i}]")
    }

    // Rule 24: a banded `growth_per_turn` must be a usable table (FR-054f).
    //
    // Rejections rather than warnings for the same reason rule 8 rejects a malformed
    // branching profile: a table whose bands do not ascend, or which starts above the
    // shortest session, silently sends some sessions to the wrong band, and every path
    // those sessions generate is then wrong in a way no later check attributes here.
    let banded: Vec<(String, &Growth)> = std::iter::once((
        "workload.sessions".to_string(),
        &d.workload.sessions.growth_per_turn,
    ))
    .chain(
        d.workload
            .mix
            .iter()
            .enumerate()
            .filter_map(|(i, m)| m.growth_per_turn.as_ref().map(|g| (mix_label(i), g))),
    )
    .collect();
    for (where_, growth) in banded {
        if let Growth::Banded(b) = growth {
            if b.by_turns.is_empty() {
                r.reject(
                    "24",
                    format!(
                        "{where_}.growth_per_turn.by_turns is empty; state a distribution or bands"
                    ),
                );
                continue;
            }
            if b.by_turns[0].from_turns > 1 {
                r.reject(
                    "24",
                    format!(
                        "{}.growth_per_turn.by_turns starts at from_turns {}, so a session \
                         shorter than that names no band. The first band must start at 1",
                        where_, b.by_turns[0].from_turns
                    ),
                );
            }
            let mut prev: Option<u32> = None;
            for band in &b.by_turns {
                if let Some(p) = prev {
                    if band.from_turns <= p {
                        r.reject(
                            "24",
                            format!(
                                "{where_}.growth_per_turn bands must ascend by from_turns; \
                                 {} follows {}",
                                band.from_turns, p
                            ),
                        );
                    }
                }
                prev = Some(band.from_turns);
            }
        }
    }

    // Rule 19: exactly one run length.
    let lengths = [
        d.duration.is_some(),
        d.requests.is_some(),
        d.blocks.is_some(),
        d.unbounded.unwrap_or(false),
    ];
    let n = lengths.iter().filter(|x| **x).count();
    if n != 1 {
        r.reject(
            "19",
            format!(
                "exactly one of duration | requests | blocks | unbounded is required, found {n}"
            ),
        );
    }

    // Rules 20 and 21: file output needs a block budget and forbids unbounded.
    let file_mode = matches!(d.run.mode.as_str(), "jsonl" | "parquet");
    if file_mode && d.blocks.is_none() {
        r.reject(
            "20",
            "a file output mode requires `blocks`: a file's size is a block count, and \
             duration/requests both leave it at the mercy of the drawn request-length \
             distribution, so an overlong run fills the filesystem",
        );
    }
    if file_mode && d.unbounded.unwrap_or(false) {
        r.reject(
            "21",
            "`unbounded` is meaningful only when nothing accumulates, i.e. when driving a \
             server directly; a file output mode must be bounded",
        );
    }

    // Rule 9's second half: an `empirical` distribution must be a usable CDF. Documented
    // for `shared_depth` — "points not in ascending value order or with a final cumulative
    // probability != 1.0" — and implemented nowhere until 2026-08-14; a grep for
    // `Empirical` in this file found nothing, and `dist.rs` does no deserialize-time
    // checking either, so no code path had ever looked at a point value.
    //
    // Values must be NON-DECREASING rather than strictly ascending, because the step
    // encoding `fit` emits repeats each value deliberately: `(v, c_before), (v, c_after)`
    // is how a discrete distribution is expressed through an interpolating reader. A check
    // demanding strict ascent would reject every fitted document.
    for (label, dist) in [
        ("corpus.trees.shared_depth", &d.corpus.trees.shared_depth),
        (
            "corpus.trees.roots.popularity",
            &d.corpus.trees.roots.popularity,
        ),
    ] {
        if let Shape::Empirical { points } = dist.shape() {
            if points.is_empty() {
                r.reject(
                    "9",
                    format!("{label} is an empirical distribution with no points"),
                );
                continue;
            }
            let mut prev = (f64::MIN, f64::MIN);
            for (v, c) in &points {
                if *v < prev.0 {
                    r.reject(
                        "9",
                        format!(
                            "{label}'s empirical points must be in non-decreasing value order; \
                             {v} follows {}",
                            prev.0
                        ),
                    );
                    break;
                }
                if *c < prev.1 {
                    r.reject(
                        "9",
                        format!(
                            "{label}'s cumulative probability must not decrease; {c} follows {}",
                            prev.1
                        ),
                    );
                    break;
                }
                prev = (*v, *c);
            }
            if (prev.1 - 1.0).abs() > 1e-6 {
                r.reject(
                    "9",
                    format!(
                        "{label}'s final cumulative probability is {}, not 1.0: the mass above it \
                         is unreachable and every draw in that range silently returns the top \
                         point instead",
                        prev.1
                    ),
                );
            }
        }
    }

    // Rule 8: roots and branching domains.
    if d.corpus.trees.roots.count < 1 {
        r.reject("8", "roots.count must be at least 1");
    }
    if d.corpus.trees.branch_skew < 0.0 {
        r.reject("8", "branch_skew must be >= 0");
    }
    // `roots.popularity`'s support IS `roots.count`, which the contract states twice and
    // nothing checked. Both halves went wrong at once in a fitted document, silently:
    //
    // * an `n` written for a Zipf was accepted and then overwritten by the generator, so
    //   the author's value had no effect and no error;
    // * an `empirical` support narrower than `roots.count` left every rank above it
    //   unreachable — 450 of 603 roots on one real trace — and because
    //   `sample_u64_clamped` only counts draws pulled *into* range, unused headroom
    //   above the support records **zero** clamps. The model had 5 populated roots and
    //   said 603.
    //
    // So the check is on the support, and a document that cannot populate the root layer
    // it declares is rejected rather than quietly generating a narrower one.
    match d.corpus.trees.roots.popularity.shape() {
        Shape::Zipf { n: Some(n), .. } => r.reject(
            "8",
            format!(
                "roots.popularity states n = {n}, but the support of a root-popularity \
                 distribution is `roots.count` ({}) and is not the author's to choose. Remove \
                 `n`: the generator supplies it, and a value here would be silently overwritten",
                d.corpus.trees.roots.count
            ),
        ),
        Shape::Empirical { points } => {
            let top = points.iter().map(|(v, _)| *v).fold(0.0f64, f64::max);
            let bottom = points.iter().map(|(v, _)| *v).fold(f64::MAX, f64::min);
            if (top - f64::from(d.corpus.trees.roots.count)).abs() > 0.5 {
                r.reject(
                    "8",
                    format!(
                        "roots.popularity's support reaches rank {top}, but roots.count is {}: \
                         the ranks between are unreachable, so {} of the declared roots would \
                         never be populated and the realised root layer would be narrower than \
                         the document says — silently, since drawing inside a narrow support \
                         records no clamp",
                        d.corpus.trees.roots.count,
                        (f64::from(d.corpus.trees.roots.count) - top).max(0.0)
                    ),
                );
            }
            if bottom < 1.0 {
                r.reject(
                    "8",
                    format!("roots.popularity draws rank {bottom}; ranks are 1-based"),
                );
            }
        }
        _ => {}
    }
    match &d.corpus.trees.branching {
        Branching::Uniform(f) if *f < 1.0 => r.reject(
            "8",
            format!(
                "branching fanout {f} is below 1: a trunk node with no children would let a \
                 session run off the end of the trunk"
            ),
        ),
        Branching::Profile(segs) => {
            if segs.is_empty() {
                r.reject(
                    "8",
                    "an explicit branching profile must have at least one segment",
                );
            }
            if segs.first().map(|s| s.from_depth) != Some(0) {
                r.reject(
                    "8",
                    "the first branching segment must start at from_depth 0",
                );
            }
            let mut prev: Option<u32> = None;
            for s in segs {
                if s.fanout < 1.0 {
                    r.reject(
                        "8",
                        format!(
                            "branching segment at depth {} has fanout {} < 1",
                            s.from_depth, s.fanout
                        ),
                    );
                }
                if s.skew.is_some_and(|k| k < 0.0) {
                    r.reject(
                        "8",
                        format!(
                            "branching segment at depth {} has skew {} < 0",
                            s.from_depth,
                            s.skew.unwrap_or_default()
                        ),
                    );
                }
                if let Some(p) = prev {
                    if s.from_depth <= p {
                        r.reject(
                            "8",
                            format!(
                                "branching segments must ascend by from_depth; {} follows {}",
                                s.from_depth, p
                            ),
                        );
                    }
                }
                prev = Some(s.from_depth);
            }
        }
        // The node-level spelling had NO rule 8 at all, while `fit::segments::fit_process`
        // carried a comment about a first-band-at-depth-0 requirement — an invariant honoured
        // by the one writer and unchecked for every other. Same three clauses as the profile
        // arm, since `ResolvedSegments::at` resolves a band exactly as `Profile::at` resolves
        // a segment: a missing depth-0 band would silently take band 0's distributions for
        // every depth above it.
        Branching::Segments(p) => {
            if p.by_depth.is_empty() {
                r.reject(
                    "8",
                    "a node-level branching process must have at least one depth band",
                );
            }
            if p.by_depth.first().map(|b| b.from_depth) != Some(0) {
                r.reject("8", "the first branching band must start at from_depth 0");
            }
            let mut prev: Option<u32> = None;
            for b in &p.by_depth {
                if b.skew.is_some_and(|k| k < 0.0) {
                    r.reject(
                        "8",
                        format!(
                            "branching band at depth {} has skew {} < 0",
                            b.from_depth,
                            b.skew.unwrap_or_default()
                        ),
                    );
                }
                // A probability, and one the walk reads as "this fraction of arrivals goes
                // private here". Above 1 every session leaves at its first split and the trunk
                // has no sharing at all; below 0 is meaningless. Both are silent in the output
                // rather than loud, which is why they are rejected rather than clamped.
                if b.singleton_share.is_some_and(|q| !(0.0..=1.0).contains(&q)) {
                    r.reject(
                        "8",
                        format!(
                            "branching band at depth {} has singleton_share {} outside 0..=1",
                            b.from_depth,
                            b.singleton_share.unwrap_or_default()
                        ),
                    );
                }
                if let Some(prev) = prev {
                    if b.from_depth <= prev {
                        r.reject(
                            "8",
                            format!(
                                "branching bands must ascend by from_depth; {} follows {prev}",
                                b.from_depth
                            ),
                        );
                    }
                }
                prev = Some(b.from_depth);
            }
        }
        _ => {}
    }

    // Rule 16: a duration-valued window needs a rate, which only open_loop has.
    let window_is_duration = d.run.wss_window.as_ref().and_then(|v| v.as_str()).is_some();
    if window_is_duration && d.workload.arrival.model == ArrivalModel::ClosedLoop {
        r.reject(
            "15",
            "run.wss_window given as a duration together with closed_loop arrival: converting \
             it to a request count needs a configured rate, which only open_loop supplies. \
             State the window as a request count instead",
        );
    }

    // open_loop needs a rate; closed_loop needs concurrency.
    match d.workload.arrival.model {
        ArrivalModel::OpenLoop if d.workload.arrival.rate.is_none() => {
            r.reject("arrival", "open_loop arrival requires a `rate`");
        }
        ArrivalModel::ClosedLoop if d.workload.arrival.concurrency.is_none() => {
            r.reject(
                "arrival",
                "closed_loop arrival requires `concurrency`, its bound on in-flight sessions",
            );
        }
        _ => {}
    }

    // FR-015b: warmup must cover the session-population ramp, and the measured
    // window must exist at all. Both are rejections rather than warnings: the
    // numbers that follow describe the clock rather than the workload.
    warmup_window(d, &mut r);

    // Rule 10: replication cannot exceed the node count.
    if let (Some(topo), Some(rep)) = (
        &d.topology,
        d.topology.as_ref().and_then(|t| t.replication.as_ref()),
    ) {
        if let Some(mean) = rep.nodes_per_key.mean() {
            if mean > topo.nodes.len() as f64 {
                r.reject(
                    "10",
                    format!(
                        "replication.nodes_per_key mean {mean} exceeds the {} configured nodes",
                        topo.nodes.len()
                    ),
                );
            }
        }
    }

    // Rule 11: membership events must name a configured node.
    if let Some(topo) = &d.topology {
        for ev in &topo.membership_events {
            if !topo.nodes.contains(&ev.node) {
                r.reject(
                    "11",
                    format!(
                        "membership event names node `{}`, which is not in topology.nodes",
                        ev.node
                    ),
                );
            }
        }
    }

    // Rules 22 and 23: fan-out needs somewhere to go and is not half-configured.
    if let Some(sp) = &d.workload.sessions.spawn {
        let nodes = d.topology.as_ref().map(|t| t.nodes.len()).unwrap_or(1);
        let other_nodes = !matches!(
            sp.placement,
            Some(super::SpawnPlacement::SameNode) | Some(super::SpawnPlacement::Any)
        );
        if sp.fanout > 0 && other_nodes && nodes < 2 {
            r.reject(
                "22",
                "spawn.fanout > 0 with other_nodes placement needs at least two topology.nodes: \
                 there is nowhere else for a child to go",
            );
        }
        if sp.generations < 1 {
            r.reject("22", "spawn.generations must be at least 1");
        }
        if !(0.0..=1.0).contains(&sp.probability) {
            r.reject("22", "spawn.probability must be in [0, 1]");
        }
        if (sp.fanout > 0) != (sp.probability > 0.0) {
            r.reject(
                "22",
                "a half-configured fan-out silently does nothing: set both spawn.fanout and \
                 spawn.probability, or neither",
            );
        }
        if sp.fanout > 0 && d.topology.as_ref().map(|t| t.placement) == Some(Placement::PerRequest)
        {
            r.reject(
                "23",
                "spawn with per_request placement: per-request placement already scatters a \
                 session across nodes, so the fan-out's defining property -- an inherited prefix \
                 resident on one specific node -- does not hold, and the measurement would \
                 attribute to fan-out what placement caused",
            );
        }
    }

    // Rule 18: churn needs a clock.
    if let Some(ch) = &d.corpus.trees.churn {
        if !is_zero_duration(&ch.half_life) && d.duration.is_none() {
            r.reject(
                "18",
                "churn.half_life is a function of elapsed plan time, so a plan specified purely \
                 as a request or block count has no clock for it to be relative to: set `duration`",
            );
        }
    }

    // Fractions in range.
    if let Some(topo) = &d.topology {
        for (name, v) in [
            ("self_affinity", topo.self_affinity),
            ("cold_fraction", topo.cold_fraction),
        ] {
            if let Some(v) = v {
                if !(0.0..=1.0).contains(&v) {
                    r.reject("7", format!("topology.{name} must be in [0, 1], got {v}"));
                }
            }
        }
    }

    // Mixture weights must be positive and present if the section exists.
    if !d.workload.mix.is_empty() {
        let total: f64 = d.workload.mix.iter().map(|m| m.weight).sum();
        if total <= 0.0 {
            r.reject("mix", "mixture weights must sum to something positive");
        }
        if d.workload.mix.iter().any(|m| m.weight < 0.0) {
            r.reject("mix", "a mixture weight cannot be negative");
        }
    }

    // Rule 16: the occupancy floor. Last, because it depends on almost every
    // other section being sane, and reported even when it passes.
    occupancy_floor(d, &mut r);

    // Under-dispersed arrival is not modelled; say so rather than silently
    // substituting Poisson for something the document asked for.
    if let Some(b) = d.workload.arrival.burstiness {
        if b < 1.0 {
            r.warn(
                "arrival",
                format!(
                    "arrival.burstiness {b} asks for an under-dispersed process, which this \
                     generator does not model; arrivals will be Poisson (the neutral 1.0). \
                     Burstiness is an index of dispersion, so 1.0 means no burstiness rather \
                     than none meaning it"
                ),
            );
        }
    }

    r
}

/// Check `run.warmup` against the population ramp and against the run length.
///
/// [`check_warmup`] states FR-015b's rejection and is exercised by its own tests,
/// but nothing reached it: a rule that exists and cannot be triggered from the
/// command line protects nobody. Two ways the measured window goes wrong, and
/// they fail in opposite directions:
///
/// - **Warmup too short.** The window opens on a partly-filled session
///   population, so it sees less concurrency, less occupancy and less sharing
///   than configured — all of which read as properties of the workload.
/// - **Warmup longer than the run.** There is no measured window at all. Every
///   event is inside warmup, so a report over the steady state is a report over
///   nothing.
fn warmup_window(d: &Document, r: &mut Report) {
    let Some(warmup) = d.run.warmup.as_deref() else {
        return;
    };
    let Ok(warmup_ns) = parse_duration_ns(warmup) else {
        r.reject("warmup", format!("run.warmup `{warmup}` is not a duration"));
        return;
    };
    if let Some(dur) = d
        .duration
        .as_deref()
        .and_then(|s| parse_duration_ns(s).ok())
    {
        if warmup_ns >= dur {
            r.reject(
                "warmup",
                format!(
                    "run.warmup ({warmup}) covers the whole {dur_s:.1}s run, so there is no \
                     measured window: every event falls inside warmup and any steady-state \
                     figure would be computed over nothing",
                    dur_s = dur as f64 / 1e9
                ),
            );
        }
    }
    let rate = d
        .workload
        .arrival
        .rate
        .as_deref()
        .and_then(|s| parse_rate_per_s(s).ok());
    if let Some(pop) = Population::derive(&d.workload, rate) {
        if let Err(e) = check_warmup(&pop, warmup_ns as f64 / 1e9) {
            r.reject("15b", e.to_string());
        }
    }
}

/// The realised occupancy at `p99(shared_depth)`, and what it implies.
///
/// This is the one check that catches a configuration which is internally
/// consistent, passes every other rule, and still does not measure what it
/// claims to. A `shared_depth` is what a session *attempts*; it is realised only
/// if some earlier session walked the same steps. So a trunk wider than the
/// session population can occupy produces sessions landing on virgin trunk, and
/// the realised sharing is far below the drawn depth — which then reads as a
/// property of the workload rather than of the arithmetic.
///
/// **Both bands warn since 2026-08-15; neither rejects.** Below 1.0 means most sessions at
/// that depth are alone on their branch, and the generator now realises that faithfully by
/// letting a session leave the trunk when its expected cohort falls below two — so the
/// document is not claiming sharing it cannot deliver, it is delivering less. Below
/// [`TARGET_OCCUPANCY`] is thinner still. The prose above describes the world before
/// sharing was derived rather than drawn, and is kept because the arithmetic is unchanged:
/// what changed is that a thin trunk is now a fact about the workload rather than a defect
/// in the document.
///
/// When an input the arithmetic needs is missing, this **says so** rather than
/// passing quietly. A silently skipped check reads exactly like a check that
/// passed, and this is the one rule that catches a document which is wrong in a
/// way nothing else notices.
pub fn occupancy_floor(d: &Document, r: &mut Report) {
    let uncheckable = |r: &mut Report, why: &str| {
        r.warn(
            "16",
            format!(
                "trunk occupancy at p99(shared_depth) could not be checked: {why}. This is the \
                 one rule that catches a configuration which is internally consistent, passes \
                 every other check, and still does not measure what it claims to, so a skipped \
                 check is worth knowing about"
            ),
        );
    };
    let Some(mean_turns) = d.workload.sessions.turns.mean() else {
        uncheckable(
            r,
            "sessions.turns has no closed-form mean, so sessions per window is not a number",
        );
        return;
    };
    let mean_turns = mean_turns.max(1.0);
    let rate = d
        .workload
        .arrival
        .rate
        .as_deref()
        .and_then(|s| parse_rate_per_s(s).ok());
    // The window is canonically a request count; a duration needs a rate. Rule 15
    // already rejects the closed-loop case that reaches the duration branch.
    let (window_requests, window_source) = match super::wss_window_requests(d) {
        Ok(v) => v,
        Err(e) => {
            uncheckable(r, &e);
            return;
        }
    };
    let defaulted = window_source == super::WindowSource::Defaulted;
    let sessions_per_window = window_requests as f64 / mean_turns;
    let p99 = d.corpus.trees.shared_depth.quantile_u32(0.99);
    if p99 == 0 {
        uncheckable(
            r,
            "shared_depth has no closed-form p99 (a zipf shared_depth carries no support), or its \
             p99 is zero — in which case no sharing was asked for",
        );
        return;
    }
    let roots = d.corpus.trees.roots.count.max(1);
    // Resolve exactly as generation will, so the number checked here is the
    // number the run realises -- including whatever `auto` solves for.
    // Fanout *steps* to the deepest shared node, which is one less than the
    // depth: a shared prefix of depth s occupies ordinals 0..s (FR-014a), so the
    // walk from the root to its deepest node takes s-1 steps. p99 is at least 1
    // here, the zero case having been reported above.
    let trunk_steps = p99.saturating_sub(1);
    let corpus = Corpus::resolve(
        &d.corpus.trees,
        d.corpus.block_bytes.clone(),
        d.seed,
        sessions_per_window,
        trunk_steps,
    );
    // Churn-adjusted where churn is configured: a path accumulates sharers only
    // while it exists, so without this term the floor approves sharing that churn
    // then destroys.
    let half_life_ns = d
        .corpus
        .trees
        .churn
        .as_ref()
        .and_then(|c| parse_duration_ns(&c.half_life).ok())
        .unwrap_or(0);
    let window_ns = rate.map(|rate| (window_requests as f64 / rate * 1e9) as u64);
    let occ = occupancy(
        &corpus.profile,
        roots,
        sessions_per_window,
        trunk_steps,
        window_ns,
        half_life_ns,
    );
    let mut churn_note = String::new();
    if half_life_ns > 0 {
        churn_note.push_str(" (churn-adjusted: a path accumulates sharers only while it exists)");
    }
    if defaulted {
        // Occupancy scales linearly with the window, so a floor reported against
        // an unstated one is reported against an assumption; name it.
        churn_note.push_str(&format!(
            ", computed against the default window of {} requests, which the document does not state",
            super::DEFAULT_WSS_WINDOW_REQUESTS
        ));
    }
    if occ < 1.0 {
        // WARNS since 2026-08-15, where it used to reject.
        //
        // The rejection's premise was that a document could *ask* for sharing its trunk
        // cannot supply: `shared_depth` was what a session attempted, so a trunk wider
        // than the population could occupy left sessions on virgin trunk while the
        // document still claimed the sharing. That premise is gone. A session now leaves
        // the trunk when its own expected cohort falls below two
        // (`plan::generate::COHORT_FLOOR`), so a wide trunk does not produce unrealisable
        // sharing — it produces *less* sharing, correctly and visibly, and the realised
        // figure is what FR-056 compares.
        //
        // Keeping the rejection would refuse honest models: it rejected five corpus
        // traces at occupancy 0.23-0.25 once the deep trunk was fitted rather than
        // amputated, for a trunk those traces demonstrably have.
        r.warn(
            "16",
            format!(
                "trunk occupancy at p99(shared_depth) = {p99} is {occ:.2}{churn_note}: fewer than \
                 one session per distinct trunk path at that depth, so most sessions there are \
                 alone on their branch and will go private rather than share. That is realised \
                 faithfully rather than misreported — a session leaves the trunk when its \
                 expected cohort falls below two — but if you meant this run to exercise \
                 sharing, widen the population or narrow the trunk ({} roots at fanout {:.3})",
                roots,
                corpus.profile.fanout_at(1)
            ),
        );
    // `branching: auto` solves for exactly TARGET_OCCUPANCY (FR-009g), so its own
    // solution lands on the boundary and float rounding decides which side of it.
    // Without the tolerance the documented default warns every time, which is both
    // wrong and the fastest way to teach a reader to ignore rule 16.
    } else if occ < TARGET_OCCUPANCY * (1.0 - 1e-9) {
        r.warn(
            "16",
            format!(
                "trunk occupancy at p99(shared_depth) = {p99} is {occ:.2}{churn_note}, below the \
                 {TARGET_OCCUPANCY:.1} this generator designs against: sharing is realised but \
                 thin, so a hit rate measured here understates what the configured shared_depth \
                 suggests"
            ),
        );
    }
}

/// Whether a duration string denotes zero.
fn is_zero_duration(s: &str) -> bool {
    let t = s.trim();
    t == "0" || t.starts_with("0s") || t.starts_with("0m") || t.starts_with("0h")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn doc(extra: &str) -> Document {
        let base = r#"
version: 1
seed: 1
duration: 60s
corpus:
  block_bytes: 131072
  trees:
    roots: {count: 4, popularity: {dist: zipf, s: 0.9}}
    shared_depth: {dist: const, value: 8}
workload:
  arrival: {model: open_loop, rate: 1000/s}
  sessions:
    turns: {dist: geometric, mean: 4}
    think_time: {dist: const, value: 1}
    private_depth: {dist: const, value: 4}
    growth_per_turn: {dist: const, value: 2}
run:
  mode: hardware
"#;
        Document::from_yaml(&(base.to_string() + extra)).expect("test fixture must parse")
    }

    #[test]
    fn a_clean_document_passes() {
        assert!(!validate(&doc("")).is_rejected());
    }

    /// A `corpus.trees.branching` clause appended to the base fixture.
    fn branching_doc(branching: &str) -> Document {
        let y = format!(
            r#"
version: 1
seed: 1
duration: 60s
corpus:
  block_bytes: 131072
  trees:
    roots: {{count: 4, popularity: {{dist: zipf, s: 0.9}}}}
    shared_depth: {{dist: const, value: 8}}
    branching: {branching}
workload:
  arrival: {{model: open_loop, rate: 1000/s}}
  sessions:
    turns: {{dist: geometric, mean: 4}}
    think_time: {{dist: const, value: 1}}
    private_depth: {{dist: const, value: 4}}
    growth_per_turn: {{dist: const, value: 2}}
run:
  mode: hardware
"#
        );
        Document::from_yaml(&y).expect("test fixture must parse")
    }

    /// Rule 8's rejection messages for one branching clause, concatenated.
    fn rule_8(branching: &str) -> String {
        validate(&branching_doc(branching))
            .rejections()
            .filter(|f| f.rule == "8")
            .map(|f| f.message.clone())
            .collect()
    }

    #[test]
    fn a_node_level_branching_process_is_checked_the_way_a_profile_is() {
        // The `Segments` spelling had NO rule 8 at all, while `fit::segments::fit_process`
        // carried a comment about a first-band-at-depth-0 requirement — an invariant the one
        // writer honoured and nothing checked. `ResolvedSegments::at` resolves a band exactly
        // as `Profile::at` resolves a segment, so a missing depth-0 band silently applies
        // band 0's distributions to every depth above it.
        let band = |from: u32| {
            format!("{{from_depth: {from}, length: {{dist: const, value: 4}}, out_degree: {{dist: const, value: 3}}}}")
        };
        let ok = format!("{{by_depth: [{}, {}]}}", band(0), band(8));
        assert_eq!(rule_8(&ok), "", "two ascending bands from depth 0 are fine");

        assert!(
            rule_8("{by_depth: []}").contains("at least one depth band"),
            "an empty band list must be rejected: {}",
            rule_8("{by_depth: []}")
        );
        let deep = format!("{{by_depth: [{}]}}", band(4));
        assert!(
            rule_8(&deep).contains("must start at from_depth 0"),
            "{}",
            rule_8(&deep)
        );
        let descending = format!("{{by_depth: [{}, {}]}}", band(0), band(0));
        assert!(
            rule_8(&descending).contains("must ascend by from_depth"),
            "{}",
            rule_8(&descending)
        );
    }

    #[test]
    fn a_negative_child_skew_is_rejected_in_both_trunk_spellings() {
        // `skew` is a Zipf exponent, and `pick_child_p` reads anything <= 0 as uniform, so a
        // negative value is not a steeper law — it is a document meaning something the
        // generator cannot express. Checked in both spellings because both now carry one.
        let profile = "[{from_depth: 0, fanout: 2.0, skew: -1.0}]";
        assert!(
            rule_8(profile).contains("skew -1 < 0"),
            "{}",
            rule_8(profile)
        );
        let segments = "{by_depth: [{from_depth: 0, length: {dist: const, value: 4}, \
                        out_degree: {dist: const, value: 3}, skew: -0.5}]}";
        assert!(
            rule_8(segments).contains("skew -0.5 < 0"),
            "{}",
            rule_8(segments)
        );
        // Zero is uniform descent, which is a real law and must be accepted — `ragbench`'s
        // 2498 deep splits are measured exactly uniform.
        let zero = "[{from_depth: 0, fanout: 2.0, skew: 0.0}]";
        assert_eq!(rule_8(zero), "");
    }

    #[test]
    fn a_popularity_support_narrower_than_roots_count_is_rejected() {
        // The defect this rule exists for, in the exact shape a fit emitted for months:
        // roots.count 603 with an empirical support reaching rank 153, which left 450
        // roots unreachable and recorded ZERO clamps, because `sample_u64_clamped` only
        // counts draws pulled *into* range. The realised root layer was 5.
        let mut d = doc("");
        d.corpus.trees.roots.count = 603;
        d.corpus.trees.roots.popularity = crate::dist::Dist::Shaped(Shape::Empirical {
            points: vec![(1.0, 0.0), (1.0, 0.61), (153.0, 0.61), (153.0, 1.0)],
        });
        let r = validate(&d);
        let msg: String = r
            .rejections()
            .filter(|f| f.rule == "8")
            .map(|f| f.message.clone())
            .collect();
        assert!(msg.contains("support reaches rank 153"), "{msg}");
        assert!(msg.contains("roots.count is 603"), "{msg}");
        assert!(msg.contains("records no clamp"), "{msg}");

        // And the same document with a support spanning the count is accepted, so this
        // is a check on the support rather than on empirical popularity as such.
        d.corpus.trees.roots.popularity = crate::dist::Dist::Shaped(Shape::Empirical {
            points: vec![(1.0, 0.0), (1.0, 0.61), (603.0, 0.61), (603.0, 1.0)],
        });
        assert!(
            !validate(&d).rejections().any(|f| f.rule == "8"),
            "a support spanning roots.count must pass"
        );
    }

    #[test]
    fn an_empirical_cdf_that_does_not_reach_one_is_rejected() {
        // Rule 9, documented since the first draft and unimplemented until 2026-08-14.
        // A CDF stopping at 0.8 makes every draw above it return the top point, so a
        // fifth of the mass silently collapses onto one value.
        let mut d = doc("");
        d.corpus.trees.shared_depth = crate::dist::Dist::Shaped(Shape::Empirical {
            points: vec![(1.0, 0.0), (1.0, 0.4), (9.0, 0.4), (9.0, 0.8)],
        });
        let msg: String = validate(&d)
            .rejections()
            .filter(|f| f.rule == "9")
            .map(|f| f.message.clone())
            .collect();
        assert!(msg.contains("final cumulative probability is 0.8"), "{msg}");

        // Descending values are rejected too, and the STEP encoding `fit` emits — which
        // repeats each value on purpose — must not be: that is the whole reason the check
        // is non-decreasing rather than strictly ascending.
        let mut back = doc("");
        back.corpus.trees.shared_depth = crate::dist::Dist::Shaped(Shape::Empirical {
            points: vec![(9.0, 0.0), (9.0, 0.5), (1.0, 0.5), (1.0, 1.0)],
        });
        assert!(validate(&back).rejections().any(|f| f.rule == "9"));
        let mut steps = doc("");
        steps.corpus.trees.shared_depth = crate::dist::Dist::Shaped(Shape::Empirical {
            points: vec![(1.0, 0.0), (1.0, 0.4), (9.0, 0.4), (9.0, 1.0)],
        });
        assert!(
            !validate(&steps).rejections().any(|f| f.rule == "9"),
            "the step encoding repeats each value and must pass"
        );
    }

    #[test]
    fn an_n_supplied_to_roots_popularity_is_rejected() {
        // Documented in the contract — "supplying `n` here is a schema error", because
        // the support is `roots.count` — and unimplemented until 2026-08-14. Measured
        // before the fix: a document with `n: 10` produced zero rejections and the
        // generator silently overwrote it, realising 602 roots.
        let d = doc("");
        let mut d = d;
        d.corpus.trees.roots.popularity = crate::dist::Dist::Shaped(Shape::Zipf {
            s: 0.9,
            n: Some(10),
        });
        let r = validate(&d);
        let msg: String = r
            .rejections()
            .filter(|f| f.rule == "8")
            .map(|f| f.message.clone())
            .collect();
        assert!(msg.contains("not the author's to choose"), "{msg}");
        // A Zipf without `n` is the normal case and must still pass.
        let mut ok = doc("");
        ok.corpus.trees.roots.popularity =
            crate::dist::Dist::Shaped(Shape::Zipf { s: 0.9, n: None });
        assert!(!validate(&ok).rejections().any(|f| f.rule == "8"));
    }

    #[test]
    fn every_violation_is_reported_not_just_the_first() {
        // Three independent problems; a reader fixing one at a time would need
        // three round trips.
        let d = doc("topology:\n  nodes: [a]\n  self_affinity: 5.0\n  membership_events: [{at: 1s, action: stop, node: zzz}]\n");
        let mut d = d;
        d.corpus.trees.roots.count = 0;
        let r = validate(&d);
        assert!(r.rejections().count() >= 3, "got {:?}", r.findings);
    }

    #[test]
    fn file_output_requires_a_block_budget() {
        let mut d = doc("");
        d.run.mode = "jsonl".into();
        let r = validate(&d);
        assert!(r.rejections().any(|f| f.rule == "20"));
    }

    #[test]
    fn unbounded_is_refused_for_file_output() {
        let mut d = doc("");
        d.run.mode = "jsonl".into();
        d.duration = None;
        d.blocks = Some(1000);
        d.unbounded = Some(true);
        let r = validate(&d);
        assert!(r.rejections().any(|f| f.rule == "21" || f.rule == "19"));
    }

    #[test]
    fn exactly_one_run_length() {
        let mut d = doc("");
        d.requests = Some(100); // duration already set
        assert!(validate(&d).rejections().any(|f| f.rule == "19"));
        let mut d2 = doc("");
        d2.duration = None;
        assert!(validate(&d2).rejections().any(|f| f.rule == "19"));
    }

    #[test]
    fn a_duration_window_under_closed_loop_is_refused() {
        let mut d = doc("");
        d.workload.arrival.model = ArrivalModel::ClosedLoop;
        d.workload.arrival.concurrency = Some(64);
        d.workload.arrival.rate = None;
        d.run.wss_window = Some(serde_yaml::Value::String("60s".into()));
        assert!(validate(&d).rejections().any(|f| f.rule == "15"));
    }

    #[test]
    fn fanout_with_nowhere_to_go_is_refused() {
        let d = doc("topology:\n  nodes: [only]\n");
        let mut d = d;
        d.workload.sessions.spawn = Some(super::super::Spawn {
            fanout: 4,
            probability: 0.5,
            at_turn: None,
            depth: None,
            generations: 1,
            placement: None,
        });
        assert!(validate(&d).rejections().any(|f| f.rule == "22"));
    }

    #[test]
    fn half_configured_fanout_is_refused() {
        let mut d = doc("topology:\n  nodes: [a, b]\n");
        d.workload.sessions.spawn = Some(super::super::Spawn {
            fanout: 4,
            probability: 0.0, // set one, not the other: silently does nothing
            at_turn: None,
            depth: None,
            generations: 1,
            placement: None,
        });
        assert!(validate(&d).rejections().any(|f| f.rule == "22"));
    }

    #[test]
    fn fanout_with_per_request_placement_is_refused() {
        let mut d = doc("topology:\n  nodes: [a, b]\n  placement: per_request\n");
        d.workload.sessions.spawn = Some(super::super::Spawn {
            fanout: 4,
            probability: 0.5,
            at_turn: None,
            depth: None,
            generations: 1,
            placement: None,
        });
        assert!(validate(&d).rejections().any(|f| f.rule == "23"));
    }

    #[test]
    fn churn_without_a_clock_is_refused() {
        let mut d = doc("");
        d.duration = None;
        d.requests = Some(1_000);
        d.corpus.trees.churn = Some(super::super::Churn {
            half_life: "6h".into(),
        });
        assert!(validate(&d).rejections().any(|f| f.rule == "18"));
    }

    #[test]
    fn replication_beyond_the_node_count_is_refused() {
        let d = doc("topology:\n  nodes: [a, b]\n  replication: {nodes_per_key: 5}\n");
        assert!(validate(&d).rejections().any(|f| f.rule == "10"));
    }

    #[test]
    fn a_wide_uniform_fanout_is_judged_by_computed_occupancy_not_by_its_value() {
        // A fanout is not wrong for being large; it is wrong for outrunning the
        // session population, and that is arithmetic rather than a threshold on
        // the number itself. The same 2.0 that is fatal at depth 40 is harmless
        // at depth 4, which no rule reading the fanout alone could express.
        // Both bands WARN since 2026-08-15 rather than rejecting, so the assertion is
        // on the finding and its band, not on rejection. The property being pinned is
        // unchanged: the same fanout is judged differently at different depths.
        let deep = validate(&occ_doc(12, "2.0", 40, 240_000));
        let f = deep
            .findings
            .iter()
            .find(|f| f.rule == "16")
            .expect("rule 16 must speak at depth 40");
        assert_eq!(f.severity, Severity::Warn);
        assert!(f.message.contains("alone on their branch"), "{}", f.message);
        let shallow = validate(&occ_doc(12, "2.0", 4, 240_000));
        assert!(
            !shallow
                .findings
                .iter()
                .any(|f| f.rule == "16" && f.message.contains("alone on their branch")),
            "{:?}",
            shallow.findings
        );
    }

    #[test]
    fn a_descending_branching_profile_is_refused() {
        let mut d = doc("");
        d.corpus.trees.branching = Branching::Profile(vec![
            super::super::Segment {
                from_depth: 0,
                fanout: 1.0,
                skew: None,
                churn_half_life: None,
            },
            super::super::Segment {
                from_depth: 0,
                fanout: 2.0,
                skew: None,
                churn_half_life: None,
            },
        ]);
        assert!(validate(&d).rejections().any(|f| f.rule == "8"));
    }

    /// A document with the occupancy floor's inputs actually present: the floor
    /// needs a window and a rate, which the minimal fixture above omits.
    fn occ_doc(roots: u32, branching: &str, shared_depth: u32, window: u64) -> Document {
        let y = format!(
            r#"
version: 1
seed: 1
duration: 60s
corpus:
  block_bytes: 131072
  trees:
    roots: {{count: {roots}, popularity: {{dist: zipf, s: 0.9}}}}
    shared_depth: {{dist: const, value: {shared_depth}}}
    branching: {branching}
workload:
  arrival: {{model: open_loop, rate: 4000/s}}
  sessions:
    turns: {{dist: geometric, mean: 6}}
    think_time: {{dist: const, value: 3}}
    private_depth: {{dist: const, value: 8}}
    growth_per_turn: {{dist: const, value: 6}}
run:
  mode: hardware
  wss_window: {window}
"#
        );
        Document::from_yaml(&y).expect("occupancy fixture must parse")
    }

    #[test]
    fn an_unoccupiable_trunk_is_reported_and_no_longer_refused() {
        // Fanout 2.0 over 40 depths is 2^40 paths from each of 12 roots against ~40k
        // sessions per window, so almost every session is alone on its branch there.
        //
        // That used to REJECT, on the premise that a document could ask for sharing its
        // trunk cannot supply — `shared_depth` was what a session attempted, and the
        // attempt could be fiction. Since sharing became derived (2026-08-15) the premise
        // is gone: a session leaves the trunk when its expected cohort falls below two, so
        // a wide trunk yields *less* sharing rather than misreported sharing, and the
        // realised figure is what FR-056 compares. Keeping the rejection refused five
        // corpus traces for a trunk they demonstrably have.
        let d = occ_doc(12, "2.0", 40, 240_000);
        let r = validate(&d);
        assert!(
            !r.rejections().any(|f| f.rule == "16"),
            "rule 16 must not reject: {:?}",
            r.findings
        );
        let f = r
            .findings
            .iter()
            .find(|f| f.rule == "16")
            .expect("but it must still speak");
        assert_eq!(f.severity, Severity::Warn);
        assert!(
            f.message.contains("p99(shared_depth) = 40"),
            "{}",
            f.message
        );
        assert!(
            f.message.contains("expected cohort falls below two"),
            "the warning must say what the generator will actually do: {}",
            f.message
        );
    }

    #[test]
    fn thin_but_real_occupancy_warns_rather_than_rejects() {
        // Still measurable, so warn: the numbers are noisy rather than wrong,
        // which is the whole reject-versus-warn distinction.
        //
        // 240k requests over mean 6 turns is 40k sessions a window; 12 roots at
        // fanout 1.2 give 12 x 1.2^40 = 17.6k paths at depth 40, so occupancy is
        // about 2.3 -- real, and under the 4.0 this generator designs against.
        let r = validate(&occ_doc(12, "1.2", 40, 240_000));
        assert!(!r.is_rejected(), "{:?}", r.findings);
        let f = r.findings.iter().find(|f| f.rule == "16").expect("rule 16");
        assert_eq!(f.severity, Severity::Warn);
        assert!(f.message.contains("thin"), "{}", f.message);
        assert!(f.message.contains("understates"), "{}", f.message);
    }

    #[test]
    fn a_comfortably_occupied_trunk_says_nothing() {
        // One root, flat trunk, 40k sessions a window: occupancy is enormous, and
        // a rule that fired here would be noise rather than signal.
        let r = validate(&occ_doc(1, "1.0", 18, 240_000));
        assert!(
            !r.findings.iter().any(|f| f.rule == "16"),
            "{:?}",
            r.findings
        );
    }

    #[test]
    fn branching_auto_lands_inside_its_own_floor() {
        // FR-009g solves for TARGET_OCCUPANCY at p99(shared_depth), so `auto`
        // tripping rule 16 would mean the closed form and the check disagree.
        // Warnings count, not just rejections: `auto` lands *on* the target, so a
        // strict comparison warned on the documented default every time — the
        // check contradicting the solver it is checking.
        for roots in [1u32, 12, 64] {
            for depth in [4u32, 18, 40] {
                let r = validate(&occ_doc(roots, "auto", depth, 240_000));
                assert!(
                    !r.findings.iter().any(|f| f.rule == "16"),
                    "auto flagged at roots={roots} depth={depth}: {:?}",
                    r.findings
                );
            }
        }
    }

    #[test]
    fn churn_tightens_the_floor_rather_than_being_ignored_by_it() {
        // Rule 16's churn clause: a half-life short against the window shortens
        // the interval over which a path can accumulate sharers, so a
        // configuration that passes without churn can fail with it.
        let base = occ_doc(1, "1.0", 40, 240_000);
        assert!(!validate(&base).rejections().any(|f| f.rule == "16"));
        let mut churned = base.clone();
        churned.corpus.trees.churn = Some(super::super::Churn {
            half_life: "50ms".into(),
        });
        let r = validate(&churned);
        let f = r
            .findings
            .iter()
            .find(|f| f.rule == "16")
            .expect("churn should have moved the floor");
        assert!(f.message.contains("churn-adjusted"), "{}", f.message);
    }

    #[test]
    fn an_unstated_window_is_defaulted_and_the_finding_says_so() {
        // The check still runs -- a skipped check reads exactly like a check that
        // passed -- but occupancy scales linearly with the window, so a floor
        // reported against an unstated one is reported against an assumption.
        let mut d = occ_doc(12, "2.0", 40, 240_000);
        d.run.wss_window = None;
        let r = validate(&d);
        let f = r
            .findings
            .iter()
            .find(|f| f.rule == "16")
            .expect("rule 16 still runs; it warns rather than rejects");
        assert!(
            f.message.contains("default window of 240000"),
            "{}",
            f.message
        );
    }

    #[test]
    fn the_validator_and_the_generator_default_the_window_identically() {
        // Two different defaults would let a document pass validation and then be
        // generated against a different check than the one it passed.
        let mut d = occ_doc(12, "1.2", 40, 240_000);
        let stated = validate(&d)
            .findings
            .iter()
            .find(|f| f.rule == "16")
            .map(|f| f.severity);
        d.run.wss_window = None;
        let defaulted = validate(&d)
            .findings
            .iter()
            .find(|f| f.rule == "16")
            .map(|f| f.severity);
        assert_eq!(stated, defaulted);
        assert_eq!(super::super::DEFAULT_WSS_WINDOW_REQUESTS, 240_000);
    }

    #[test]
    fn a_floor_that_genuinely_cannot_be_computed_is_reported_as_such() {
        // A zipf shared_depth carries no support, so its p99 is not a number and
        // inventing one would make the floor a statement about a different corpus.
        let mut d = occ_doc(12, "1.2", 40, 240_000);
        d.corpus.trees.shared_depth =
            crate::dist::Dist::Shaped(crate::dist::Shape::Zipf { s: 0.9, n: None });
        let r = validate(&d);
        let f = r.findings.iter().find(|f| f.rule == "16").expect("rule 16");
        assert_eq!(f.severity, Severity::Warn);
        assert!(f.message.contains("could not be checked"), "{}", f.message);
    }

    #[test]
    fn under_dispersed_burstiness_warns_that_it_is_not_modelled() {
        let mut d = doc("");
        d.workload.arrival.burstiness = Some(0.4);
        let r = validate(&d);
        assert!(!r.is_rejected());
        let f = r
            .findings
            .iter()
            .find(|f| f.message.contains("under-dispersed"))
            .unwrap();
        assert_eq!(f.rule, "arrival", "not a numbered rule in the contract");
        assert!(f.message.contains("index of dispersion"), "{}", f.message);
    }

    #[test]
    fn a_warmup_shorter_than_the_population_ramp_is_reachable_from_validate() {
        // FR-015b was implemented and tested in `session.rs` and reachable from
        // nowhere. A rule that cannot be triggered protects nobody, so what is
        // asserted here is the wiring rather than the arithmetic.
        let mut d = occ_doc(12, "1.2", 40, 240_000);
        // mean 6 turns with a 3s think time is a ~15s ramp.
        d.run.warmup = Some("2s".into());
        let r = validate(&d);
        let f = r.rejections().find(|f| f.rule == "15b").expect("rule 15b");
        assert!(f.message.contains("measures the ramp"), "{}", f.message);
        // And a warmup that covers the ramp passes.
        d.run.warmup = Some("20s".into());
        d.duration = Some("120s".into());
        assert!(!validate(&d).rejections().any(|f| f.rule == "15b"));
    }

    #[test]
    fn a_warmup_covering_the_whole_run_leaves_no_measured_window() {
        // The opposite failure, and the one a hand-written config falls into: a
        // 20s warmup on a 10s run means every event is warmup, so a steady-state
        // figure is computed over nothing.
        let mut d = occ_doc(12, "1.2", 40, 240_000);
        d.duration = Some("10s".into());
        d.run.warmup = Some("20s".into());
        let r = validate(&d);
        let f = r
            .rejections()
            .find(|f| f.rule == "warmup")
            .expect("empty measured window");
        assert!(f.message.contains("no measured window"), "{}", f.message);
        assert!(f.message.contains("computed over nothing"), "{}", f.message);
    }

    #[test]
    fn an_unparseable_warmup_is_refused_rather_than_treated_as_absent() {
        let mut d = occ_doc(12, "1.2", 40, 240_000);
        d.run.warmup = Some("20 seconds".into());
        assert!(validate(&d).rejections().any(|f| f.rule == "warmup"));
    }

    #[test]
    fn an_unimplemented_version_is_refused() {
        let mut d = doc("");
        d.version = 99;
        assert!(validate(&d).rejections().any(|f| f.rule == "version"));
    }

    /// A document whose `growth_per_turn` is the banded spelling (FR-054f).
    ///
    /// `bands` is `(from_turns, blocks per turn)`; an empty slice emits `by_turns: []`,
    /// which is the degenerate table rule 24 has to catch.
    fn banded_doc(bands: &[(u32, u32)]) -> Document {
        let table = if bands.is_empty() {
            " []".to_string()
        } else {
            let mut s = String::new();
            for (from, growth) in bands {
                s.push_str(&format!(
                    "\n      - {{from_turns: {from}, growth: {{dist: const, value: {growth}}}}}"
                ));
            }
            s
        };
        let y = format!(
            r#"
version: 1
seed: 1
duration: 60s
corpus:
  block_bytes: 131072
  trees:
    roots: {{count: 4, popularity: {{dist: zipf, s: 0.9}}}}
    shared_depth: {{dist: const, value: 8}}
workload:
  arrival: {{model: open_loop, rate: 1000/s}}
  sessions:
    turns: {{dist: geometric, mean: 4}}
    think_time: {{dist: const, value: 1}}
    private_depth: {{dist: const, value: 4}}
    growth_per_turn:
      by_turns:{table}
run:
  mode: hardware
"#
        );
        Document::from_yaml(&y).expect("banded fixture must parse")
    }

    /// Rule 24's message for a document, or `None` if it did not fire.
    fn rule_24(d: &Document) -> Option<String> {
        validate(d)
            .rejections()
            .find(|f| f.rule == "24")
            .map(|f| f.message.clone())
    }

    #[test]
    fn a_banded_growth_table_is_accepted_and_the_bare_spelling_still_is() {
        // The untagged enum is what keeps every pre-FR-054f document working, so both
        // spellings are asserted together: `doc("")` above uses the bare one.
        assert!(!validate(&doc("")).is_rejected());
        let d = banded_doc(&[(1, 2), (8, 20)]);
        let r = validate(&d);
        assert!(!r.is_rejected(), "{:?}", r.findings);
    }

    #[test]
    fn a_growth_table_that_starts_above_one_turn_is_refused() {
        // Rule 24. A session shorter than the first band names no band at all;
        // `Growth::at` clamps it into the first one, and a table that starts higher is
        // then silently disagreeing with what it says.
        let m = rule_24(&banded_doc(&[(4, 2)])).expect("rule 24 should fire");
        assert!(m.contains("must start at 1"), "{m}");
    }

    #[test]
    fn a_growth_table_whose_bands_do_not_ascend_is_refused() {
        let m = rule_24(&banded_doc(&[(1, 2), (8, 20), (8, 30)])).expect("rule 24 should fire");
        assert!(m.contains("must ascend"), "{m}");
    }

    #[test]
    fn an_empty_growth_table_is_refused() {
        // `by_turns: []` parses — an empty list is a valid list — and would panic in
        // `Growth::at`, which indexes the first band. Rule 24 is what stops it here.
        let m = rule_24(&banded_doc(&[])).expect("rule 24 should fire");
        assert!(m.contains("is empty"), "{m}");
    }

    #[test]
    fn a_mixture_arms_growth_table_is_checked_too() {
        // An arm's override is a `growth_per_turn` like any other, and a document whose
        // arms went unchecked would put exactly the sessions the arm describes in the
        // wrong band.
        let mut d = banded_doc(&[(1, 2), (8, 20)]);
        let bad = match banded_doc(&[(4, 2)]).workload.sessions.growth_per_turn {
            super::Growth::Banded(b) => b,
            super::Growth::Uniform(_) => panic!("fixture is banded"),
        };
        d.workload.mix = vec![super::super::MixEntry {
            turn1_path_length: None,
            weight: 1.0,
            turns: None,
            think_time: None,
            private_depth: None,
            growth_per_turn: Some(super::Growth::Banded(bad)),
        }];
        let m = rule_24(&d).expect("rule 24 should fire for an arm");
        assert!(m.contains("workload.mix[0]"), "{m}");
    }
}
