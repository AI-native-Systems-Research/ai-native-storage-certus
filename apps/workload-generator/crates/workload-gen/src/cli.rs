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

use clap::{Parser, Subcommand};
use workload_model::description::WorkloadDescription;
use workload_model::plan::OperationPlan;
use workload_model::project::{project, span_for_invocations, Projection};
use workload_model::sim::Simulation;
use workload_trace::cachesim::CsvWriter;
use workload_trace::mooncake::MooncakeWriter;
use workload_trace::qwen::QwenWriter;
use workload_trace::record::InvocationRecord;

use crate::report::{EmitReport, ProjectionSummary, Reproduction};

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

/// Everything `run` takes.
///
/// A struct rather than fifteen enum fields threaded through a fifteen-argument call, which is
/// how a `--stamp-keys` ended up where a `--verify-payload` was meant more than once.
#[cfg(feature = "live")]
#[derive(Debug, clap::Args)]
pub struct RunArgs {
    /// The workload description.
    pub description: PathBuf,
    /// Optional virtual-second cap. Absent means run until interrupted.
    #[arg(long)]
    pub until: Option<f64>,
    /// Execution concurrency, per instance: one agent connection and one mailbox channel each.
    ///
    /// Refused above the node's channel count, because the mailbox is depth-1 per channel and
    /// the extra lanes would serialise silently.
    #[arg(long, default_value_t = 4)]
    pub lanes: usize,
    /// Keys per request. MUST NOT change the plan (FR-072).
    #[arg(long, default_value_t = 64)]
    pub batch_keys: usize,
    /// Seed.
    #[arg(long)]
    pub seed: u64,
    /// GPU device for each agent's payload buffer.
    ///
    /// `LOOKUP` and `COPY_TO_STORE` name GPU memory — a load DMAs into a device buffer and a
    /// store copies out of one — so without a device those two operations cannot be issued.
    #[arg(long, default_value_t = 0)]
    pub gpu_device: i32,
    /// Run the control path only, issuing no data-moving operations.
    ///
    /// For a node with no accelerator. The run is **partial** and its report says so,
    /// because a throughput from a stream missing its loads and stores is not
    /// comparable with a complete run's.
    #[arg(long)]
    pub no_payload: bool,
    /// Clear every node's memory tier once before the timed window opens (FR-046).
    ///
    /// Setup, not part of the operation stream, and never timed. It clears the memory
    /// tier only — disk-backed entries survive — so it does not guarantee a cold cache.
    #[arg(long)]
    pub clear_cache: bool,
    /// Check each loaded block against its key, and report mismatches.
    ///
    /// Implies `--stamp-keys`. This is what distinguishes "bytes arrived" from "the right
    /// bytes arrived": without it every block in the buffer is interchangeable, so a cache
    /// returning the wrong block would produce a run that looked correct. Costs a
    /// device-to-host copy per key, and wants a **cold** cache — a block stored by a run
    /// that did not stamp holds the fill byte.
    #[arg(long)]
    pub verify_payload: bool,
    /// Stamp each stored block with its key, for identity checking.
    ///
    /// Costs one host-to-device copy per key, which puts work on the per-key path
    /// (FR-070), so it is opt-in.
    #[arg(long)]
    pub stamp_keys: bool,
    /// How a turn discovers residency: `check` (default, what the production client does)
    /// or `lookup`.
    ///
    /// **This is the switch that decides whether a run exercises remote lookup at all.**
    /// `CHECK` answers from the local dispatch-map only, so a key a peer holds reads as
    /// absent and the client stores it instead of asking for it; only `LOOKUP` reaches
    /// `batch_lookup`, which forwards a local miss to `remote_lookup`. `lookup` mode
    /// therefore turns every local miss into a remote-lookup attempt, and is one fewer
    /// round trip per turn.
    ///
    /// It selects a **client rule, not a workload**: plan `OpKind`s never reach the wire,
    /// so the plan is byte-identical either way and FR-072 is untouched. The run report
    /// names the mode, because two runs that differ only in this are different experiments.
    ///
    /// `check` is what FR-039 requires of the production-faithful stream; `lookup` is a
    /// documented deviation, taken deliberately to reach a path the production client
    /// cannot.
    #[arg(long, default_value = "check")]
    pub probe: String,
    /// A Certus instance to drive, as `host[:port][:mailbox]`, repeatable.
    ///
    /// **An instance, not a machine** (FR-081): a host routinely runs several — one per NUMA
    /// domain, or one per NVMe device — so a hostname is not an identity and two entries may
    /// share a host. A component that parses as a number is the port, one starting with `/` is
    /// the mailbox, so `node5`, `node5:7001`, `node5:/dev/shm/certus-shmq-1` and
    /// `node5:7001:/dev/shm/certus-shmq-1` are all unambiguous.
    ///
    /// Every instance runs an agent and the generator reaches it over TCP, so only keys cross
    /// the network (FR-047) — **including the local one**, which is launched as a child process
    /// rather than over ssh (FR-079). Sessions are placed uniformly across the instances given
    /// and a session with a `migration_interval` moves between them (FR-048), so this list is a
    /// property of the deployment and deliberately not of the description.
    ///
    /// Absent, the hardware file's list is used; absent that, this host alone. Given, it
    /// **replaces** the file's list rather than adding to it.
    #[arg(long = "instance", value_name = "HOST[:PORT][:MAILBOX]")]
    pub instances: Vec<crate::hardware::Instance>,
    /// The hardware file, describing the deployment rather than the workload (FR-082).
    ///
    /// Absent, `./cluster.yml` is read if it exists. The command line overrides it per field.
    #[arg(long, value_name = "FILE")]
    pub hardware: Option<PathBuf>,
    /// Path to the agent binary **on each instance**.
    ///
    /// A bare name is looked for beside the generator's own executable when the node is local,
    /// which is where a cargo build puts it.
    #[arg(long, default_value = "workload-node-agent")]
    pub agent_binary: String,
    /// Use agents that are already running instead of launching them.
    ///
    /// For an operator managing the daemons themselves. It weakens FR-052 — a leftover of the
    /// current build is reused rather than replaced — which is why it is opt-in.
    #[arg(long)]
    pub no_launch: bool,
    /// Virtual seconds per wallclock second, or `inf` to issue as fast as the transport allows.
    ///
    /// **The only pacing knob** (FR-081). A finite rate paces the run: each turn is held until
    /// its virtual time is due, and the run measures the latency Certus delivers under the load
    /// this workload actually represents. `inf` makes it work-conserving, which measures how
    /// fast Certus can go. One knob rather than two makes the contradiction the old
    /// `--pacing none --rate 10` needed a refusal for unsayable.
    ///
    /// **Paced is the default**, because the failure of the other default is quiet: paced when a
    /// ceiling was wanted returns a throughput capped at the rate asked for, which is
    /// conspicuous, while work-conserving when the workload's latency was wanted returns
    /// percentiles from a saturated queue — plausible-looking figures describing a queue the
    /// workload would never form.
    ///
    /// A **calibration** control: a description's durations are arbitrary with respect to any
    /// particular machine, so this is how one description is aimed at faster or slower hardware
    /// without being rewritten. It changes only the tempo — the same keys in the same order —
    /// and a run's plan fingerprint is independent of it.
    ///
    /// It also sets the cost: at rate 1.0 a run takes wallclock equal to its virtual span.
    ///
    /// A large **finite** rate is not `inf`. `--rate 1e12` keeps a schedule the machine cannot
    /// meet, so its lateness is real and the run is correctly invalid; `inf` has no schedule to
    /// miss. Somebody will type a large number meaning "flat out".
    #[arg(long, default_value_t = 1.0)]
    pub rate: f64,
    /// The 99th-percentile lateness a paced run tolerates, in milliseconds (FR-080).
    ///
    /// Beyond this the run is **invalid**: the generator, not Certus, set the pace, so its
    /// latency percentiles describe a schedule that was not kept.
    #[arg(long, default_value_t = crate::live::DEFAULT_LATENESS_TOLERANCE_US / 1_000)]
    pub lateness_tolerance_ms: u64,
    /// Structured report destination.
    #[arg(long)]
    pub report: Option<PathBuf>,
}

