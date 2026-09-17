//! The emit run's report: completeness, and nothing it did not measure (FR-071).
//!
//! # Omitted, not zeroed — and the type is what enforces it
//!
//! An emit run has no lanes, no requests and no server, so latency percentiles,
//! lane utilisation and the virtual-to-wallclock ratio are not small numbers for
//! it — they are *not numbers*. Reporting them as zero would invite exactly the
//! comparison the requirement exists to prevent, because a zero is
//! indistinguishable from a measurement.
//!
//! The way that is enforced here is worth stating: [`EmitReport`] simply **has no
//! fields for them**. Not `Option`, not zero — absent from the struct. A future
//! change that wanted to report a latency from an emit run would have to add a
//! field and would be visible in review, whereas an `Option` left at `None` is one
//! careless `unwrap_or(0.0)` away from a published zero.
//!
//! `#[serde(skip_serializing_if)]` is deliberately **not** used for the same
//! reason: a field that vanishes when empty is a field that exists.
//!
//! # Generation speed is labelled, because it is the one wallclock number allowed
//!
//! FR-071 permits reporting generation speed, and it is genuinely useful — it is
//! what says whether emitting a longer span is minutes or hours. It is named
//! `generation_rate_invocations_per_second` rather than anything shorter, so it
//! cannot be mistaken for a throughput the *server* achieved. That is the whole
//! risk with this one figure.

use serde::Serialize;
use workload_model::project::Projection;

/// Which containers a run wrote, and how many records went into each.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct ContainerRecords {
    /// Rows written to JSONL, if that container was requested.
    pub jsonl: Option<u64>,
    /// Rows written to parquet, if that container was requested.
    pub parquet: Option<u64>,
}

/// Everything needed to reproduce the run that produced a trace (FR-072).
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Reproduction {
    /// The seed.
    pub seed: u64,
    /// The span requested, in virtual seconds.
    pub until: f64,
    /// Digest of the description, so a trace and a description can be checked
    /// against each other.
    pub description_digest: String,
    /// Path of the description, for a human reading the report.
    pub description_path: String,
}

/// The report an emit run writes.
///
/// Deliberately not a superset of the live report and deliberately not sharing a
/// type with it. They answer different questions, and a shared type would need
/// every live-only field to be optional — which is the shape that lets a zero
/// escape.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct EmitReport {
    /// Always `"emit"`, so a tool reading a directory of reports can tell at a
    /// glance which kind it has without inferring from which fields are present.
    pub run_kind: &'static str,
    /// Sessions created, including the generation seeded at `t = 0`.
    pub sessions_started: u64,
    /// Sessions that took their last turn.
    ///
    /// Less than `sessions_started` at the end of any run, because the span cuts
    /// the last generation short — that is right-censoring, not an error, and the
    /// manifest declares it.
    pub sessions_completed: u64,
    /// Turns emitted; one LLM request each.
    pub invocations: u64,
    /// Blocks minted by turn growth.
    pub blocks_minted: u64,
    /// Block references summed over every turn's prefix.
    pub block_references: u64,
    /// Virtual seconds covered — the span asked for, not the busiest part of it.
    pub virtual_span: f64,
    /// Rows per container.
    pub records: ContainerRecords,
    /// How fast the plan was generated, **labelled** because it is wallclock.
    ///
    /// This is the generator's own speed and says nothing about any server. See the
    /// module docs on why the name is long.
    pub generation_rate_invocations_per_second: f64,
    /// Wallclock seconds spent generating. Also the generator's own, not a result.
    pub generation_wallclock_seconds: f64,
    /// How to reproduce it.
    pub reproduction: Reproduction,
    /// The pre-flight projection, kept so its accuracy can be checked after the
    /// fact against what the run actually wrote.
    pub projection: ProjectionSummary,
    /// Anything the run needs a reader to know — a short span, a pool that could
    /// not turn over.
    pub warnings: Vec<String>,
}

/// The projection as recorded in a report.
///
/// A flattened copy rather than the [`Projection`] itself, so that the report's
/// serialised shape does not change if the projection grows a field.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ProjectionSummary {
    /// Invocations projected before the run.
    pub invocations: u64,
    /// Keys projected to be minted.
    pub keys_minted: u64,
    /// Key references projected.
    pub key_references: u64,
}

impl From<&Projection> for ProjectionSummary {
    fn from(p: &Projection) -> Self {
        Self {
            invocations: p.invocations,
            keys_minted: p.keys_minted,
            key_references: p.key_references,
        }
    }
}

