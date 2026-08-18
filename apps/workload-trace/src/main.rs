//! `certus-trace` — fit a workload model from a real trace, and validate a plan
//! against one.
//!
//! A separate binary from `certus-workload` because the two point in opposite
//! directions: the generator turns parameters into keys, `fit` turns keys into
//! parameters. Different inputs, different failure modes, no shared control flow.
//!
//! Every statistic this binary reports comes from `workload_model::stats` and none is
//! implemented here (spec FR-021i). That is not tidiness: `validate` compares a
//! fitted model against the trace it was fitted from, and a second implementation of
//! reuse distance would make that a comparison of two definitions rather than of two
//! measurements — a failure that reports itself as a success.
//!
//! `convert` — the parquet output mode of FR-021h — is behind the default-off
//! `parquet` feature, together with the parquet reader. `fit` and `validate` accept
//! either container in either build; a parquet trace named by a build without the
//! feature is refused by name rather than read as empty.

use std::path::{Path, PathBuf};
use std::process::ExitCode;

use clap::{Parser, Subcommand};
use workload_model::corpus::Profile;
use workload_model::fit::branching;
use workload_model::fit::document::{assemble, RootPopularity, Supplied};
use workload_model::fit::sessions::{scale_values, SessionShapes};
use workload_model::plan::{read_plan, Generator, PlanEvent};
use workload_model::stats::divergence::{
    compare, Measure, Statistic, Tolerances, DEFAULT_TOLERANCE_MIN_REQUESTS,
};
use workload_model::stats::{Ref, Report, Statistics};

#[cfg(feature = "parquet")]
mod parquet;
mod read;

/// Iterations the fit will spend raising the attempted sharing to meet the realised.
///
/// Each one regenerates the whole plan, so the cap is a real cost bound rather than a
/// formality. Eight is enough for the ratio step to converge geometrically from any
/// starting shortfall within the clamp, and a fit that has not converged by then is
/// reported as not converged rather than ground on.
const MAX_FIT_ITERATIONS: usize = 8;

#[derive(Parser)]
#[command(
    name = "certus-trace",
    version,
    about = "Fit a workload model from a trace, and validate a plan against one",
    long_about = None
)]
struct Cli {
    #[command(subcommand)]
    cmd: Cmd,
}

#[derive(Subcommand)]
enum Cmd {
    /// Fit a workload model from a trace, and validate it against that trace.
    ///
    /// The whole loop in one command: measure the trace, emit the YAML, generate a
    /// plan from it, and compare the two. FR-057 requires the fit to **fail** rather
    /// than emit a model whose divergence exceeds tolerance, so the YAML is written
    /// only when the comparison passes.
    Fit {
        /// The trace to fit: a `.jsonl` file, or a directory holding a parquet trace.
        #[arg(short = 't', long, value_name = "PATH")]
        trace: PathBuf,
        /// Where to write the fitted YAML. Omit to print the report only.
        #[arg(short = 'o', long, value_name = "FILE")]
        out: Option<PathBuf>,
        /// Which blocking to read, in **tokens** per block.
        ///
        /// Distinct from `--block-bytes`, which is a payload size. A trace may carry
        /// several blockings as sibling partitions; this names one. Defaults to the
        /// manifest's `block_size`, and is required where a trace declares none.
        #[arg(long, value_name = "TOKENS")]
        block_size: Option<u32>,
        /// `corpus.block_bytes`, which no trace can supply: its block size is
        /// **tokens**, and converting needs the model geometry no trace carries.
        #[arg(long, value_name = "BYTES")]
        block_bytes: u64,
        /// The measurement window, in requests (FR-009h).
        #[arg(long, default_value_t = 20_000, value_name = "N")]
        wss_window: u64,
        /// The seed the emitted document carries. A property of the sample rather
        /// than of the workload, so it is stated rather than invented.
        #[arg(long, default_value_t = 1, value_name = "N")]
        seed: u64,
        /// Requests per second, for a trace whose timestamps cannot supply one.
        #[arg(long, value_name = "RATE")]
        rate: Option<f64>,
        /// Accept a trace shorter than its manifest declares. Refused by default:
        /// sharing, width and reuse distance are properties of the whole stream and
        /// every one is understated by a prefix of it (FR-055e).
        #[arg(long)]
        allow_partial: bool,
        /// Per-statistic tolerance overrides, as `name=value`.
        #[arg(long = "tolerance", value_name = "NAME=VALUE")]
        tolerances: Vec<String>,
        /// Print the bucket-by-bucket working behind each divergence.
        ///
        /// A divergence number says how much two distributions differ and cannot say
        /// **where**, which is the part that decides what to change: a KS distance
        /// whose medians agree is a tail or shoulder problem and wants a different
        /// parameter than a uniform shift would.
        #[arg(long)]
        explain: bool,
        /// Emit the node-level `branching: {by_depth: [...]}` spelling.
        ///
        /// OFF by default because it is not yet an improvement, and the reason is
        /// specific: the spelling states a split's **total** out-degree, but the law
        /// choosing among those children is still the document-wide `branch_skew`. On
        /// `qwen_code` the census measures a 4739-way root split with 0.496 of sessions on
        /// its top child, while `branch_skew: 0.9` puts 0.054 there — so sessions scatter
        /// about nine times more than the trace and sharing collapses (`sharing_depth`
        /// 0.060 -> 0.364). Out-degree and the child law are a pair; fitting one without
        /// the other is worse than fitting neither.
        ///
        /// The flag exists so the two can be A/B'd on one trace as the child law is
        /// fitted, rather than the capability sitting unexercised until it is finished.
        #[arg(long)]
        branching_segments: bool,
    },
    /// Compare two reference streams statistic by statistic.
    ///
    /// Either two plans, or a plan against a trace. Both forms answer the same
    /// question — do these two streams have the same shape — and both report
    /// per-statistic divergence and fail rather than emit a verdict that hides one
    /// (FR-057).
    Validate {
        /// The plan directory to compare.
        #[arg(short = 'p', long, value_name = "DIR")]
        plan: PathBuf,
        /// A second plan to compare against.
        #[arg(long, value_name = "DIR", conflicts_with = "trace")]
        against_plan: Option<PathBuf>,
        /// A trace to compare against: a `.jsonl` file or a parquet directory.
        #[arg(long, value_name = "PATH")]
        trace: Option<PathBuf>,
        /// Which blocking to read, in **tokens** per block. See `fit --block-size`.
        #[arg(long, value_name = "TOKENS")]
        block_size: Option<u32>,
        /// Accept a trace shorter than its manifest declares.
        ///
        /// `fit` may never do this (FR-055e); `validate` may, because comparing
        /// shapes over a sample is a weaker claim rather than an invalid one — and
        /// the report says the sample was partial.
        #[arg(long)]
        allow_partial: bool,
        /// Per-statistic tolerance overrides, as `name=value`.
        ///
        /// On the command line and never in the YAML: fitting is an operation
        /// performed *on* a model, not a property *of* one (FR-057a).
        #[arg(long = "tolerance", value_name = "NAME=VALUE")]
        tolerances: Vec<String>,
        /// Emit JSON instead of the human summary.
        #[arg(long)]
        json: bool,
        /// Print the bucket-by-bucket working behind each divergence.
        #[arg(long)]
        explain: bool,
    },
    /// Convert a plan's `events.bin` into a parquet trace (FR-021h, mode 3).
    ///
    /// The columnar half of the interchange format. It lives in this binary rather
    /// than the generator because `arrow` would otherwise be built by every
    /// `cargo test --all`, and because FR-021c already requires modes 2 and 3 to be
    /// producible from an existing `events.bin` **without regenerating** — so
    /// conversion was always independent of generation.
    ///
    /// Writes the same records `certus-workload emit` writes as JSONL, including
    /// skipping warmup requests: a warmup window is a property of a measured run,
    /// not of a workload, and this schema gives an invocation no field to say it was
    /// one.
    #[cfg(feature = "parquet")]
    Convert {
        /// The plan directory to convert.
        #[arg(short = 'p', long, value_name = "DIR")]
        plan: PathBuf,
        /// The trace directory to write: `manifest.json` plus
        /// `invocations/block_size_<N>/part-0.parquet`.
        #[arg(short = 'o', long, value_name = "DIR")]
        out: PathBuf,
        /// Tokens per block, which sets the partition name.
        #[arg(long, default_value_t = workload_model::trace::DEFAULT_BLOCK_SIZE_TOKENS, value_name = "TOKENS")]
        block_size: u32,
        /// Block budget, in references. Honoured at request granularity: a truncated
        /// request is not a request, and a reader reconstructing block lists from one
        /// would see a shorter conversation rather than an obviously partial file.
        #[arg(long, default_value_t = u64::MAX, value_name = "N")]
        blocks: u64,
    },
}

