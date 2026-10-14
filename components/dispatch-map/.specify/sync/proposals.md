# Drift Resolution Proposals

Generated: 2026-10-14
Based on: `.specify/sync/drift-report.json` (2026-10-14, branch `fix/dispatch-map-get-pool-id-panic`, HEAD `0ca07fca`)
Mode: batch. **Every proposal is `approved: false`. Nothing has been applied** (apply was not run, and no spec or source file was edited).
Paths: `spec.md`, `contracts/…` and `data-model.md` are under `components/dispatch-map/specs/001-dispatch-map/`, and `src/…` is under `components/dispatch-map/`.

## Summary

| Resolution Type | Count |
|-----------------|-------|
| Backfill (Code → Spec) | 5 (P1–P5) |
| Align (Spec → Code) | 4 (P6–P9) |
| Supersede (status note out of date) | 1 (P10) |
| Human Decision | 0 (P6 and P7 each have an alternative listed for the reviewer) |
| New Specs | 0 |
| Remove from Spec | 0 |

---

### Proposal P1: 001-dispatch-map/FR-003 — document `InvalidSize` and `NotInitialized`

**Direction**: BACKFILL · **Confidence**: HIGH · **approved: false**

**Requirement**: FR-003, `create_memory_tier_entry`.

**Spec says** (`spec.md:242`): "… records the entry in the map with a write reference of 1, and registers it with the eviction policy. MUST return `AlreadyExists` if the key is already present."
Contract (`contracts/idispatch_map.md:45`): "Error on `size == 0` (`InvalidSize`) or key exists (`AlreadyExists`)."

**Code does**: `src/lib.rs:399-401` returns `InvalidSize` for `size == 0`. `src/lib.rs:403-407` returns `NotInitialized` when no `IEvictionPolicy` is connected; this path is new in `c877b74e` and used to panic. Only then does it check for `AlreadyExists` (`:410-412`). Nothing is inserted on any of these errors. Test: `src/repair_test_get_pool_id.rs:19-28`.

**Old text — `spec.md:242`**:
```
- **FR-003**: System MUST provide `create_memory_tier_entry(key, pointer, size)` that creates an entry with `MemoryTier` location (accepting a `*mut u8` pointer and `u32` size in bytes), records the entry in the map with a write reference of 1, and registers it with the eviction policy. MUST return `AlreadyExists` if the key is already present.
```
**New text**:
```
- **FR-003**: System MUST provide `create_memory_tier_entry(key, pointer, size)` that creates an entry with `MemoryTier` location (accepting a `*mut u8` pointer and `u32` size in bytes), records the entry in the map with a write reference of 1, and registers it with the eviction policy. MUST return `InvalidSize` if `size == 0`, `NotInitialized` if no `IEvictionPolicy` is connected, and `AlreadyExists` if the key is already present (checked in that order); on any error no entry is recorded.
```
**Old text — `contracts/idispatch_map.md:45`**:
```
| `create_memory_tier_entry` | `(&self, key: CacheKey, pointer: *mut u8, size: u32) -> Result<(), DispatchMapError>` | Create a `MemoryTier` entry, write_ref=1, registers with `IEvictionPolicy`. Error on `size == 0` (`InvalidSize`) or key exists (`AlreadyExists`). |
```
**New text**:
```
| `create_memory_tier_entry` | `(&self, key: CacheKey, pointer: *mut u8, size: u32) -> Result<(), DispatchMapError>` | Create a `MemoryTier` entry, write_ref=1, registers with `IEvictionPolicy`. Error on `size == 0` (`InvalidSize`), no `IEvictionPolicy` connected (`NotInitialized`), or key exists (`AlreadyExists`). No entry is recorded on error. |
```
**Rationale**: The fix was found by formal verification and has a regression test, and it is what FR-012 and the "no panics" rule require. The spec should list the error the code now returns. This proposal doesn't touch the null-pointer gap, which is P6.

---

### Proposal P2: 001-dispatch-map/FR-024 — document `NotInitialized` from `recover_extent`

**Direction**: BACKFILL · **Confidence**: HIGH · **approved: false**

**Requirement**: FR-024, `recover_extent`.

**Spec says** (`spec.md:263`): "… registers it with the eviction policy, and returns success. MUST return `AlreadyExists` if the key is already present." Contract `contracts/idispatch_map.md:60`: "Error (`AlreadyExists`) if key already present."

**Code does**: `src/lib.rs:588-592` returns `NotInitialized` when no `IEvictionPolicy` is connected (new in `c877b74e`); `:595-597` returns `AlreadyExists`. Tests: `src/repair_test_get_pool_id.rs:31-39,44-66`.

