#![recursion_limit = "1024"]
#![allow(non_snake_case, non_camel_case_types, dead_code, unused_variables, unused_imports, unused_mut, unreachable_patterns, unused_assignments)]
//! Creusot verification mirror for `components/dispatcher` (pin 7d280cd3). See README in
//! `../verif/creusot_advisory.yaml` for the per-property boundaries.

include!("core.rs");
include!("boundary.rs");
include!("methods.rs");
include!("evict.rs");
include!("store.rs");
include!("lookup.rs");
include!("misc.rs");
include!("batch.rs");
include!("init.rs");
include!("bg.rs");
include!("drivers.rs");
