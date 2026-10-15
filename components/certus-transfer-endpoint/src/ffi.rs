//! Verbs FFI declarations — **the single copy** (SC-008).
//!
//! `remote-lookup-rdma-initiator` and `remote-lookup-rdma-responder` each
//! declare their own `ffi.rs`, with 28 identical public items between them; this
//! module exists so a third copy is never made. It also declares the send/recv
//! surface neither predecessor ever used: `ibv_post_recv`, the receive work
//! request and SGE types, and `IBV_WR_SEND` as a live opcode rather than an
//! unused constant (research D3).
//!
//! Every item here is `unsafe` and requires a `// SAFETY:` justification
//! (constitution Principle V).
//!
//! Scaffolding: lands in T007.