fn main() -> ExitCode {
    let cli = Cli::parse();
    let r = match cli.cmd {
        Cmd::Fit {
            trace,
            out,
            block_size,
            block_bytes,
            wss_window,
            seed,
            rate,
            allow_partial,
            tolerances,
            explain,
            branching_segments,
        } => cmd_fit(
            &trace,
            out.as_deref(),
            block_size,
            block_bytes,
            wss_window,
            seed,
            rate,
            allow_partial,
            &tolerances,
            explain,
            branching_segments,
        ),
        Cmd::Validate {
            plan,
            against_plan,
            trace,
            block_size,
            allow_partial,
            tolerances,
            json,
            explain,
        } => cmd_validate(
            &plan,
            against_plan.as_deref(),
            trace.as_deref(),
            block_size,
            allow_partial,
            &tolerances,
            json,
            explain,
        ),
        #[cfg(feature = "parquet")]
        Cmd::Convert {
            plan,
            out,
            block_size,
            blocks,
        } => cmd_convert(&plan, &out, block_size, blocks),
    };
    match r {
        Ok(true) => ExitCode::SUCCESS,
        // A divergence beyond tolerance is a failed comparison, not a broken tool,
        // so it exits non-zero without the "certus-trace:" error prefix.
        Ok(false) => ExitCode::FAILURE,
        Err(e) => {
            eprintln!("certus-trace: {e}");
            ExitCode::FAILURE
        }
    }
}

/// Parse `--tolerance name=value` overrides onto the derived defaults.
fn parse_tolerances(args: &[String]) -> Result<Tolerances, String> {
    let mut t = Tolerances::default();
    for a in args {
        let (name, value) = a
            .split_once('=')
            .ok_or_else(|| format!("--tolerance wants NAME=VALUE, got `{a}`"))?;
        let v: f64 = value
            .parse()
            .map_err(|_| format!("--tolerance {name}: `{value}` is not a number"))?;
        match name {
            "reuse_distance_objects" => t.reuse_distance_objects = v,
            "reuse_distance_bytes" => t.reuse_distance_bytes = v,
            "sharing_depth" => t.sharing_depth = v,
            "request_length" => t.request_length = v,
            "unique_keys" => t.unique_keys = v,
            other => {
                return Err(format!(
                    "unknown statistic `{other}`; the ones FR-056 names are \
                     reuse_distance_objects, reuse_distance_bytes, sharing_depth, \
                     request_length, unique_keys"
                ))
            }
        }
    }
    Ok(t)
}

/// Fit a model from a trace, validate it against that trace, and write it (T085, T087).
///
/// The order matters and is FR-057's: measure, assemble, generate, compare, and only
/// then write. A fitted model whose synthetic output does not resemble its source is
/// not a weak result — it is a wrong one, and emitting it would put a plausible YAML
/// in front of someone who would reasonably trust it.
/// Convert a plan into a parquet trace (T084, FR-021h mode 3).
///
/// Deliberately the same record construction as `certus-workload emit`'s JSONL path,
/// through the same `trace::Emitter`: the two modes are one schema in two containers,
/// and a second emitter would make that a claim rather than a fact. What differs is
/// only where the bytes go.
#[cfg(feature = "parquet")]
fn cmd_convert(plan: &Path, out: &Path, block_size: u32, blocks: u64) -> Result<bool, String> {
    use workload_model::plan::record::flags;
    use workload_model::trace::{requests, Emitter, TraceManifest};

    let (m, events) = workload_model::plan::read_plan(plan).map_err(|e| e.to_string())?;
    let trace_id = out
        .file_name()
        .and_then(|s| s.to_str())
        .unwrap_or("synthetic")
        .to_string();
    let mut em = Emitter::new(&trace_id, block_size, m.time_origin_ns);
    let mut rows = Vec::new();
    let mut written = 0u64;
    let mut truncated = false;
    let mut warmup_skipped = 0u64;
    for r in requests(&events) {
        // Warmup is withheld here for the same reason `emit` withholds it: a warmup
        // window says which operations a *report* excludes (FR-045), which is a
        // property of a measured run rather than of a workload, and this schema gives
        // an invocation no field in which to say it was one. `events.bin` keeps the
        // flag, so nothing is lost from the native artifact.
        if r.first().is_some_and(|e| e.has(flags::WARMUP)) {
            warmup_skipped += 1;
            continue;
        }
        if written + r.len() as u64 > blocks {
            truncated = true;
            break;
        }
        if let Some(rec) = em.request(r) {
            rows.push(rec);
            written += r.len() as u64;
        }
    }

    let stats = em.stats();
    let manifest = TraceManifest::synthetic(&trace_id, em.block_size(), stats.clone());
    std::fs::create_dir_all(out).map_err(|e| format!("{}: {e}", out.display()))?;
    let part = crate::parquet::write_trace(out, &manifest, em.block_size(), &rows)
        .map_err(|e| e.to_string())?;
    println!(
        "{} invocations, {} sessions, {} unique blocks, {written} blocks\n{}\nmanifest: {} \
         (provenance synthetic, timestamps synthetic)",
        stats.invocations,
        stats.sessions,
        stats.unique_blocks,
        part.display(),
        out.join("manifest.json").display(),
    );
    if warmup_skipped > 0 {
        println!(
            "note: {warmup_skipped} warmup requests were not converted. A warmup window \
             belongs to a measured run, not to a workload, so the trace is the plan's \
             measured window — which is also what makes the two compare exactly"
        );
    }
    if truncated {
        eprintln!(
            "note: stopped at the {blocks}-block budget; the plan carries {} events",
            events.len()
        );
    }
    Ok(true)
}

