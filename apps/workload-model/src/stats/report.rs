//! The plan report: every FR-034a statistic, assembled in one pass.
//!
//! [`Statistics`] drives the eight accumulators over a reference stream and
//! [`Report`] is what they produce. FR-034a is the single normative enumeration of
//! what a report contains, and this module holds it in exactly that order so a
//! reader can check the two against each other.
//!
//! # What a report deliberately does not contain
//!
//! No hit rate, at any capacity, under any policy (FR-034). No Belady/OPT figure:
//! it evicts furthest-next-use *when full*, so it is a curve over a capacity this
//! crate does not know, and it defers with the rest of cache simulation (FR-034b).
//! No eviction counts, no promotion traffic, no byte provenance — each of those
//! needs a model of the consumer's internals, so each is something a consumer
//! reports and this crate cannot express.
//!
//! What the reuse-distance CDF gives a reader instead is better than a hit rate:
//! the whole curve, from which any capacity point can be read off without this
//! tool having modelled a cache to produce it.

use serde::{Deserialize, Serialize};

use super::floor::{Floor, FloorReport};
use super::length::{LengthReport, RequestLength};
use super::reuse_distance::{ReuseDistance, ReuseDistanceReport};
use super::sharing::{Sharing, SharingReport};
use super::trunk::{Trunk, TrunkReport};
use super::unique::{UniqueKeys, UniqueKeysReport};
use super::wss::{WorkingSet, WorkingSetReport};
use super::{KeyTable, Ref, WindowTable};
use crate::dist::Dist;

/// Warmup activity, counted separately from everything measured (spec FR-045).
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct WarmupCounts {
    /// Warmup references.
    pub references: u64,
    /// Warmup requests.
    pub requests: u64,
    /// Warmup bytes.
    pub bytes: u128,
    /// Distinct keys warmup reached — the set whose compulsory cost it paid.
    pub distinct_keys: u64,
}

/// Drives every statistic over one reference stream.
///
/// One pass, one shared key table. Push references in stream order; a request is
/// closed by the next `request_start` or by [`Statistics::finish`].
#[derive(Debug)]
pub struct Statistics {
    keys: KeyTable,
    window: WindowTable,
    reuse: ReuseDistance,
    floor: Floor,
    sharing: Sharing,
    length: RequestLength,
    unique: UniqueKeys,
    trunk: Trunk,
    working_set: WorkingSet,
    warmup: WarmupCounts,
    window_requests: u64,
    requests_in_window: u64,
    requests: u64,
    references: u64,
    bytes: u128,
}

impl Statistics {
    /// An accumulator with windows of `window_requests` requests.
    ///
    /// The caller passes the window rather than defaulting it here, so that the
    /// value used to validate a document and the value used to characterise its
    /// plan cannot differ — two defaults would let a document pass validation and
    /// then be measured against a different check.
    pub fn new(window_requests: u64) -> Statistics {
        Statistics {
            keys: KeyTable::new(),
            window: WindowTable::new(),
            reuse: ReuseDistance::new(),
            floor: Floor::new(),
            sharing: Sharing::new(),
            length: RequestLength::new(),
            unique: UniqueKeys::new(),
            trunk: Trunk::new(),
            working_set: WorkingSet::new(window_requests.max(1)),
            warmup: WarmupCounts::default(),
            window_requests: window_requests.max(1),
            requests_in_window: 0,
            requests: 0,
            references: 0,
            bytes: 0,
        }
    }