**Old text — `spec.md:263`**:
```
- **FR-024**: System MUST provide `recover_extent(key, offset, size_blocks)` that directly inserts a `BlockDevice` entry with the given offset and size (in 4KiB blocks), registers it with the eviction policy, and returns success. MUST return `AlreadyExists` if the key is already present. Used for incremental recovery (inserting individual extents) as an alternative to the bulk `initialize()` walk via `IExtentManager::for_each_extent`.
```
**New text**:
```
- **FR-024**: System MUST provide `recover_extent(key, offset, size_blocks)` that directly inserts a `BlockDevice` entry with the given offset and size (in 4KiB blocks), registers it with the eviction policy, and returns success. MUST return `NotInitialized` if no `IEvictionPolicy` is connected and `AlreadyExists` if the key is already present; on either error no entry is recorded. Used for incremental recovery (inserting individual extents) as an alternative to the bulk `initialize()` walk via `IExtentManager::for_each_extent`.
```
**Old text — `contracts/idispatch_map.md:60`**:
```
| `recover_extent` | `(&self, key: CacheKey, offset: u64, size_blocks: u32) -> Result<(), DispatchMapError>` | Directly inserts a `BlockDevice` entry and registers it with `IEvictionPolicy`, for incremental recovery. Error (`AlreadyExists`) if key already present. |
```
**New text**:
```
| `recover_extent` | `(&self, key: CacheKey, offset: u64, size_blocks: u32) -> Result<(), DispatchMapError>` | Directly inserts a `BlockDevice` entry and registers it with `IEvictionPolicy`, for incremental recovery. Error (`NotInitialized`) if no `IEvictionPolicy` is connected; error (`AlreadyExists`) if key already present. No entry is recorded on error. |
```
**Rationale**: Same as P1.

---

### Proposal P3: 001-dispatch-map/FR-017 — `oldest_keys` with no eviction policy

**Direction**: BACKFILL · **Confidence**: HIGH · **approved: false**

**Requirement**: FR-017, `oldest_keys`.

**Spec says** (`spec.md:256`): "returns up to `n` keys ordered oldest-first by delegating to `IEvictionPolicy::get_eviction_candidates(pool, n)`." It doesn't say what happens when no policy is connected. Contract `:58` says the same.

**Code does**: `src/lib.rs:383-385` returns `Vec::new()` if `get_pool_id()` fails; it used to panic. `:386-390` returns empty if the receptacle is unbound after a pool id was cached. Tests: `src/repair_test_get_pool_id.rs:13-16,50,58`. The signature `-> Vec<CacheKey>` has no way to report an error.

**Old text — `spec.md:256`**:
```
- **FR-017**: System MUST provide `oldest_keys(n)` that returns up to `n` keys ordered oldest-first by delegating to `IEvictionPolicy::get_eviction_candidates(pool, n)`. Used by the dispatcher's eviction logic to identify victim entries. MUST be thread-safe.
```
**New text**:
```
- **FR-017**: System MUST provide `oldest_keys(n)` that returns up to `n` keys ordered oldest-first by delegating to `IEvictionPolicy::get_eviction_candidates(pool, n)`. Used by the dispatcher's eviction logic to identify victim entries. MUST be thread-safe. If no `IEvictionPolicy` is connected, it MUST return an empty list rather than panic (the signature has no error channel, so callers cannot distinguish this from an empty map).
```
**Old text — `contracts/idispatch_map.md:58`**:
```
| `oldest_keys` | `(&self, n: usize) -> Vec<CacheKey>` | Delegates to `IEvictionPolicy::get_eviction_candidates(pool, n)`; returns up to `n` keys oldest-first. Used by the dispatcher's eviction logic to select victims. |
```
**New text**:
```
| `oldest_keys` | `(&self, n: usize) -> Vec<CacheKey>` | Delegates to `IEvictionPolicy::get_eviction_candidates(pool, n)`; returns up to `n` keys oldest-first. Used by the dispatcher's eviction logic to select victims. Returns an empty list if no `IEvictionPolicy` is connected. |
```
**Rationale**: The spec is silent here and the fix chose a behaviour, so the spec should record it. The 2026-10-13 note already called this consistent with FR-017.

---

### Proposal P4: 001-dispatch-map/FR-012 — `data-model.md` still calls `NotInitialized` "not currently returned"

**Direction**: BACKFILL · **Confidence**: HIGH · **approved: false**

**Requirement**: FR-012 (`spec.md:251`, "returns an error if unbound"), as described in the data model.