impl EmitReport {
    /// Render the human-readable form (FR-065's terminal half).
    ///
    /// The projection and the actual are printed **side by side**, because a
    /// projection nobody checks is a projection that quietly stops being accurate.
    ///
    /// # Examples
    ///
    /// ```
    /// use workload_gen::report::{
    ///     ContainerRecords, EmitReport, ProjectionSummary, Reproduction,
    /// };
    ///
    /// let report = EmitReport {
    ///     run_kind: "emit",
    ///     sessions_started: 40,
    ///     sessions_completed: 31,
    ///     invocations: 214,
    ///     blocks_minted: 642,
    ///     block_references: 1_908,
    ///     virtual_span: 300.0,
    ///     records: ContainerRecords { jsonl: Some(214), parquet: None },
    ///     generation_rate_invocations_per_second: 178_000.0,
    ///     generation_wallclock_seconds: 0.0012,
    ///     reproduction: Reproduction {
    ///         seed: 42,
    ///         until: 300.0,
    ///         description_digest: "9f1c2e".to_string(),
    ///         description_path: "chat.yml".to_string(),
    ///     },
    ///     projection: ProjectionSummary {
    ///         invocations: 220,
    ///         keys_minted: 660,
    ///         key_references: 1_960,
    ///     },
    ///     warnings: vec![],
    /// };
    ///
    /// let text = report.render();
    /// // Projected against actual, on the same line, so drift is visible.
    /// assert!(text.contains("invocations       214 (projected 220)"));
    /// // The one wallclock figure allowed, and it says whose speed it is.
    /// assert!(text.contains("not a server result"));
    /// // Fewer completed than started is right-censoring: the span cut the last
    /// // generation short, and the manifest declares it.
    /// assert!(text.contains("40 started, 31 completed"));
    ///
    /// // And the structured form carries no latency, lane or ratio field at all —
    /// // an emit run did not measure them, so there is nowhere to put a zero.
    /// let json = report.to_json().unwrap();
    /// assert!(!json.contains("latency"));
    /// assert!(!json.contains("lane"));
    /// ```
    pub fn render(&self) -> String {
        let mut out = String::new();
        out.push_str("emit run complete\n");
        out.push_str(&format!(
            "  virtual span      {:.0} s\n  sessions          {} started, {} completed\n",
            self.virtual_span, self.sessions_started, self.sessions_completed
        ));
        out.push_str(&format!(
            "  invocations       {} (projected {})\n",
            self.invocations, self.projection.invocations
        ));
        out.push_str(&format!(
            "  blocks minted     {} (projected {})\n  block references  {} (projected {})\n",
            self.blocks_minted,
            self.projection.keys_minted,
            self.block_references,
            self.projection.key_references
        ));
        if let Some(n) = self.records.jsonl {
            out.push_str(&format!("  jsonl records     {n}\n"));
        }
        if let Some(n) = self.records.parquet {
            out.push_str(&format!("  parquet records   {n}\n"));
        }
        out.push_str(&format!(
            "  generation        {:.0} invocations/s over {:.2} s wallclock \
             (the generator's own speed, not a server result)\n",
            self.generation_rate_invocations_per_second, self.generation_wallclock_seconds
        ));
        out.push_str(&format!(
            "  reproduce with    --seed {} --until {:.0}   description {} ({})\n",
            self.reproduction.seed,
            self.reproduction.until,
            self.reproduction.description_path,
            self.reproduction.description_digest
        ));
        for w in &self.warnings {
            out.push_str("  warning: ");
            out.push_str(w);
            out.push('\n');
        }
        out
    }