    /// Record one reference.
    pub fn push(&mut self, r: &Ref) {
        let facts = self.keys.observe(r);

        // Both of these see every reference. A warmup fetch really did occupy the
        // consumer's capacity, so it sits inside the reuse distance of whatever
        // follows it, and it means the key it fetched is no longer a compulsory
        // miss. Each gates its own *samples* on the warmup flag (FR-045).
        self.reuse.observe(r, &facts);
        self.floor.observe(r, &facts);

        if r.warmup {
            if r.request_start {
                self.warmup.requests += 1;
            }
            self.warmup.references += 1;
            self.warmup.bytes += u128::from(facts.entry_size);
            if facts.first_touch {
                self.warmup.distinct_keys += 1;
            }
            return;
        }

        if r.request_start {
            self.close_request();
        }
        // Sharing must read the window before this request enters it, so that
        // "already seen" means seen in an *earlier* request.
        self.sharing.observe(r, &self.window);
        self.window.observe(r);
        self.length.observe(r);
        self.unique
            .observe(&facts, facts.entry_size, r.request_start);
        self.references += 1;
        self.bytes += u128::from(facts.entry_size);
    }

    /// Push every event of a plan chunk.
    pub fn push_events(&mut self, events: &[crate::plan::PlanEvent]) {
        for e in events {
            self.push(&Ref::from(e));
        }
    }

    /// Close the open request, and the window with it if it is full.
    fn close_request(&mut self) {
        let had_open = self.window.open_references() > 0;
        self.window.end_request();
        self.sharing.end_request();
        self.length.end_request();
        if !had_open {
            return;
        }
        self.requests += 1;
        self.requests_in_window += 1;
        if self.requests_in_window >= self.window_requests {
            self.close_window();
        }
    }

    fn close_window(&mut self) {
        self.trunk.close_window(&self.window);
        self.working_set.close_window(&self.window);
        self.window.reset();
        self.requests_in_window = 0;
    }

    /// Finish the stream and assemble the report.
    pub fn finish(mut self) -> Report {
        self.close_request();
        // The final window is closed even if short of `wss_window`. It is marked
        // partial, which is what lets a 12 000-request plan still carry a
        // working-set figure against a 240 000-request default.
        if !self.window.is_empty() {
            self.close_window();
        }
        self.unique.end();

        let trunk = self.trunk.finish(&self.keys);
        let floor = self.floor.finish();
        let reuse_distance = self.reuse.finish();
        let sharing = self.sharing.finish();
        let request_length = self.length.finish();
        let unique_keys = self.unique.finish();
        let working_set = self.working_set.finish();

        let mut report = Report {
            references: self.references,
            requests: self.requests,
            distinct_keys: self.keys.steady_distinct_keys(),
            total_bytes: self.bytes,
            distinct_bytes: self.keys.steady_distinct_bytes(),
            warmup: self.warmup,
            reuse_distance,
            floor,
            intended_shared_depth: None,
            sharing,
            request_length,
            unique_keys,
            trunk,
            working_set,
            provenance: Provenance::default(),
            warnings: Vec::new(),
        };
        report.warnings = report.plan_side_warnings();
        report
    }
}

/// What a report is attributable to (spec FR-047).
///
/// The symmetry certificate and per-node software versions FR-047 also requires
/// are properties of a *run*: no node participates in characterising a plan, so a
/// plan report carries the two fields it can and leaves the rest to the runner
/// rather than inventing placeholders for them.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Provenance {
    /// `blake3:` hash over the normalised input and the events together.
    pub content_hash: Option<String>,
    /// `blake3:` hash over the parameters alone, for an unbounded run that has no
    /// events to hash.
    pub parameter_hash: Option<String>,
    /// Digest over the key sequence, so two arms can be *proven* equal (FR-036).
    pub stream_digest: Option<String>,
    /// The normalised input YAML, embedded in full.
    pub normalised_yaml: Option<String>,
}

/// The intended `shared_depth`, stated separately from the realised histogram.
///
/// FR-012a: the configured value must never be presented as if it were the
/// measured one. They are two statistics, and where they diverge the divergence is
/// the finding — the drawn value is only an *upper bound* on realised sharing,
/// with trunk occupancy deciding whether the bound is tight.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IntendedSharing {
    /// The configured distribution, as written.
    pub shape: String,
    /// Its mean, where the shape has one.
    pub mean: Option<f64>,
    /// Its median.
    pub p50: Option<f64>,
    /// Its 90th percentile.
    pub p90: Option<f64>,
    /// Its 99th percentile.
    pub p99: Option<f64>,
}

