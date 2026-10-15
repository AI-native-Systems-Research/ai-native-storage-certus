//! Safe wrappers over the verbs FFI: queue-pair lifecycle, registration,
//! posting and completion polling.
//!
//! The protection domain is owned by the **endpoint**, one per device, for the
//! endpoint's lifetime — memory regions are registered into it once and outlive
//! every queue pair, so an `rkey` already on the wire stays valid across a
//! reconnect (research D17). This is a deliberate departure from the predecessor
//! crates, where the PD lives in the connection and the pool is re-registered on
//! every connect.
//!
//! An explicit transition to the ERROR state is part of this surface: it is what
//! makes a late write impossible, which the deadline path depends on (FR-023).
//!
//! Scaffolding: lands in T009, PD ownership in T069.
