//! Symmetric RDMA endpoint for explicit certus-to-certus KV cache transfer.
//!
//! A destination reserves landing slots in its own memory tier, names a single
//! peer (chosen externally by llm-d), and asks that peer for a run of block
//! keys; the source RDMA-writes the values it holds directly into those slots
//! and replies with a per-key outcome. There is no broadcast, no peer polling
//! and no reap: exactly one peer is asked, exactly once.
//!
//! The endpoint is symmetric at the connection level — one queue pair per peer
//! pair, either end able to post work requests — but **not** at the API level.
//! A destination drives an explicit `fetch()`; a source is reactive, answering
//! inbound requests from its own thread, which is why there is deliberately no
//! `serve()` method.
//!
//! See `specs/001-rdma-cache-transfer/` for the specification, the research
//! decisions D1–D15, and the task list.
//!
//! # Status
//!
//! Scaffolding only — task T001 of `specs/001-rdma-cache-transfer/tasks.md`.
//! The module layout lands in T004 and the interface in T006; no transfer logic
//! exists yet.

#![deny(missing_docs)]