    /// The structured form (FR-065's file half).
    ///
    /// # Errors
    ///
    /// If serialisation fails, which for this type means a bug.
    pub fn to_json(&self) -> serde_json::Result<String> {
        let mut s = serde_json::to_string_pretty(self)?;
        s.push('\n');
        Ok(s)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn report() -> EmitReport {
        EmitReport {
            run_kind: "emit",
            sessions_started: 120,
            sessions_completed: 100,
            invocations: 4_000,
            blocks_minted: 12_000,
            block_references: 900_000,
            virtual_span: 1_000.0,
            records: ContainerRecords {
                jsonl: Some(4_000),
                parquet: None,
            },
            generation_rate_invocations_per_second: 250_000.0,
            generation_wallclock_seconds: 0.016,
            reproduction: Reproduction {
                seed: 42,
                until: 1_000.0,
                description_digest: "0123456789abcdef".into(),
                description_path: "example.yml".into(),
            },
            projection: ProjectionSummary {
                invocations: 4_000,
                keys_minted: 12_000,
                key_references: 880_000,
            },
            warnings: vec![],
        }
    }

    #[test]
    fn the_live_only_fields_are_absent_from_the_serialised_form() {
        // FR-071, checked on the JSON rather than on the type, because the JSON is
        // what a downstream tool reads. A zero here would be indistinguishable from
        // a measurement.
        let json = report().to_json().unwrap();
        for forbidden in [
            "latency",
            "p50",
            "p99",
            "lane_utilisation",
            "lane_utilization",
            "plan_queue",
            "virtual_to_wallclock",
            "throughput",
        ] {
            assert!(
                !json.contains(forbidden),
                "an emit report must not mention {forbidden}:\n{json}"
            );
        }
    }

    #[test]
    fn generation_speed_is_present_and_labelled_as_the_generators_own() {
        // The one wallclock figure FR-071 permits, and the one that could be
        // mistaken for a server result if it were named `throughput`.
        let r = report();
        let json = r.to_json().unwrap();
        assert!(json.contains("generation_rate_invocations_per_second"));
        assert!(r.render().contains("not a server result"));
    }

    #[test]
    fn the_report_shows_projected_against_actual_side_by_side() {
        // A projection nobody checks stops being accurate quietly. Printing them
        // together is what makes a drift visible without anyone running a test.
        let text = report().render();
        assert!(text.contains("4000 (projected 4000)"), "got:\n{text}");
        assert!(text.contains("900000 (projected 880000)"), "got:\n{text}");
    }

    #[test]
    fn reproduction_parameters_are_enough_to_repeat_the_run() {
        let text = report().render();
        assert!(text.contains("--seed 42"));
        assert!(text.contains("--until 1000"));
        assert!(text.contains("0123456789abcdef"), "no description digest");
    }

    #[test]
    fn a_container_that_was_not_written_is_absent_rather_than_zero() {
        // The same rule as the live-only fields, one level down: `parquet: 0` would
        // read as "wrote a parquet trace with no rows in it".
        let r = report();
        assert!(r.records.parquet.is_none());
        assert!(!r.render().contains("parquet records"));
        let json = r.to_json().unwrap();
        assert!(json.contains("\"parquet\": null"), "got:\n{json}");
    }

    #[test]
    fn completed_may_trail_started_and_that_is_not_an_error() {
        // Right-censoring: the span cuts the last generation short. Asserted so
        // nobody later "fixes" it by draining sessions past the span, which would
        // make the emitted span longer than the one requested.
        let r = report();
        assert!(r.sessions_completed < r.sessions_started);
    }

    #[test]
    fn warnings_are_rendered_when_present() {
        let mut r = report();
        r.warnings
            .push("the span is short relative to a lifetime".into());
        assert!(r.render().contains("warning: the span is short"));
    }
}

/// The report a **live** run writes (FR-061, FR-062, FR-065).
///
/// A separate type from [`EmitReport`], deliberately. They answer different questions,
/// and a shared type would need every live-only field to be optional — which is the
/// shape that lets a zero escape into an emit report.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct LiveReport {
    /// Always `"live"`.
    pub run_kind: &'static str,
    /// **Whether this run's numbers may be used at all** (FR-062, FR-080).
    ///
    /// First field on purpose: a reader scanning a directory of reports should meet the
    /// validity before the throughput, not after it.
    pub valid: bool,
    /// Why, when it is not.
    pub invalid_reason: Option<String>,
    /// Which question this run was asking: `paced` or `work-conserving` (FR-080).
    ///
    /// Named on the report rather than left to be remembered, because the two are **not
    /// comparable**: a paced throughput is capped by the rate that was asked for and a
    /// work-conserving one is a ceiling. Two reports side by side with no mode on them is how
    /// they get quoted as though they measured the same thing.
    pub mode: &'static str,
    /// The schedule the run set itself, and how well it kept it. `None` when unpaced.
    pub schedule: Option<Schedule>,
    /// Requests issued.
    pub requests: u64,
    /// Key references issued.
    pub key_references: u64,
    /// Keys per second over the timed window.
    pub keys_per_second: f64,
    /// Payload bytes per second, read plus written.
    ///
    /// From the hit/miss results, since only a `LOOKUP` hit and an accepted
    /// `COPY_TO_STORE` move a payload. It was previously `key_references * block_bytes`,
    /// which charged a block to every control operation and overstated bandwidth three- to
    /// fourfold.
    pub bytes_per_second: f64,
    /// Virtual seconds advanced per wallclock second.
    pub virtual_to_wallclock: f64,
    /// Wallclock seconds of the timed window, excluding any startup cache clear.
    pub elapsed_seconds: f64,
    /// Virtual seconds covered.
    pub virtual_span: f64,
    /// The plan queue between the producer and the lanes.
    pub queue: QueueStats,
    /// What Certus decided per key — outcomes, never failures.
    pub outcomes: CacheOutcomes,
    /// Payload bandwidth, computed from the hit/miss results.
    pub bandwidth: Bandwidth,
    /// Latency per opcode, because an aggregate describes the operation mix.
    pub latency_by_op: Vec<OpLatency>,
    /// Entries the startup memory-tier clear dropped, if `--clear-cache` was given.
    pub cleared_entries: Option<u64>,
    /// Whether the producer reached the end of its span rather than being interrupted.
    pub producer_completed: bool,
    /// Distinct keys the run referenced, when it was asked to count them.
    ///
    /// # Why this is the generator's count and not the nodes'
    ///
    /// Distinct counts do **not** sum. A shared prefix block referenced by sessions on two nodes
    /// is one key, and adding each node's count would double it — the same arithmetic mistake as
    /// averaging two nodes' percentiles, one type along. So it is counted once, by the producer,
    /// over the paths it built.
    ///
    /// `None` unless `--count-distinct` was given: maintaining the set is one insert per key
    /// reference on the producer's own path, which a throughput run should not pay and a capacity
    /// sweep needs. A sweep needs it because hit rate is only interpretable against the size of
    /// the key space it was measured over.
    pub distinct_keys: Option<u64>,
    /// The effective working set the `selection` spread produced, per shared class.
    ///
    /// The point of a capacity sweep is hit rate against capacity *relative to the working set*,
    /// and under uniform selection the working set is the whole key space — which is why the
    /// curve has a step rather than a slope and no policy is distinguishable. Reporting the
    /// spread's effective size is what lets a sweep say which regime it was in rather than
    /// leaving it to be guessed from the shape of the answer.
    pub working_set: Vec<WorkingSet>,
    /// The node that was lost, if one was (FR-064).
    ///
    /// Separate from `invalid_reason` so a sweep driver can act on it without parsing prose: a
    /// lost node is worth retrying the run for, whereas an underrun means the generator needs
    /// looking at.
    pub lost_node: Option<String>,
    /// Lanes used, which equals channels claimed.
    pub lanes: usize,
    /// The node's channel count, for comparison with `lanes`.
    pub node_channels: usize,
    /// Request latency in microseconds.
    pub latency_us: LatencyPercentiles,
    /// How to reproduce it.
    pub reproduction: Reproduction,
    /// Batch size and lane count, which MUST NOT have changed the plan (FR-072).
    pub tuning: Tuning,
    /// Operations skipped for want of a GPU payload buffer.
    ///
    /// Non-zero means the run exercised the **control path only** and moved no data, so
    /// its throughput is not comparable with a complete run's. Reported rather than
    /// hidden, because a keys-per-second figure from a run that never transferred a
    /// block is the kind of number that gets quoted.
    pub skipped_needing_gpu: u64,
    /// Keys those skipped operations would have moved.
    pub skipped_keys: u64,
}

