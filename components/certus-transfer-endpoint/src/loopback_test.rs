//! Single-host RDMA loopback integration tests (hardware-gated, `#[ignore]`d).
//!
//! Every test here is `#[ignore]`d so a default `cargo test` never requires a
//! NIC (constitution Principle VIII). Run them explicitly on an RDMA host:
//!
//! ```text
//! cargo test -p certus-transfer-endpoint --features rdma -- --ignored
//! ```
//!
//! Scaffolding: the fetch round trip lands in T054 and the legacy push path in
//! T055, both mirroring `remote-lookup-rdma-initiator/src/loopback_test.rs`.
