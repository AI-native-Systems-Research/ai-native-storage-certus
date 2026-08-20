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
        /// Skip the achievable-floor measurement and use the global default tolerances.
        ///
        /// The floor costs a pass over the trace for each half — about twice the
        /// statistics phase, which on the corpus's largest traces is minutes rather
        /// than seconds. This is the escape hatch for a quick fit.
        ///
        /// It makes the verdict **uncalibrated**, which is the thing FR-057c exists to
        /// fix: the default tolerances describe the generator's repeatability, not what
        /// a correct model of this workload would score, and measured they are 1.5x too
        /// tight on one real trace's `request_length` and 2-10x too loose on its reuse
        /// distance. So the report says so rather than printing a bare `within`.
        #[arg(long)]
        no_floor: bool,
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
    /// Measure the **achievable floor**: two samples of one real trace, compared.
    ///
    /// Not to be confused with `stats::floor`, which is the **compulsory-miss** floor of
    /// FR-044/FR-060 — the miss rate an unbounded cache would still report. This is the
    /// *divergence* floor: how far apart two samples of one workload are, and therefore the
    /// score below which no model can be distinguished from a correct one.
    ///
    /// The FR-056 tolerances were calibrated from the generator against *itself* across
    /// seeds, where any bias the model shares with itself cancels exactly. So they say
    /// how repeatable the generator is, and not what score a **perfect** model of a real
    /// workload could achieve. Without that second number a divergence cannot be read:
    /// 0.10 is a failure if two samples of the same trace score 0.01, and is at the noise
    /// floor if they score 0.09.
    ///
    /// Splits the trace two ways, because each split has a confound and they point in
    /// opposite directions — see `cmd_floor`. Also compares two whole traces with
    /// `--against`, which bounds the same question from above: how far apart are two
    /// *different* workloads from one family.
    Floor {
        /// The trace to split: a `.jsonl` file, or a directory holding a parquet trace.
        #[arg(short = 't', long, value_name = "PATH")]
        trace: PathBuf,
        /// Compare against a whole second trace instead of splitting the first.
        #[arg(long, value_name = "PATH")]
        against: Option<PathBuf>,
        /// Which blocking to read, in **tokens** per block. See `fit --block-size`.
        #[arg(long, value_name = "TOKENS")]
        block_size: Option<u32>,
        /// The measurement window, in requests (FR-009h).
        #[arg(long, default_value_t = 5_000, value_name = "N")]
        wss_window: u64,
        /// Accept a trace shorter than its manifest declares.
        #[arg(long)]
        allow_partial: bool,
        /// Per-statistic tolerance overrides, as `name=value`.
        #[arg(long = "tolerance", value_name = "NAME=VALUE")]
        tolerances: Vec<String>,
        /// Emit JSON instead of the human summary.
        #[arg(long)]
        json: bool,
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
            no_floor,
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
            no_floor,
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
        Cmd::Floor {
            trace,
            against,
            block_size,
            wss_window,
            allow_partial,
            tolerances,
            json,
        } => cmd_floor(
            &trace,
            against.as_deref(),
            block_size,
            wss_window,
            allow_partial,
            &tolerances,
            json,
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
    parse_tolerances_onto(Tolerances::default(), args)
}

/// As [`parse_tolerances`], but over a stated base rather than the seed-to-seed defaults.
///
/// Exists so that an explicit `--tolerance` still wins over a **floor-derived** base (FR-057c):
/// the derivation must not be able to silently override what a caller asked for, and a caller
/// must not have to know whether a derivation ran in order to override it.
fn parse_tolerances_onto(base: Tolerances, args: &[String]) -> Result<Tolerances, String> {
    let mut t = base;
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

/// One iteration's candidate model, and what it scored.
///
/// A struct rather than a tuple because the selection rule needs the whole per-statistic vector,
/// not just the worst ratio: a candidate is rejected if it improves the maximum by making another
/// gated statistic worse. See the Pareto rule in `cmd_fit`.
struct Candidate {
    /// Worst divergence as a multiple of its own tolerance.
    worst: f64,
    /// The document this iteration assembled.
    doc: workload_model::schema::Document,
    /// What generating from it measured.
    synthetic: Report,
    /// The attempted-sharing scale that produced it.
    scale: f64,
    /// Which iteration.
    iteration: usize,
    /// Every comparable statistic's divergence, for the Pareto comparison.
    values: Vec<(Statistic, f64)>,
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
    no_floor: bool,
) -> Result<bool, String> {
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

    // The trace's own ACHIEVABLE FLOOR, and the tolerances derived from it (FR-057c).
    //
    // Measured before anything is fitted, because it is a property of the trace rather than of
    // any model: two independent samples of this workload, compared with the same accumulators
    // the fit will be judged by. The seed-to-seed defaults describe the generator's
    // repeatability and cannot say what a correct model of THIS workload would score — measured,
    // they are 1.5x too tight on `tau2_airline`'s `request_length` and 2-10x too loose on reuse
    // distance, so one constant was wrong in both directions at once.
    //
    // The session split, not the time split: it preserves the run's duration and therefore its
    // stationarity, where a time split charges real drift to the floor and would excuse a model
    // for failing to reproduce structure the trace genuinely has.
    //
    // `--no-floor` skips it, at the cost of an uncalibrated verdict; the report says which.
    let floor = if no_floor {
        None
    } else {
        let (mut a, mut b) = (Statistics::new(wss_window), Statistics::new(wss_window));
        for r in trace.refs_of(|_, inv| floor_side(inv.session.0) == 0) {
            a.push(&r);
        }
        for r in trace.refs_of(|_, inv| floor_side(inv.session.0) == 1) {
            b.push(&r);
        }
        Some(compare(&a.finish(), &b.finish(), &Tolerances::default()))
    };
    // Explicit overrides are applied ON TOP of the derivation, so a caller who names a tolerance
    // still gets it and does not have to know whether a floor was measured.
    let tol = parse_tolerances_onto(
        floor
            .as_ref()
            .map_or_else(Tolerances::default, Tolerances::from_floor),
        tolerance_args,
    )?;

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
            // The path length rides along so the per-root turn-1 table (FR-054j) is measured over
            // `roots.popularity`'s own population, at its own ranks, in one pass.
            roots.observe(inv.session.0, inv.turn, *root, inv.blocks.len() as u64);
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
        let trace_rows = print_segment_census(
            &trace.invocations,
            fitted_segments.as_ref().map(|f| f.skews.as_slice()),
        );
        // Structure, not marginals: the four gated statistics cannot see the trunk's shape, and
        // a collapsed trunk passes all of them. Compared through the same census both sides.
        print_root_path_correlation(&trace.invocations, &trace_rows);
        if let Err(e) = print_structure_diff(&trace_rows, &doc) {
            println!("\n  trunk structure — unavailable: {e}");
        }
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
    let mut best: Option<Candidate> = None;
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

        // Every comparable statistic's value, so a candidate can be rejected for buying one
        // statistic with another. See the Pareto rule below.
        let values: Vec<(Statistic, f64)> = d
            .divergences
            .iter()
            .filter(|x| x.incomparable.is_none())
            .map(|x| (x.statistic, x.value))
            .collect();

        let realised = synthetic.sharing.realised_depth.p50.unwrap_or(0) as f64;
        history.push(format!(
            "iteration {iteration}: attempt scale {scale:.3}, realised sharing p50 \
             {realised:.0} against the trace's {target_sharing:.0}, worst divergence \
             {worst:.2}x its tolerance"
        ));

        // A candidate must improve the worst relative excess AND make **no gated statistic
        // worse** — a Pareto rule, not just a better maximum.
        //
        // Selecting on the maximum alone lets the loop buy one statistic with another, and
        // that is not a hypothetical: when the tolerances became floor-derived (FR-057c),
        // `request_length`'s tolerance on `tau2_airline` widened from 0.020 to 0.060, the
        // loop's objective changed with it, and it began choosing a 4x-scaled attempt that
        // improved the worst ratio while taking `request_length` from 0.026 to 0.411 and the
        // reference count from 7.8M to 20.0M. Two traces regressed the same way on the
        // default path. **The tolerances must not be able to change what the loop optimises
        // for**, and every statistic here is gated, so a trade between them is never a win.
        //
        // Strict rather than epsilon-tolerant: the comparison is deterministic at a fixed
        // seed, so a statistic that moves at all moved because the scale moved it. The loop's
        // own premise — that raising the attempt closes a median shortfall — was already
        // measured to be false, so the conservative rule also matches the evidence.
        let regressed = best.as_ref().and_then(|prev| {
            values.iter().find_map(|(st, v)| {
                prev.values
                    .iter()
                    .find(|(p, _)| p == st)
                    .filter(|(_, pv)| *v > *pv + 1e-12)
                    .map(|(_, pv)| (*st, *pv, *v))
            })
        });
        let improved = best.as_ref().map_or(true, |b| worst < b.worst) && regressed.is_none();
        if let Some((st, was, now)) = regressed {
            history.push(format!(
                "iteration {iteration} rejected: it would improve the worst ratio but move \
                 {} from {was:.5} to {now:.5}, and a trade between two gated statistics is \
                 not an improvement",
                st.name()
            ));
        }
        if improved {
            best = Some(Candidate {
                worst,
                doc: candidate,
                synthetic,
                scale,
                iteration,
                values,
            });
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

    let Candidate {
        worst,
        doc,
        synthetic,
        scale: best_scale,
        iteration: best_iteration,
        ..
    } = best.ok_or("no candidate model could be generated")?;

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
    // The floor is printed beside the divergence because the tolerance is now derived from it
    // (FR-057c) and a verdict is unreadable without it: 0.026 against a 0.02 default looks like a
    // failure and against a 0.030 floor is indistinguishable from a perfect model.
    println!(
        "\n  {:<24} {:>10} {:>10} {:>10} {:>9}  verdict",
        "statistic", "divergence", "floor", "tolerance", "samples"
    );
    for x in &d.divergences {
        let f = floor.as_ref().and_then(|fl| {
            fl.divergences
                .iter()
                .find(|y| y.statistic == x.statistic && y.incomparable.is_none())
        });
        println!(
            "  {:<24} {:>10.5} {:>10} {:>10.5} {:>9}  {}",
            x.statistic.name(),
            x.value,
            f.map_or("--".into(), |y| format!("{:.5}", y.value)),
            x.tolerance,
            x.samples,
            match &x.incomparable {
                Some(_) => "incomparable",
                None if x.within => "within",
                None => "EXCEEDED",
            }
        );
    }
    match floor.as_ref() {
        Some(_) => println!(
            "  floor = two samples of THIS trace compared with each other (session split, \
             half-size),\n  so a divergence at or below it is unreachable by any model. tolerance \
             is {}x the\n  floor projected to full size, never tighter than the seed-to-seed \
             default (FR-057c).",
            workload_model::stats::divergence::FLOOR_TOLERANCE_FACTOR
        ),
        // Stated as loudly as a caveat, because an uncalibrated `within` is the misreading this
        // whole measurement exists to prevent: the defaults describe the GENERATOR's
        // repeatability, and measured against real traces they are wrong in BOTH directions.
        None => println!(
            "  UNCALIBRATED (--no-floor): these are the seed-to-seed defaults, which describe the \
             generator's\n  own repeatability rather than what a correct model of THIS workload \
             would score. Measured on\n  real traces they run 1.5x too tight on request_length \
             and 2-10x too loose on reuse distance,\n  so neither a `within` nor an `EXCEEDED` \
             here is evidence about the model (FR-057c)."
        ),
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

/// How a trace is cut into two samples of itself.
///
/// Two modes and not one, because each carries a confound and the two point in **opposite**
/// directions, so the pair brackets what a single split cannot establish alone.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Split {
    /// By session, on a hash of the session id.
    ///
    /// Preserves the run's duration and therefore its stationarity, but **halves the
    /// concurrent population** — and sharing is a population property (realised sharing is
    /// the LCP against earlier requests of *other* sessions), so both halves genuinely share
    /// less than the whole. Biases `sharing_depth` and reuse distance downward.
    Session,
    /// By time, at the median request timestamp.
    ///
    /// Preserves the concurrent population, so sharing and reuse distance are measured at
    /// the density the whole trace has — but it is exposed to **nonstationarity**, which is
    /// measured to be strong on this corpus (key-lifetime span over its stationary null is
    /// 0.13 on `tau2_airline` and 0.098 on `tau2_retail`). Charges real drift to the floor.
    Time,
}

impl Split {
    fn name(self) -> &'static str {
        match self {
            Split::Session => "by session",
            Split::Time => "by time",
        }
    }

    fn confound(self) -> &'static str {
        match self {
            Split::Session => {
                "halves the concurrent population, so both halves really do \
                               share less than the whole — reads sharing and reuse LOW"
            }
            Split::Time => {
                "preserves density but charges real nonstationarity to the floor \
                            — reads every statistic that drifts HIGH"
            }
        }
    }
}

/// Mix a session id so that a split is not aligned with arrival order.
///
/// Session ids are dense indices assigned on ingest, i.e. in arrival order, so their parity
/// would split the trace into alternating arrivals — a defensible interleave, but one that
/// makes the two halves correlated with anything periodic in the arrival process. SplitMix64's
/// finalizer decorrelates it for three multiplications, and being a pure function of the id
/// keeps the split reproducible without storing it.
fn mix64(x: u64) -> u64 {
    let mut z = x.wrapping_add(0x9E37_79B9_7F4A_7C15);
    z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    z ^ (z >> 31)
}

/// Fan-in per block: its achievable floor, and its sibling bound where one is asked for.
///
/// Measured, not gated (FR-057c): a statistic earns a place in the gate by having a low floor and a
/// high sibling bound, and `request_length` is the standing proof that sounding relevant is not
/// evidence. Fan-in is first on the list of quantities a hinted KV cache actually reacts to and
/// nothing in the model fits it, so it is the first candidate — but it goes in only if it separates
/// workloads by more than it separates a workload from itself.
///
/// Fed in **session-sorted** order, which `Trace::refs_of` cannot supply: it preserves the trace's
/// own order, and `FanIn` needs each session's references contiguous to count distinct sessions
/// exactly. Fan-in wants only `(key, session)`, so iterating invocations directly here is a
/// projection of the reference stream rather than a second definition of it — and `FanIn` reports
/// any contiguity violation instead of trusting this.
fn print_fanin_floor(
    trace: &read::Trace,
    against: Option<&Path>,
    block_size: Option<u32>,
    allow_partial: bool,
) -> Result<(), String> {
    use workload_model::stats::fanin::{FanIn, FanInReport};

    // One side's fan-in, over the invocations a predicate keeps, in session order.
    let measure = |t: &read::Trace, keep: &dyn Fn(usize, &read::NormalisedInvocation) -> bool| {
        let mut order: Vec<usize> = (0..t.invocations.len())
            .filter(|i| keep(*i, &t.invocations[*i]))
            .collect();
        order.sort_by_key(|i| (t.invocations[*i].session.0, t.invocations[*i].turn));
        let mut f = FanIn::new();
        for i in order {
            let inv = &t.invocations[i];
            for k in &inv.blocks {
                f.observe(*k, inv.session);
            }
        }
        f.finish()
    };

    let row = |name: &str, r: &FanInReport| {
        println!(
            "    {:<26} {:>10} {:>9.3} {:>9.4} {:>10}",
            name,
            r.keys,
            r.mean().unwrap_or(f64::NAN),
            r.shared_fraction(),
            r.per_key.max().unwrap_or(0),
        );
    };
    // KS between two fan-in distributions over keys, using the same bucket-based comparison the
    // gated statistics use, so a number here is on the same scale as the ones above.
    let ks = |a: &FanInReport, b: &FanInReport| {
        workload_model::stats::divergence::ks_from_buckets(
            &a.per_key.buckets(),
            a.per_key.count(),
            &b.per_key.buckets(),
            b.per_key.count(),
        )
    };

    println!(
        "\n  fan-in per block — distinct sessions touching each key. MEASURED, NOT GATED:\n           a candidate joins the gate on a low floor and a high sibling bound, never on relevance."
    );
    println!(
        "    {:<26} {:>10} {:>9} {:>9} {:>10}",
        "sample", "keys", "mean", "shared", "max"
    );
    let a = measure(trace, &|_, inv| floor_side(inv.session.0) == 0);
    let b = measure(trace, &|_, inv| floor_side(inv.session.0) == 1);
    row("this trace, half A", &a);
    row("this trace, half B", &b);
    let floor = ks(&a, &b);
    if a.out_of_order_sessions + b.out_of_order_sessions > 0 {
        println!(
            "    WARNING {} non-contiguous session appearances, so these are upper bounds",
            a.out_of_order_sessions + b.out_of_order_sessions
        );
    }
    match against {
        None => println!("    floor (KS between halves, fan-in over keys) {floor:.5}"),
        Some(other) => {
            let sibling =
                read::read_trace(other, allow_partial, block_size).map_err(|e| e.to_string())?;
            let s = measure(&sibling, &|_, _| true);
            let whole = measure(trace, &|_, _| true);
            row("sibling, whole", &s);
            let bound = ks(&whole, &s);
            println!(
                "    floor {floor:.5}   sibling {bound:.5}   range {:.2}x",
                {
                    if floor > 0.0 {
                        bound / floor
                    } else {
                        f64::INFINITY
                    }
                }
            );
        }
    }
    Ok(())
}

/// Which side of a **session split** a session falls on: `0` or `1`.
///
/// The one definition both `floor` and `fit` use. Two copies could drift, and then the floor a fit
/// derives its tolerances from would not be the floor `floor` reports for the same trace — a
/// disagreement that would look like a bug in whichever was read second.
fn floor_side(session: u32) -> u64 {
    mix64(u64::from(session)) & 1
}

/// Measure the achievable floor: what a *perfect* model would score against a real trace.
///
/// # Why this is the first thing to measure
///
/// The FR-056 tolerances came from the generator against itself across seeds. That measures
/// the generator's repeatability, and any bias the model shares with itself cancels exactly —
/// so the numbers say nothing about what score a correct model of a real workload could get.
/// Six sessions of fitting have been judged against them anyway. If two samples of one trace
/// score 0.09 on a statistic whose tolerance is 0.02, then **no model can pass it** and every
/// hour spent chasing that residual was spent against an unreachable target.
///
/// # What is compared
///
/// Two samples of one trace, under both [`Split`] modes, or two whole traces with `--against`
/// (which bounds the question from above: how far apart are two different workloads of one
/// family — a divergence near that bound means a statistic cannot tell workloads apart).
///
/// Every statistic comes from the same accumulators `fit` uses, over the same
/// [`read::Trace::refs_of`], so a half of a trace is measured by the rules a whole one is.
///
/// # Reading the numbers
///
/// A half-vs-half comparison is **half-size on both sides**, while `fit` compares full against
/// full. A two-sample KS distance scales as `sqrt(2/n)`, so halving both sides inflates it by
/// about `sqrt(2)`; the projection to full size is printed for the two KS statistics **only**.
/// Reuse distance is gated on the *area* between CDFs and unique-keys on a *log-ratio* of
/// counts, neither of which scales that way, so applying one correction to all four would
/// manufacture three wrong numbers to keep a column tidy.
fn cmd_floor(
    path: &Path,
    against: Option<&Path>,
    block_size: Option<u32>,
    window: u64,
    allow_partial: bool,
    tolerance_args: &[String],
    json: bool,
) -> Result<bool, String> {
    let tol = parse_tolerances(tolerance_args)?;
    let trace = read::read_trace(path, allow_partial, block_size).map_err(|e| e.to_string())?;

    let mut arms: Vec<(String, String, Report, Report)> = Vec::new();
    if let Some(other) = against {
        let sibling =
            read::read_trace(other, allow_partial, block_size).map_err(|e| e.to_string())?;
        let (mut a, mut b) = (Statistics::new(window), Statistics::new(window));
        for r in trace.refs() {
            a.push(&r);
        }
        for r in sibling.refs() {
            b.push(&r);
        }
        arms.push((
            format!("whole vs whole ({})", other.display()),
            "two DIFFERENT workloads, so this is an upper bound rather than a floor: a \
             statistic scoring near it cannot tell these workloads apart"
                .into(),
            a.finish(),
            b.finish(),
        ));
        // The primary trace's own floor as well, so the DYNAMIC RANGE can be printed rather
        // than left as arithmetic for the reader. It is the number that decides whether a
        // statistic is worth gating on at all, and measured on `tau2_airline` against
        // `tau2_retail` three of the four gated statistics turn out to have almost none.
        let (mut c, mut d) = (Statistics::new(window), Statistics::new(window));
        for r in trace.refs_of(|_, inv| floor_side(inv.session.0) == 0) {
            c.push(&r);
        }
        for r in trace.refs_of(|_, inv| floor_side(inv.session.0) == 1) {
            d.push(&r);
        }
        arms.push((
            "by session (for the range below)".into(),
            Split::Session.confound().to_string(),
            c.finish(),
            d.finish(),
        ));
    } else {
        for split in [Split::Session, Split::Time] {
            let (mut a, mut b) = (Statistics::new(window), Statistics::new(window));
            match split {
                Split::Session => {
                    for r in trace.refs_of(|_, inv| floor_side(inv.session.0) == 0) {
                        a.push(&r);
                    }
                    for r in trace.refs_of(|_, inv| floor_side(inv.session.0) == 1) {
                        b.push(&r);
                    }
                }
                Split::Time => {
                    // The median of whatever ordering the trace can actually supply. With
                    // real timestamps that is wall-clock; without them the trace has no time
                    // axis and the honest fallback is its own order, reported as such rather
                    // than presented as a time split.
                    let mut stamps: Vec<f64> = trace
                        .invocations
                        .iter()
                        .filter_map(|i| i.request_start)
                        .collect();
                    let cut = if stamps.len() == trace.invocations.len() && !stamps.is_empty() {
                        stamps
                            .sort_by(|x, y| x.partial_cmp(y).unwrap_or(std::cmp::Ordering::Equal));
                        Some(stamps[stamps.len() / 2])
                    } else {
                        None
                    };
                    let half = trace.invocations.len() / 2;
                    for r in trace.refs_of(|i, inv| match (cut, inv.request_start) {
                        (Some(c), Some(t)) => t < c,
                        _ => i < half,
                    }) {
                        a.push(&r);
                    }
                    for r in trace.refs_of(|i, inv| match (cut, inv.request_start) {
                        (Some(c), Some(t)) => t >= c,
                        _ => i >= half,
                    }) {
                        b.push(&r);
                    }
                }
            }
            let note = if split == Split::Time
                && trace.invocations.iter().any(|i| i.request_start.is_none())
            {
                format!(
                    "{} — NO timestamps on every request, so this split is by the trace's own \
                     order and is a time split only if that order is chronological",
                    split.confound()
                )
            } else {
                split.confound().to_string()
            };
            arms.push((split.name().to_string(), note, a.finish(), b.finish()));
        }
    }

    let reports: Vec<(String, String, workload_model::stats::divergence::Report)> = arms
        .iter()
        .map(|(name, note, a, b)| {
            (
                name.clone(),
                note.clone(),
                // Deliberately NOT marking reuse_distance_bytes incomparable: both sides are
                // traces, so both are in token units and the comparison is of workloads. This
                // is the only context in which that statistic means anything, since no trace
                // carries the model_config a plan's KV bytes would need.
                compare(a, b, &tol),
            )
        })
        .collect();

    if json {
        let out = serde_json::json!({
            "trace": path.display().to_string(),
            "against": against.map(|p| p.display().to_string()),
            "window_requests": window,
            "arms": reports.iter().map(|(name, note, d)| serde_json::json!({
                "arm": name,
                "confound": note,
                "within_tolerance": d.within_tolerance(),
                "divergences": d.divergences,
            })).collect::<Vec<_>>(),
        });
        println!(
            "{}",
            serde_json::to_string_pretty(&out).map_err(|e| e.to_string())?
        );
        return Ok(true);
    }

    println!("floor — two samples of one workload, which is what a perfect model would score");
    println!("  trace   {}", path.display());
    println!("  window  {window} requests, applied to both sides");
    println!(
        "\n  The FR-056 tolerances were calibrated generator-against-itself, where a shared bias\n  \
         cancels. These numbers are the other half: a divergence at or below the floor is \
         unreachable\n  by any model, so a tolerance tighter than the floor can never be met."
    );
    for (name, note, d) in &reports {
        println!("\n  split {name} — {note}");
        println!(
            "    {:<24} {:>10} {:>10} {:>11}  reachable?",
            "statistic", "floor", "tolerance", "full-size"
        );
        for x in &d.divergences {
            let measure = match x.measure {
                Measure::KolmogorovSmirnov => "ks",
                Measure::AreaBetweenCdfs => "area",
                Measure::MaxLogRatio => "log-ratio",
            };
            // Only the KS statistics get the sample-size projection; see the doc comment.
            let projected = match x.measure {
                Measure::KolmogorovSmirnov => {
                    format!("{:>10.5}", x.value / std::f64::consts::SQRT_2)
                }
                _ => format!("{:>10}", "n/a"),
            };
            let verdict = if x.value > x.tolerance {
                "NO — tolerance is below the floor"
            } else {
                "yes"
            };
            println!(
                "    {:<24} {:>10.5} {:>10.5} {projected}  {verdict} ({measure})",
                x.statistic.name(),
                x.value,
                x.tolerance,
            );
        }
    }
    println!(
        "\n  full-size projects a HALF-size KS floor to the size `fit` compares at, dividing by\n  \
         sqrt(2) because a two-sample KS distance scales as sqrt(2/n). It is `n/a` for area and\n  \
         log-ratio measures, which do not scale that way — one correction applied to all four\n  \
         would manufacture three wrong numbers."
    );

    // Fan-in per block, measured the same two ways, because FR-057c requires a candidate
    // statistic's floor and sibling bound BEFORE it is gated on. Reported separately from the
    // table above because it has no tolerance yet — that is the decision this measurement informs.
    print_fanin_floor(&trace, against, block_size, allow_partial)?;

    // With `--against` there are two arms — the sibling bound and this trace's own floor — and
    // their ratio is the statistic's DYNAMIC RANGE: how far apart two different workloads sit,
    // measured in units of how far apart two samples of one workload sit. A statistic with a
    // range near 1 cannot tell workloads apart at all, and gating on one is measuring noise.
    if against.is_some() && reports.len() == 2 {
        let (sibling, own) = (&reports[0].2, &reports[1].2);
        println!("\n  dynamic range — sibling bound over own floor, per statistic");
        println!(
            "    {:<24} {:>10} {:>10} {:>9}  verdict",
            "statistic", "own floor", "sibling", "range"
        );
        for x in &own.divergences {
            let Some(s) = sibling
                .divergences
                .iter()
                .find(|y| y.statistic == x.statistic)
            else {
                continue;
            };
            let range = if x.value > 0.0 {
                s.value / x.value
            } else {
                f64::INFINITY
            };
            // 2x is the weakest range at which a statistic can distinguish a real difference
            // from its own noise at all; below 1 the two workloads are closer than two samples
            // of one, which is not a weak statistic but an actively misleading one.
            let verdict = if range < 1.0 {
                "USELESS — two workloads are closer than two samples of one"
            } else if range < 2.0 {
                "WEAK — barely separates workloads from noise"
            } else {
                "usable"
            };
            println!(
                "    {:<24} {:>10.5} {:>10.5} {:>8.2}x  {verdict}",
                x.statistic.name(),
                x.value,
                s.value,
                range
            );
        }
        println!(
            "    A good statistic needs BOTH a low floor and a high sibling bound. This is the\n    \
             criterion for choosing what to gate on — not how relevant the quantity sounds."
        );
    }
    // Always true: this command measures, it does not judge. A floor above a tolerance is the
    // finding, not a failure of the run, and exiting non-zero would make a sweep over the
    // corpus look like a broken tool.
    Ok(true)
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

/// One per-session quantity decomposed **between** and **within** roots.
///
/// Every "is this a property of the root?" question is this same decomposition, so it is computed
/// once here and named by its caller rather than per quantity: turn-1 path length (FR-054j) and a
/// session's deepest reach (FR-054l) both read it. This file has already had three copies of one
/// arithmetic disagree by a level, which is why there is one.
#[derive(Debug, Clone)]
struct RootAnova {
    /// Between-root share of the variance — `eta²`.
    eta2: f64,
    sessions: u64,
    roots: usize,
    /// Grand mean of the quantity over sessions.
    mean: f64,
    /// Per root: sessions, mean turn-1 path length, and the within-root standard deviation.
    ///
    /// `eta²` is one number over this table, and one number cannot say whether a two-level draw
    /// can reproduce it: that needs the **level** of each root and the **spread** around it, and
    /// whether that spread is a constant number of blocks or a constant fraction of the level.
    /// The table is already built to compute `eta²` and was previously discarded.
    by_root: Vec<RootLevel>,
}

/// One root's turn-1 path-length level and the spread of its sessions around it.
#[derive(Debug, Clone, Copy)]
struct RootLevel {
    sessions: u64,
    mean: f64,
    /// Sample standard deviation, `0.0` for a root with a single session.
    sd: f64,
}

/// One session's shape, as the root-correlation diagnostics need it.
///
/// Turn-1 is the **lowest-numbered** turn, never the first arrival — a disordered session's first
/// arrival is mid-conversation, a trap already recorded for `private_depth`. The root is that turn's
/// first block, which is the binding the emitted root layer uses.
#[derive(Debug, Clone, Copy)]
struct SessionReach {
    turn1: u32,
    root: workload_model::keys::CacheKey,
    /// Turn-1's path length in blocks.
    turn1_blocks: f64,
    /// The DEEPEST turn's path length: how far down the trunk this session ever walks.
    ///
    /// The trunk is re-walked from the root every turn, so a node at depth `d` is reached if ANY
    /// turn reaches it. This is therefore the depth at which the session stops contributing to the
    /// trunk's fan-in — the quantity attrition is made of.
    deepest_blocks: f64,
    turns: f64,
}

/// Per-session root, turn-1 length, deepest reach and turn count.
///
/// Measured here rather than in `fit::sessions` because `SessionShapes` accumulates per-session
/// shapes and does not observe **root identity**: a root is a property of a session's path.
/// Teaching it about roots for one statistic is the larger change, and measuring it here keeps the
/// emitted parameter and the diagnostics below as one computation rather than two that could
/// disagree.
fn session_reaches(
    invocations: &[read::NormalisedInvocation],
) -> workload_model::stats::FastMap<u32, SessionReach> {
    use workload_model::stats::FastMap;

    let mut per: FastMap<u32, SessionReach> = FastMap::default();
    for inv in invocations {
        if inv.blocks.is_empty() {
            continue;
        }
        let len = inv.blocks.len() as f64;
        let e = per.entry(inv.session.0).or_insert(SessionReach {
            turn1: u32::MAX,
            root: inv.blocks[0],
            turn1_blocks: 0.0,
            deepest_blocks: 0.0,
            turns: 0.0,
        });
        // Ties on the lowest turn number resolve to the last one seen, as the turn-1 measurement
        // has always done.
        if inv.turn <= e.turn1 {
            e.turn1 = inv.turn;
            e.root = inv.blocks[0];
            e.turn1_blocks = len;
        }
        e.deepest_blocks = e.deepest_blocks.max(len);
        e.turns += 1.0;
    }
    per
}

/// The between-root variance decomposition of one per-session quantity.
///
/// `obs` is `(root key, value)` per session. `None` when fewer than two sessions carry the
/// quantity, which is when neither `eta²` nor a per-root spread means anything.
fn root_anova(obs: &[(u64, f64)]) -> Option<RootAnova> {
    use workload_model::stats::FastMap;

    if obs.len() < 2 {
        return None;
    }
    // (count, sum, sum of squares) per root: the squares cost nothing here and are what turn the
    // between-root decomposition into the per-root level and spread a two-level draw needs.
    let mut by_root: FastMap<u64, (u64, f64, f64)> = FastMap::default();
    let mut n = 0u64;
    let mut sum = 0.0f64;
    for (root, l) in obs {
        let e = by_root.entry(*root).or_insert((0, 0.0, 0.0));
        e.0 += 1;
        e.1 += l;
        e.2 += l * l;
        n += 1;
        sum += l;
    }
    let grand = sum / n as f64;
    let between: f64 = by_root
        .values()
        .map(|(c, s, _)| *c as f64 * (s / *c as f64 - grand).powi(2))
        .sum();
    let total: f64 = obs.iter().map(|(_, l)| (l - grand).powi(2)).sum();
    let mut levels: Vec<RootLevel> = by_root
        .values()
        .map(|(c, s, sq)| {
            let mean = s / *c as f64;
            // Sample variance from the running sums, floored at zero: the algebraic form can go
            // very slightly negative on a root whose sessions all share one length.
            let var = if *c > 1 {
                (sq - s * mean).max(0.0) / (*c - 1) as f64
            } else {
                0.0
            };
            RootLevel {
                sessions: *c,
                mean,
                sd: var.sqrt(),
            }
        })
        .collect();
    // Descending by population: the roots that carry the sessions are the ones a reader needs, and
    // a stable order makes two runs of the report comparable.
    levels.sort_by(|a, b| b.sessions.cmp(&a.sessions).then(b.mean.total_cmp(&a.mean)));
    Some(RootAnova {
        eta2: if total > 0.0 {
            (between / total).clamp(0.0, 1.0)
        } else {
            0.0
        },
        sessions: n,
        roots: by_root.len(),
        mean: grand,
        by_root: levels,
    })
}

/// Turn-1 path length decomposed by root — the FR-054j measurement.
fn root_path_eta2(invocations: &[read::NormalisedInvocation]) -> Option<RootAnova> {
    let per = session_reaches(invocations);
    let obs: Vec<(u64, f64)> = per.values().map(|r| (r.root.0, r.turn1_blocks)).collect();
    root_anova(&obs)
}

/// Report whether turn-1 path length is a property of the **root**, and what was fitted from it.
///
/// The hypothesis behind both the unmoved `sharing_depth` and the 42%-against-2.5% attrition gap is
/// that a trace correlates these two while the model draws them independently: one root is one task
/// family, whose requests comfortably exceed their shared preamble, so no session ever stops
/// part-way along it. Measured, `eta²` is **0.9941** on `exgentic_swebench` and **0.9869** on
/// `exgentic_tau2_airline` — on an agentic trace, root identity explains 99% of how long a session's
/// requests are — against **0.5791** on `qwen_code`, which is why FR-054j fits the share rather than
/// assuming it is 1.
fn print_root_path_correlation(
    invocations: &[read::NormalisedInvocation],
    trace_rows: &[workload_model::fit::segments::SegmentRow],
) {
    let Some(m) = root_path_eta2(invocations) else {
        return;
    };
    let shortest_path = invocations
        .iter()
        .filter(|i| !i.blocks.is_empty())
        .map(|i| i.blocks.len() as u32)
        .min()
        .unwrap_or(0);
    let root_preamble: Vec<u32> = trace_rows
        .iter()
        .filter(|r| r.start_depth == 0)
        .map(|r| r.length)
        .collect();
    let outrun = root_preamble.iter().filter(|p| **p > shortest_path).count();

    println!(
        "\n  root vs path length — is turn-1 path length a property of the ROOT?\n  \
         eta^2 is the between-root share of the variance in turn-1 path length: near 0 means root\n  \
         identity says nothing about request length, near 1 means it says everything."
    );
    println!(
        "    sessions {}, roots {}, mean turn-1 path {:.1} blocks, eta^2 {:.4}",
        m.sessions, m.roots, m.mean, m.eta2
    );
    // Compared against the GLOBAL shortest path, not each root's own sessions, because the census
    // rows do not carry the root key. So it is an UPPER BOUND on the real violation count, and a
    // loose one exactly when eta^2 is high — short-path sessions are then concentrated on
    // particular roots whose own preambles are short too. Stated rather than presented as the
    // per-root figure it is not.
    println!(
        "    shortest turn-1 path {shortest_path} blocks; {outrun} of {} depth-0 segments exceed \
         it —\n    an UPPER BOUND on preamble-outruns-path violations, loose when eta^2 is high, \
         since the\n    comparison is against the global minimum rather than each root's own \
         sessions",
        root_preamble.len()
    );
    print_within_root_spread(&m);
    print_root_reach(invocations, trace_rows);
}

/// Is a session's DEEPEST reach a property of the root, the way its turn-1 depth is? (FR-054l)
///
/// FR-054j states each root's turn-1 level instead of drawing it, and that fixed the root preamble
/// exactly (124 blocks against the trace's 124 on `exgentic_tau2_airline`). It cannot touch the
/// trunk **below** turn-1 depth, which is reached only by later turns — and their count and growth
/// are drawn from population marginals with no reference to the root. So if reach is a property of
/// the root too, the model scatters departures across deep runs where the trace synchronises them,
/// which is what the structural gate measures as **attrition**: 4129 of 6307 deep segments on
/// `qwen_code` against the trace's 29, and a fitted run length of 136 blocks realised as 2.
///
/// The decisive figure is not `eta²` on its own but the **within-root spread against the length of
/// the runs it has to survive**: a run is chopped by attrition if the sessions walking it stop at
/// different depths, so a within-root sd larger than the trace's median deep run length is enough
/// to fragment every one of them, whatever `eta²` says.
fn print_root_reach(
    invocations: &[read::NormalisedInvocation],
    trace_rows: &[workload_model::fit::segments::SegmentRow],
) {
    let per = session_reaches(invocations);
    let quantity = |f: &dyn Fn(&SessionReach) -> f64| {
        let obs: Vec<(u64, f64)> = per.values().map(|r| (r.root.0, f(r))).collect();
        root_anova(&obs)
    };
    let Some(turn1) = quantity(&|r| r.turn1_blocks) else {
        return;
    };
    let Some(deepest) = quantity(&|r| r.deepest_blocks) else {
        return;
    };
    let Some(turns) = quantity(&|r| r.turns) else {
        return;
    };
    println!(
        "\n  root vs REACH — is a session's DEEPEST turn a property of the root, as turn 1 is?\n  \
         the trunk below turn-1 depth is walked only by later turns, whose count and growth are\n  \
         drawn independently of the root. A within-root spread wider than the runs down there is\n  \
         enough to chop every one of them by attrition, however high eta^2 is."
    );
    println!(
        "        quantity                     eta^2       mean    sd p50   sd/mean p50   roots>1"
    );
    // `sd` summaries over roots holding more than one session only: a lone session states no
    // spread, and counting it as `sd = 0` biases every median toward "tight".
    let row = |name: &str, m: &RootAnova| {
        let multi: Vec<&RootLevel> = m.by_root.iter().filter(|r| r.sessions > 1).collect();
        let med = |mut v: Vec<f64>| {
            if v.is_empty() {
                return f64::NAN;
            }
            v.sort_by(f64::total_cmp);
            v[(v.len() - 1) / 2]
        };
        let sd = med(multi.iter().map(|r| r.sd).collect());
        let cv = med(multi
            .iter()
            .map(|r| if r.mean > 0.0 { r.sd / r.mean } else { 0.0 })
            .collect());
        println!(
            "        {name:<28} {:>6.4} {:>10.1} {sd:>9.1} {cv:>13.4} {:>9}",
            m.eta2,
            m.mean,
            multi.len()
        );
    };
    row("turn-1 path length", &turn1);
    row("deepest turn's path length", &deepest);
    row("turns per session", &turns);
    // The scale that makes the spread mean something: the runs a deep departure would chop.
    let mut deep: Vec<u64> = trace_rows
        .iter()
        .filter(|r| r.start_depth >= 32)
        .map(|r| u64::from(r.length))
        .collect();
    deep.sort_unstable();
    if deep.is_empty() {
        return;
    }
    let run_med = deep[(deep.len() - 1) / 2];
    let multi: Vec<&RootLevel> = deepest.by_root.iter().filter(|r| r.sessions > 1).collect();
    let over = multi.iter().filter(|r| r.sd > run_med as f64).count();
    println!(
        "    for scale: the trace's median shared run below depth 32 is {run_med} blocks, and \
         {over} of {}\n    roots spread their deepest reach wider than that — every run those roots \
         walk is\n    fragmentable by attrition unless reach is stated per root rather than drawn",
        multi.len()
    );
}

/// Is the spread of turn-1 path length **within** a root a constant number of blocks, or a constant
/// fraction of that root's level?
///
/// A two-level draw needs both halves of the variance decomposition, and the second half is not a
/// free choice. Drawing one pooled residual and adding it to a root's level assumes the spread is
/// **additive** — the same number of blocks whether the root's requests run 40 blocks or 4000. If
/// instead it is **multiplicative**, a pooled residual is too wide for short roots and too narrow
/// for long ones, and no correlation parameter repairs that: the marginal comes out right on
/// average and wrong per root.
///
/// The two are discriminated by which quantity is the tighter across roots — the standard deviation
/// itself, or the coefficient of variation. Whichever varies less is the one that behaves like a
/// constant, and the correlation between a root's level and its spread says the same thing a second
/// way. Both are reported, along with the weight of evidence behind them, because a corpus trace
/// can have very few roots with more than one session.
fn print_within_root_spread(m: &RootAnova) {
    // A root with one session states no spread, and including it as `sd = 0` would bias every
    // summary below toward "additive, and tiny".
    let multi: Vec<&RootLevel> = m.by_root.iter().filter(|r| r.sessions > 1).collect();
    let covered: u64 = multi.iter().map(|r| r.sessions).sum();
    println!(
        "\n  within-root spread — ADDITIVE (constant blocks) or MULTIPLICATIVE (constant fraction)?\n  \
         the second half of a two-level draw: a pooled residual added to a root's level assumes\n  \
         additive, and is too wide for short roots and too narrow for long ones if it is not."
    );
    if multi.len() < 3 {
        println!(
            "    only {} of {} roots hold more than one session — not enough to say",
            multi.len(),
            m.roots
        );
        return;
    }
    // Degrees of freedom, so a root observed 400 times counts for more than one observed twice.
    let w: Vec<f64> = multi.iter().map(|r| (r.sessions - 1) as f64).collect();
    let wsum: f64 = w.iter().sum();
    let wmean = |v: &[f64]| v.iter().zip(&w).map(|(x, wi)| x * wi).sum::<f64>() / wsum;
    let means: Vec<f64> = multi.iter().map(|r| r.mean).collect();
    let sds: Vec<f64> = multi.iter().map(|r| r.sd).collect();
    let cvs: Vec<f64> = multi
        .iter()
        .map(|r| if r.mean > 0.0 { r.sd / r.mean } else { 0.0 })
        .collect();
    let (mbar, sbar) = (wmean(&means), wmean(&sds));
    let cov: f64 = means
        .iter()
        .zip(&sds)
        .zip(&w)
        .map(|((x, y), wi)| wi * (x - mbar) * (y - sbar))
        .sum::<f64>()
        / wsum;
    let var = |v: &[f64], bar: f64| {
        v.iter()
            .zip(&w)
            .map(|(x, wi)| wi * (x - bar).powi(2))
            .sum::<f64>()
            / wsum
    };
    let (vm, vs) = (var(&means, mbar), var(&sds, sbar));
    let r = if vm > 0.0 && vs > 0.0 {
        cov / (vm.sqrt() * vs.sqrt())
    } else {
        0.0
    };
    // Relative dispersion of each candidate invariant. Quartiles rather than a standard deviation,
    // because both quantities are bounded below by zero and are visibly right-skewed.
    let quart = |v: &mut Vec<f64>| {
        v.sort_by(f64::total_cmp);
        let at = |q: f64| v[((v.len() - 1) as f64 * q).round() as usize];
        (at(0.25), at(0.5), at(0.75))
    };
    let (sq1, sq2, sq3) = quart(&mut sds.clone());
    let (cq1, cq2, cq3) = quart(&mut cvs.clone());
    let spread_of = |q1: f64, q2: f64, q3: f64| if q2 > 0.0 { (q3 - q1) / q2 } else { f64::NAN };
    let (disp_sd, disp_cv) = (spread_of(sq1, sq2, sq3), spread_of(cq1, cq2, cq3));
    println!(
        "    {} of {} roots hold >1 session, covering {covered} of {} sessions",
        multi.len(),
        m.roots,
        m.sessions
    );
    println!("      sd    per root, blocks    p25 {sq1:>9.1}  p50 {sq2:>9.1}  p75 {sq3:>9.1}   IQR/p50 {disp_sd:>6.3}");
    println!("      sd/mean per root          p25 {cq1:>9.4}  p50 {cq2:>9.4}  p75 {cq3:>9.4}   IQR/p50 {disp_cv:>6.3}");
    println!("      corr(root level, root sd), weighted by degrees of freedom: {r:>+.3}");
    // The verdict is stated as the rule that produced it, so a reader can disagree with the rule
    // rather than having to reverse-engineer it from the numbers.
    let verdict = match () {
        _ if !disp_sd.is_finite() || !disp_cv.is_finite() => "indeterminate — a zero median",
        _ if disp_cv < disp_sd && r > 0.3 => {
            "MULTIPLICATIVE — sd/mean is the tighter invariant and spread rises with level, \
             so a pooled additive residual is the wrong shape"
        }
        _ if disp_sd < disp_cv && r.abs() <= 0.3 => {
            "ADDITIVE — sd is the tighter invariant and does not track level, so a pooled \
             residual in blocks is right"
        }
        _ => "MIXED — the two tests disagree; neither residual form is clearly right",
    };
    println!("      verdict: {verdict}");
}

/// A per-band structural summary of one census, for comparing two of them.
///
/// Deliberately the quantities `print_segment_census` already prints, so a reader can hold the
/// two tables against each other, and deliberately **structural** rather than marginal: FR-056's
/// four statistics are distributions over requests and references, and a trunk can collapse to a
/// handful of chains without any of them firing. That is not hypothetical — it is what this branch
/// spent several sessions failing to see.
#[derive(Debug, Clone, Copy, Default)]
struct BandShape {
    segments: u64,
    len_med: u64,
    fan_in_med: u64,
    /// Segments ending in a real split.
    fanout: u64,
    /// Segments ending because the cohort SHRANK without dividing.
    ///
    /// The discriminator for why a preamble is fragmented. A run ends in `Fanout` when the
    /// structure branches, and in `Attrition` when sessions simply stopped arriving — which in a
    /// generated plan means they ran out of path length part-way along the run. A trace's root
    /// preamble ends in fanout: every session on the root walks the whole thing and then they
    /// split together. So attrition-heavy bands say the model's departures are spread across a
    /// run where the trace's are synchronised to its end.
    attrition: u64,
    /// Total shared blocks in the band — the DENOMINATOR attrition has to be read against.
    ///
    /// A count of attrition events is not comparable between two censuses whose runs are different
    /// lengths: a band whose runs are one block long has no interior for a departure to land in, so
    /// it scores zero attrition however unsynchronised its sessions are. That is not a hypothesis —
    /// `exgentic_tau2_airline`'s trace runs below depth 32 have a median length of exactly 1, and
    /// its zero attrition was read as an invariant. The per-block hazard is the comparable form.
    blocks: u64,
    /// Nothing below at all — every session walking the run stopped at the same node.
    ///
    /// Counted so the three end reasons account for every segment. A band where `fanout + attrition`
    /// falls short of `segments` is one whose runs are ending because the whole cohort ran out, which
    /// is a different defect from losing part of it.
    leaf: u64,
    /// Median run length among segments ending in a real split.
    ///
    /// The discriminator for *why* a realised run is shorter than the fitted one. A run drawn as 136
    /// blocks can only appear in the plan as far as some session walked it, so a short realised run
    /// has two possible causes: the drawn length was short, or the walkers truncated it. Splitting
    /// the median by end reason separates them — if fanout-terminated runs are long and
    /// attrition-terminated ones are short, the draw is right and the walk is cutting it.
    len_med_fanout: u64,
    /// Median run length among segments ending because part of the cohort stopped arriving.
    len_med_attrition: u64,
}

/// Summarise a census's rows into the same bands the fit uses.
fn band_shapes(rows: &[workload_model::fit::segments::SegmentRow]) -> Vec<(u32, BandShape)> {
    let bands = workload_model::fit::segments::BANDS;
    let med = |mut v: Vec<u64>| -> u64 {
        if v.is_empty() {
            return 0;
        }
        v.sort_unstable();
        v[v.len() / 2]
    };
    let len_where = |rows: &[&workload_model::fit::segments::SegmentRow],
                     end: workload_model::fit::segments::SegmentEnd| {
        rows.iter()
            .filter(|r| r.ends == end)
            .map(|r| u64::from(r.length))
            .collect::<Vec<u64>>()
    };
    bands
        .iter()
        .enumerate()
        .map(|(i, lo)| {
            let hi = bands.get(i + 1).copied().unwrap_or(u32::MAX);
            let in_band: Vec<&_> = rows
                .iter()
                .filter(|r| r.start_depth >= *lo && (hi == u32::MAX || r.start_depth < hi))
                .collect();
            (
                *lo,
                BandShape {
                    segments: in_band.len() as u64,
                    len_med: med(in_band.iter().map(|r| u64::from(r.length)).collect()),
                    fan_in_med: med(in_band.iter().map(|r| u64::from(r.fan_in)).collect()),
                    fanout: in_band
                        .iter()
                        .filter(|r| r.ends == workload_model::fit::segments::SegmentEnd::Fanout)
                        .count() as u64,
                    attrition: in_band
                        .iter()
                        .filter(|r| r.ends == workload_model::fit::segments::SegmentEnd::Attrition)
                        .count() as u64,
                    blocks: in_band.iter().map(|r| u64::from(r.length)).sum(),
                    leaf: in_band
                        .iter()
                        .filter(|r| r.ends == workload_model::fit::segments::SegmentEnd::Leaf)
                        .count() as u64,
                    len_med_fanout: med(len_where(
                        &in_band,
                        workload_model::fit::segments::SegmentEnd::Fanout,
                    )),
                    len_med_attrition: med(len_where(
                        &in_band,
                        workload_model::fit::segments::SegmentEnd::Attrition,
                    )),
                },
            )
        })
        .collect()
}

/// The median of `(value, weight)` pairs, weighting each value by its weight.
///
/// Written out because it is the estimator the run-length fit uses (fan-in weighted) and the
/// diagnostic below has to reproduce it exactly to show what it costs against the unweighted one.
/// `0` for an empty input, which is how the report prints an empty band.
fn weighted_median(pairs: &[(u64, u64)]) -> u64 {
    let total: u64 = pairs.iter().map(|(_, w)| *w).sum();
    if total == 0 {
        return 0;
    }
    let mut sorted = pairs.to_vec();
    sorted.sort_unstable();
    // The first value at or past half the weight — the same convention as the unweighted medians in
    // this report, which take the lower of two middles.
    let half = total / 2;
    let mut acc = 0u64;
    for (v, w) in &sorted {
        acc += *w;
        if acc > half {
            return *v;
        }
    }
    sorted.last().map(|(v, _)| *v).unwrap_or(0)
}

/// What a run-length **draw** yields, against what the band states and what the trace has.
///
/// Three medians get confused with each other, and the confusion is a measure mismatch rather than
/// an arithmetic slip:
///
/// * the band's stated `length` is **fan-in weighted** (`fit::segments::fit_process`), so its median
///   is the run length a typical *arrival* walks — a run carrying 16045 sessions counts 16045 times;
/// * the walk draws that distribution once **per node** (`Corpus::run_length`), where every node
///   counts once, whatever its fan-in;
/// * the census reports the median over **segments**, also unweighted.
///
/// So a stated median of 136 against a realised one of 2 is not by itself a defect — the report
/// already warns the two are meant to differ. This makes the comparison an honest one by drawing the
/// band's own distribution per node and printing the empirical median beside the trace's unweighted
/// per-segment median, which is the quantity the walk should be reproducing.
fn print_run_length_draw(
    doc: &workload_model::schema::Document,
    trace_rows: &[workload_model::fit::segments::SegmentRow],
) {
    use workload_model::rng::Stream;
    use workload_model::schema::Branching;

    /// Its own stream tag, so sampling here cannot alias the walk's own run-length stream.
    const LEN_DRAW_TAG: u64 = 0x6c65_6e5f_6472_6177;

    let Branching::Segments(p) = &doc.corpus.trees.branching else {
        return;
    };
    println!(
        "\n    run length: what the band SAYS against what a per-node draw YIELDS\n    \
         the stated quantiles are fan-in weighted (the run a typical ARRIVAL walks); the walk draws\n    \
         them once per NODE. `trace/seg` is the trace's unweighted per-segment median — the quantity\n    \
         a per-node draw is reproducing, and the one to compare `drawn` against."
    );
    println!(
        "    {:>8}  {:>9} {:>9}  {:>9}  {:>9}  {:>9}",
        "depths", "stated~", "drawn~", "drawn mean", "trace/seg", "arrivals~"
    );
    for (i, b) in p.by_depth.iter().enumerate() {
        let hi = p
            .by_depth
            .get(i + 1)
            .map(|n| n.from_depth)
            .unwrap_or(u32::MAX);
        // 20k draws, keyed off the band so two bands cannot share a stream and look alike.
        let mut st = Stream::new(doc.seed ^ LEN_DRAW_TAG, u64::from(b.from_depth));
        let mut v: Vec<u64> = (0..20_000)
            .map(|_| b.length.sample_u64(&mut st).max(1))
            .collect();
        let mean = v.iter().sum::<u64>() as f64 / v.len() as f64;
        v.sort_unstable();
        let drawn = v[v.len() / 2];
        let stated = b.length.quantile(0.5).unwrap_or(f64::NAN);
        // The trace's own segments in this band, unweighted and then fan-in weighted, so the two
        // measures of the same rows can be read against each other.
        let mut seg: Vec<u64> = trace_rows
            .iter()
            .filter(|r| r.start_depth >= b.from_depth && (hi == u32::MAX || r.start_depth < hi))
            .map(|r| u64::from(r.length))
            .collect();
        seg.sort_unstable();
        let arrivals = weighted_median(
            &trace_rows
                .iter()
                .filter(|r| r.start_depth >= b.from_depth && (hi == u32::MAX || r.start_depth < hi))
                .map(|r| (u64::from(r.length), u64::from(r.fan_in)))
                .collect::<Vec<_>>(),
        );
        println!(
            "    {:>8}  {:>9.1} {drawn:>9}  {mean:>10.1}  {:>9}  {arrivals:>9}",
            format!("{}+", b.from_depth),
            stated,
            if seg.is_empty() {
                0
            } else {
                seg[seg.len() / 2]
            }
        );
    }
}

/// Compare the **structure** of the generated trunk against the source trace's.
///
/// The gap this closes: every gated statistic is a marginal over requests or references, and the
/// trunk's *shape* is not recoverable from any of them. A model whose trunk has collapsed to five
/// chains can sit inside all four tolerances, and a model that reproduces three marginals while
/// inverting the segment structure is not a model of the workload. So the generated plan is put
/// through the same `Census` the trace is, and the two censuses are diffed band for band.
///
/// Plan events arrive interleaved by session, and `Census::observe` needs one session's paths
/// contiguous for an exact fan-in — so requests are collected, grouped and then fed, exactly as the
/// trace side does. Reconstructed by `request_id` with `depth` as the ordinal, which is the plan
/// format's own invariant rather than a second convention.
fn print_structure_diff(
    trace_rows: &[workload_model::fit::segments::SegmentRow],
    doc: &workload_model::schema::Document,
) -> Result<(), String> {
    use workload_model::fit::segments::Census;

    // Regenerated here rather than buffered during the search: holding every event of a 20M-event
    // plan costs hundreds of megabytes, and this runs only under `--explain`.
    let mut g = Generator::new(doc).map_err(|e| e.to_string())?;
    let mut plan: Vec<PlanEvent> = Vec::new();
    let mut chunk: Vec<PlanEvent> = Vec::new();
    while !g.is_done() {
        chunk.clear();
        if g.fill(&mut chunk) == 0 {
            break;
        }
        plan.extend_from_slice(&chunk);
    }
    let plan = &plan;

    // request_id -> (session, turn, blocks by depth). Ascending request_id groups one request.
    let mut reqs: Vec<(u32, u32, u16, Vec<workload_model::keys::CacheKey>)> = Vec::new();
    for e in plan {
        match reqs.last_mut() {
            Some((rid, ..)) if *rid == e.request_id => {}
            _ => reqs.push((e.request_id, e.session_id.0, e.turn, Vec::new())),
        }
        let last = reqs.last_mut().expect("just pushed");
        if e.depth as usize >= last.3.len() {
            last.3
                .resize(e.depth as usize + 1, workload_model::keys::CacheKey(0));
        }
        last.3[e.depth as usize] = e.key;
    }
    reqs.sort_by_key(|(_, s, t, _)| (*s, *t));
    let mut census = Census::new();
    for (_, s, _, blocks) in &reqs {
        census.observe(workload_model::keys::SessionId(*s), blocks);
    }
    let plan_rows = census.finish(2);

    println!(
        "\n  trunk structure — the GENERATED trunk against the source's, same census both sides\n           Four marginal tests cannot see the trunk's shape: a collapsed trunk passes all of them.\n           T = trace, S = synthetic; fanin~ is the median cohort. attrit vs fanout is WHY a run\n  \
         ended: fanout means the structure branched, attrition means sessions stopped arriving\n  \
         part-way along it. A trace preamble ends in FANOUT — every session walks it, then they\n  \
         split together. Attrition-heavy means departures are spread across a run, not \
         synchronised.\n  \
         at/kb is attrition per 1000 shared blocks — the COMPARABLE form, since a band whose runs\n  \
         are one block long has no interior for a mid-run departure to land in."
    );
    println!(
        "    {:>8}  {:>8} {:>8}  {:>6} {:>6}  {:>8} {:>8}  {:>9} {:>9}  {:>7} {:>7}  {:>9} {:>9}",
        "depths",
        "segs T",
        "segs S",
        "len T",
        "len S",
        "fanin~T",
        "fanin~S",
        "attrit T",
        "attrit S",
        "at/kb T",
        "at/kb S",
        "fanout T",
        "fanout S"
    );
    let (t, p) = (band_shapes(trace_rows), band_shapes(&plan_rows));
    for ((lo, ts), (_, ps)) in t.iter().zip(p.iter()) {
        if ts.segments == 0 && ps.segments == 0 {
            continue;
        }
        // Per 1000 blocks, so the two sides are comparable when their run lengths are not.
        let hazard = |s: &BandShape| {
            if s.blocks == 0 {
                f64::NAN
            } else {
                s.attrition as f64 * 1000.0 / s.blocks as f64
            }
        };
        println!(
            "    {:>8}  {:>8} {:>8}  {:>6} {:>6}  {:>8} {:>8}  {:>9} {:>9}  {:>7.1} {:>7.1}  {:>9} {:>9}",
            format!("{lo}+"),
            ts.segments,
            ps.segments,
            ts.len_med,
            ps.len_med,
            ts.fan_in_med,
            ps.fan_in_med,
            ts.attrition,
            ps.attrition,
            hazard(ts),
            hazard(ps),
            ts.fanout,
            ps.fanout
        );
    }
    let (tt, pt): (u64, u64) = (
        t.iter().map(|(_, s)| s.segments).sum(),
        p.iter().map(|(_, s)| s.segments).sum(),
    );
    print_run_length_draw(doc, trace_rows);
    // Why a realised run is shorter than the fitted one, which is the sharpest open gap in the
    // trunk fit: `qwen_code`'s band 32-127 states a run length of 136 blocks and realises 2. A run
    // drawn as 136 can only appear in the plan as far as some session walked it, so the median split
    // by end reason separates a short DRAW from a truncated WALK.
    println!(
        "\n    run length by why the run ended — a short draw and a truncated walk look the same in\n    \
         the median above. leaf means the whole cohort stopped at one node, attrition means part of it."
    );
    println!(
        "    {:>8}  {:>9} {:>9}  {:>9} {:>9}  {:>7} {:>7}",
        "depths", "len fan T", "len fan S", "len att T", "len att S", "leaf T", "leaf S"
    );
    for ((lo, ts), (_, ps)) in t.iter().zip(p.iter()) {
        if ts.segments == 0 && ps.segments == 0 {
            continue;
        }
        // A dash where no segment in the band ended that way, so an empty class cannot read as 0.
        let cell = |n: u64, count: u64| {
            if count == 0 {
                "-".to_string()
            } else {
                n.to_string()
            }
        };
        println!(
            "    {:>8}  {:>9} {:>9}  {:>9} {:>9}  {:>7} {:>7}",
            format!("{lo}+"),
            cell(ts.len_med_fanout, ts.fanout),
            cell(ps.len_med_fanout, ps.fanout),
            cell(ts.len_med_attrition, ts.attrition),
            cell(ps.len_med_attrition, ps.attrition),
            ts.leaf,
            ps.leaf
        );
    }
    let (tb, pb): (u64, u64) = (
        t.iter().map(|(_, s)| s.blocks).sum(),
        p.iter().map(|(_, s)| s.blocks).sum(),
    );
    let (ta, pa): (u64, u64) = (
        t.iter().map(|(_, s)| s.attrition).sum(),
        p.iter().map(|(_, s)| s.attrition).sum(),
    );
    let rate = |a: u64, b: u64| {
        if b == 0 {
            f64::NAN
        } else {
            a as f64 * 1000.0 / b as f64
        }
    };
    println!(
        "    whole trunk: trace {ta} attrition over {tb} shared blocks ({:.2}/kblk), synthetic \
         {pa} over {pb} ({:.2}/kblk)",
        rate(ta, tb),
        rate(pa, pb)
    );
    println!(
        "    total shared segments: trace {tt}, synthetic {pt}{}",
        if tt > 0 && pt * 20 < tt {
            "  <- the generated trunk has COLLAPSED relative to the trace's"
        } else {
            ""
        }
    );
    Ok(())
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
) -> Vec<workload_model::fit::segments::SegmentRow> {
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
        return rows;
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
    rows
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
        "    {:>10}  {:>7}  {:>7}  {:>7}  {:>8}  {:>7}  {:>8}  {:>6}  {:>7}  {:>10}  {:>9}  {:>9}",
        "depths",
        "len_p10",
        "len_p50",
        "len_p90",
        "len_mean",
        "deg_p50",
        "deg_mean",
        "skew",
        "escape",
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
            "    {:>10}  {:>7}  {:>7}  {:>7}  {:>8}  {:>7}  {:>8}  {:>6}  {:>7}  {:>10}  {:>9}  {:>9}",
            span,
            q(&b.length, 0.10),
            q(&b.length, 0.50),
            q(&b.length, 0.90),
            m(&b.length),
            q(&b.out_degree, 0.50),
            m(&b.out_degree),
            b.skew.map_or("-".to_string(), |s| format!("{s:.3}")),
            b.singleton_share
                .map_or("-".to_string(), |q| format!("{q:.4}")),
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

    /// `eta²` is 1 when a root fully determines the quantity and 0 when it says nothing, and the
    /// per-root table carries the level and spread a two-level draw would need.
    ///
    /// The arithmetic is shared by every "is this a property of the root?" measurement — turn-1 depth
    /// (FR-054j) and deepest reach (FR-054l) — so a defect here would move a conclusion on both, and
    /// this file has already had three copies of one such computation disagree by a level.
    #[test]
    fn root_anova_separates_between_root_from_within_root_variance() {
        // Two roots, no spread inside either: root identity explains everything.
        let pinned = root_anova(&[(1, 10.0), (1, 10.0), (2, 90.0), (2, 90.0)]).expect("two roots");
        assert!((pinned.eta2 - 1.0).abs() < 1e-12, "{}", pinned.eta2);
        assert_eq!(pinned.roots, 2);
        assert_eq!(pinned.sessions, 4);
        assert!((pinned.mean - 50.0).abs() < 1e-12);
        // Sorted by population, then level: both roots hold two sessions with sd 0.
        assert!(pinned.by_root.iter().all(|r| r.sd == 0.0));

        // The same spread on both roots with the same level: root identity explains nothing.
        let flat = root_anova(&[(1, 10.0), (1, 90.0), (2, 10.0), (2, 90.0)]).expect("two roots");
        assert!(flat.eta2 < 1e-12, "{}", flat.eta2);
        // Sample sd of {10, 90}, which is what a pooled residual would have to reproduce.
        assert!((flat.by_root[0].sd - 40.0_f64 * 2.0_f64.sqrt()).abs() < 1e-9);

        // A single session states neither, and is refused rather than reported as eta^2 = 0.
        assert!(root_anova(&[(1, 10.0)]).is_none());
    }

    /// Fan-in weighting moves the run-length median, and which way is a property of the trace.
    ///
    /// The point of the diagnostic it serves: `length` is fitted **fan-in weighted** but consumed
    /// **per node**, so the two estimators answer different questions about the same rows. On
    /// `qwen_code`'s root band the weighted median is 1 where the unweighted one is 29, because the
    /// splits carrying thousands of sessions are the short ones.
    #[test]
    fn a_weighted_median_can_sit_far_from_the_unweighted_one() {
        // Three long runs walked by one session each, one short run walked by 100.
        let rows = [(30, 1), (31, 1), (32, 1), (2, 100)];
        assert_eq!(weighted_median(&rows), 2);
        let mut plain: Vec<u64> = rows.iter().map(|(v, _)| *v).collect();
        plain.sort_unstable();
        assert_eq!(plain[plain.len() / 2], 31);
        // Equal weights reduce to the ordinary median, so the estimator is not two different rules.
        assert_eq!(weighted_median(&[(1, 1), (5, 1), (9, 1)]), 5);
        assert_eq!(weighted_median(&[]), 0);
    }

    /// The attrition denominator is the band's shared blocks, so two censuses whose runs are
    /// different lengths are comparable (FR-054l).
    ///
    /// Counting attrition *segments* alone is what let `exgentic_tau2_airline`'s zero be read as an
    /// invariant when its deep runs have a median length of exactly 1 — a band of one-block runs has
    /// no interior for a mid-run departure to land in.
    #[test]
    fn band_shapes_carry_the_shared_blocks_attrition_is_read_against() {
        use workload_model::fit::segments::{SegmentEnd, SegmentRow};
        let row = |start_depth, length, ends| SegmentRow {
            start_depth,
            length,
            fan_in: 3,
            ends,
            out_degree: 1,
            shared_children: 0,
            child_fan_in_sum: 0,
            shared_fan_in_sum: 0,
            child_fan_in_sq: 0.0,
            child_fan_in_sq_all: 0.0,
            top_child_fan_in: 0,
        };
        // One 100-block run losing a session, and four 1-block runs that cannot: same band.
        let rows = vec![
            row(40, 100, SegmentEnd::Attrition),
            row(41, 1, SegmentEnd::Fanout),
            row(42, 1, SegmentEnd::Fanout),
            row(43, 1, SegmentEnd::Fanout),
            row(44, 1, SegmentEnd::Fanout),
        ];
        let band = band_shapes(&rows)
            .into_iter()
            .find(|(lo, s)| *lo == 32 && s.segments > 0)
            .expect("band 32+")
            .1;
        assert_eq!(band.segments, 5);
        assert_eq!(band.attrition, 1);
        assert_eq!(band.blocks, 104);
        // 1 in 5 segments, but 1 in 104 blocks: the two readings differ by 20x on the same data.
        assert!((band.attrition as f64 * 1000.0 / band.blocks as f64 - 9.615).abs() < 0.001);
    }
}