#[allow(clippy::too_many_arguments)]
fn cmd_fit(
    trace_path: &Path,
    out: Option<&Path>,
    block_size: Option<u32>,
    block_bytes: u64,
    wss_window: u64,
    seed: u64,
    rate: Option<f64>,
    allow_partial: bool,
    tolerance_args: &[String],
    explain: bool,
    branching_segments: bool,
) -> Result<bool, String> {
    let tol = parse_tolerances(tolerance_args)?;
    let trace =
        read::read_trace(trace_path, allow_partial, block_size).map_err(|e| e.to_string())?;
    if !trace.capabilities.trunk_fittable() {
        return Err(
            "MODEL LIMITATION (FR-054a): this trace carries no session identity, and this \
             model's corpus is defined in terms of sessions — cross-session sharing is what \
             distinguishes a trunk from one long private path, and occupancy has no denominator \
             without it. The trace is valid and its arrival and size distributions are \
             measurable, which is `supports: R = partial` doing what it says; what is missing is \
             a corpus model that can be fitted without session grouping"
                .into(),
        );
    }

    // One pass drives every measurement, so the sharing prefix a `private_depth`
    // subtracts is the same one a validator recomputes.
    let mut stats = Statistics::new(wss_window);
    let mut sharing = workload_model::stats::sharing::Sharing::new();
    let mut window = workload_model::stats::WindowTable::new();
    let mut shapes = SessionShapes::new();
    let mut roots = RootPopularity::new();
    let mut window_requests = 0u64;

    for inv in &trace.invocations {
        if inv.blocks.is_empty() {
            continue;
        }
        if let Some(root) = inv.blocks.first() {
            roots.observe(inv.session.0, *root);
        }
        let refs: Vec<Ref> = inv
            .blocks
            .iter()
            .enumerate()
            .map(|(depth, key)| Ref {
                key: *key,
                size: trace.capabilities.block_size,
                depth: depth as u32,
                session: inv.session,
                request_start: depth == 0,
                warmup: false,
            })
            .collect();
        for r in &refs {
            stats.push(r);
            sharing.observe(r, &window);
            window.observe(r);
        }
        sharing.end_request();
        window.end_request();
        shapes.observe(
            inv.session.0,
            inv.turn,
            inv.blocks.len() as u64,
            sharing.last_prefix_len(),
            inv.request_start,
        );
        window_requests += 1;
        if window_requests >= wss_window {
            window.reset();
            window_requests = 0;
        }
    }

    let trace_report = stats.finish();
    let sharing_report = sharing.finish();
    let fitted_sessions = shapes.finish(&sharing_report);

    let fitted_branching =
        branching::fit(&trace_report.trunk).ok_or("the trace has no width profile to fit")?;

    // A rate from the trace's own span where it has one; the caller's otherwise.
    let measured_rate = if trace.chronological {
        let firsts: Vec<f64> = trace
            .invocations
            .iter()
            .filter_map(|i| i.request_start)
            .collect();
        match (firsts.first(), firsts.last()) {
            (Some(a), Some(b)) if b > a => Some(firsts.len() as f64 / (b - a)),
            _ => None,
        }
    } else {
        None
    };

    let supplied = Supplied {
        block_bytes: workload_model::dist::Dist::Scalar(block_bytes as f64),
        rate_per_s: rate.or(measured_rate),
        wss_window_requests: wss_window,
        seed,
    };
    // The segment census, and the node-level trunk process fitted from it. Built once:
    // it is the only description that can state per-root preamble lengths and the TOTAL
    // out-degree a session needs in order to land on a child nobody else took.
    let fitted_segments = if !branching_segments {
        None
    } else {
        let mut order: Vec<usize> = (0..trace.invocations.len()).collect();
        order.sort_by_key(|i| (trace.invocations[*i].session.0, trace.invocations[*i].turn));
        let mut census = workload_model::fit::segments::Census::new();
        for i in order {
            census.observe(trace.invocations[i].session, &trace.invocations[i].blocks);
        }
        workload_model::fit::segments::fit_process(&census.finish(2))
    };
    let fitted_process = fitted_segments.as_ref().map(|f| &f.process);
    let fitted = assemble(
        &fitted_branching,
        fitted_process,
        &fitted_sessions,
        &roots,
        &supplied,
        trace_report.requests,
        trace.chronological,
    )
    .map_err(|e| e.to_string())?;
    let doc = fitted.document.clone().expect("assembled");

    // FR-055a: fail rather than emit a combination the generator cannot realise.
    let findings = workload_model::schema::validate::validate(&doc);
    let rejections: Vec<String> = findings
        .rejections()
        .map(|f| format!("[rule {}] {}", f.rule, f.message))
        .collect();

    println!("fit");
    println!("  trace     {}", trace_path.display());
    println!(
        "  measured  {} requests, {} references, {} sessions, {} roots, block size {} tokens",
        trace_report.requests,
        trace_report.references,
        fitted.sessions,
        roots.roots(),
        trace.capabilities.block_size
    );
    println!(
        "  order     {}",
        if trace.chronological {
            "chronological"
        } else {
            "FILE ORDER (FR-055d)"
        }
    );
    println!(
        "  trunk     roots.count {} (shared keys at depth 0), {} segments, fitted to depth {} \
         of {} (retention {:.3} there)",
        fitted_branching.roots,
        fitted_branching.segments.len(),
        fitted_branching.fitted_to_depth,
        fitted_branching.observed_to_depth,
        fitted_branching.retention_at_fitted_to
    );

    // FR-055e: which fitted values came from a reconstructed field. Printed before
    // the values themselves, because it qualifies all of them.
    let provenance = trace.capabilities.provenance();
    if fitted_sessions.max_depth.is_some() {
        // The emitted parameter is a distribution over where sessions topped out; the
        // scalar solve is printed beside it because the two disagreeing is informative.
        // A scalar reproduces the accumulated depth and destroys the distribution
        // (FR-054c), so a large gap between them says how much of the ceiling's effect
        // is concentrated in the saturating tail.
        let q = shapes.deepest_depth();
        println!(
            "\n  depth ceiling  sessions.max_depth is drawn per session from where sessions \
             topped out:\n                 p50 {} p90 {} p99 {} max {} blocks (FR-054c). A single \
             ceiling solved\n                 against the same accumulation would be {}.",
            q.p50.unwrap_or(0),
            q.p90.unwrap_or(0),
            q.p99.unwrap_or(0),
            q.max.unwrap_or(0),
            shapes
                .fit_max_depth()
                .map(|c| format!("{c} blocks"))
                .unwrap_or_else(|| "unset".into())
        );
    }

    if let Some(workload_model::schema::Growth::Banded(b)) = &fitted_sessions.growth_per_turn {
        // The bands are a fitting decision, so they are shown with the sessions behind
        // them: a band fitted from a handful of sessions is a thin measurement and
        // FR-055 requires a reader to be able to see that.
        let counts = shapes.growth_band_sessions();
        println!(
            "\n  growth bands   growth_per_turn is banded by session length, because a session's \
             accumulated\n                 depth weights each increment by (turns - position) and \
             so is quadratic in\n                 length (FR-054c). {} bands from {} rungs:",
            b.by_turns.len(),
            counts.len()
        );
        for band in &b.by_turns {
            let sessions: u64 = counts
                .iter()
                .filter(|(from, _)| *from >= u64::from(band.from_turns))
                .map(|(_, n)| *n)
                .sum();
            println!(
                "                   from {:>4} turns: mean {:>6.1} blocks/turn, {} sessions at or \
                 above this rung",
                band.from_turns,
                band.growth.mean().unwrap_or(0.0),
                sessions
            );
        }
    }

    println!("\n  provenance");
    let reconstructed: Vec<_> = provenance.iter().filter(|p| p.is_reconstructed()).collect();
    if reconstructed.is_empty() {
        println!(
            "    - every field this fit read is native: {}",
            provenance
                .iter()
                .map(|p| p.field.as_str())
                .collect::<Vec<_>>()
                .join(", ")
        );
    } else {
        for p in &provenance {
            println!("    - {p}");
        }
    }

    println!("\n  not fitted");
    for u in &fitted.unset {
        println!("    - {u}");
    }
    println!("\n  caveats");
    for c in &fitted.caveats {
        println!("    - {c}");
    }

    // Printed before any refusal, because a schema rejection is usually *about* the
    // trunk and the profile is what says whether the fit read the trace wrongly or
    // the trace genuinely describes something the model cannot express.
    if explain {
        print_trunk_profile(&trace_report, &fitted_branching);
        print_segment_census(
            &trace.invocations,
            fitted_segments.as_ref().map(|f| f.skews.as_slice()),
        );
        // After the census, so the fitted bands are read against the measurement they came
        // from: the two tables carry the same quantities per the same bands, and the whole
        // point is to diff them.
        if let Some(f) = fitted_segments.as_ref() {
            print_fitted_process(f);
        }
    }

    if !rejections.is_empty() {
        // FR-054a: the trace is ground truth, so a fitted document that fails our own
        // schema is this model's restrictions failing to cover a real workload. Rule 16
        // in particular is the generator's occupancy floor — a statement about what the
        // generator can realise, not about what a trace is allowed to contain.
        println!(
            "\n  MODEL LIMITATION (FR-054a): the measured parameters do not satisfy this \
             model's own constraints, so the generator could not realise them. The trace is \
             ground truth here — what follows is a restriction of the model that a real \
             workload does not respect, not a defect in the data"
        );
        for r in &rejections {
            println!("    {r}");
        }
        return Ok(false);
    }

    // Generate from the fitted model and compare it against the trace it came from.
    // Ground truth is the trace, so this is the only check that says whether the fit
    // is any good — and it is also the feedback the iteration below needs.
    //
    // Why iterate at all: `shared_depth` is fitted from *realised* sharing but the
    // generator consumes it as what a session **attempts**, and FR-012a makes the drawn
    // value an upper bound on the realised one. Feed the realised value back in as an
    // attempt and realised sharing comes out short. So the attempt is raised until the
    // realised sharing it produces matches the trace's.
    //
    // The two moves have to happen together. Raising the attempt alone lengthens every
    // path by the shortfall, because `private_depth` was measured as
    // `turn-1 depth − realised prefix`; so each iteration also recomputes
    // `private_depth` against the raised attempt, which holds path length fixed while
    // sharing moves. Without that, the loop appears to converge on sharing while
    // quietly ruining request length.
    let target_sharing = trace_report.sharing.realised_depth.p50.unwrap_or(0) as f64;
    let mut scale = 1.0f64;
    let mut best: Option<(f64, workload_model::schema::Document, Report, f64, usize)> = None;
    let mut history: Vec<String> = Vec::new();

    for iteration in 0..MAX_FIT_ITERATIONS {
        let candidate = if iteration == 0 {
            doc.clone()
        } else {
            // Re-assemble with the raised attempt and the matching private part.
            let mut adjusted = fitted_sessions.clone();
            adjusted.shared_depth = fitted_sessions
                .shared_depth
                .as_ref()
                .map(|d| scale_values(d, scale));
            adjusted.private_depth = shapes.private_depth_at(scale);
            match assemble(
                &fitted_branching,
                fitted_process,
                &adjusted,
                &roots,
                &supplied,
                trace_report.requests,
                trace.chronological,
            ) {
                Ok(f) => f.document.expect("assembled"),
                Err(e) => {
                    history.push(format!("iteration {iteration}: {e}"));
                    break;
                }
            }
        };

        // A raised attempt can push the combination past the occupancy floor, which is
        // a real stop rather than something to iterate through (FR-055a).
        if workload_model::schema::validate::validate(&candidate).is_rejected() {
            history.push(format!(
                "iteration {iteration} at scale {scale:.3}: the raised attempt no longer \
                 passes the schema, so the search stops here"
            ));
            break;
        }

        let mut g = Generator::new(&candidate).map_err(|e| e.to_string())?;
        let mut plan_stats = Statistics::new(wss_window);
        let mut chunk: Vec<PlanEvent> = Vec::new();
        while !g.is_done() {
            chunk.clear();
            if g.fill(&mut chunk) == 0 {
                break;
            }
            plan_stats.push_events(&chunk);
        }
        let synthetic = plan_stats.finish();

        let mut d = compare(&synthetic, &trace_report, &tol);
        d.mark_incomparable(
            Statistic::ReuseDistanceBytes,
            "the synthetic plan's sizes are the supplied block_bytes and the trace's are \
             tokens per block, so this compares units rather than workloads",
        );
        // Worst *relative* excess over tolerance, so statistics on different scales are
        // comparable: a KS of 0.2 against 0.05 is as bad as a log-ratio of 0.6 against
        // 0.15, and picking the best candidate by absolute divergence would let the
        // loosest statistic decide.
        let worst = d
            .divergences
            .iter()
            .filter(|x| x.incomparable.is_none() && x.tolerance > 0.0)
            .map(|x| x.value / x.tolerance)
            .fold(0.0f64, f64::max);

        let realised = synthetic.sharing.realised_depth.p50.unwrap_or(0) as f64;
        history.push(format!(
            "iteration {iteration}: attempt scale {scale:.3}, realised sharing p50 \
             {realised:.0} against the trace's {target_sharing:.0}, worst divergence \
             {worst:.2}x its tolerance"
        ));

        let improved = best.as_ref().map(|(w, ..)| worst < *w).unwrap_or(true);
        if improved {
            best = Some((worst, candidate, synthetic, scale, iteration));
        }
        if worst <= 1.0 {
            break;
        }

        // Raise the attempt by the shortfall. Clamped per step so one noisy
        // measurement cannot send the search somewhere it will not come back from.
        if realised <= 0.0 || target_sharing <= 0.0 {
            break;
        }
        let step = (target_sharing / realised).clamp(0.5, 2.0);
        if (step - 1.0).abs() < 0.01 {
            break;
        }
        scale = (scale * step).clamp(1.0, 16.0);
    }

    let (worst, doc, synthetic, best_scale, best_iteration) =
        best.ok_or("no candidate model could be generated")?;

    println!("\n  iterations");
    for h in &history {
        println!("    {h}");
    }
    println!(
        "  best      iteration {best_iteration} at attempt scale {best_scale:.3}, worst \
         divergence {worst:.2}x its tolerance"
    );
    if best_scale > 1.0 {
        println!(
            "  note      shared_depth is emitted {best_scale:.3}x the realised sharing, \
             because the generator reads it as what a session *attempts* and realised \
             sharing falls short of that (FR-012a). private_depth was lowered to match, so \
             path length is unchanged"
        );
    }

    println!(
        "\n  synthetic {} requests, {} references from the fitted model",
        synthetic.requests, synthetic.references
    );
    let mut d = compare(&synthetic, &trace_report, &tol);
    d.mark_incomparable(
        Statistic::ReuseDistanceBytes,
        "the synthetic plan's sizes are the supplied block_bytes and the trace's are tokens \
         per block, so this compares units rather than workloads",
    );
    println!(
        "\n  {:<24} {:>10} {:>10} {:>9}  verdict",
        "statistic", "divergence", "tolerance", "samples"
    );
    for x in &d.divergences {
        println!(
            "  {:<24} {:>10.5} {:>10.5} {:>9}  {}",
            x.statistic.name(),
            x.value,
            x.tolerance,
            x.samples,
            match &x.incomparable {
                Some(_) => "incomparable",
                None if x.within => "within",
                None => "EXCEEDED",
            }
        );
    }

    if explain {
        print_path_budget(&trace_report, &synthetic, &shapes.path_budget());
        print_sharing_spaces(&trace_report, &mut shapes);
        print_explanation(&synthetic, &trace_report, &d, "synthetic", "trace");
    }

    if !d.within_tolerance() {
        println!(
            "\n  MODEL LIMITATION (FR-054a): the best fit this model admits does not reproduce \
             the trace, so the model cannot express this workload to within the stated \
             tolerances. The divergences above are the measure of the shortfall, and the trace \
             is ground truth — this is not a verdict on the data.\n  Nothing is written: FR-057 \
             refuses to emit a model whose output does not resemble its source, because a \
             plausible YAML nobody can tell is wrong is worse than no YAML"
        );
        return Ok(false);
    }

    match out {
        Some(path) => {
            let yaml = doc.to_yaml().map_err(|e| e.to_string())?;
            std::fs::write(path, &yaml).map_err(|e| format!("{}: {e}", path.display()))?;
            println!("\n  wrote {}", path.display());
        }
        None => println!("\n  every statistic is within tolerance (no --out, so nothing written)"),
    }
    Ok(true)
}

