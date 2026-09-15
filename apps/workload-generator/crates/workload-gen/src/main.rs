//! The synthetic workload generator binary.
//!
//! Subcommands are specified in `contracts/cli.md`: `run` drives a live Certus
//! node, `emit` writes a trace file, `plan` writes the canonical plan
//! serialisation, `convert` projects a trace into the cache simulator's shape,
//! and `validate` runs the load-time checks and the projection without writing.
//!
//! Exit codes are part of the contract. In particular **3 means "completed but
//! INVALID"** — distinct from 0 — so a sweep driver cannot mistake an invalid
//! run for a data point.

fn main() {
    // Subcommand dispatch lands with the CLI wiring tasks (T050, T057-T059).
}
