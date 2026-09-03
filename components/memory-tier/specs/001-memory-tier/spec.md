# Feature Specification: Memory Tier (DRAM Cache Pool)

**Feature Branch**: `001-memory-tier`
**Created**: 2026-07-08
**Status**: Backfilled — aligned to implementation
**Source**: Generated from existing implementation
**Last-Synced**: 2026-09-03 (spec-sync: version reconciled to 0.3.0 and residual interface-doc sharding drift resolved; single-`RwLock<Pool>` reality confirmed. See "Spec-Sync Notes" at end)

## Backfill Notice
> This spec was generated from existing code via `speckit.sync.backfill`.
> It documents current behavior, not original intent.
> Review carefully and update to reflect desired behavior.

## Overview

The `memory-tier` component provides a high-performance DRAM-resident cache pool for the Certus storage system. It serves as a fast staging area between higher-level dispatch logic and persistent NVMe storage, enabling low-latency reads for cached objects and write-staging for objects destined for disk.

The pool is held as a single, unsharded structure guarded by one `RwLock<Pool>`: a single first-fit free-list allocator and a single `HashMap<CacheKey, Slot>` slot map. Read operations (`get`, `peek`, `contains`, `batch_touch`, `capacity`, `used`) take a shared read lock; mutations (`insert`, `remove`, `evict`, `clear`) take an exclusive write lock. Eviction decisions are delegated to an external `IEvictionPolicy` component via a receptacle binding, which provides pluggable eviction strategies (currently LRU) and its own internal synchronization; eviction-order touches are performed after releasing the pool lock.

Memory is allocated as a single contiguous region via `mmap` (preferring 2 MiB hugepage backing) or SPDK DMA-capable hugepages when the SPDK environment is active. All internal allocations are 4 KiB-aligned, making returned pointers directly usable for NVMe DMA transfers without additional alignment or pinning.

## User Scenarios & Testing

### User Story 1 - Cache Object Insertion and Retrieval (Priority: P1)

**As** a dispatcher thread,
**I want** to insert an object into the DRAM cache by key and later retrieve it by the same key,
**so that** subsequent reads for the same object are served from DRAM without going to NVMe.

**Acceptance Criteria:**
- `insert(key, size)` allocates a slot and returns a valid writable pointer
- `get(key)` returns the same pointer and size, and updates LRU recency
- `peek(key)` returns the pointer and size without updating LRU recency
- `contains(key)` returns true after insert and false after removal
- Duplicate insertion for the same key returns `AlreadyExists` error

**Test Coverage:**
- `insert_and_get` - verifies pointer round-trip
- `insert_duplicate_fails` - verifies AlreadyExists error
- `contains` - verifies presence tracking
- `peek_does_not_update_eviction_order` - verifies peek does not update eviction order

### User Story 2 - Eviction Under Memory Pressure (Priority: P1)

**As** the dispatch layer,
**I want** to evict the entry chosen by the eviction policy when the pool is full,
**so that** space is freed for new insertions without requiring external coordination.

**Acceptance Criteria:**
- `evict_next()` removes the eviction policy's next victim (chosen by the bound `IEvictionPolicy` via `identify_next_to_evict`) from the single global pool
- `evict_next_for_key(key)` is equivalent to `evict_next()`; because the pool is not sharded, the `key` argument does not constrain which victim is chosen (any freed space is globally allocatable)
- Eviction correctly frees the allocation and removes the slot from the index
- After eviction, the freed space is available for new allocations
- `oldest_keys(n)` returns up to N keys in oldest-first order without removing them

**Test Coverage:**
- `evict_next_returns_some` - verifies eviction returns a key
- `touch_updates_eviction_order` - verifies touch updates the entry's eviction-order position
- `pool_full_returns_error` - verifies PoolFull when pool capacity is exhausted

### User Story 3 - Pool Initialization with NUMA and DMA Awareness (Priority: P1)

**As** the system startup sequence,
**I want** to initialize the memory pool with a specified size and optional NUMA node binding,
**so that** allocations are local to the processing cores and DMA-ready for NVMe transfers.

**Acceptance Criteria:**
- `initialize(pool_size, numa_node)` allocates a contiguous pool of the specified size
- Pool allocation prefers hugepage backing (MAP_HUGETLB) with fallback to regular pages
- When SPDK env is active, uses `spdk_zmalloc` for DMA-capable hugepages
- Optional NUMA binding via `mbind(MPOL_BIND)` with graceful fallback on failure
- Double-initialization returns an error
- Zero pool size returns `InvalidSize` error
- `is_dma_capable()` returns true only when SPDK-allocated

**Test Coverage:**
- `initialize_twice_fails` - verifies double-init rejection
- `insert_zero_size_fails` - verifies zero-size rejection

