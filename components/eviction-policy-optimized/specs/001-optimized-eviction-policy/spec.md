# Feature Specification: Optimized (TinyLFU) Eviction Policy Component

**Feature Branch**: `001-optimized-eviction-policy`
**Created**: 2026-09-23
**Status**: Backfilled
**Source**: Generated from existing implementation

## Backfill Notice

> This spec was generated from existing code via `speckit.sync.backfill`.
> It documents current behavior, not original intent.
> Review carefully and update to reflect desired behavior.

## Overview

A shared, thread-safe eviction-policy component that augments an O(1) LRU
ordering with a **TinyLFU admission filter**. Like `eviction-policy-lru` it
provides O(1) track/touch/remove/pop and supports multiple independent pools
within a single instance (memory-tier: 16 pools, dispatch-map: 1 pool). The
distinguishing behavior is admission control: on `track`, a frequency estimate
from a per-pool Count-Min Sketch decides whether a newly tracked key enters at
the **most-recently-used** end (protected) or the **least-recently-used** end
(first to be evicted). This prevents a burst of one-off (scan) keys from
displacing established hot keys — the classic TinyLFU admission policy.

This component is a drop-in alternative to `eviction-policy-lru`: it provides
the same `IEvictionPolicy` interface with the same single `logger` receptacle,
and is selected via the `full-optimized` YAML server profile
(`apps/certus-server-yaml/profiles/full-optimized.yaml`).

## User Scenarios & Testing

### User Story 1 - Track and Evict Cache Entries (Priority: P1)

As a memory-tier or dispatch-map component,
I want to register cache entries and retrieve an eviction victim,
so that bounded memory is reclaimed under pressure while favouring frequently
reused entries.

**Acceptance Scenarios**:

1. **Given** three first-time (never-before-seen) keys tracked in order,
   **When** `identify_next_to_evict` is called repeatedly,
   **Then** the keys are returned most-recent-first — first-time keys (frequency
   estimate = 1) are admitted at the LRU head (`push_front`), so the last
   tracked is evicted first (`src/lib.rs:289`).
2. **Given** a tracked entry is touched,
   **When** `identify_next_to_evict` is called,
   **Then** the touched entry moves to the MRU end and is NOT evicted first.
3. **Given** a tracked entry is removed,
   **When** `identify_next_to_evict` is called,
   **Then** the removed entry is never returned.

### User Story 2 - Frequency-Based Admission (Priority: P1)

As a cache under a scan-heavy or mixed workload,
I want a newly tracked key to be admitted at the protected (MRU) end only if it
is estimated to be at least as valuable as the current eviction victim,
so that a flood of one-off keys does not evict established hot keys.

**Acceptance Scenarios**:

1. **Given** a non-empty pool with a current victim (LRU-head) key,
   **When** a key is tracked whose estimated frequency is `>= 3` AND strictly
   greater than the victim's estimated frequency,
   **Then** the new key is admitted at the MRU end (`push_back`, protected).
2. **Given** a non-empty pool,
   **When** a key is tracked whose estimated frequency is `< 3` OR not greater
   than the victim's,
   **Then** the new key is admitted at the LRU head (`push_front`) and is a
   candidate for immediate eviction.
3. **Given** an empty pool,
   **When** any key is tracked,
   **Then** it is admitted unconditionally (`push_back`), since there is no
   victim to compare against.

### User Story 3 - Multiple Independent Pools (Priority: P1)

As a system with separate eviction domains (e.g., per-NUMA-node pools),
I want independent eviction tracking and independent frequency sketches per
pool,
so that decisions in one domain do not affect another.

**Acceptance Scenarios**:

1. **Given** two pools each with distinct entries, **When** an entry is popped
   from pool A, **Then** pool B's entries and sketch remain unchanged.

### User Story 4 - Concurrent Access (Priority: P1)

As a multi-threaded storage engine,
I want to safely call track/touch/remove/pop from multiple threads,
so that the policy is safe under concurrent workloads.

**Acceptance Scenarios**:

1. **Given** 4 threads each tracking 100 entries in the same pool, **When** all
   threads complete, **Then** the pool contains exactly 400 entries with no
   corruption.

## Requirements

### Functional Requirements

