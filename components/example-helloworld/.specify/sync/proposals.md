# Proposals: example-helloworld

**Generated**: 2026-09-02T21:28:34Z
**Based on**: drift-report.md / drift-report.json (status: CLEAN)

No drift, not-implemented items, or unspecced features were found. All 10
requirements (FR-1..7, NFR-1..3) plus both user stories and the success criteria
are aligned with concrete `file:line` evidence. No BACKFILL or ALIGN proposals
are required in this run.

| ID | Type | Description | Approved |
|----|------|-------------|----------|
| (none) | — | No proposals — implementation and spec are in sync. | n/a |

## Carried-over items (not new proposals)

These pre-existing items remain tracked and are unchanged by this run:

- **align-tasks Task 1** (HUMAN_DECISION → deferred code change): make
  `apps/helloworld-mainline` actually wire `ILogger` via
  `GreeterHandler::with_logger(...)`. The spec text was already corrected on
  2026-07-22 to describe current (logger-less) app behavior, so this is not an
  active drift — it is the preferred code follow-up only. Out of scope for
  spec-sync apply (never edits code).
- **align-tasks Task 2** (informational): open design decision on whether to
  promote `IGreeter` to the shared `interfaces` crate. No action.
