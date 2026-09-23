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
the **most-recently-used** (protected) end or the **least-recently-used**
(first-to-be-evicted) end. This lets a key that is estimated to be more valuable
than the current eviction victim displace it in eviction order, while a stream
of equally-cold (e.g. scan) keys stays bunched at the victim end — the classic
TinyLFU admission idea.

This component is a drop-in alternative to `eviction-policy-lru`: it provides
the same `IEvictionPolicy` interface with the same single `logger` receptacle,
and is selected via the `full-optimized` YAML server profile
(`apps/certus-server-yaml/profiles/full-optimized.yaml`).

## User Scenarios & Testing

### User Story 1 - Track and Evict Cache Entries (Priority: P1)

As a memory-tier or dispatch-map component,
I want to register cache entries and retrieve an eviction victim,
so that bounded memory is reclaimed under pressure while favouring entries
estimated to be more valuable than the current victim.

**Acceptance Scenarios**:

1. **Given** three first-time (never-before-seen) keys tracked in order into a
   fresh pool,
   **When** `identify_next_to_evict` is called repeatedly,
   **Then** the keys are returned most-recent-first — the first key is admitted
   at the MRU end (empty pool), and each subsequent first-time key has frequency
   estimate equal to the current head's (both 1), so it is admitted at the LRU
   head (`push_front`) and the last tracked is evicted first
   (`src/lib.rs:129-137`; test `track_and_evict_order`, `src/lib.rs:260-276`).
2. **Given** a tracked entry is touched,
   **When** `identify_next_to_evict` is called,
   **Then** the touched entry moves to the MRU end and is NOT evicted first
   (`src/lib.rs:142-156`; test `touch_moves_to_back`).
3. **Given** a tracked entry is removed,
   **When** `identify_next_to_evict` is called,
   **Then** the removed entry is never returned (`src/lib.rs:186-200`; test
   `remove_invalidates_entry`).

### User Story 2 - Frequency-Based Admission (Priority: P1)

As a cache under a scan-heavy or mixed workload,
I want a newly tracked key to be admitted at the protected (MRU) end only if its
estimated frequency exceeds that of the current eviction victim,
so that a colder or equally-cold key cannot displace an entry the sketch already
regards as more valuable.

**Acceptance Scenarios**:

1. **Given** a non-empty pool with a current victim (LRU-head) key,
   **When** a key is tracked whose estimated frequency is strictly greater than
   the victim's estimated frequency,
   **Then** the new key is admitted at the MRU end (`push_back`, protected).
2. **Given** a non-empty pool,
   **When** a key is tracked whose estimated frequency is less than OR equal to
   the victim's,
   **Then** the new key is admitted at the LRU head (`push_front`) and becomes
   the next eviction candidate.
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
   from pool A, **Then** pool B's entries and sketch remain unchanged (test
   `pools_are_independent`, `src/lib.rs:332-347`).

### User Story 4 - Concurrent Access (Priority: P1)

As a multi-threaded storage engine,
I want to safely call track/touch/remove/pop from multiple threads,
so that the policy is safe under concurrent workloads.

**Acceptance Scenarios**:

1. **Given** 4 threads each tracking 100 entries in the same pool, **When** all
   threads complete, **Then** the pool contains exactly 400 entries with no
   corruption (test `concurrent_access`, `src/lib.rs:382-409`).

## Requirements

### Functional Requirements

- **FR-001**: System MUST provide `create_pool()` returning a new, unique
  `PoolId`. Pool IDs are sequential starting from 0. Each pool owns its own LRU
  list AND its own Count-Min Sketch, `access_count`, and `max_len` counters
  (`src/lib.rs:89-102`).
- **FR-002**: System MUST provide `track(pool, key, semantics)` that (a)
  increments the pool's frequency sketch for `key`, (b) performs periodic aging
  (see FR-013), (c) applies the admission decision (FR-011) to choose the LRU
  insertion end, and (d) returns an opaque `EvictionHandle` for O(1) subsequent
  operations. The `semantics: BlockSemantics` argument is accepted for interface
  conformance but ignored by this policy (`src/lib.rs:108`, `_semantics`).
  **Re-registration is NOT idempotent**: tracking a key already tracked in the
  same pool creates a *new* node and returns a *new* handle rather than
  refreshing an existing one (same deliberate divergence from the interface's
  general idempotency clause as `eviction-policy-lru`). Consumers
  (`dispatch-map`, `memory-tier`) enforce a `contains_key`/`AlreadyExists` guard
  upstream and MUST NOT `track` a key still tracked in the pool.
- **FR-003**: System MUST provide `touch(handle)` that moves the referenced
  entry to the MRU end in O(1) time. `touch` does NOT update the frequency
  sketch (only `track` does) (`src/lib.rs:142-156`).
- **FR-004**: System MUST provide `batch_touch(handles)` that moves multiple
  entries to the MRU end, acquiring each pool's lock once across a run of
  handles for the same pool to amortize lock overhead. Empty input is a no-op
  returning `Ok(())` (`src/lib.rs:158-184`).
- **FR-005**: System MUST provide `remove(handle)` that unlinks the referenced
  entry from the ordering in O(1) time (`src/lib.rs:186-200`).
- **FR-006**: System MUST provide `identify_next_to_evict(pool)` that removes and
  returns the LRU-head key from the pool in O(1) time, or `None` if the pool is
  empty (or the pool does not exist) (`src/lib.rs:202-207`).
- **FR-007**: System MUST provide `get_eviction_candidates(pool, n)` that returns
  up to `n` LRU-head keys without removing them, in O(n) time. Returns an empty
  vector for a non-existent pool (`src/lib.rs:209-218`).
