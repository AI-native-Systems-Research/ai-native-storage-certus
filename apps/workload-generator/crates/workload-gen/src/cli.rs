//! Subcommand wiring. `contracts/cli.md` is normative.
//!
//! # `emit` refuses before it writes, and the two checks are not the same
//!
//! FR-073 gives the projection two jobs, and conflating them would make one of them
//! useless:
//!
//! - **Free space** is a hard check. `--force` does not override it, because
//!   overriding it does not get you a trace — it gets you a truncated directory and a
//!   full filesystem, and on a shared box it gets *other people* a full filesystem.
//! - **The size ceiling** is a guard rail, and `--force` does override it. It exists
//!   because `--until 100000` is legal on the shipped example and costs tens of
//!   gigabytes, which is a thing somebody will do by accident exactly once.
//!
//! Both refusals name **both figures** — what was projected and what the limit is —
//! because a refusal that says only "too large" cannot be acted on.
//!
//! # Free space comes from `statvfs`, and its absence is a refusal too
//!
//! Read through `libc::statvfs` on the output directory's own filesystem, not on the
//! working directory: those are routinely different mounts, and checking the wrong
//! one is worse than not checking, since it reports a number that looks authoritative.
//!
//! If the call fails the run is **refused**, not permitted. A projection that cannot
//! be compared against anything is not a check that passed.

use std::fs;
use std::io::{BufWriter, Write};
use std::path::{Path, PathBuf};
use std::time::Instant;

use clap::{Parser, Subcommand, ValueEnum};
use workload_model::description::WorkloadDescription;
use workload_model::plan::OperationPlan;
use workload_model::project::{project, span_for_invocations, Projection};
use workload_model::sim::Simulation;
use workload_trace::cachesim::CsvWriter;
use workload_trace::jsonl::JsonlWriter;
use workload_trace::manifest::{BlockStats, Manifest};
use workload_trace::mooncake::MooncakeWriter;
use workload_trace::record::InvocationRecord;
use workload_trace::simulator::SimulatorWriter;

use crate::report::{ContainerRecords, EmitReport, ProjectionSummary, Reproduction};

/// Documented size ceiling for an emit run, in bytes.
///
/// 32 GiB. Chosen as "larger than any deliberate run so far, smaller than a
/// filesystem", and overridable with `--force` — the point is to catch a span typed
/// with an extra zero, not to express a policy about disk use.
pub const SIZE_CEILING_BYTES: u64 = 32 * 1024 * 1024 * 1024;

/// Exit codes, from `contracts/cli.md`.
///
/// All five are defined here even though the emit path can only produce three.
/// `INVALID` and `PEER` belong to the live path (US1) and are part of the contract
/// now — defining the set in one place is what keeps a later addition from inventing
/// a sixth code with an overlapping meaning.
#[allow(
    dead_code,
    reason = "INVALID and PEER are the live path's, and it is US1"
)]
pub mod exit {
    /// Success; for a live run, a valid one.
    pub const OK: i32 = 0;
    /// Anything not covered by a more specific code.
    pub const OTHER: i32 = 1;
    /// Configuration rejected at load — nothing was issued.
    pub const CONFIG: i32 = 2;
    /// Completed but **invalid**. Distinct from success so a sweep driver cannot
    /// mistake an invalid run for a data point.
    pub const INVALID: i32 = 3;
    /// A peer refused: protocol or `build_id` mismatch.
    pub const PEER: i32 = 4;
}

/// What `convert` projects a stored trace into.
#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum ConvertTo {
    /// The shape `apps/eviction-replay-benchmark` reads.
    Simulator,
    /// The Mooncake FAST'25 trace format — the standard-format export.
    Mooncake,
    /// libCacheSim CSV.
    Cachesim,
    /// libCacheSim's binary `oracleGeneral`, with next-access ordinals — which no real
    /// trace can supply, so this is what makes Belady baselines available.
    OracleGeneral,
}

/// Which containers to write.
#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum Format {
    /// Newline-delimited JSON.
    Jsonl,
    /// Parquet. Requires the `parquet` feature.
    Parquet,
    /// Both, which is what SC-004's equivalence claim is about.
    Both,
}

