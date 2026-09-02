# Alignment Tasks

Generated: 2026-06-19

## Task: Align 001-lru-eviction-policy/NFR-004

**Spec Requirement**: NFR-004 — Component MUST conform to Certus component model with receptacle for ILogger
**Current Code**: ILogger receptacle is declared in `define_component!` but never used — zero log calls exist.
**Required Change**: Add trace-level logging for key lifecycle events using the ILogger receptacle.
**Files to Modify**: `src/lib.rs`
**Estimated Effort**: small

### Implementation Details

Add logging calls for:
1. `create_pool()` — log new pool creation with assigned pool ID
2. Error paths in `track`/`touch`/`remove` — log when InvalidPool is returned

The logger receptacle may not be connected (optional wiring), so guard log calls with a check that the receptacle is connected before calling.

### Acceptance Criteria
- [ ] `create_pool()` emits a trace/debug log with the new pool ID
- [ ] `track`/`touch`/`remove` on invalid pool emit a warning log
- [ ] Component still works correctly when ILogger is not connected (no panic)
- [ ] All existing tests continue to pass
- [ ] `cargo clippy -- -D warnings` passes

---

## Task: Align 001-lru-eviction-policy/FR-012 — add batch_touch tests (A1)

Generated: 2026-09-02

**Spec Requirement**: FR-012 — `batch_touch(handles)` marks multiple entries MRU in a single lock acquisition.
**Current Code**: Implemented at `src/lib.rs:89-115`; correct and clippy/fmt-clean, but no dedicated test asserts its behavior.
**Required Change**: Add unit/integration tests for `batch_touch`.
**Files to Modify**: `src/lib.rs` (tests module)
**Estimated Effort**: small

### Acceptance Criteria
- [ ] Same-pool batch: touching a slice of handles reorders those entries to MRU in the given order
- [ ] Multi-pool batch: a slice spanning >1 pool relocks correctly and each pool's ordering is updated
- [ ] Empty slice returns `Ok(())` with no effect
- [ ] Handle referencing a non-existent pool returns `EvictionPolicyError::InvalidPool`
- [ ] All existing tests still pass; `cargo clippy -- -D warnings` clean

> Note: the earlier NFR-004 align task above (wire ILogger logging) is now
> SATISFIED by the current implementation (`src/lib.rs:47-49,61-65,76-81,120-126`)
> and requires no further action.
