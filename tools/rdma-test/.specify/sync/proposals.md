# Spec-Sync Proposals

Generated: 2026-09-02T21:46:49Z
Component: `tools/rdma-test`
Source: `.specify/sync/drift-report.{md,json}` (2026-09-02)

The 2026-07-22 backfill resolved all major/minor drift and unspecced code
(FR-014–FR-017 added, port default reconciled, primary `--test` value set
corrected). This pass finds one remaining requirement-level drift (FR-012,
already an align task) and three residual stale `throughput` references.

| # | Item | Proposal | Rationale | Approved |
|---|------|----------|-----------|----------|
| P1 | FR-012 — connection retry + partial results not implemented | HUMAN_DECISION / ALIGN (existing) | Guarantee-violation, not stale docs. Pre-existing align task in `align-tasks.md`; spec correctly documents intended behavior. No new action beyond keeping the task open. | true |
| P2 | D1 — `tasks.md:61` US1 checkpoint used `-t throughput` | BACKFILL | Invalid enum value; code is correct (`write`). Stale documentation left over from prior backfill. Edit `.md`. | true |
| P3 | D2 — `quickstart.md:74` `jq .results.throughput...` | BACKFILL | Nonexistent JSON key; actual key is `write`. Stale documentation. Edit `.md`. | true |
| P4 | D3 — `launch.sh:11` comment `--test throughput` | ALIGN | Invalid enum value in a code file (shell script). Cannot be fixed by spec-only backfill; recorded as a trivial align task. | true |

## Notes

- P2 and P3 are confident BACKFILLs (code is the source of truth and is
  correct; only the docs are stale) and were applied this pass.
- P1 is unchanged: it remains the single open align task; the spec continues
  to describe the target behavior per spec-sync hard rules.
- P4 is a new, trivial align task (comment-only, no behavioral impact).
