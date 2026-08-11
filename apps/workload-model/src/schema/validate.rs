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

use super::{ArrivalModel, Branching, Document, Placement};
use crate::corpus::{occupancy, Corpus, TARGET_OCCUPANCY};
use crate::session::{check_warmup, Population};
use crate::units::{count_from_yaml, parse_duration_ns, parse_rate_per_s};

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

    // Rule 8: roots and branching domains.
    if d.corpus.trees.roots.count < 1 {
        r.reject("8", "roots.count must be at least 1");
    }
    if d.corpus.trees.branch_skew < 0.0 {
        r.reject("8", "branch_skew must be >= 0");
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
/// Below **1.0** rejects: on average not one other session has been down the
/// path, so the sharing the document describes does not exist. Below
/// [`TARGET_OCCUPANCY`] warns: still measurable, just thin.
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
    // The window is canonically a request count; a duration needs a rate.
    let mut defaulted = false;
    let window_requests = match d.run.wss_window.as_ref() {
        Some(v) => match count_from_yaml(v) {
            Some(n) => n,
            None => match (v.as_str().and_then(|s| parse_duration_ns(s).ok()), rate) {
                (Some(ns), Some(rate)) => (ns as f64 / 1e9 * rate) as u64,
                // Rule 15 already rejects the closed-loop case that gets here.
                _ => {
                    uncheckable(
                        r,
                        "run.wss_window is a duration and no rate is configured to convert it",
                    );
                    return;
                }
            },
        },
        None => {
            defaulted = true;
            super::DEFAULT_WSS_WINDOW_REQUESTS
        }
    };
    if window_requests == 0 {
        uncheckable(r, "run.wss_window is zero");
        return;
    }
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
    let corpus = Corpus::resolve(
        &d.corpus.trees,
        d.corpus.block_bytes.clone(),
        d.seed,
        sessions_per_window,
        p99,
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
        p99,
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
        r.reject(
            "16",
            format!(
                "trunk occupancy at p99(shared_depth) = {p99} is {occ:.2}{churn_note}: fewer than \
                 one session per distinct trunk path, so sessions land on virgin trunk and the \
                 sharing this document describes is not realised. Widen the population, narrow \
                 the trunk ({} roots at fanout {:.3}), or reduce shared_depth",
                roots,
                corpus.profile.fanout_at(1)
            ),
        );
    } else if occ < TARGET_OCCUPANCY {
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
        let deep = validate(&occ_doc(12, "2.0", 40, 240_000));
        assert!(
            deep.rejections().any(|f| f.rule == "16"),
            "{:?}",
            deep.findings
        );
        let shallow = validate(&occ_doc(12, "2.0", 4, 240_000));
        assert!(
            !shallow.rejections().any(|f| f.rule == "16"),
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
    fn an_unoccupiable_trunk_is_rejected_rather_than_measured() {
        // Rule 16, the reject half. Fanout 2.0 over 40 depths is 2^40 paths from
        // each of 12 roots against ~40k sessions per window: every session walks
        // virgin trunk, so the shared_depth it drew is fiction.
        let d = occ_doc(12, "2.0", 40, 240_000);
        let r = validate(&d);
        assert!(r.is_rejected());
        let f = r.rejections().find(|f| f.rule == "16").expect("rule 16");
        assert!(f.message.contains("virgin trunk"), "{}", f.message);
        assert!(f.message.contains("p99(shared_depth) = 40"));
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
        for roots in [1u32, 12, 64] {
            for depth in [4u32, 18, 40] {
                let r = validate(&occ_doc(roots, "auto", depth, 240_000));
                assert!(
                    !r.rejections().any(|f| f.rule == "16"),
                    "auto rejected at roots={roots} depth={depth}: {:?}",
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
        let f = r.rejections().find(|f| f.rule == "16").expect("rule 16");
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
}