/// The report for a plan directory, and the window it was generated against.
fn plan_report(dir: &Path) -> Result<(Report, u64), String> {
    let (m, events) = read_plan(dir).map_err(|e| e.to_string())?;
    let window = m.corpus_summary.wss_window_requests;
    if window == 0 {
        return Err(format!(
            "{}: the manifest carries no wss_window; refusing to invent one",
            dir.display()
        ));
    }
    let mut s = Statistics::new(window);
    s.push_events(&events);
    Ok((s.finish(), window))
}

/// The report for a trace, measured over the **plan's** window.
///
/// Using the plan's window rather than a default of its own is what makes the
/// windowed statistics — realised sharing, trunk occupancy, working-set size —
/// comparable at all. Two reports over different windows would differ for that reason
/// alone, and the divergence would be attributed to the workload.
fn trace_report(
    path: &Path,
    window: u64,
    allow_partial: bool,
    block_size: Option<u32>,
) -> Result<(Report, String), String> {
    let trace = read::read_trace(path, allow_partial, block_size).map_err(|e| e.to_string())?;
    let mut s = Statistics::new(window);
    for r in trace.refs() {
        s.push(&r);
    }
    let note = format!(
        "{} invocations, {} references, {} sessions, block size {} tokens; {}{}",
        trace.invocations.len(),
        trace.references(),
        trace.sessions(),
        trace.capabilities.block_size,
        if trace.chronological {
            "chronological order"
        } else {
            "FILE ORDER, so every order-dependent statistic below is order-dependent \
             rather than measured (FR-055d)"
        },
        if trace.capabilities.trunk_fittable() {
            ""
        } else {
            "; session identity is unavailable, so sharing cannot be observed at all"
        }
    );
    Ok((s.finish(), note))
}

