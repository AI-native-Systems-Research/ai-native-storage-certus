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

    // Warn where the configuration is usable but likely not what was meant.
    if let Branching::Uniform(f) = &d.corpus.trees.branching {
        if *f > 1.25 {
            r.warn(
                "16",
                format!(
                    "a uniform fanout of {f} widens the trunk fast; check occupancy at p99 \
                     shared_depth, since sessions landing on virgin trunk realise far less \
                     sharing than they ask for"
                ),
            );
        }
    }

    r
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
    fn a_wide_uniform_fanout_warns_rather_than_rejects() {
        // Usable but likely wrong: still measurable, so warn.
        let mut d = doc("");
        d.corpus.trees.branching = Branching::Uniform(2.0);
        let r = validate(&d);
        assert!(!r.is_rejected());
        assert!(r.findings.iter().any(|f| f.severity == Severity::Warn));
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

    #[test]
    fn an_unimplemented_version_is_refused() {
        let mut d = doc("");
        d.version = 99;
        assert!(validate(&d).rejections().any(|f| f.rule == "version"));
    }
}
