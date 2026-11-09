# Drift Resolution Proposals

Generated: 2026-06-19
Based on: drift-report from 2026-06-19

## Summary

| Resolution Type | Count |
|-----------------|-------|
| Backfill (Code → Spec) | 3 |
| Align (Spec → Code) | 1 |
| Human Decision | 0 |
| New Specs | 0 |
| Remove from Spec | 0 |

## Proposals

### Proposal 1: 001-lru-eviction-policy/FR-009

**Direction**: BACKFILL
**Status**: APPROVED

**Current State**:
- Spec says: "Operations on an invalid PoolId MUST return EvictionPolicyError::InvalidPool"
- Code does: Only Result-returning methods report InvalidPool. Others gracefully degrade.

**Proposed Resolution**:
- **FR-009**: Methods returning `Result` (`track`, `touch`, `remove`) MUST return `EvictionPolicyError::InvalidPool` when given a non-existent pool. Methods returning `Option` or scalar (`pop_oldest`, `peek_oldest`, `len`, `clear_pool`) MUST gracefully degrade: returning `None`, empty collection, `0`, or no-op respectively.

**Rationale**: Interface signatures inherently cannot carry errors for non-Result methods. Code behavior is intentional and well-tested.

**Confidence**: HIGH

---

### Proposal 2: 001-lru-eviction-policy/FR-010

**Direction**: BACKFILL
**Status**: APPROVED

**Current State**:
- Spec says: "`remove` and `move_to_back` on an already-removed entry MUST be idempotent"
- Code does: Public API uses `touch`, not `move_to_back`.

**Proposed Resolution**:
- **FR-010**: `touch` and `remove` on an already-removed handle MUST be idempotent (no panic, no effect).

**Rationale**: Spec used internal method name; behavior requirement is correct, terminology needs correction.

**Confidence**: HIGH

---

### Proposal 3: 001-lru-eviction-policy/NFR-004 (ILogger)

**Direction**: ALIGN (Spec → Code)
**Status**: APPROVED

**Current State**:
- Spec says: Component MUST conform to Certus component model with receptacle for ILogger
- Code does: ILogger receptacle declared but never used

**Proposed Resolution**:
Add trace-level logging for key lifecycle events:
- `create_pool()`: log pool creation with assigned ID
- Error paths: log invalid pool/handle errors

**Rationale**: User chose to make the receptacle useful rather than removing it.

**Confidence**: HIGH

---

### Proposal 4: 001-lru-eviction-policy/SC-001

**Direction**: BACKFILL
**Status**: APPROVED

**Current State**:
- Spec says: "All 8 unit tests in lib.rs and 12 unit tests in lru_list.rs pass"
- Actual: 9 tests in lib.rs, 13 in lru_list.rs (22 total)

**Proposed Resolution**:
- **SC-001**: All tests in `lib.rs` and `lru_list.rs` pass (`cargo test -p eviction-policy-lru`).

**Rationale**: Hard-coded counts become stale when tests are added.

**Confidence**: HIGH
