//! `certus-workload` — turn a compact statement of a workload's statistics into a
//! deterministic plan, and emit that plan as an interchange trace.
//!
//! Two subcommands here, and the division between the three binaries follows the
//! direction of the dependency rather than convenience (spec FR-021h). `plan` and
//! `emit` need nothing beyond `serde_json`, so they live here; parquet conversion
//! lives in `certus-trace` because a columnar writer would otherwise put `arrow`
//! in a crate that `cargo test --all` builds on every run; and driving a server
//! lives in `certus-workload-run`, which is the only mode that involves Certus at
//! all.
//!
//! `report` prints what a workload *is*, computed from the plan alone: the
//! reuse-distance CDF, the compulsory-miss floor, realised sharing and trunk
//! shape, and the working-set size. It involves no consumer, no capacity and no
//! cache model (spec FR-034), and the statistics themselves live in
//! `workload-model::stats` rather than here, because `certus-trace` computes the
//! same ones over real traces and two implementations would drift (FR-021i).

use std::path::{Path, PathBuf};
use std::process::ExitCode;

use clap::{Parser, Subcommand};
use workload_model::plan::manifest::Identity;
use workload_model::plan::{read_plan, unbounded_manifest, write_plan, Budget, Generator};
use workload_model::schema::validate::{validate, Severity};
use workload_model::schema::{extends, Document};
use workload_model::stats::{Provenance, Statistics};
use workload_model::trace::{requests, Emitter, TraceManifest, DEFAULT_BLOCK_SIZE_TOKENS};

#[derive(Parser)]
#[command(
    name = "certus-workload",
    version,
    about = "Generate a deterministic KV workload plan from a compact YAML statement",
    long_about = None
)]
struct Cli {
    #[command(subcommand)]
    cmd: Cmd,
}

#[derive(Subcommand)]
enum Cmd {
    /// Generate a plan artifact from a workload document.
    Plan {
        /// The workload document.
        #[arg(short, long, value_name = "FILE")]
        config: PathBuf,
        /// Where to write the `.plan/` directory.
        #[arg(short = 'o', long, value_name = "DIR")]
        out: Option<PathBuf>,
        /// Print the document after `extends` merge and defaulting, and stop.
        /// This is what makes two configurations comparable by `diff`.
        #[arg(long)]
        print_normalised: bool,
        /// Validate and report, without generating.
        #[arg(long)]
        check: bool,
        /// Look-ahead depth in events. Reported either way, because a horizon too
        /// short makes the generator the bottleneck (FR-021f).
        #[arg(long, value_name = "EVENTS")]
        horizon: Option<usize>,
    },
    /// Characterise a plan, without running anything.
    ///
    /// Every statistic is a property of the reference stream: none needs a
    /// capacity, a replacement policy or a consumer of any kind (FR-034a). The
    /// reuse-distance CDF is the primary one — a consumer reads any capacity point
    /// off it, so this tool never has to model a cache to tell it one.
    Report {
        /// The `.plan/` directory.
        #[arg(short = 'p', long, value_name = "DIR")]
        plan: PathBuf,
        /// Emit the machine-readable form instead of the human summary (FR-048).
        #[arg(long)]
        json: bool,
        /// Also print the normalised input the plan was generated from, which the
        /// report embeds either way (FR-047).
        #[arg(long)]
        show_input: bool,
    },
    /// Emit an existing plan as a JSONL interchange trace.
    ///
    /// Reads `events.bin` rather than regenerating, which FR-021c requires: the
    /// interchange formats are a view of the native artifact, not a second
    /// generation of it that might differ.
    Emit {
        /// The `.plan/` directory.
        #[arg(short = 'p', long, value_name = "DIR")]
        plan: PathBuf,
        /// Output `.jsonl` path; its `manifest.json` goes beside it.
        #[arg(short = 'o', long, value_name = "FILE")]
        out: PathBuf,
        /// Block budget. **Required**: blocks are the only unit that converts
        /// directly to a file size, so without one a long run fills the
        /// filesystem (FR-021d).
        #[arg(long, value_name = "N")]
        blocks: u64,
        /// Tokens per block, as declared in the trace manifest.
        #[arg(long, default_value_t = DEFAULT_BLOCK_SIZE_TOKENS, value_name = "TOKENS")]
        block_size: u32,
    },
}

