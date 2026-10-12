# Sync Apply Report — dispatcher

Applied: 2026-10-11 · Branch: `perf/coldload-striping-fixes` · Backup: `.specify/sync/backups/20261012T021231Z/` (`spec.md`, `errors.md`, `idispatcher.md`, `data-model.md`)

## Specs Updated

| Spec | Requirement | Change |
|---|---|---|
| 001 | FR-025 | Rewritten (BACKFILL): multi-drive always formats with warning; recovery from stripe allocator only, `N · extent.size` |
| 001 | FR-061 cap bullet | Rewritten (BACKFILL): 24-per-drive cap only with ≥ 4 participating drives; `split_queue_depth` order; `LEGACY_POLICY` |
| 001 | FR-061 head / drain / results | Modified (BACKFILL): public fn vs `_with_policy` callers; initial send failure; per-object errors/results; duplicated "The function MUST:" removed |
| 001 | FR-019 | Modified (BACKFILL): single-drive vs striped paths; cap gated to ≥ 4 drives; batch uses policy variant |
| 001 | FR-039 step (5) | Rewritten (BACKFILL): `_with_policy` + `BATCH_SCATTER_POLICY`, gated cap, backlog-aware H2D, checkout failure fails all |
| 001 | Q&A pipelined reader | Modified (BACKFILL) |
| 001 | Q&A SSD evictor drive ownership | Modified (BACKFILL): recovery single-drive only; first slice to run; enqueue refusal |
| 001 | User Story 9 narrative + scenario 1 | Modified (BACKFILL): striped read path for N > 1 |
| 001 | User Story 10 narrative | Modified (BACKFILL): utilization on the stripe allocator |
| 001 | User Story 11 narrative, scenarios 1/3/4 | Modified (BACKFILL) |
| 001 | User Story 5 scenario 5 | Added (BACKFILL-UNSPECCED) |
| 001 | Edge cases | 1 modified (SSD evictor), 2 added (forced format; partial GPU bytes) |
| 001 | FR-015, FR-034, FR-040, FR-041 | Modified (BACKFILL) |
| 001 | FR-062 (striped placement) | Added (BACKFILL-UNSPECCED) |
| 001 | FR-063 (multi-drive geometry validation) | Added (BACKFILL-UNSPECCED) |
| 001 | Key Entities: Extent, Background Writer, Pipelined Reader | Modified (BACKFILL) |
| 001 | SC-007 | Rewritten (BACKFILL): striping replaces key-based selection |
| 001 | SC-018 | Added (BACKFILL-UNSPECCED) |
| 001 | Last Synced header | 2026-10-11 entry added |
| 001 | contracts/errors.md (`InvalidParameter`) | Modified: FR-063 conditions |
| 001 | contracts/idispatcher.md (`format_on_init`) | Modified: forced true with > 1 drive |
| 001 | data-model.md (`DispatcherConfig.format_on_init`, `BackgroundWriteJob`) | Modified: forced format; `stripe` field; `device_index` role |

## Not Applied

| Item | Reason |
|---|---|
| `components/interfaces/src/idispatcher.rs:31-32` `format_on_init` doc comment ("Set to false when re-initializing to preserve on-disk data") | Outside this sync's editable scope (source file in another component); recorded as a follow-up in `align-tasks.md` (T2) |
| `drift-report.json` | Left unchanged: it carries the stamp fields (`inputs_sha256`, `git_commit`, `synced_at`) that the coordinator re-stamps |

Intra-spec contradictions resolved: FR-061 result indexing; unconditional 24 cap (FR-019 / Q&A / FR-061); `promote_and_serve` path (FR-019 / US-9 / Pipelined Reader / Q&A vs FR-061); FR-025 vs Q&A recovery; US-11 "grouped by target drive"; SC-007 "key-based drive selection"; FR-039(5) vs FR-061; US-10 vs FR-030; duplicated "The function MUST:".

No source files modified. Align tasks: 1 documentation-only follow-up (T2). Drift status after apply: clean (pending re-stamp).
