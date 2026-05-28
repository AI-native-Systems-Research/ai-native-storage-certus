# Drift Resolution Proposals

Generated: 2026-05-28
Based on: drift-report from 2026-05-28

## Summary

| Resolution Type | Count |
|-----------------|-------|
| Backfill (Code -> Spec) | 1 |
| Align (Spec -> Code) | 0 |
| Human Decision | 0 |

## Proposals

### Proposal 1: FR-003 — Lookup uses batch_lookup internally

**Direction**: BACKFILL
**Status**: APPROVED and APPLIED

**Current State**:
- Spec says: "execute the dispatcher's `lookup()` for each pair server-side"
- Code does: calls `disp.batch_lookup(&valid_batch)` for parallel cold promotion

**Resolution Applied**: Updated FR-003 to reflect that lookup uses `batch_lookup()` internally, noting this enables parallel cold promotion across drives while maintaining the same external contract (per-entry results).

**Rationale**: The `batch_lookup` optimization is a transparent internal change that significantly improves cold throughput (0.34 -> 5.58 GB/s). External behavior is identical — per-entry results in input order.
