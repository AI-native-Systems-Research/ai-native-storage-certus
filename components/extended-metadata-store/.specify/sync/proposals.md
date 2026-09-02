# Spec-Sync Proposals: extended-metadata-store

**Generated**: 2026-09-02T21:41:34Z
**Based on**: `.specify/sync/drift-report.{json,md}`
**Git commit**: 2fc1cd3c

| ID | Spec/Req | Direction | Severity | Approved |
|----|----------|-----------|----------|----------|
| P1 | 001/FR-11 | ALIGN + spec status annotation | moderate | true |
| P2 | plan.md | BACKFILL | low | true |
| P3 | tasks.md | BACKFILL | low | true |
| P4 | 001/FR-011 (ex-002) | ALIGN (carry-forward) | moderate | true |
| P5 | 001/FR-007 (ex-002) | ALIGN (carry-forward) | moderate | true |
| P6 | missing 002 spec | HUMAN_DECISION | moderate | n/a |

## P1 — FR-11 dirty-threshold trigger not implemented (NEW) — ALIGN
`FlushConfig::dirty_threshold` (default 100, `src/flush.rs:61,68`) is configurable
but never read; the worker loop (`src/flush.rs:172-208`) flushes only on the timer
or on explicit `trigger_flush()`. The threshold trigger promised by FR-11 / US6 is
absent and the field is inert. **Do not relax the spec.** File a code-side task to
wire the threshold (`ALIGN-EMS-003`) and annotate FR-11's status in `spec.md` to
"Partial (timer only)" with a Known-Gaps entry, keeping the requirement as the
target. Applied: spec status annotated; ALIGN-EMS-003 filed.

## P2 — plan.md stale claims — BACKFILL
`plan.md` claimed `force_flush()` is an unconditional no-op (contradicted by
`src/lib.rs:201-215`) and that the crate is not a workspace member (contradicted by
`Cargo.toml:23,105`). Both are spec-lag against merged code. Applied: both notes
rewritten to current reality (backup taken first).

## P3 — tasks.md stale defects — BACKFILL
`tasks.md` listed ALIGN-001/ALIGN-002 as open MAJOR defects and T056 as blocked on
ALIGN-001; all resolved. Applied: marked resolved and cross-referenced the current
ALIGN-EMS-001/002/003 tasks.

## P4 — SSD test uses internal APIs for durability — ALIGN (carry-forward)
`tests/integration_ssd.rs` obtains durability via internal APIs rather than
`force_flush()`. Now that FR-05 is implemented, move durability onto the interface.
Task `ALIGN-EMS-001` (already filed) stands. No spec relaxed; no code edited.

## P5 — CapacityExhausted never surfaced — ALIGN (carry-forward)
`put()` enforces only `ValueTooLarge`; region capacity is a flush-time `String`
error never mapped to `CapacityExhausted`. Task `ALIGN-EMS-002` (already filed)
stands. No spec relaxed; no code edited.

## P6 — Missing spec 002-ssd-integration-test — HUMAN_DECISION
The 001 spec and ALIGN tasks reference `002-FR-007`/`002-FR-011`, but the 002 spec
is absent from the repo (backups only). Decide: (A) restore from
`.specify/sync/backups/specs/002-ssd-integration-test/spec.md.bak`, or (B) fold the
requirements into 001 and renumber. Left in report + align-tasks; not resolved.