**Spec says** (`data-model.md:82`): "NotInitialized | msg: String | Reserved; not currently returned (`initialize()` errors via a missing-`IEvictionPolicy` failure instead)".

**Code does**: `NotInitialized("eviction_policy receptacle not connected")` (`src/lib.rs:67-69`) is returned by `initialize` (`:77`, `:78-81`), `create_memory_tier_entry` (`:403-407`) and `recover_extent` (`:588-592`).

**Old text — `data-model.md:82`**:
```
| NotInitialized | msg: String | Reserved; not currently returned (`initialize()` errors via a missing-`IEvictionPolicy` failure instead) |
```
**New text**:
```
| NotInitialized | msg: String | `initialize`, `create_memory_tier_entry`, or `recover_extent` called with no `IEvictionPolicy` connected |
```
**Rationale**: The current text is wrong, and since `c877b74e` it is the only error those three methods return when no policy is connected.

---

### Proposal P5: 001-dispatch-map/FR-026 — contract and data-model still say `InvalidState` for held references

**Direction**: BACKFILL · **Confidence**: HIGH · **approved: false**

**Requirement**: FR-026, `try_evict_to_block`.

**Spec says**: `spec.md:265` is correct ("`ActiveReferences` if a read or write reference is held"). But `contracts/idispatch_map.md:49` says "Error on `KeyNotFound`, or `InvalidState` if not evictable (active refs, no `ssd_offset`, or not `MemoryTier`)". `data-model.md:78` lists `ActiveReferences` only for `remove`, and `data-model.md:86` lumps every `try_evict_to_block` precondition under `InvalidState`.

**Code does**: `src/lib.rs:558-560` returns `ActiveReferences(key)`; `:571-578` returns `InvalidState` for no `ssd_offset` or a non-MemoryTier entry. Tests: `src/lib.rs:990-1019`. The interface doc says the same (`components/interfaces/src/idispatch_map.rs:181-185`). The 2026-10-09 sync updated `spec.md` but not these two files.

**Old text — `contracts/idispatch_map.md:49`**:
```
| `try_evict_to_block` | `(&self, key: CacheKey) -> Result<(), DispatchMapError>` | Atomically (single lock hold) checks evictability — `MemoryTier` with `ssd_offset: Some(_)` and `read_ref == 0 && write_ref == 0` — and transitions to `BlockDevice { offset }`. Error on `KeyNotFound`, or `InvalidState` if not evictable (active refs, no `ssd_offset`, or not `MemoryTier`). No partial state change on error. |
```
**New text**:
```
| `try_evict_to_block` | `(&self, key: CacheKey) -> Result<(), DispatchMapError>` | Atomically (single lock hold) checks evictability — `MemoryTier` with `ssd_offset: Some(_)` and `read_ref == 0 && write_ref == 0` — and transitions to `BlockDevice { offset }`. Error on `KeyNotFound`, `ActiveReferences` if a read or write reference is held, or `InvalidState` if otherwise not evictable (no `ssd_offset`, or not `MemoryTier`). No partial state change on error. |
```
**Old text — `data-model.md:78`**:
```
| ActiveReferences | key: CacheKey | `remove` while refs > 0 |
```
**New text**:
```
| ActiveReferences | key: CacheKey | `remove` or `try_evict_to_block` while refs > 0 |
```
**Old text — `data-model.md:86`**:
```
| InvalidState | msg: String | `convert_to_storage`/`convert_memory_tier_to_block` on a non-matching entry state, or `promote_block_to_memory_tier`/`try_evict_to_block` preconditions not met |
```
**New text**:
```
| InvalidState | msg: String | `convert_to_storage`/`convert_memory_tier_to_block` on a non-matching entry state, `promote_block_to_memory_tier` on a `MemoryTier` entry, or `try_evict_to_block` on an unreferenced entry with no `ssd_offset` or not in `MemoryTier` state |
```
**Rationale**: `spec.md` and the code agree, and the dispatcher depends on this error (its FR-033 counts `evictions_blocked_by_pin` on `ActiveReferences`). Only the two secondary files lag behind.

---

### Proposal P6: 001-dispatch-map/FR-003 · US1-AS3 — null pointer accepted

**Direction**: ALIGN · **Confidence**: MEDIUM · **approved: false**

**Requirement**: FR-003 / User Story 1 Acceptance Scenario 3 / Edge Cases.

**Spec says**: `spec.md:27`: "**Given** a null pointer is passed, **When** `create_memory_tier_entry(key=42, null, size=16384)` is called, **Then** an error is returned." `spec.md:225`: "`create_memory_tier_entry` with a null pointer returns an error; no entry is recorded in the map."

