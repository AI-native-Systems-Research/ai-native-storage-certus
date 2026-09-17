//! The generator's library half, so every subcommand is testable in-process.
//!
//! The binary is a three-line `main` over this. A CLI that can only be exercised by
//! spawning a process tends not to be exercised at all — and the exit codes are part
//! of `contracts/cli.md`, so they need asserting rather than eyeballing.
#![warn(missing_docs)]

#[cfg(feature = "live")]
pub mod agents;
pub mod cli;
#[cfg(feature = "live")]
pub mod drive;
#[cfg(feature = "live")]
pub mod live;
pub mod report;