### User Story 4 - Explicit Removal and Cache Clear (Priority: P2)

**As** the write-through completion handler,
**I want** to explicitly remove a specific entry or clear the entire cache,
**so that** stale entries are cleaned up after successful persistence.

**Acceptance Criteria:**
- `remove(key)` frees the slot and returns Ok
- `remove(absent_key)` returns `KeyNotFound`
- `clear()` removes all entries from the pool and returns the count
- After clear, `used()` returns 0 and previous keys are no longer contained

**Test Coverage:**
- `remove_and_reuse` - verifies removal frees space for reuse
- `clear_resets_all` - verifies full cache clear

### User Story 5 - Capacity Monitoring (Priority: P2)

**As** an operator or monitoring subsystem,
**I want** to query the pool's total capacity and current usage,
**so that** I can track memory pressure and trigger proactive eviction.

**Acceptance Criteria:**
- `capacity()` returns the total pool size
- `used()` returns the sum of all currently-allocated bytes (4 KiB-aligned)
- `pool_info()` returns the base pointer and total pool size for CUDA host registration

**Test Coverage:**
- `capacity_and_used` - verifies accounting correctness

### User Story 6 - Batch LRU Touch for Hot-Path Throughput (Priority: P2)

**As** a dispatcher processing a batch of read hits,
**I want** to update LRU positions for multiple keys in a single call,
**so that** lock acquisition overhead is amortized over the batch.

**Acceptance Criteria:**
- `batch_touch(keys)` updates LRU for all keys in the batch
- Keys not present in the cache are silently skipped
- Empty batch is a no-op

**Test Coverage:**
- No dedicated test currently (relies on integration with eviction-policy-lru)

### User Story 7 - Operational Telemetry (Priority: P3)

**As** an operator or monitoring subsystem,
**I want** to observe eviction counts and lock-contention counters for the pool,
**so that** I can diagnose contention hotspots and tune eviction/allocation behavior.

**Acceptance Criteria:**
- When compiled with the optional `telemetry` Cargo feature, the component tracks:
  - total eviction count
  - write-lock contention count (lock not acquired on first attempt)
  - read-lock contention count (lock not acquired on first attempt)
- `telemetry_snapshot()` (interface method, always available) returns the current counters, or all-zero when the `telemetry` feature is disabled
- `telemetry()` / `reset_telemetry()` (inherent methods, feature-gated) expose and reset the same counters
- The feature is zero-cost when disabled: no atomics are allocated or updated

**Test Coverage:**
- No dedicated test currently (feature-gated; not exercised by the default `cargo test -p memory-tier` run)

**Backfill note**: This user story documents functionality identified during spec-drift analysis (`speckit.sync.backfill`, 2026-07-22) that existed in code but was not previously captured in this spec.

## Requirements

### Functional Requirements

| ID | Requirement | Verified |
|----|-------------|----------|
| FR-001 | Pool is allocated as a single contiguous mmap'd region | Implementation |
| FR-002 | Hugepage backing (MAP_HUGETLB) is preferred with automatic fallback | Implementation |
| FR-003 | When SPDK env is active, pool uses spdk_zmalloc for DMA-capable memory | Implementation |
| FR-004 | All allocations are 4 KiB-aligned for NVMe DMA compatibility | Unit test |
| FR-005 | Pool state (allocator + slot map) is held as a single, unsharded structure behind one `RwLock<Pool>` (no shards) | Implementation |
| FR-006 | Concurrency uses one reader-writer lock: read operations (`get`, `peek`, `contains`, `batch_touch`, `capacity`, `used`) take a shared read lock; mutations (`insert`, `remove`, `evict`, `clear`) take an exclusive write lock | Implementation |
| FR-007 | A single first-fit `FreeList` allocator and a single `HashMap<CacheKey, Slot>` slot map serve the whole pool | Implementation |
| FR-008 | insert() rejects zero size with InvalidSize | Unit test |
| FR-009 | insert() rejects duplicate keys with AlreadyExists | Unit test |
| FR-010 | insert() returns PoolFull when the allocator cannot satisfy the request | Unit test |
| FR-011 | get() returns pointer and size, and updates eviction order via eviction policy | Unit test |
| FR-012 | peek() returns pointer and size without eviction-order update | Unit test |
| FR-013 | evict_next() delegates victim selection to the eviction policy (`identify_next_to_evict(pool_id)`), then removes that slot and frees its allocation; there is no shard round-robin counter | Implementation |
| FR-014 | evict_next_for_key(key) is an alias for evict_next(); the `key` argument is ignored because the pool is not sharded | Implementation |
| FR-015 | remove() frees the slot and returns KeyNotFound for absent keys | Unit test |
| FR-016 | touch() updates the entry's eviction-order position without returning data | Unit test |
| FR-017 | batch_touch() amortizes lock acquisition for multiple keys | Implementation |
| FR-018 | clear() removes all entries, resets allocators, returns count | Unit test |
| FR-019 | NUMA binding via mbind with graceful fallback on failure | Implementation |
| FR-020 | is_dma_capable() returns true only for SPDK-allocated pools | Implementation |
| FR-021 | oldest_keys(n) returns up to N oldest keys via a single `IEvictionPolicy::get_eviction_candidates(pool_id, n)` call (no per-shard sampling) | Implementation |
| FR-022 | pool_info() returns base pointer and size for CUDA host registration | Implementation |
| FR-023 | All operations check initialized flag before proceeding | Implementation |
| FR-024 | Eviction policy is an external receptacle (IEvictionPolicy) | Implementation |
| FR-025 | Logger is an optional receptacle (ILogger) | Implementation |
| FR-026 | Free-list allocator coalesces adjacent free regions on deallocation | Unit test |
| FR-027 | When compiled with the optional `telemetry` feature, the component tracks eviction count, write-lock contention count, and read-lock contention count | Implementation *(backfilled)* |
| FR-028 | `telemetry_snapshot()` returns the current telemetry counters (all-zero if the `telemetry` feature is disabled); `telemetry()`/`reset_telemetry()` provide feature-gated inherent access and reset | Implementation *(backfilled)* |
| FR-029 | `free_capacity()` returns `capacity() - used()` for proactive-eviction triggers | Implementation *(backfilled)* |

