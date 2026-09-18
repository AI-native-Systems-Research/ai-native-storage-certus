//! Trace output containers and the projections onto other tools' formats.
//!
//! `contracts/trace-io.md` is normative for what an emit run writes;
//! `contracts/trace-interop.md` is normative for what `convert` projects it into.
//! This crate is CUDA-free and a workspace default member, so container
//! equivalence and every schema invariant are testable with no accelerator, no
//! server and no network.
//!
//! # Two halves, and the line between them
//!
//! A **container** ([`jsonl`], and `parquet` behind its feature) is a trace: it carries the full
//! [`record`] schema and a [`manifest`] describing itself. A **projection**
//! ([`mooncake`], [`cachesim`], [`simulator`]) is another tool's shape, written
//! from that schema and lossy on purpose — no manifest, and never accepted in
//! place of a trace for a reproducibility check (FR-075b). Every projection
//! declares what it dropped rather than leaving it to be discovered (FR-077).
//!
//! Because a projection's input is the *schema* and not this generator's internals,
//! the same converter runs over a real corpus trace and a generated one, which is
//! what makes the two comparable (FR-075a).
//!
//! # Examples
//!
//! Write a trace, then project it onto what `apps/eviction-replay-benchmark` reads:
//!
//! ```
//! use workload_model::description::WorkloadDescription;
//! use workload_model::sim::Simulation;
//! use workload_trace::jsonl::JsonlWriter;
//! use workload_trace::manifest::Manifest;
//! use workload_trace::simulator;
//!
//! let yaml = r#"
//! version: 1
//! blocks: {tokens: 16, bytes: 32768}
//! shared_classes:
//!   docs: {length: {constant: 3}, lifetime: {constant: .inf}, pool: {size: {exact: 2}}}
//! session_classes:
//!   chat:
//!     pool: {size: {exact: 2}}
//!     uses: [{class: docs, count: {constant: 1}}]
//!     turns: {constant: 3}
//!     input_growth: {constant: 2}
//!     output_growth: {constant: 1}
//!     think_time: {constant: 5}
//! "#;
//! let description: WorkloadDescription = yaml.parse().unwrap();
//!
//! // The container.
//! let mut trace = Vec::new();
//! let mut sim = Simulation::new(&description, 7).unwrap();
//! let mut writer = JsonlWriter::new(&mut trace, "demo", description.blocks.tokens);
//! sim.run_until(60.0, &mut |s, t| writer.write(s, t).unwrap());
//! let stats = writer.finish().unwrap(); // releases the borrow on `trace`
//!
//! // The manifest, written last so an incomplete directory is unreadable (FR-073).
//! let manifest = Manifest::new("demo", &description, yaml, 7, 60.0, stats.clone());
//! assert_eq!(manifest.block_id_space, "chained_u64");
//!
//! // The projection, driven by the schema rather than by the simulation.
//! let mut projected = Vec::new();
//! let p = simulator::convert_jsonl(trace.as_slice(), &mut projected).unwrap();
//! assert_eq!(p.records, stats.invocations);
//! assert_eq!(p.sessions, stats.sessions);
//! ```
#![warn(missing_docs)]

/// What a projection is, said once so that both entry points say it identically (FR-075b).
///
/// Printed after a projection's declared losses by `emit` and by `convert` alike. It is a
/// property of every projection rather than of any one format, so it lives here and not in
/// a `declared_losses` list — and it is one string rather than two so the two paths cannot
/// drift into saying it differently, which is the same argument as FR-075a's for the
/// projections themselves.
pub const PROJECTION_IS_NOT_A_TRACE: &str =
    "a projection is not a trace: no manifest, not self-describing, and not accepted in \
     place of the native trace or the canonical plan for a reproducibility check (FR-075b)";

pub mod cachesim;
pub mod jsonl;
pub mod manifest;
pub mod mooncake;
#[cfg(feature = "parquet")]
pub mod parquet;
pub mod record;
pub mod simulator;