**Code does**: it checks only `size == 0` (`src/lib.rs:399-401`). A null `pointer` is stored in `Location::MemoryTier` (`:415-430`), and `DispatchMapError` has no null-pointer variant (`components/interfaces/src/idispatch_map.rs:38-61`). `data-model.md:88` acknowledges the gap.

**Proposed code change**: at the top of `create_memory_tier_entry` (before `:399`), add `if pointer.is_null() { return Err(DispatchMapError::InvalidState("null memory-tier pointer".into())); }`, or add a dedicated `NullPointer(CacheKey)` variant to `components/interfaces/src/idispatch_map.rs` together with a `Display` arm. Add a unit test asserting the error and that `entry_size(key)` then returns `KeyNotFound`. Consider the same guard in `promote_block_to_memory_tier` (`:482-490`).

**Alternative for the reviewer**: SUPERSEDE US1-AS3 and Edge Case 1. The pointer is opaque to the map, which never dereferences it, so the check could be left to the caller. The `Location` SAFETY comment (`src/entry.rs:21-23`) doesn't depend on the pointer being non-null.

**Rationale**: The spec has stated this since 2026-04-27 and it was re-confirmed as an align on 2026-08-20. It is unchanged by this branch.

---

### Proposal P7: no-panics rule — stale cached pool id makes `track(...).unwrap()` panic while the map mutex is held

**Direction**: ALIGN · **Confidence**: MEDIUM · **approved: false**

**Requirement**: Clarification 2026-04-27 Q3 (`spec.md:295`: "Return error for all invalid operations. No panics or silent no-ops."), contract Error Semantics (`contracts/idispatch_map.md:79`), FR-002 (`spec.md:241`: "a separate `Mutex<Option<PoolId>>` holds the bound pool id"), and FR-012.

**Spec says**: no operation panics. The pool id is "the bound pool id".

**Code does**:
- `get_pool_id` caches the first pool id it sees and never invalidates it (`src/lib.rs:52-62`).
- After `eviction_policy` is disconnected and a different policy connected, `ep.track(pool_id, …).unwrap()` runs with a pool id the new policy never issued. Sites: `src/lib.rs:101-103` (`initialize`), `:414` (`create_memory_tier_entry`), `:598` (`recover_extent`). The LRU policy returns `InvalidPool` for an unknown index (`components/eviction-policy-lru/src/lib.rs:60-67`; ids are `pools.len()`, `:43`), and the `unwrap()` panics.
- Each of these runs while `self.state.inner` is locked (`:97`, `:409`, `:594`). The panic poisons the mutex, and every later `lock().unwrap()` in the component panics.
- If the new policy already has a pool at that index (it is shared and multi-pool), `track` succeeds into another component's pool. `oldest_keys` then returns that component's keys and `touch`/`remove` act on its LRU list.
- `src/repair_test_get_pool_id.rs:44-66` tests disconnect only, not reconnect.

**Proposed code change**:
1. Replace the three `track(...).unwrap()` calls with `.map_err(|e| DispatchMapError::NotInitialized(format!("eviction policy rejected pool: {e:?}")))?`. For the `initialize` closure (`:100-118`), collect the first error and return it after the walk.
2. Make the cached pool id belong to the policy instance it came from. Either store `(Arc<dyn IEvictionPolicy>, PoolId)` and compare with `Arc::ptr_eq` on each use, re-running `create_pool` on a mismatch, or document that `eviction_policy` must not be rebound after first use and reject `connect` after that.
3. Add a test that connects policy A, calls `oldest_keys`, disconnects, connects a fresh policy B, then checks that `create_memory_tier_entry` and `recover_extent` return `Err` (not a panic) and that the map still works afterwards.

**Alternative for the reviewer**: if rebinding is out of scope, BACKFILL FR-015 with "the `eviction_policy` receptacle MUST NOT be rebound after the first `IDispatchMap` call", but still do step 1 so a misuse returns an error instead of poisoning the map.

**Rationale**: The 2026-10-13 sync flagged this as still drifting. Proposal P1 of 2026-08-20 already listed the `track` unwraps (`:399, :579` at the time). Panicking inside the lock makes the failure permanent for the whole map, not just one call.

---

### Proposal P8: 001-dispatch-map/FR-012 — no test for `initialize()` with no eviction policy

**Direction**: ALIGN (test only) · **Confidence**: HIGH · **approved: false**

**Requirement**: FR-012 (`spec.md:251`): "On initialization, the `IEvictionPolicy` receptacle MUST be connected (returns an error if unbound)."

