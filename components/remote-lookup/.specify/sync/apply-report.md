# Sync Apply Report

Applied: 2026-07-22
Mode: AUTO-BACKFILL
Source: `.specify/sync/drift-report.{json,md}` (generated 2026-07-22T22:40:55Z)

## Backups

Pre-edit copies of both spec files saved under `.specify/sync/backups/`:

- `001-spec.md.2026-07-22.bak`
- `002-spec.md.2026-07-22.bak`

(A stale `spec.md.2026-06-19.bak` from a prior sync run predates this pass and was left as-is.)

## Changes Made

### Specs Updated

| Spec | Item | Change Type | Resolution |
|------|------|--------------|------------|
| 001-remote-lookup-placeholder | Status header | Modified | `Synced (2026-06-19)` → `Superseded (2026-07-22)`; was mis-stamped — code implements 002, not this placeholder. |
| 001-remote-lookup-placeholder | New "Supersession Notice" section | Added | Banner pointing to `002-remote-lookup-rdma/spec.md`; documents that the `batch_lookup` `IpcHandle`→`u32` divergence (FR-001/SC-002 drift) is intentional per 002's Clarification Q1, not a defect. |
| 002-remote-lookup-rdma | Status header | Modified | `Draft` → `Synced (2026-07-22)`, noting the two backfills below. |
| 002-remote-lookup-rdma | FR-023 (receptacles) | Backfilled | Added the missing `responder_admin: IRemoteLookupRdmaResponderAdmin` receptacle to the list (code, `data-model.md`, and `contracts/iremote_lookup.md` already required it — only FR-023's prose omitted it). |
| 002-remote-lookup-rdma | FR-025 (responder wiring) | Backfilled | Named the `responder_admin` / `responder` receptacles explicitly, for consistency with the FR-023 fix. |
| 002-remote-lookup-rdma | FR-009 (client, failure) | Backfilled | Reclaim-timing prose rewritten to match `contracts/wire-protocol.md`'s ordering & safety invariants: a landing slot may be reclaimed on any RDMA_STATUS receipt (success or failure) while the peer is a live member; the FR-014 `DisconnectAck` gate applies only to the peer-departure path (no status received at all). Code already followed the contract; only the FR-009 text was out of date. |
| 002-remote-lookup-rdma | New FR-029 (out-of-interface lifecycle/test hooks) | Backfilled | Documents `RemoteLookupComponent::peers_seen()`, `::signal_shutdown()`, `::shutdown()` as intentional, non-`IRemoteLookup` `pub fn`s needed for multi-actor zyre/czmq teardown ordering and test discovery; not part of the interface contract. |

### New Specs Created

None (`NEW_SPEC: none` per instructions).

### Align/Defect/Ambiguous Tasks Generated

Appended to `.specify/sync/align-tasks.md`:

- **Task 1** (medium): missing `tests/mesh.rs` coverage for User Story 7 / `tasks.md` T025
  (peer-Exit: cached-reply drop, in-progress→unsatisfied transition, `DisconnectAck`-gated slot
  reclaim). Implementation reviewed as correct; test-only gap, out of scope for this Markdown-only
  pass.
- **Task 2** (low, monitoring only): watch that `peers_seen()`/`signal_shutdown()`/`shutdown()`
  stay test/teardown-only; promote to a real interface/admin receptacle if a production caller
  outside teardown ordering appears.

### Deferred Items

None. Every drift item in the report had an explicit resolution directive (SUPERSEDE 001, BACKFILL
FR-023/FR-009/three methods on 002, ALIGN-task the US7 test gap) and was applied; nothing was
ambiguous enough to require deferral.

## Not Applied

None — no proposal was rejected; all drift items had an explicit, unambiguous resolution.

## Files Touched

- `specs/001-remote-lookup-placeholder/spec.md` (superseded banner + status)
- `specs/002-remote-lookup-rdma/spec.md` (FR-023, FR-025, FR-009 backfills; new FR-029; status)
- `.specify/sync/align-tasks.md` (created)
- `.specify/sync/apply-report.md` (this file, overwrites the 2026-06-19 report from a prior sync
  run against 001 only)
- `.specify/sync/backups/001-spec.md.2026-07-22.bak`, `002-spec.md.2026-07-22.bak` (created)

No source code under `src/` or any other non-Markdown file was modified, per the hard rule
restricting this pass to `specs/**` and `.specify/sync/**` Markdown.

## Next Steps

1. Review the backfilled FR-009/FR-023/FR-029 text in `specs/002-remote-lookup-rdma/spec.md` against
   the actual code (`src/lib.rs`, `src/actor.rs`) to confirm wording precision.
2. Assign Task 1 in `align-tasks.md` (T025 mesh test) to an implementation pass.
3. Commit: `git add specs/ .specify/sync/ && git commit -m "sync: apply drift resolutions (auto-backfill) for remote-lookup"`
