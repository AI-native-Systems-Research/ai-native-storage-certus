# Sync Proposals — block-device-kernel (Spec-Sync Phase B)

**Generated**: 2026-08-20
**Policy**: `.specify/sync/PHASE_B_POLICY.md`
**Drift source**: `.specify/sync/drift-report.{json,md}` (regenerated 2026-08-20 — 43 requirements checked, 41 aligned, 2 drifted, 0 not-implemented, 1 unspecced)
**Spec backup**: `.specify/sync/backups/001-block-device-kernel.spec.md.20260820T171219Z.bak`

This run supersedes the 2026-08-07 proposals; it reflects the freshly regenerated drift report, which shows the 2026-08-07 telemetry-latency fix only partially landed.

---

## Proposal 1 — FR-021 / SC-006 (ALIGN)

- **Requirement**: FR-021 (feature-gated telemetry tracks min/max/mean latency) and SC-006 (accurate `TelemetrySnapshot` when enabled)
- **Direction**: ALIGN (task only — no source edited)
- **Severity**: moderate | **Confidence**: HIGH
- **Location**: `src/actor.rs:776` (`harvest_completions`)

**Rationale**: Correct spec, buggy code. The 2026-08-07 fix landed only partially. Fixed sites confirmed present in the current code:
- `handle_read_sync` — `src/actor.rs:332` records `start.elapsed().as_nanos()`
- `handle_write_sync` — `src/actor.rs:397`
- `write_zeros` — `src/actor.rs:637`
- `wait_for_cqe` (blocking async completion) — `src/actor.rs:718` records `op.start.elapsed().as_nanos()`

But the primary async-completion path `harvest_completions()` still calls `record_op(0, op.bytes)` (`src/actor.rs:776`) with a hardcoded `0`, even though `InflightOp` carries a populated `start: Instant` (`src/actor.rs:101`, set at `:480` and `:554`). Async `ReadAsync`/`WriteAsync` completions therefore record 0 ns latency, driving `min_latency_ns` to 0 and skewing the mean whenever async IO is present. This is a genuine behavioral violation of a correct, agreed requirement → ALIGN, not BACKFILL. A single task covers both FR-021 and SC-006 (identical root cause), per the per-component policy note.

**Before**: FR-021 requirement text is correct and unchanged. The header Last-Synced note (2026-08-07) incorrectly claimed the telemetry defect was fully fixed and "those requirements now hold as written."

**After**: FR-021 text unchanged. Header Last-Synced note corrected to record that the fix is partial (sync paths + `wait_for_cqe` fixed; `harvest_completions` still records 0 ns for async ops), that FR-021/SC-006 do NOT hold for async IO, and that the residual defect is tracked as an ALIGN task. No `.rs` source modified.

**Deliverable**: `align-tasks.md` → "2026-08-20 Phase B" section.

---

## Proposal 2 — FlushSync handler (BACKFILL-UNSPECCED → new FR-027)

- **Requirement**: `Command::FlushSync` handler — unspecced working feature
- **Direction**: BACKFILL-UNSPECCED
- **Severity**: low | **Confidence**: HIGH
- **Location**: `src/actor.rs:233-247`

**Rationale**: `process_command` handles `Command::FlushSync { ns_id }`: for `ns_id == 1` it returns `Completion::FlushDone { Ok(()) }` as a validated no-op (the device is opened `O_DIRECT | O_DSYNC`, so there is no volatile write cache to drain); `ns_id != 1` returns `InvalidNamespace`. The kernel spec never mentioned `FlushSync`. Working, intentional behavior mirroring `block-device-filesys` FR-022 (which instead issues a real `fdatasync(2)`).

**Before**: No requirement mentioning `FlushSync`; US2 acceptance scenarios ended at scenario 4 (`NsProbe`).

**After**: New **FR-027** documenting `FlushSync` as a validated no-op justified by `O_DIRECT | O_DSYNC` durability, with `ns_id != 1` → `InvalidNamespace`, explicitly contrasted with filesys FR-022's real `fdatasync`. New US2 acceptance scenario 5 exercising `FlushSync{ns_id:1}` → `FlushDone(Ok)` and `ns_id != 1` → `InvalidNamespace`.

**Deliverable**: applied to `specs/001-block-device-kernel/spec.md` (FR-027 + US2 scenario 5).

---

## Summary

| Direction | Count |
|-----------|-------|
| BACKFILL (drifted requirement) | 0 |
| BACKFILL-UNSPECCED | 1 (FR-027 FlushSync) |
| ALIGN task | 1 (FR-021 + SC-006, single task) |
| RESOLVED | 0 |
| HUMAN_DECISION | 0 |
