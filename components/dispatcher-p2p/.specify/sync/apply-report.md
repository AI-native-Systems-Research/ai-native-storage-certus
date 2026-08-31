# Spec Sync — Apply Report
Project: dispatcher-p2p
Spec: 001-gpudirect-cold-path
Applied: 2026-08-31
Mode: interactive (the single spec-editing proposal was approved by the user)
Source: `.specify/sync/drift-report.{json,md}`, `.specify/sync/proposals.{json,md}`

## Backup

- `specs/001-gpudirect-cold-path/spec.md` → `.specify/sync/backups/specs/001-gpudirect-cold-path/spec.md.bak` (re-taken pre-edit this run)
- Prior backups retained under `.specify/sync/backups/`.

## Specs Updated

| Requirement / Section | Change Type | Notes |
|---|---|---|
| FR-008 | BACKFILL (amend) | Appended interface-parity note: the `IDispatcher` telemetry methods now include `tier_event_stats()` (commit `4659626b`), satisfied by a zeroed `TierEventStats::default()` stub since dispatcher-p2p does no tier-movement tracking (counters live in dispatcher FR-058); `read_write_stats()` aggregated from per-drive block-device counters. |
| Header — Last-Synced | metadata | Added the 2026-08-31 sync line (FR-008 amend; carried FR-017 ALIGN and `cold_staging_*` HUMAN_DECISION; `memcpy_batch_async` mock keep-up noted as test-only). |

## Align Tasks

| Requirement | Severity | Status |
|---|---|---|
| FR-017 — increment `eviction_dropped` on all live eviction publish sites | Moderate | **Open, retained** — code-side; `src/**` + `src/background.rs` edits outside sync scope. Full task in `align-tasks.md` (unchanged, reconfirmed 2026-08-31). |

## Unspecced Backfilled

| Feature | Source | Requirement |
|---|---|---|
| `tier_event_stats()` zeroed stub | `src/lib.rs:2665-2668` | Documented via FR-008 amendment (not a new FR — it is interface-parity behavior, not a new capability). |

## Human Decision (carried, unresolved)

| Item | Source | Reason |
|---|---|---|
| `cold_staging_slots` / `cold_staging_buf_bytes` | `../interfaces/src/idispatcher.rs:84,87` | Still unreferenced in `dispatcher-p2p/src/` (grep-verified). The 64-slot ring is governed by FR-003. Not backfilled (would invent behavior). Resolution needs an `interfaces/**` + `src/**` change — outside sync scope. Human to wire in or remove. |

## Not Applied / Deferred (out of scope)

| Item | Reason |
|---|---|
| FR-017 drop-count fix | `src/**` edit outside sync editable scope (`.specify/sync/**`, `specs/**` only). |
| `cold_staging_*` wire-in-or-remove | `interfaces/**` + `src/**` change outside sync scope. |
| `memcpy_batch_async` MockGpuServices (`src/lib.rs:3424+`, `#[cfg(test)]`) | Test-only mock keep-up (commit `495c5acc`); no requirement to reconcile. |
| `plan.md` source-layout / bench-name staleness | Doc-refresh item; not a requirement drift. |

## Counts

- BACKFILL applied (spec amend): 1 (FR-008)
- BACKFILL-UNSPECCED (new FR/SC): 0
- ALIGN tasks retained: 1 (FR-017)
- HUMAN_DECISION carried: 1 (`cold_staging_*`)
- RESOLVED since last sync: 0

## Notes

- Only Markdown under `specs/**` and `.specify/sync/**` was modified. No `src/**` was touched and `cargo` was not run.
