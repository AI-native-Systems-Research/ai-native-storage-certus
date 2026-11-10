# Spec Sync: Align Tasks

Generated: 2026-08-20 (Phase B)
Updated: 2026-10-14 — apply on branch `fix/dispatch-map-get-pool-id-panic` (proposals P6–P9, approved by Cornel Constantinescu as ALIGN tasks only)
Based on: `components/dispatch-map/.specify/sync/drift-report.json`, `components/dispatch-map/.specify/sync/proposals.json`
Policy: `.specify/sync/PHASE_B_POLICY.md`

These are spec → code alignment tasks: the spec requirement is correct and agreed, and the
code violates it. Per the sync-apply workflow, **code is NOT modified here** — each task below
describes the change for a follow-up implementation pass (e.g. via `speckit-implement`).

Approval note on P6, P7, P8 and P9 (2026-10-14): "approved as an ALIGN task only: record in align-tasks.md, no code change in this PR".

---

## Task: Align 001-dispatch-map / FR-012 (unbound `IEvictionPolicy` must error, not panic)

**Status (2026-10-14)**: code fixed by `c877b74e` — `get_pool_id` now returns `Result` (`src/lib.rs:51-63`), and `initialize`, `create_memory_tier_entry` and `recover_extent` return `NotInitialized` when no policy is connected; `oldest_keys` returns an empty list. The spec side was backfilled on 2026-10-14 (P1–P4). The only open criterion is the `initialize()` test, now tracked as the FR-012 test task below (P8). The `track(...).unwrap()` sites mentioned in the audit below are tracked separately (P7).

**Spec Requirement**: FR-012 — "On initialization, the `IEvictionPolicy` receptacle MUST be connected (returns an error if unbound)." Reinforced by the contract's "Error Semantics: no panics".

**Current Code** (as of 2026-08-20): `initialize()` (`src/lib.rs:67-68`) calls `self.get_pool_id()` first; `get_pool_id` (`src/lib.rs:50-59`) does `self.eviction_policy.get().unwrap()` at `src/lib.rs:55`, which **panics** when the `IEvictionPolicy` receptacle is unbound. The graceful `.map_err(|_| NotInitialized(...))?` guard at `src/lib.rs:69-71` is therefore unreachable in the unbound case. The same unwrap-on-unbound pattern recurs in `create_memory_tier_entry` (`src/lib.rs:392`), `recover_extent` (`src/lib.rs:573`), and via `get_pool_id` in `oldest_keys` (`src/lib.rs:373`).

**Required Change**: Return `Err(DispatchMapError::NotInitialized(...))` instead of panicking when `IEvictionPolicy` is unbound. Either (a) make `get_pool_id` return `Result<PoolId, DispatchMapError>` and propagate, or (b) reorder `initialize` to check `self.eviction_policy.get().map_err(|_| NotInitialized(...))?` *before* the first `get_pool_id()` call. Audit the other `eviction_policy.get().unwrap()` / `.track(...).unwrap()` sites (`src/lib.rs:55, 92, 392, 399, 573, 579`) for the same panic-on-unbound / panic-on-track-failure risk.

**Files to Modify**: `components/dispatch-map/src/lib.rs`

**Estimated Effort**: small

### Acceptance Criteria
- [x] `initialize()` on a component with no bound `IEvictionPolicy` returns `Err(NotInitialized(_))`, not a panic. *(`c877b74e`)*
- [ ] A unit test constructs a `DispatchMapComponent` without connecting `eviction_policy` and asserts `initialize()` returns `Err(NotInitialized(_))`. *(→ FR-012 test task, P8)*
- [x] No `unwrap()` on `eviction_policy.get()` remains on a caller-reachable path. *(`c877b74e`; the `track(...).unwrap()` calls are a different panic, see P7)*

---

## Task: Align 001-dispatch-map / FR-003 · US1-AS3 (null-pointer rejection in `create_memory_tier_entry`)

**Status (2026-10-14)**: still open; re-confirmed as proposal P6 and approved as an ALIGN task only. Line numbers below are refreshed to HEAD `0ca07fca`.

**Spec Requirement**: FR-003; User Story 1 Acceptance Scenario 3 (`spec.md:27`); Edge Cases bullet 1 (`spec.md:225`) — "`create_memory_tier_entry` with a null pointer returns an error; no entry is recorded."

