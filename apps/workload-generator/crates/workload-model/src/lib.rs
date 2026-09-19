//! The CUDA-free simulation core of the synthetic workload generator.
//!
//! This crate turns a workload *description* (the YAML schema in
//! `contracts/workload-input.example.yml`) into an operation plan: a totally
//! ordered sequence of cache operations in **virtual time**. It performs no
//! I/O, links no accelerator, and opens no socket — the plan is a value.
//!
//! # Why this is a separate crate
//!
//! Both execution paths consume it: `workload-gen`'s live path drives the plan
//! into a Certus node, and its emit path writes the same plan to a trace file.
//! Because there is exactly one simulation core and it cannot reach hardware,
//! FR-072 ("the plan and trace are identical across live and emit runs, and
//! independent of batch size and lane count") holds by construction instead of
//! by convention, and the whole determinism and distribution test surface runs
//! with no accelerator and no server.
//!
//! # Examples
//!
//! The whole crate is three steps: parse a description, run a
//! [`sim::Simulation`] over a span of virtual seconds, and record the turns it
//! yields into an [`plan::OperationPlan`].
//!
//! ```
//! use workload_model::description::WorkloadDescription;
//! use workload_model::plan::OperationPlan;
//! use workload_model::sim::Simulation;
//!
//! let description: WorkloadDescription = r#"
//! version: 1
//! blocks: {tokens: 16, bytes: 32768}
//! shared_classes:
//!   docs:
//!     length: {constant: 4}
//!     lifetime: {constant: .inf}
//!     pool: {size: {exact: 8}}
//! session_classes:
//!   chat:
//!     pool: {size: {exact: 4}}
//!     uses: [{class: docs, count: {constant: 1}}]
//!     turns: {constant: 3}
//!     input_growth: {constant: 2}
//!     output_growth: {constant: 1}
//!     think_time: {constant: 5}
//! "#
//! .parse()
//! .unwrap();
//!
//! let mut sim = Simulation::new(&description, 42, 1).unwrap();
//! let mut plan = OperationPlan::default();
//! sim.run_until(120.0, &mut |session, turn| plan.record_turn(session, turn));
//!
//! // A plan is a value: totally ordered in virtual time, and nothing in it
//! // touched a device, a socket or a file.
//! plan.check_ordered().unwrap();
//! assert!(plan.turns() > 0);
//! ```
//!
//! The same description and seed give the same bytes, which is what makes a run
//! reproducible from its report alone (FR-012, FR-034):
//!
//! ```
//! # use workload_model::description::WorkloadDescription;
//! # use workload_model::plan::OperationPlan;
//! # use workload_model::sim::Simulation;
//! fn plan_of(seed: u64) -> OperationPlan {
//!     let description: WorkloadDescription = r#"
//! version: 1
//! blocks: {tokens: 16, bytes: 32768}
//! shared_classes:
//!   docs: {length: {constant: 4}, lifetime: {constant: .inf}, pool: {size: {exact: 8}}}
//! session_classes:
//!   chat:
//!     pool: {size: {exact: 4}}
//!     uses: [{class: docs, count: {constant: 1}}]
//!     turns: {constant: 3}
//!     input_growth: {constant: 2}
//!     output_growth: {constant: 1}
//!     think_time: {constant: 5}
//! "#
//!     .parse()
//!     .unwrap();
//!     let mut sim = Simulation::new(&description, seed, 1).unwrap();
//!     let mut plan = OperationPlan::default();
//!     sim.run_until(120.0, &mut |s, t| plan.record_turn(s, t));
//!     plan
//! }
//!
//! assert_eq!(plan_of(42).to_canonical_bytes(), plan_of(42).to_canonical_bytes());
//! assert_ne!(plan_of(42).fingerprint(), plan_of(43).fingerprint());
//! ```
#![warn(missing_docs)]

pub mod description;
pub mod distribution;
mod error;
pub mod keys;
pub mod plan;
pub mod pool;
pub mod project;
pub mod rng;
pub mod selection;
pub mod session;
pub mod sim;
mod special;

pub use error::{Error, Result};
