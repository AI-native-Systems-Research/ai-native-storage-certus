//! The shared model beneath the `certus-workload` tool suite.
//!
//! A workload is a statistical statement about *which blocks are asked for, by
//! whom, in what order, at what size*. This crate holds that statement's types,
//! the key derivation that turns it into a block reference stream, the plan
//! codec, and the statistics computed over it.
//!
//! There is deliberately **no concept of a tier, cache, memory or disk** here.
//! Where a block was resolved from is an outcome a consumer reports; this crate
//! cannot express it, and a workload therefore means the same thing whether its
//! consumer has two storage tiers, five, one, or none.
//!
//! The statistics live in this library rather than in a binary for a correctness
//! reason: they are computed over both real traces and generated plans, and two
//! implementations would drift — making a comparison between a fitted model and
//! its source trace a comparison of two different definitions.

pub mod corpus;
pub mod dist;
pub mod keys;
pub mod rng;

pub mod plan;

pub mod schema;
pub mod session;
