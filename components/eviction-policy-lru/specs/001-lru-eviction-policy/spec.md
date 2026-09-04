# Feature Specification: LRU Eviction Policy Component

**Feature Branch**: `001-lru-eviction-policy`
**Created**: 2026-06-19
**Status**: Backfilled
**Source**: Generated from existing implementation

## Backfill Notice

> This spec was generated from existing code via `speckit.sync.backfill`.
> It documents current behavior, not original intent.
> Review carefully and update to reflect desired behavior.

## Overview

A shared, thread-safe LRU (Least Recently Used) eviction-policy component that provides O(1) tracking, touch, remove, and eviction-victim selection. It supports multiple independent pools within a single instance, enabling shared use across the memory-tier (16 pools) and dispatch-map (1 pool) components.

## User Scenarios & Testing

### User Story 1 - Track and Evict Cache Entries (Priority: P1)

As a memory-tier or dispatch-map component,
I want to register cache entries and retrieve the least-recently-used entry for eviction,
so that bounded memory is reclaimed in LRU order under pressure.

**Acceptance Scenarios**:

1. **Given** entries tracked in FIFO order, **When** `identify_next_to_evict` is called, **Then** the first-tracked entry is returned.
2. **Given** a tracked entry is touched, **When** `identify_next_to_evict` is called, **Then** the touched entry is NOT evicted first; the next-oldest is.
3. **Given** a tracked entry is removed, **When** `identify_next_to_evict` is called, **Then** the removed entry is never returned.

### User Story 2 - Multiple Independent Pools (Priority: P1)

As a system with separate eviction domains (e.g., per-NUMA-node pools),
I want independent eviction tracking per pool,
so that eviction decisions in one domain do not affect another.

**Acceptance Scenarios**:

1. **Given** two pools each with distinct entries, **When** an entry is popped from pool A, **Then** pool B's entries remain unchanged.
2. **Given** an entry tracked in pool A, **When** `touch` or `remove` is called with the entry's handle, **Then** only pool A's ordering is affected.

### User Story 3 - Concurrent Access (Priority: P1)

As a multi-threaded storage engine,
I want to safely call track/touch/remove/pop from multiple threads,
so that the eviction policy is safe under concurrent workloads.

**Acceptance Scenarios**:

1. **Given** 4 threads each tracking 100 entries in the same pool, **When** all threads complete, **Then** the pool contains exactly 400 entries with no corruption.

## Requirements

### Functional Requirements

- **FR-001**: System MUST provide `create_pool()` returning a new, unique `PoolId`. Pool IDs are sequential starting from 0.
- **FR-002**: System MUST provide `track(pool, key, semantics)` that registers a key in the given pool as most-recently-used and returns an opaque `EvictionHandle` for O(1) subsequent operations. The `semantics: BlockSemantics` argument carries per-block hints (e.g. `session_id`) for lineage-aware policies; this LRU policy accepts it for interface conformance but ignores it (`src/lib.rs:57`, `_semantics`). **Re-registration is NOT idempotent in this policy**: calling `track` again for a key already tracked in the same pool creates a *new* node and returns a *new* handle (`src/lib.rs:69` unconditionally `push_back`s), rather than refreshing recency and returning the existing handle. This deliberately diverges from the general idempotent-re-registration clause in the shared `IEvictionPolicy` interface doc (`components/interfaces/src/ieviction_policy.rs`), which was authored for and is honored only by lineage-aware policies (e.g. `eviction-policy-session-lists`, whose `register()` dedupes via a `by_key` map); LRU's non-idempotent behavior was intentionally preserved when that clause was added (interfaces commit `1da4e777`). Callers therefore MUST NOT `track` a key that is still tracked in the pool — all production consumers (`dispatch-map`, `memory-tier`) enforce this upstream with a `contains_key`/`AlreadyExists` guard before calling `track`. *(Spec-sync backfill 2026-09-04: documented the non-idempotent re-registration behavior and its deliberate divergence from the interface's general idempotency clause; no code change — see drift report.)*
- **FR-003**: System MUST provide `touch(handle)` that moves the referenced entry to most-recently-used position in O(1) time.
- **FR-004**: System MUST provide `remove(handle)` that unlinks the referenced entry from the ordering in O(1) time.
- **FR-005**: System MUST provide `identify_next_to_evict(pool)` that removes and returns the least-recently-used key from the pool in O(1) time, or `None` if the pool is empty.
- **FR-006**: System MUST provide `get_eviction_candidates(pool, n)` that returns up to `n` least-recently-used keys without removing them, in O(n) time.
- **FR-007**: System MUST provide `len(pool)` that returns the number of active entries in the pool.
- **FR-008**: System MUST provide `clear_pool(pool)` that removes all entries from the pool, resetting it to empty.
- **FR-009**: Methods returning `Result` (`track`, `touch`, `remove`) MUST return `EvictionPolicyError::InvalidPool` when given a non-existent pool. Methods returning `Option` or scalar (`identify_next_to_evict`, `get_eviction_candidates`, `len`, `clear_pool`) MUST gracefully degrade: returning `None`, empty collection, `0`, or no-op respectively.
- **FR-010**: `touch` and `remove` on an already-removed handle MUST be idempotent (no panic, no effect). These return `Ok(())` silently rather than `Err(InvalidHandle)` — the `InvalidHandle` error variant is defined in the interface but is currently unused (reserved for future stricter validation).
- **FR-012**: The component MUST provide a `batch_touch(handles: &[EvictionHandle])` method that marks multiple entries as most-recently-used in a single lock acquisition, amortizing lock overhead for the hot-path batch lookup use case.
- **FR-011**: Removed node slots MUST be recycled via a free list to avoid unbounded memory growth for long-lived pools with high churn.

