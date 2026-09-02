# Drift Resolution Proposals

Generated: 2026-09-02T21:32:00Z
Based on: drift-report from 2026-09-02

## Summary

| Resolution Type | Count |
|-----------------|-------|
| Backfill (Code → Spec) | 2 |
| Align (Spec → Code) | 1 |
| Human Decision | 1 |
| New Specs | 0 |
| Remove from Spec | 0 |

## Proposals

### Proposal 1 (D1): 001-lru-eviction-policy/FR-009

**Direction**: BACKFILL
**Status**: APPROVED

**Current State**:
- Spec says: Result-returning methods are `track`, `touch`, `remove`.
- Code does: `batch_touch` also returns `Result` and reports `InvalidPool` (`src/lib.rs:98,109`).

**Proposed Resolution**:
- **FR-009**: extend the Result-method list to `track`, `touch`, `remove`, `batch_touch`.

**Rationale**: `batch_touch` (FR-012) has the same error contract; FR-009 was simply incomplete.

**Confidence**: HIGH

---

### Proposal 2 (D2): 001-lru-eviction-policy/Dependencies + plan

**Direction**: BACKFILL
**Status**: APPROVED

**Current State**:
- Spec/plan list 7 consumers; `apps/eviction-replay-benchmark` is a real consumer and was omitted. Plan test counts (8/12) are stale.

**Proposed Resolution**:
- Add `eviction-replay-benchmark` to spec Dependencies and plan Consumer Graph.
- Correct plan test counts to 9 (lib.rs) / 13 (lru_list.rs).

**Rationale**: `apps/eviction-replay-benchmark/Cargo.toml` depends on the crate and instantiates `EvictionPolicyLruComponent`; counts verified by `cargo test`.

**Confidence**: HIGH

---

### Proposal 3 (A1): 001-lru-eviction-policy/FR-012 — batch_touch test coverage

**Direction**: ALIGN (Spec → Code)
**Status**: PENDING (recorded in align-tasks.md; not auto-applied)

**Current State**:
- `batch_touch` is implemented (`src/lib.rs:89-115`) but has no dedicated test.

**Proposed Resolution**:
Add tests covering: single-pool amortized touch reorders entries; multi-pool
handle slice relocks correctly; empty slice returns `Ok(())`; invalid pool
returns `InvalidPool`.

**Rationale**: Hot-path method with no direct assertions.

**Confidence**: HIGH

---

### Proposal 4 (H1): 001-lru-eviction-policy/FR-002 — track() idempotent re-registration

**Direction**: HUMAN_DECISION
**Status**: OPEN (not applied)

**Current State**:
- Interface `ieviction_policy.rs:84-86` documents idempotent re-registration of an
  already-tracked key (refresh recency, return existing handle, no new node).
- LRU impl `src/lib.rs:69` always `push_back` — no dedup; duplicate keys create
  duplicate nodes.

**Options**:
- (a) Implement dedup in `eviction-policy-lru` to honor the interface contract (code change).
- (b) Relax the interface doc to make idempotent re-registration policy-optional and
  document LRU's always-append behavior in FR-002 (interface edit is OUT OF SCOPE for this workflow).

**Rationale**: Substantive contract divergence; interfaces are not editable here and
the correct direction depends on intended semantics for duplicate tracking.

**Confidence**: N/A (requires human)
