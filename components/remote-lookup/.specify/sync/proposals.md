# Drift Resolution Proposals

Generated: 2026-06-19
Based on: drift-report from 2026-06-19

## Summary

| Resolution Type | Count |
|-----------------|-------|
| Backfill (Code → Spec) | 5 |
| Align (Spec → Code) | 0 |
| Human Decision | 0 |
| New Specs | 0 |
| Remove from Spec | 0 |

## Proposals

### Proposal 1: 001-remote-lookup-placeholder/FR-003

**Direction**: BACKFILL

**Current State**:
- Spec says: "When connected, each entry in batch_lookup MUST produce a log message via the ILogger receptacle"
- Code does: Logging is unconditional — no connection state exists. Logs for every entry always.

**Proposed Resolution**:

Update FR-003 to:
> FR-003: Each entry in `batch_lookup` MUST produce a log message via the `ILogger` receptacle (placeholder behavior). If the ILogger receptacle is not bound, logging is silently skipped.

**Rationale**: Interface simplified — connect/disconnect removed per user direction. Code is authoritative.

**Confidence**: HIGH

**Action**:
- [x] Approve

---

### Proposal 2: 001-remote-lookup-placeholder/FR-005

**Direction**: BACKFILL (Replace)

**Current State**:
- Spec says: "When not connected, batch_lookup MUST return Err(RemoteLookupError::NotConnected) for every entry"
- Code does: NotConnected variant removed. No connection state.

**Proposed Resolution**:

Replace FR-005 with:
> FR-005: The `batch_lookup` method MUST be callable at any time after component instantiation. No connection precondition is required.

**Rationale**: connect/disconnect model replaced with join_cluster/leave_cluster. NotConnected error variant removed.

**Confidence**: HIGH

**Action**:
- [x] Approve

---

### Proposal 3: 001-remote-lookup-placeholder/FR-008

**Direction**: BACKFILL (Clarify)

**Current State**:
- Spec says: "Component MUST expose functionality only through the IRemoteLookup interface"
- Code does: join_cluster and leave_cluster are interface methods (compliant)

**Proposed Resolution**:

No text change to FR-008. It is satisfied — join_cluster/leave_cluster are interface methods. Add FR-009/FR-010 to document them.

**Confidence**: HIGH

**Action**:
- [x] Approve (no change needed)

---

### Proposal 4: Unspecced — join_cluster

**Direction**: BACKFILL (Add requirement)

**Feature**: join_cluster
**Location**: components/interfaces/src/iremote_lookup.rs:66

**Proposed Resolution**:

Add new requirement:
> FR-009: The `IRemoteLookup` interface MUST expose a `join_cluster` method with signature `fn join_cluster(&self, endpoint: &str) -> Result<(), RemoteLookupError>`. The placeholder implementation MUST log the endpoint and return `Ok(())`.

**Confidence**: HIGH

**Action**:
- [x] Approve

---

### Proposal 5: Unspecced — leave_cluster

**Direction**: BACKFILL (Add requirement)

**Feature**: leave_cluster
**Location**: components/interfaces/src/iremote_lookup.rs:80

**Proposed Resolution**:

Add new requirement:
> FR-010: The `IRemoteLookup` interface MUST expose a `leave_cluster` method with signature `fn leave_cluster(&self) -> Result<(), RemoteLookupError>`. The placeholder implementation MUST log the call and return `Ok(())`.

**Confidence**: HIGH

**Action**:
- [x] Approve