### Non-Functional Requirements

- **NFR-001**: All single-entry operations (`track`, `touch`, `remove`, `identify_next_to_evict`) MUST be O(1).
- **NFR-002**: The component MUST be thread-safe — concurrent access from multiple threads MUST NOT cause data corruption.
- **NFR-003**: Per-pool locking granularity — operations on different pools MUST NOT contend with each other (except during pool creation).
- **NFR-004**: The component MUST conform to the Certus component model (`define_component!`, provides `IEvictionPolicy`, receptacle for `ILogger`).

## Key Entities

- **CacheKey** (`u64`): The cache key tracked for eviction ordering, defined in the `interfaces` crate (`idispatch_map::CacheKey`) and used verbatim throughout `src/` and the `IEvictionPolicy` interface. *(Spec-sync backfill 2026-09-04: there is no distinct `EvictionKey` type or alias anywhere in the code — the earlier "EvictionKey (u64) … same underlying type as CacheKey" entry named a type that does not exist. The tracked key is `CacheKey`.)*
- **PoolId** (`u32`): Identifier for an independent eviction-tracking pool.
- **EvictionHandle**: Opaque handle embedding `(pool_id, index)` returned by `track()` for O(1) touch/remove.
- **BlockSemantics**: Per-block hint struct passed by value to `track()` (currently `session_id: SessionId`, `Default` yields `session_id = 0`). Ignored by this LRU policy; consumed by lineage-aware policies. Extensible without changing the `track` signature.
- **EvictionPolicyError**: Error enum with variants `InvalidPool(PoolId)` and `InvalidHandle`.
- **LruList**: Internal index-based doubly-linked list with free-list recycling.

## Dependencies

- **component-framework**: Provides `define_component!` macro, component lifecycle.
- **interfaces**: Provides `IEvictionPolicy` trait definition and associated types.
- **Consumers**: `dispatch-map`, `memory-tier`, `dispatcher`, `dispatcher-p2p`, `certus-connector`, `certus-server`, `certus-server-yaml`.

## Success Criteria

- **SC-001**: All tests in `lib.rs` and `lru_list.rs` pass (`cargo test -p eviction-policy-lru`).
- **SC-002**: Concurrent access test with 4 threads x 100 entries completes without panic or data corruption.
- **SC-003**: `cargo clippy -- -D warnings` and `cargo fmt --check` pass cleanly.
- **SC-004**: Component integrates via `query_interface!` + receptacle wiring in all consumer crates.

## Implementation Notes

> These notes capture current implementation details that may or may not
> belong in the spec long-term.

- The internal data structure is a `Vec<Node>`-based doubly-linked list indexed by `u32`, avoiding pointer indirection and allocation per entry.
- Each pool is protected by its own `Mutex`, wrapped in a `Vec` behind a top-level `RwLock` (read-locked for all operations except `create_pool`).
- Free-list recycling reuses removed node slots so the `Vec` does not grow monotonically for long-lived pools with high churn.
- The `active` flag on each node enables idempotent remove/move operations on stale handles.
