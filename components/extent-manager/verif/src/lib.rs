//! Creusot verification crate for `components/extent-manager`.
//!
//! These are FAITHFUL WHOLE-FUNCTION MIRRORS of the shipped functions in
//! `components/extent-manager/src/*.rs`. The extent-manager crate itself cannot
//! be built under Creusot (it depends on the `interfaces` crate, `parking_lot`,
//! `std::collections::{HashMap, BTreeMap}`, SPDK/DMA FFI, and async), so each
//! function below reproduces the source body faithfully and attaches the
//! spec-derived contract to the mirror. Each mirror's doc comment cites the
//! exact source line range it tracks, so the mirror-vs-source body equality
//! (the drift guard) is auditable by inspection.
//!
//! A green proof here covers the MIRROR only. The residual gap between mirror
//! and shipped function is exactly the body-equality obligation, which is
//! discharged by inspection (each mirror body is quoted against its source line
//! range in `extent-manager_properties.md`).

#![cfg_attr(creusot, allow(unused))]

pub mod slab_geom;
pub mod align;
pub mod shard;
pub mod bitmap;
pub mod buddy;
