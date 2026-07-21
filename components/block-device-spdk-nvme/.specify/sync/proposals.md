# Drift Resolution Proposals

Generated: 2026-07-21
Based on: drift-report 2026-07-21

## Summary

| Resolution Type | Count |
|-----------------|-------|
| Backfill (Code -> Spec) | 1 |
| Align (Spec -> Code) | 0 |
| Human Decision | 0 |
| New Specs | 0 |

## Proposals

### Proposal 1: 001-spdk-nvme-block-device/FR-026 (new)

Direction: BACKFILL (Code -> Spec)

Current State:
- Spec says: nothing about completion-delivery back-pressure. FR-002/FR-014 only
  say each client has a callback channel and the actor polls all clients.
- Code does: the single-threaded actor delivers completions non-blocking
  (try_send + per-client FIFO backlog drained each poll cycle), so a client that
  stops draining its ring cannot head-of-line-block delivery to other clients.

Proposed Resolution (add FR-026):
- FR-026: The actor MUST deliver completions to a client's callback channel
  without blocking. Because a single actor thread serves all clients on a
  controller, it MUST NOT block delivering a completion to one client, as that
  would head-of-line-block completion delivery to every other client on the same
  controller. Completions that cannot be delivered immediately (callback ring
  full) MUST be buffered per-client in FIFO order and retried on subsequent poll
  cycles. Per-client backlog is bounded by that client's outstanding operations.

Also note in the Completion Callback entity: `Completion` derives `Clone`.

Rationale: Non-blocking delivery is a tested fix for a whole-drive deadlock
observed under concurrent multi-client cold-read load. Code is authoritative.

Confidence: HIGH

Action:
- [ ] Approve
- [ ] Reject
- [ ] Modify