/// What a paced run asked for, and what it achieved (FR-080).
///
/// # Lateness is the validity metric here, and the queue is not
///
/// FR-062 invalidates a run whose plan queue reached zero, which is meaningful only while the
/// generator is trying to sprint. Under pacing an empty queue is the normal, intended state —
/// nothing is due yet — so the queue carries no information and this replaces it. The failure it
/// catches is the same one: the pace came from somewhere other than the workload's own timing.
///
/// `virtual_to_wallclock` beside `rate` is a **second route to the same fact**. A run that fell
/// behind shows it as accumulated lateness and as a ratio below the rate it asked for, and the two
/// must agree; if they do not, one of them is measuring something else.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Schedule {
    /// Virtual seconds per wallclock second the run was aimed at.
    pub rate: f64,
    /// The fraction of that rate the run fell short by. Negative means it ran ahead.
    pub rate_shortfall: f64,
    /// Turns held for their due time — the `n` the percentiles below belong to (FR-066a).
    pub turns: u64,
    /// How far past its due time each turn was submitted, in microseconds. One-sided: pacing
    /// never submits early.
    pub lateness_us: LatencyPercentiles,
    /// The 99th-percentile lateness this run tolerated before calling itself invalid.
    pub tolerance_us: u64,
    /// Whether it stayed inside that.
    pub kept: bool,
}

