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
#![warn(missing_docs)]

pub mod description;
pub mod distribution;
mod error;
pub mod keys;
pub mod pool;
pub mod rng;
mod special;

pub use error::{Error, Result};