impl IntendedSharing {
    /// Read the intended distribution out of a document's `shared_depth`.
    pub fn from_dist(d: &Dist) -> IntendedSharing {
        IntendedSharing {
            shape: match d {
                Dist::Scalar(v) => format!("const {v}"),
                Dist::Shaped(s) => format!("{s:?}"),
            },
            mean: d.mean(),
            p50: d.quantile(0.50),
            p90: d.quantile(0.90),
            p99: d.quantile(0.99),
        }
    }
}

/// Which protection a warning belongs to.
///
/// The vocabulary is shared with the runner deliberately: FR-059 and half of
/// FR-060 can only be raised against a consumer's own reporting, and a runner
/// raising them under different names would leave a reader unable to tell that the
/// plan-side and run-side halves of one check are the same check.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum WarningKind {
    /// FR-060, plan side: the compulsory-miss floor approaches 1.0, so the live
    /// key space is almost never re-read and the consumer's cache does no useful
    /// work whatever its policy. Knowable before anything runs.
    NothingIsReRead,
    /// Sharing was configured but barely realised: trunk occupancy at the
    /// configured depth is too low for sessions to meet on the same paths.
    SharingNotRealised,
    /// No window reached `wss_window` requests, so every windowed statistic is
    /// over a short window and must not be read as one over the configured one.
    NoCompleteWindow,
    /// FR-059, **runner side**: the consumer reports steady-state evictions at
    /// ~zero, so the working set fits and its policy is untested. A plan report
    /// never carries this — the generator does not know the capacity.
    EvictionsAtZero,
    /// FR-060, **runner side**: the measured hit rate is within noise of the
    /// compulsory-miss floor, so every policy looks alike.
    HitRateAtTheFloor,
    /// FR-062: a hit-rate comparison was attempted between arms whose stream
    /// digests differ, so they did not see the same workload.
    StreamDigestsDiffer,
}

impl WarningKind {
    /// The requirement this warning discharges.
    pub fn requirement(&self) -> &'static str {
        match self {
            WarningKind::NothingIsReRead | WarningKind::HitRateAtTheFloor => "FR-060",
            WarningKind::EvictionsAtZero => "FR-059",
            WarningKind::StreamDigestsDiffer => "FR-062",
            WarningKind::SharingNotRealised | WarningKind::NoCompleteWindow => "FR-034a",
        }
    }

    /// Whether this warning can be raised from a plan alone.
    ///
    /// The two that cannot are not omissions: FR-059 depends on the consumer's
    /// eviction reporting and FR-060's run-side half on its measured hit rate,
    /// and the generator knows no capacity from which to derive either.
    pub fn is_plan_side(&self) -> bool {
        matches!(
            self,
            WarningKind::NothingIsReRead
                | WarningKind::SharingNotRealised
                | WarningKind::NoCompleteWindow
        )
    }
}

/// A warning that protects the measurement.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Warning {
    /// Which check.
    pub kind: WarningKind,
    /// The requirement it discharges, e.g. `FR-060`.
    pub requirement: String,
    /// What was found, with the numbers that triggered it.
    pub message: String,
}

impl Warning {
    /// A warning of `kind`.
    pub fn new(kind: WarningKind, message: impl Into<String>) -> Warning {
        Warning {
            kind,
            requirement: kind.requirement().to_string(),
            message: message.into(),
        }
    }
}

/// The compulsory-miss floor above which nothing is meaningfully re-read.
///
/// A chosen threshold, not a derived one: at a floor of 0.95 nineteen of every
/// twenty references are for a block the stream has never asked for, and no
/// capacity or policy can change that. Stated as a constant so the number a
/// warning fires on is visible rather than buried in a comparison.
pub const NOTHING_RE_READ_FLOOR: f64 = 0.95;

