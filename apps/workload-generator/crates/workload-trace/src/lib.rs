//! Trace output containers for generated workloads.
//!
//! Writes a [`workload_model`] operation plan to a trace directory in the
//! schema real decoded
//! LLM traces use, so a generated workload and a real trace are interchangeable
//! inputs to the same analysis. Two containers are supported — JSONL always,
//! parquet behind the non-default `parquet` feature — and they carry identical
//! records.
//!
//! A trace directory is self-describing: its `manifest.json` records the source
//! class, the encoding, the block geometry, and the block identifier space. The
//! manifest is written **last**, so a directory without one is incomplete by
//! construction and needs no separate "incomplete" flag.
#![warn(missing_docs)]
