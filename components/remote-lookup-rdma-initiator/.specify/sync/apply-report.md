# Sync Apply Report

Component: remote-lookup-rdma-initiator
Mode: AUTO-BACKFILL
Applied: 2026-07-22T23:21:48Z
Source: `.specify/sync/drift-report.{json,md}` (generated 2026-07-22T21:31:30Z)

## Changes Made

### Specs Updated

| Spec | Section | Change Type |
|------|---------|-------------|
| 002-rdma-push-initiator | Boundary with `remote-lookup` | Added note on `set_local_peer_id` / peer-correlation half of the responder's teardown-before-reclaim contract |
| 002-rdma-push-initiator | Functional Requirements | Added **FR-015** (`set_local_peer_id(peer: PeerId)` connect-stamping) |
| 002-rdma-push-initiator | Key Entities | Added **PeerId** entity |
| 002-rdma-push-initiator | Edge Cases | Added bullet: push/connect called before `set_local_peer_id` is set |

Backfill source: `interfaces/src/iremote_lookup_rdma_initiator.rs:161-168` (5th
interface method, already shipped) + `src/lib.rs:81,114-120,223-228`,
`src/connection.rs:440-461`, `rdma.rs:386-495` (private_data stamping on connect).

### Spec Superseded (no-op — already applied)

| Spec | Action |
|------|--------|
| 001-rdma-remote-request-handler | Verified: already carries the `⚠️ SUPERSEDED (2026-07-09)` banner, `Status: Superseded by spec-002`, and `Superseded By: 002-rdma-push-initiator` pointer. No further edit made — this is the correct end state per the hard rule (banner + pointer to 002, no align-tasks for its 23 by-design not-implemented FRs/SCs). |

### New Specs Created

_None._ (`NEW_SPEC: none` per task directions.)

### Implementation / Doc Tasks Generated

2 tasks appended to `.specify/sync/align-tasks.md`:

1. **Align 002-rdma-push-initiator/SC-004** — stale `<5%` pass/fail wording in
   `benches/push_telemetry.rs:1-18` header comment vs. the spec's revised
   "small fixed absolute cost / ZST-when-off" criterion. Severity: **minor**
   (doc-comment only; `.rs` file, out of scope to edit here — code change, not
   applied).
2. **DEFERRED** — whether `tests/mr_registration_bench.rs` (unspecced hardware
   investigation informing FR-004) should get a Known-Limitations line in
   spec-002. Directions did not specify a resolution; left as a human decision
   rather than guessed.

### Not Applied

| Item | Reason |
|------|--------|
| Spec-001's 23 not-implemented FRs/SCs (FR-001..FR-017, SC-001..SC-006) | By-design: role wholesale-replaced by spec-002; explicitly excluded from align-tasks per task directions |
| `tests/mr_registration_bench.rs` spec note | Ambiguous scope (two valid options in drift report, no direction given) — deferred to align-tasks.md rather than resolved |

## Backups

- `.specify/sync/backups/001-spec.md.bak` (pre-supersede content; pre-existing from an earlier sync-apply pass)
- `.specify/sync/backups/002-spec.md.bak` (pre-backfill content, created this pass)

## Next Steps

1. Review the FR-015 / PeerId backfill in `specs/002-rdma-push-initiator/spec.md`.
2. Implement align-task 1 (bench header comment fix) as a normal code change —
   out of scope for this Markdown-only sync-apply.
3. Resolve the deferred `mr_registration_bench.rs` documentation question
   (align-tasks.md task 2) with a human decision.
4. Re-run `speckit.sync.analyze` to confirm the backfill closes the "Unspecced
   Code" item for `set_local_peer_id`.

---

# 2026-08-07 Sweep

Applied: 2026-08-07 on branch `sync/spec-drift-sweep-20260807`
Mode: Interactive drift sweep (`component-sync-specs`, all specced components)
Source: `.specify/sync/drift-report.{json,md}` (regenerated 2026-08-07T15:31:02Z).

Drift headline: **spec-002 (current, RDMA Push Initiator) shows zero drift** —
all 23 FRs/SCs map to code + tests. All 9 drifted + 11 not-implemented items
belong to the correctly-superseded spec-001 and are expected by design. The two
exceptions are spec-001's **stale self-annotations** (FR-014, FR-015), which now
describe the *opposite* of the current code; those were annotated in place this
pass. Three unspecced behaviors + one supersession-id mismatch were addressed by
backfill.

## Changes Made (branch only, nothing committed to `unstable`)

### Specs Updated (BACKFILL / archival correction — applied)

