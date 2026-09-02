# Spec-Sync Phase B Proposals — `interfaces`

**Generated**: 2026-08-20 (Phase B)
**Spec**: `components/interfaces/specs/001-interfaces/spec.md`
**Policy**: `.specify/sync/PHASE_B_POLICY.md`
**Drift source**: `.specify/sync/drift-report.json` (2 drifted requirements, 0 not_implemented, 0 unspecced)

## Summary

| Direction | Count |
|-----------|-------|
| BACKFILL (spec → code) | 0 |
| ALIGN (task, no code change) | 0 |
| RESOLVED (code already fixed) | 2 |
| BACKFILL-UNSPECCED | 0 |
| HUMAN_DECISION | 0 |

Both drift items were the same orphaned-module defect (`IExtendedMetadataStore` /
`ExtendedMetadataStoreError` defined but never wired into `lib.rs`). Per the Phase B
per-component note for `interfaces`, this defect was **already fixed on the main
development thread**. Both items are verified present in the current source and marked
**RESOLVED**. The spec's FR-014/FR-025 text already describes the implemented trait, so
**no spec backfill is required and no spec.md is edited**.

---

## Proposal 1 — FR-014 `IExtendedMetadataStore` Interface

- **Direction**: RESOLVED (code already fixed on main thread)
- **Reported drift**: Trait defined in `src/iextended_metadata_store.rs` but the module
  was never declared (`mod`) or re-exported (`pub use`) in `src/lib.rs`, so it was not part
  of the compiled crate (major; would break the `extended-metadata-store` consumer, masked
  only because that crate was out-of-workspace).
- **Verification (current source)**:
  - `src/lib.rs:77` — `mod iextended_metadata_store;` (ungated) ✓
  - `src/lib.rs:99` — `pub use iextended_metadata_store::{ExtendedMetadataStoreError, IExtendedMetadataStore};` (ungated) ✓
  - Trait defined at `src/iextended_metadata_store.rs:30` with `put`/`get`/`delete`/`iterate_all`/`force_flush`.
- **Rationale**: The trait is now compiled into and exported from the crate exactly as
  FR-014 describes. Spec text is already accurate.
- **before / after**: n/a (no spec edit).

---

## Proposal 2 — FR-025 Supporting Types: `ExtendedMetadataStoreError`

- **Direction**: RESOLVED (code already fixed on main thread)
- **Reported drift**: 4-variant enum existed but lived in the undeclared module, so it was
  not exported. Same orphaned-module root cause as FR-014.
- **Verification (current source)**:
  - Enum with exactly `NotFound`, `StorageError`, `CapacityExhausted`, `ValueTooLarge` at
    `src/iextended_metadata_store.rs:5-15` ✓
  - Re-exported via the ungated `pub use` at `src/lib.rs:99` ✓
- **Rationale**: The enum is now exported with exactly the 4 variants FR-025 requires. Spec
  text is already accurate.
- **before / after**: n/a (no spec edit).

---

## Note (informational, no action taken)

The spec header's `Last Synced 2026-08-07` block (`spec.md:17-22`) still carries a stale
"**Deferred (not applied):** FR-014/FR-025 ... remains an orphaned module" note that
predates the main-thread fix. Per the Phase B policy for `interfaces` ("Spec needs no
change"), the spec is **left untouched** this pass. Refreshing that historical sync note is
optional cleanup for a future edit and is out of scope for this RESOLVED-only pass.
