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
