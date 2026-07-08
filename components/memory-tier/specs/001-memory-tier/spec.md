# Feature Specification: Memory Tier (DRAM Cache Pool)

**Feature Branch**: `001-memory-tier`
**Created**: 2026-07-08
**Status**: Backfilled
**Source**: Generated from existing implementation

## Backfill Notice
> This spec was generated from existing code via `speckit.sync.backfill`.
> It documents current behavior, not original intent.
> Review carefully and update to reflect desired behavior.

## Overview

The `memory-tier` component provides a high-performance DRAM-resident cache pool for the Certus storage system. It serves as a fast staging area between higher-level dispatch logic and persistent NVMe storage, enabling low-latency reads for cached objects and write-staging for objects destined for disk.

The pool is internally sharded into 16 independent partitions to minimize lock contention under concurrent access from multiple dispatcher threads. Each shard maintains its own free-list allocator and slot map. Eviction decisions are delegated to an external `IEvictionPolicy` component via a receptacle binding, providing pluggable eviction strategies (currently LRU).

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
- `peek_does_not_update_lru` - verifies peek does not promote

### User Story 2 - LRU Eviction Under Memory Pressure (Priority: P1)

**As** the dispatch layer,
**I want** to evict the least-recently-used entry when the pool is full,
**so that** space is freed for new insertions without requiring external coordination.

**Acceptance Criteria:**
- `evict_lru()` removes the globally-oldest entry across shards (round-robin)
- `evict_lru_for_key(key)` evicts the oldest entry from the same shard as the target key
- Eviction correctly frees the allocation and removes the slot from the index
- After eviction, the freed space is available for new allocations
- `oldest_keys(n)` returns up to N keys in oldest-first order without removing them

**Test Coverage:**
- `evict_lru_returns_some` - verifies eviction returns a key
- `touch_updates_lru` - verifies touch promotes entry, oldest is evicted first
- `pool_full_returns_error` - verifies PoolFull when shard capacity exhausted

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
- `clear()` removes all entries from all shards and returns the count
- After clear, `used()` returns 0 and previous keys are no longer contained

**Test Coverage:**
- `remove_and_reuse` - verifies removal frees space for reuse
- `clear_resets_all` - verifies full cache clear

### User Story 5 - Capacity Monitoring (Priority: P2)

**As** an operator or monitoring subsystem,
**I want** to query the pool's total capacity and current usage,
**so that** I can track memory pressure and trigger proactive eviction.

**Acceptance Criteria:**
- `capacity()` returns the total pool size across all shards
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

## Requirements

### Functional Requirements

| ID | Requirement | Verified |
|----|-------------|----------|
| FR-001 | Pool is allocated as a single contiguous mmap'd region | Implementation |
| FR-002 | Hugepage backing (MAP_HUGETLB) is preferred with automatic fallback | Implementation |
| FR-003 | When SPDK env is active, pool uses spdk_zmalloc for DMA-capable memory | Implementation |
| FR-004 | All allocations are 4 KiB-aligned for NVMe DMA compatibility | Unit test |
| FR-005 | Pool is divided into 16 independent shards | Implementation |
| FR-006 | Shard selection uses key modulo 16 | Creusot P4, P5 |
| FR-007 | Each shard has its own Mutex-protected allocator and slot map | Implementation |
| FR-008 | insert() rejects zero size with InvalidSize | Creusot P1, unit test |
| FR-009 | insert() rejects duplicate keys with AlreadyExists | Creusot P3, unit test |
| FR-010 | insert() returns PoolFull when shard allocator cannot satisfy request | Creusot P8, unit test |
| FR-011 | get() returns pointer and size, and updates LRU via eviction policy | Unit test |
| FR-012 | peek() returns pointer and size without LRU update | Unit test |
| FR-013 | evict_lru() cycles through shards via atomic counter (round-robin) | Creusot P10 |
| FR-014 | evict_lru_for_key() evicts from the same shard as the target key | Creusot P4, P5 |
| FR-015 | remove() frees the slot and returns KeyNotFound for absent keys | Creusot P9, unit test |
| FR-016 | touch() promotes entry in LRU without returning data | Unit test |
| FR-017 | batch_touch() amortizes lock acquisition for multiple keys | Implementation |
| FR-018 | clear() removes all entries, resets allocators, returns count | Unit test |
| FR-019 | NUMA binding via mbind with graceful fallback on failure | Implementation |
| FR-020 | is_dma_capable() returns true only for SPDK-allocated pools | Implementation |
| FR-021 | oldest_keys(n) peeks at N oldest keys across shards | Implementation |
| FR-022 | pool_info() returns base pointer and size for CUDA host registration | Implementation |
| FR-023 | All operations check initialized flag before proceeding | Creusot P2 |
| FR-024 | Eviction policy is an external receptacle (IEvictionPolicy) | Implementation |
| FR-025 | Logger is an optional receptacle (ILogger) | Implementation |
| FR-026 | Free-list allocator coalesces adjacent free regions on deallocation | Unit test |