- **FR-001**: System MUST provide `create_pool()` returning a new, unique
  `PoolId`. Pool IDs are sequential starting from 0. Each pool owns its own LRU
  list AND its own Count-Min Sketch, `access_count`, and `max_len` counters
  (`src/lib.rs:104`).
- **FR-002**: System MUST provide `track(pool, key, semantics)` that (a)
  increments the pool's frequency sketch for `key`, (b) performs periodic aging
  (see FR-013), (c) applies the admission decision (FR-011) to choose the LRU
  insertion end, and (d) returns an opaque `EvictionHandle` for O(1) subsequent
  operations. The `semantics: BlockSemantics` argument is accepted for interface
  conformance but ignored by this policy (`src/lib.rs:122`, `_semantics`).
  **Re-registration is NOT idempotent**: tracking a key already tracked in the
  same pool creates a *new* node and returns a *new* handle rather than
  refreshing an existing one (same deliberate divergence from the interface's
  general idempotency clause as `eviction-policy-lru`). Consumers
  (`dispatch-map`, `memory-tier`) enforce a `contains_key`/`AlreadyExists` guard
  upstream and MUST NOT `track` a key still tracked in the pool.
- **FR-003**: System MUST provide `touch(handle)` that moves the referenced
  entry to the MRU end in O(1) time. `touch` does NOT update the frequency
  sketch (only `track` does).
- **FR-004**: System MUST provide `batch_touch(handles)` that moves multiple
  entries to the MRU end, acquiring each pool's lock once across a run of
  handles for the same pool to amortize lock overhead. Empty input is a no-op
  returning `Ok(())`.
- **FR-005**: System MUST provide `remove(handle)` that unlinks the referenced
  entry from the ordering in O(1) time.
- **FR-006**: System MUST provide `identify_next_to_evict(pool)` that removes and
  returns the LRU-head key from the pool in O(1) time, or `None` if the pool is
  empty (or the pool does not exist).
- **FR-007**: System MUST provide `get_eviction_candidates(pool, n)` that returns
  up to `n` LRU-head keys without removing them, in O(n) time. Returns an empty
  vector for a non-existent pool.
- **FR-008**: System MUST provide `len(pool)` returning the number of active
  entries in the pool, or `0` for a non-existent pool.
- **FR-009**: System MUST provide `clear_pool(pool)` that removes all entries
  from the pool (no-op for a non-existent pool). Clearing resets the LRU list
  but does NOT reset the frequency sketch, `access_count`, or `max_len` — the
  learned frequency history persists across a clear (`src/lib.rs:248`).
- **FR-010**: Methods returning `Result` (`track`, `touch`, `batch_touch`,
  `remove`) MUST return `EvictionPolicyError::InvalidPool` for a non-existent
  pool. Methods returning `Option`/collection/scalar
  (`identify_next_to_evict`, `get_eviction_candidates`, `len`, `clear_pool`)
  MUST gracefully degrade (return `None`, empty, `0`, or no-op).
- **FR-011**: On `track` into a **non-empty** pool the system MUST apply the
  TinyLFU admission gate: let `new_est` be the sketch estimate of the tracked
  key and `victim_est` the estimate of the current LRU-head key. The key is
  admitted at the MRU end (`push_back`, protected) **if and only if**
  `new_est >= 3 && new_est > victim_est`; otherwise it is admitted at the LRU
  head (`push_front`, first to be evicted) (`src/lib.rs:144-154`). On `track`
  into an **empty** pool the key is admitted unconditionally at the back.
- **FR-012**: `touch` and `remove` on an already-removed handle MUST be
  idempotent (no panic, no effect), returning `Ok(())` silently. The
  `InvalidHandle` error variant is defined in the interface but currently unused.
- **FR-013**: The system MUST periodically age the frequency sketch to bound
  counter growth and let the policy adapt to shifting working sets: on each
  `track`, after updating `max_len` (the high-water mark of pool size) and
  incrementing `access_count`, if `max_len > 0` and
  `access_count % (max_len * 2) == 0` the sketch is halved (every counter
  right-shifted by 1) (`src/lib.rs:136-142`).

### Non-Functional Requirements

- **NFR-001**: All single-entry operations (`track`, `touch`, `remove`,
  `identify_next_to_evict`) MUST be O(1). Sketch `increment`/`estimate`/`halve`
  are bounded by the fixed sketch size (constant `CMS_ROWS`, and `halve` by
  `CMS_ROWS * CMS_COLS`), independent of pool size.
