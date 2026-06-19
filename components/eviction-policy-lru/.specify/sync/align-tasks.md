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