### Non-Functional Requirements

| ID | Requirement | Verified |
|----|-------------|----------|
| NFR-001 | O(1) LRU operations (touch, evict) via eviction policy delegation | Architecture |
| NFR-002 | Per-shard locking minimizes contention (16-way parallelism) | Architecture |
| NFR-003 | No system calls on the data path after initialization | Architecture |
| NFR-004 | Memory pool is thread-safe (Send + Sync) | Compile-time |
| NFR-005 | Allocator uses BTreeMap for O(log n) first-fit search | Implementation |
| NFR-006 | Pool memory is properly freed on Drop (munmap or spdk_free) | Implementation |
| NFR-007 | Default pool size is 256 MiB | Implementation |
| NFR-008 | Component version is 0.2.0 | Cargo.toml |
| NFR-009 | SPDK feature is optional (compile-time gated) | Cargo.toml |
| NFR-010 | Returned pointers are DMA-suitable (page-aligned, contiguous) | Architecture |

## Key Entities

| Entity | Type | Description |
|--------|------|-------------|
| `MemoryTierComponent` | Component | Main component struct, defined via `define_component!` macro |
| `MemoryTierState` | Internal struct | Holds pool pointer, shard vector, initialization flag |
| `Shard` | Internal struct | Per-partition allocator + slot map (16 total) |
| `Slot` | Internal struct | Maps a CacheKey to an offset, size, and eviction handle |
| `FreeList` | Internal struct | BTreeMap-based first-fit allocator with coalescing |
| `CacheKey` | Type alias (u64) | Unique identifier for cached objects |
| `EvictionHandle` | Type alias | Opaque handle for the eviction policy tracker |
| `PoolId` | Type alias (u32) | Identifies a shard within the eviction policy |
| `MemoryTierError` | Enum | Error type covering all failure modes |
| `IMemoryTier` | Interface trait | Public API exposed to other components |
| `IEvictionPolicy` | Receptacle | External eviction strategy (required) |
| `ILogger` | Receptacle | Optional operational logging |

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
8. 10 formal properties verified with Creusot (21 verification conditions)

## Implementation Notes

- The component uses `RwLock<MemoryTierState>` at the top level, but after initialization the state fields (`pool_ptr`, `pool_size`, `shard_size`) are effectively immutable. The RwLock is taken as a read-lock on all data-path operations, with per-shard Mutex providing fine-grained write access.
- `unsafe impl Send/Sync` is justified because the pool pointer references mmap'd/SPDK memory accessible from any thread, and all mutable access is serialized through Mutex.
- The `evict_counter` atomic provides round-robin shard selection for global eviction, preventing starvation of any single shard.
- When SPDK has already shut down at Drop time, the pool is intentionally leaked (same pattern as DmaBuffer) to avoid use-after-free in the SPDK allocator.
- The allocator rounds all sizes up to 4 KiB, which means small objects waste up to 4095 bytes. This is acceptable because the primary use case is large extent-sized allocations (typically 4 KiB or multiples thereof).
- `oldest_keys()` samples per-shard with `(n / NUM_SHARDS).max(1)` which may return fewer than N keys if shards are unevenly populated.