/// The plan queue's own figures (FR-037, FR-062).
///
/// Counts of consumer stalls, not samples of a gauge: a sampled depth can miss a brief
/// exhaustion between samples, which is the same failure as reporting an average one level
/// down. Per-lane arrays are kept beside the totals because session sharding creates
/// imbalance, and an aggregate would hide one lane underrunning constantly behind seven
/// that never did.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct QueueStats {
    /// Turn batches the producer built.
    pub batches_produced: u64,
    /// Batches taken by lanes.
    pub pops: u64,
    /// Pops that found an empty queue — a buffer underrun. **Non-zero invalidates the
    /// run.**
    pub underruns: u64,
    /// Fraction of pops that underran.
    pub fraction_underrun: f64,
    /// Smallest depth any lane saw at pop time.
    pub min_depth: usize,
    /// Queue capacity per lane, in turn batches — what bounds memory instead of the span.
    pub capacity_per_lane: usize,
    /// Times the producer blocked on a full queue.
    ///
    /// The positive counterpart to an underrun: backpressure working, and the evidence
    /// that the generator was ahead rather than merely keeping up. Zero underruns *and* a
    /// non-zero block count is a queue that demonstrably did its job.
    pub producer_blocked: u64,
    /// Underruns per lane.
    pub per_lane_underruns: Vec<u64>,
    /// Minimum depth per lane.
    pub per_lane_min_depth: Vec<usize>,
}

/// What Certus decided, per key.
///
/// **None of these is a generator failure**, and the report must not imply otherwise. We do
/// not know when Certus will evict anything, and must not: a block stored earlier and absent
/// later is eviction working, which is the behaviour under measurement. They are here
/// because a run in which every reserve was declined is a run worth knowing about — the
/// generator previously ignored the per-key result bytes entirely and reported full
/// throughput regardless.
///
/// # Declines are reported, not interpreted
///
/// The wire carries a bare per-key `0` with no reason code, so the report gives the count
/// and its denominator and stops there. It is not the generator's job to be gracious about
/// a server that declines a store: if Certus refuses, the number says so.
///
/// Measured on node2 with **exactly one** server (see the warning below):
///
/// | run | reserves declined | commits declined |
/// | --- | --- | --- |
/// | cold cache | 0 of 72 | 0 of 72 |
/// | same seed again | 72 of 72 | 72 of 72 |
/// | different seed | 66 of 72 | 66 of 72 |
/// | warm, `--clear-cache` | **0 of 72** | **72 of 72** |
///
/// A cold cache declines nothing, so the store path is sound. A warm one declines because
/// the key is already there — `create_memory_tier_entry` answers `AlreadyExists`. The last
/// row is the one to know about: `CLEAR_MEMORY_TIER` frees the memory tier, so `RESERVE`
/// succeeds, but the dispatch map keeps its disk-backed entries, so the commit still cannot
/// land. **`--clear-cache` is therefore not a cold cache and is no substitute for
/// restarting the server.**
///
/// A commit is declined when its reserve was, because `op_commit_store` needs a pending
/// write. Transfers are not, because `copy_gpu_to_memory_async` does not require one —
/// which is why the counts read 72 / 0 / 72 rather than 72 / 72 / 72.
///
/// # Two servers on one mailbox invalidate all of this
///
/// While measuring the above I had, without noticing, left **two** `certus-server-yaml`
/// processes polling the same `/dev/shm` mailbox and the same device file. Each keeps its
/// own `pending_stores`, so a `RESERVE` answered by one and a `COMMIT_STORE` answered by
/// the other finds no pending write. Every decline figure taken that way is meaningless,
/// and nothing in the report could reveal it. Check with
/// `ps -eo args | awk '$1 ~ /certus-server-yaml$/'` before trusting a number, and beware
/// that `pkill -f <pattern>` matches the invoking shell's own command line.
///
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct CacheOutcomes {
    /// `CHECK` keys reported `RESIDENT` — committed and loadable now.
    pub check_resident: u64,
    /// `CHECK` keys reported `PENDING` — another lane's store is in flight. Not a miss.
    pub check_pending: u64,
    /// `CHECK` keys reported `MISS`.
    pub check_miss: u64,
    /// `LOOKUP` keys that returned data.
    pub lookup_hits: u64,
    /// `LOOKUP` keys that did not.
    ///
    /// An upper bound on true cache misses: `op_lookup` reports a handle it could not open
    /// as a 0 as well, and the wire does not distinguish the two.
    pub lookup_misses: u64,
    /// Keys a `RESERVE` was asked for, so a decline count has a denominator.
    pub reserves_attempted: u64,
    /// Keys a `COMMIT_STORE` was asked for.
    pub commits_attempted: u64,
    /// Keys a `RESERVE` declined.
    pub reserves_declined: u64,
    /// Keys a `COPY_TO_STORE` declined.
    pub transfers_declined: u64,
    /// Keys a `COMMIT_STORE` declined, which follows a declined reserve.
    pub commits_declined: u64,
    /// Loaded blocks whose stamp did not match the key asked for.
    ///
    /// The one figure here that is **not** a cache outcome: it means Certus returned the wrong
    /// block. Zero unless the run asked for verification.
    pub payload_mismatches: u64,
    /// Loaded blocks checked, so the mismatch count has a denominator.
    pub payloads_verified: u64,
}

