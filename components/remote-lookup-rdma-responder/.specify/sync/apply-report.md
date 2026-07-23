# Sync Apply Report — `remote-lookup-rdma-responder`

**Mode**: AUTO-BACKFILL
**Applied**: 2026-07-22T23:21:11Z
**Source**: `.specify/sync/drift-report.{json,md}` (generated 2026-07-22T22:39:05Z)
**Backups**: `.specify/sync/backups/20260722T232111Z/{spec.md,plan.md}`

No `proposals.json` gate existed for this component (drift report only); per
AUTO-BACKFILL mode, resolutions below were derived and applied directly from
the drift report's findings and recommendations, respecting the hard rule that
only Markdown under `specs/**` and `.specify/sync/**` may be edited.

## Changes Made

### Specs Updated

| Spec | Section | Change Type | Reason |
|------|---------|--------------|--------|
| 001-rdma-lookup-responder/spec.md | Key Entities → `Endpoint` | Modified | Backfill/doc-fix: text said `ip` is "supplied by `set_bind_ip()` ... (never auto-detected)", contradicting FR-002a (auto-detect fallback) and its own Clarifications session in the same file. Rewritten to match FR-002a's resolution precedence. Found during this pass (not separately flagged in drift-report.json, but same root cause as the interfaces-crate conflict it does flag). |
| 001-rdma-lookup-responder/spec.md | New "Build & Feature Flags" section (before "Known Limitations") | Backfill | Drift-report unspecced-feature item + recommendation #4: names the `rdma` Cargo feature explicitly, documents the build-configuration `Bind` error as distinct from the FR-002/FR-010 runtime `Bind`/`Registration` failure modes. Also cross-referenced the pre-existing `telemetry` feature (FR-016) for completeness. |
| 001-rdma-lookup-responder/plan.md | "Testing" / "Target Platform" | Backfill (supporting) | Named `--features rdma` explicitly in the test-command list and cross-referenced the new spec.md section, per recommendation #4's "spec/plan/contract docs" latitude. |

### New Specs Created

None (NEW_SPEC: n/a per task scope).

### Superseded

None (SUPERSEDE: n/a per task scope).

### Implementation Tasks Generated

4 tasks appended to `.specify/sync/align-tasks.md` (file did not previously
exist — created fresh):

1. **FR-016** — wire `TelemetryCollector::record_accept_loop_error()` into a real failure branch (currently dead code; counter permanently 0). Severity: medium.
2. **Contract §3** — construct `ResponderEvent::Error` on the QP-creation-failure branch (`src/rdma.rs:373-382`, `accept_child`); currently the failure is silently `rdma_reject`ed with no signal to `remote-lookup`. Severity: medium.
3. **FR-014** — thread a logger handle into the accept-loop closure so `RealCmSeam` failure branches can emit `ILogger` diagnostics; today only `initialize()`/`shutdown()` log. Severity: low-medium.
4. **Interfaces doc comment** — `components/interfaces/src/iremote_lookup_rdma_responder.rs:256-262` (`set_bind_ip`/`initialize` on `IRemoteLookupRdmaResponderAdmin`) states the pre-clarification "never auto-detects" behavior, contradicting FR-002a and the shipped code. Flagged as an align-task rather than fixed directly — the file is a `.rs` source file outside this pass's writable scope (`components/interfaces/**`, not `specs/**`/`.specify/sync/**`).

Tasks 1–3 are code DEFECTS per the assignment framing (spec is correct/
load-bearing; code declares the surface but never exercises it on the real
failure path) — resolved by future code changes, not spec weakening. Task 4 is
a doc defect in a shared crate outside this component's spec-sync write scope.

### Not Applied / Deferred

| Item | Reason |
|------|--------|
| Editing `components/interfaces/src/iremote_lookup_rdma_responder.rs` directly | Out of scope for this spec-sync apply pass (source `.rs` file, not under `specs/**` or `.specify/sync/**`); filed as align-task #4 instead. |
| Wiring `record_accept_loop_error()`, `ResponderEvent::Error`, or `ILogger` calls into `src/rdma.rs`/`src/lib.rs`/`src/connection.rs` | Source-code changes are explicitly out of scope for spec-sync apply; filed as align-tasks #1-3. |

No proposal was rejected outright — every drift item in the report is
accounted for by either a spec/doc backfill (the unspecced `rdma` feature +
the same-doc stale Endpoint description) or an align-task (the three
error/diagnostics-path defects + the cross-doc stale comment). Nothing was
DEFERRED as "unsure" — all four remaining items had clear, unambiguous
resolutions per the hard rules given for this pass.

## Next Steps

1. Review the updated `spec.md` / `plan.md` sections above.
2. Implement the four tasks in `.specify/sync/align-tasks.md` in a follow-up code PR (tasks 1 and 2 should land together — both consume the same QP-creation-failure signal; task 4 touches the shared `components/interfaces` crate and should be checked for other stale copies of the same claim while there).
3. Commit spec changes: `git add components/remote-lookup-rdma-responder/specs/ components/remote-lookup-rdma-responder/.specify/sync/ && git commit -m "spec-sync: backfill rdma feature docs, align-tasks for responder error path"`.
