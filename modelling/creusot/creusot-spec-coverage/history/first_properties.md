# Dispatcher Spec: First Verification Properties

Source: `spec.md` in this folder.
Goal: define a first, high-value property set for formal verification of the Rust implementation with Creusot.

## Core properties (priority set)

1. Initialization gate (`FR-012`): `initialize()` fails iff required receptacles (`dispatch_map`, `memory_tier`) are missing.
2. Initialized-state precondition (`FR-002` intent): operational APIs fail with `NotInitialized` before successful init.
3. Populate/prepare uniqueness (`FR-003`, `FR-020`): `populate` and `prepare_store` on existing key return `AlreadyExists` and do not mutate the existing entry.
4. Populate inserts MemoryTier entry (`FR-003`): successful `populate` creates a `MemoryTier` entry for the key with correct size.
5. Populate atomic failure (`Edge cases`): on allocation failure during populate, no partial dispatch-map entry remains.
6. Check correctness (`FR-008`): `check(key)` is equivalent to dispatch-map membership.
7. Lookup miss behavior (`FR-007`): missing key implies `KeyNotFound` and no state mutation.
8. Lookup MemoryTier hit behavior (`FR-006`): MemoryTier hit keeps key present and refreshes eviction metadata (touch effect).
9. Lookup BlockDevice promotion transition (`FR-028`): successful BlockDevice lookup transitions key back to `MemoryTier`.
10. Lookup Staging compatibility (`FR-006`): Staging lookup remains valid and succeeds.
11. Lookup size-match contract (User Story 2, Acceptance Scenario 4): if key exists but requested size differs from stored size, `lookup` returns `InvalidParameter` and does not perform partial copy or state mutation.
12. Remove postcondition (`FR-009`): successful `remove(key)` guarantees key absent afterward.
13. Remove miss behavior (User Story 4): removing absent key returns `KeyNotFound` and preserves state.
14. Touch semantics (`FR-023`): existing key updates timestamp only; absent key returns `KeyNotFound`.
15. Eviction attempt bound (`FR-024`): `evict_for_space` performs at most `MAX_ATTEMPTS=512` iterations.
16. Eviction success postcondition (`FR-024`): success implies `used + needed <= capacity`.
17. Eviction failure postcondition (`FR-024`): `AllocationFailed` implies the capacity condition was not achieved.
18. Clean eviction transition (`FR-024`): clean candidate eviction transitions `MemoryTier -> BlockDevice` (not removal).
19. Blind eviction fallback (`FR-024`): if transition-to-BlockDevice fails after blind LRU eviction, key is removed (no dangling state).
20. `prepare_store` argument validation (`FR-020`): `size == 0` returns `InvalidParameter` and preserves state.
21. Pending-write protocol (`FR-020`, `FR-021`, `FR-022`): `prepare_store` creates pending state; `commit`/`cancel` consume it exactly once.
22. Commit transition (`FR-021`): successful `commit_store` ends in `BlockDevice` and clears pending write.
23. Cancel transition (`FR-022`): successful `cancel_store` removes key and clears pending write.
24. Commit/cancel miss behavior (`FR-021`, `FR-022`): no pending write => `KeyNotFound`, no state mutation.
25. `clear_memory_tier` state postcondition (`FR-038`): after success, no entry remains in `MemoryTier`.
26. `clear_memory_tier` accounting (`FR-038`): returned count equals number of entries cleared from `MemoryTier`.
27. Recovery soundness (`FR-025`): each recovered extent yields a matching dispatch-map entry `(key, offset, size)`.
28. Drive-selection determinism (`FR-039`, clarifications): key-to-drive mapping is consistent (`key % num_drives`).
29. Config threshold consistency (`FR-033`): threshold and low-watermark comparisons use the intended direction and fields.
30. Exclusive-state invariant (global): each key is in exactly one logical entry state (`MemoryTier | BlockDevice | Staging | PendingWrite-transitional`).
31. Reference/state consistency invariant (`FR-013` intent): operations do not leave contradictory reference/state combinations.

## Secondary properties (defer to later phases)

1. Background write-through eventuality (`FR-004`, `FR-017`).
2. Shutdown writer/evictor drain and thread joins (`FR-014`, `FR-029`).
3. SSD evictor periodic hysteresis behavior (`FR-030`..`FR-033`).
4. Async stream semantics and synchronization (`FR-036`, `FR-037`).
5. Throughput/performance claims (`SC-012`, `SC-014`, parts of `FR-019`, `FR-039`).

## Notes for scope

- The core list favors state-machine correctness, safety invariants, and error/postcondition behavior.
- Performance and liveness-heavy claims are intentionally deferred because they require stronger environment modeling and/or temporal reasoning beyond a first Creusot pass.
