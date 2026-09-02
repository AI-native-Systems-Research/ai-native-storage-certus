# Spec Sync — Apply Report
Project: dispatcher-p2p
Spec: 001-gpudirect-cold-path
Applied: 2026-08-20 (Spec-Sync Phase B)
Source: `.specify/sync/drift-report.json`, resolved per `.specify/sync/PHASE_B_POLICY.md`

## Backup

- `specs/001-gpudirect-cold-path/spec.md` → `.specify/sync/backups/specs/001-gpudirect-cold-path/spec.md.bak`

(Prior backups from earlier cycles retained under `.specify/sync/backups/`:
`spec.md.bak`, `spec.md.20260722T232132Z.bak`, `data-model.md.20260722T232132Z.bak`.)

## Specs Updated

| Requirement | Change Type | Notes |
|-------------|-------------|-------|
| SC-006 | BACKFILL (reword) | Init logs non-fatal diagnostic + continues; panic deferred to first cold `batch_lookup`; single-key `lookup()` falls back to DRAM. Now consistent with FR-006/FR-007/US2 AC-1. |
| FR-018 | BACKFILL-UNSPECCED (new FR) | ParallelBackgroundWriter — per-drive write-through persistence. |
| FR-019 | BACKFILL-UNSPECCED (new FR) | BackgroundEvictor — SSD capacity reclamation at watermarks. |
| FR-020 | BACKFILL-UNSPECCED (new FR) | MemoryTierEvictor — proactive DRAM→SSD demotion sweep. |
| FR-021 | BACKFILL-UNSPECCED (new FR) | clear_memory_tier() admin flush. |
| FR-022 | BACKFILL-UNSPECCED (new FR) | lookup_async() — caller-side pipelined GpuStream. |
| FR-023 | BACKFILL-UNSPECCED (new FR) | PinnedKeys — read-pin lifetime guard across async copy. |
| Key Entities | BACKFILL-UNSPECCED (add) | Added ParallelBackgroundWriter, BackgroundEvictor, MemoryTierEvictor, PinnedKeys. |
| User Story 5 | BACKFILL-UNSPECCED (add) | "Automatic Tier Capacity Management" — 5 acceptance scenarios for FR-018..FR-022. |
| Status/Last-Synced | metadata | Added Last-Synced 2026-08-20 line. |

## Align Tasks Generated

| Requirement | Severity | Task |
|-------------|----------|------|
| FR-017 | Moderate | Increment `eviction_dropped` at all live eviction publish sites (lib.rs:602-645; background.rs:414-419,611-616) so `eviction_dropped_count()` reflects reality; thread shared counter into background evictors. See `align-tasks.md`. |

## Unspecced Backfilled

| Feature | Source | New Requirement |
|---------|--------|-----------------|
| ParallelBackgroundWriter (per-drive write-through) | background.rs:154-219 | FR-018 |
| BackgroundEvictor (SSD reclamation) | background.rs:303-488 | FR-019 |
| MemoryTierEvictor (DRAM→SSD demotion) | background.rs:490-654 | FR-020 |
| clear_memory_tier() | lib.rs:2606-2637 | FR-021 |
| lookup_async() | lib.rs:2044-2110 | FR-022 |
| pins::PinnedKeys | pins.rs:26-57 | FR-023 |

## Resolved

_None._ (No items in this drift cycle were pre-fixed on the main thread.)

## Human Decision

| Item | Source | Reason |
|------|--------|--------|
| cold_staging_slots / cold_staging_buf_bytes | interfaces/src/idispatcher.rs:81-87 | Config fields unreferenced anywhere in dispatcher-p2p/src/ (grep-verified). Dead config on this component's surface; the 64-slot ring is governed by FR-003. Not backfilled (would invent behavior). Human to wire in or remove from config surface. |

## Counts

- BACKFILL applied (drifted req): 1 (SC-006)
- ALIGN tasks: 1 (FR-017)
- UNSPECCED backfilled: 6 (FR-018..FR-023)
- RESOLVED: 0
- HUMAN_DECISION: 1 (cold_staging_* config fields)