fn main() -> ExitCode {
    let cli = Cli::parse();
    let r = match cli.cmd {
        Cmd::Plan {
            config,
            out,
            print_normalised,
            check,
            horizon,
        } => cmd_plan(&config, out.as_deref(), print_normalised, check, horizon),
        Cmd::Report {
            plan,
            json,
            show_input,
        } => cmd_report(&plan, json, show_input),
        Cmd::Emit {
            plan,
            out,
            blocks,
            block_size,
        } => cmd_emit(&plan, &out, blocks, block_size),
    };
    match r {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("certus-workload: {e}");
            ExitCode::FAILURE
        }
    }
}

/// Read a document, resolve its `extends` chain, and normalise it.
///
/// The normalised form is what identity is taken over, so it is produced once
/// here and used for both the hash and `--print-normalised`. Two paths to it
/// could differ, and then a plan's hash would not describe what the user was
/// shown.
///
/// Unit normalisation happens inside [`Document::from_value`] rather than here,
/// after the `extends` merge, so an inherited `128KiB` is converted once and a
/// preset and the document that extends it cannot disagree about what a suffix
/// meant.
fn load(path: &Path) -> Result<(Document, String), String> {
    let text = std::fs::read_to_string(path).map_err(|e| format!("{}: {e}", path.display()))?;
    let root: serde_yaml::Value =
        serde_yaml::from_str(&text).map_err(|e| format!("{}: {e}", path.display()))?;
    // Presets resolve relative to the including document, so a config directory
    // is movable.
    let base = path.parent().unwrap_or(Path::new(".")).to_path_buf();
    let merged = extends::resolve(root, &|p: &str| {
        let candidate = if Path::new(p).is_absolute() {
            PathBuf::from(p)
        } else {
            base.join(p)
        };
        std::fs::read_to_string(&candidate).map_err(|e| format!("{}: {e}", candidate.display()))
    })
    .map_err(|e| e.to_string())?;
    let doc = Document::from_value(merged).map_err(|e| e.to_string())?;
    let normalised = doc.to_yaml().map_err(|e| e.to_string())?;
    Ok((doc, normalised))
}

/// Print findings; returns whether the document was rejected.
fn report_findings(doc: &Document) -> bool {
    let r = validate(doc);
    for f in &r.findings {
        let label = match f.severity {
            Severity::Reject => "reject",
            Severity::Warn => "warn",
        };
        eprintln!("{label} [rule {}] {}", f.rule, f.message);
    }
    r.is_rejected()
}

fn cmd_plan(
    config: &Path,
    out: Option<&Path>,
    print_normalised: bool,
    check: bool,
    horizon: Option<usize>,
) -> Result<(), String> {
    let (doc, normalised) = load(config)?;
    if print_normalised {
        // Deliberately before validation and with nothing else on stdout: this
        // output is diffed against another document's, so any commentary on the
        // stream would show up as a difference between the configurations.
        print!("{normalised}");
        return Ok(());
    }
    if report_findings(&doc) {
        return Err("the document was rejected; nothing was generated".to_string());
    }
    if check {
        println!("ok: the document is usable");
        return Ok(());
    }
    let out = out.ok_or(
        "plan needs -o <DIR> to write to, or --check to validate only, or \
         --print-normalised to inspect the merged document",
    )?;
    let mut g = match horizon {
        Some(h) => Generator::with_horizon(&doc, h),
        None => Generator::new(&doc),
    }
    .map_err(|e| e.to_string())?;

    // FR-021f requires the look-ahead stated, whether or not the run is unbounded:
    // a horizon too short makes the generator the bottleneck FR-037 exists to
    // prevent, and a horizon too long is what an unbounded run cannot afford.
    eprintln!("{}", g.horizon());

    if g.budget().is_unbounded() {
        // No events.bin, and that is the feature: nothing accumulates. Identity is
        // the generator's own (FR-021g), and the manifest says which kind it is so
        // it can never be read as a plan hash.
        let m = unbounded_manifest(&g, &normalised);
        std::fs::create_dir_all(out).map_err(|e| e.to_string())?;
        std::fs::write(
            out.join("manifest.json"),
            m.to_json().map_err(|e| e.to_string())?,
        )
        .map_err(|e| e.to_string())?;
        println!(
            "unbounded run: no events.bin (nothing accumulates)\nidentity: {} {}",
            m.identity.label(),
            m.identity.digest()
        );
        return Ok(());
    }

    let m = write_plan(out, &mut g, &normalised).map_err(|e| e.to_string())?;
    let events = m.event_count.unwrap_or(0);
    println!(
        "{} events in {} requests, {:.3} GiB referenced\nidentity: {} {}\nstream digest: {}",
        events,
        g.requests_emitted(),
        m.corpus_summary.total_bytes as f64 / (1024.0 * 1024.0 * 1024.0),
        m.identity.label(),
        m.identity.digest(),
        m.stream_digest,
    );
    if let Budget::Blocks(n) = g.budget() {
        if events < n {
            // Said rather than left for the reader to notice: the budget is
            // honoured at request granularity, because half a request is not a
            // request.
            eprintln!(
                "note: {events} of the {n} block budget used; a plan stops at a request boundary"
            );
        }
    }
    Ok(())
}

