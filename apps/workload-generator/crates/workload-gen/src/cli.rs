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
#[cfg(feature = "parquet")]
use workload_trace::parquet::ParquetWriter;
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
    /// Drive a live Certus node over its `/dev/shm` mailbox.
    ///
    /// Unbounded by default (FR-059); stops cleanly on SIGINT/SIGTERM, and an
    /// interrupted run whose plan queue never reached zero is **valid** (FR-074).
    #[cfg(feature = "live")]
    Run {
        /// The workload description.
        description: PathBuf,
        /// Optional virtual-second cap. Absent means run until interrupted.
        #[arg(long)]
        until: Option<f64>,
        /// The node's mailbox.
        #[arg(long, default_value = "/dev/shm/certus-shmq")]
        shm_path: String,
        /// Execution concurrency. Refused above the node's channel count, because the
        /// mailbox is depth-1 per channel and the extra lanes would serialise silently.
        #[arg(long, default_value_t = 4)]
        lanes: usize,
        /// Keys per request. MUST NOT change the plan (FR-072).
        #[arg(long, default_value_t = 64)]
        batch_keys: usize,
        /// Seed.
        #[arg(long)]
        seed: u64,
        /// GPU device for the payload buffer.
        ///
        /// `LOOKUP` and `COPY_TO_STORE` name GPU memory — a load DMAs into a device
        /// buffer and a store copies out of one — so without a device those two
        /// operations cannot be issued.
        #[arg(long, default_value_t = 0)]
        gpu_device: i32,
        /// Run the control path only, issuing no data-moving operations.
        ///
        /// For a node with no accelerator. The run is **partial** and its report says so,
        /// because a throughput from a stream missing its loads and stores is not
        /// comparable with a complete run's.
        #[arg(long)]
        no_payload: bool,
        /// Clear the memory tier once before the timed window opens (FR-046).
        ///
        /// Setup, not part of the operation stream, and never timed. It clears the memory
        /// tier only — disk-backed entries survive — so it does not guarantee a cold cache.
        #[arg(long)]
        clear_cache: bool,
        /// Check each loaded block against its key, and report mismatches.
        ///
        /// Implies `--stamp-keys`. This is what distinguishes "bytes arrived" from "the right
        /// bytes arrived": without it every block in the buffer is interchangeable, so a cache
        /// returning the wrong block would produce a run that looked correct. Costs a
        /// device-to-host copy per key, and wants a **cold** cache — a block stored by a run
        /// that did not stamp holds the fill byte.
        #[arg(long)]
        verify_payload: bool,
        /// Stamp each stored block with its key, for identity checking.
        ///
        /// Costs one host-to-device copy per key, which puts generator work on the
        /// per-key path (FR-070), so it is opt-in.
        #[arg(long)]
        stamp_keys: bool,
        /// A node to drive, repeatable. Absent means the local mailbox only.
        ///
        /// Each node runs an agent, and the generator reaches it over TCP: only keys cross the
        /// network (FR-047). Sessions are placed uniformly across the nodes given, and a
        /// session with a `migration_interval` moves between them (FR-048) — so the node list
        /// is a property of the deployment and deliberately not of the description.
        #[arg(long = "node")]
        nodes: Vec<String>,
        /// Port each node's agent listens on.
        #[arg(long, default_value_t = 7420)]
        agent_port: u16,
        /// Path to the agent binary **on each node**.
        #[arg(long, default_value = "workload-node-agent")]
        agent_binary: String,
        /// Use agents that are already running instead of launching them over ssh.
        ///
        /// For an operator managing the daemons themselves, and for a local node where ssh is
        /// unnecessary. It weakens FR-052 — a leftover of the current build is reused rather
        /// than replaced — which is why it is opt-in.
        #[arg(long)]
        no_launch: bool,
        /// Structured report destination.
        #[arg(long)]
        report: Option<PathBuf>,
    },
    /// Write a trace file. Contacts no server and needs no accelerator.
    Emit {
        /// The workload description.
        description: PathBuf,
        /// Virtual-second span. **Required**: an unbounded file is not a thing.
        #[arg(long)]
        until: f64,
        /// Seed. Required for reproducibility; there is no random default, because a
        /// run whose seed was never printed cannot be repeated.
        #[arg(long)]
        seed: u64,
        /// Native trace, JSONL container. **A directory**, not a file.
        ///
        /// The native format is a self-describing *directory* — `manifest.json` plus
        /// `invocations/block_size_<N>/part-0.jsonl` — so this names the directory.
        /// Point this and `--certus-unified-parquet` at the **same** directory to get
        /// one trace holding both containers, with one manifest and the two record
        /// counts checked against each other (SC-004). Different directories give two
        /// independent traces.
        #[arg(long = "certus-unified-jsonl")]
        unified_jsonl: Option<PathBuf>,
        /// Native trace, parquet container. **A directory**; see
        /// `--certus-unified-jsonl`. Requires the `parquet` feature.
        #[arg(long = "certus-unified-parquet")]
        unified_parquet: Option<PathBuf>,
        /// Mooncake projection. A single **file**.
        #[arg(long)]
        mooncake: Option<PathBuf>,
        /// libCacheSim CSV projection. A single **file**.
        #[arg(long = "libcachesim", alias = "cachesim")]
        cachesim: Option<PathBuf>,
        /// Cache-simulator projection, the shape `apps/eviction-replay-benchmark`
        /// reads. A single **file**.
        ///
        /// A projection is a file rather than a directory because it is not a trace
        /// (FR-075b): no manifest, and never accepted in place of the native trace for
        /// a reproducibility check.
        #[arg(long)]
        simulator: Option<PathBuf>,
        /// Structured report destination.
        ///
        /// Always rendered to the terminal. Written as `report.json` inside each native
        /// trace directory as well; this names an additional destination, and is the
        /// only way to keep the structured form of a projection-only run.
        #[arg(long)]
        report: Option<PathBuf>,
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
        #[cfg(feature = "live")]
        Command::Run {
            description,
            until,
            shm_path,
            lanes,
            batch_keys,
            seed,
            gpu_device,
            no_payload,
            clear_cache,
            stamp_keys,
            verify_payload,
            nodes,
            agent_port,
            agent_binary,
            no_launch,
            report,
        } => match live_run(
            &description,
            until,
            &shm_path,
            lanes,
            batch_keys,
            seed,
            (!no_payload).then_some(gpu_device),
            stamp_keys,
            verify_payload,
            clear_cache,
            &nodes,
            agent_port,
            &agent_binary,
            no_launch,
            report,
        ) {
            Ok((text, code)) => {
                print!("{text}");
                code
            }
            Err(e) => {
                eprintln!("{e}");
                e.code()
            }
        },
        Command::Emit {
            description,
            until,
            seed,
            unified_jsonl,
            unified_parquet,
            mooncake,
            cachesim,
            simulator,
            report,
            force,
        } => match emit(
            &description,
            until,
            seed,
            Outputs {
                unified_jsonl,
                unified_parquet,
                mooncake,
                cachesim,
                simulator,
            },
            report,
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

/// Where an emit run writes. **At least one** must be set.
///
/// Every output is named the same way — one flag, one destination — so nothing is
/// privileged and no run is obliged to produce a format it does not want. That was not
/// true while the native trace had `--output` and the projections had their own flags:
/// obtaining a Mooncake file then meant writing the native trace as well, at gigabytes
/// for a legal span, to get a file a fraction of the size.
///
/// The two native destinations are **directories** and the three projections are
/// **files**, which reflects a real difference rather than a convention: a native trace
/// is self-describing, so it is a directory holding a manifest beside its records,
/// while a projection has no manifest and is not a trace (FR-075b).
#[derive(Debug, Default)]
pub struct Outputs {
    /// Native trace directory, JSONL container.
    pub unified_jsonl: Option<PathBuf>,
    /// Native trace directory, parquet container.
    ///
    /// The same directory as `unified_jsonl` gives one trace with both containers and
    /// one manifest; a different one gives a second, independent trace.
    pub unified_parquet: Option<PathBuf>,
    /// Mooncake projection file.
    pub mooncake: Option<PathBuf>,
    /// libCacheSim CSV projection file.
    pub cachesim: Option<PathBuf>,
    /// Cache-simulator projection file.
    pub simulator: Option<PathBuf>,
}

impl Outputs {
    /// Whether nothing at all was asked for.
    fn is_empty(&self) -> bool {
        self.unified_jsonl.is_none()
            && self.unified_parquet.is_none()
            && self.mooncake.is_none()
            && self.cachesim.is_none()
            && self.simulator.is_none()
    }

    /// The distinct native trace directories, in flag order.
    ///
    /// One entry when both containers share a directory, which is the case that makes
    /// SC-004's equivalence claim about a single trace rather than about two.
    fn trace_dirs(&self) -> Vec<PathBuf> {
        let mut dirs: Vec<PathBuf> = Vec::new();
        for d in [self.unified_jsonl.as_ref(), self.unified_parquet.as_ref()]
            .into_iter()
            .flatten()
        {
            if !dirs.contains(d) {
                dirs.push(d.clone());
            }
        }
        dirs
    }
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

    /// A peer refused the run: exit 4.
    ///
    /// Distinct from a run that completed and was invalid (3), because the actions differ. An
    /// invalid run may be worth repeating; a refused peer means the deployment is wrong — a
    /// stale agent, a lane count the node cannot serve, a block size that disagrees with the
    /// description — and repeating it will fail identically.
    pub fn peer(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
            code: exit::PEER,
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

/// Bytes each output kind costs per key reference.
///
/// **Calibrated, not derived**: measured on a 30-virtual-second run of the shipped
/// example (1 962 304 key references), by dividing each file's size by that count.
/// The reference count is the right denominator because every one of these formats
/// is dominated by its block lists — a row's fixed fields are noise beside a prefix
/// of hundreds of keys.
///
/// | Output | Measured bytes | Per reference |
/// | --- | --- | --- |
/// | native JSONL | 44 122 546 | 22.5 |
/// | native parquet | 12 181 962 | 6.2 |
/// | Mooncake JSONL | 12 485 126 | 6.4 |
/// | libCacheSim CSV | 62 863 382 | 32.0 |
/// | simulator JSONL | 40 451 114 | 20.6 |
///
/// Rounded **up** in every case, because the check exists to refuse a run that would
/// fill a filesystem and an estimator that reads low fails at exactly the job it has.
/// Parquet's figure is the compressed size, so it is the one that can be beaten by an
/// incompressible workload; it is also the smallest, so being wrong about it costs
/// least. `oracleGeneral` is not here because it is exact — 24 bytes per reference,
/// fixed layout — and `convert` sizes nothing, since its input is already on disk.
mod bytes_per_reference {
    /// Our JSONL container: keys as decimal text, repeated across `full_*` and `new_*`.
    pub const JSONL: u64 = 23;
    /// Our parquet container, zstd-compressed.
    pub const PARQUET: u64 = 7;
    /// Mooncake: dense small integers, one prompt list per row.
    pub const MOONCAKE: u64 = 7;
    /// libCacheSim CSV: one whole row per reference, so the largest of all.
    pub const CACHESIM: u64 = 32;
    /// The simulator projection.
    pub const SIMULATOR: u64 = 21;
}

/// One requested output: what it is called, where it goes, and what it will cost.
#[derive(Debug, Clone)]
struct PlannedOutput {
    /// The flag that asked for it, for a refusal that names which one to drop.
    flag: &'static str,
    /// Its destination.
    path: PathBuf,
    /// Projected bytes.
    bytes: u64,
}

/// What each requested output will cost.
///
/// Only the outputs actually requested are counted (FR-073). Sizing everything the
/// tool *could* write would refuse runs that write one small file, which is now the
/// ordinary case rather than a corner.
fn planned_outputs(projection: &Projection, outputs: &Outputs) -> Vec<PlannedOutput> {
    let refs = projection.key_references;
    let mut planned = Vec::new();
    let mut add = |flag: &'static str, path: &Option<PathBuf>, per: u64| {
        if let Some(p) = path {
            planned.push(PlannedOutput {
                flag,
                path: p.clone(),
                bytes: refs.saturating_mul(per),
            });
        }
    };
    add(
        "--certus-unified-jsonl",
        &outputs.unified_jsonl,
        bytes_per_reference::JSONL,
    );
    add(
        "--certus-unified-parquet",
        &outputs.unified_parquet,
        bytes_per_reference::PARQUET,
    );
    add(
        "--mooncake",
        &outputs.mooncake,
        bytes_per_reference::MOONCAKE,
    );
    add(
        "--libcachesim",
        &outputs.cachesim,
        bytes_per_reference::CACHESIM,
    );
    add(
        "--simulator",
        &outputs.simulator,
        bytes_per_reference::SIMULATOR,
    );
    planned
}

/// Check every requested output against the free space on **its own** filesystem.
///
/// Outputs are grouped by device before checking, because five independent
/// destinations can be on five different mounts. Summing them all against one
/// filesystem would refuse a run that fits — and, worse, checking each alone against
/// its own would admit two large outputs that together overflow a mount they share.
/// Grouping is the only version that is right in both directions.
///
/// # Errors
///
/// [`Failure::config`] naming the flags on the offending filesystem and both figures.
fn check_sizes(
    projection: &Projection,
    planned: &[PlannedOutput],
    force: bool,
    free_of: &dyn Fn(&Path) -> Result<u64, Failure>,
) -> Result<u64, Failure> {
    use std::collections::BTreeMap;
    use std::os::unix::fs::MetadataExt;

    // Grouped by the device the destination lands on. `nearest_existing` because a
    // destination need not exist yet, and creating it to find out would leave a stray
    // directory behind on a refusal.
    let mut by_device: BTreeMap<u64, (PathBuf, Vec<&PlannedOutput>)> = BTreeMap::new();
    for p in planned {
        let dir = match p.flag {
            // A native destination is itself a directory; a projection is a file.
            "--certus-unified-jsonl" | "--certus-unified-parquet" => p.path.clone(),
            _ => p
                .path
                .parent()
                .filter(|d| !d.as_os_str().is_empty())
                .unwrap_or(Path::new("."))
                .to_path_buf(),
        };
        let probe = nearest_existing(&dir);
        let device = fs::metadata(&probe)
            .map_err(|e| Failure::other(format!("cannot stat {}: {e}", probe.display())))?
            .dev();
        by_device
            .entry(device)
            .or_insert_with(|| (probe, Vec::new()))
            .1
            .push(p);
    }

    let total: u64 = planned.iter().map(|p| p.bytes).sum();
    for (probe, group) in by_device.values() {
        let need: u64 = group.iter().map(|p| p.bytes).sum();
        // Injected rather than read here, so both refusal branches are testable
        // deterministically: on a box with less free space than the ceiling the
        // free-space check always fires first, and the ceiling branch would never run.
        let free = free_of(probe)?;
        let breakdown = || {
            group
                .iter()
                .map(|p| {
                    format!(
                        "  {:<26} {} bytes -> {}\n",
                        p.flag,
                        p.bytes,
                        p.path.display()
                    )
                })
                .collect::<String>()
        };
        if need > free {
            return Err(Failure::config(format!(
                "refusing to emit: the outputs on {}'s filesystem need about {need} \
                 bytes and it has {free} free. --force does not override this, because \
                 overriding it produces a truncated trace and a full filesystem rather \
                 than a trace.\n{}{}",
                probe.display(),
                breakdown(),
                projection.render()
            )));
        }
        if need > SIZE_CEILING_BYTES && !force {
            return Err(Failure::config(format!(
                "refusing to emit: the outputs on {}'s filesystem need about {need} \
                 bytes, above the documented ceiling of {SIZE_CEILING_BYTES}. Shorten \
                 --until, drop an output, or pass --force if this is \
                 deliberate.\n{}{}",
                probe.display(),
                breakdown(),
                projection.render()
            )));
        }
    }
    Ok(total)
}

/// Create a projection file, making its parent directory if needed.
///
/// A projection is a single file and its flag names that file, so `--mooncake
/// out/mc.jsonl` should work without a separate `mkdir` — unlike a native destination,
/// where the directory *is* the artifact.
///
/// # Errors
///
/// If the parent cannot be created or the file cannot be opened.
fn create_projection_file(path: &Path) -> Result<fs::File, Failure> {
    if let Some(parent) = path.parent() {
        if !parent.as_os_str().is_empty() {
            fs::create_dir_all(parent)
                .map_err(|e| Failure::other(format!("cannot create {}: {e}", parent.display())))?;
        }
    }
    fs::File::create(path)
        .map_err(|e| Failure::other(format!("cannot create {}: {e}", path.display())))
}

/// The nearest ancestor of `dir` that exists, for a `statvfs` that must not create
/// anything.
///
/// Falls back to `.`, which always exists. `statvfs` needs a path that is really
/// there, and creating the destination to obtain one would leave an empty directory
/// behind on a refusal.
fn nearest_existing(dir: &Path) -> PathBuf {
    let mut at = dir;
    loop {
        if at.exists() {
            return at.to_path_buf();
        }
        match at.parent() {
            Some(p) if !p.as_os_str().is_empty() => at = p,
            _ => return PathBuf::from("."),
        }
    }
}

/// The `emit` subcommand.
fn emit(
    description_path: &Path,
    until: f64,
    seed: u64,
    outputs: Outputs,
    report_path: Option<PathBuf>,
    force: bool,
) -> Result<String, Failure> {
    // NaN takes the is_finite branch, so it is refused rather than slipping past a
    // comparison that is false either way.
    if until <= 0.0 || !until.is_finite() {
        return Err(Failure::config(
            "--until must be a positive, finite number of virtual seconds".to_string(),
        ));
    }

    // At least one output, and none of the five is privileged. Asking for one
    // projection and nothing else is the ordinary case: a legal span costs gigabytes as
    // a native trace, and there is no reason to pay that to obtain a Mooncake file
    // (`contracts/cli.md`).
    if outputs.is_empty() {
        return Err(Failure::config(
            "nothing to write: pass at least one of --certus-unified-jsonl <dir>, \
             --certus-unified-parquet <dir>, --mooncake <file>, --libcachesim <file>, \
             --simulator <file>"
                .to_string(),
        ));
    }

    let (description, text, effective) = load(description_path)?;

    let projection = project(&description, until, seed)
        .map_err(|e| Failure::config(format!("cannot project the run: {e}")))?;

    if outputs.unified_parquet.is_some() && !cfg!(feature = "parquet") {
        return Err(Failure::config(
            "this build has no parquet support; rebuild with --features parquet, or \
             use --certus-unified-jsonl instead"
                .to_string(),
        ));
    }

    // Sized against exactly what will be written, each destination checked against the
    // free space on its own filesystem.
    let planned = planned_outputs(&projection, &outputs);
    check_sizes(&projection, &planned, force, &|dir| free_bytes(dir))?;

    let block_size = description.blocks.tokens;
    let trace_id = description_path
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("trace")
        .to_string();

    // Directories are created only for the native destinations actually asked for. A
    // projection-only run must leave no trace directory behind: an empty one with no
    // manifest is exactly the incomplete-looking thing FR-073 relies on being
    // meaningful.
    let records_dir = |root: &Path| -> Result<PathBuf, Failure> {
        let d = root.join(format!("invocations/block_size_{block_size}"));
        fs::create_dir_all(&d)
            .map_err(|e| Failure::other(format!("cannot create {}: {e}", d.display())))?;
        Ok(d)
    };

    let mut sim = Simulation::new(&description, seed)
        .map_err(|e| Failure::config(format!("cannot start the simulation: {e}")))?;

    let started = Instant::now();

    // Two writers over one pass rather than a conversion afterwards: a conversion would
    // prove the converter right and say nothing about the writers, and SC-004's claim
    // is about the writers.
    let jsonl_path = match &outputs.unified_jsonl {
        Some(root) => records_dir(root)?.join("part-0.jsonl"),
        None => PathBuf::new(),
    };
    let mut writer = match &outputs.unified_jsonl {
        Some(_) => {
            let file = fs::File::create(&jsonl_path).map_err(|e| {
                Failure::other(format!("cannot create {}: {e}", jsonl_path.display()))
            })?;
            Some(JsonlWriter::new(
                BufWriter::new(file),
                &trace_id,
                block_size,
            ))
        }
        None => None,
    };

    #[cfg(feature = "parquet")]
    let parquet_path = match &outputs.unified_parquet {
        Some(root) => records_dir(root)?.join("part-0.parquet"),
        None => PathBuf::new(),
    };
    #[cfg(feature = "parquet")]
    let mut parquet_writer = match &outputs.unified_parquet {
        Some(_) => {
            let file = fs::File::create(&parquet_path).map_err(|e| {
                Failure::other(format!("cannot create {}: {e}", parquet_path.display()))
            })?;
            Some(
                ParquetWriter::new(BufWriter::new(file), &trace_id, block_size).map_err(|e| {
                    Failure::other(format!("cannot start {}: {e}", parquet_path.display()))
                })?,
            )
        }
        None => None,
    };

    // The projections, written in the same pass rather than by converting the trace
    // afterwards (FR-075): a projection of a workload nobody wants stored should not
    // require storing it first.
    let mut mooncake_writer = match &outputs.mooncake {
        Some(path) => {
            let f = create_projection_file(path)?;
            Some(MooncakeWriter::new(BufWriter::new(f), block_size))
        }
        None => None,
    };
    let mut cachesim_writer = match &outputs.cachesim {
        Some(path) => {
            let bytes = u32::try_from(description.blocks.bytes).map_err(|_| {
                Failure::config("blocks.bytes exceeds a 32-bit object size".to_string())
            })?;
            Some(CsvWriter::new(
                BufWriter::new(create_projection_file(path)?),
                bytes,
            ))
        }
        None => None,
    };
    let mut simulator_writer = match &outputs.simulator {
        Some(path) => Some(SimulatorWriter::new(BufWriter::new(
            create_projection_file(path)?,
        ))),
        None => None,
    };

    // A message rather than an `io::Error`, because the parquet writer's error type is
    // its own and every one of these failures wants naming its file anyway.
    let mut write_error: Option<String> = None;
    sim.run_until(until, &mut |s, t| {
        if write_error.is_none() {
            if let Some(w) = writer.as_mut() {
                if let Err(e) = w.write(s, t) {
                    write_error = Some(format!("writing {}: {e}", jsonl_path.display()));
                    return;
                }
            }
            #[cfg(feature = "parquet")]
            if let Some(w) = parquet_writer.as_mut() {
                if let Err(e) = w.write(s, t) {
                    write_error = Some(format!("writing {}: {e}", parquet_path.display()));
                    return;
                }
            }
            if mooncake_writer.is_some() || cachesim_writer.is_some() || simulator_writer.is_some()
            {
                let record = InvocationRecord::from_turn(&trace_id, s, t, block_size);
                if let Some(w) = mooncake_writer.as_mut() {
                    if let Err(e) = w.write_record(&record) {
                        write_error = Some(format!("writing the mooncake projection: {e}"));
                        return;
                    }
                }
                if let Some(w) = cachesim_writer.as_mut() {
                    if let Err(e) = w.write_record(&record) {
                        write_error = Some(format!("writing the cachesim projection: {e}"));
                        return;
                    }
                }
                if let Some(w) = simulator_writer.as_mut() {
                    if let Err(e) = w.write_record(&record) {
                        write_error = Some(format!("writing the simulator projection: {e}"));
                    }
                }
            }
        }
    });
    if let Some(e) = write_error {
        return Err(Failure::other(e));
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
    let jsonl_stats: Option<BlockStats> = match writer {
        Some(w) => Some(
            w.finish()
                .map_err(|e| Failure::other(format!("closing {}: {e}", jsonl_path.display())))?,
        ),
        None => None,
    };

    #[cfg(feature = "parquet")]
    let parquet_stats: Option<BlockStats> = match parquet_writer {
        Some(w) => Some(
            w.finish()
                .map_err(|e| Failure::other(format!("closing {}: {e}", parquet_path.display())))?,
        ),
        None => None,
    };
    #[cfg(not(feature = "parquet"))]
    let parquet_stats: Option<BlockStats> = None;

    // With both containers written, their counts must agree — that is SC-004's
    // equivalence claim, checked on every real run rather than only in the test that
    // compares a handful of records. A disagreement here means one writer dropped or
    // duplicated a row, which is exactly the failure that would otherwise be found by
    // whoever later compared the two files.
    if let (Some(j), Some(p)) = (jsonl_stats.as_ref(), parquet_stats.as_ref()) {
        if j != p {
            return Err(Failure::other(format!(
                "the two containers disagree: jsonl wrote {j:?} and parquet wrote {p:?}. \
                 One of them dropped or duplicated a row (SC-004)"
            )));
        }
    }
    // Either container's counts describe the run; they are equal when both were
    // written, and there are none on a projection-only run.
    let stats: BlockStats = jsonl_stats
        .clone()
        .or_else(|| parquet_stats.clone())
        .unwrap_or_default();
    let wallclock = started.elapsed().as_secs_f64();

    // Built either way, because the report's reproduction block needs the description
    // digest — but **written** only into native trace directories. A projection carries
    // no manifest by design (FR-075b), and writing one beside a projection would make it
    // look like a trace.
    //
    // One per distinct directory: pointing both container flags at the same directory
    // gives one trace with two containers and therefore one manifest, while separate
    // directories are two independent traces and each needs its own.
    //
    // It goes last, so a directory without one is incomplete by construction (FR-073).
    // Nothing between here and the write may fail silently.
    let manifest = Manifest::new(&trace_id, &description, &text, seed, until, stats.clone());
    let manifest_json = manifest
        .to_json()
        .map_err(|e| Failure::other(format!("serialising the manifest: {e}")))?;
    for root in outputs.trace_dirs() {
        let manifest_path = root.join("manifest.json");
        fs::write(&manifest_path, &manifest_json)
            .map_err(|e| Failure::other(format!("writing {}: {e}", manifest_path.display())))?;
    }

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
            jsonl: jsonl_stats.as_ref().map(|s| s.invocations),
            parquet: parquet_stats.as_ref().map(|s| s.invocations),
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

    // `report.json` goes into every native trace directory, and to `--report` if given.
    // A projection-only run has nowhere it obviously belongs, so there `--report` is the
    // only way to keep the structured form — it is rendered to the terminal either way,
    // so nothing is lost silently.
    let report_json = report
        .to_json()
        .map_err(|e| Failure::other(format!("serialising the report: {e}")))?;
    let mut report_paths: Vec<PathBuf> = outputs
        .trace_dirs()
        .into_iter()
        .map(|d| d.join("report.json"))
        .collect();
    if let Some(explicit) = report_path {
        if !report_paths.contains(&explicit) {
            report_paths.push(explicit);
        }
    }
    for path in report_paths {
        if let Some(parent) = path.parent() {
            if !parent.as_os_str().is_empty() {
                fs::create_dir_all(parent).map_err(|e| {
                    Failure::other(format!("cannot create {}: {e}", parent.display()))
                })?;
            }
        }
        fs::write(&path, &report_json)
            .map_err(|e| Failure::other(format!("writing {}: {e}", path.display())))?;
    }

    let mut out = effective;
    out.push_str(&report.render());
    if let (Some(path), Some(s)) = (&outputs.mooncake, &mooncake_stats) {
        out.push_str(&format!(
            "  mooncake          {} records to {} ({} distinct identifiers)\n",
            s.records,
            path.display(),
            s.distinct_ids
        ));
    }
    if let (Some(path), Some(s)) = (&outputs.cachesim, &cachesim_stats) {
        out.push_str(&format!(
            "  cachesim          {} accesses to {} ({} distinct objects)\n    \
             read it with: {}\n",
            s.accesses,
            path.display(),
            s.distinct_objects,
            s.example_command(&path.display().to_string())
        ));
    }
    if let (Some(path), Some(s)) = (&outputs.simulator, &simulator_stats) {
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

/// The `run` subcommand: drive a live node.
///
/// Returns the report text and the exit code, so validity travels in the process status
/// and not only in the report (FR-062). A sweep driver that treats "the process exited"
/// as "I have a data point" is exactly how an invalid run gets published.
///
/// `--until` is **optional**: absent means an unbounded run (FR-059), which works because
/// the producer is throttled by the lanes' queues rather than by memory. An earlier cut
/// pre-built the whole plan and silently substituted a 60-second span, which ran a
/// different experiment than the one asked for.
#[cfg(feature = "live")]
#[allow(clippy::too_many_arguments)]
fn live_run(
    description_path: &Path,
    until: Option<f64>,
    shm_path: &str,
    lanes: usize,
    batch_keys: usize,
    seed: u64,
    gpu_device: Option<i32>,
    stamp_keys: bool,
    verify_payload: bool,
    clear_cache: bool,
    nodes: &[String],
    agent_port: u16,
    agent_binary: &str,
    no_launch: bool,
    report_path: Option<PathBuf>,
) -> Result<(String, i32), Failure> {
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::Arc;

    // Remote mode starts and verifies the agents here — startup is outside the timed window
    // (FR-050) — and the turn-routing driver that follows is T073a. Refusing plainly is the
    // honest state: a `--node` that connected and then drove nothing would report a run that
    // never happened.
    // Loaded and the handler installed before anything is claimed. A remote run needs the
    // description and must NOT need a local mailbox: FR-079's point is that the generator can
    // drive a cluster from a host running no Certus, and calling `attach` first quietly required
    // one.
    let (description, text, effective) = load(description_path)?;

    // SIGINT/SIGTERM end the run cleanly; an interrupted run is not a failed one (FR-074),
    // so the handler asks rather than aborts.
    install_stop_handler().map_err(Failure::other)?;
    let stop = Arc::new(AtomicBool::new(false));
    let watcher = {
        let stop = Arc::clone(&stop);
        std::thread::spawn(move || {
            while !stop.load(Ordering::Relaxed) {
                if STOP.load(Ordering::Relaxed) {
                    stop.store(true, Ordering::Relaxed);
                    return;
                }
                std::thread::sleep(std::time::Duration::from_millis(20));
            }
        })
    };

    // Remote mode. Agents are started and verified before the clock starts (FR-050), driven
    // over TCP by the one driver `remote::run`, then stopped with their teardown checked.
    if !nodes.is_empty() {
        let block_bytes = u32::try_from(description.blocks.bytes)
            .map_err(|_| Failure::config("blocks.bytes exceeds a 32-bit reservation"))?;
        let specs: Vec<crate::agents::AgentSpec> = nodes
            .iter()
            .map(|node| crate::agents::AgentSpec {
                node: node.clone(),
                port: agent_port,
                shm_path: shm_path.to_string(),
                binary: agent_binary.to_string(),
                lanes,
                block_bytes,
                batch_keys,
                extra_args: Vec::new(),
            })
            .collect();
        // A peer refused for provenance or capacity is exit 4: the deployment is wrong, and
        // rerunning it will fail identically.
        let depth = workload_wire::client::DEFAULT_DEPTH;
        let mut agents = if no_launch {
            crate::agents::Agents::start_with(&crate::agents::NoLaunch, &specs, depth, false)
        } else {
            crate::agents::Agents::start_with(
                &crate::agents::SshLauncher::default(),
                &specs,
                depth,
                true,
            )
        }
        .map_err(Failure::peer)?;
        let options = crate::live::RunOptions {
            seed,
            until,
            batch_keys,
            gpu_device,
            stamp_keys,
            verify_payload,
            clear_cache,
        };
        // Captured before the agents are stopped: the report names the capacity the nodes
        // actually reported, and printing 0 there would be a wrong number rather than a missing
        // one.
        let node_channels: usize = agents
            .agents()
            .iter()
            .map(|a| a.ack.channels as usize)
            .sum();
        let driven = crate::remote::run(&mut agents, &description, &options, Arc::clone(&stop));
        stop.store(true, Ordering::Relaxed);
        let _ = watcher.join();
        // Stopped whatever happened, and its teardown checked: a run that failed must still
        // leave nothing holding mailbox channels (FR-053).
        if let Err(e) = agents.stop() {
            eprintln!("warning: agent teardown: {e}");
        }
        let out = match driven {
            Ok(o) => o,
            // A lost node is exit 3, not 4: the run began and was abandoned, rather than the
            // deployment being refused before it started.
            Err(lost) => {
                return Err(Failure {
                    message: format!("RUN INVALID \u{2014} {lost}"),
                    code: exit::INVALID,
                })
            }
        };
        let mut rendered = effective;
        rendered.push_str(&live_report(
            &out.stats,
            None,
            node_channels,
            lanes,
            batch_keys,
            seed,
            description_path,
            report_path,
        )?);
        rendered.push_str(&format!("  nodes             {}\n", nodes.len()));
        for (node, c) in &out.per_node {
            rendered.push_str(&format!(
                "    {node:<20} {:>8} requests, {:>7} blocks read, {:>7} written\n",
                c.requests,
                c.blocks_read(),
                c.blocks_written()
            ));
        }
        let _ = text;
        let code = if out.stats.is_valid() {
            exit::OK
        } else {
            exit::INVALID
        };
        return Ok((rendered, code));
    }

    let (client, channels) = crate::live::attach(shm_path, lanes).map_err(Failure::config)?;
    let node_channels = client.channel_count();

    let stats = crate::live::run(
        client,
        channels,
        &description,
        &crate::live::RunOptions {
            seed,
            until,
            batch_keys,
            gpu_device,
            stamp_keys,
            verify_payload,
            clear_cache,
        },
        Arc::clone(&stop),
    )
    .map_err(Failure::other)?;
    stop.store(true, Ordering::Relaxed);
    let _ = watcher.join();

    let text_out = live_report(
        &stats,
        None,
        node_channels,
        lanes,
        batch_keys,
        seed,
        description_path,
        report_path,
    )?;
    let mut out = effective;
    out.push_str(&text_out);
    Ok((
        out,
        if stats.is_valid() {
            exit::OK
        } else {
            exit::INVALID
        },
    ))
}

/// Build a live run's report.
///
/// Shared by the local and the remote paths, so one cannot report something the other would
/// not. Under FR-079 the two collapse into a single caller; until then this is where they are
/// held together.
///
/// # Errors
///
/// If the structured report cannot be written.
#[cfg(feature = "live")]
#[allow(clippy::too_many_arguments)]
fn live_report(
    stats: &crate::live::LiveStats,
    lost_node: Option<String>,
    node_channels: usize,
    lanes: usize,
    batch_keys: usize,
    seed: u64,
    description_path: &Path,
    report_path: Option<PathBuf>,
) -> Result<String, Failure> {
    use crate::report::{LatencyPercentiles, LiveReport, QueueStats, Tuning};

    let valid = stats.is_valid();
    let report = LiveReport {
        run_kind: "live",
        valid,
        invalid_reason: (!valid).then(|| {
            format!(
                "the plan queue underran {} times out of {} pops ({:.3}%), so the \
                 generator and not Certus set the pace at those instants and the \
                 throughput describes the instrument (FR-062)",
                stats.underruns(),
                stats.pops(),
                stats.fraction_underrun() * 100.0
            )
        }),
        requests: stats.requests(),
        key_references: stats.key_references(),
        keys_per_second: stats.keys_per_second(),
        bytes_per_second: stats.bytes_per_second(),
        virtual_to_wallclock: stats.virtual_to_wallclock(),
        elapsed_seconds: stats.elapsed,
        virtual_span: stats.virtual_span,
        queue: QueueStats {
            batches_produced: stats.batches_produced,
            pops: stats.pops(),
            underruns: stats.underruns(),
            fraction_underrun: stats.fraction_underrun(),
            min_depth: stats.min_depth(),
            capacity_per_lane: crate::live::QUEUE_CAPACITY,
            per_lane_underruns: stats.lanes.iter().map(|l| l.underruns).collect(),
            producer_blocked: stats.producer_blocked,
            per_lane_min_depth: stats.lanes.iter().map(|l| l.min_depth).collect(),
        },
        bandwidth: crate::report::Bandwidth {
            blocks_read: stats.blocks_read(),
            blocks_written: stats.blocks_written(),
            read_bytes: stats.read_bytes(),
            write_bytes: stats.write_bytes(),
        },
        latency_by_op: stats
            .latency_by_op
            .iter()
            .map(|(opcode, h)| crate::report::OpLatency {
                op: crate::report::opcode_name(*opcode),
                requests: h.len(),
                us: LatencyPercentiles {
                    p50: h.value_at_quantile(0.50),
                    p90: h.value_at_quantile(0.90),
                    p99: h.value_at_quantile(0.99),
                    max: h.max(),
                },
            })
            .collect(),
        outcomes: crate::report::CacheOutcomes {
            check_resident: stats.check_resident(),
            check_pending: stats.check_pending(),
            check_miss: stats.check_miss(),
            lookup_hits: stats.lookup_hits(),
            lookup_misses: stats.lookup_misses(),
            reserves_attempted: stats.reserves_attempted(),
            commits_attempted: stats.commits_attempted(),
            payload_mismatches: stats.payload_mismatches(),
            payloads_verified: stats.payloads_verified(),
            reserves_declined: stats.reserves_declined(),
            transfers_declined: stats.transfers_declined(),
            commits_declined: stats.commits_declined(),
        },
        cleared_entries: stats.cleared_entries,
        // `None` locally; the remote driver passes the node it lost (FR-064).
        lost_node,
        // Filled by the caller when a sweep asked for them: a distinct-key count costs an insert
        // per key reference on the producer's path, and the working set comes from the
        // simulation rather than from the run.
        distinct_keys: None,
        working_set: Vec::new(),
        producer_completed: stats.producer_completed,
        lanes: stats.lanes.len(),
        node_channels,
        latency_us: LatencyPercentiles {
            p50: stats.latency.value_at_quantile(0.50),
            p90: stats.latency.value_at_quantile(0.90),
            p99: stats.latency.value_at_quantile(0.99),
            max: stats.latency.max(),
        },
        reproduction: Reproduction {
            seed,
            // The span actually covered, which for an unbounded run is the only answer there is.
            until: stats.virtual_span,
            description_digest: digest_of(
                &std::fs::read_to_string(description_path).unwrap_or_default(),
            ),
            description_path: description_path.display().to_string(),
        },
        tuning: Tuning { batch_keys, lanes },
        skipped_needing_gpu: stats.skipped_needing_gpu(),
        skipped_keys: stats.skipped_keys(),
    };

    if let Some(path) = report_path {
        fs::write(
            &path,
            report
                .to_json()
                .map_err(|e| Failure::other(format!("serialising the report: {e}")))?,
        )
        .map_err(|e| Failure::other(format!("writing {}: {e}", path.display())))?;
    }

    let out = report.render();
    Ok(out)
}

/// Set by the signal handler, polled by the drive loop.
///
/// A plain `static` rather than a closure or a `OnceLock`: a signal handler must be
/// async-signal-safe, and an atomic store to a static is the whole of what that allows.
/// An earlier attempt threaded a generic closure through and needed `unsafe impl Sync`
/// to compile, which is a sign the design was wrong rather than a thing to justify.
#[cfg(feature = "live")]
static STOP: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// Handle SIGINT and SIGTERM by asking the run to stop.
#[cfg(feature = "live")]
extern "C" fn on_stop_signal(_sig: libc::c_int) {
    STOP.store(true, std::sync::atomic::Ordering::Relaxed);
}

/// Install the stop handler for SIGINT and SIGTERM.
///
/// # Errors
///
/// If `signal` refuses, which would leave an interrupt killing the run outright and
/// losing its report — worth reporting rather than ignoring.
#[cfg(feature = "live")]
fn install_stop_handler() -> Result<(), String> {
    for sig in [libc::SIGINT, libc::SIGTERM] {
        // SAFETY: `on_stop_signal` is an `extern "C"` fn of the right signature whose
        // body is a single relaxed atomic store — async-signal-safe, no allocation, no
        // locking, no I/O.
        let previous =
            unsafe { libc::signal(sig, on_stop_signal as *const () as libc::sighandler_t) };
        if previous == libc::SIG_ERR {
            return Err(format!("cannot install a handler for signal {sig}"));
        }
    }
    Ok(())
}

/// Non-cryptographic digest of a description, matching the trace manifest's.
///
/// Live-only: the emit path takes its digest from the manifest it is already building.
#[cfg(feature = "live")]
fn digest_of(text: &str) -> String {
    let mut acc = 0x9e37_79b9_7f4a_7c15u64;
    for b in text.as_bytes() {
        acc = workload_model::keys::splitmix64(acc ^ *b as u64);
    }
    format!("{acc:016x}")
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

    /// One planned output of a given size, so a size check can be exercised without a
    /// description or a run.
    fn planned(bytes: u64) -> Vec<PlannedOutput> {
        vec![PlannedOutput {
            flag: "--certus-unified-jsonl",
            path: PathBuf::from("/tmp"),
            bytes,
        }]
    }

    /// A stub filesystem reporting a fixed amount free, whatever it is asked about.
    fn free_stub(free: u64) -> impl Fn(&Path) -> Result<u64, Failure> {
        move |_| Ok(free)
    }

    #[test]
    fn free_space_is_a_hard_refusal_that_force_does_not_override() {
        // Overriding it does not produce a trace — it produces a truncated directory
        // and a full filesystem, and on a shared box it does that to other people.
        let p = projection(1_000);
        for force in [false, true] {
            let err = check_sizes(&p, &planned(1_000), force, &free_stub(500)).unwrap_err();
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
        let big = planned(SIZE_CEILING_BYTES + 1);
        let err = check_sizes(&p, &big, false, &free_stub(u64::MAX)).unwrap_err();
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

        check_sizes(&p, &big, true, &free_stub(u64::MAX)).expect("--force overrides the ceiling");
    }

    #[test]
    fn a_run_within_both_limits_is_permitted() {
        // A guard that always fires is not a guard.
        check_sizes(
            &projection(1_000),
            &planned(1_000),
            false,
            &free_stub(u64::MAX),
        )
        .unwrap();
    }

    #[test]
    fn a_refusal_names_the_flag_that_is_expensive() {
        // "needs 800 GB" is not actionable; "--libcachesim needs 780 GB of it" says
        // which flag to drop.
        let p = projection(1_000);
        let outputs = vec![
            PlannedOutput {
                flag: "--mooncake",
                path: PathBuf::from("/tmp/mc.jsonl"),
                bytes: 10,
            },
            PlannedOutput {
                flag: "--libcachesim",
                path: PathBuf::from("/tmp/lcs.csv"),
                bytes: 10_000,
            },
        ];
        let msg = check_sizes(&p, &outputs, false, &free_stub(500))
            .unwrap_err()
            .to_string();
        assert!(msg.contains("--libcachesim"), "{msg}");
        assert!(msg.contains("10000"), "{msg}");
        assert!(msg.contains("/tmp/lcs.csv"), "{msg}");
    }

    #[test]
    fn outputs_on_one_filesystem_are_summed_rather_than_checked_alone() {
        // The failure this prevents: two outputs that each fit, and together do not.
        // Checking them separately admits the run and fills the mount.
        let p = projection(1_000);
        let outputs = vec![
            PlannedOutput {
                flag: "--mooncake",
                path: PathBuf::from("/tmp/mc.jsonl"),
                bytes: 400,
            },
            PlannedOutput {
                flag: "--libcachesim",
                path: PathBuf::from("/tmp/lcs.csv"),
                bytes: 400,
            },
        ];
        // Both are under 500 alone; 800 together is not.
        assert!(check_sizes(&p, &outputs, false, &free_stub(500)).is_err());
        assert!(check_sizes(&p, &outputs, false, &free_stub(1_000)).is_ok());
    }

    #[test]
    fn only_the_outputs_requested_are_sized() {
        // FR-073. Sizing everything the tool could write would refuse a run that writes
        // one small file, which is now the ordinary case.
        let p = Projection {
            key_references: 1_000,
            ..projection(0)
        };
        let just_mooncake = planned_outputs(
            &p,
            &Outputs {
                mooncake: Some(PathBuf::from("/tmp/mc.jsonl")),
                ..Default::default()
            },
        );
        assert_eq!(just_mooncake.len(), 1);
        assert_eq!(just_mooncake[0].flag, "--mooncake");
        assert_eq!(
            just_mooncake[0].bytes,
            1_000 * bytes_per_reference::MOONCAKE
        );

        // And libCacheSim CSV is the expensive one, which is worth knowing before a
        // sweep: it writes a row per reference rather than per request.
        let just_cachesim = planned_outputs(
            &p,
            &Outputs {
                cachesim: Some(PathBuf::from("/tmp/lcs.csv")),
                ..Default::default()
            },
        );
        assert!(just_cachesim[0].bytes > just_mooncake[0].bytes * 4);
    }

    #[test]
    fn both_native_flags_on_one_directory_are_one_trace() {
        let same = Outputs {
            unified_jsonl: Some(PathBuf::from("/tmp/t")),
            unified_parquet: Some(PathBuf::from("/tmp/t")),
            ..Default::default()
        };
        assert_eq!(same.trace_dirs().len(), 1, "one directory is one trace");

        let apart = Outputs {
            unified_jsonl: Some(PathBuf::from("/tmp/a")),
            unified_parquet: Some(PathBuf::from("/tmp/b")),
            ..Default::default()
        };
        assert_eq!(
            apart.trace_dirs().len(),
            2,
            "two directories are two traces"
        );

        // A projection is not a trace, so it contributes no trace directory and
        // therefore no manifest (FR-075b).
        let projection_only = Outputs {
            mooncake: Some(PathBuf::from("/tmp/mc.jsonl")),
            ..Default::default()
        };
        assert!(projection_only.trace_dirs().is_empty());
        assert!(!projection_only.is_empty(), "it does have an output");
        assert!(Outputs::default().is_empty());
    }

    #[test]
    fn free_space_is_read_from_the_output_directorys_own_filesystem() {
        // Reading the working directory instead would report a number that looks
        // authoritative and describes a different mount.
        let free = free_bytes(Path::new("/tmp")).expect("statvfs on /tmp");
        assert!(free > 0, "no free space reported for /tmp");
    }
}