/// Trunk occupancy at or below which configured sharing is not being realised.
///
/// Inclusive at 1.0, and that is the whole point: one session per distinct path
/// means every session is landing on virgin trunk with nobody to share with, so
/// 1.0 is already the failure rather than the boundary of it. Measured occupancy
/// is a lower bound (see [`super::trunk`]), so this still fires only on the clear
/// case.
pub const SHARING_NOT_REALISED_OCCUPANCY: f64 = 1.0;

/// Everything FR-034a requires, and nothing that needs a capacity.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Report {
    /// Measured references.
    pub references: u64,
    /// Measured requests.
    pub requests: u64,
    /// Distinct keys in the measured window.
    pub distinct_keys: u64,
    /// Bytes referenced in the measured window, counting repeats.
    pub total_bytes: u128,
    /// Summed entry size over the measured window's distinct keys.
    pub distinct_bytes: u128,
    /// Warmup, counted separately (FR-045).
    pub warmup: WarmupCounts,
    /// The primary statistic.
    pub reuse_distance: ReuseDistanceReport,
    /// The compulsory-miss floor.
    pub floor: FloorReport,
    /// The **configured** sharing distribution, where a document supplied one.
    pub intended_shared_depth: Option<IntendedSharing>,
    /// The **realised** prefix-sharing depth histogram.
    pub sharing: SharingReport,
    /// Blocks and bytes per request.
    pub request_length: LengthReport,
    /// Unique keys over time.
    pub unique_keys: UniqueKeysReport,
    /// Realised trunk width and occupancy per depth.
    pub trunk: TrunkReport,
    /// The realised working-set size.
    pub working_set: WorkingSetReport,
    /// What this report is attributable to (FR-047).
    pub provenance: Provenance,
    /// Warnings that protect the measurement.
    pub warnings: Vec<Warning>,
}

impl Report {
    /// Attach the plan's identity and input (FR-047).
    pub fn with_provenance(mut self, p: Provenance) -> Report {
        self.provenance = p;
        self
    }

    /// Attach the configured `shared_depth`, and re-derive the warnings that
    /// depend on knowing what was intended.
    pub fn with_intended_shared_depth(mut self, d: &Dist) -> Report {
        self.intended_shared_depth = Some(IntendedSharing::from_dist(d));
        self.warnings = self.plan_side_warnings();
        self
    }

    /// The warnings a plan alone can justify.
    fn plan_side_warnings(&self) -> Vec<Warning> {
        let mut out = Vec::new();
        if let Some(floor) = self.floor.per_object {
            if floor >= NOTHING_RE_READ_FLOOR {
                out.push(Warning::new(
                    WarningKind::NothingIsReRead,
                    format!(
                        "the compulsory-miss floor is {:.4}: {:.1}% of references are for a block \
                         the stream has never asked for, so no capacity or policy can improve on \
                         it and the run will measure nothing about the consumer's cache",
                        floor,
                        floor * 100.0
                    ),
                ));
            }
        }
        if self.working_set.complete_windows == 0 && self.working_set.windows > 0 {
            let reached = self
                .working_set
                .observations
                .iter()
                .map(|w| w.requests)
                .max()
                .unwrap_or(0);
            out.push(Warning::new(
                WarningKind::NoCompleteWindow,
                format!(
                    "no window reached the configured wss_window of {} requests (the longest held \
                     {reached}), so the working-set size, trunk occupancy and realised sharing are \
                     all over a shorter window than configured",
                    self.working_set.window_requests
                ),
            ));
        }
        if let Some(intended) = &self.intended_shared_depth {
            // Compare intended against realised at the depth the document says
            // sessions reach, which is where occupancy has to hold up.
            let target = intended.p50.unwrap_or(0.0).round().max(0.0) as usize;
            if let Some(d) = self.trunk.depths.get(target) {
                let occ = d.occupancy.unwrap_or(0.0);
                if target > 0 && occ <= SHARING_NOT_REALISED_OCCUPANCY {
                    out.push(Warning::new(
                        WarningKind::SharingNotRealised,
                        format!(
                            "shared_depth has median {target} but realised occupancy at depth \
                             {target} is {occ:.2}: sessions are landing on virgin trunk, so \
                             realised sharing will fall far below the configured value"
                        ),
                    ));
                }
            }
        }
        out
    }

