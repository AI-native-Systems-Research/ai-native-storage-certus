#![recursion_limit = "1024"]
#![allow(non_snake_case, dead_code, unused_variables, unused_imports, unused_mut, unreachable_patterns)]
//! Creusot verification mirror for `components/dispatch-map` (pin 08a5ae88).
//!
//! The shipped component cannot be translated by Creusot: it is built by `define_component!`,
//! keeps its state behind `Mutex`/`Condvar`, reaches its collaborators through `Arc<dyn …>`
//! receptacles and stores `AtomicU32` and `String` payloads. This crate therefore proves a
//! standalone, line-faithful mirror of `../src/lib.rs` and `../src/state.rs`:
//!
//! * `core.rs`    — the mirror types, the contract-carrying `#[trusted]` boundaries (HashMap as
//!                  an FMap ghost mirror; the eviction policy, extent manager, Mutex/Condvar and
//!                  clock as consumed-interface models) and one mirror function per method,
//!                  each with a full functional contract that its own module proves;
//! * `drivers.rs` — one `verify_<ID>` driver per level-2 inventory property (its contract IS
//!                  the obligation), `verify_<ID>__mutant` anti-vacuity twins (deliberately
//!                  false, must fail) and `refute_<ID>` modules for obligations the code breaks.
//!
//! Every disclosed boundary and modelling choice is recorded per property in
//! `../verif/creusot_advisory.yaml`.

include!("core.rs");
include!("drivers.rs");