/// Print the bucket-by-bucket working behind a comparison (`--explain`).
///
/// One table per distributional statistic, each row a shared bucket bound with both
/// counts, both CDFs and their signed difference; the row that set the KS distance is
/// marked. The tables come from `divergence::explain`, which is the same arithmetic
/// the verdict used — a diagnostic that recomputed the CDFs a second way could point
/// somewhere the verdict never looked.
/// Account for a difference in *mean* request length, term by term.
///
/// A `request_length` divergence says the two distributions differ and cannot say which
/// of FR-014a's three terms did it. This can, by arithmetic: the generator builds a path
/// as `shared_depth + private_depth + Σ growth_per_turn`, and `private_depth` is defined
/// as `turn-1 depth − turn-1 shared prefix`. So the identity that must hold for a
/// generated turn-1 path to match the trace's is
///
/// ```text
/// E[shared_depth drawn]  ==  E[turn-1 shared prefix subtracted]
/// ```
///
/// and any gap between those two is added to **every** generated path before a single
/// increment of growth. It is worth printing precisely because it is invisible in every
/// distributional check: both sides can be individually well-fitted measurements of
/// *different populations*.
/// How much of the sharing divergence is a difference of **space** rather than of fit.
///
/// `shared_depth` is drawn once per session; the FR-056 sharing statistic is measured over
/// every request. The generator turns the first into the second by replaying the drawn
/// prefix on every turn, so it produces the *turn-weighted* image of its per-session draw
/// — and that can only equal the trace's all-requests histogram if the trace's sharing is
/// constant within a session.
///
/// Two KS distances say what is available. The first is what the fit leaves today. The
/// second is the floor that conditioning `shared_depth` on session length would leave,
/// which is the part of the gap that is within-session deepening and therefore not
/// expressible by any single per-session draw. Printed as KS rather than as means because
/// that is what FR-056 gates on, and the mean-space split in the path budget above
/// disagrees with it about which half is worth closing.
fn print_sharing_spaces(trace: &Report, shapes: &mut workload_model::fit::sessions::SessionShapes) {
    use workload_model::stats::divergence::ks_from_buckets;
    let all = &trace.sharing.depth_buckets;
    let all_total: u64 = all.iter().map(|(_, _, c)| *c).sum();
    let (per_session, n1) = shapes.shared_depth_buckets();
    let (weighted, n2) = shapes.shared_depth_buckets_turn_weighted();
    if all_total == 0 || n1 == 0 {
        return;
    }
    let now = ks_from_buckets(all, all_total, &per_session, n1);
    let floor = ks_from_buckets(all, all_total, &weighted, n2);
    println!("\n    term 1b: sharing is validated over REQUESTS, drawn per SESSION");
    println!(
        "    {:<46} {:>10}",
        "  KS all-requests vs the per-session fit",
        format!("{now:.5}")
    );
    println!(
        "    {:<46} {:>10}   conditioning `shared_depth` on `turns`",
        "  KS all-requests vs that fit turn-weighted",
        format!("{floor:.5}")
    );
    println!(
        "    {:<46} {:>36}",
        "", "closes the difference between these two;"
    );
    println!(
        "    {:<46} {:>36}",
        "", "the lower figure is within-session deepening,"
    );
    println!(
        "    {:<46} {:>36}",
        "", "which one per-session draw cannot express"
    );
}

fn print_path_budget(
    trace: &Report,
    synthetic: &Report,
    budget: &workload_model::fit::sessions::PathBudget,
) {
    let per_request = |r: &Report| {
        if r.requests == 0 {
            0.0
        } else {
            r.references as f64 / r.requests as f64
        }
    };
    let trace_mean = per_request(trace);
    // Turn-weighted: what a *request* carries of turn-1 depth, not the mean over
    // sessions. The two differ whenever session length varies, and it is the weighted
    // one that has to add up to the mean request length.
    let weighted_turn_one = if budget.requests == 0 {
        0.0
    } else {
        budget.weighted_turn_one_depth as f64 / budget.requests as f64
    };
    let syn_mean = per_request(synthetic);
    // The population `shared_depth` is fitted over: every sharing request, at every
    // turn. The generator draws from it once per session.
    let all_shared = trace
        .sharing
        .depth_buckets
        .iter()
        .fold((0.0, 0u64), |acc, (lo, hi, c)| {
            (
                acc.0 + (*lo as f64 + *hi as f64) / 2.0 * *c as f64,
                acc.1 + c,
            )
        });
    let all_shared_mean = if all_shared.1 == 0 {
        0.0
    } else {
        all_shared.0 / all_shared.1 as f64
    };

    println!("\n  path budget — where a mean request length comes from");
    println!(
        "  FR-014a: path = shared_depth + private_depth + SUM(growth_per_turn). Every figure below\n          is PER REQUEST and turn-weighted, because a session with many turns contributes many\n          requests — the plain mean over sessions is not what a request carries."
    );
    println!(
        "    {:<46} {:>10}",
        "trace mean blocks/request",
        format!("{trace_mean:.1}")
    );
    println!(
        "    {:<46} {:>10}",
        "synthetic mean blocks/request",
        format!("{syn_mean:.1}")
    );
    println!(
        "    {:<46} {:>10}",
        "  excess to account for",
        format!("{:+.1}", syn_mean - trace_mean)
    );

    println!("\n    the trace's own decomposition");
    println!(
        "    {:<46} {:>10}",
        "  turn-1 depth (turn-weighted)",
        format!("{:.1}", weighted_turn_one)
    );
    println!(
        "    {:<46} {:>10}",
        "  accumulated growth",
        format!("{:.1}", budget.accumulated_per_request())
    );

    println!("\n    term 1: the shared prefix, drawn against subtracted");
    println!(
        "    {:<46} {:>10}",
        "  SUBTRACTED to get private_depth (turn 1 only)",
        format!("{:.1}", budget.turn_one_shared)
    );
    println!(
        "    {:<46} {:>10}",
        "  DRAWN by the generator (per session, turn 1)",
        format!("{:.1}", budget.turn_one_shared)
    );
    println!(
        "    {:<46} {:>10}   added to every path",
        "  mismatch",
        format!("{:+.1}", 0.0)
    );
    // The FR-056 statistic lives in a different space from the parameter, and the two
    // rows below are what say whether the model can hold both at once. `shared_depth` is
    // drawn once per session, so the generator's all-requests histogram is the
    // turn-weighted image of that draw -- which can only match the trace's if the
    // trace's sharing is constant within a session.
    println!(
        "    {:<46} {:>10}   the FR-056 statistic's space",
        "  for reference, sharing over ALL turns",
        format!("{all_shared_mean:.1}")
    );
    // Split into its two causes rather than judged against a threshold. Both are
    // typically non-zero, and a binary verdict on a continuum reads as "this one does not
    // apply" when it means "this one is smaller" -- so the two magnitudes are printed and
    // the reader is told which fix each calls for.
    let weighted_turn_one_shared = budget.turn_one_shared_weighted();
    let correlation = weighted_turn_one_shared - budget.turn_one_shared;
    let within = all_shared_mean - weighted_turn_one_shared;
    let total = (correlation + within).abs().max(1e-9);
    println!(
        "    {:<46} {:>10}",
        "    turn-1 sharing, turn-WEIGHTED",
        format!("{weighted_turn_one_shared:.1}")
    );
    println!(
        "    {:<46} {:>10}   {:.0}% — sessions that share more deeply run",
        "    of the gap: length-to-sharing correlation",
        format!("{correlation:+.1}"),
        100.0 * correlation / total
    );
    println!(
        "    {:<46} {:>36}",
        "", "longer. EXPRESSIBLE: condition on `turns`"
    );
    println!(
        "    {:<46} {:>10}   {:.0}% — sharing deepens along the conversation.",
        "    of the gap: within-session growth",
        format!("{within:+.1}"),
        100.0 * within / total
    );
    println!(
        "    {:<46} {:>36}",
        "", "NOT EXPRESSIBLE: one per-session draw cannot"
    );

    println!("\n    term 2: accumulated growth, true against i.i.d. per turn");
    if let (Some(g), Some(iid), Some(inflation)) = (
        budget.growth,
        budget.accumulated_per_request_iid(),
        budget.iid_inflation(),
    ) {
        let weighted_g = if budget.accumulated_steps == 0 {
            0.0
        } else {
            budget.accumulated_growth as f64 / budget.accumulated_steps as f64
        };
        println!(
            "    {:<46} {:>10}",
            "  pooled mean increment (what is drawn from)",
            format!("{g:.2}")
        );
        println!(
            "    {:<46} {:>10}",
            "  mean increment as the SUM weights it",
            format!("{weighted_g:.2}")
        );
        println!(
            "    {:<46} {:>10}",
            "  accumulated if drawn i.i.d. at the pooled mean",
            format!("{iid:.1}")
        );
        println!(
            "    {:<46} {:>10}   <- overstates by this factor",
            "  against the trace's actual accumulation",
            format!("{inflation:.3}x")
        );
        println!(
            "    {:<46} {:>10}",
            "  excess from this term alone",
            format!("{:+.1}", iid - budget.accumulated_per_request())
        );
        println!(
            "  An increment at position i of a T-turn session is inherited by every later turn, so\n              it enters the sum with weight (T - i) — quadratic in T once summed over a session. An\n              i.i.d. per-turn draw is right only if the mean increment under THAT weighting equals\n              the pooled mean. Where the longest sessions grow at a different rate from the\n              population, these two diverge while every marginal distribution stays correct."
        );
    }
}