    /// Refuse a comparison between arms that did not see the same stream (FR-062).
    ///
    /// A refusal rather than a warning, because a hit-rate difference between two
    /// arms that saw different workloads is not a weak result — it is not a result.
    /// This is the generator's whole contribution to a comparison's validity, and
    /// it is what makes an externally-run comparison as trustworthy as one run
    /// here (FR-036).
    pub fn refuse_unless_same_stream(a: &str, b: &str) -> Result<(), Warning> {
        if a == b {
            return Ok(());
        }
        Err(Warning::new(
            WarningKind::StreamDigestsDiffer,
            format!(
                "stream digests differ ({a} vs {b}): the arms did not consume the same key \
                 sequence, so no hit-rate comparison between them is valid"
            ),
        ))
    }

    /// The runner-side half of FR-060: a measured hit rate at the floor.
    ///
    /// Lives here rather than in the runner so that "within noise of the floor"
    /// has one definition. `noise` is the runner's own measurement noise, which
    /// only it knows.
    pub fn hit_rate_at_the_floor(&self, measured_hit_rate: f64, noise: f64) -> Option<Warning> {
        let floor = self.floor.per_object?;
        let best_possible = 1.0 - floor;
        if measured_hit_rate + noise >= best_possible {
            Some(Warning::new(
                WarningKind::HitRateAtTheFloor,
                format!(
                    "measured hit rate {measured_hit_rate:.4} is within noise ({noise:.4}) of the \
                     compulsory-miss ceiling {best_possible:.4}, so the working set is too large \
                     for the capacity under test and every policy will look alike"
                ),
            ))
        } else {
            None
        }
    }