impl CacheOutcomes {
    /// `CHECK` references answered.
    pub fn checks(&self) -> u64 {
        self.check_resident + self.check_pending + self.check_miss
    }

    /// Fraction of `CHECK` references that were resident, or `None` if nothing was checked.
    ///
    /// `PENDING` is excluded from the numerator and kept in the denominator: the key is
    /// coming but is not loadable at that instant, so counting it as a hit would overstate
    /// what the cache could serve, and counting it as a miss would understate what it
    /// holds. It is reported separately instead of being folded either way.
    ///
    /// `None` rather than zero: a run that checked nothing has no hit rate, and printing
    /// 0.0% would read as "everything missed".
    pub fn check_hit_rate(&self) -> Option<f64> {
        let total = self.checks();
        (total > 0).then(|| self.check_resident as f64 / total as f64)
    }

    /// Fraction of `LOOKUP` references that returned data.
    pub fn lookup_hit_rate(&self) -> Option<f64> {
        let total = self.lookup_hits + self.lookup_misses;
        (total > 0).then(|| self.lookup_hits as f64 / total as f64)
    }

    /// Whether the store path was declined anywhere.
    pub fn any_store_declined(&self) -> bool {
        self.reserves_declined + self.transfers_declined + self.commits_declined > 0
    }
}

/// Render `n` of `d` as a percentage, or `n/a` when nothing was attempted.
fn pct(n: u64, d: u64) -> String {
    if d == 0 {
        "n/a".to_string()
    } else {
        format!("{:.1}%", n as f64 / d as f64 * 100.0)
    }
}

/// Payload bandwidth, split by direction.
///
/// Only two operations move a payload: a `LOOKUP` **hit** brings a block from Certus, and an
/// accepted `COPY_TO_STORE` sends one to it. `CHECK`, `TOUCH`, `RESERVE` and `COMMIT_STORE`
/// are control and move nothing, so bandwidth can only be computed from the hit/miss
/// results — which is why they are captured. A `LOOKUP` miss transfers no bytes, and neither
/// does a declined transfer.
///
/// Certus's own write-through to SSD is not counted: this is what crossed the client
/// boundary.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct Bandwidth {
    /// Blocks Certus sent us.
    pub blocks_read: u64,
    /// Blocks we sent Certus.
    pub blocks_written: u64,
    /// Payload bytes read.
    pub read_bytes: u64,
    /// Payload bytes written.
    pub write_bytes: u64,
}

/// Requests an opcode needs before its percentiles are worth quoting.
///
/// A p50 over 20 samples is noise and a p99 over 20 samples *is* the maximum. This is not
/// hypothetical: a 24-request run appeared to show `TOUCH` costing three times `CHECK`,
/// which prompted an investigation that found nothing — at 8000 requests each they differ
/// by 1 microsecond, and `TOUCH` is marginally the faster in most runs. A hundred is the
/// point where a p50 is stable enough to compare and a p90 means something; a p99 still
/// wants thousands, so it is flagged rather than hidden.
pub const QUOTABLE_REQUESTS: u64 = 100;

/// Latency of one opcode, in microseconds.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct OpLatency {
    /// The opcode's name, as `shmq-dispatcher` calls it.
    pub op: &'static str,
    /// Requests timed.
    pub requests: u64,
    /// Percentiles.
    pub us: LatencyPercentiles,
}

/// The name for an operation, for a report a human reads.
///
/// Delegated to [`workload_wire::frame::op_kind`], which is where the field itself is defined.
/// The generator no longer depends on `shmq-dispatcher` at all — that is most of the point of
/// FR-079 — so it could not read the dispatcher's own constants even if it wanted to; the wire
/// carries the number and the agent-side test pins it to the dispatcher.
pub fn opcode_name(opcode: u32) -> &'static str {
    workload_wire::frame::op_kind::name(opcode)
}