/// The observed width-by-depth profile beside what was fitted from it.
///
/// The instrument for a schema rejection about the trunk, which is otherwise a number
/// with no working shown. Rule 16 evaluates occupancy at `p99(shared_depth)`, and that
/// depth is routinely far beyond `fitted_to_depth` — so the printed rows say whether
/// the model's implied width there resembles the trace's observed width, or whether a
/// fanout fitted over a shallow region has been carried somewhere it was never
/// measured. A product compounds, so those two can differ by orders of magnitude
/// while every individual segment looks reasonable.
///
/// `shared` is the count of keys at a depth that **two or more sessions** reached,
/// which is the trunk as the fit defines it; `width` counts every distinct key there,
/// trunk plus every private descent. Where the two diverge, the trunk has ended and
/// what remains is private.
/// The segment census, banded by depth — the fitting input for the cohort mechanism.
///
/// Printed under `--explain` because it is what says whether the trunk the fit describes is
/// the trunk the trace has. The per-depth width table above cannot: it is the trie's
/// marginals, and "one preamble shared by 5922 sessions" and "5922 sessions over 603
/// unrelated roots" produce identical rows in it.
///
/// The bands are the same geometric ones `research/segment_census.py` uses, and the columns
/// are the same quantities, deliberately: the two implementations are meant to be diffed on
/// a real trace. Out-degree is the **total**, singletons included, because that is where
/// privacy comes from and where a shared-width profile is blind.
fn print_segment_census(
    invocations: &[read::NormalisedInvocation],
    skews: Option<&[workload_model::fit::segments::BandSkew]>,
) {
    use workload_model::fit::segments::{Census, SegmentEnd};

    // Grouped by session, which `Census::observe` requires for an exact fan-in.
    let mut order: Vec<usize> = (0..invocations.len()).collect();
    order.sort_by_key(|i| (invocations[*i].session.0, invocations[*i].turn));
    let mut census = Census::new();
    for i in order {
        census.observe(invocations[i].session, &invocations[i].blocks);
    }
    let rows = census.finish(2);
    if rows.is_empty() {
        println!("\n  segment census — no shared segment to report");
        return;
    }
    // Derived from the fit's own band list rather than restated, so this table and the
    // fitted document cannot come to disagree about which depths a row covers.
    let bands = workload_model::fit::segments::BANDS;
    let spans: Vec<(u32, u32)> = bands
        .iter()
        .enumerate()
        .map(|(i, lo)| {
            (
                *lo,
                bands
                    .get(i + 1)
                    .map_or(u32::MAX, |next| next.saturating_sub(1)),
            )
        })
        .collect();
    println!("\n  segment census — the trunk as runs of blocks one cohort walks together");
    println!(
        "  a segment is a maximal chain of constant fan-in; out-degree is the TOTAL at the split\n  \
         that ends it, singletons included, because that is what makes a session go private."
    );
    println!(
        "    {:>10}  {:>6}  {:>8}  {:>8}  {:>8}  {:>8}  {:>7}  {:>7}  {:>8}",
        "depths",
        "segs",
        "len_med",
        "len_max",
        "fanin_med",
        "fanin_max",
        "deg_med",
        "shared",
        "leak_wt"
    );
    for (lo, hi) in spans.iter().copied() {
        let mut v: Vec<&_> = rows
            .iter()
            .filter(|r| r.start_depth >= lo && r.start_depth <= hi)
            .collect();
        if v.is_empty() {
            continue;
        }
        v.sort_by_key(|r| r.length);
        let len_med = v[v.len() / 2].length;
        let len_max = v.iter().map(|r| r.length).max().unwrap_or(0);
        let mut f: Vec<u32> = v.iter().map(|r| r.fan_in).collect();
        f.sort_unstable();
        let fan_med = f[f.len() / 2];
        let fan_max = *f.last().unwrap_or(&0);
        let splits: Vec<&&_> = v.iter().filter(|r| r.ends == SegmentEnd::Fanout).collect();
        let mut deg: Vec<u32> = splits.iter().map(|r| r.out_degree).collect();
        deg.sort_unstable();
        let deg_med = if deg.is_empty() {
            0
        } else {
            deg[deg.len() / 2]
        };
        let shared_med = {
            let mut s: Vec<u32> = splits.iter().map(|r| r.shared_children).collect();
            s.sort_unstable();
            if s.is_empty() {
                0
            } else {
                s[s.len() / 2]
            }
        };
        // Session-weighted, because a split holding 5000 sessions matters more than one
        // holding 2 — and because the median is exactly 0.000 on every trace measured, which
        // is the finding that established sharing ends by subdivision rather than retirement.
        let lw: f64 = splits.iter().map(|r| r.leak() * f64::from(r.fan_in)).sum();
        let ld: f64 = splits.iter().map(|r| f64::from(r.fan_in)).sum();
        let span = if hi == u32::MAX {
            format!("{lo}+")
        } else if lo == hi {
            format!("{lo}")
        } else {
            format!("{lo}-{hi}")
        };
        println!(
            "    {:>10}  {:>6}  {:>8}  {:>8}  {:>8}  {:>8}  {:>7}  {:>7}  {:>8.3}",
            span,
            v.len(),
            len_med,
            len_max,
            fan_med,
            fan_max,
            deg_med,
            shared_med,
            if ld > 0.0 { lw / ld } else { 0.0 }
        );
    }
    if census.violations() > 0 {
        println!(
            "    {} keys contradict rolling-prefix identity, so these rows describe something \
             that is not a trie",
            census.violations()
        );
    }
    print_child_law(&rows, &spans, skews);
}

/// The child-choice law per band: what the trace does, and what the fit stated.
///
/// `coll` is the **collision probability** at a split — the chance two sessions arriving there
/// descend into the same child, and so the factor by which a session's cohort shrinks in
/// expectation as it takes the step. It is printed because it is exactly what the generator
/// multiplies (`cohort *= p(child taken)`, the child drawn from `p`), which makes it the one
/// functional of the child law the cohort mechanism can observe, and the thing `skew` is
/// fitted to. `1/coll` is the effective branching that occupancy and rule 16 depend on.
///
/// The p10/p90 columns are the question this banding leaves open: one exponent per depth band
/// matches the band's weighted mean and cannot match every split in it. A wide spread here
/// says the law is better conditioned on out-degree than on depth — which is measurable from
/// this table rather than assumable.
fn print_child_law(
    rows: &[workload_model::fit::segments::SegmentRow],
    spans: &[(u32, u32)],
    skews: Option<&[workload_model::fit::segments::BandSkew]>,
) {
    let any = rows.iter().any(|r| r.collision().is_some());
    if !any {
        return;
    }
    println!(
        "\n  child law — how a cohort divides at a split. coll = SUM p^2 over children, \
         fan-in weighted;\n  1/coll is the effective branching. skew is the fitted Zipf \
         exponent reproducing coll_wt.\n  ess is Kish's effective sample size of those weights, \
         (SUM w)^2/SUM w^2 — how many splits coll_wt\n  effectively averages. Far below splits \
         means one wide segment is setting the band's law."
    );
    println!(
        "    {:>10}  {:>7}  {:>6}  {:>8}  {:>8}  {:>8}  {:>8}  {:>6}  {:>8}",
        "depths",
        "splits",
        "ess",
        "coll_wt",
        "coll_p10",
        "coll_p50",
        "coll_p90",
        "skew",
        "achieved"
    );
    for (lo, hi) in spans.iter().copied() {
        let v: Vec<&workload_model::fit::segments::SegmentRow> = rows
            .iter()
            .filter(|r| r.start_depth >= lo && r.start_depth <= hi)
            .filter(|r| r.collision().is_some())
            .collect();
        if v.is_empty() {
            continue;
        }
        // Fan-in weighted, for the same reason the fit is: a walker meets a split in
        // proportion to the sessions arriving at it, and the shared region is numerically
        // dominated by tiny cohorts while the reference mass sits in a few large segments.
        let num: f64 = v
            .iter()
            .map(|r| r.collision().unwrap_or(0.0) * f64::from(r.fan_in).max(1.0))
            .sum();
        let den: f64 = v.iter().map(|r| f64::from(r.fan_in).max(1.0)).sum();
        let mut c: Vec<f64> = v.iter().filter_map(|r| r.collision()).collect();
        c.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
        let q = |f: f64| c[((c.len() - 1) as f64 * f).round() as usize];
        let span = if hi == u32::MAX {
            format!("{lo}+")
        } else if lo == hi {
            format!("{lo}")
        } else {
            format!("{lo}-{hi}")
        };
        let fitted = skews.and_then(|s| s.iter().find(|b| b.from_depth == lo));
        // Kish's effective sample size of the fan-in weights, recomputed here from the same rows
        // the table above is built from rather than read off the fit, so that the number shown
        // beside the measurement belongs to the measurement even when no law was fitted.
        let wsum: f64 = v.iter().map(|r| f64::from(r.fan_in).max(1.0)).sum();
        let wsq: f64 = v.iter().map(|r| f64::from(r.fan_in).max(1.0).powi(2)).sum();
        println!(
            "    {:>10}  {:>7}  {:>6}  {:>8.4}  {:>8.4}  {:>8.4}  {:>8.4}  {:>6}  {:>8}",
            span,
            v.len(),
            if wsq > 0.0 {
                format!("{:.1}", wsum * wsum / wsq)
            } else {
                "-".to_string()
            },
            if den > 0.0 { num / den } else { 0.0 },
            q(0.10),
            q(0.50),
            q(0.90),
            fitted.map_or("-".to_string(), |b| format!("{:.3}", b.skew)),
            fitted.map_or("-".to_string(), |b| format!("{:.4}", b.achieved)),
        );
    }
    for b in skews.unwrap_or(&[]) {
        if let Some(note) = b.clamped {
            println!("    depth {}+: {note}", b.from_depth);
        }
    }
    if skews.is_none() {
        println!(
            "    skew unfitted: pass --branching-segments. Without the node-level spelling the \
             document\n    states one law for every depth, and coll is what it would have to \
             reproduce."
        );
    }
}

