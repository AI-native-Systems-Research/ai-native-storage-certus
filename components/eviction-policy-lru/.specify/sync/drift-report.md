# Spec Drift Report

Generated: 2026-06-19
Project: eviction-policy-lru

## Summary

| Category | Count |
|----------|-------|
| Specs Analyzed | 1 |
| Requirements Checked | 15 (11 FR + 4 NFR) |
| ✓ Aligned | 12 (80%) |
| ⚠️ Drifted | 3 (20%) |
| ✗ Not Implemented | 0 (0%) |
| 🆕 Unspecced Code | 1 |

## Detailed Findings

### Spec: 001-lru-eviction-policy - LRU Eviction Policy Component

#### Aligned ✓
- FR-001: `create_pool()` returns sequential PoolIds → `src/lib.rs:41-48`
- FR-002: `track(pool, key)` registers key, returns EvictionHandle → `src/lib.rs:50-63`
- FR-003: `touch(handle)` moves to MRU position in O(1) → `src/lib.rs:65-74`
- FR-004: `remove(handle)` unlinks entry in O(1) → `src/lib.rs:76-85`
- FR-005: `pop_oldest(pool)` removes and returns LRU key → `src/lib.rs:87-92`
- FR-006: `peek_oldest(pool, n)` returns up to n oldest keys → `src/lib.rs:94-103`
- FR-007: `len(pool)` returns active entry count → `src/lib.rs:105-114`
- FR-008: `clear_pool(pool)` resets pool to empty → `src/lib.rs:116-123`
- FR-011: Free-list recycling for removed slots → `src/lru_list.rs:38-55`
- NFR-001: All single-entry operations are O(1) → linked-list operations are constant-time
- NFR-002: Thread-safe via RwLock + per-pool Mutex → `src/lib.rs:9,23-25`
- NFR-003: Per-pool locking granularity → each pool is `Mutex<Pool>`, read-lock on outer state

#### Drifted ⚠️

- **FR-009**: Spec says "Operations on an invalid PoolId MUST return `EvictionPolicyError::InvalidPool`"
  - Actual: Only `track`, `touch`, and `remove` return `Result` and can report `InvalidPool`. `pop_oldest` returns `None`, `peek_oldest` returns empty `Vec`, `len` returns `0`, `clear_pool` silently no-ops.
  - Location: `src/lib.rs:87-123`
  - Severity: **moderate** — spec overstates error reporting vs. actual graceful-degradation behavior

- **FR-010**: Spec references "`remove` and `move_to_back` on an already-removed entry"
  - Actual: The public interface exposes `touch`, not `move_to_back`. The method `move_to_back` is an internal `LruList` detail.
  - Location: `src/lib.rs:65-74` (touch delegates to move_to_back)
  - Severity: **minor** — spec uses internal name instead of public API name

- **NFR-004**: Spec says component declares `ILogger` receptacle
  - Actual: The receptacle is declared but never used — no log calls exist anywhere in the implementation.
  - Location: `src/lib.rs:32`
  - Severity: **minor** — dead receptacle, no behavioral impact

#### Not Implemented ✗

(None — all requirements have corresponding implementation)

### Success Criteria Drift

- **SC-001**: Spec says "8 unit tests in `lib.rs` and 12 unit tests in `lru_list.rs`"
  - Actual: 9 tests in `lib.rs` and 13 tests in `lru_list.rs` (22 total)
  - Severity: **minor** — spec undercounts; tests are a superset

### Unspecced Code 🆕

| Feature | Location | Lines | Suggested Action |
|---------|----------|-------|------------------|
| `ILogger` receptacle (declared but unused) | `src/lib.rs:32` | 1 | Either add logging or remove receptacle |

## Inter-Spec Conflicts

None — single spec in this component.

## Recommendations

1. **Update FR-009** to distinguish between `Result`-returning methods (which report `InvalidPool`) and `Option`/scalar-returning methods (which gracefully degrade).
2. **Update FR-010** to use public API name `touch` instead of internal method name `move_to_back`.
3. **Update SC-001** to remove hard-coded test counts or replace with "All tests pass".
4. **Decide on ILogger**: Either add meaningful log calls (e.g., pool creation, clear events) or remove the unused receptacle declaration.