**Spec says**: as above. `align-tasks.md` (FR-012 task) also asks for "a unit test constructs a `DispatchMapComponent` without connecting `eviction_policy` and asserts `initialize()` returns `Err(NotInitialized(_))`."

**Code does**: the behaviour is right (`src/lib.rs:77` → `:56-59`, and `:78-81` when a pool is already cached), but nothing tests it. The four repair tests (`src/repair_test_get_pool_id.rs:13-66`) never call `initialize`. The only callers, `tests/integration.rs:509,540`, have a policy bound.

**Proposed code change**: add two tests to `src/repair_test_get_pool_id.rs`. `initialize_without_eviction_policy_errors` calls `unbound_component()` then `initialize()` and asserts `Err(NotInitialized(_))`. `initialize_after_disconnect_errors` connects, calls `oldest_keys(1)`, disconnects, then asserts that `initialize()` returns `Err(NotInitialized(_))`.

**Rationale**: FR-012 is the requirement the branch fixed, but its own entry point has no test. That leaves the last open item of the align task.

---

### Proposal P9: 001-dispatch-map/FR-023 — `entry_size` overflows `u32`

**Direction**: ALIGN · **Confidence**: MEDIUM · **approved: false**

**Requirement**: FR-023 (`spec.md:262`) and "no panics" (`contracts/idispatch_map.md:79`).

**Spec says** (`spec.md:262`): "returns the stored size of an entry in block-aligned bytes (`size_blocks * 4096`) … Used by the dispatcher's `promote_to_memory_tier` to determine memory-tier allocation size."

**Code does**: `Ok(entry.size_blocks * 4096)` as `u32` (`src/lib.rs:379`). `create_memory_tier_entry(k, p, u32::MAX)` stores `size_blocks = 1_048_576` (`:421`), and so does `promote_block_to_memory_tier` (`:509`). `recover_extent`/`initialize` store any `size_blocks` the caller passes (`:601`, `:108`). The multiply overflows for `size_blocks >= 2^20`: dev/test builds panic (the workspace has no `[profile]` override), and release builds return `0`, which the dispatcher would use to size an allocation.

**Proposed code change**: use `entry.size_blocks.checked_mul(4096).ok_or(DispatchMapError::InvalidSize)`. Or reject `size > u32::MAX - 4095` with `InvalidSize` in `create_memory_tier_entry`/`promote_block_to_memory_tier`, and `size_blocks > u32::MAX / 4096` in `recover_extent` (and skip or report such extents in `initialize`). Add a unit test with `size = u32::MAX`.

**Alternative for the reviewer**: BACKFILL FR-023 with "MUST return `InvalidSize` if the block-aligned size does not fit in `u32`". The spec change is needed under either option.

**Rationale**: It's a real input-validation gap in a hand-written multiply. In practice it needs a single entry of almost 4 GiB, so the severity is minor.

---

### Proposal P10: spec.md status notes still list FR-012 as an open align

**Direction**: SUPERSEDE · **Confidence**: HIGH · **approved: false**

**Requirement**: the spec.md header notes (`spec.md:6`, `spec.md:8`).

**Spec says**: `spec.md:6`: "The two remaining drift items are code-side aligns: FR-012 (unbound `IEvictionPolicy` panics instead of returning `NotInitialized`) and FR-003/US1-AS3 …". `spec.md:8`: "The two pre-existing code-side aligns (FR-012 panic, FR-003/US1-AS3 null-pointer) remain unresolved".

**Code does**: the FR-012 panic is fixed by `c877b74e` (`src/lib.rs:51-63,77`).

**Proposed change**: leave the dated history notes as they are and add a new note after `spec.md:10`. If P1–P5 are applied together with this note, the note can say they were applied:
```
**Last Synced**: 2026-10-14 — branch `fix/dispatch-map-get-pool-id-panic`. CODE: the FR-012 align is resolved — with no `IEvictionPolicy` connected, `initialize`, `create_memory_tier_entry` and `recover_extent` return `NotInitialized` and `oldest_keys` returns an empty list (previously all four panicked). FR-003/FR-017/FR-024, the contract and the data-model now list these outcomes, and the contract/data-model are brought in line with the 2026-10-09 `ActiveReferences` change to `try_evict_to_block`. Remaining code-side aligns: FR-003/US1-AS3 null pointer; a stale cached pool id makes `track(...).unwrap()` panic under the map lock after `eviction_policy` is rebound; `entry_size` `u32` overflow (FR-023).
```
**Rationale**: The earlier notes are history and should stay. A new note records that their statement about FR-012 no longer holds.

---

PROPOSALS: 10 (backfill 5, align 4, other 1)
