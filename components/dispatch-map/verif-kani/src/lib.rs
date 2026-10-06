// Kani verification crate for `dispatch-map`.
//
// Layout:
//   (crate root)  the REAL components/dispatch-map/src/lib.rs (+ entry.rs, state.rs), compiled
//                 in situ via build.rs + include! below. Not a copy, not a model.
//   mocks.rs      instrumented IEvictionPolicy / IExtentManager / ILogger providers and the
//                 Kani stubs (RandomState seed, Instant::now, Condvar) — see its header.
//   proofs_*.rs   the harnesses: one `verify_<id>` per level-2 inventory property (id
//                 lowercased, '-' -> '_'), `__mutant` anti-vacuity twins, `refute_<id>`
//                 refutations and `__<lever>` variants.
//
// Deliberately no inner doc comment (`//!`) here: the included production file is spliced in
// at the crate root.
#![allow(non_snake_case)]
#![allow(dead_code)]
#![allow(unused_imports)]
#![allow(unused_mut)]
#![allow(unused_variables)]
#![allow(clippy::all)]

include!(concat!(env!("OUT_DIR"), "/dm_lib.rs"));

#[cfg(kani)]
pub mod mocks;

#[cfg(kani)]
pub mod hashmap_model;

#[cfg(kani)]
mod proofs_refs;

#[cfg(kani)]
mod proofs_lookup;

#[cfg(kani)]
mod proofs_tier;

#[cfg(kani)]
mod proofs_misc;

#[cfg(kani)]
mod proofs_inv;

#[cfg(kani)]
mod proofs_model;
