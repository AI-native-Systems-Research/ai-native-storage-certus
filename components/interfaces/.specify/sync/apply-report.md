# Sync Apply Report — interfaces

**Date**: 2026-08-07T15:54:25Z
Based on: proposals from 2026-08-07T15:54:25Z (drift-report 2026-08-07T15:29:55Z)
Backups: `.specify/sync/backups/20260807T155425Z/` (spec.md)

## Changes Made

### Specs Updated (BACKFILL — applied directly)

| Requirement | Change | Detail |
|-------------|--------|--------|
| Overview features | Modified | Added `integrity-check` optional feature. |
| FR-006 | Modified | `track` signature gains `semantics: BlockSemantics`; documents default/session-aware usage. |
| FR-020 | Modified | Added `SessionId` alias and `BlockSemantics` struct. |
| FR-007 | Modified | Added feature-gated `set_checksum`/`get_checksum` (consistent with dispatch-map). |
| FR-030 | Modified | Added `push_async` (callback-based non-blocking push). |
| FR-033 | Modified | Added `PushCompletion` type. |
| FR-018 | Modified | `LookupResult` label corrected "3-variant" → "4-variant" (auto; trivial typo). |

### New Specs Created
- None.

### Not Applied (deferred by user)

| Proposal | Direction | Reason |
|----------|-----------|--------|
| P5 — FR-014/FR-025 `IExtendedMetadataStore` orphaned module | ALIGN (code) | User selected **defer**. Left orphaned; spec FR-014/FR-025 retained. |

## ⚠️ Latent build issue (flagged, deferred)

`IExtendedMetadataStore` is defined in `src/iextended_metadata_store.rs` but `src/lib.rs`
(113 lines) never declares `mod iextended_metadata_store` nor re-exports the trait/error.
The `extended-metadata-store` component does `use interfaces::{IExtendedMetadataStore,
ExtendedMetadataStoreError}` and `impl`s the trait, so it **would fail to compile** — the
break is masked only because `extended-metadata-store` is absent from the root workspace
members (see that component's own drift). Wiring it in was deferred this session; revisit
alongside adding `extended-metadata-store` to the workspace.

## Next Steps
1. Review the updated spec.
2. If/when the deferred item is picked up: add `mod iextended_metadata_store;` + re-exports
   to `interfaces/src/lib.rs` and add `extended-metadata-store` to root workspace members.
3. Commit on the feature branch `sync/spec-drift-sweep-20260807`.