/// The generator's command line.
#[derive(Debug, Parser)]
#[command(
    name = "workload-gen",
    about = "Synthetic workload generator for Certus"
)]
pub struct Cli {
    /// What to do.
    #[command(subcommand)]
    pub command: Command,
}

/// Subcommands.
#[derive(Debug, Subcommand)]
pub enum Command {
    /// Write a trace file. Contacts no server and needs no accelerator.
    Emit {
        /// The workload description.
        description: PathBuf,
        /// Virtual-second span. **Required**: an unbounded file is not a thing.
        #[arg(long)]
        until: f64,
        /// Directory for the native trace.
        #[arg(long)]
        output: PathBuf,
        /// Which containers to write.
        #[arg(long, value_enum, default_value_t = Format::Jsonl)]
        format: Format,
        /// Seed. Required for reproducibility; there is no random default, because a
        /// run whose seed was never printed cannot be repeated.
        #[arg(long)]
        seed: u64,
        /// Structured report destination. Defaults to `report.json` in `--output`.
        #[arg(long)]
        report: Option<PathBuf>,
        /// Also write the Mooncake projection here, in the same pass.
        #[arg(long)]
        mooncake: Option<PathBuf>,
        /// Also write libCacheSim CSV here, in the same pass.
        #[arg(long)]
        cachesim: Option<PathBuf>,
        /// Also write the cache-simulator projection here, in the same pass.
        ///
        /// A projection is not a trace (FR-075b): no manifest, and never accepted in
        /// place of the native trace for a reproducibility check.
        #[arg(long)]
        simulator: Option<PathBuf>,
        /// Override the documented size ceiling. Never overrides the free-space
        /// check.
        #[arg(long)]
        force: bool,
    },
    /// Project a stored trace into another tool's format.
    ///
    /// Its input is the *schema*, so it works on any trace in it — including the real
    /// ones in the corpus, which is what makes a real workload and a generated one
    /// comparable through the identical projection (FR-075a).
    Convert {
        /// A trace directory, or a single JSONL part file.
        trace: PathBuf,
        /// Target format.
        #[arg(long, value_enum)]
        to: ConvertTo,
        /// Output file.
        #[arg(long)]
        output: PathBuf,
    },
    /// Run the load-time checks and report the effective distributions and the
    /// projection, without writing anything.
    Validate {
        /// The workload description.
        description: PathBuf,
        /// Project a span as well as validating.
        #[arg(long)]
        until: Option<f64>,
        /// Invert the projection: suggest a span for this many invocations.
        #[arg(long)]
        for_invocations: Option<u64>,
        /// Seed for the projection's Monte Carlo.
        #[arg(long, default_value_t = 1)]
        seed: u64,
    },
    /// Write the canonical plan serialisation — the artifact byte-identity is
    /// asserted against.
    Plan {
        /// The workload description.
        description: PathBuf,
        /// Virtual-second span.
        #[arg(long)]
        until: f64,
        /// Output file.
        #[arg(long)]
        output: PathBuf,
        /// Seed.
        #[arg(long)]
        seed: u64,
    },
}

/// Parse the real process arguments and run.
///
/// # Errors
///
/// Never returns an error; a usage problem becomes an exit code.
pub fn run_argv_os() -> i32 {
    let argv: Vec<String> = std::env::args().collect();
    run_argv(&argv)
}

/// Parse `argv` and run, returning the exit code.
///
/// A clap usage error becomes [`exit::CONFIG`], not a panic: the invocation was
/// rejected before anything was issued, which is exactly what code 2 means.
pub fn run_argv(argv: &[String]) -> i32 {
    match Cli::try_parse_from(argv) {
        Ok(cli) => run(cli),
        Err(e) => {
            // clap renders help and version through the same error channel; those are
            // a success, not a refusal.
            let is_help = matches!(
                e.kind(),
                clap::error::ErrorKind::DisplayHelp | clap::error::ErrorKind::DisplayVersion
            );
            let _ = e.print();
            if is_help {
                exit::OK
            } else {
                exit::CONFIG
            }
        }
    }
}

