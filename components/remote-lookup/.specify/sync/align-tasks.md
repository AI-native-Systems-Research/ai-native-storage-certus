# Align Tasks

Tasks generated from spec-sync (AUTO-BACKFILL) review, `components/remote-lookup`. These are
follow-up work items surfaced during drift resolution that are out of scope for a spec-only edit
(implementation/test work) or require a decision the spec-sync pass deferred.

## Task 1 — Add the missing US7 peer-Exit mesh test (tasks.md T025)

- **Severity**: medium (test-coverage gap, not a missing feature)
- **Spec**: `002-remote-lookup-rdma`
- **Source**: drift-report.json → specs[1].not_implemented[0]; tasks.md line 280 (`T025`, still
  unchecked `[ ]`)
- **What**: `tasks.md` T025 calls for a `tests/mesh.rs` scenario covering User Story 7 (peer
  departure): (1) a zyre `Exit` for a peer drops that peer's cached `PeerReply`, returns its
  in-progress keys to unsatisfied, and re-evaluates completion; (2) an in-flight landing slot
  exposed to that peer is not returned to the allocator until `ResponderEvent::DisconnectAck` is
  received (backs SC-005/SC-006). The corresponding implementation
  (`ActorState::on_exit`/`::teardown_peer` in `src/actor.rs`) exists and was reviewed as correct
  against FR-013/FR-014, but no automated test exercises it.
- **Action**: write the T025 mesh test(s) in `tests/mesh.rs` per the two acceptance scenarios in
  spec.md's User Story 7, following the existing pattern used by T010/T018/T020/T022/T030/T032.
- **Owner**: remote-lookup maintainer (implementation/test change — out of scope for this
  Markdown-only spec-sync pass).

## Task 2 — Confirm out-of-interface lifecycle hooks stay test/teardown-only

- **Severity**: low
- **Spec**: `002-remote-lookup-rdma`
- **Source**: drift-report.json → unspecced[0..2] (`peers_seen`, `signal_shutdown`, `shutdown`)
- **What**: FR-029 (backfilled in this pass) documents `peers_seen`/`signal_shutdown`/`shutdown` as
  intentionally outside the `IRemoteLookup` contract, needed for zyre/czmq multi-actor teardown
  ordering and test discovery barriers. No spec or code change is required now, but if any new
  component starts depending on `peers_seen()` for non-test logic, promote it into a proper
  interface method (or an admin-style receptacle, mirroring `responder_admin`) rather than growing
  ad hoc `pub fn`s on `RemoteLookupComponent`.
- **Action**: none required at this time; revisit if a production caller of `peers_seen()` appears
  outside `apps/certus-server` teardown-ordering use.
- **Owner**: remote-lookup maintainer (monitoring item).