/// Subcommands.
#[derive(Debug, Subcommand)]
pub enum Command {
    /// Drive Certus through a per-node agent, on the local node or across a cluster.
    ///
    /// Unbounded by default (FR-059); stops cleanly on SIGINT/SIGTERM, and an
    /// interrupted run that kept its schedule is **valid** (FR-074).
    #[cfg(feature = "live")]
    Run {
        /// Everything a run takes, as one struct so nothing is passed positionally.
        #[command(flatten)]
        args: Box<RunArgs>,
    },
    /// Write the workload in another tool's trace format. Contacts no server and needs no
    /// accelerator.
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
        /// Mooncake projection. A single **file**.
        #[arg(long)]
        mooncake: Option<PathBuf>,
        /// libCacheSim CSV projection. A single **file**.
        #[arg(long = "libcachesim", alias = "cachesim")]
        cachesim: Option<PathBuf>,
        /// Qwen-Bailian usage-trace JSONL, which `apps/eviction-replay-benchmark`
        /// reads. A single **file**.
        ///
        /// Named for the format rather than for that one consumer: the shape is
        /// Alibaba's anonymized Bailian usage trace (`qwen-bailian-usagetraces-anon`),
        /// so a generated file and a captured one go through the same readers.
        ///
        /// A projection is a file rather than a directory because it is not a trace
        /// (FR-075b): lossy, not self-describing, and never accepted in place of the
        /// description and seed for a reproducibility check.
        #[arg(long = "qwen-bailian")]
        qwen_bailian: Option<PathBuf>,
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
        Command::Run { args } => match live_run(&args) {
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
            mooncake,
            cachesim,
            qwen_bailian,
            report,
            force,
        } => match emit(
            &description,
            until,
            seed,
            Outputs {
                mooncake,
                cachesim,
                qwen_bailian,
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
    }
}

/// Where an emit run writes. **At least one** must be set.
///
/// Every output is named the same way — one flag, one destination — so nothing is
/// privileged and no run is obliged to produce a format it does not want. Each is a
/// **file**, because a projection is not a trace (FR-075b): it is one other tool's
/// shape, lossy on purpose, and a workload is repeated from its description and seed
/// rather than from a stored copy (FR-072).
#[derive(Debug, Default)]
pub struct Outputs {
    /// Mooncake projection file.
    pub mooncake: Option<PathBuf>,
    /// libCacheSim CSV projection file.
    pub cachesim: Option<PathBuf>,
    /// Qwen-Bailian usage-trace JSONL projection file.
    pub qwen_bailian: Option<PathBuf>,
}

impl Outputs {
    /// Whether nothing at all was asked for.
    fn is_empty(&self) -> bool {
        self.mooncake.is_none() && self.cachesim.is_none() && self.qwen_bailian.is_none()
    }
}

/// Render one projection's declared losses (FR-077).
///
/// # Why this is shared rather than written per format
///
/// FR-077's declaration is only worth anything if it is the *same* declaration for every
/// format. Each format owns its list — it alone knows what it dropped — but the rendering
/// is one function, for the same reason FR-056 gives about the record: two hand-written
/// copies of something that must agree will eventually not. Until this existed, only one
/// of the three formats declared anything at all.
fn one_projections_losses(indent: &str, losses: &[String]) -> String {
    let mut out = String::new();
    for loss in losses {
        out.push_str(indent);
        out.push_str("DROPPED: ");
        out.push_str(loss);
        out.push('\n');
    }
    out
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
/// | Mooncake JSONL | 12 485 126 | 6.4 |
/// | libCacheSim CSV | 62 863 382 | 32.0 |
/// | qwen-bailian JSONL | 40 451 114 | 20.6 |
///
/// Rounded **up** in every case, because the check exists to refuse a run that would
/// fill a filesystem and an estimator that reads low fails at exactly the job it has.
/// `oracleGeneral` is not here because it is exact — 24 bytes per reference, fixed
/// layout.
mod bytes_per_reference {
    /// Mooncake: dense small integers, one prompt list per row.
    pub const MOONCAKE: u64 = 7;
    /// libCacheSim CSV: one whole row per reference, so the largest of all.
    pub const CACHESIM: u64 = 32;
    /// The Qwen-Bailian projection.
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
        "--qwen-bailian",
        &outputs.qwen_bailian,
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
        // Every output is a file, so the filesystem to check is the one its parent
        // directory sits on.
        let dir = p
            .path
            .parent()
            .filter(|d| !d.as_os_str().is_empty())
            .unwrap_or(Path::new("."))
            .to_path_buf();
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
                 overriding it produces a truncated file and a full filesystem rather \
                 than an output anything can read.\n{}{}",
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

    // At least one output, and none of the three is privileged (`contracts/cli.md`).
    if outputs.is_empty() {
        return Err(Failure::config(
            "nothing to write: pass at least one of --mooncake <file>, \
             --libcachesim <file>, --qwen-bailian <file>"
                .to_string(),
        ));
    }

    let (description, text, effective) = load(description_path)?;

    let projection = project(&description, until, seed)
        .map_err(|e| Failure::config(format!("cannot project the run: {e}")))?;

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

    let mut sim = Simulation::new(&description, seed, 1)
        .map_err(|e| Failure::config(format!("cannot start the simulation: {e}")))?;

    let started = Instant::now();

    // Two writers over one pass rather than a conversion afterwards: a conversion would
    // prove the converter right and say nothing about the writers, and SC-004's claim
    // is about the writers.
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
    let mut qwen_writer = match &outputs.qwen_bailian {
        Some(path) => Some(QwenWriter::new(BufWriter::new(create_projection_file(
            path,
        )?))),
        None => None,
    };

    // A message rather than an `io::Error`, because every one of these failures wants
    // naming the file it happened on.
    let mut write_error: Option<String> = None;
    sim.run_until(until, &mut |s, t| {
        // The `is_some` guard is not redundant with the loop: building the record walks a
        // turn's whole prefix, so a run that asked for no projection must not pay for it.
        if write_error.is_none()
            && (mooncake_writer.is_some() || cachesim_writer.is_some() || qwen_writer.is_some())
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
            if let Some(w) = qwen_writer.as_mut() {
                if let Err(e) = w.write_record(&record) {
                    write_error = Some(format!("writing the qwen-bailian projection: {e}"));
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
    let qwen_stats = match qwen_writer {
        Some(w) => Some(
            w.finish()
                .map_err(|e| Failure::other(format!("closing the qwen-bailian file: {e}")))?,
        ),
        None => None,
    };
    let wallclock = started.elapsed().as_secs_f64();

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
        generation_rate_invocations_per_second: if wallclock > 0.0 {
            sim.turns_taken() as f64 / wallclock
        } else {
            f64::INFINITY
        },
        generation_wallclock_seconds: wallclock,
        reproduction: Reproduction {
            seed,
            until,
            description_digest: digest_of(&text),
            description_path: description_path.display().to_string(),
        },
        projection: ProjectionSummary::from(&projection),
        warnings: {
            let mut w = projection.warnings.clone();
            w.extend(migration_is_not_emitted(&description));
            w
        },
    };

    // Every output is one other tool's file, so there is no artifact of ours a report
    // could sit beside: `--report` is the only destination for the structured form. It is
    // rendered to the terminal either way, so nothing is lost silently.
    let report_json = report
        .to_json()
        .map_err(|e| Failure::other(format!("serialising the report: {e}")))?;
    let mut report_paths: Vec<PathBuf> = Vec::new();
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
    // Each projection declares its own losses here, exactly as `convert` does and from the
    // same lists (FR-077). This path declared nothing until it did, which was the wrong way
    // round: FR-075 exists so that a projection can be had *without* writing the native
    // trace, making this the ordinary way to obtain one, and a run that writes only a
    // projection has no other place the declaration could appear.
    let mut wrote_a_projection = false;
    if let (Some(path), Some(s)) = (&outputs.mooncake, &mooncake_stats) {
        out.push_str(&format!(
            "  mooncake          {} records to {} ({} distinct identifiers)\n",
            s.records,
            path.display(),
            s.distinct_ids
        ));
        out.push_str(&one_projections_losses("    ", &s.declared_losses()));
        wrote_a_projection = true;
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
        out.push_str(&one_projections_losses("    ", &s.declared_losses()));
        wrote_a_projection = true;
    }
    if let (Some(path), Some(s)) = (&outputs.qwen_bailian, &qwen_stats) {
        out.push_str(&format!(
            "  qwen-bailian      {} records to {} ({} sessions, {} distinct keys)\n",
            s.records,
            path.display(),
            s.sessions,
            s.distinct_keys
        ));
        out.push_str(&one_projections_losses("    ", &s.declared_losses()));
        wrote_a_projection = true;
    }
    // Once for the run rather than once per projection: three copies of it would read as
    // three different claims about three files instead of one property of all of them.
    if wrote_a_projection {
        out.push_str("  ");
        out.push_str(workload_trace::PROJECTION_IS_NOT_A_TRACE);
        out.push('\n');
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
    let mut sim = Simulation::new(&description, seed, 1)
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

/// The `run` subcommand: drive Certus through its per-node agents.
///
/// Returns the report text and the exit code, so validity travels in the process status
/// and not only in the report (FR-062, FR-080). A sweep driver that treats "the process exited"
/// as "I have a data point" is exactly how an invalid run gets published.
///
/// `--until` is **optional**: absent means an unbounded run (FR-059), which works because
/// the producer is throttled by the lanes' queues rather than by memory. An earlier cut
/// pre-built the whole plan and silently substituted a 60-second span, which ran a
/// different experiment than the one asked for.
///
/// # One path, local and remote (FR-079)
///
/// There is no separate local branch. With no `--node` the generator drives **this** host through
/// an agent it launches itself, as a child process with no ssh — so the simplest possible
/// invocation still needs no setup, while the transport, the driver and the teardown are the same
/// mechanism everywhere. The description is loaded and the signal handler installed before
/// anything is started, and nothing here requires a local mailbox: a run can be driven from a
/// host with no Certus and no accelerator on it at all.
///
/// # Errors
///
/// [`Failure`] with the exit code the contract gives that outcome: 2 for a refused invocation, 3
/// for a run that began and was abandoned, 4 for a peer refused before it started.
#[cfg(feature = "live")]
fn live_run(args: &RunArgs) -> Result<(String, i32), Failure> {
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::Arc;

    use crate::live::{Pacing, RunOptions};

    // The deployment: the command line over the hardware file over the built-in defaults, per
    // field (FR-082). Read before anything is launched, because a refusal here has issued
    // nothing and is exit 2 rather than an abandoned run.
    let hardware = crate::hardware::read(args.hardware.as_deref()).map_err(Failure::config)?;
    if let Some(h) = &hardware {
        // An implicit file changes a run's meaning without appearing in the command line, so it
        // is announced as well as recorded in the report (FR-063). Required, not a courtesy.
        if h.implicit {
            eprintln!(
                "using the hardware file {} ({}), found by default; --hardware names another",
                h.path, h.digest
            );
        }
    }
    // `--rate` overrides the file's; the default value is indistinguishable from an explicit
    // 1.0, so the file wins only when the flag was left alone.
    let rate = match hardware.as_ref().and_then(|h| h.rate) {
        Some(from_file) if args.rate == 1.0 => from_file,
        _ => args.rate,
    };
    // Derived, never given: `inf` *is* work-conserving, so the two cannot disagree.
    //
    // It must switch the schedule off rather than divide by it. Left to the arithmetic,
    // `due = t0 + virtual/inf` gives `due == t0`, so every turn records as late by however long
    // the run has been going, lateness grows without bound, and every work-conserving run
    // reports *itself* invalid.
    let pacing = if rate.is_infinite() {
        Pacing::None
    } else {
        Pacing::Real
    };
    // Refused rather than clamped. A rate of zero would make every turn due at `t0` and quietly
    // run work-conserving; a negative one has no meaning at all; NaN compares false with
    // everything and would pace on a coin toss.
    if pacing == Pacing::Real && !(rate.is_finite() && rate > 0.0) {
        return Err(Failure::config(format!(
            "--rate must be greater than zero, not {rate}. It is virtual seconds per wallclock \
             second, so zero would mean a run that never becomes due. Use `inf` for a \
             work-conserving run"
        )));
    }

    // Loaded first, and before anything is started or claimed.
    let (description, text, effective) = load(&args.description)?;
    let block_bytes = u32::try_from(description.blocks.bytes)
        .map_err(|_| Failure::config("blocks.bytes exceeds a 32-bit reservation"))?;
    let _ = text;

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

    // What a run costs in wallclock, said before it starts. At rate 1.0 a paced run takes its
    // virtual span, so `--until 3600` is an hour — and an hour of silence looks like a hang.
    // Symmetrical with FR-073's size projection on the emit path.
    if pacing == Pacing::Real {
        match args.until {
            Some(span) => eprintln!(
                "paced at {rate:.3} virtual seconds per wallclock second: this run will take \
                 about {} of wallclock time, because a paced run's cost *is* its virtual span \
                 divided by the rate",
                humanise(span / rate)
            ),
            None => eprintln!(
                "paced at {rate:.3} virtual seconds per wallclock second, unbounded: this run \
                 continues until interrupted, and a paced run is often idle by design"
            ),
        }
    }

    // The instance list: the command line replaces the file's rather than adding to it, because
    // the case that forces the choice is a rate sweep — vary the rate, hold the deployment — and
    // appending would silently double a cluster on the second invocation.
    let instances: Vec<crate::hardware::Instance> = if !args.instances.is_empty() {
        args.instances.clone()
    } else {
        match hardware.as_ref().map(|h| h.instances.clone()) {
            Some(from_file) if !from_file.is_empty() => from_file,
            // This host alone, through an agent launched as a child process.
            _ => vec![crate::hardware::Instance {
                host: LOCAL_NODE.to_string(),
                port: crate::hardware::DEFAULT_PORT,
                mailbox: crate::hardware::DEFAULT_MAILBOX.to_string(),
            }],
        }
    };
    crate::hardware::check_distinct(&instances).map_err(Failure::config)?;
    // Local means every instance is on this host, so the child-process launcher applies. A
    // mixed list is driven over ssh, which reaches a local host too.
    let local = instances.iter().all(|i| i.host == LOCAL_NODE);
    let binary = if local {
        crate::agents::local_agent_binary(&args.agent_binary)
    } else {
        args.agent_binary.clone()
    };
    // The payload options are the agent's, not the generator's: the agent owns the device buffer,
    // and under FR-079 the generator has none. Passing them on the command line is what keeps
    // `--no-payload` and `--verify-payload` meaning the same thing they always did.
    let mut extra_args = Vec::new();
    // Refused here as well as in the agent: the generator must not launch four agents and
    // then learn from the first one's exit that the mode was misspelled.
    let probe = workload_wire::probe::Probe::parse(&args.probe).map_err(Failure::config)?;
    if probe != workload_wire::probe::Probe::default() {
        extra_args.push("--probe".to_string());
        extra_args.push(probe.as_str().to_string());
    }
    if args.no_payload {
        extra_args.push("--no-payload".to_string());
    } else {
        extra_args.push("--gpu-device".to_string());
        extra_args.push(args.gpu_device.to_string());
    }
    if args.stamp_keys {
        extra_args.push("--stamp-keys".to_string());
    }
    if args.verify_payload {
        extra_args.push("--verify-payload".to_string());
    }
    let specs: Vec<crate::agents::AgentSpec> = instances
        .iter()
        .map(|instance| crate::agents::AgentSpec {
            node: instance.host.clone(),
            port: instance.port,
            shm_path: instance.mailbox.clone(),
            binary: binary.clone(),
            lanes: args.lanes,
            block_bytes,
            batch_keys: args.batch_keys,
            extra_args: extra_args.clone(),
        })
        .collect();

    // Agents are started and verified **before the clock starts** (FR-050): launching, waiting
    // for a port and replacing a leftover are setup, and charging them to the system under test
    // would make a slow launch look like a slow cache. A peer refused for provenance or capacity
    // is exit 4 — the deployment is wrong, and rerunning it will fail identically.
    //
    // The launcher outlives the agents deliberately: declared first, so it is dropped last, and
    // its own teardown can reap a child that ignored `Shutdown`.
    let depth = workload_wire::client::DEFAULT_DEPTH;
    let launcher = (!args.no_launch && local).then(crate::agents::LocalLauncher::default);
    let mut agents = match (&launcher, args.no_launch) {
        (_, true) => {
            crate::agents::Agents::start_with(&crate::agents::NoLaunch, &specs, depth, false)
        }
        (Some(l), false) => crate::agents::Agents::start(l, &specs, depth),
        (None, false) => {
            crate::agents::Agents::start(&crate::agents::SshLauncher::default(), &specs, depth)
        }
    }
    .map_err(Failure::peer)?;

    let options = RunOptions {
        seed: args.seed,
        until: args.until,
        batch_keys: args.batch_keys,
        clear_cache: args.clear_cache,
        pacing,
        rate,
        lateness_tolerance_us: args.lateness_tolerance_ms.saturating_mul(1_000),
    };
    // Captured before the agents are stopped: the report names the capacity the nodes actually
    // reported, and printing 0 there would be a wrong number rather than a missing one.
    let node_channels: usize = agents
        .agents()
        .iter()
        .map(|a| a.ack.channels as usize)
        .sum();
    let lanes = agents.total_lanes();

    let driven = crate::drive::run(&mut agents, &description, &options, Arc::clone(&stop));
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
        // deployment being refused before it started. A refused setup issued nothing at all, so
        // it is exit 2 like any other rejected invocation.
        Err(crate::drive::DriveError::Lost(lost)) => {
            return Err(Failure {
                message: format!("RUN INVALID \u{2014} {lost}"),
                code: exit::INVALID,
            })
        }
        Err(crate::drive::DriveError::Setup(why)) => return Err(Failure::config(why)),
    };

    let mut rendered = effective;
    rendered.push_str(&live_report(
        &out.stats,
        None,
        out.migrations,
        node_channels,
        lanes,
        args.batch_keys,
        args.seed,
        probe,
        &args.description,
        args.report.clone(),
    )?);
    rendered.push_str(&format!("  instances         {}\n", instances.len()));
    // Named by instance, not by host: two instances on one machine would otherwise produce two
    // rows reading `node5`, which is not a report. The mailbox is listed here once rather than
    // repeated on every counter row.
    for instance in &instances {
        rendered.push_str(&format!(
            "    {:<24} mailbox {}\n",
            instance.label(),
            instance.mailbox
        ));
    }
    if let Some(h) = &hardware {
        // Recorded under FR-063 so a report says which deployment file this was, not only that
        // there was one. Together with the announcement above, this is what keeps an implicit
        // file from changing a run's meaning invisibly.
        rendered.push_str(&format!(
            "  hardware file     {} ({}{})\n",
            h.path,
            h.digest,
            if h.implicit { ", by default" } else { "" }
        ));
    }
    for (node, c) in &out.per_node {
        rendered.push_str(&format!(
            "    {node:<24} {:>8} requests, {:>7} blocks read, {:>7} written\n",
            c.requests,
            c.blocks_read(),
            c.blocks_written()
        ));
    }
    // Printed always, including zero. A multi-instance run whose migration interval is long
    // relative to session lifetime performs none, and it would otherwise be indistinguishable
    // from one that exercised the case — which is what FR-049's "inert rather than an error"
    // makes easy to arrive at by accident. Zero here is a finding, not a missing field.
    rendered.push_str(&format!(
        "  migrations        {}{}\n",
        out.migrations,
        if instances.len() < 2 {
            " (inert: migration needs two or more instances, FR-049)"
        } else if out.migrations == 0 {
            " — none performed, so this run did not exercise a cold prefix on arrival"
        } else {
            ""
        }
    ));
    let code = if out.stats.is_valid() {
        exit::OK
    } else {
        exit::INVALID
    };
    Ok((rendered, code))
}

/// The node a run drives when `--node` was not given.
///
/// `127.0.0.1` rather than `localhost`, so it cannot resolve to an IPv6 address the agent's
/// default bind does not listen on — a failure that presents as "no agent accepted a connection"
/// with an agent plainly running.
#[cfg(feature = "live")]
const LOCAL_NODE: &str = "127.0.0.1";

/// A duration a person can read, for the wallclock projection.
#[cfg(feature = "live")]
fn humanise(seconds: f64) -> String {
    if !seconds.is_finite() || seconds < 0.0 {
        return "an unknown amount".to_string();
    }
    if seconds < 90.0 {
        return format!("{seconds:.0} seconds");
    }
    if seconds < 5_400.0 {
        return format!("{:.1} minutes", seconds / 60.0);
    }
    if seconds < 172_800.0 {
        return format!("{:.1} hours", seconds / 3_600.0);
    }
    format!("{:.1} days", seconds / 86_400.0)
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
    migrations: u64,
    node_channels: usize,
    lanes: usize,
    batch_keys: usize,
    seed: u64,
    probe: workload_wire::probe::Probe,
    description_path: &Path,
    report_path: Option<PathBuf>,
) -> Result<String, Failure> {
    use crate::report::{LatencyPercentiles, LiveReport, QueueStats, Tuning};

    use crate::live::Pacing;

    let valid = stats.is_valid();
    let report = LiveReport {
        run_kind: "live",
        valid,
        // Which reason, because the two modes fail differently — and a report that gave the
        // underrun reason for a paced run would send a reader to look at a queue that was
        // supposed to be empty.
        invalid_reason: (!valid).then(|| {
            if stats.payload_mismatches() > 0 {
                format!(
                    "Certus returned the wrong block for {} of {} loaded keys. That is a \
                     correctness failure rather than a cache outcome, so no throughput from \
                     this run is a result",
                    stats.payload_mismatches(),
                    stats.payloads_verified()
                )
            } else if stats.pacing == Pacing::Real {
                format!(
                    "the run missed its own schedule: p99 lateness {} us over {} turns, above \
                     the {} us tolerance, so the generator and not Certus set the pace and the \
                     latency percentiles describe a schedule that was not kept (FR-080)",
                    stats.lateness_us(0.99),
                    stats.paced_turns(),
                    stats.lateness_tolerance_us
                )
            } else {
                format!(
                    "lanes spent {:.3}% of their time waiting for the generator, above the \
                     {:.3}% a work-conserving run tolerates, so the generator and not Certus \
                     set the pace and the throughput describes the instrument (FR-062). \
                     Worst lane waited {:.3}s of {:.3}s; the queue was found empty {} times \
                     out of {} pops",
                    stats.producer_wait_fraction() * 100.0,
                    crate::report::DEFAULT_PRODUCER_WAIT_TOLERANCE * 100.0,
                    stats.worst_lane_producer_wait_us() as f64 / 1e6,
                    stats.elapsed,
                    stats.underruns(),
                    stats.pops()
                )
            }
        }),
        mode: stats.pacing.name(),
        probe: probe.as_str(),
        schedule: (stats.pacing == Pacing::Real).then(|| crate::report::Schedule {
            rate: stats.rate,
            rate_shortfall: stats.rate_shortfall().unwrap_or(0.0),
            turns: stats.paced_turns(),
            lateness_us: LatencyPercentiles {
                p50: stats.lateness_us(0.50),
                p90: stats.lateness_us(0.90),
                p99: stats.lateness_us(0.99),
                max: stats.max_lateness_us(),
            },
            tolerance_us: stats.lateness_tolerance_us,
            kept: stats.kept_the_schedule(),
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
            producer_wait_us: stats.producer_wait_us(),
            worst_lane_producer_wait_us: stats.worst_lane_producer_wait_us(),
            producer_wait_fraction: stats.producer_wait_fraction(),
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
        migrations,
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

/// Declare that an emitted trace carries no migrations, when the description asks for them.
///
/// # Why this is a loss and not a warning about nothing
///
/// A trace is deliberately free of instance identities — it says what the sessions did, not
/// which cache served them, and that is what makes one file replayable against any deployment.
/// But only the live driver has a node count to give the simulation, so an emit run passes one,
/// where migration is **inert** by FR-049. A description that declares a `migration_interval`
/// therefore emits a trace in which no session ever migrates.
///
/// So what is missing is not the *target* of a migration, which nothing here should record — it
/// is the **event**. And the event is the part that matters to a cache: a migrated session's
/// prefix is cold on arrival, which is the whole reason FR-048 exists. A reader comparing this
/// trace against a live multi-instance run would find fewer misses and no explanation.
///
/// FR-077's rule applied at the emit boundary rather than the projection boundary. It belongs
/// here and **not** in a projection's `declared_losses`: the projections drop what the trace
/// carries, and the trace never carried this, so declaring it there would tell a reader the
/// loss happened one stage later than it did. `research.md`'s D10 records the design that would
/// close it.
fn migration_is_not_emitted(description: &WorkloadDescription) -> Option<String> {
    let asked: Vec<&str> = description
        .session_classes
        .iter()
        .filter(|(_, _, class)| class.migration_interval.is_some())
        .map(|(_, name, _)| name)
        .collect();
    if asked.is_empty() {
        return None;
    }
    Some(format!(
        "migration is NOT represented in this trace: session class{} {} declare{} a \
         migration_interval, but an emit run simulates a single node, where migration is inert \
         (FR-049). No session migrates here, so this trace has fewer cold-prefix misses than the \
         same description driven across two or more instances. The event is missing, not just \
         its target",
        if asked.len() == 1 { "" } else { "es" },
        asked.join(", "),
        if asked.len() == 1 { "s" } else { "" }
    ))
}

/// Non-cryptographic digest of the description text a run was given.
///
/// Recorded by both `run` and `emit` so that a report names which description produced it
/// — with the seed, that is the whole of what FR-072 needs to repeat the workload.
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
            flag: "--mooncake",
            path: PathBuf::from("/tmp/mc.jsonl"),
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
    fn free_space_is_read_from_the_output_directorys_own_filesystem() {
        // Reading the working directory instead would report a number that looks
        // authoritative and describes a different mount.
        let free = free_bytes(Path::new("/tmp")).expect("statvfs on /tmp");
        assert!(free > 0, "no free space reported for /tmp");
    }
}
