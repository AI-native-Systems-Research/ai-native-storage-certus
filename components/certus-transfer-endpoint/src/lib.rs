//! Symmetric RDMA endpoint for explicit certus-to-certus KV cache transfer.
//!
//! A destination reserves landing slots in its own memory tier, names a single
//! peer (chosen externally by llm-d), and asks that peer for a run of block
//! keys; the source RDMA-writes the values it holds directly into those slots
//! and replies with a per-key outcome. There is no broadcast, no peer polling
//! and no reap: exactly one peer is asked, exactly once.
//!
//! The endpoint is symmetric at the connection level — one queue pair per peer
//! *instance* pair, either end able to post work requests — but **not** at the
//! API level. A destination drives an explicit `fetch()`; a source is reactive,
//! answering inbound requests from its own thread, which is why there is
//! deliberately no `serve()` method.
//!
//! # Build configuration
//!
//! Nothing is enabled by default, and the default build is the one CI runs:
//!
//! | feature | adds |
//! |---|---|
//! | *(none)* | connection table, framing, credits and pin logic over an in-process seam |
//! | `rdma` | the real libibverbs/librdmacm transport |
//! | `spdk` | block-tier serving: block-device receptacle and bounce-buffer pool |
//! | `telemetry` | metrics counters |
//!
//! Without `spdk` a block-tier lookup answer is reported as a miss rather than
//! an error (research D14). Hardware-dependent tests are `#[ignore]`d, so a
//! plain `cargo test` never needs a NIC or an NVMe device (constitution
//! Principle VIII).
//!
//! # Status
//!
//! Scaffolding. Tasks T001–T010, T012, T013, T068 and T081 of
//! `specs/001-rdma-cache-transfer/tasks.md` are complete: the crate builds as a
//! workspace default member, peer identity is implemented, and the verbs FFI, its
//! C shim and the safe wrappers over them exist behind `rdma`, and the transport
//! seam with its in-process mock, the connection table over it and its per-link
//! `wr_id` slab exist in every build. No transfer logic exists yet — the endpoint is T016 and the engine is
//! Phase 2.

#![deny(missing_docs)]

mod connection;
mod message;
mod serve;
mod telemetry;

#[cfg(feature = "rdma")]
mod ffi;
#[cfg(feature = "rdma")]
mod rdma;

#[cfg(feature = "spdk")]
mod bounce;

#[cfg(all(test, feature = "rdma"))]
mod loopback_test;
