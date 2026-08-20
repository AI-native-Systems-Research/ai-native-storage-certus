# Spec Sync Apply Report — gpu-services

**Applied**: 2026-08-20
**Component**: gpu-services
**Based on**: proposals from 2026-08-20 (drift-report 2026-08-20)

## Summary

| Metric | Count |
|---|---|
| BACKFILL applied | 1 |
| ALIGN tasks generated | 0 |
| UNSPECCED backfilled | 0 |
| RESOLVED | 0 |
| HUMAN_DECISION | 0 |

Prior-round canonical deliverables were archived before regeneration:
`proposals-20260721.{md,json}`, `apply-report-20260721.{md,json}` (the
2026-08-07 sweep proposals were already archived as `proposals-20260807.json`).

## Specs Updated

| Spec | Requirement | Change Type |
|---|---|---|
| 003-gpu-p2p-server | FR-012 | BACKFILL — reworded: chunked reads confirmed; MDTS ceiling is an operator responsibility documented via CLI help + 128KB default, not runtime-validated |
| 003-gpu-p2p-server | US1 Acceptance Scenario 4 | ADD — chunked-read scenario (`ceil(size/chunk-size)` chunks, `<n>` reported in `OK` response) |
| 003-gpu-p2p-server | Assumptions | ADD — bullet: operator responsible for `--chunk-size` ≤ MDTS (refs FR-012) |
| 003-gpu-p2p-server | Metadata | ADD — `Last-Synced: 2026-08-20` line |

## Align Tasks Generated

None. No drift item this run was a real behavioral bug (0 ALIGN).

## Unspecced Backfilled

None. Drift report reported 0 unspecced features (auxiliary `dma.rs` /
`gdrcopy_ffi.rs` items were backfilled into spec 002 in prior rounds).

## Resolved

None. No per-component "already fixed on main thread" items for gpu-services.

## Backups

| Spec file edited | Backup |
|---|---|
| `specs/003-gpu-p2p-server/spec.md` | `.specify/sync/backups/003-spec.md.20260820T171427Z.bak` |

`specs/001-gpu-cuda-services/spec.md` and `specs/002-gpu-ssd-dma-prepare/spec.md`
were not edited this run (0 drift each), so no backups were required for them.