- **FR-008**: System MUST provide `len(pool)` returning the number of active
  entries in the pool, or `0` for a non-existent pool (`src/lib.rs:220-229`).
- **FR-009**: System MUST provide `clear_pool(pool)` that removes all entries
  from the pool (no-op for a non-existent pool). Clearing resets the LRU list
  but does NOT reset the frequency sketch, `access_count`, or `max_len` — the
  learned frequency history persists across a clear (`src/lib.rs:231-237`;
  `clear` on the list only, `src/lru_list.rs:186-192`).
- **FR-010**: Methods returning `Result` (`track`, `touch`, `batch_touch`,
  `remove`) MUST return `EvictionPolicyError::InvalidPool` for a non-existent
  pool. Methods returning `Option`/collection/scalar
  (`identify_next_to_evict`, `get_eviction_candidates`, `len`, `clear_pool`)
  MUST gracefully degrade (return `None`, empty, `0`, or no-op)
  (`src/lib.rs:111-118,144-152,164-167,175-178,188-196,204,211,222,233`).
- **FR-011**: On `track` into a **non-empty** pool the system MUST apply the
  TinyLFU admission gate: let `new_est` be the sketch estimate of the tracked
  key and `victim_est` the estimate of the current LRU-head key. The key is
  admitted at the LRU head (`push_front`, first to be evicted) when
  `new_est <= victim_est`; otherwise (`new_est > victim_est`) it is admitted at
  the MRU end (`push_back`, protected) (`src/lib.rs:129-137`). On `track` into an
  **empty** pool the key is admitted unconditionally at the back
  (`src/lib.rs:136`). Note there is no absolute frequency threshold — the
  decision depends only on the comparison against the current victim.
- **FR-012**: `touch` and `remove` on an already-removed handle MUST be
  idempotent (no panic, no effect), returning `Ok(())` silently. The guard is
  the node's `active` flag in the LRU list (`src/lru_list.rs:108-111`
  `move_to_back`, `:159-162` `remove`). The `InvalidHandle` error variant is
  defined in the interface but currently unused.
- **FR-013**: The system MUST periodically age the frequency sketch to bound
  counter growth and let the policy adapt to shifting working sets: on each
  `track`, after updating `max_len` (the high-water mark of pool size) and
  incrementing `access_count`, if `max_len > 0` and
  `access_count % (max_len * 10) == 0` the sketch is halved (every counter
  right-shifted by 1) (`src/lib.rs:121-127`, `CountMinSketch::halve`
  `src/lib.rs:54-60`).

### Non-Functional Requirements

- **NFR-001**: All single-entry operations (`track`, `touch`, `remove`,
  `identify_next_to_evict`) MUST be O(1). Sketch `increment`/`estimate`/`halve`
  are bounded by the fixed sketch size (constant `CMS_ROWS`, and `halve` by
  `CMS_ROWS * CMS_COLS`), independent of pool size (`src/lib.rs:18-19,38-60`).
- **NFR-002**: The component MUST be thread-safe — concurrent access MUST NOT
  cause data corruption (`RwLock<EvictionState>` + per-pool `Mutex`,
  `src/lib.rs:71-73,83,90,110,119`).
- **NFR-003**: Per-pool locking granularity — operations on different pools MUST
  NOT contend (except during `create_pool`, which write-locks the state).
- **NFR-004**: The Count-Min Sketch MUST use bounded memory that is independent
  of the number of tracked keys: a fixed `CMS_ROWS × CMS_COLS` array of `u8`
  saturating counters per pool (`src/lib.rs:27-29`; see Key Entities for current
  dimensions).
- **NFR-005**: The component MUST conform to the Certus component model
  (`define_component!`, provides `IEvictionPolicy`, single `logger` receptacle),
  making it a drop-in replacement for `eviction-policy-lru` (`src/lib.rs:75-86`).

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
  and `push_back` so the admission gate can choose the insertion end
  (`src/lru_list.rs`).
- **CountMinSketch**: Per-pool frequency estimator. Current dimensions:
  `CMS_ROWS = 4` rows × `CMS_COLS = 1024` columns of `u8` saturating counters
  (`src/lib.rs:18-19`). Each row uses a distinct fixed odd multiplier
  (`CMS_PRIMES`, `src/lib.rs:20-25`); the column index is
  `(key.wrapping_mul(prime) >> 54)` (top 10 bits → 1024 buckets,
  `src/lib.rs:40,48`). `estimate` returns the minimum counter across rows;
  `increment` adds 1 (saturating) to each row's counter; `halve` right-shifts
  every counter by 1.
- **Pool**: `{ lru: LruList, sketch: CountMinSketch, access_count: u64,
  max_len: usize }` (`src/lib.rs:63-68`).

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
  exactly 400 entries and no panic or corruption (`src/lib.rs:382-409`).
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
- The admission rule compares only against the current victim
  (`new_est > victim_est`); it has no absolute frequency threshold. A run of
  equally-cold first-time keys (all estimate 1) therefore piles at the LRU head
  and is evicted most-recent-first, which the `track_and_evict_order` test
  asserts.
- The aging period (`max_len * 10` accesses) is a hard-coded constant of the
  current tuning, not yet configurable.
- `max_len` tracks the pool's high-water mark, not its current size; it never
  decreases, so the aging period only lengthens over a pool's lifetime.
- `clear_pool` intentionally preserves the sketch so frequency history survives
  a logical clear; whether this is desirable is a review question.
- This implementation emits no startup banner; the only logging is `debug` on
  pool creation and `warn` on invalid-pool `track`/`touch`/`remove`.