/// Run the CLI, returning the process exit code.
///
/// Returns a code rather than calling `exit` so that the whole surface is testable
/// in-process: a subcommand that can only be checked by spawning a binary tends not
/// to be checked at all.
pub fn run(cli: Cli) -> i32 {
    match cli.command {
        Command::Emit {
            description,
            until,
            output,
            format,
            seed,
            report,
            mooncake,
            cachesim,
            simulator,
            force,
        } => match emit(
            &description,
            until,
            &output,
            format,
            seed,
            report,
            Projections {
                mooncake,
                cachesim,
                simulator,
            },
            force,
        ) {
            Ok(text) => {
                print!("{text}");
                exit::OK
            }
            Err(e) => {
                eprintln!("{e}");
                e.code()
            }
        },
        Command::Validate {
            description,
            until,
            for_invocations,
            seed,
        } => match validate(&description, until, for_invocations, seed) {
            Ok(text) => {
                print!("{text}");
                exit::OK
            }
            Err(e) => {
                eprintln!("{e}");
                e.code()
            }
        },
        Command::Plan {
            description,
            until,
            output,
            seed,
        } => match plan(&description, until, &output, seed) {
            Ok(text) => {
                print!("{text}");
                exit::OK
            }
            Err(e) => {
                eprintln!("{e}");
                e.code()
            }
        },
        Command::Convert { trace, to, output } => match convert(&trace, to, &output) {
            Ok(text) => {
                print!("{text}");
                exit::OK
            }
            Err(e) => {
                eprintln!("{e}");
                e.code()
            }
        },
    }
}

/// Projections an emit run may write alongside the native trace (FR-075).
#[derive(Debug, Default)]
pub struct Projections {
    /// Mooncake output path.
    pub mooncake: Option<PathBuf>,
    /// libCacheSim CSV output path.
    pub cachesim: Option<PathBuf>,
    /// Cache-simulator output path.
    pub simulator: Option<PathBuf>,
}

