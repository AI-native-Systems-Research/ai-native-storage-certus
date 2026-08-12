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
//! `fit` and `convert` are not in this build yet; `validate` is.

use std::path::{Path, PathBuf};
use std::process::ExitCode;

use clap::{Parser, Subcommand};
use workload_model::plan::read_plan;
use workload_model::stats::divergence::{
    compare, Measure, Statistic, Tolerances, DEFAULT_TOLERANCE_MIN_REQUESTS,
};
use workload_model::stats::{Report, Statistics};

mod read;

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
    },
}

fn main() -> ExitCode {
    let cli = Cli::parse();
    let r = match cli.cmd {
        Cmd::Validate {
            plan,
            against_plan,
            trace,
            allow_partial,
            tolerances,
            json,
        } => cmd_validate(
            &plan,
            against_plan.as_deref(),
            trace.as_deref(),
            allow_partial,
            &tolerances,
            json,
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

fn cmd_validate(
    plan: &Path,
    against_plan: Option<&Path>,
    trace: Option<&Path>,
    allow_partial: bool,
    tolerance_args: &[String],
    json: bool,
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
