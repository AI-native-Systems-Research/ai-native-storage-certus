# Apply Report: interfaces

**Project**: interfaces
**Spec**: `components/interfaces/specs/001-interfaces/spec.md`
**Backup**: `.specify/sync/backups/20260902T214143Z/` (spec.md + prior sync artifacts backed up before edits)

## Summary

| Metric | Count |
|--------|-------|
| Backfill applied (spec edits) | 12 |
| Align tasks generated | 1 |
| Unspecced backfilled (new FR) | 1 |
| Resolved (already fixed in code) | 2 (FR-014, FR-025) |
| Human decisions | 0 |

## Backfill applied to `spec.md`

- **P1 (FR-008)** `batch_lookup` signature → `&[(CacheKey, Vec<IpcHandle>)]` with scatter semantics.
- **P2 (FR-008)** `copy_gpu_to_memory_async` signature → `regions: &[IpcHandle]` with contiguous-gather note.
- **P3 (FR-008)** added `batch_populate` method.
- **P4 (FR-008)** added `tier_event_stats` method.
- **P5 (FR-018)** added `TierEventStats` supporting type.
- **P6 (FR-017)** `Command` 12 → 13 variants (added `FlushSync`).
- **P7 (FR-017)** `Completion` 11 → 12 variants (added `FlushDone`).
- **P8 (FR-017)** documented `ReadWriteStats` size histograms + new `IO_SIZE_BUCKETS` const.
- **P9 (FR-021)** `FormatParams` corrected "10-field" → "9-field" with the 9 fields enumerated.
- **P10 (FR-023)** `LookupConfig` corrected "10-field" → "12-field"; added `caller_wait` and `connection_teardown_timeout`.
- **P11 (FR-014/FR-025)** refreshed the stale "Last Synced 2026-08-07 … Deferred (not applied)" note into a 2026-09-02 note recording the resolution and this pass's changes.
- **P12 (FR-035, unspecced backfill)** added a new FR-035 documenting `IIpcServer` + `IpcServerConfig`/`IpcError`/`IpcMetricsSnapshot`, with an explicit orphaned-module caveat.

## Align tasks generated (no `.rs` edited)

- **ALIGN-IFACE-001** — wire `mod iipc;` + `pub use iipc::{IIpcServer, IpcError, IpcMetricsSnapshot, IpcServerConfig};` into `src/lib.rs` so the interface becomes part of the exported crate and the `ipc-component` consumer stops being a latent build break. Recorded in `align-tasks.md`.

## Resolved (already correct in code; no spec change beyond the refreshed note)

- **FR-014** `IExtendedMetadataStore` — module declared `src/lib.rs:78`, re-exported `src/lib.rs:100`; trait at `src/iextended_metadata_store.rs:30`.
- **FR-025** `ExtendedMetadataStoreError` (4-variant) — `src/iextended_metadata_store.rs:5`, exported via `src/lib.rs:100`.

## Human decisions

- None.

## Files changed

- `components/interfaces/specs/001-interfaces/spec.md` (12 backfill edits)
- `components/interfaces/.specify/sync/drift-report.md` / `drift-report.json`
- `components/interfaces/.specify/sync/proposals.md` / `proposals.json`
- `components/interfaces/.specify/sync/apply-report.md` / `apply-report.json`
- `components/interfaces/.specify/sync/align-tasks.md`
- `components/interfaces/.specify/sync/backups/20260902T214143Z/` (backups)

No `.rs` files were modified.
