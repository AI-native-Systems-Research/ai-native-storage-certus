# Spec-Sync Apply Report: extended-metadata-store

**Applied**: 2026-08-07
**Branch**: `sync/spec-drift-sweep-20260807` (all changes; nothing committed to `unstable`)
**Source**: `.specify/sync/drift-report.{json,md}` (generated 2026-08-07T15:31:25Z)
**Backups**: `.specify/sync/backups/20260807T160256Z/{spec-001.md, spec-002.md}`
(prior AUTO-BACKFILL pass backups remain at `.../20260722T232035Z/`)

## User Decisions Driving This Pass

- **P1 (workspace membership / ALIGN-001)** = **Keep deferred** — no code change; documented.
- **P2 (`force_flush` / ALIGN-002)** = **Draft code fix** — drafted on branch.
- **P3 (capacity + persistence/interface tests)** = **Backfill 002 spec.**
- **P4 (plan/tasks docs + dead code)** = **Reconcile docs + add spec note.**

## Changes Made

### Code Drafted on Branch (ALIGN — see `align-tasks.md`)

| Item | Direction | Status | Files |
|------|-----------|--------|-------|
| FR-05 `force_flush` durability (ALIGN-002) | ALIGN (HIGH) | **Drafted** — compiles in default build; unit-test verification blocked (see below) | `components/extended-metadata-store/src/lib.rs` (`FlushTrigger` alias, `flush_trigger` field, `attach_flush_trigger`, rewritten `force_flush`) |
| Workspace membership + stale mock (ALIGN-001) | ALIGN (MAJOR) | **Deferred** by user decision — task kept open, documented | (none drafted) |

### Specs Updated (BACKFILL / reconcile — applied directly)

**001-extended-metadata-store/spec.md**
| Location | Change |
|----------|--------|
| Header | Added "Last Synced 2026-08-07" note summarizing this sweep. |
| FR-05 status | "Partially Implemented" → "Fix drafted (branch …)" — `force_flush` now delegates to an installed flush trigger. |
| Known Gaps | Rewrote FR-05 gap to describe the drafted trigger mechanism, and the two independent reasons verification is deferred (crate outside workspace; pre-existing `MockBlockDevice` missing `read_write_stats`). |
| Known Gaps | Added a "Dead public API surface" note documenting `region_capacity_bytes()`, `create_test_component_from_state()`, and the `CapacityExhausted`/`StorageError` variants (StorageError now produced by the drafted `force_flush`). |

**002-ssd-integration-test/spec.md**
| Location | Change |
|----------|--------|
| Header | Added "Last Synced 2026-08-07" note. |
| US3 scenario 1/2 | Reworded to "made durable"; added a sync note that durability is currently via the internal `flush_to_disk` path, to move to `force_flush()` once the FR-05 fix is verified. |
| US5 scenario 2 + Edge Cases | Reworded to describe capacity exhaustion as a **flush-time** error, with a sync note explaining `test_capacity_exhaustion` never reaches the exhaustion branch. |
| FR-007 | Annotated: durability currently via internal wiring path (consequence of FR-05 no-op); re-point at interface once fix verified. |
| FR-011 | Annotated: interface-only usage holds for put/get/delete/iterate_all; creation/durability use inherent APIs as a direct consequence of the FR-05 no-op. |
| Functional Requirements | Added a "Capacity note" documenting flush-time-only enforcement and the two resolution options; this spec documents actual behavior (option b). |

**002-ssd-integration-test/plan.md**
| Location | Change |
|----------|--------|
| Phase 1 Prerequisites | "IBlockDevice and IPartitionTable receptacles" → `ILogger` receptacle only; block I/O wired via `BlockDeviceClient`; partitions via `DiskPartitionManager`. Added a sync note. |
| Phase 2 step 1 | Dev-dependency `console-logger` → `logger`. |

**002-ssd-integration-test/tasks.md**
| Location | Change |
|----------|--------|
| T001 | `console-logger` → `logger` (+ sync note). |
| T007 | `create_store_instance()` → `create_store`; "wires IBlockDevice and IPartitionTable receptacles" → `ILogger` receptacle + manually-constructed `BlockDeviceClient` (+ sync note). |
| External Dependencies | Reconciled receptacle wording to `ILogger` + `BlockDeviceClient`/`DiskPartitionManager`. |
| Notes | `create_store_instance(ctx)` → `create_store(ctx)`. |

## Verification

- `cargo build -p extended-metadata-store` (default, in-memory) — **clean**;
  header doc-test passes. (Confirmed via throwaway workspace wiring since the
  crate is not a normal member; the throwaway wiring — a temporary
  `interfaces` re-export and a temporary `Cargo.toml` member entry — was
  reverted with `git checkout` per the "Keep deferred" decision, so the branch
  does NOT include workspace/interface changes.)
- `cargo test -p extended-metadata-store --features testing` — **not run**:
  blocked by a pre-existing, unrelated defect (`MockBlockDevice` missing
  `read_write_stats` at `src/test_support.rs`) and by the crate being outside
  the workspace. Both tracked under ALIGN-001.

## Not Applied / Deferred

| Item | Reason |
|------|--------|
| ALIGN-001 (workspace membership) | User chose "Keep deferred" this sweep. |
| Mock refresh (`read_write_stats`) | Bundled with ALIGN-001; pre-existing defect, out of this sweep's approved scope. |
| Surfacing `CapacityExhausted` via the interface | User chose to backfill the 002 spec to flush-time behavior instead of a code change. |

## Next Steps

1. Review the drafted `force_flush` change and the spec/plan/tasks edits on the branch.
2. When ALIGN-001 is un-deferred: add the crate to the workspace, refresh the
   mock, re-run `speckit.sync.analyze`, and verify the `force_flush` fix under
   `--features testing`.
3. Commit on `sync/spec-drift-sweep-20260807` (do NOT commit to `unstable`).