/// Characterise a plan from its own events.
///
/// The window comes from the manifest rather than being re-derived from the
/// embedded YAML: the manifest records what the plan was actually generated
/// against, and occupancy scales linearly with the window, so re-resolving it here
/// could characterise a plan against a different window than the one whose
/// occupancy floor it passed.
fn cmd_report(plan: &Path, json: bool, show_input: bool) -> Result<(), String> {
    let (m, events) = read_plan(plan).map_err(|e| e.to_string())?;
    let window = m.corpus_summary.wss_window_requests;
    if window == 0 {
        return Err("the plan's manifest carries no wss_window; refusing to invent one".into());
    }

    let mut stats = Statistics::new(window);
    stats.push_events(&events);
    let mut report = stats.finish();

    // FR-047: a report must be attributable to the exact input that produced it,
    // and FR-012a wants the configured sharing distribution stated beside the
    // realised one -- as a second statistic, never as a stand-in for it.
    let (content_hash, parameter_hash) = match &m.identity {
        Identity::ContentHash(h) => (Some(h.clone()), None),
        Identity::ParameterHash(h) => (None, Some(h.clone())),
    };
    report = report.with_provenance(Provenance {
        content_hash,
        parameter_hash,
        stream_digest: Some(m.stream_digest.clone()),
        normalised_yaml: Some(m.normalised_yaml.clone()),
    });
    if let Ok(doc) = serde_yaml::from_str::<Document>(&m.normalised_yaml) {
        report = report.with_intended_shared_depth(&doc.corpus.trees.shared_depth);
    }

    if json {
        println!("{}", report.to_json().map_err(|e| e.to_string())?);
        return Ok(());
    }
    print!("{}", report.to_text());
    if show_input {
        println!("normalised input\n{}", m.normalised_yaml);
    }
    Ok(())
}

fn cmd_emit(plan: &Path, out: &Path, blocks: u64, block_size: u32) -> Result<(), String> {
    let (m, events) = read_plan(plan).map_err(|e| e.to_string())?;
    let trace_id = out
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("synthetic")
        .to_string();
    let mut em = Emitter::new(&trace_id, block_size, m.time_origin_ns);
    let file = std::fs::File::create(out).map_err(|e| format!("{}: {e}", out.display()))?;
    let mut w = std::io::BufWriter::new(file);
    let mut written = 0u64;
    let mut truncated = false;
    for r in requests(&events) {
        // The budget is honoured at request granularity for the same reason the
        // plan's is: a truncated request is not a request, and a reader
        // reconstructing block lists from one would get a shorter conversation
        // rather than an obviously partial file.
        if written + r.len() as u64 > blocks {
            truncated = true;
            break;
        }
        if let Some(rec) = em.request(r) {
            use std::io::Write;
            let line = serde_json::to_string(&rec).map_err(|e| e.to_string())?;
            writeln!(w, "{line}").map_err(|e| e.to_string())?;
            written += r.len() as u64;
        }
    }
    {
        use std::io::Write;
        w.flush().map_err(|e| e.to_string())?;
    }
    let stats = em.stats();
    let tm = TraceManifest::synthetic(&trace_id, em.block_size(), stats.clone());
    let manifest_path = out.parent().unwrap_or(Path::new(".")).join("manifest.json");
    std::fs::write(&manifest_path, tm.to_json().map_err(|e| e.to_string())?)
        .map_err(|e| format!("{}: {e}", manifest_path.display()))?;
    println!(
        "{} invocations, {} sessions, {} unique blocks, {written} blocks\nmanifest: {} \
         (provenance synthetic, timestamps synthetic)",
        stats.invocations,
        stats.sessions,
        stats.unique_blocks,
        manifest_path.display(),
    );
    if truncated {
        // A reader tells a full trace from a sample by the manifest's invocation
        // count, and that count is what was written -- so this note is about the
        // plan being longer than the budget rather than about the file being a
        // partial copy of something.
        eprintln!(
            "note: stopped at the {blocks}-block budget; the plan carries {} events",
            events.len()
        );
    }
    Ok(())
}
