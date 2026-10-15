//! Feature-gated metrics.
//!
//! A zero-sized no-op when the `telemetry` feature is off, atomic counters when
//! it is on, with an identical method surface either way so call sites never
//! change. Counters must record *attempts* alongside completions: constitution
//! Principle XI forbids structuring metrics so an unexercised path is
//! indistinguishable from a working one.
//!
//! Scaffolding: lands in T015.
