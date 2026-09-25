//! Projections of a generated workload onto other tools' trace formats.
//!
//! `contracts/trace-io.md` is normative for what an emit run writes. This crate is
//! CUDA-free and a workspace default member, so every schema invariant is testable with
//! no accelerator, no server and no network.
//!
//! # Projections, and what they deliberately are not
//!
//! A **projection** ([`mooncake`], [`cachesim`], [`qwen`]) is another tool's shape,
//! written from the [`record`] schema as each turn happens, and lossy on purpose. It is
//! not a trace: not self-describing, and never accepted in place of the description and
//! seed or the canonical plan for a reproducibility check (FR-075b). Every projection
//! declares what it dropped rather than leaving it to be discovered (FR-077).
//!
//! Each target is a **published** format with a real consumer — Mooncake's FAST'25
//! shape, libCacheSim's CSV and `oracleGeneral`, and the Qwen-Bailian usage trace that
//! `apps/eviction-replay-benchmark` reads. Nothing here writes a Certus-private
//! interchange format: a workload is reproduced from its description and seed (FR-072),
//! so storing one is never the way to repeat it.
//!
//! # Examples
//!
//! Project a run onto the Qwen-Bailian shape `apps/eviction-replay-benchmark` reads:
//!
//! ```
//! use workload_model::description::WorkloadDescription;
//! use workload_model::sim::Simulation;
//! use workload_trace::qwen::QwenWriter;
//! use workload_trace::record::InvocationRecord;
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
//! let block_size = description.blocks.tokens;
//!
//! // Written in the same pass as the simulation, not by converting a stored file.
//! let mut projected = Vec::new();
//! let mut sim = Simulation::new(&description, 7, 1).unwrap();
//! let mut turns = 0u64;
//! let stats = {
//!     let mut writer = QwenWriter::new(&mut projected);
//!     sim.run_until(60.0, &mut |s, t| {
//!         turns += 1;
//!         writer
//!             .write_record(&InvocationRecord::from_turn("demo", s, t, block_size))
//!             .unwrap();
//!     });
//!     writer.finish().unwrap()
//! };
//!
//! assert_eq!(stats.records, turns);
//! assert!(!stats.declared_losses().is_empty(), "a projection says what it dropped");
//! ```
#![warn(missing_docs)]

/// What a projection is, said once so that both entry points say it identically (FR-075b).
///
/// Printed once per emit run, after every projection's declared losses. It is a property of
/// every projection rather than of any one format, so it lives here and not in a
/// `declared_losses` list, and it is one string so that no two callers can drift into
/// saying it differently.
pub const PROJECTION_IS_NOT_A_TRACE: &str =
    "a projection is not a trace: lossy on purpose, not self-describing, and not accepted \
     in place of the description and seed or the canonical plan for a reproducibility \
     check (FR-075b)";

pub mod cachesim;
pub mod mooncake;
pub mod qwen;
pub mod record;
