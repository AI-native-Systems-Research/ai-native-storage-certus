# Sync Apply Report

Applied: 2026-09-02T21:46:49Z
Component: `tools/rdma-test`
Source: `.specify/sync/proposals.{json,md}` + `.specify/sync/drift-report.{json,md}` (2026-09-02)

## Context

This is a follow-up pass to the 2026-07-22 AUTO-BACKFILL, which added
FR-014/FR-015/FR-016/FR-017 and reconciled the port default and the primary
`--test` value set. Re-verifying every FR/SC against the current code found:

- 22 / 23 requirements Aligned.
- 1 Drifted: **FR-012** (major) — already tracked as an align task; unchanged.
- 0 Unspecced features (all now covered by FR-014–FR-017).
- 3 residual stale `throughput` references left over from the prior backfill.

## Changes Made

### Specs Updated (BACKFILL — code correct, docs stale)

| Spec file | Location | Change |
|-----------|----------|--------|
| `quickstart.md` | line 74 | `jq .results.throughput.bandwidth_gbps` → `jq .results.write.bandwidth_gbps` (D2). `results.throughput` is not a JSON key the tool emits; the correct key is `write`. |
| `tasks.md` | line 61 | US1 checkpoint `-t throughput` (×2) → `-t write` (D1). `throughput` is not a valid `--test` value. |

Backups written to
`.specify/sync/backups/20260902T214649Z/` before editing
(`quickstart.md.20260902T214649Z.bak`, `tasks.md.20260902T214649Z.bak`).

### Implementation Tasks (ALIGN — not applied to specs)

| Task | Severity | File | Status |
|------|----------|------|--------|
| FR-012 — connection-level retry + partial-results reporting | Major | `src/rdma.rs`, `src/client.rs`, `src/main.rs`, `tasks.md` T032 | Pre-existing in `align-tasks.md`; unchanged this pass |
| launch.sh comment `--test throughput` (D3) | Minor | `scripts/launch.sh:11` | **New** align task appended to `align-tasks.md` |

### New Specs Created

None.

### HUMAN_DECISION

None outstanding. FR-012's disposition (ALIGN vs. relaxing the requirement)
remains the one open judgment call, already documented in `align-tasks.md`
(item 4 of the Required Change list).

### Not Applied / Deferred

- D3 (`launch.sh:11`) is a code-file edit and per spec-sync rules is recorded
  as an align task rather than applied by this spec-only pass.

## Next Steps

1. Implement the FR-012 align task, then re-run spec-sync to confirm FR-012
   moves to Aligned.
2. Apply the trivial `scripts/launch.sh:11` comment fix (D3).
3. `drift_status` will remain `drift` until FR-012 is resolved.
