//! Creusot verification crate for `components/disk-partition-manager`.
//!
//! These are FAITHFUL WHOLE-FUNCTION MIRRORS of the shipped functions in
//! `components/disk-partition-manager/src/gpt.rs` and `src/lib.rs`. The
//! disk-partition-manager crate itself cannot be built under Creusot (it depends
//! on the `interfaces` crate, `component_macros`, `crc32fast`, `std::sync::Mutex`,
//! SPDK/DMA FFI channels and `/dev/urandom`), so each mirror below reproduces the
//! relevant source body faithfully and attaches the inventory-derived contract to
//! the mirror. Each mirror's doc comment cites the exact source line range it
//! tracks, so the mirror-vs-source body equality (the drift guard) is auditable
//! by inspection.
//!
//! A green proof here covers the MIRROR only. The residual gap between mirror and
//! shipped function is the body-equality obligation, discharged by inspection
//! against the cited line ranges in
//! `disk-partition-manager_creusot_properties.md`.
//!
//! Properties are keyed to the ids in
//! `disk-partition-manager_property_inventory.md` (Role 1). The status of each is
//! recorded in the properties file, so Role 3 can attach it by id.

#![cfg_attr(creusot, allow(unused))]

pub mod layout; // format layout arithmetic: entry_sectors, usable LBAs, ceil, non-overlap, within-usable, 128x128
pub mod roundtrip; // G6 DPM-ROUNDTRIP-OFFSETS + DPM-INIT-RETURNS-CORRECT-LAYOUT
pub mod name; // G7 DPM-ROUNDTRIP-NAME (per-code-unit LE round-trip) + name-len-36 bound
pub mod guid; // DPM-FORMAT-{DISK,PART}-GUID-V4 (bit-level v4 version/variant)
pub mod header; // DPM-FORMAT-HEADER-CONSTANTS, BACKUP-MIRRORS-PRIMARY, MBR type 0xEE
pub mod state; // G2 INIT-GATE, G3 STATE-CACHED, G5 COUNT-INDEX-AGREE, PINFO/NUMP
pub mod errors; // G4 IO-PROPAGATE, G8 CRC gate, G9 NAMESPACE-ID, init/format/iof control flow
