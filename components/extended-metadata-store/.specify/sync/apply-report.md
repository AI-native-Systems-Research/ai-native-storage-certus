# Spec-Sync Apply Report: extended-metadata-store

**Applied**: 2026-09-02T21:41:34Z
**Git commit**: 2fc1cd3c
**Source**: `.specify/sync/drift-report.{json,md}`, `.specify/sync/proposals.{json,md}`
**Backups**: `.specify/sync/backups/20260902T214134Z/{spec.md,plan.md,tasks.md}`
**Inputs sha256**: c9722d87a830647222f54f6550ec5f51c9751bad91036748e822863f1d2cd1b0

## Summary counts

| Outcome | Count |
|---------|-------|
| BACKFILL applied | 3 (spec.md FR-11 annotation, plan.md, tasks.md) |
| ALIGN tasks (net new) | 1 (ALIGN-EMS-003) |
| ALIGN tasks (carried forward) | 2 (ALIGN-EMS-001, ALIGN-EMS-002) |
| HUMAN_DECISION | 1 (missing 002 spec) |
| not_implemented | 0 |

## Specs Updated

### 001-extended-metadata-store/spec.md
| Item | Change type | Change |
|------|-------------|--------|
| FR-11 | BACKFILL (status annotation) | Status "Implemented" -> "Partial — timer trigger only; dirty-threshold trigger NOT wired (`src/flush.rs:61,68` field inert)". Requirement text preserved as the target. |
| Known Gaps | BACKFILL | New entry documenting the FR-11 dirty-threshold gap and its ALIGN-EMS-003 tracking. |

### 001-extended-metadata-store/plan.md
| Item | Change type | Change |
|------|-------------|--------|
| Data Flow note (~L104) | BACKFILL | Removed stale "force_flush() is an unconditional no-op" claim; now describes the `attach_flush_trigger`/`FlushTrigger` durable path (`src/lib.rs:201-215`). |
| Running Tests note (~L160) | BACKFILL | Removed stale "crate is not a workspace member" claim; now notes membership at `Cargo.toml:23,105`. |

### 001-extended-metadata-store/tasks.md
| Item | Change type | Change |
|------|-------------|--------|
| T056 | BACKFILL | "blocked on ALIGN-001 (crate not in workspace)" -> no longer blocked (ALIGN-001 resolved). |
| Known Code Defects | BACKFILL | ALIGN-001/ALIGN-002 marked RESOLVED; added references to ALIGN-EMS-001/002/003. |

## Align Tasks
| Task | Req | Severity | Status | Files to modify (by implementer) |
|------|-----|----------|--------|----------------------------------|
| ALIGN-EMS-003 | 001/FR-11 | moderate | NEW this sweep | `src/flush.rs` (+ possibly `src/lib.rs`), `tests/persistence.rs` |
| ALIGN-EMS-001 | FR-011 (ex-002) | moderate | carried forward | `tests/integration_ssd.rs` |
| ALIGN-EMS-002 | FR-007 (ex-002) | moderate | carried forward | `src/lib.rs` and/or `src/flush.rs` |

## HUMAN_DECISION (not resolved)
- Missing spec `002-ssd-integration-test`: restore from backup or fold into 001 and renumber. Recorded in `.specify/sync/align-tasks.md` and the drift report.

## Not modified
- No `.rs` source was edited. No interface files touched. `cargo` not run.
- Only files under `components/extended-metadata-store/` (specs/*.md and .specify/sync/**) were changed.