/// The `convert` subcommand.
fn convert(trace: &Path, to: ConvertTo, output: &Path) -> Result<String, Failure> {
    let input_path = if trace.is_dir() {
        find_jsonl_part(trace).ok_or_else(|| {
            Failure::config(format!(
                "{} holds no invocations/*/part-*.jsonl to convert",
                trace.display()
            ))
        })?
    } else {
        trace.to_path_buf()
    };
    let input = fs::File::open(&input_path)
        .map_err(|e| Failure::config(format!("cannot read {}: {e}", input_path.display())))?;
    let out = fs::File::create(output)
        .map_err(|e| Failure::other(format!("cannot create {}: {e}", output.display())))?;

    if matches!(to, ConvertTo::Cachesim | ConvertTo::OracleGeneral) {
        // Bytes per block, not tokens: a cache holds bytes, and `obj_size` is what the
        // simulator's capacity is measured against.
        let object_bytes = u32::try_from(block_bytes_of(trace)?).map_err(|_| {
            Failure::config(
                "the description's blocks.bytes exceeds oracleGeneral's 32-bit obj_size"
                    .to_string(),
            )
        })?;
        let input = std::io::BufReader::new(input);
        let sink = BufWriter::new(out);
        let (stats, kind) = if matches!(to, ConvertTo::Cachesim) {
            (
                workload_trace::cachesim::convert_jsonl_csv(input, sink, object_bytes),
                "csv",
            )
        } else {
            (
                workload_trace::cachesim::convert_jsonl_oracle(input, sink, object_bytes),
                "oracleGeneral",
            )
        };
        let stats = stats
            .map_err(|e| Failure::other(format!("converting {}: {e}", input_path.display())))?;
        let mut text = format!(
            "converted {} to {} as libCacheSim {kind}\n  \
             accesses {}  distinct objects {}  object size {} bytes\n",
            input_path.display(),
            output.display(),
            stats.accesses,
            stats.distinct_objects,
            stats.object_bytes,
        );
        if kind == "csv" {
            // The columns are configurable, so a CSV file cannot say what its own
            // columns mean. Printing the command is the only way that does not get lost.
            text.push_str(&format!(
                "  read it with: {}\n",
                stats.example_command(&output.display().to_string())
            ));
        }
        text.push_str(
            "  DROPPED: session identity, the input/output distinction, and virtual \
             time as anything but an integer clock. A projection is not a trace \
             (FR-075b, FR-077).\n",
        );
        return Ok(text);
    }

    if let ConvertTo::Mooncake = to {
        // Block size comes from the trace's own manifest, since the Mooncake format
        // carries no block-geometry field and a guess would be silently wrong.
        let block_size = block_size_of(trace)?;
        let stats = workload_trace::mooncake::convert_jsonl(
            std::io::BufReader::new(input),
            BufWriter::new(out),
            block_size,
        )
        .map_err(|e| Failure::other(format!("converting {}: {e}", input_path.display())))?;
        let mut text = format!(
            "converted {} to {} in the Mooncake format\n  \
             records {}  distinct identifiers {}  references {}\n",
            input_path.display(),
            output.display(),
            stats.records,
            stats.distinct_ids,
            stats.references,
        );
        for loss in stats.declared_losses() {
            text.push_str("  DROPPED: ");
            text.push_str(&loss);
            text.push('\n');
        }
        return Ok(text);
    }

    let stats = workload_trace::simulator::convert_jsonl(
        std::io::BufReader::new(input),
        BufWriter::new(out),
    )
    .map_err(|e| Failure::other(format!("converting {}: {e}", input_path.display())))?;

    Ok(format!(
        "converted {} to {} for the cache simulator\n           records {}  sessions {}  distinct keys {}  key references {}\n           dropped {} rows with no blocks (the simulator skips them)\n           DROPPED by this projection: virtual time, session identity as such, the \
         input/output distinction. A projection is not a trace and is not accepted \
         in place of one for a reproducibility check (FR-077, FR-075b).\n",
        input_path.display(),
        output.display(),
        stats.records,
        stats.sessions,
        stats.distinct_keys,
        stats.key_references,
        stats.dropped_empty,
    ))
}

/// Read `block_bytes` out of a trace's manifest.
fn block_bytes_of(trace: &Path) -> Result<u64, Failure> {
    manifest_field(trace, "block_bytes")
}

/// Read `block_size` out of a trace's manifest.
///
/// Not guessed and not defaulted: the Mooncake format carries no block geometry, so a
/// wrong value here produces a file whose lengths are silently wrong by a constant
/// factor — which nothing downstream would flag.
fn block_size_of(trace: &Path) -> Result<u64, Failure> {
    manifest_field(trace, "block_size")
}

/// Read one integer field from a trace's manifest.
fn manifest_field(trace: &Path, field: &str) -> Result<u64, Failure> {
    let path = if trace.is_dir() {
        trace.join("manifest.json")
    } else {
        trace
            .parent()
            .and_then(|p| p.parent())
            .and_then(|p| p.parent())
            .map(|p| p.join("manifest.json"))
            .unwrap_or_else(|| Path::new("manifest.json").to_path_buf())
    };
    let text = fs::read_to_string(&path).map_err(|e| {
        Failure::config(format!(
            "cannot read {} for {field}: {e}. Neither the Mooncake nor the libCacheSim \
             format carries block geometry, so it has to come from the trace's manifest",
            path.display()
        ))
    })?;
    let v: serde_json::Value = serde_json::from_str(&text)
        .map_err(|e| Failure::config(format!("{}: {e}", path.display())))?;
    v.get(field)
        .and_then(|b| b.as_u64())
        .ok_or_else(|| Failure::config(format!("{} has no {field}", path.display())))
}

/// The first `invocations/*/part-*.jsonl` under a trace directory.
fn find_jsonl_part(dir: &Path) -> Option<PathBuf> {
    for entry in fs::read_dir(dir.join("invocations")).ok()? {
        let sub = entry.ok()?.path();
        for f in fs::read_dir(sub).ok()? {
            let f = f.ok()?.path();
            if f.extension().is_some_and(|e| e == "jsonl") {
                return Some(f);
            }
        }
    }
    None
}

