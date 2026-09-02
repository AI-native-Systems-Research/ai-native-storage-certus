# Spec-Sync Apply Report — block-device-filesys

**Generated**: 2026-09-02 (spec-sync re-run)
**Specs edited**: `specs/001-block-device-filesys/spec.md`, `specs/001-block-device-filesys/data-model.md`
**Backup**: `.specify/sync/backups/20260902T212901Z/` (`001-spec.md`, `data-model.md`)
**Source changes**: none (`.rs` files untouched; cargo not run)

## Result counts

| Category | Count |
|---|---|
| BACKFILL applied | 6 |
| BACKFILL-UNSPECCED | 0 |
| ALIGN tasks | 0 |
| RESOLVED | 0 |
| HUMAN_DECISION | 0 |

## Specs Updated

| Item | Artifact | Change |
|---|---|---|
| EDGE-SQ-FULL | spec.md | Rewrote the "io_uring submission queue full" edge case: was "actor MUST back-pressure by waiting"; now documents that the actor surfaces an error `Completion` (`Err(NotInitialized("io_uring submission queue full"))`) to the caller, matching FR-002 and the code (`src/actor.rs:469-480`, `588-601`). |
| DM-FILE-PATH-TYPE | data-model.md | `file_path` type corrected `RwLock<Option<PathBuf>>` → `Mutex<Option<PathBuf>>` (`src/lib.rs:62`). |
| DM-PROVIDES | data-model.md | `Provides` corrected `[IBlockDevice]` → `[IBlockDevice, IBlockDeviceAdmin]` (`src/lib.rs:57`). |
| DM-RING-TYPE | data-model.md | `ring` corrected `IoUring` → `Option<IoUring>`; added `shutdown_requested: bool` and feature-gated `telemetry: Arc<TelemetryStats>`; noted concrete struct is `FilesysHandler` (`src/actor.rs:109-120`). |
| DM-INFLIGHT-START | data-model.md | `InflightOp` telemetry field corrected `start_ns: u64` (stale "~0, never read, see align-tasks" defect) → `start: Instant` recorded on completion; removed the obsolete defect/align-task reference (fix documented in FR-019). |
| DM-CONFIGURED-STATE | data-model.md | "Configured" lifecycle step reworded to `create(...)`; noted the `set_*` mutators are reserved dead code per FR-023. |
| Metadata | spec.md | `Last Synced` line updated to record the 2026-09-02 re-run; prior notes retained. |

## Requirements verification

All 29 FR/SC (FR-001..023, SC-001..006) verified aligned against `src/{lib.rs,actor.rs,config.rs,telemetry.rs}`, `Cargo.toml`, `tests/integration.rs`, and the shared `components/interfaces/src/iblock_device.rs`. FR-015 — flagged drifted in the 2026-08-20 report — is now aligned (spec matches the intentional ` ```ignore ` `create()` example, `src/lib.rs:77-81`). The former unspecced `set_*` mutators are now specced as FR-023 (unspecced count → 0).

## Align Tasks Generated

None. Every divergence found was stale documentation against working, tested code
(resolved by BACKFILL), not a behavioral bug. No `components/interfaces/**` drift found.

## Notes

- Faithful-to-code stance: the SQ-full edge case was aligned to the code's deliberate,
  FR-002-documented error-surfacing behavior rather than proposing a code change to add
  waiting/back-pressure. Adding real back-pressure would be a new feature, out of scope
  for spec-sync.
- No code was modified; `.rs` files are byte-identical to HEAD (`2fc1cd3c`).