- **NFR-002**: The component MUST be thread-safe — concurrent access MUST NOT
  cause data corruption.
- **NFR-003**: Per-pool locking granularity — operations on different pools MUST
  NOT contend (except during `create_pool`).
- **NFR-004**: The Count-Min Sketch MUST use bounded memory that is independent
  of the number of tracked keys: a fixed `CMS_ROWS × CMS_COLS` array of `u8`
  saturating counters per pool (see Key Entities for current dimensions).
- **NFR-005**: The component MUST conform to the Certus component model
  (`define_component!`, provides `IEvictionPolicy`, single `logger` receptacle),
  making it a drop-in replacement for `eviction-policy-lru`.
- **NFR-006**: On the first `create_pool` call (the earliest point the `logger`
  receptacle is bound) the component SHOULD emit a one-time startup banner
  describing the policy; the banner MUST be emitted at most once per component
  instance regardless of pool count (`src/lib.rs:66,96`).

## Key Entities

- **CacheKey** (`u64`): The cache key tracked for eviction ordering and hashed
  into the sketch, defined in the `interfaces` crate.
- **PoolId** (`u32`): Identifier for an independent eviction-tracking pool.
- **EvictionHandle**: Opaque handle embedding `(pool_id, index)` returned by
  `track()` for O(1) touch/remove.
- **BlockSemantics**: Per-block hint struct passed by value to `track()`;
  ignored by this policy.
- **EvictionPolicyError**: Error enum with variants `InvalidPool(PoolId)` and
  `InvalidHandle`.
- **LruList**: Internal index-based doubly-linked list with free-list recycling
  (identical structure to `eviction-policy-lru`), exposing both `push_front`
  and `push_back` so the admission gate can choose the insertion end.
- **CountMinSketch**: Per-pool frequency estimator. Current dimensions:
  `CMS_ROWS = 4` rows × `CMS_COLS = 8192` columns of `u8` saturating counters.
  Each row uses a distinct fixed odd multiplier (`CMS_PRIMES`); the column index
  is `(key.wrapping_mul(prime) >> 51)` (top 13 bits → 8192 buckets).
  `estimate` returns the minimum counter across rows; `increment` adds 1
  (saturating) to each row's counter; `halve` right-shifts every counter by 1.
- **Pool**: `{ lru: LruList, sketch: CountMinSketch, access_count: u64,
  max_len: usize }`.

## Dependencies

- **component-framework** / **component-core**: component model macros,
  lifecycle, `query_interface!`.
- **interfaces**: `IEvictionPolicy` trait and associated types.
- **Consumers**: `dispatch-map`, `memory-tier` (wired via the `full-optimized`
  server profile); interchangeable with `eviction-policy-lru` at the
  `IEvictionPolicy` receptacle.

## Success Criteria

- **SC-001**: All tests in `lib.rs` and `lru_list.rs` pass
  (`cargo test -p eviction-policy-optimized`).
- **SC-002**: The concurrent test (4 threads × 100 entries) completes with
  exactly 400 entries and no panic or corruption.
- **SC-003**: `cargo clippy -- -D warnings` and `cargo fmt --check` pass cleanly.
- **SC-004**: The component builds and wires into the `full-optimized` profile
  (`CERTUS_PROFILE=full-optimized cargo build -p certus-server-yaml --release`)
  and is selectable in place of `eviction-policy-lru` with no other wiring
  changes.

## Implementation Notes

> These notes capture current implementation details that may or may not
> belong in the spec long-term.

- The LRU list is the same index-based doubly-linked list used by
  `eviction-policy-lru`, with free-list recycling. The only structural
  difference in `track` is the choice between `push_front` and `push_back`.
- The admission threshold (`new_est >= 3`) and the aging period
  (`max_len * 2` accesses) are hard-coded constants of the current tuning, not
  yet configurable.
- `max_len` tracks the pool's high-water mark, not its current size; it never
  decreases, so the aging period only lengthens over a pool's lifetime.
- `clear_pool` intentionally preserves the sketch so frequency history survives
  a logical clear; whether this is desirable is a review question.
- The startup banner is guarded by a `std::sync::Once` (`STARTUP_BANNER`) shared
  across all instances in the process; with multiple component instances only
  the first would log the banner. Single-instance is the production case.
