# Drift Report: interfaces

**Generated**: pending
**Project**: interfaces

## Summary

| Metric | Count |
|--------|-------|
| Specs Analyzed | 1 |
| Requirements Checked | 47 |
| Aligned | 45 |
| Drifted | 2 |
| Not Implemented | 0 |
| Unspecced Features | 0 |

Spec: `001-interfaces` — Shared Interface Trait Definitions (34 FR + 6 NFR + 7 SC). "Implementation" here means the trait/type definitions themselves. The spec was self-synced 2026-08-07 and explicitly defers the one substantive drift below.

## Detailed Findings

### Spec 001-interfaces — Shared Interface Trait Definitions

**Aligned ✓** (spot-verified against source)
- FR-002 `ILogger` (error/warn/info/debug) — `src/ilogger.rs:3-10`; re-exported `src/lib.rs:45`
- FR-004 `IBlockDevice` incl. `read_write_stats` — `src/iblock_device.rs:589`
- FR-006 `IEvictionPolicy::track(..., semantics: BlockSemantics)` + FR-020 `BlockSemantics`/`SessionId` — exported `src/lib.rs:33-38`
- FR-007 `IDispatchMap::set_checksum`/`get_checksum` behind `integrity-check` — `src/idispatch_map.rs:286-295`
- FR-017 `ReadWriteStats`, FR-018 `LookupResult` (4-variant) — present/exported
- FR-030/FR-031/FR-033/FR-034 RDMA initiator/responder split, `push_async`/`PushCompletion` — exported `src/lib.rs:52-60`
- Cargo features `spdk`, `gpu`, `integrity-check` all declared — `Cargo.toml:17-19` (matches Overview / NFR-002)
- Remaining FR-001, FR-003, FR-005, FR-008..FR-013, FR-015, FR-016, FR-019, FR-021..FR-024, FR-026..FR-029, FR-032, and NFR-001..006 — modules present and re-exported per `src/lib.rs`

**Drifted ⚠️**
- FR-014 `IExtendedMetadataStore` interface — **major**
  - Spec: FR-014 defines the `IExtendedMetadataStore` trait (put/get/delete/iterate_all/force_flush) as part of the crate.
  - Actual: the trait is defined in `src/iextended_metadata_store.rs:30-47`, but that module is **never declared** (`mod`) nor re-exported (`pub use`) in `src/lib.rs`. Grep confirms no reference to the file anywhere else in `src/`. The interface is therefore not part of the compiled crate. The consumer `extended-metadata-store` does `use interfaces::IExtendedMetadataStore` (`../extended-metadata-store/src/lib.rs:21`) and would fail to build; the break is masked only because that crate is excluded from the workspace. Documented as "Deferred (not applied)" in the spec's 2026-08-07 sync note.
  - Location: `src/lib.rs` (missing `mod`/`pub use`), definition at `src/iextended_metadata_store.rs:30`
- FR-025 `ExtendedMetadataStoreError` supporting type — **major** (same root cause)
  - Spec: 4-variant enum `ExtendedMetadataStoreError` (NotFound, StorageError, CapacityExhausted, ValueTooLarge) exported from `interfaces`.
  - Actual: the enum exists with exactly those 4 variants (`src/iextended_metadata_store.rs:5-15`) but, living in the undeclared module, it is **not exported** from the crate. Same orphaned-module cause as FR-014.
  - Location: `src/iextended_metadata_store.rs:5`

**Not Implemented ✗**
- None.

## Unspecced Features

| Feature | Location | Lines | Suggested Spec |
|---------|----------|-------|----------------|
| (none) | | | |

## Recommendations
1. Add `mod iextended_metadata_store;` and `pub use iextended_metadata_store::{IExtendedMetadataStore, ExtendedMetadataStoreError};` to `src/lib.rs` to make FR-014/FR-025 real. This is a one-line-each fix that removes the latent compile break for the `extended-metadata-store` consumer and is a prerequisite for bringing that crate into the workspace.
2. After wiring the module, confirm `cargo build`/`cargo build --features spdk` still pass and the consumer crate compiles.
