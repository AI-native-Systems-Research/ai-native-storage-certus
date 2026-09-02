# Spec-Sync Proposals — block-device-filesys

**Generated**: 2026-09-02 (spec-sync re-run)
**Based on**: `drift-report.json` (regenerated 2026-09-02)
**Backup**: `.specify/sync/backups/20260902T212901Z/`

All 29 FR/SC requirements verified aligned against the current code. No FR/SC drift.
Six stale-documentation items were found in the supporting artifacts and resolved by
BACKFILL (docs updated to match tested, authoritative code). No ALIGN tasks, no
HUMAN_DECISION items, no code changes.

| Item | Artifact | Direction | Severity | Status |
|---|---|---|---|---|
| EDGE-SQ-FULL | spec.md (edge cases) | BACKFILL | minor | APPLIED |
| DM-FILE-PATH-TYPE | data-model.md | BACKFILL | minor | APPLIED |
| DM-PROVIDES | data-model.md | BACKFILL | minor | APPLIED |
| DM-RING-TYPE | data-model.md | BACKFILL | minor | APPLIED |
| DM-INFLIGHT-START | data-model.md | BACKFILL | moderate | APPLIED |
| DM-CONFIGURED-STATE | data-model.md | BACKFILL | minor | APPLIED |

---

## Proposal 1 — EDGE-SQ-FULL (BACKFILL)

**Direction**: BACKFILL (spec prose → matches code)

**Rationale**: The "io_uring submission queue full" edge case claimed the actor "MUST
back-pressure by waiting for completions before submitting new operations." The code
does not wait: on `ReadAsync`/`WriteAsync`, a failed SQE push is surfaced to the caller
as an error `Completion` (`Err(NotInitialized("io_uring submission queue full"))`), and
FR-002 already documents exactly this behavior. The edge-case bullet is the stale
artifact contradicting both FR-002 and the code → BACKFILL. (If true back-pressure were
ever desired, that is a new feature request, not a sync alignment — the current design
is deliberate and tested.)

**Code**: `src/actor.rs:469-480` (read), `src/actor.rs:588-601` (write).

**Files**: `specs/001-block-device-filesys/spec.md`

---

## Proposal 2 — DM-FILE-PATH-TYPE (BACKFILL)

**Direction**: BACKFILL. data-model.md listed `file_path: RwLock<Option<PathBuf>>`;
the field is `Mutex<Option<PathBuf>>` (`src/lib.rs:62`). Corrected to `Mutex`.

**Files**: `specs/001-block-device-filesys/data-model.md`

---

## Proposal 3 — DM-PROVIDES (BACKFILL)

**Direction**: BACKFILL. data-model.md listed `Provides: [IBlockDevice]`; the component
provides `[IBlockDevice, IBlockDeviceAdmin]` (`src/lib.rs:57`). Added `IBlockDeviceAdmin`.

**Files**: `specs/001-block-device-filesys/data-model.md`

---

## Proposal 4 — DM-RING-TYPE (BACKFILL)

**Direction**: BACKFILL. data-model.md listed `ring: IoUring` and omitted two fields;
the code has `ring: Option<IoUring>` (`None` on the sync-fallback path, FR-008),
`shutdown_requested: bool`, and (telemetry feature) `telemetry: Arc<TelemetryStats>`
(`src/actor.rs:109-120`). Also noted the concrete struct is `FilesysHandler`
(`src/actor.rs:109`), with "FilesysActor" retained as the conceptual name.

**Files**: `specs/001-block-device-filesys/data-model.md`

---

## Proposal 5 — DM-INFLIGHT-START (BACKFILL)

**Direction**: BACKFILL. data-model.md described the telemetry field as
`start_ns: u64` "computed as ~0 and never read on completion; see align-tasks
(telemetry-latency defect)." That defect was fixed (FR-019, 2026-08-07): the field is
`start: Instant` and `start.elapsed()` is recorded on the harvest path
(`src/actor.rs:103,822-823`). Corrected the field name/type and removed the stale
defect/align-task reference so data-model.md no longer contradicts FR-019.

**Files**: `specs/001-block-device-filesys/data-model.md`

---

## Proposal 6 — DM-CONFIGURED-STATE (BACKFILL)

**Direction**: BACKFILL. The "Configured" lifecycle step described
`set_file_path()`/`set_block_size()`/`set_num_blocks()` as the configuration mechanism.
Per FR-023 these are reserved crate-private `#[allow(dead_code)]` helpers, not the
config path; configuration is supplied at construction by `create(...)`. Updated the
prose to point at `create(...)` and cross-reference FR-023.

**Files**: `specs/001-block-device-filesys/data-model.md`

---

## Not proposed

- **ALIGN**: none. No code violates an agreed, correct requirement; every divergence
  found was stale documentation against working, tested code.
- **HUMAN_DECISION**: none. All items are unambiguous after reading the code.
