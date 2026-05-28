# Sync Proposals: Dispatcher Component

**Generated**: 2026-05-28
**Spec**: `components/dispatcher/specs/001-dispatcher-cache-interface/spec.md`
**Status**: All 3 proposals approved and applied

## Summary

| Resolution Type | Count |
|-----------------|-------|
| Backfill (Code -> Spec) | 3 |
| Align (Spec -> Code) | 0 |
| Human Decision | 0 |

## Proposals

### Proposal 1: FR-001 — Add `batch_lookup` to method list

**Direction**: BACKFILL
**Status**: APPROVED and APPLIED

**Current State**:
- Spec says: interface provides `initialize`, `shutdown`, `lookup`, `lookup_async`, `check`, `remove`, `populate`, `prepare_store`, `commit_store`, `cancel_store`, and `touch`
- Code does: additionally provides `batch_lookup`

**Resolution Applied**: Updated FR-001 to include `batch_lookup` in the method enumeration. Added FR-039 with full `batch_lookup` semantics.

---

### Proposal 2: FR-019 — Parameterized pipeline queue depth

**Direction**: BACKFILL
**Status**: APPROVED and APPLIED

**Current State**:
- Spec says: "up to 16 concurrent NVMe reads"
- Code does: accepts `max_queue_depth` parameter (16 for single-entry, 16/num_queues for batch)

**Resolution Applied**: Updated FR-019 to describe the parameterized `max_queue_depth` and multi-queue sharing strategy.

---

### Proposal 3: New User Story 11 — Parallel Batch Cold Promotion

**Direction**: NEW_SPEC (backfill)
**Status**: APPROVED and APPLIED

**Resolution Applied**: Added User Story 11 describing batch parallel cold promotion with per-drive thread groups, multi-queue threads, reduced queue depth, and acceptance scenarios. Added SC-014 measuring batch throughput improvement.