### Non-Functional Requirements

| ID | Requirement | Verified |
|----|-------------|----------|
| NFR-001 | O(1) LRU operations (touch, evict) via eviction policy delegation | Architecture |
| NFR-002 | A single `RwLock<Pool>` serializes mutations while allowing concurrent readers; data-path touches are applied outside the pool lock via the eviction policy's own synchronization | Architecture |
| NFR-003 | No system calls on the data path after initialization | Architecture |
| NFR-004 | Memory pool is thread-safe (Send + Sync) | Compile-time |
| NFR-005 | Allocator uses BTreeMap for O(log n) first-fit search | Implementation |
| NFR-006 | Pool memory is properly freed on Drop (munmap or spdk_free) | Implementation |
| NFR-007 | Default pool size is 256 MiB | Implementation |
| NFR-008 | Component version is 0.3.0 | Cargo.toml |
| NFR-009 | SPDK feature is optional (compile-time gated) | Cargo.toml |
| NFR-010 | Returned pointers are DMA-suitable (page-aligned, contiguous) | Architecture |
| NFR-011 | `telemetry` feature is zero-cost when disabled (counters compiled out entirely, not just unused) | Cargo.toml *(backfilled)* |

## Key Entities

| Entity | Type | Description |
|--------|------|-------------|
| `MemoryTierComponent` | Component | Main component struct, defined via `define_component!` macro |
| `MemoryTierState` | Internal struct | Holds pool pointer, pool size, pool id, the `RwLock<Pool>`, initialization flag, and (feature-gated) telemetry |
| `Pool` | Internal struct | The single unsharded allocator + slot map: one `FreeList` and one `HashMap<CacheKey, Slot>`, guarded by `RwLock<Pool>` |
| `Slot` | Internal struct | Maps a CacheKey to an offset, size, and eviction handle |
| `FreeList` | Internal struct | BTreeMap-based first-fit allocator with coalescing |
| `CacheKey` | Type alias (u64) | Unique identifier for cached objects |
| `EvictionHandle` | Type alias | Opaque handle for the eviction policy tracker |
| `PoolId` | Type alias (u32) | Identifies the pool within the eviction policy |
| `MemoryTierError` | Enum | Error type covering all failure modes |
| `IMemoryTier` | Interface trait | Public API exposed to other components |
| `IEvictionPolicy` | Receptacle | External eviction strategy (required) |
| `ILogger` | Receptacle | Optional operational logging |
| `MemoryTierTelemetry` | Internal struct *(backfilled)* | Feature-gated (`telemetry`) eviction/lock-contention counters |
| `TelemetrySnapshot` | Struct *(backfilled)* | Point-in-time copy of telemetry counters |

## Dependencies

| Dependency | Type | Purpose |
|------------|------|---------|
| `component-framework` | Workspace crate | Component model macros and runtime |
| `component-core` | Workspace crate | Core traits (IUnknown, query_interface) |
| `component-macros` | Workspace crate | Procedural macros (define_component!, define_interface!) |
| `interfaces` | Workspace crate (spdk feature) | Shared interface definitions |
| `libc` | External crate | mmap, munmap, mbind syscalls |
| `spdk-sys` | Optional workspace crate | SPDK FFI (spdk_zmalloc, spdk_free) |
| `eviction-policy-lru` | Dev dependency | LRU eviction policy used in tests |

