# Apply Report: eviction-policy-lru

Applied: 2026-09-02T21:32:00Z
Backup: `.specify/sync/backups/2026-09-02T21:29:45Z/` (spec.md, plan.md, prior reports)

## Spec changes applied (BACKFILL)

| Proposal | Target | Change |
|----------|--------|--------|
| P1 (D1) | spec.md FR-009 | Added `batch_touch` to the list of Result-returning methods that report `InvalidPool`. |
| P2 (D2) | spec.md Dependencies | Added `eviction-replay-benchmark` to the Consumers list. |
| P2 (D2) | plan.md Consumer Graph | Added `eviction-replay-benchmark (trace-replay benchmark harness)`. |
| P2 (D2) | plan.md Testing | Corrected test counts: lru_list 12→13, lib.rs 8→9; noted missing batch_touch test. |

## Align tasks recorded (NOT applied — code changes)

| Proposal | Target | Task |
|----------|--------|------|
| P3 (A1) | src/lib.rs | Add dedicated `batch_touch` tests. Appended to `align-tasks.md`. |

## Human decisions (left open in drift-report.md)

| Proposal | Target | Decision needed |
|----------|--------|-----------------|
| P4 (H1) | FR-002 / interface | Interface documents idempotent re-registration; LRU always appends. Implement dedup vs relax interface doc. Interfaces out of scope to edit here. |

## Not modified

- `components/interfaces/**` — out of scope (never edited).
- No source code edited.

## Notes

- Prior NFR-004 align task (wire ILogger) is satisfied by current code; recorded in `align-tasks.md`.
- `inputs_sha256` unchanged by these edits — the hash tool does not walk `.specify/specs/` (spec-location quirk).
