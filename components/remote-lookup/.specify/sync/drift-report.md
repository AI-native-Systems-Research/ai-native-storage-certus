# Spec Drift Report

Generated: 2026-07-15
Project: remote-lookup (feature 002-remote-lookup-rdma)

## Summary

| Category | Count |
|----------|-------|
| Specs Analyzed | 1 |
| Requirements Checked | 26 (FR-001..FR-026) |
| ✓ Aligned | 25 |
| ⚠️ Drifted | 1 |
| ✗ Not Implemented | 0 |
| 🆕 Unspecced Code | 0 |

## Detailed Findings

### Spec: 002-remote-lookup-rdma — Remote Lookup over Zyre + RDMA

#### Aligned ✓

- FR-001 `IRemoteLookup` + `batch_lookup(&[(CacheKey,u32)])` (+ `initialize(LookupConfig)`) → `src/lib.rs`
- FR-002 actor on a dedicated thread owning the zyre node → `src/actor.rs::run`
- FR-003 join group on activation / leave on deactivate → `initialize`, `leave_cluster`, `ActorState::shutdown`
- FR-004 `(key,size)` wire identity → `src/wire.rs`
- FR-005 SHOUT KEY_QUERY, split by `max_keys_per_query` → `ActorState::on_submit`
- FR-006 / FR-006a landing slot + publish-on-success, greedy per-KEY_RESPONSE fetch → `on_key_response`, `publish_success`
- FR-007 RDMA_REQUEST carries requester endpoint + pool rkey + slot → `on_key_response`
- FR-008 publish on RDMA_STATUS(Success) → `publish_success`
- FR-010 Phase-1→Phase-2 transition on quorum% or `phase1_timeout` (whichever first), then cached-reply disk re-scan → `src/actor.rs::advance` (`Operation::quorum_reached` + `phase1_deadline`), fired time-driven by `tick_deadlines`
- FR-011 bounded retry to alternates (memory then disk) → `try_retry`
- FR-012 completion (all-satisfied / nothing-left / retry-cap / deadline) → `advance`, `finalize`, `tick_deadlines`
- FR-013 peer Exit handling → `on_exit`
- FR-014 teardown-before-reclaim (Disconnect → DisconnectAck; orphan slots) → `on_exit`, `teardown_peer`, `finalize`
- FR-015 KEY_QUERY classification → `src/server.rs::classify_query`
- FR-016 / FR-017 serve + disk promotion + promotion-failure → `src/server.rs::serve_rdma_request`
- FR-018 framing → `src/wire.rs`
- FR-019 stale op_id discarded → `on_key_response`, `on_rdma_status`
- FR-020 concurrent ops → `ActorState::ops`
- FR-021 self-SHOUT filter → `handle_key_query`
- FR-022 config (`quorum_pct`, `phase1_timeout`, `op_deadline`, `max_keys_per_query` all consulted) → `LookupConfig`
- FR-023 receptacles → `define_component!` block
- FR-024 no direct RDMA (delegates to initiator/responder) → server/client paths
- FR-025 responder wiring + cached endpoint/rkey → `initialize`
- FR-026 single-flight → `in_flight` index in `src/actor.rs`

#### Drifted ⚠️

- **FR-009 (reclaim timing on failure status)** — Severity: **minor / wording ambiguity**
  - Spec: on RDMA_STATUS(UnableToConnect | KeyNoLongerAvailable) the actor "MUST NOT yet reclaim the landing slot if a late write could still be in flight (see FR-014)."
  - Actual: `on_rdma_status` reclaims the slot immediately (`memory_tier.remove`) on a failure status.
  - Location: `src/actor.rs::on_rdma_status`
  - Assessment: consistent with `contracts/wire-protocol.md` ("free a slot exposed to a peer only on (1) RDMA_STATUS received, or (2) peer Exit after DisconnectAck") — a received status means the peer's push attempt is complete, so no late write is in flight. FR-009's prose reads more conservatively than the contract; spec-internal ambiguity, not a safety gap.

### Unspecced Code 🆕

None. (`wire` is a public module for the codec bench; `seams` is public test support — both spec-anticipated.)

## Inter-Spec Conflicts

- FR-009 (spec.md) vs `contracts/wire-protocol.md` on when a slot exposed to a **live** peer may be reclaimed after a failure status (see FR-009 finding).

## Recommendations

1. **FR-010 / FR-022**: RESOLVED via option (a) — `advance` now transitions Phase-1→Phase-2 at the first of quorum% (`Operation::quorum_reached`) or `phase1_timeout` (`phase1_deadline`, fired time-driven by `tick_deadlines`), with `op_deadline` as the backstop. `quorum_pct` and `phase1_timeout` are now consulted. Covered by `tests/mesh.rs::phase1_timeout_triggers_disk_fallback_without_waiting_for_slow_peer`.
2. **FR-009**: reword to state that a received RDMA_STATUS (success *or* failure) is itself a safe reclaim point (the peer's push is complete); the "no reclaim yet" rule applies to the *timeout* path (no status), which the orphan mechanism already covers.