/// The node-level trunk process as fitted: the parameters the document will state.
///
/// Printed because nothing else could read them. The census above is the *measurement* and the
/// child-law table is one of the three fitted halves; the run length and out-degree reached only
/// the emitted YAML, which FR-057 refuses to write whenever the fit does not resemble its source
/// — i.e. exactly when someone is trying to find out why. Every column here is a function of the
/// fitted document alone, so this table says what the model will do, not what the trace did.
///
/// Read it against the census's `len_med` / `deg_med` columns, band for band. Expect them to
/// differ: the fit weights both by **fan-in** while the census medians are per segment, and on
/// `tau2_airline` at depths 128-511 that moves the out-degree from a census median of 3 to a
/// fitted 19. That gap is the fan-in weighting doing its job, and it is visible only here.
///
/// The derived columns are the reason the table exists. `splits/blk` is the rate the run length
/// sets — off the **mean**, not the median, since the number of splits over a depth is a renewal
/// rate and these distributions are heavily skewed. `decay/band` is the expected cohort factor
/// across the band's **own span**, `coll ^ (span / mean len)`, and `cum` is the running product
/// down the trunk. Sharing ends where a cohort reaches one session, so `cum` against the session
/// count per root is what decides where — not either fitted half alone.
fn print_fitted_process(fit: &workload_model::fit::segments::ProcessFit) {
    let implied = fit.implied();
    println!(
        "\n  fitted trunk process — the node-level law the document states, per band\n  \
         a walker draws a run length from `len`, walks it, then splits `deg` ways and picks a \
         child\n  under `skew`. splits/blk is off the MEAN length; decay/band = \
         coll^(span/mean len);\n  cum is the product down the trunk. Compare len/deg against the \
         census above: the fit\n  weights both by fan-in, so they are meant to differ from its \
         per-segment medians."
    );
    println!(
        "    {:>10}  {:>7}  {:>7}  {:>7}  {:>8}  {:>7}  {:>8}  {:>6}  {:>10}  {:>9}  {:>9}",
        "depths",
        "len_p10",
        "len_p50",
        "len_p90",
        "len_mean",
        "deg_p50",
        "deg_mean",
        "skew",
        "splits/blk",
        "decay/band",
        "cum"
    );
    let bands = &fit.process.by_depth;
    for (i, b) in bands.iter().enumerate() {
        let span = match bands.get(i + 1) {
            Some(next) => format!("{}-{}", b.from_depth, next.from_depth.saturating_sub(1)),
            None => format!("{}+", b.from_depth),
        };
        // `-` rather than a substituted value wherever the distribution declines to answer: a
        // `zipf` has no closed-form mean, and printing a stand-in here would be a fitted-looking
        // number nothing states.
        let q = |d: &workload_model::dist::Dist, p: f64| {
            d.quantile(p).map_or("-".to_string(), |v| format!("{v:.1}"))
        };
        let m = |d: &workload_model::dist::Dist| {
            d.mean().map_or("-".to_string(), |v| format!("{v:.1}"))
        };
        let this = implied.iter().find(|x| x.from_depth == b.from_depth);
        // A cohort of a few thousand sessions is long gone by 1e-6, so below that the figure has
        // stopped describing anything realisable and an exponent would only imply precision.
        let factor = |v: Option<f64>| match v {
            None => "-".to_string(),
            Some(d) if d < 1e-6 => "<1e-6".to_string(),
            Some(d) => format!("{d:.5}"),
        };
        println!(
            "    {:>10}  {:>7}  {:>7}  {:>7}  {:>8}  {:>7}  {:>8}  {:>6}  {:>10}  {:>9}  {:>9}",
            span,
            q(&b.length, 0.10),
            q(&b.length, 0.50),
            q(&b.length, 0.90),
            m(&b.length),
            q(&b.out_degree, 0.50),
            m(&b.out_degree),
            b.skew.map_or("-".to_string(), |s| format!("{s:.3}")),
            this.map_or("-".to_string(), |x| format!("{:.4}", x.splits_per_block)),
            factor(this.and_then(|x| x.decay_in_band)),
            factor(this.and_then(|x| x.cumulative)),
        );
    }
    if bands.len() != implied.len() {
        println!(
            "    {} of {} bands state no child law, so no decay is derived for them; those \
             defer\n    to the document-level `branch_skew`, which is not this fit's statement, \
             and every\n    `cum` below such a band is withheld rather than multiplied across the \
             gap.",
            bands.len() - implied.len(),
            bands.len()
        );
    }
    println!(
        "    the last band is unbounded — the profile applies it to every depth below its \
         start —\n    so it has no span to integrate over and its decay/band and cum read `-`."
    );
}

fn print_trunk_profile(trace: &Report, fitted: &workload_model::fit::branching::FittedBranching) {
    let depths = &trace.trunk.depths;
    if depths.is_empty() {
        return;
    }
    let p99 = trace.sharing.realised_depth.p99.unwrap_or(0) as usize;

    // The depths worth showing: the fitted region's ends, each segment boundary, the
    // depth rule 16 judges, and the deepest observed. Printing every depth would be
    // thousands of rows for an agentic trace and would bury exactly this comparison.
    let mut want: Vec<usize> = vec![0];
    want.extend(fitted.segments.iter().map(|s| s.from_depth as usize));
    want.push(fitted.fitted_to_depth as usize);
    want.push(p99);
    want.push(fitted.observed_to_depth as usize);
    want.retain(|d| *d < depths.len());
    want.sort_unstable();
    want.dedup();

    println!("\n  trunk profile — observed against fitted");
    println!(
        "  the model's width at depth d is roots x PROD(fanout) over the segments up to d, and \
         it is\n  extrapolated past depth {} because nothing was fitted beyond there.",
        fitted.fitted_to_depth
    );
    println!(
        "    {:>7}  {:>10}  {:>10}  {:>12}  {:>9}  note",
        "depth", "width", "shared", "model width", "occupancy"
    );
    for d in want {
        let row = &depths[d];
        // The model's width at this depth, through the generator's OWN arithmetic —
        // the quantity rule 16 divides into the session population. `Profile::paths`
        // rather than a second implementation of it: `paths` multiplies
        // `fanout_at(1..=d)`, so `fanout_at(0)` is never read and the first segment
        // describes the step *into* depth 1. The hand-rolled version this replaces
        // counted a level at depth 0 and overstated every row by the first segment's
        // fanout — on a fixture whose true width at depth 0 is 4 it printed 12.
        let model = Profile::from_segments(&fitted.segments).paths(d as u32, fitted.roots as u32);
        let mut note = Vec::new();
        if d == fitted.fitted_to_depth as usize {
            note.push("last fitted depth");
        }
        if d == p99 {
            note.push("p99(shared_depth): the depth rule 16 judges");
        }
        if d == fitted.observed_to_depth as usize {
            note.push("deepest observed");
        }
        println!(
            "    {:>7}  {:>10}  {:>10}  {:>12.1}  {:>9}  {}",
            d,
            row.width_run,
            row.shared_keys_run,
            model,
            row.occupancy
                .map(|o| format!("{o:.2}"))
                .unwrap_or_else(|| "-".into()),
            note.join(", ")
        );
    }
}

