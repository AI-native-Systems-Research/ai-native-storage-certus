# Spec Sync: Align Tasks

Generated: 2026-08-07T14:55:33Z
Based on: proposals from 2026-08-06T23:27:50Z

These are spec → code alignment tasks approved during `speckit.sync.propose --interactive`.
Per the sync-apply workflow, code is **not** modified here — each task below describes the
change for a follow-up implementation pass (e.g. via `speckit-implement`).

---

## Task: Align 001-dispatch-map / FR-012 (unbound eviction policy must error, not panic)

**Spec Requirement**: FR-012 (+ contract "Error Semantics: no panics")
**Current Code**: `initialize()` (`src/lib.rs:67`) calls `self.get_pool_id()` first; `get_pool_id` (`src/lib.rs:50-59`) does `self.eviction_policy.get().unwrap()`, which **panics** when the `IEvictionPolicy` receptacle is unbound. The `.map_err(NotInitialized)` guard at `src/lib.rs:69-71` is therefore unreachable in the unbound case.
**Required Change**: Return `Err(DispatchMapError::NotInitialized(...))` instead of panicking when `IEvictionPolicy` is unbound. Either (a) make `get_pool_id` return `Result<PoolId, DispatchMapError>` and propagate, or (b) reorder `initialize` to `self.eviction_policy.get().map_err(|_| NotInitialized(...))?` before the first `get_pool_id()` call. Audit the other `eviction_policy.get().unwrap()` / `.track(...).unwrap()` sites (`src/lib.rs:55,92,392,399,573,579`) for the same panic-on-unbound / panic-on-track-failure risk.
**Files to Modify**: `components/dispatch-map/src/lib.rs`
**Estimated Effort**: small

### Acceptance Criteria
- [ ] `initialize()` on a component with no bound `IEvictionPolicy` returns `Err(NotInitialized)`, not a panic.
- [ ] A unit test constructs a `DispatchMapComponent` without connecting `eviction_policy` and asserts `initialize()` returns `Err(NotInitialized(_))`.
- [ ] No `unwrap()` on `eviction_policy.get()` remains on a caller-reachable path.

---

## Task: Align 001-dispatch-map / US1-AS3 (null-pointer rejection in create_memory_tier_entry)

**Spec Requirement**: User Story 1, Acceptance Scenario 3 + Edge Cases item 1
**Current Code**: `create_memory_tier_entry` (`src/lib.rs:381-424`) checks `size == 0` but has no null-pointer guard; a null `*mut u8` is accepted and stored. `DispatchMapError` (`components/interfaces/src/idispatch_map.rs:38-62`) has no null-pointer variant.
**Required Change**: Add a `pointer.is_null()` check at the top of `create_memory_tier_entry` returning a new `DispatchMapError::NullPointer(CacheKey)` variant before any map insertion, so no entry is recorded on a null pointer. Add the variant to the interface enum (+ `Display` arm) and to the contract's error list. (Consider whether `promote_block_to_memory_tier`, which also takes a `*mut u8`, should share the guard.)
**Files to Modify**: `components/interfaces/src/idispatch_map.rs` (new error variant + Display), `components/dispatch-map/src/lib.rs` (guard), `components/dispatch-map/specs/001-dispatch-map/contracts/idispatch_map.md` (error list)
**Estimated Effort**: small

### Acceptance Criteria
- [ ] `create_memory_tier_entry(key, ptr::null_mut(), size)` returns `Err(DispatchMapError::NullPointer(key))`.
- [ ] No entry is recorded in the map when a null pointer is passed (subsequent `lookup` returns `NotExist`).
- [ ] A unit test covers the null-pointer path.

---

## Task: Align 001-dispatch-map / reuse_count (remove dead hot-path metric)

**Spec Requirement**: N/A (unspecced dead instrumentation — approved for removal)
**Current Code**: `reuse_count: AtomicU32` (`src/entry.rs:37`) is incremented via `fetch_add` on `lookup` (`src/lib.rs:142`), `take_read` (`src/lib.rs:221`), and `downgrade_reference` (`src/lib.rs:314`), and only ever read in the `Debug` impl (`src/entry.rs:54-55`). It is never exposed through any `IDispatchMap` method — a pure atomic-write overhead on every hot-path access with no consumer.
**Required Change**: Remove the `reuse_count` field from `DispatchEntry`, the three `fetch_add` call sites, the `Debug` field, and all constructor initializers (`src/lib.rs:101,410,584`). Confirm `size_of::<DispatchEntry>()` still meets the SC-004 compactness expectation after removal, and update the `dispatch_map_benchmark.rs`/any assertion if it references the field size.
**Files to Modify**: `components/dispatch-map/src/entry.rs`, `components/dispatch-map/src/lib.rs`
**Estimated Effort**: small

### Acceptance Criteria
- [ ] `reuse_count` and all `fetch_add`/initializer references are removed; the crate builds with `cargo build -p dispatch-map`.
- [ ] `cargo clippy -p dispatch-map -- -D warnings` is clean (no unused-field/import warnings).
- [ ] Existing tests still pass.

---

## Task: Align interfaces / correct stale Creusot verification claims (P1–P10)

**Spec Requirement**: Documentation accuracy (contract "Error Semantics" / verification claims)
**Current Code**: `components/interfaces/src/idispatch_map.rs:84-99` states P1–P10 are "formally proved with Creusot ... see `components/dispatch-map/verif/`" and individual methods carry `# Verified: Pn` doc annotations, but the `components/dispatch-map/verif/` directory **does not exist** — no proofs are present in the tree.
**Required Change**: Either (a) remove/soften the P1–P10 comment block and the per-method `# Verified:` annotations so they no longer assert proofs that do not exist (e.g. reword to "Intended invariant (not yet mechanically proved)"), or (b) if the proofs were lost, restore the `verif/` Creusot harness. Recommended: reword to "planned/intended" pending restoration of the harness. Keep the spec.md side consistent (no "verified" claims there).
**Files to Modify**: `components/interfaces/src/idispatch_map.rs` (and, if restoring, `components/dispatch-map/verif/`)
**Estimated Effort**: small (reword) / medium (restore proofs)

### Acceptance Criteria
- [ ] The interface no longer claims proofs exist at a path that is absent from the repository.
- [ ] `cargo doc -p dispatch-map --no-deps` remains warning-free.
- [ ] If reworded: annotations clearly distinguish "intended/planned" from "mechanically verified".
