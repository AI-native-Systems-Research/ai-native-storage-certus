# Sync Proposals — block-device-kernel

**Generated**: 2026-09-02 (Spec-Sync re-verify against HEAD `2fc1cd3c`)
**Drift source**: `.specify/sync/drift-report.{json,md}` (regenerated 2026-09-02 — 44 requirements checked, 42 aligned, 2 drifted, 0 not-implemented, 0 unspecced)
**Spec backup**: `.specify/sync/backups/20260902T212814Z/spec.md` (metadata-only edit this run)

This run supersedes the 2026-08-20 proposals. The 2026-08-20 FlushSync backfill (FR-027) is confirmed landed in the spec and aligned in code, so it is no longer a proposal. The only open item is the async-path telemetry-latency defect, unchanged at HEAD.

---

## Proposal 1 — FR-021 / SC-006 (ALIGN)

- **Requirement**: FR-021 (feature-gated telemetry tracks min/max/mean latency) and SC-006 (accurate `TelemetrySnapshot` when enabled)
- **Direction**: ALIGN (task only — no source edited)
- **Severity**: moderate | **Confidence**: HIGH
- **Location**: `src/actor.rs:776` (`harvest_completions`)
- **approved**: true

**Rationale**: Correct spec, buggy code. Fixed sites confirmed present at HEAD:
- `handle_read_sync` — `src/actor.rs:332` records `start.elapsed().as_nanos()`
- `handle_write_sync` — `src/actor.rs:397`
- `write_zeros` — `src/actor.rs:637`
- `wait_for_cqe` (blocking async completion) — `src/actor.rs:718` records `op.start.elapsed().as_nanos()`

But the primary async-completion path `harvest_completions()` still calls `record_op(0, op.bytes)` (`src/actor.rs:776`) with a hardcoded `0`, even though `InflightOp` carries a populated `start: Instant` (`src/actor.rs:101`, set at `:480` and `:554`). Async `ReadAsync`/`WriteAsync` completions therefore record 0 ns latency, driving `min_latency_ns` to 0 (`src/telemetry.rs:41-52`) and skewing the mean. A genuine behavioral violation of a correct, agreed requirement → ALIGN, not BACKFILL. A single task covers both FR-021 and SC-006 (identical root cause).

**Before**: FR-021 / SC-006 requirement text correct and unchanged. Spec header note already honestly documents the residual async defect.

**After**: FR-021 / SC-006 text unchanged. No `.rs` source modified. Standing ALIGN task re-affirmed with a 2026-09-02 re-verification note in `align-tasks.md`.

**Deliverable**: `align-tasks.md` → "2026-09-02 re-verify" section (re-affirms the 2026-08-20 Phase B task; still open).

---

## Summary

| Direction | Count |
|-----------|-------|
| BACKFILL (drifted requirement) | 0 |
| BACKFILL-UNSPECCED | 0 (FlushSync/FR-027 already landed 2026-08-20) |
| ALIGN task | 1 (FR-021 + SC-006, single task — still open) |
| RESOLVED | 0 |
| HUMAN_DECISION | 0 |
