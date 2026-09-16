//! Trace output containers and the projections onto other tools' formats.
//!
//! `contracts/trace-io.md` is normative for what an emit run writes;
//! `contracts/trace-interop.md` is normative for what `convert` projects it into.
//! This crate is CUDA-free and a workspace default member, so container
//! equivalence and every schema invariant are testable with no accelerator, no
//! server and no network.
#![warn(missing_docs)]

pub mod jsonl;
pub mod manifest;
pub mod record;