/// What one shared class's `selection` spread actually covers.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct WorkingSet {
    /// The shared class.
    pub class: String,
    /// Instances the pool holds — the key space's size in instances.
    pub nominal: u64,
    /// Ranks the spread effectively covers, or `None` under uniform selection.
    ///
    /// `None` is the case a sweep must be able to recognise: it means the working set *is* the
    /// key space, so the curve will step rather than slope and the run cannot discriminate
    /// between policies however it is swept.
    pub effective_ranks: Option<f64>,
}

/// Request latency, in microseconds.
///
/// Percentiles rather than a mean: a mean latency hides the tail that a cache's
/// behaviour actually shows up in, and it is the tail an eviction policy moves.
#[derive(Debug, Clone, Copy, PartialEq, Serialize)]
pub struct LatencyPercentiles {
    /// Median.
    pub p50: u64,
    /// 90th percentile.
    pub p90: u64,
    /// 99th percentile.
    pub p99: u64,
    /// Largest observed.
    pub max: u64,
}

/// Execution knobs, recorded so a sweep can be reconstructed.
#[derive(Debug, Clone, Copy, PartialEq, Serialize)]
pub struct Tuning {
    /// Keys per request.
    pub batch_keys: usize,
    /// Execution concurrency.
    pub lanes: usize,
}