    /// The machine-readable form (FR-048).
    pub fn to_json(&self) -> Result<String, serde_json::Error> {
        serde_json::to_string_pretty(self)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dist::{Dist, Shape};
    use crate::keys::{CacheKey, SessionId};

    /// Requests as `(session, path)`, all measured.
    fn run(window: u64, requests: &[(u32, Vec<u64>)]) -> Report {
        let mut s = Statistics::new(window);
        for (session, path) in requests {
            for (i, k) in path.iter().enumerate() {
                s.push(&Ref {
                    key: CacheKey(*k),
                    size: 100,
                    depth: i as u32,
                    session: SessionId(*session),
                    request_start: i == 0,
                    warmup: false,
                });
            }
        }
        s.finish()
    }

    fn repeated(n: u32, path: &[u64]) -> Vec<(u32, Vec<u64>)> {
        (0..n).map(|s| (s, path.to_vec())).collect()
    }

    #[test]
    fn the_totals_agree_with_the_stream_that_produced_them() {
        let r = run(10, &repeated(4, &[1, 2, 3]));
        assert_eq!(r.requests, 4);
        assert_eq!(r.references, 12);
        assert_eq!(r.distinct_keys, 3);
        assert_eq!(r.total_bytes, 1200, "every reference");
        assert_eq!(r.distinct_bytes, 300, "each key once");
        assert_eq!(r.request_length.blocks.mean, Some(3.0));
    }

    #[test]
    fn every_statistic_fr_034a_enumerates_is_present() {
        // The enumeration is normative, so its coverage is asserted rather than
        // left to inspection.
        let r = run(2, &repeated(4, &[1, 2]));
        assert!(r.reuse_distance.references > 0, "reuse-distance CDF");
        assert!(r.floor.per_object.is_some(), "compulsory-miss floor");
        assert!(r.sharing.requests > 0, "prefix-sharing depth histogram");
        assert!(r.request_length.requests > 0, "request-length distribution");
        assert!(!r.unique_keys.points.is_empty(), "unique keys over time");
        assert!(r.distinct_keys > 0, "distinct keys");
        assert!(r.total_bytes > 0, "total bytes");
        assert!(r.trunk.depths[0].width_run > 0, "trunk width per depth");
        assert!(r.trunk.depths[0].occupancy.is_some(), "trunk occupancy");
        assert!(
            r.working_set.max_distinct_keys.is_some(),
            "working-set size"
        );
    }

    #[test]
    fn warmup_is_separated_without_being_credited_to_the_floor() {
        // FR-045 and the floor's subtlety together: the warmed key is counted as
        // warmup, and is not a compulsory miss when the measured window asks for
        // it.
        let mut s = Statistics::new(10);
        let mk = |k: u64, warmup: bool| Ref {
            key: CacheKey(k),
            size: 50,
            depth: 0,
            session: SessionId(1),
            request_start: true,
            warmup,
        };
        s.push(&mk(1, true));
        s.push(&mk(1, false));
        s.push(&mk(2, false));
        let r = s.finish();
        assert_eq!(r.warmup.references, 1);
        assert_eq!(r.warmup.requests, 1);
        assert_eq!(r.warmup.distinct_keys, 1);
        assert_eq!(r.references, 2, "warmup is not a measured reference");
        assert_eq!(r.floor.compulsory_misses, 1, "only key 2");
        assert_eq!(r.floor.per_object, Some(0.5));
    }

    #[test]
    fn a_window_boundary_falls_on_the_configured_request_count() {
        let r = run(2, &repeated(6, &[1]));
        assert_eq!(r.working_set.windows, 3);
        assert_eq!(r.working_set.complete_windows, 3);
        assert!(r.working_set.observations.iter().all(|w| w.requests == 2));
    }

    #[test]
    fn the_final_short_window_is_reported_and_marked_rather_than_dropped() {
        let r = run(4, &repeated(6, &[1]));
        assert_eq!(r.working_set.windows, 2);
        assert_eq!(r.working_set.complete_windows, 1);
        assert!(r.working_set.observations[1].partial);
        assert_eq!(r.working_set.observations[1].requests, 2);
    }

    #[test]
    fn a_never_re_read_stream_warns_before_anything_runs() {
        // FR-060's plan side. Every request novel, so the floor is 1.0.
        let reqs: Vec<(u32, Vec<u64>)> = (0..200u32).map(|i| (i, vec![u64::from(i)])).collect();
        let r = run(50, &reqs);
        assert_eq!(r.floor.per_object, Some(1.0));
        assert!(
            r.warnings
                .iter()
                .any(|w| w.kind == WarningKind::NothingIsReRead),
            "a floor of 1.0 must warn: {:?}",
            r.warnings
        );
    }

    #[test]
    fn a_healthy_stream_raises_no_plan_side_warning() {
        // A bounded key space, re-read often, with sharing that is realised.
        let mut reqs = Vec::new();
        for i in 0..400u32 {
            reqs.push((i % 8, vec![1, 2, u64::from(i % 4) + 10]));
        }
        let r = run(50, &reqs);
        assert!(
            r.warnings.is_empty(),
            "unexpected warnings: {:?}",
            r.warnings
        );
    }

    #[test]
    fn a_report_never_carries_a_warning_that_needs_the_consumers_own_numbers() {
        // FR-059 and FR-060's run-side half have no plan-side evidence, so a plan
        // report that raised either would be inventing one.
        let reqs: Vec<(u32, Vec<u64>)> = (0..100u32).map(|i| (i, vec![u64::from(i)])).collect();
        let r = run(10, &reqs);
        assert!(r.warnings.iter().all(|w| w.kind.is_plan_side()));
        assert!(!WarningKind::EvictionsAtZero.is_plan_side());
        assert!(!WarningKind::HitRateAtTheFloor.is_plan_side());
    }

    #[test]
    fn the_run_side_floor_check_uses_the_plans_own_floor() {
        let r = run(10, &repeated(20, &[1, 2]));
        let floor = r.floor.per_object.unwrap();
        let ceiling = 1.0 - floor;
        assert!(r.hit_rate_at_the_floor(ceiling - 0.001, 0.01).is_some());
        assert!(r.hit_rate_at_the_floor(0.0, 0.0).is_none());
    }

    #[test]
    fn differing_stream_digests_are_refused_not_warned_about() {
        assert!(Report::refuse_unless_same_stream("blake3:aa", "blake3:aa").is_ok());
        let e = Report::refuse_unless_same_stream("blake3:aa", "blake3:bb").unwrap_err();
        assert_eq!(e.kind, WarningKind::StreamDigestsDiffer);
        assert_eq!(e.requirement, "FR-062");
    }

    #[test]
    fn intended_and_realised_sharing_are_two_statistics_not_one() {
        // FR-012a. The configured median is 8; realised sharing here is 1 level,
        // and the report must show both rather than reconcile them.
        let r = run(10, &repeated(4, &[1, 90, 91])).with_intended_shared_depth(&Dist::Shaped(
            Shape::Lognormal {
                median: 8.0,
                sigma: 0.1,
            },
        ));
        let intended = r.intended_shared_depth.as_ref().unwrap();
        assert_eq!(intended.p50.map(|v| v.round()), Some(8.0));
        assert_eq!(r.sharing.realised_depth.max, Some(2));
        assert!(intended.p50.unwrap() > r.sharing.realised_depth.max.unwrap() as f64);
    }

    #[test]
    fn unrealised_sharing_is_reported_as_a_warning_with_both_numbers() {
        // Configured deep sharing, every session on its own root: occupancy at
        // the configured depth is 1, so nobody has anyone to share with.
        let reqs: Vec<(u32, Vec<u64>)> = (0..40u32)
            .map(|i| (i, vec![u64::from(i) * 10, u64::from(i) * 10 + 1]))
            .collect();
        let r = run(20, &reqs).with_intended_shared_depth(&Dist::Scalar(1.0));
        let w = r
            .warnings
            .iter()
            .find(|w| w.kind == WarningKind::SharingNotRealised)
            .expect("should warn");
        assert!(w.message.contains("virgin trunk"));
        assert_eq!(w.requirement, "FR-034a");
    }

    #[test]
    fn realised_sharing_at_a_healthy_occupancy_raises_no_warning() {
        // The other side of the same check: forty sessions meeting on four shared
        // paths at depth 1, so occupancy there is ten and the configured depth is
        // reachable.
        let reqs: Vec<(u32, Vec<u64>)> = (0..40u32)
            .map(|i| (i, vec![1, u64::from(i % 4) + 10]))
            .collect();
        let r = run(40, &reqs).with_intended_shared_depth(&Dist::Scalar(1.0));
        assert_eq!(r.trunk.depths[1].occupancy, Some(10.0));
        assert!(
            !r.warnings
                .iter()
                .any(|w| w.kind == WarningKind::SharingNotRealised),
            "unexpected warnings: {:?}",
            r.warnings
        );
    }

    #[test]
    fn provenance_carries_the_plan_hash_and_its_input() {
        // FR-047: a report must be traceable to the exact input that produced it.
        let r = run(10, &repeated(2, &[1])).with_provenance(Provenance {
            content_hash: Some("blake3:abc".into()),
            parameter_hash: None,
            stream_digest: Some("blake3:def".into()),
            normalised_yaml: Some("version: 1\n".into()),
        });
        assert_eq!(r.provenance.content_hash.as_deref(), Some("blake3:abc"));
        assert_eq!(
            r.provenance.normalised_yaml.as_deref(),
            Some("version: 1\n")
        );
    }

    #[test]
    fn the_json_form_round_trips() {
        let r = run(4, &repeated(8, &[1, 2, 3]));
        let json = r.to_json().unwrap();
        let back: Report = serde_json::from_str(&json).unwrap();
        assert_eq!(back.references, r.references);
        assert_eq!(back.distinct_keys, r.distinct_keys);
        assert_eq!(
            back.reuse_distance.fraction_within_objects(2),
            r.reuse_distance.fraction_within_objects(2)
        );
    }
}