**Current Code**: `create_memory_tier_entry` (`src/lib.rs:393-439`) validates only `size == 0` → `InvalidSize` (`src/lib.rs:399-401`); there is no `pointer.is_null()` guard, so a null `*mut u8` is accepted and an entry is inserted into the map (`:415-430`). `DispatchMapError` (`components/interfaces/src/idispatch_map.rs:38-61`) has no null-pointer variant; `data-model.md` acknowledges the gap.

**Required Change**: Add a `pointer.is_null()` check at the top of `create_memory_tier_entry` (before `:399`) returning an error **before** any map insertion, so no entry is recorded on a null pointer — either `InvalidState("null memory-tier pointer")` or a new `DispatchMapError::NullPointer(CacheKey)` variant (+ `Display` arm in the interface enum, and the contract's error list). Consider the same guard in `promote_block_to_memory_tier` (`:482-490`), which also takes a `*mut u8`.

**Alternative recorded for the reviewer**: SUPERSEDE US1-AS3 and Edge Case 1 — the map never dereferences the pointer, so the check could be left to the caller.

**Files to Modify**: `components/interfaces/src/idispatch_map.rs` (new error variant + `Display`, if chosen), `components/dispatch-map/src/lib.rs` (guard), `components/dispatch-map/specs/001-dispatch-map/contracts/idispatch_map.md` and `data-model.md` (error list).

**Estimated Effort**: small

### Acceptance Criteria
- [ ] `create_memory_tier_entry(key, ptr::null_mut(), size)` returns an error (e.g. `Err(DispatchMapError::NullPointer(key))`).
- [ ] No entry is recorded in the map when a null pointer is passed (subsequent `entry_size(key)` returns `KeyNotFound`).
- [ ] A unit test covers the null-pointer path.

---

## Task: Align 001-dispatch-map / no-panics rule · FR-002 · FR-012 (stale cached pool id; `track(...).unwrap()` panics under the map lock) — P7

**Spec Requirement**: Clarification 2026-04-27 Q3 (`spec.md`: "Return error for all invalid operations. No panics or silent no-ops."); contract Error Semantics (`contracts/idispatch_map.md:79`, "All error conditions return Err(DispatchMapError) -- no panics"); FR-002 ("a separate `Mutex<Option<PoolId>>` holds the bound pool id"); FR-012.

**Current Code**: `get_pool_id` caches the first pool id forever (`src/lib.rs:52-62`). After `eviction_policy` is disconnected and a different policy connected, `ep.track(stale_pool, …).unwrap()` at `src/lib.rs:101-103` (`initialize`), `:414` (`create_memory_tier_entry`) and `:598` (`recover_extent`) panics with `InvalidPool` (`components/eviction-policy-lru/src/lib.rs:60-67`). Each runs while `self.state.inner` is locked (`:97`, `:409`, `:594`), so the panic poisons the mutex and every later call panics. If the new policy already has a pool at that index, the entry is silently tracked in another component's pool. `src/repair_test_get_pool_id.rs:44-66` tests disconnect only, not reconnect.

**Required Change**:
1. Map `track` errors to `DispatchMapError` instead of `unwrap()` (e.g. `.map_err(|e| DispatchMapError::NotInitialized(format!("eviction policy rejected pool: {e:?}")))?`); in `initialize`, collect the first error from the `for_each_extent` closure and return it after the walk.
2. Tie the cached pool id to the policy instance it came from (store the `Arc` and compare with `Arc::ptr_eq`, re-running `create_pool` on a mismatch), or forbid rebinding `eviction_policy` after first use.
3. Add a reconnect-to-a-fresh-policy test.

**Alternative recorded for the reviewer**: BACKFILL FR-015 — "the `eviction_policy` receptacle MUST NOT be rebound after the first `IDispatchMap` call" — but still do step 1.

**Files to Modify**: `components/dispatch-map/src/lib.rs`, `components/dispatch-map/src/repair_test_get_pool_id.rs` (test)

**Estimated Effort**: medium

### Acceptance Criteria
- [ ] No `track(...).unwrap()` remains on a caller-reachable path; a `track` failure returns `Err(DispatchMapError)`.
- [ ] A test connects policy A, calls `oldest_keys`, disconnects, connects a fresh policy B, and asserts `create_memory_tier_entry` and `recover_extent` return `Err` (not a panic) — or succeed against a fresh pool in B, if option 2 re-creates the pool.
- [ ] After that sequence the map still works (no poisoned mutex): a later `entry_size`/`lookup` call does not panic.
- [ ] No entry is ever tracked in a pool the component did not create.

---

## Task: Align 001-dispatch-map / FR-012 (test coverage for `initialize()` with no eviction policy) — P8

**Spec Requirement**: FR-012 (`spec.md:251`) — "On initialization, the `IEvictionPolicy` receptacle MUST be connected (returns an error if unbound)."

**Current Code**: the behaviour is right (`src/lib.rs:77` → `:56-59`; `:78-81` when a pool id is already cached), but no test calls `initialize()` unbound. The repair tests (`src/repair_test_get_pool_id.rs:13-66`) never call `initialize`, and `tests/integration.rs:509,540` have a policy bound.

**Required Change**: Add two tests to `src/repair_test_get_pool_id.rs`: `initialize_without_eviction_policy_errors` (`unbound_component()` then `initialize()`) and `initialize_after_disconnect_errors` (connect, `oldest_keys(1)`, disconnect, `initialize()`), both asserting `Err(NotInitialized(_))`. Test-only; no production code change.

**Files to Modify**: `components/dispatch-map/src/repair_test_get_pool_id.rs`

**Estimated Effort**: small

### Acceptance Criteria
- [ ] `initialize_without_eviction_policy_errors` exists and asserts `Err(DispatchMapError::NotInitialized(_))`.
- [ ] `initialize_after_disconnect_errors` exists and asserts `Err(DispatchMapError::NotInitialized(_))`.
- [ ] Both pass under `cargo test -p dispatch-map`; this closes the last open criterion of the FR-012 task above.

---

## Task: Align 001-dispatch-map / FR-023 (`entry_size` `u32` overflow) — P9

**Spec Requirement**: FR-023 (`spec.md:262`) — "returns the stored size of an entry in block-aligned bytes (`size_blocks * 4096`) … Used by the dispatcher's `promote_to_memory_tier` to determine memory-tier allocation size"; contract Error Semantics "no panics" (`contracts/idispatch_map.md:79`).

**Current Code**: `Ok(entry.size_blocks * 4096)` as `u32` (`src/lib.rs:379`) overflows for `size_blocks >= 2^20`. Such entries come from `create_memory_tier_entry`/`promote_block_to_memory_tier` with `size > 4_294_963_200` (`:421`, `:509`) or from any `size_blocks` passed to `recover_extent`/`initialize` (`:601`, `:108`). Dev/test builds panic; release builds return `0`, which the dispatcher would use to size an allocation.

**Required Change**: Use `entry.size_blocks.checked_mul(4096).ok_or(DispatchMapError::InvalidSize)`, or reject out-of-range sizes with `InvalidSize` in `create_memory_tier_entry`/`promote_block_to_memory_tier` (`size > u32::MAX - 4095`) and `recover_extent` (`size_blocks > u32::MAX / 4096`), skipping or reporting such extents in `initialize`. Add a test with `size = u32::MAX`.

**Spec follow-up (needed under either option)**: BACKFILL FR-023 with "MUST return `InvalidSize` if the block-aligned size does not fit in `u32`" once the code change is chosen.

**Files to Modify**: `components/dispatch-map/src/lib.rs`; then `components/dispatch-map/specs/001-dispatch-map/spec.md` (FR-023) and the contract row for `entry_size`.

**Estimated Effort**: small (severity minor — needs a single entry of almost 4 GiB)

### Acceptance Criteria
- [ ] `entry_size` never panics and never returns a wrapped/zero size for a stored entry.
- [ ] A unit test with `size = u32::MAX` (and one with `recover_extent(.., size_blocks = u32::MAX)`) asserts the chosen behaviour (`Err(InvalidSize)` at creation or at `entry_size`).
- [ ] FR-023 and the contract state the `InvalidSize` outcome.

---

## Open items from unapproved proposals

None. All ten proposals of 2026-10-14 (P1–P10) are `approved: true`; no unapproved proposal carries an `approval_note`.
