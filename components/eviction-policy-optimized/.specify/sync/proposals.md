# Drift Resolution Proposals: eviction-policy-optimized

Generated: 2026-10-31T04:54:40Z
Based on: drift-report from 2026-10-31T04:54:40Z (clippy commit `55a4ddd4`)

| Resolution Type | Count |
|---|---|
| Backfill (Code → Spec) | 1 |
| Align (Spec → Code) | 0 |
| Human Decision | 0 |

## B-1: specs/001-optimized-eviction-policy/spec.md / all line citations

**Direction**: BACKFILL (citation only)

- Spec says: `src/lib.rs` citations past line 74 (US1 AS1-3, US3, US4, FR-001 to FR-011, FR-013, NFR-002, NFR-005, SC-002) and `src/lru_list.rs` citations in FR-009 and FR-012
- Code is now at: each mapped by the diff to its HEAD line (+37 before `create_pool`, +42 after it, +10 in `lru_list.rs`); citations at or before line 73 (`CMS_*`, `CountMinSketch`, `Pool`, `EvictionState`) did not move

**Rationale**: 55a4ddd4 inserted lines above the cited code; the code is unchanged, so the code is authoritative.

**Confidence**: HIGH

**Action**: [x] Approve (rule: BACKFILL of code already on the branch)

## Not proposed

- The spec does not say that a poisoned lock panics. This is pre-existing and unchanged by `55a4ddd4`. Recorded in the drift report only.