/// A failure with the exit code it should produce.
#[derive(Debug)]
pub struct Failure {
    message: String,
    code: i32,
}

impl Failure {
    /// A configuration refusal: exit 2, nothing was written.
    pub fn config(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
            code: exit::CONFIG,
        }
    }

    /// Anything else: exit 1.
    pub fn other(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
            code: exit::OTHER,
        }
    }

    /// The exit code.
    pub fn code(&self) -> i32 {
        self.code
    }
}

impl std::fmt::Display for Failure {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.message)
    }
}

impl std::error::Error for Failure {}

/// Load and validate a description, reporting its effective distributions.
fn load(path: &Path) -> Result<(WorkloadDescription, String, String), Failure> {
    let text = fs::read_to_string(path)
        .map_err(|e| Failure::config(format!("cannot read {}: {e}", path.display())))?;
    let description: WorkloadDescription = text
        .parse()
        .map_err(|e| Failure::config(format!("{}: {e}", path.display())))?;
    let report = description
        .validate()
        .map_err(|e| Failure::config(format!("{}: {e}", path.display())))?;
    if !report.refusals().is_empty() {
        let mut msg = format!("{}: configuration rejected\n", path.display());
        for r in report.refusals() {
            msg.push_str("  - ");
            msg.push_str(r);
            msg.push('\n');
        }
        return Err(Failure::config(msg));
    }
    Ok((description, text, report.render()))
}

/// Bytes free on the filesystem holding `dir`.
///
/// # Errors
///
/// If the directory cannot be created or `statvfs` fails. Both are refusals: a
/// projection with nothing to compare against is not a check that passed.
fn free_bytes(dir: &Path) -> Result<u64, Failure> {
    fs::create_dir_all(dir)
        .map_err(|e| Failure::other(format!("cannot create {}: {e}", dir.display())))?;
    let stat = fs::metadata(dir)
        .map_err(|e| Failure::other(format!("cannot stat {}: {e}", dir.display())))?;
    let _ = stat;
    // `statvfs` through the filesystem's own reporting. Read on the *output*
    // directory, since it is routinely a different mount from the working directory.
    let path = std::ffi::CString::new(dir.as_os_str().as_encoded_bytes())
        .map_err(|e| Failure::other(format!("bad output path: {e}")))?;
    // SAFETY: `path` is a valid NUL-terminated C string that outlives the call, and
    // `buf` is a correctly sized, writable `statvfs` for the kernel to fill. The
    // return value is checked before any field is read.
    let free = unsafe {
        let mut buf: libc::statvfs = std::mem::zeroed();
        if libc::statvfs(path.as_ptr(), &mut buf) != 0 {
            return Err(Failure::other(format!(
                "cannot determine free space on {}: {}",
                dir.display(),
                std::io::Error::last_os_error()
            )));
        }
        buf.f_bavail as u64 * buf.f_frsize as u64
    };
    Ok(free)
}

/// Refuse if the projection exceeds free space, or the ceiling without `--force`.
///
/// `free` is passed in rather than read here so both branches are testable: on a box
/// with less free space than the ceiling the free-space check always fires first, so
/// a test that only ran the real thing could never reach the ceiling branch.
fn check_size(projection: &Projection, dir: &Path, free: u64, force: bool) -> Result<(), Failure> {
    let need = projection.plan_bytes;
    if need > free {
        return Err(Failure::config(format!(
            "refusing to emit: the projection needs {need} bytes and {} has {free} \
             free. --force does not override this, because overriding it produces a \
             truncated trace and a full filesystem rather than a trace.\n{}",
            dir.display(),
            projection.render()
        )));
    }
    if need > SIZE_CEILING_BYTES && !force {
        return Err(Failure::config(format!(
            "refusing to emit: the projection needs {need} bytes, above the \
             documented ceiling of {SIZE_CEILING_BYTES}. Shorten --until, or pass \
             --force if this is deliberate.\n{}",
            projection.render()
        )));
    }
    Ok(())
}

