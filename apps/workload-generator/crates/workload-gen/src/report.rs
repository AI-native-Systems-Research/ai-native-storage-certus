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
    /// **Whether this run's numbers may be used at all** (FR-062).
    ///
    /// First field on purpose: a reader scanning a directory of reports should meet the
    /// validity before the throughput, not after it.
    pub valid: bool,
    /// Why, when it is not.
    pub invalid_reason: Option<String>,
    /// Requests issued.
    pub requests: u64,
    /// Key references issued.
    pub key_references: u64,
    /// Keys per second over the timed window.
    pub keys_per_second: f64,
    /// Bytes per second over the timed window.
    pub bytes_per_second: f64,
    /// Virtual seconds advanced per wallclock second.
    pub virtual_to_wallclock: f64,
    /// Wallclock seconds of the timed window, excluding any startup cache clear.
    pub elapsed_seconds: f64,
    /// Virtual seconds covered.
    pub virtual_span: f64,
    /// Smallest plan-queue depth observed. Zero invalidates the run.
    pub plan_queue_min_depth: usize,
    /// Fraction of samples at zero depth.
    pub plan_queue_fraction_at_zero: f64,
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
            if let Some(why) = &self.invalid_reason {
                out.push_str(&format!("  reason            {why}\n"));
            }
        } else {
            out.push_str("live run complete and valid\n");
        }
        out.push_str(&format!(
            "  requests          {}\n  key references    {}\n",
            self.requests, self.key_references
        ));
        if self.valid {
            out.push_str(&format!(
                "  throughput        {:.0} keys/s, {:.1} MiB/s\n  \
                 virtual/wallclock {:.2}\n",
                self.keys_per_second,
                self.bytes_per_second / (1024.0 * 1024.0),
                self.virtual_to_wallclock
            ));
        }
        out.push_str(&format!(
            "  latency us        p50 {} p90 {} p99 {} max {}\n",
            self.latency_us.p50, self.latency_us.p90, self.latency_us.p99, self.latency_us.max
        ));
        out.push_str(&format!(
            "  plan queue        min depth {}, {:.3}% of samples at zero\n",
            self.plan_queue_min_depth,
            self.plan_queue_fraction_at_zero * 100.0
        ));
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