fn print_explanation(
    a: &Report,
    b: &Report,
    d: &workload_model::stats::divergence::Report,
    a_label: &str,
    b_label: &str,
) {
    use workload_model::stats::divergence::{explain, worst_log_ratio_point};

    println!("\n  explanation — `{a_label}` is A, `{b_label}` is B");
    println!(
        "  delta is F_A - F_B, so a positive run means A has already accumulated mass \
         that B has not:\n  A is the shorter-tailed side there."
    );
    for e in explain(a, b) {
        let Some(x) = d.divergences.iter().find(|x| x.statistic == e.statistic) else {
            continue;
        };
        println!(
            "\n  {} ({}, {} against tolerance {}{})",
            e.statistic.name(),
            match x.measure {
                Measure::KolmogorovSmirnov => "ks",
                Measure::AreaBetweenCdfs => "area",
                Measure::MaxLogRatio => "log-ratio",
            },
            format_args!("{:.5}", x.value),
            format_args!("{:.5}", x.tolerance),
            if x.incomparable.is_some() {
                ", INCOMPARABLE — units differ, so these rows compare units too"
            } else {
                ""
            }
        );
        if e.rows.is_empty() {
            println!("    no shared buckets: one side has no samples");
            continue;
        }
        let worst = e
            .rows
            .iter()
            .map(|r| r.delta().abs())
            .fold(0.0f64, f64::max);
        println!(
            "    {:>12} {:>12} {:>12} {:>9} {:>9} {:>9}",
            "upper", "A count", "B count", "F_A", "F_B", "delta"
        );
        for r in &e.rows {
            println!(
                "    {:>12} {:>12} {:>12} {:>9.5} {:>9.5} {:>+9.5}{}",
                r.upper,
                r.a_count,
                r.b_count,
                r.a_cdf,
                r.b_cdf,
                r.delta(),
                if r.delta().abs() >= worst - 1e-12 {
                    "  <- sup"
                } else {
                    ""
                }
            );
        }
        println!(
            "    totals: A {} samples, B {} samples",
            e.a_total, e.b_total
        );
    }

    // Unique-keys is a curve of counts rather than a distribution, so it has no
    // buckets to tabulate; the equivalent question is which point set its verdict.
    match worst_log_ratio_point(&a.unique_keys, &b.unique_keys) {
        Some(p) => println!(
            "\n  unique_keys (log-ratio {:.5}): worst at request {} — A {} distinct keys, \
             B {:.1}, so A is {} by a factor of {:.3}",
            p.log_ratio,
            p.requests,
            p.a_distinct,
            p.b_distinct,
            if p.signed_log_ratio > 0.0 {
                "ahead"
            } else {
                "behind"
            },
            p.signed_log_ratio.abs().exp()
        ),
        None => println!(
            "\n  unique_keys: no point survived the ramp and counting-floor \
             restrictions, so the curves were not compared at all"
        ),
    }
}

#[allow(clippy::too_many_arguments)]
fn cmd_validate(
    plan: &Path,
    against_plan: Option<&Path>,
    trace: Option<&Path>,
    block_size: Option<u32>,
    allow_partial: bool,
    tolerance_args: &[String],
    json: bool,
    explain: bool,
) -> Result<bool, String> {
    let tol = parse_tolerances(tolerance_args)?;
    let (a, window) = plan_report(plan)?;

    let (b, what, note, against_trace) = match (against_plan, trace) {
        (Some(other), None) => {
            let (b, _) = plan_report(other)?;
            (b, format!("plan {}", other.display()), String::new(), false)
        }
        (None, Some(t)) => {
            let (b, note) = trace_report(t, window, allow_partial, block_size)?;
            (b, format!("trace {}", t.display()), note, true)
        }
        _ => return Err("give exactly one of --against-plan or --trace to compare against".into()),
    };

    let mut d = compare(&a, &b, &tol);
    if against_trace {
        // A plan's entry sizes are KV bytes; a trace's are tokens per block, and no
        // trace carries the `model_config` that would convert between them. So the
        // byte-weighted divergence measures the unit rather than the workload — it is
        // excluded from the verdict rather than counted as a failure, which it is not.
        d.mark_incomparable(
            Statistic::ReuseDistanceBytes,
            "a plan's sizes are KV bytes and a trace's are tokens per block; no trace \
             carries the model_config that would convert between them, so this compares \
             units rather than workloads",
        );
    }

    if json {
        let out = serde_json::json!({
            "left": plan.display().to_string(),
            "right": what,
            "window_requests": window,
            "right_note": note,
            "within_tolerance": d.within_tolerance(),
            "divergences": d.divergences,
        });
        println!(
            "{}",
            serde_json::to_string_pretty(&out).map_err(|e| e.to_string())?
        );
        return Ok(d.within_tolerance());
    }

    println!("validate");
    println!("  left    plan {}", plan.display());
    println!("  right   {what}");
    if !note.is_empty() {
        println!("  note    {note}");
    }
    println!(
        "  window  {window} requests (the plan's, applied to both — two windows would \
         make the windowed statistics differ for that reason alone)"
    );
    if a.requests < DEFAULT_TOLERANCE_MIN_REQUESTS || b.requests < DEFAULT_TOLERANCE_MIN_REQUESTS {
        // The defaults were derived at a stated size and every floor falls as
        // 1/sqrt(n), so applying them to a smaller comparison compares against a
        // floor the sample cannot reach.
        println!(
            "  WARNING the default tolerances were derived at {DEFAULT_TOLERANCE_MIN_REQUESTS} \
             requests; this comparison has {} and {}, so a pass is weaker than it looks",
            a.requests, b.requests
        );
    }
    println!();
    println!(
        "  {:<24} {:>10} {:>10} {:>10} {:>9}  verdict",
        "statistic", "divergence", "tolerance", "sup", "samples"
    );
    for x in &d.divergences {
        let measure = match x.measure {
            Measure::KolmogorovSmirnov => "ks",
            Measure::AreaBetweenCdfs => "area",
            Measure::MaxLogRatio => "log-ratio",
        };
        println!(
            "  {:<24} {:>10.5} {:>10.5} {:>10} {:>9}  {} ({measure})",
            x.statistic.name(),
            x.value,
            x.tolerance,
            x.sup
                .map(|v| format!("{v:.5}"))
                .unwrap_or_else(|| "--".into()),
            x.samples,
            match &x.incomparable {
                Some(_) => "incomparable",
                None if x.within => "within",
                None => "EXCEEDED",
            },
        );
        if let Some(why) = &x.incomparable {
            println!("  {:<24} not compared: {why}", "");
        }
    }
    if explain {
        print_explanation(&a, &b, &d, "plan", &what);
    }
    println!();
    if d.within_tolerance() {
        println!("  every statistic is within tolerance");
    } else {
        for x in d.failures() {
            println!(
                "  {} diverges by {:.5} against a tolerance of {:.5}",
                x.statistic.name(),
                x.value,
                x.tolerance
            );
        }
    }
    Ok(d.within_tolerance())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tolerance_overrides_land_on_the_named_statistic() {
        let t = parse_tolerances(&["sharing_depth=0.2".into(), "unique_keys=0.3".into()]).unwrap();
        assert_eq!(t.sharing_depth, 0.2);
        assert_eq!(t.unique_keys, 0.3);
        // Unmentioned statistics keep their derived defaults.
        assert_eq!(
            t.reuse_distance_objects,
            Tolerances::default().reuse_distance_objects
        );
    }

    #[test]
    fn an_unknown_statistic_is_refused_and_lists_the_real_ones() {
        let e = parse_tolerances(&["hit_rate=0.1".into()]).expect_err("must refuse");
        assert!(e.contains("unknown statistic"), "{e}");
        assert!(e.contains("reuse_distance_objects"), "{e}");
    }

    #[test]
    fn a_malformed_override_names_what_was_expected() {
        assert!(parse_tolerances(&["sharing_depth".into()])
            .unwrap_err()
            .contains("NAME=VALUE"));
        assert!(parse_tolerances(&["sharing_depth=x".into()])
            .unwrap_err()
            .contains("is not a number"));
    }
}
