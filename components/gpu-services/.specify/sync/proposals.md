# Spec-Sync Proposals — gpu-services

**Generated**: 2026-09-02
**Based on**: `.specify/sync/drift-report.{json,md}` (generated 2026-09-02)
**Component**: gpu-services

## Summary

| Direction | Count |
|---|---|
| BACKFILL | 0 |
| ALIGN (task) | 0 |
| RESOLVED | 0 |
| BACKFILL-UNSPECCED | 0 |
| HUMAN_DECISION | 0 |

The 2026-09-02 drift report found all 78 requirements aligned across the three
specs (`drift_status: clean`). No proposals are required this run.

The only drift carried by earlier rounds — spec 003 FR-012 (MDTS ceiling) —
was backfilled into `specs/003-gpu-p2p-server/spec.md` on 2026-08-20 (FR-012
reworded to state the MDTS bound is an operator responsibility, plus US1
Acceptance Scenario 4 and an Assumptions bullet). This run re-verified that
resolution against `src/bin/p2p_server.rs` (`do_chunked_read` :273-323, CLI
help :54) and confirmed it still holds, so there is nothing further to propose.

---

## BACKFILL proposals

None this run (0 drift; the prior FR-012 backfill is already in the spec).

## ALIGN tasks

None this run (0 behavioral-bug drifts).

## Unspecced features

None this run (0 unspecced; all `dma.rs` / `gdrcopy_ffi.rs` public helpers are
spec-tracked via spec 001 FR-008 and spec 002 FR-021..024 + Auxiliary Public
Helpers).

## HUMAN_DECISION

None. (Non-blocking observation carried in the drift report: spec 003 is still
labeled "Draft (backfilled — needs human review)"; graduating that status is a
maintainer editorial decision, not a drift.)