impl LiveReport {
    /// Render the human-readable form.
    ///
    /// An invalid run leads with the invalidity and **does not print a throughput at
    /// all** (FR-062): a number printed beside "invalid" gets copied out of the
    /// terminal without the word.
    pub fn render(&self) -> String {
        let mut out = String::new();
        if !self.valid {
            out.push_str("RUN INVALID — its throughput is not a result\n");
            if let Some(node) = &self.lost_node {
                out.push_str(&format!("  lost node         {node}\n"));
            }
            if let Some(why) = &self.invalid_reason {
                out.push_str(&format!("  reason            {why}\n"));
            }
        } else {
            out.push_str("live run complete and valid\n");
        }
        // Before the figures, always. A paced throughput and a work-conserving one are not
        // comparable, so the mode has to be read before the number it qualifies.
        out.push_str(&format!("  mode              {}\n", self.mode));
        out.push_str(&format!(
            "  requests          {}\n  key references    {}\n",
            self.requests, self.key_references
        ));
        if let Some(s) = &self.schedule {
            out.push_str(&format!(
                "  schedule          rate {:.3} virtual s/wallclock s, achieved {:.3} ({:+.1}% \
                 short)\n  \
                 lateness us       p50 {} p90 {} p99 {} max {} over {} turns, tolerance {}\n",
                s.rate,
                self.virtual_to_wallclock,
                s.rate_shortfall * 100.0,
                s.lateness_us.p50,
                s.lateness_us.p90,
                s.lateness_us.p99,
                s.lateness_us.max,
                s.turns,
                s.tolerance_us,
            ));
            if !s.kept {
                out.push_str(
                    "                    the schedule was NOT kept, so this machine could not \
                     serve this workload at this rate; the latency below describes a queue the \
                     workload would not have formed (FR-080)\n",
                );
            }
        }
        if self.valid {
            out.push_str(&format!(
                "  payload bw        read {:.1} MiB/s ({} blocks), write {:.1} MiB/s ({} \
                 blocks)\n                    control operations move no payload, so this \
                 comes from the hit/miss results\n  \
                 request rate      {:.0} key references/s\n  \
                 virtual/wallclock {:.2}\n",
                self.bandwidth.read_bytes as f64
                    / self.elapsed_seconds.max(1e-9)
                    / (1024.0 * 1024.0),
                self.bandwidth.blocks_read,
                self.bandwidth.write_bytes as f64
                    / self.elapsed_seconds.max(1e-9)
                    / (1024.0 * 1024.0),
                self.bandwidth.blocks_written,
                self.keys_per_second,
                self.virtual_to_wallclock
            ));
        }
        out.push_str(&format!(
            "  latency us        p50 {} p90 {} p99 {} max {}\n",
            self.latency_us.p50, self.latency_us.p90, self.latency_us.p99, self.latency_us.max
        ));
        out.push_str(&format!(
            "  plan queue        {} batches produced, {} pops, {} underran ({:.3}%), \
             min depth {} of {} per lane\n    \
             per lane underran {:?}, min depth {:?}\n    \
             producer blocked on a full queue {} times{}\n",
            self.queue.batches_produced,
            self.queue.pops,
            self.queue.underruns,
            self.queue.fraction_underrun * 100.0,
            self.queue.min_depth,
            self.queue.capacity_per_lane,
            self.queue.per_lane_underruns,
            self.queue.per_lane_min_depth,
            self.queue.producer_blocked,
            if self.queue.producer_blocked > 0 {
                " (backpressure working: the generator was ahead)"
            } else {
                " (the generator never got ahead of the lanes)"
            },
        ));
        if !self.producer_completed {
            out.push_str(
                "  interrupted       the producer was stopped before its span ended; an \
                 interrupted run whose lanes never underran is still valid (FR-074)\n",
            );
        }
        if let Some(cleared) = self.cleared_entries {
            out.push_str(&format!(
                "  cache cleared     {cleared} memory-tier entries dropped before the \
                 timed window (FR-046); disk-backed entries survive a clear\n"
            ));
        }
        // Led with, and before any throughput: a wrong block is a correctness failure, not a
        // performance figure, and it must not be read past.
        if self.outcomes.payload_mismatches > 0 {
            out.push_str(&format!(
                "  WRONG DATA        {} of {} loaded blocks carried a different key than the \
                 one asked for. Certus returned the wrong block; this is not a cache \
                 outcome and no throughput below is meaningful\n",
                self.outcomes.payload_mismatches, self.outcomes.payloads_verified
            ));
        } else if self.outcomes.payloads_verified > 0 {
            out.push_str(&format!(
                "  payload verified  {} loaded blocks carried the key they were asked for\n",
                self.outcomes.payloads_verified
            ));
        }
        if let Some(rate) = self.outcomes.check_hit_rate() {
            out.push_str(&format!(
                "  check             {} resident, {} pending, {} miss of {} ({:.1}% \
                 resident)\n",
                self.outcomes.check_resident,
                self.outcomes.check_pending,
                self.outcomes.check_miss,
                self.outcomes.checks(),
                rate * 100.0
            ));
        }
        if let Some(rate) = self.outcomes.lookup_hit_rate() {
            out.push_str(&format!(
                "  lookup            {} returned data, {} did not ({:.1}% hit)\n",
                self.outcomes.lookup_hits,
                self.outcomes.lookup_misses,
                rate * 100.0
            ));
        }
        if self.outcomes.any_store_declined() {
            out.push_str(&format!(
                "  stores declined   {} of {} reserves ({}), {} transfers, {} of {} \
                 commits ({}). The wire carries no reason code, so these are \
                 observations and not a diagnosis\n",
                self.outcomes.reserves_declined,
                self.outcomes.reserves_attempted,
                pct(
                    self.outcomes.reserves_declined,
                    self.outcomes.reserves_attempted
                ),
                self.outcomes.transfers_declined,
                self.outcomes.commits_declined,
                self.outcomes.commits_attempted,
                pct(
                    self.outcomes.commits_declined,
                    self.outcomes.commits_attempted
                ),
            ));
        }
        if !self.latency_by_op.is_empty() {
            out.push_str("  latency by op     (us)  requests    p50    p90    p99    max\n");
            let mut thin = false;
            for l in &self.latency_by_op {
                let mark = if l.requests < QUOTABLE_REQUESTS {
                    thin = true;
                    " <- too few to quote"
                } else {
                    ""
                };
                out.push_str(&format!(
                    "    {:<14} {:>10} {:>6} {:>6} {:>6} {:>6}{}\n",
                    l.op, l.requests, l.us.p50, l.us.p90, l.us.p99, l.us.max, mark
                ));
            }
            if thin {
                out.push_str(&format!(
                    "                    a p50 over a few dozen requests is noise and the \
                     p99 is just the max;\n                    {QUOTABLE_REQUESTS}+ before \
                     comparing operations\n"
                ));
            }
        }
        out.push_str(&format!(
            "  lanes             {} of {} channels\n",
            self.lanes, self.node_channels
        ));
        if self.skipped_needing_gpu > 0 {
            out.push_str(&format!(
                "  PARTIAL RUN       {} operations ({} keys) were NOT issued: LOOKUP and \
                 COPY_TO_STORE need a GPU IPC handle per key, which this build has no \
                 payload buffer for. The control path was exercised; no data moved, so \
                 this throughput is not comparable with a complete run's.\n",
                self.skipped_needing_gpu, self.skipped_keys
            ));
        }
        out.push_str(&format!(
            "  reproduce with    --seed {} --lanes {} --batch-keys {}   description {} ({})\n",
            self.reproduction.seed,
            self.tuning.lanes,
            self.tuning.batch_keys,
            self.reproduction.description_path,
            self.reproduction.description_digest
        ));
        out
    }

    /// The structured form.
    ///
    /// # Errors
    ///
    /// If serialisation fails, which for this type means a bug.
    pub fn to_json(&self) -> serde_json::Result<String> {
        let mut s = serde_json::to_string_pretty(self)?;
        s.push('\n');
        Ok(s)
    }
}
