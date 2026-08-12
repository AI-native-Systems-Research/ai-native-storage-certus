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
//! `convert` is not in this build yet; `fit` and `validate` are.

use std::path::{Path, PathBuf};
use std::process::ExitCode;

use clap::{Parser, Subcommand};
use workload_model::fit::branching;
use workload_model::fit::document::{assemble, RootPopularity, Supplied};
use workload_model::fit::sessions::{scale_values, SessionShapes};
use workload_model::plan::{read_plan, Generator, PlanEvent};
use workload_model::stats::divergence::{
    compare, Measure, Statistic, Tolerances, DEFAULT_TOLERANCE_MIN_REQUESTS,
};
use workload_model::stats::{Ref, Report, Statistics};

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
        /// The `.jsonl` trace to fit.
        #[arg(short = 't', long, value_name = "FILE")]
        trace: PathBuf,
        /// Where to write the fitted YAML. Omit to print the report only.
        #[arg(short = 'o', long, value_name = "FILE")]
        out: Option<PathBuf>,
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
        /// A `.jsonl` trace to compare against.
        #[arg(long, value_name = "FILE")]
        trace: Option<PathBuf>,
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
}

fn main() -> ExitCode {
    let cli = Cli::parse();
    let r = match cli.cmd {
        Cmd::Fit {
            trace,
            out,
            block_bytes,
            wss_window,
            seed,
            rate,
            allow_partial,
            tolerances,
            explain,
        } => cmd_fit(
            &trace,
            out.as_deref(),
            block_bytes,
            wss_window,
            seed,
            rate,
            allow_partial,
            &tolerances,
            explain,
        ),
        Cmd::Validate {
            plan,
            against_plan,
            trace,
            allow_partial,
            tolerances,
            json,
            explain,
        } => cmd_validate(
            &plan,
            against_plan.as_deref(),
            trace.as_deref(),
            allow_partial,
            &tolerances,
            json,
            explain,
        ),
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
#[allow(clippy::too_many_arguments)]
fn cmd_fit(
    trace_path: &Path,
    out: Option<&Path>,
    block_bytes: u64,
    wss_window: u64,
    seed: u64,
    rate: Option<f64>,
    allow_partial: bool,
    tolerance_args: &[String],
    explain: bool,
) -> Result<bool, String> {
    let tol = parse_tolerances(tolerance_args)?;
    let trace = read::jsonl::read_trace(trace_path, allow_partial).map_err(|e| e.to_string())?;
    if !trace.capabilities.trunk_fittable() {
        return Err(
            "this trace has no session identity, so cross-session sharing is invisible and \
             occupancy has no denominator: the trunk cannot be fitted from it at all. Arrival \
             and size parameters would still be available, which is `supports: R = partial` \
             doing what it says"
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

    // The occupancy numerator the root fold needs: sessions reaching the depth the
    // fit treats as the sharing depth.
    let sessions_at_sharing = trace_report
        .sharing
        .realised_depth
        .p99
        .and_then(|d| trace_report.trunk.depths.get(d as usize))
        .and_then(|d| d.occupancy.map(|o| o * d.width_run as f64))
        .unwrap_or(trace.sessions() as f64);
    let fitted_branching = branching::fit(&trace_report.trunk, sessions_at_sharing)
        .ok_or("the trace has no width profile to fit")?;

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
    let fitted = assemble(
        &fitted_branching,
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
        "  trunk     roots.count {} at boundary depth {} (retention {:.3}), {} segments, \
         fitted to depth {} of {}",
        fitted_branching.roots,
        fitted_branching.root_boundary_depth,
        fitted_branching.retention_at_boundary,
        fitted_branching.segments.len(),
        fitted_branching.fitted_to_depth,
        fitted_branching.observed_to_depth
    );

    println!("\n  not fitted");
    for u in &fitted.unset {
        println!("    - {u}");
    }
    println!("\n  caveats");
    for c in &fitted.caveats {
        println!("    - {c}");
    }

    if !rejections.is_empty() {
        println!("\n  REFUSED: the fitted document does not pass the schema");
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
        print_explanation(&synthetic, &trace_report, &d, "synthetic", "trace");
    }

    if !d.within_tolerance() {
        println!(
            "\n  REFUSED: the fitted model's synthetic output does not resemble its source. \
             FR-057 fails rather than emitting it — a plausible YAML nobody can tell is wrong \
             is worse than no YAML"
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
fn trace_report(path: &Path, window: u64, allow_partial: bool) -> Result<(Report, String), String> {
    let trace = read::jsonl::read_trace(path, allow_partial).map_err(|e| e.to_string())?;
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
            let (b, note) = trace_report(t, window, allow_partial)?;
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
