# Spec Sync: Align Tasks

Generated: 2026-08-20 (Phase B) · Reconfirmed: 2026-08-31 (both tasks still open — code unchanged since 2026-08-20, verified: `src/lib.rs:55` still `unwrap()`s eviction_policy before the guard; `create_memory_tier_entry` still has no `is_null` check)
Based on: `components/dispatch-map/.specify/sync/drift-report.json`
Policy: `.specify/sync/PHASE_B_POLICY.md`

These are spec → code alignment tasks: the spec requirement is correct and agreed, and the
code violates it. Per the sync-apply workflow, **code is NOT modified here** — each task below
describes the change for a follow-up implementation pass (e.g. via `speckit-implement`).

---

## Task: Align 001-dispatch-map / FR-012 (unbound `IEvictionPolicy` must error, not panic)

**Spec Requirement**: FR-012 — "On initialization, the `IEvictionPolicy` receptacle MUST be connected (returns an error if unbound)." Reinforced by the contract's "Error Semantics: no panics".

**Current Code**: `initialize()` (`src/lib.rs:67-68`) calls `self.get_pool_id()` first; `get_pool_id` (`src/lib.rs:50-59`) does `self.eviction_policy.get().unwrap()` at `src/lib.rs:55`, which **panics** when the `IEvictionPolicy` receptacle is unbound. The graceful `.map_err(|_| NotInitialized(...))?` guard at `src/lib.rs:69-71` is therefore unreachable in the unbound case. The same unwrap-on-unbound pattern recurs in `create_memory_tier_entry` (`src/lib.rs:392`), `recover_extent` (`src/lib.rs:573`), and via `get_pool_id` in `oldest_keys` (`src/lib.rs:373`).

**Required Change**: Return `Err(DispatchMapError::NotInitialized(...))` instead of panicking when `IEvictionPolicy` is unbound. Either (a) make `get_pool_id` return `Result<PoolId, DispatchMapError>` and propagate, or (b) reorder `initialize` to check `self.eviction_policy.get().map_err(|_| NotInitialized(...))?` *before* the first `get_pool_id()` call. Audit the other `eviction_policy.get().unwrap()` / `.track(...).unwrap()` sites (`src/lib.rs:55, 92, 392, 399, 573, 579`) for the same panic-on-unbound / panic-on-track-failure risk.

**Files to Modify**: `components/dispatch-map/src/lib.rs`

**Estimated Effort**: small

### Acceptance Criteria
- [ ] `initialize()` on a component with no bound `IEvictionPolicy` returns `Err(NotInitialized(_))`, not a panic.
- [ ] A unit test constructs a `DispatchMapComponent` without connecting `eviction_policy` and asserts `initialize()` returns `Err(NotInitialized(_))`.
- [ ] No `unwrap()` on `eviction_policy.get()` remains on a caller-reachable path.

---

## Task: Align 001-dispatch-map / FR-003 · US1-AS3 (null-pointer rejection in `create_memory_tier_entry`)

**Spec Requirement**: FR-003; User Story 1 Acceptance Scenario 3; Edge Cases bullet 1 — "`create_memory_tier_entry` with a null pointer returns an error; no entry is recorded."

**Current Code**: `create_memory_tier_entry` (`src/lib.rs:381-424`) validates only `size == 0` → `InvalidSize` (`src/lib.rs:387-389`); there is no `pointer.is_null()` guard, so a null `*mut u8` is accepted and an entry is inserted into the map. `DispatchMapError` (`components/interfaces/src/idispatch_map.rs`) has no null-pointer variant.

**Required Change**: Add a `pointer.is_null()` check at the top of `create_memory_tier_entry` returning a null-pointer error (e.g. a new `DispatchMapError::NullPointer(CacheKey)` variant) **before** any map insertion, so no entry is recorded on a null pointer. Add the variant to the interface enum (+ `Display` arm) and to the contract's error list. Consider whether `promote_block_to_memory_tier`, which also takes a `*mut u8`, should share the guard.

**Files to Modify**: `components/interfaces/src/idispatch_map.rs` (new error variant + `Display`), `components/dispatch-map/src/lib.rs` (guard), `components/dispatch-map/specs/001-dispatch-map/contracts/idispatch_map.md` (error list).

**Estimated Effort**: small

### Acceptance Criteria
- [ ] `create_memory_tier_entry(key, ptr::null_mut(), size)` returns an error (e.g. `Err(DispatchMapError::NullPointer(key))`).
- [ ] No entry is recorded in the map when a null pointer is passed (subsequent `lookup` returns `NotExist`).
- [ ] A unit test covers the null-pointer path.
