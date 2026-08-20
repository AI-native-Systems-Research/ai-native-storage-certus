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

---

# 2026-08-07 Sweep (branch `sync/spec-drift-sweep-20260807`)

Two ALIGN (spec→code) items surfaced against the design-of-record spec 002.
Per the sweep pacing, non-HIGH ALIGN items are **queued as tasks, not drafted**
(only HIGH code bugs get a drafted fix). Both are queued below. All five
unspecced behaviors were BACKFILLED into spec 002 (FR-030…FR-034) and applied;
the two stale `src/lib.rs` docstrings were corrected in place (doc-only,
matching shipped code — see apply-report.md).

## Task 3 — Log unknown wire frames (002 FR-018, Low)

- **Severity**: low
- **Spec**: `002-remote-lookup-rdma` FR-018 — "Unknown message types MUST be logged and ignored."
- **Source**: drift-report 2026-08-07 → spec 002 FR-018 Drifted (Low).
- **Current code**: `on_wire`'s `WireMessage::Unknown => {}` (`src/actor.rs:330`) silently drops
  unknown frames — framing/echo are correct and the frame is ignored, but no log line is emitted,
  so the "logged" half of the requirement is unmet.
- **Required change**: emit a `logger.debug`/`warn` (via the actor's `ILogger`) on the
  `WireMessage::Unknown` arm before dropping the frame. Trivial, low-risk; not drafted this pass
  to keep source churn scoped to the drafted HIGH fixes elsewhere in the sweep.
- **Files**: `components/remote-lookup/src/actor.rs:330`.
- **Owner**: remote-lookup maintainer.
- **Superseded by the 2026-08-20 task below** (widened to cover the malformed-decode arm too).

## Task 4 — (resolved this pass) stale lib.rs docstrings (002 FR-001, Medium)

- **Severity**: medium (doc-vs-code) — **APPLIED in this sweep, not deferred.**
- The `src/lib.rs` module header (`:5-8`) and `batch_lookup` doc (`:251-253`) claimed the
  KEY_QUERY→RDMA protocol was unbuilt and that `batch_lookup` finalizes every key as `NotFound`.
  That contradicted the fully-implemented `actor.rs` and spec 002's `Synced` status. Both
  docstrings were rewritten in place to describe the shipped behavior (protocol implemented,
  `Ok(())` ⇒ resident in local memory tier; the uninitialized-no-actor path still returns
  `NotFound`, which is what the existing doctest exercises). `cargo build -p remote-lookup` clean.
  Logged here for the audit trail; no further action required.

---

# 2026-08-20 Sweep (Phase B — current pending drift report)

One ALIGN item against the design-of-record spec 002. Five spec-001 items were
BACKFILLED (superseded-placeholder annotations) and three unspecced behaviors
were BACKFILLED into spec 002 (FR-006, FR-014, FR-018) and applied — see
`apply-report.md`. No source was modified.

## Task: Align 002-remote-lookup-rdma/FR-018 (log unknown AND malformed wire frames)

- **Severity**: low
- **Spec Requirement**: FR-018 — unknown message types and malformed/truncated frames MUST be
  **logged and ignored** (the ignore half is met; the logged half is not). This 2026-08-20 pass
  widened FR-018's text (and this task) to cover **both** the unknown-`msg_type` arm and the
  malformed-decode arm; it supersedes the narrower 2026-08-07 Task 3 (unknown arm only).
- **Current Code** (`components/remote-lookup/src/actor.rs`, `on_wire`):
  - `Err(_) => return,` at `:314` — malformed/truncated frame dropped silently (no log).
  - `WireMessage::Unknown { .. } => {}` at `:330` — unknown `msg_type` dropped silently (no log).
- **Required Change**: emit a `logger` line (debug or warn, via the actor's `ILogger`) on **both**
  arms before dropping the frame — e.g. log the peer id and byte length on the malformed arm, and
  the peer id plus the unknown tag/version on the `Unknown` arm. Ignore behavior is unchanged; only
  a log call is added. Do not abort the poll loop.
- **Files to Modify**: `components/remote-lookup/src/actor.rs:314,330` (plus a possible unit test in
  `src/` / `tests/` asserting a log is emitted for an unknown-tag and a truncated frame).
- **Estimated Effort**: trivial (~2 log lines + optional test); low-risk.

### Acceptance Criteria

- [ ] An inbound frame with an unrecognized `msg_type` produces a log line and is dropped; the poll
      loop continues.
- [ ] A malformed/truncated frame that fails `WireMessage::decode` produces a log line and is
      dropped; the poll loop continues.
- [ ] No behavioral change beyond logging (frames are still ignored; no `op_id` processing; no panic).
- [ ] `cargo build -p remote-lookup` and existing tests remain green.
- **Owner**: remote-lookup maintainer (source/test change — out of scope for this Markdown-only spec-sync pass).
