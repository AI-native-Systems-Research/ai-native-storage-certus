# Sync Apply Report

Applied: 2026-07-22T16:23:53Z (AUTO-BACKFILL mode)

Component: `tools/rdma-test`
Source: `.specify/sync/drift-report.{json,md}` (generated 2026-07-22T21:32:51Z)

## Changes Made

### Specs Updated (BACKFILL — code intentional/correct/tested, spec was stale)

| Spec | Requirement | Change Type | Notes |
|------|-------------|-------------|-------|
| 001-rdma-network-test | FR-005 / User Story 1 | Modified | Acceptance scenarios rewritten to use real `--test` values (`write`/`read`/`send`/`recv`/`all`) instead of the non-existent `throughput` value; added scenarios 4-7 covering Read/Send/Recv/All. |
| 001-rdma-network-test | FR-007 | Modified | `RDMA_TEST_PORT` default corrected from `50000` to `7471` in Implementation Details, matching contract/script/code. Example invocations' `--test throughput` corrected to `--test write`. |
| 001-rdma-network-test | FR-002 (new: FR-014, FR-015, FR-016, FR-017) | Added | Backfilled functional requirements for RDMA Read throughput, Send bandwidth, Recv bandwidth, and the full `--test` enum contract (`write`/`read`/`send`/`recv`/`latency`/`all`), plus an Implementation Details subsection documenting each variant's module and shared metrics. |
| 001-rdma-network-test | contracts/cli-interface.md | Modified | `--test` row updated to list real enum values; JSON output example expanded to show `write`/`read`/`send`/`recv`/`latency` result keys (omitted-when-unselected semantics documented); human-readable output contract annotated with per-test-kind heading labels; added a "Known gap" callout pointing at the FR-012 align task instead of rewriting the contract to match the bug. |
| 001-rdma-network-test | quickstart.md | Modified | `-t throughput` examples corrected to `-t write`; added a short note enumerating all six `--test` values. |
| 001-rdma-network-test | data-model.md | Modified | `TestConfig.test_type` enum corrected from `Enum(Throughput, Latency, All)` to `Enum(Write, Read, Send, Recv, Latency, All)`. |
| 001-rdma-network-test | tasks.md | Modified | Fixed stale `--test throughput` reference in US1's Independent Test; added Phase 8 (T037-T039) documenting the already-implemented Read/Send/Recv bandwidth work for traceability; added a note flagging that T032 ("partial results reporting") is checked off but not actually implemented (see align-tasks.md). |

Backups of all modified files were written to
`.specify/sync/backups/001-rdma-network-test/` before editing
(timestamp `20260722T162353Z`).

### New Specs Created

None. The three unspecced features (RDMA Read throughput, Send bandwidth,
Recv bandwidth) were extensions of the existing, single-spec CLI tool
(`001-rdma-network-test`) rather than standalone features, so they were
backfilled as new FRs (FR-014/FR-015/FR-016/FR-017) into that spec instead
of spawning new numbered spec directories — consistent with the drift
report's own "Suggested Spec" column, which pointed at extending
001-rdma-network-test in every case.

### Superseded

None.

### Implementation Tasks Generated (ALIGN — not applied to specs)

- 1 task in `.specify/sync/align-tasks.md`: **FR-012** (major) — the spec's
  documented connection-retry + partial-results-on-failure guarantee is not
  implemented in code (`src/rdma.rs`, `src/client.rs`, `src/main.rs`); this
  is a guarantee-violation, not stale documentation, so per the hard rules
  the spec was left describing the intended behavior and the gap was
  recorded as an implementation task instead of being papered over.

### Not Applied / Deferred

None deferred — all three drift items and all three unspecced items were
resolved (two via BACKFILL, one via ALIGN).

## Next Steps

1. Review the backfilled FR-014/FR-015/FR-016/FR-017 text and acceptance
   scenarios in `specs/001-rdma-network-test/spec.md` for accuracy.
2. Implement the FR-012 align task (connection-level retry + partial
   results reporting) per `.specify/sync/align-tasks.md`, then re-run
   spec-sync drift analysis to confirm FR-012 moves to Aligned.
3. Commit changes: `git add specs/ .specify/sync/ && git commit -m "docs(rdma-test): backfill spec-sync drift resolutions"`.