/// The `emit` subcommand.
#[allow(clippy::too_many_arguments)]
fn emit(
    description_path: &Path,
    until: f64,
    output: &Path,
    format: Format,
    seed: u64,
    report_path: Option<PathBuf>,
    projections: Projections,
    force: bool,
) -> Result<String, Failure> {
    // NaN takes the is_finite branch, so it is refused rather than slipping past a
    // comparison that is false either way.
    if until <= 0.0 || !until.is_finite() {
        return Err(Failure::config(
            "--until must be a positive, finite number of virtual seconds".to_string(),
        ));
    }
    let (description, text, effective) = load(description_path)?;

    let projection = project(&description, until, seed)
        .map_err(|e| Failure::config(format!("cannot project the run: {e}")))?;
    check_size(&projection, output, free_bytes(output)?, force)?;

    if matches!(format, Format::Parquet | Format::Both) && !cfg!(feature = "parquet") {
        return Err(Failure::config(
            "this build has no parquet support; rebuild with --features parquet, or \
             pass --format jsonl"
                .to_string(),
        ));
    }

    let block_size = description.blocks.tokens;
    let trace_id = description_path
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("trace")
        .to_string();
    let dir = output.join(format!("invocations/block_size_{block_size}"));
    fs::create_dir_all(&dir)
        .map_err(|e| Failure::other(format!("cannot create {}: {e}", dir.display())))?;

    let mut sim = Simulation::new(&description, seed)
        .map_err(|e| Failure::config(format!("cannot start the simulation: {e}")))?;

    let started = Instant::now();
    let jsonl_path = dir.join("part-0.jsonl");
    let file = fs::File::create(&jsonl_path)
        .map_err(|e| Failure::other(format!("cannot create {}: {e}", jsonl_path.display())))?;
    let mut writer = JsonlWriter::new(BufWriter::new(file), &trace_id, block_size);

    // The simulator projection, written in the same pass rather than by converting the
    // trace afterwards (FR-075): a projection of a workload nobody wants stored should
    // not require storing it first.
    let mut mooncake_writer = match &projections.mooncake {
        Some(path) => {
            let f = fs::File::create(path)
                .map_err(|e| Failure::other(format!("cannot create {}: {e}", path.display())))?;
            Some(MooncakeWriter::new(BufWriter::new(f), block_size))
        }
        None => None,
    };
    let mut cachesim_writer = match &projections.cachesim {
        Some(path) => {
            let bytes = u32::try_from(description.blocks.bytes).map_err(|_| {
                Failure::config("blocks.bytes exceeds a 32-bit object size".to_string())
            })?;
            let f = fs::File::create(path)
                .map_err(|e| Failure::other(format!("cannot create {}: {e}", path.display())))?;
            Some(CsvWriter::new(BufWriter::new(f), bytes))
        }
        None => None,
    };
    let mut simulator_writer = match &projections.simulator {
        Some(path) => {
            let f = fs::File::create(path)
                .map_err(|e| Failure::other(format!("cannot create {}: {e}", path.display())))?;
            Some(SimulatorWriter::new(BufWriter::new(f)))
        }
        None => None,
    };

    let mut write_error = None;
    sim.run_until(until, &mut |s, t| {
        if write_error.is_none() {
            if let Err(e) = writer.write(s, t) {
                write_error = Some(e);
                return;
            }
            if mooncake_writer.is_some() || cachesim_writer.is_some() || simulator_writer.is_some()
            {
                let record = InvocationRecord::from_turn(&trace_id, s, t, block_size);
                if let Some(w) = mooncake_writer.as_mut() {
                    if let Err(e) = w.write_record(&record) {
                        write_error = Some(e);
                        return;
                    }
                }
                if let Some(w) = cachesim_writer.as_mut() {
                    if let Err(e) = w.write_record(&record) {
                        write_error = Some(e);
                        return;
                    }
                }
                if let Some(w) = simulator_writer.as_mut() {
                    if let Err(e) = w.write_record(&record) {
                        write_error = Some(e);
                    }
                }
            }
        }
    });
    if let Some(e) = write_error {
        return Err(Failure::other(format!(
            "writing {}: {e}",
            jsonl_path.display()
        )));
    }
    let mooncake_stats = match mooncake_writer {
        Some(w) => Some(
            w.finish()
                .map_err(|e| Failure::other(format!("closing the mooncake file: {e}")))?,
        ),
        None => None,
    };
    let cachesim_stats = match cachesim_writer {
        Some(w) => Some(
            w.finish()
                .map_err(|e| Failure::other(format!("closing the cachesim file: {e}")))?,
        ),
        None => None,
    };
    let simulator_stats = match simulator_writer {
        Some(w) => Some(
            w.finish()
                .map_err(|e| Failure::other(format!("closing the simulator file: {e}")))?,
        ),
        None => None,
    };
    let stats: BlockStats = writer
        .finish()
        .map_err(|e| Failure::other(format!("closing {}: {e}", jsonl_path.display())))?;
    let wallclock = started.elapsed().as_secs_f64();

    // The manifest goes last, so a directory without one is incomplete by
    // construction (FR-073). Nothing between here and the write may fail silently.
    let manifest = Manifest::new(&trace_id, &description, &text, seed, until, stats.clone());
    let manifest_path = output.join("manifest.json");
    fs::write(
        &manifest_path,
        manifest
            .to_json()
            .map_err(|e| Failure::other(format!("serialising the manifest: {e}")))?,
    )
    .map_err(|e| Failure::other(format!("writing {}: {e}", manifest_path.display())))?;

    // Across every session class, not class 0's: a two-class description would
    // otherwise under-report with nothing to show it had.
    let (sessions_started, sessions_completed) = sim.session_totals();
    let report = EmitReport {
        run_kind: "emit",
        sessions_started,
        sessions_completed,
        invocations: sim.turns_taken(),
        blocks_minted: sim.blocks_minted(),
        block_references: sim.blocks_read(),
        virtual_span: until,
        records: ContainerRecords {
            jsonl: Some(stats.invocations),
            parquet: None,
        },
        generation_rate_invocations_per_second: if wallclock > 0.0 {
            sim.turns_taken() as f64 / wallclock
        } else {
            f64::INFINITY
        },
        generation_wallclock_seconds: wallclock,
        reproduction: Reproduction {
            seed,
            until,
            description_digest: manifest.description_digest.clone(),
            description_path: description_path.display().to_string(),
        },
        projection: ProjectionSummary::from(&projection),
        warnings: projection.warnings.clone(),
    };

    let report_path = report_path.unwrap_or_else(|| output.join("report.json"));
    fs::write(
        &report_path,
        report
            .to_json()
            .map_err(|e| Failure::other(format!("serialising the report: {e}")))?,
    )
    .map_err(|e| Failure::other(format!("writing {}: {e}", report_path.display())))?;

    let mut out = effective;
    out.push_str(&report.render());
    if let (Some(path), Some(s)) = (&projections.mooncake, &mooncake_stats) {
        out.push_str(&format!(
            "  mooncake          {} records to {} ({} distinct identifiers)\n",
            s.records,
            path.display(),
            s.distinct_ids
        ));
    }
    if let (Some(path), Some(s)) = (&projections.cachesim, &cachesim_stats) {
        out.push_str(&format!(
            "  cachesim          {} accesses to {} ({} distinct objects)\n    \
             read it with: {}\n",
            s.accesses,
            path.display(),
            s.distinct_objects,
            s.example_command(&path.display().to_string())
        ));
    }
    if let (Some(path), Some(s)) = (&projections.simulator, &simulator_stats) {
        out.push_str(&format!(
            "  simulator         {} records to {} ({} sessions, {} distinct keys)\n",
            s.records,
            path.display(),
            s.sessions,
            s.distinct_keys
        ));
    }
    Ok(out)
}

