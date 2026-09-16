//! The synthetic workload generator binary.
//!
//! Subcommands are specified in `contracts/cli.md`: `run` drives a live Certus
//! node, `emit` writes a trace file, `plan` writes the canonical plan
//! serialisation, `convert` projects a trace into another tool's format, and
//! `validate` runs the load-time checks and the projection without writing.
//!
//! Exit codes are part of the contract. In particular **3 means "completed but
//! INVALID"** — distinct from 0 — so a sweep driver cannot mistake an invalid run
//! for a data point.

fn main() {
    // Everything is in the library half so it can be tested in-process; this is
    // deliberately the only thing that knows about the process.
    std::process::exit(workload_gen::cli::run_argv_os());
}
