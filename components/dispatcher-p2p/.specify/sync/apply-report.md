# Spec Sync — Apply Report
Project: dispatcher-p2p
Spec: 001-gpudirect-cold-path
Applied: 2026-09-02T21:32:13Z (re-analysis cycle)
Source: `.specify/sync/proposals.json`

## Backup

- Prior sync artifacts (drift-report, proposals, apply-report — both `.md` and `.json`) backed up
  to `.specify/sync/backups/20260902T213213Z/` before regeneration.
- Spec backups from earlier cycles retained under `.specify/sync/backups/` (`spec.md.bak`,
  `spec.md.20260722T232132Z.bak`, `data-model.md.20260722T232132Z.bak`,
  `specs/001-gpudirect-cold-path/spec.md.bak`).

## Specs Updated

_None._ The 2026-08-20 cycle already applied SC-006 (reword) and FR-018..FR-023 (backfill);
this cycle re-verified them against current source and found them aligned. No new backfill.

## Align Tasks Generated

| Requirement | Severity | Task |
|-------------|----------|------|
| FR-017 | Moderate | Increment `eviction_dropped` at all live eviction publish sites (lib.rs:603-645; background.rs:414-419,611-616) so `eviction_dropped_count()` reflects reality; thread a shared `Arc<AtomicU64>` counter into the background evictors. Task already present in `align-tasks.md` (carried from 2026-08-20, line refs re-verified this cycle). Not applied — sync does not edit `.rs`. |

## Unspecced Backfilled

_None this cycle._ (FR-018..FR-023 were backfilled 2026-08-20.)

## Resolved

_None._ FR-017 code fix remains outstanding (ALIGN task, not applied by sync).

## Human Decision

| Item | Source | Reason |
|------|--------|--------|
| cold_staging_slots / cold_staging_buf_bytes | interfaces/src/idispatcher.rs:84,87 | Config fields unreferenced in dispatcher-p2p/src/ (grep-verified). Ring size governed by FR-003. Out of scope (interfaces not editable by this sync). Human to wire in or remove. |
| FR-022 vs FR-023 lookup_async pin lifetime | src/lib.rs:2100 vs FR-023 | `lookup_async` releases the read pin at submission before caller sync; FR-023 says the pin must outlive completion for the local hot-path async copy. Code matches FR-022; FR-023 scope claim in tension. Human to confirm intent (narrow FR-023) or treat as latent race (code ALIGN). |

## Counts

- BACKFILL applied (drifted req): 0
- ALIGN tasks: 1 (FR-017, pre-existing, re-verified)
- UNSPECCED backfilled: 0
- RESOLVED: 0
- HUMAN_DECISION: 2 (cold_staging_* config; FR-022/FR-023 pin lifetime)

## Drift Status

`drift` — actionable code drift (FR-017 drop-count) remains unresolved; recorded as an ALIGN
task, deliberately not fixed by the sync pass (sync does not modify source).