| Spec | Requirement | Change |
|------|-------------|--------|
| 002-rdma-push-initiator | `Supersedes:` header | Corrected id `001-rdma-remote-lookup-rdma-initiator` → `001-rdma-remote-request-handler` (matched the real directory name; fixes the Conflicts[1] chain-resolution break) |
| 002-rdma-push-initiator | Status header | Added 2026-08-07 re-sweep note |
| 002-rdma-push-initiator | FR-015 | Appended the after-build no-op clarification: `local_peer_id` is snapshotted when the connection table is lazily constructed on the first `push`/`connect`; a later `set_local_peer_id` is silently ignored (Recommendation 5) |
| 002-rdma-push-initiator | Assumptions | Backfilled the `rdma` Cargo feature note: real transport is feature-gated; a no-`rdma` build returns `NotInitialized` from `push`/`push_async`/`connect` (Recommendation 3) |
| 002-rdma-push-initiator | Known Limitations | Backfilled the validation-tooling item: `src/loopback_test.rs` (hardware loopback) and `tests/mr_registration_bench.rs` (MR-registration sweep) are deliberately unspecced engineering tooling (Recommendation 4; resolves the July DEFERRED align-task 2) |
| 001-rdma-remote-request-handler | FR-014 | Annotated the stale self-annotation as **false**: telemetry IS integrated (`connection.rs:1025,968,1064,380`). Retained to mark the drift (Recommendation 1) |
| 001-rdma-remote-request-handler | FR-015 | Annotated the stale self-annotation as **false**: trait methods are functional, not `NotInitialized` stubs; no `serve` module exists (Recommendation 1) |

### Code

_None._ Spec-002 shows zero drift; no source change is warranted. The only
open code item is the July SC-004 bench-comment align-task (below), still queued.

### Implementation Tasks (see align-tasks.md 2026-08-07 section)

- July align-task 1 (SC-004 stale `<5%` bench header comment,
  `benches/push_telemetry.rs:1-18`): **still queued, not drafted** — carried
  forward unchanged (Low, doc-comment only).
- July align-task 2 (`mr_registration_bench.rs` Known-Limitations note):
  **RESOLVED this pass** by the Known-Limitations backfill above; the human
  decision defaulted to "document as validation tooling."

### Not Applied / Deferred

None deferred. Spec-001's remaining 9 drifted + 11 not-implemented items are the
intentional, documented supersession (no action — role wholesale-replaced by
spec-002). The stale FR-014/FR-015 annotations were corrected rather than
deleted, preserving the archival record while flagging them as non-authoritative.

## Backups

- `.specify/sync/backups/001-spec.md.bak`, `002-spec.md.bak` (pre-existing; the
  July pass copies. The 2026-08-07 edits build on those; branch history on
  `sync/spec-drift-sweep-20260807` is the recovery path for this pass.)

## Next Steps

1. Implement the SC-004 bench-comment fix (align-task 1) at maintainer discretion.
2. Consider fully retiring spec-001 to an archive path (Recommendation 1) — the
   in-place annotations are the interim mitigation.
3. Commit on the branch only — never to `unstable`.

---

# 2026-09-02 Re-analysis Apply Report

Applied: 2026-09-02T21:46:01Z
Component: remote-lookup-rdma-initiator
Git commit: 2fc1cd3c
Inputs sha256: 5f0bd4af9625e093efa50899b01081b2c33e96551f1671c2cd0644dde395a610 (stamp-time value; see the drift report's concurrency caveat — a parallel interfaces sync was in flight)
Source: `.specify/sync/drift-report.{json,md}` (regenerated 2026-09-02T21:46:01Z)
Mode: verify-and-reclassify (Markdown-only; no code edits)

## Headline

Spec-002 shipped behavior is fully aligned (22/23 FRs/SCs verified at
`file:line`; component `src/` unchanged since 2026-07-30). The remaining item is
the SC-004 benchmark doc-comment ALIGN task, still open — so this pass reports
`drift_status: drift` honestly, correcting the 2026-08-07 "clean" headline that
had folded the same still-open task under it.

## Changes Made

### Specs Updated (BACKFILL)

_None._ Spec-002 text is accurate against the implementation; the SC-004 drift is
code→spec (a stale comment), so there is nothing to backfill into the spec.

### Code

_None._ The only actionable item is a `.rs` doc-comment fix, out of scope for a
Markdown-only apply.

### Align Tasks

Appended a `2026-09-02 Re-analysis` section to `align-tasks.md` re-confirming
Task A (SC-004 bench header comment) as still open and carried forward. No new
align work surfaced.

## Backups

None created this pass — no spec `.md` file was modified. (Pre-existing backups:
`.specify/sync/backups/001-spec.md.bak`, `002-spec.md.bak`.)

## Not Applied / Deferred

| Item | Reason |
|------|--------|
| SC-004 bench-comment fix (`benches/push_telemetry.rs:1-18`) | `.rs` file — out of scope for Markdown-only apply; queued in align-tasks.md Task A |
| Spec-001's 23 superseded FRs/SCs | By design — role wholesale-replaced by spec-002; stale FR-014/FR-015 self-annotations already annotated in place |

## Next Steps

1. Land align-task A (bench header comment reword) as an ordinary code change.
2. Consider retiring spec-001 to an archive path (in-place annotations are the
   interim mitigation).
3. Re-run the hash tool if `src/`, `specs/`, or the interface change after this
   report.