/// The `validate` subcommand.
fn validate(
    description_path: &Path,
    until: Option<f64>,
    for_invocations: Option<u64>,
    seed: u64,
) -> Result<String, Failure> {
    let (description, _, effective) = load(description_path)?;
    let mut out = effective;
    if let Some(target) = for_invocations {
        let span = span_for_invocations(&description, target)
            .map_err(|e| Failure::config(format!("cannot invert the projection: {e}")))?;
        out.push_str(&format!(
            "about {target} invocations needs --until {span:.0}\n"
        ));
    }
    if let Some(span) = until {
        let p = project(&description, span, seed)
            .map_err(|e| Failure::config(format!("cannot project the run: {e}")))?;
        out.push_str(&p.render());
    }
    Ok(out)
}

/// The `plan` subcommand.
fn plan(description_path: &Path, until: f64, output: &Path, seed: u64) -> Result<String, Failure> {
    let (description, _, effective) = load(description_path)?;
    let mut sim = Simulation::new(&description, seed)
        .map_err(|e| Failure::config(format!("cannot start the simulation: {e}")))?;
    let mut plan = OperationPlan::default();
    sim.run_until(until, &mut |s, t| plan.record_turn(s, t));
    plan.check_ordered()
        .map_err(|e| Failure::other(format!("the plan is not ordered: {e}")))?;

    let file = fs::File::create(output)
        .map_err(|e| Failure::other(format!("cannot create {}: {e}", output.display())))?;
    let mut w = BufWriter::new(file);
    plan.write_canonical(&mut w)
        .and_then(|()| w.flush())
        .map_err(|e| Failure::other(format!("writing {}: {e}", output.display())))?;

    let mut out = effective;
    out.push_str(&format!(
        "wrote {} operations and {} key references to {} (fingerprint {:016x})\n",
        plan.len(),
        plan.key_references(),
        output.display(),
        plan.fingerprint()
    ));
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn projection(bytes: u64) -> Projection {
        Projection {
            span: 1_000.0,
            invocations: 1,
            operations: 1,
            keys_minted: 1,
            key_references: 1,
            plan_bytes: bytes,
            warnings: vec![],
        }
    }

    #[test]
    fn free_space_is_a_hard_refusal_that_force_does_not_override() {
        // Overriding it does not produce a trace — it produces a truncated directory
        // and a full filesystem, and on a shared box it does that to other people.
        let p = projection(1_000);
        for force in [false, true] {
            let err = check_size(&p, Path::new("/tmp"), 500, force).unwrap_err();
            assert_eq!(err.code(), exit::CONFIG);
            let msg = err.to_string();
            assert!(
                msg.contains("1000") && msg.contains("500"),
                "both figures must be named: {msg}"
            );
            assert!(msg.contains("--force does not override"));
        }
    }

    #[test]
    fn the_ceiling_refuses_without_force_and_yields_to_it() {
        // The guard rail: `--until` with an extra zero is a thing somebody does by
        // accident exactly once. Free space is set generously so this branch is the
        // one under test.
        let p = projection(SIZE_CEILING_BYTES + 1);
        let free = u64::MAX;
        let err = check_size(&p, Path::new("/tmp"), free, false).unwrap_err();
        assert_eq!(err.code(), exit::CONFIG);
        let msg = err.to_string();
        assert!(
            msg.contains("ceiling"),
            "the refusal should name the ceiling: {msg}"
        );
        assert!(
            msg.contains(&SIZE_CEILING_BYTES.to_string()),
            "the limit must be named"
        );
        assert!(msg.contains("--force"), "the way out must be named");

        check_size(&p, Path::new("/tmp"), free, true).expect("--force overrides the ceiling");
    }

    #[test]
    fn a_run_within_both_limits_is_permitted() {
        // A guard that always fires is not a guard.
        check_size(&projection(1_000), Path::new("/tmp"), u64::MAX, false).unwrap();
    }

    #[test]
    fn free_space_is_read_from_the_output_directorys_own_filesystem() {
        // Reading the working directory instead would report a number that looks
        // authoritative and describes a different mount.
        let free = free_bytes(Path::new("/tmp")).expect("statvfs on /tmp");
        assert!(free > 0, "no free space reported for /tmp");
    }
}
