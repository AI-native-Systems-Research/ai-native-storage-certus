//! Request/reply framing, context identifiers and message credits.
//!
//! Two verbs SEND messages carry the control plane — `OP_FETCH` and
//! `OP_FETCH_REPLY` — while values travel by one-sided RDMA WRITE and never
//! appear in a message. See `specs/001-rdma-cache-transfer/contracts/wire-protocol.md`.
//!
//! Scaffolding: the codecs land in T027–T029, the context table in T031 and
//! message credits in T032.