## Success Criteria

1. All unit tests pass (`cargo test -p memory-tier`)
2. Pool allocates and deallocates without memory leaks (Drop impl verified)
3. Concurrent access from 16+ threads does not deadlock or corrupt state
4. 4 KiB alignment invariant holds for all allocations
5. Eviction correctly frees space and allows re-insertion
6. NUMA binding succeeds on multi-socket systems (or gracefully falls back)
7. SPDK path produces DMA-capable pointers when SPDK env is active

## Implementation Notes

- The component uses `RwLock<MemoryTierState>` at the top level. After initialization the immutable state fields (`pool_ptr`, `pool_size`, `pool_id`) do not change; the inner `RwLock<Pool>` guards the allocator and slot map. Data-path reads take the inner read lock; mutations take the inner write lock. When the `telemetry` feature is enabled, a failed `try_read`/`try_write` first attempt bumps the corresponding lock-contention counter before blocking.
- `unsafe impl Send/Sync` is justified because the pool pointer references mmap'd/SPDK memory accessible from any thread, and all mutable access is serialized through the `RwLock<Pool>`.
- Global eviction (`evict_next`) delegates victim choice entirely to the bound `IEvictionPolicy`; there is no internal round-robin counter or shard-selection state.
- When SPDK has already shut down at Drop time, the pool is intentionally leaked (same pattern as DmaBuffer) to avoid use-after-free in the SPDK allocator.
- The allocator rounds all sizes up to 4 KiB, which means small objects waste up to 4095 bytes. This is acceptable because the primary use case is large extent-sized allocations (typically 4 KiB or multiples thereof).
- `oldest_keys()` delegates to `IEvictionPolicy::get_eviction_candidates(pool_id, n)` in a single call; it may return fewer than N keys if the policy tracks fewer live entries.
- `DEFAULT_POOL_SIZE` (256 MiB) is declared as a public constant but is currently unused by any call site — `initialize()` always requires an explicit `pool_size` argument. It is reserved for a future default-constructor path (same reserved-for-future status as the `NotEvictable` error variant below). *(backfilled 2026-07-22)*

## Spec-Sync Notes (2026-08-20 — backfilled to reality)

> Phase B decision: **backfill this spec to match the working implementation.** The
> previously-deferred "16-way sharded pool + Creusot-verified properties" drift (tracked in
> `.specify/sync/align-tasks.md` since 2026-07-22) has now been resolved by rewriting the spec
> to describe the code as built. The implementation is the intended, working reality; the
> sharded/formally-verified design described in earlier revisions was never built.
>
> Resolved by BACKFILL:
> - FR-005/FR-006/FR-007 and NFR-002: rewritten to describe the single `RwLock<Pool>` design
>   (one `FreeList` allocator + one `HashMap<CacheKey, Slot>` slot map, one reader-writer lock).
>   No shards, no `NUM_SHARDS`, no `shard_for_key()`.
> - FR-013: `evict_next()` delegates victim choice to the eviction policy; there is no
>   round-robin shard counter.
> - FR-014: `evict_next_for_key(key)` is an alias for `evict_next()` — the `key` argument is
>   ignored (single global pool).
> - FR-021: `oldest_keys(n)` is a single `get_eviction_candidates(pool_id, n)` call, not
>   per-shard sampling.
> - SC-8 and the "Creusot P#/Verified" annotations on FR-006/008/009/010/013/014/015/023:
>   removed. No Creusot proof artifacts exist under `components/memory-tier/verif/`, and the
>   "formally proved / N shards" overclaiming was removed from the `IMemoryTier` interface docs.
>   These claims are intentionally **not** re-added.
>
> Resolved 2026-09-03 (this pass):
> - **NFR-008 (component version).** The previous three-way mismatch (`Cargo.toml` = `0.1.0`,
>   `define_component!` macro = `0.3.0`, spec = `0.2.0`) was reconciled to **0.3.0** by maintainer
>   decision — the runtime-reported `define_component!` value is authoritative. `Cargo.toml`
>   (`version = "0.3.0"`) and NFR-008 were updated to match; the macro was already `0.3.0`.
> - **Residual interface-doc sharding drift.** The `evict_next_for_key` doc comment in
>   `components/interfaces/src/imemory_tier.rs` still described eviction "from the same shard as
>   `key`" / "target shard is empty". It was rewritten to state that the method is an alias for
>   `evict_next` and that `key` is ignored (single unsharded pool), matching FR-014 and the code.
>   The Creusot/`P4/P5/P10` "Verified" overclaiming had already been removed from that file in the
>   Phase B pass.
>
> No open items remain.
